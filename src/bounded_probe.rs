//! A slow platform provider must not own the input worker's lifetime.
use std::{
    sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
    thread,
    time::{Duration, Instant},
};

struct Request<K, V> {
    key: K,
    reply: SyncSender<V>,
}

struct Pending<K, V> {
    key: K,
    started: Instant,
    reply: Receiver<V>,
}

/// Transport failures are distinct from a provider's explicit negative answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    Timeout,
    Busy,
    Disconnected,
    Expired,
}

/// One provider thread, at most one queued request, no unbounded thread retries.
/// A hung provider may outlive this object; dropping it never joins that thread.
pub struct BoundedProbe<K, V> {
    sender: SyncSender<Request<K, V>>,
    pending: Option<Pending<K, V>>,
}

impl<K: Copy + Eq + Send + 'static, V: Send + 'static> BoundedProbe<K, V> {
    pub fn spawn<Factory, Provider>(name: &str, factory: Factory) -> std::io::Result<Self>
    where
        Factory: FnOnce() -> Provider + Send + 'static,
        Provider: FnMut(K) -> V + 'static,
    {
        let (sender, receiver) = sync_channel::<Request<K, V>>(1);
        thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                // COM objects, if any, are constructed and destroyed on this thread.
                let mut provider = factory();
                while let Ok(request) = receiver.recv() {
                    let result = provider(request.key);
                    let _ = request.reply.try_send(result);
                }
            })?;
        Ok(Self {
            sender,
            pending: None,
        })
    }

    /// Late replies are accepted only for the exact same context and within
    /// max_age. Timeout means unavailable, never an implicit permission grant.
    pub fn query(&mut self, key: K, wait: Duration, max_age: Duration) -> Option<V> {
        self.query_result(key, wait, max_age).ok()
    }

    pub fn query_result(
        &mut self,
        key: K,
        wait: Duration,
        max_age: Duration,
    ) -> Result<V, ProbeFailure> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.key != key || pending.started.elapsed() >= max_age)
        {
            self.pending = None;
        }
        if self.pending.is_none() {
            let (reply, receiver) = sync_channel(1);
            self.sender
                .try_send(Request { key, reply })
                .map_err(|error| match error {
                    TrySendError::Full(_) => ProbeFailure::Busy,
                    TrySendError::Disconnected(_) => ProbeFailure::Disconnected,
                })?;
            self.pending = Some(Pending {
                key,
                started: Instant::now(),
                reply: receiver,
            });
        }
        let pending = self.pending.as_ref().expect("request was submitted");
        let remaining = max_age.saturating_sub(pending.started.elapsed());
        let result = match pending.reply.recv_timeout(wait.min(remaining)) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => return Err(ProbeFailure::Timeout),
            Err(RecvTimeoutError::Disconnected) => {
                self.pending = None;
                return Err(ProbeFailure::Disconnected);
            }
        };
        let fresh = pending.started.elapsed() < max_age;
        self.pending = None;
        if fresh {
            Ok(result)
        } else {
            Err(ProbeFailure::Expired)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_provider_unavailable_is_not_a_transport_timeout() {
        let mut probe = BoundedProbe::spawn("probe-unavailable", || |_: u32| None::<bool>).unwrap();
        assert_eq!(
            probe.query_result(1, Duration::from_secs(1), Duration::from_secs(2)),
            Ok(None)
        );
    }

    #[test]
    fn timeout_keeps_one_request_and_accepts_its_late_reply() {
        let (release, blocked) = sync_channel(1);
        let mut probe = BoundedProbe::spawn("probe-late", move || {
            move |key: u32| {
                blocked.recv().unwrap();
                key
            }
        })
        .unwrap();
        assert_eq!(
            probe.query_result(1, Duration::ZERO, Duration::from_secs(2)),
            Err(ProbeFailure::Timeout)
        );
        release.send(()).unwrap();
        assert_eq!(
            probe.query_result(1, Duration::from_secs(1), Duration::from_secs(2)),
            Ok(1)
        );
    }

    #[test]
    fn delayed_reply_can_complete_within_a_larger_bounded_wait() {
        let mut probe = BoundedProbe::spawn("probe-delayed", || {
            |key: u32| {
                thread::sleep(Duration::from_millis(45));
                key
            }
        })
        .unwrap();
        // Use scheduling headroom in the test; production uses a 100 ms budget.
        assert_eq!(
            probe.query_result(1, Duration::from_secs(1), Duration::from_secs(2)),
            Ok(1)
        );
    }

    #[test]
    fn full_request_queue_reports_busy_without_starting_more_threads() {
        let (sender, _receiver) = sync_channel(1);
        let (reply, _reply_receiver) = sync_channel(1);
        sender
            .try_send(Request {
                key: 1u32,
                reply: reply as SyncSender<u32>,
            })
            .unwrap();
        let mut probe = BoundedProbe {
            sender,
            pending: None,
        };
        assert_eq!(
            probe.query_result(2, Duration::ZERO, Duration::from_secs(1)),
            Err(ProbeFailure::Busy)
        );
        assert!(probe.pending.is_none());
    }

    #[test]
    fn disconnected_reply_is_reported_and_pending_request_is_cleared() {
        let (sender, _receiver) = sync_channel::<Request<u32, u32>>(1);
        let (reply, receiver) = sync_channel(1);
        drop(reply);
        let mut probe = BoundedProbe {
            sender,
            pending: Some(Pending {
                key: 1,
                started: Instant::now(),
                reply: receiver,
            }),
        };
        assert_eq!(
            probe.query_result(1, Duration::ZERO, Duration::from_secs(1)),
            Err(ProbeFailure::Disconnected)
        );
        assert!(probe.pending.is_none());
    }

    #[test]
    fn blocked_provider_does_not_block_input_or_drop() {
        let (release, blocked) = sync_channel(1);
        let (entered, started) = sync_channel(1);
        let mut probe = BoundedProbe::spawn("probe-test", move || {
            move |key: u32| {
                let _ = entered.try_send(());
                let _ = blocked.recv();
                key
            }
        })
        .unwrap();
        assert_eq!(probe.query(1, Duration::ZERO, Duration::from_secs(1)), None);
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(probe.query(1, Duration::ZERO, Duration::from_secs(1)), None);
        // A control event can run here, while the provider is still blocked.
        drop(probe);
        release.send(()).unwrap();
    }

    #[test]
    fn reply_from_old_focus_never_authorizes_new_focus() {
        let (release, blocked) = sync_channel(1);
        let (entered, started) = sync_channel(1);
        let mut probe = BoundedProbe::spawn("probe-focus-test", move || {
            move |key: u32| {
                if key == 1 {
                    let _ = entered.send(());
                    let _ = blocked.recv();
                }
                key
            }
        })
        .unwrap();
        assert_eq!(probe.query(1, Duration::ZERO, Duration::from_secs(1)), None);
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        release.send(()).unwrap();
        assert_eq!(
            probe.query(2, Duration::from_secs(1), Duration::from_secs(2)),
            Some(2)
        );
    }

    #[test]
    fn expired_reply_is_not_reused() {
        let mut probe = BoundedProbe::spawn("probe-age-test", || |key: u32| key).unwrap();
        let (reply, receiver) = sync_channel(1);
        reply.send(99).unwrap();
        probe.pending = Some(Pending {
            key: 1,
            started: Instant::now() - Duration::from_secs(10),
            reply: receiver,
        });
        assert_eq!(
            probe.query(1, Duration::from_secs(1), Duration::from_secs(2)),
            Some(1)
        );
    }
}
