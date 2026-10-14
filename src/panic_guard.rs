//! Containing panics where unwinding must not escape or must not wedge the application.
//!
//! The Windows agent runs its input worker on a thread and lets the operating system call into it
//! through `extern "system"` callbacks. A panic must neither leave the application believing that
//! the worker is still busy (which would make Exit impossible) nor unwind out of a callback (which
//! aborts the process). These helpers are portable so that every platform tests them.

use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    time::{Duration, Instant},
};

/// Marks a worker busy for the lifetime of the guard. The flag is cleared on drop, including when
/// a panic unwinds through the guard, so a crashed worker cannot leave a stale "busy".
pub struct BusyGuard<'a>(&'a AtomicBool);

impl<'a> BusyGuard<'a> {
    pub fn new(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::Release);
        Self(flag)
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Runs `body`. A panic is caught and `on_panic` produces the result instead. Use it at
/// `extern "system"` boundaries, where unwinding would abort the process.
pub fn guarded<T>(body: impl FnOnce() -> T, on_panic: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|_| on_panic())
}

/// Recent faults, to tell a one-off bug from a loop.
pub struct FaultWindow {
    limit: usize,
    window: Duration,
    times: VecDeque<Instant>,
}

impl FaultWindow {
    pub fn new(limit: usize, window: Duration) -> Self {
        Self {
            limit,
            window,
            times: VecDeque::new(),
        }
    }

    /// Records a fault at `now`. True when `limit` faults fall within the window.
    pub fn record(&mut self, now: Instant) -> bool {
        while self
            .times
            .front()
            .is_some_and(|&earlier| now.saturating_duration_since(earlier) > self.window)
        {
            self.times.pop_front();
        }
        self.times.push_back(now);
        self.times.len() >= self.limit
    }
}

/// Why [`run_contained`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerExit {
    /// Every sender is gone.
    Closed,
    /// The stop flag was set.
    Stopped,
    /// Too many panics in a short time: the loop gave up instead of spinning on a bug.
    TooManyFaults,
}

/// Hands every received event to `process` until the channel closes or `stop` is set. A panic in
/// `process` is contained: the busy flag is cleared, `on_fault` runs (it may repair `state`), and
/// the loop continues with the next event, unless `faults` reports a run of panics.
pub fn run_contained<S, E>(
    state: &mut S,
    receiver: &Receiver<E>,
    busy: &AtomicBool,
    stop: &AtomicBool,
    faults: &mut FaultWindow,
    process: impl Fn(&mut S, E),
    on_fault: impl Fn(&mut S),
) -> WorkerExit {
    while let Ok(event) = receiver.recv() {
        if stop.load(Ordering::Acquire) {
            return WorkerExit::Stopped;
        }
        let outcome = {
            let _busy = BusyGuard::new(busy);
            catch_unwind(AssertUnwindSafe(|| process(state, event)))
        };
        if outcome.is_err() {
            // The repair must not be able to take the loop down with it.
            let _ = catch_unwind(AssertUnwindSafe(|| on_fault(state)));
            if faults.record(Instant::now()) {
                return WorkerExit::TooManyFaults;
            }
        }
    }
    WorkerExit::Closed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn busy_guard_clears_the_flag_on_drop_and_when_a_panic_unwinds_through_it() {
        let busy = AtomicBool::new(false);
        {
            let _guard = BusyGuard::new(&busy);
            assert!(busy.load(Ordering::Acquire));
        }
        assert!(!busy.load(Ordering::Acquire));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = BusyGuard::new(&busy);
            assert!(busy.load(Ordering::Acquire));
            panic!("worker fault");
        }));
        assert!(result.is_err());
        assert!(!busy.load(Ordering::Acquire), "a panic left the flag set");
    }

    #[test]
    fn guarded_passes_a_result_through_and_reports_a_panic_through_on_panic() {
        assert_eq!(guarded(|| 7, || -1), 7);
        let mut reported = false;
        let value = guarded(
            || -> i32 { panic!("callback fault") },
            || {
                reported = true;
                -1
            },
        );
        assert_eq!(value, -1);
        assert!(reported);
    }

    #[test]
    fn fault_window_trips_on_a_run_of_faults_but_not_on_spaced_ones() {
        let start = Instant::now();
        let second = |n: u64| start + Duration::from_secs(n);
        let mut faults = FaultWindow::new(3, Duration::from_secs(60));
        assert!(!faults.record(second(0)));
        assert!(!faults.record(second(10)));
        assert!(faults.record(second(20)));
        let mut spaced = FaultWindow::new(3, Duration::from_secs(60));
        for n in 0..10 {
            assert!(!spaced.record(second(n * 61)), "fault {n}");
        }
    }

    #[test]
    fn a_panic_is_contained_and_later_events_are_still_processed() {
        let (sender, receiver) = channel();
        for value in [1, 2, 3, 4] {
            sender.send(value).unwrap();
        }
        drop(sender);
        let busy = AtomicBool::new(false);
        let stop = AtomicBool::new(false);
        let mut faults = FaultWindow::new(3, Duration::from_secs(60));
        let mut state = (Vec::new(), 0u32);
        let exit = run_contained(
            &mut state,
            &receiver,
            &busy,
            &stop,
            &mut faults,
            |state, value| {
                assert_ne!(value, 2, "fault on the second event");
                state.0.push(value);
            },
            |state| state.1 += 1,
        );
        assert_eq!(exit, WorkerExit::Closed);
        assert_eq!(state, (vec![1, 3, 4], 1));
        assert!(!busy.load(Ordering::Acquire));
    }

    #[test]
    fn three_panics_in_a_row_stop_the_loop_without_leaving_it_busy() {
        let (sender, receiver) = channel();
        for value in 0..6 {
            sender.send(value).unwrap();
        }
        let busy = AtomicBool::new(false);
        let stop = AtomicBool::new(false);
        let mut faults = FaultWindow::new(3, Duration::from_secs(60));
        let mut handled = 0u32;
        let exit = run_contained(
            &mut handled,
            &receiver,
            &busy,
            &stop,
            &mut faults,
            |handled, _| {
                *handled += 1;
                panic!("always fails");
            },
            |_| {},
        );
        assert_eq!(exit, WorkerExit::TooManyFaults);
        assert_eq!(handled, 3, "the loop must give up at the third fault");
        assert!(!busy.load(Ordering::Acquire));
        assert_eq!(
            receiver.try_iter().count(),
            3,
            "unprocessed events remain queued"
        );
    }

    #[test]
    fn a_failing_repair_does_not_take_the_loop_down() {
        let (sender, receiver) = channel();
        sender.send(()).unwrap();
        drop(sender);
        let busy = AtomicBool::new(false);
        let stop = AtomicBool::new(false);
        let mut faults = FaultWindow::new(3, Duration::from_secs(60));
        let exit = run_contained(
            &mut (),
            &receiver,
            &busy,
            &stop,
            &mut faults,
            |_, _| panic!("fault"),
            |_| panic!("repair fault"),
        );
        assert_eq!(exit, WorkerExit::Closed);
    }

    #[test]
    fn the_stop_flag_ends_the_loop_before_the_next_event_is_processed() {
        let (sender, receiver) = channel();
        sender.send(1).unwrap();
        let busy = AtomicBool::new(false);
        let stop = AtomicBool::new(true);
        let mut faults = FaultWindow::new(3, Duration::from_secs(60));
        let mut seen = 0;
        let exit = run_contained(
            &mut seen,
            &receiver,
            &busy,
            &stop,
            &mut faults,
            |seen, _| *seen += 1,
            |_| {},
        );
        assert_eq!(exit, WorkerExit::Stopped);
        assert_eq!(seen, 0);
    }
}
