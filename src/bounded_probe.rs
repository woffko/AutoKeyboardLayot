//! A slow platform provider must not own the input worker's lifetime.
use std::{
    io,
    sync::{
        Arc,
        mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
    },
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

/// Starts a provider thread and returns the sender of its one-slot request queue.
fn start_provider<K, V, Factory, Provider>(
    name: &str,
    factory: Factory,
) -> io::Result<SyncSender<Request<K, V>>>
where
    K: Send + 'static,
    V: Send + 'static,
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
    Ok(sender)
}

/// Starts a fresh provider thread with its own request queue.
type Respawn<K, V> = Box<dyn Fn() -> io::Result<SyncSender<Request<K, V>>> + Send>;

/// When to replace a provider that stopped answering.
struct Healing<K, V> {
    respawn: Respawn<K, V>,
    /// How long requests may keep finding the provider busy before it counts as stuck.
    stuck_after: Duration,
    remaining: usize,
    /// Since when the provider has been seen busy without answering; `None` while it answers.
    busy_since: Option<Instant>,
}

/// One provider thread, at most one queued request, no unbounded thread retries.
/// A hung provider may outlive this object; dropping it never joins that thread.
pub struct BoundedProbe<K, V> {
    sender: SyncSender<Request<K, V>>,
    pending: Option<Pending<K, V>>,
    healing: Option<Healing<K, V>>,
    respawns: u32,
}

impl<K: Copy + Eq + Send + 'static, V: Send + 'static> BoundedProbe<K, V> {
    pub fn spawn<Factory, Provider>(name: &str, factory: Factory) -> io::Result<Self>
    where
        Factory: FnOnce() -> Provider + Send + 'static,
        Provider: FnMut(K) -> V + 'static,
    {
        Ok(Self {
            sender: start_provider(name, factory)?,
            pending: None,
            healing: None,
            respawns: 0,
        })
    }

    /// Like [`spawn`](Self::spawn), but a provider that keeps the queue busy for `stuck_after`
    /// without answering (a hung COM call, say) is abandoned and replaced by a fresh one built
    /// by `factory` on a new thread, at most `max_respawns` times. The hung thread is never
    /// joined or killed; it ends on its own if it ever wakes up.
    pub fn spawn_resilient<Factory, Provider>(
        name: &str,
        factory: Factory,
        stuck_after: Duration,
        max_respawns: usize,
    ) -> io::Result<Self>
    where
        Factory: Fn() -> Provider + Send + Sync + 'static,
        Provider: FnMut(K) -> V + 'static,
    {
        let factory = Arc::new(factory);
        let name = name.to_owned();
        let start = move |factory: &Arc<Factory>, name: &str| {
            let factory = Arc::clone(factory);
            start_provider(name, move || factory())
        };
        let sender = start(&factory, &name)?;
        let respawn: Respawn<K, V> = Box::new(move || start(&factory, &name));
        Ok(Self {
            sender,
            pending: None,
            healing: Some(Healing {
                respawn,
                stuck_after,
                remaining: max_respawns,
                busy_since: None,
            }),
            respawns: 0,
        })
    }

    /// How many times a stuck provider has been replaced.
    pub fn respawn_count(&self) -> u32 {
        self.respawns
    }

    /// Replaces the provider when the queue has been busy for `stuck_after` and respawns remain.
    fn replace_if_stuck(&mut self) -> bool {
        let Some(healing) = self.healing.as_mut() else {
            return false;
        };
        let since = *healing.busy_since.get_or_insert_with(Instant::now);
        if healing.remaining == 0 || since.elapsed() < healing.stuck_after {
            return false;
        }
        let Ok(sender) = (healing.respawn)() else {
            return false;
        };
        healing.remaining -= 1;
        healing.busy_since = None;
        self.sender = sender;
        self.pending = None;
        self.respawns += 1;
        true
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
            let request = Request { key, reply };
            let request = match self.sender.try_send(request) {
                Ok(()) => None,
                Err(TrySendError::Full(request)) => Some(request),
                Err(TrySendError::Disconnected(_)) => return Err(ProbeFailure::Disconnected),
            };
            if let Some(request) = request {
                // The provider has not taken the previous request: busy, or stuck for good.
                if !self.replace_if_stuck() {
                    return Err(ProbeFailure::Busy);
                }
                self.sender.try_send(request).map_err(|error| match error {
                    TrySendError::Full(_) => ProbeFailure::Busy,
                    TrySendError::Disconnected(_) => ProbeFailure::Disconnected,
                })?;
            }
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
        // An answer, however late, shows that the provider is alive.
        if let Some(healing) = self.healing.as_mut() {
            healing.busy_since = None;
        }
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
            healing: None,
            respawns: 0,
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
            healing: None,
            respawns: 0,
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

    /// A provider factory whose first provider hangs until `release` is set and whose later
    /// providers answer with the key. `started` counts how many providers were built.
    fn hanging_then_healthy(
        release: Arc<AtomicBool>,
        started: Arc<AtomicUsize>,
        hang_always: bool,
    ) -> impl Fn() -> Box<dyn FnMut(u32) -> u32> + Send + Sync + 'static {
        move || {
            let index = started.fetch_add(1, Ordering::SeqCst);
            let release = Arc::clone(&release);
            Box::new(move |key: u32| {
                if index == 0 || hang_always {
                    while !release.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                key
            })
        }
    }

    /// Queries until the probe answers or `limit` passes; returns the answer if any.
    fn query_until_answered(
        probe: &mut BoundedProbe<u32, u32>,
        key: u32,
        limit: Duration,
    ) -> Option<u32> {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Ok(value) =
                probe.query_result(key, Duration::from_millis(5), Duration::from_millis(20))
            {
                return Some(value);
            }
            thread::sleep(Duration::from_millis(2));
        }
        None
    }

    #[test]
    fn a_provider_that_hangs_on_its_first_call_is_replaced_after_stuck_after() {
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicUsize::new(0));
        let factory = hanging_then_healthy(Arc::clone(&release), Arc::clone(&started), false);
        let mut probe = BoundedProbe::<u32, u32>::spawn_resilient(
            "probe-heal",
            move || {
                let mut provider = factory();
                move |key| provider(key)
            },
            Duration::from_millis(80),
            3,
        )
        .unwrap();
        // Before the stuck threshold nothing is replaced and nothing answers.
        assert_eq!(
            probe.query_result(1, Duration::from_millis(5), Duration::from_millis(20)),
            Err(ProbeFailure::Timeout)
        );
        assert_eq!(probe.respawn_count(), 0);
        let answer = query_until_answered(&mut probe, 7, Duration::from_secs(5));
        assert_eq!(answer, Some(7), "the replacement provider must answer");
        assert_eq!(probe.respawn_count(), 1);
        assert_eq!(
            started.load(Ordering::SeqCst),
            2,
            "exactly one replacement was built"
        );
        release.store(true, Ordering::SeqCst);
    }

    #[test]
    fn replacements_are_capped_and_the_probe_then_stays_busy() {
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicUsize::new(0));
        let factory = hanging_then_healthy(Arc::clone(&release), Arc::clone(&started), true);
        let mut probe = BoundedProbe::<u32, u32>::spawn_resilient(
            "probe-cap",
            move || {
                let mut provider = factory();
                move |key| provider(key)
            },
            Duration::from_millis(30),
            2,
        )
        .unwrap();
        let answer = query_until_answered(&mut probe, 1, Duration::from_millis(700));
        assert_eq!(answer, None, "every provider hangs");
        assert_eq!(probe.respawn_count(), 2);
        assert_eq!(
            started.load(Ordering::SeqCst),
            3,
            "the original plus two replacements"
        );
        release.store(true, Ordering::SeqCst);
    }

    #[test]
    fn a_slow_but_alive_provider_is_never_replaced() {
        let started = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&started);
        let mut probe = BoundedProbe::<u32, u32>::spawn_resilient(
            "probe-slow",
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                |key: u32| {
                    thread::sleep(Duration::from_millis(25));
                    key
                }
            },
            Duration::from_millis(150),
            3,
        )
        .unwrap();
        let until = Instant::now() + Duration::from_millis(600);
        let mut answers = 0;
        while Instant::now() < until {
            if probe
                .query_result(1, Duration::from_millis(40), Duration::from_millis(200))
                .is_ok()
            {
                answers += 1;
            }
        }
        assert!(answers > 5, "a slow provider still answers ({answers})");
        assert_eq!(probe.respawn_count(), 0);
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_abandoned_provider_thread_ends_when_it_wakes_up() {
        struct OnDrop(Arc<AtomicBool>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let release = Arc::new(AtomicBool::new(false));
        let first_gone = Arc::new(AtomicBool::new(false));
        let built = Arc::new(AtomicUsize::new(0));
        let (release_in, gone_in, built_in) = (
            Arc::clone(&release),
            Arc::clone(&first_gone),
            Arc::clone(&built),
        );
        let mut probe = BoundedProbe::<u32, u32>::spawn_resilient(
            "probe-exit",
            move || {
                let first = built_in.fetch_add(1, Ordering::SeqCst) == 0;
                let guard = first.then(|| OnDrop(Arc::clone(&gone_in)));
                let release = Arc::clone(&release_in);
                move |key: u32| {
                    let _keep = &guard;
                    if first {
                        while !release.load(Ordering::SeqCst) {
                            thread::sleep(Duration::from_millis(1));
                        }
                    }
                    key
                }
            },
            Duration::from_millis(40),
            1,
        )
        .unwrap();
        assert_eq!(
            query_until_answered(&mut probe, 3, Duration::from_secs(5)),
            Some(3)
        );
        assert!(
            !first_gone.load(Ordering::SeqCst),
            "the old thread is still stuck"
        );
        release.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !first_gone.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            first_gone.load(Ordering::SeqCst),
            "the abandoned thread must exit"
        );
    }
}
