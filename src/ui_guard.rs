//! Rules that keep the UI thread, which owns the low-level hooks, healthy.
//!
//! Windows silently removes a low-level hook whose thread is too slow, and some session changes
//! leave a hook dead without any notice. The agent compares user input with hook callbacks to
//! notice that, and limits the work that can block the thread. The decisions are pure so that
//! every platform tests them; the Windows adapter supplies the facts.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Input newer than this counts as "the user is typing right now".
pub const INPUT_RECENT: Duration = Duration::from_millis(400);
/// No hook callback for longer than this while the user types means that the hooks are gone.
pub const HOOK_SILENT: Duration = Duration::from_secs(3);
/// Shortest time between two reinstallations triggered by the watchdog.
pub const REINSTALL_MIN_INTERVAL: Duration = Duration::from_secs(15);
/// Reinstallations the watchdog may trigger per hour before it reports instability instead.
pub const REINSTALL_PER_HOUR: usize = 4;
const HOUR: Duration = Duration::from_secs(60 * 60);

/// What is known about the foreground window's privilege compared with the agent's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForegroundPrivilege {
    /// No foreground window: lock screen, secure desktop, or between windows.
    None,
    /// Same or lower integrity level: its input must reach the hooks.
    NotHigher,
    /// Higher integrity level: Windows may legitimately not deliver its input to the hooks.
    Higher,
    /// The level could not be read.
    Unknown,
}

/// Whether the hooks look silent while the user types. The Windows adapter checks this cheap
/// condition first and gathers the other facts only when it holds.
pub fn hooks_look_silent(input_idle: Duration, hook_silent: Duration) -> bool {
    input_idle <= INPUT_RECENT && hook_silent > HOOK_SILENT
}

impl ForegroundPrivilege {
    /// Compares integrity levels: `theirs` is the foreground process, `ours` the agent's. A level
    /// that could not be read on either side is `Unknown`, which never triggers a reinstall.
    pub fn compare(theirs: Option<u32>, ours: Option<u32>) -> Self {
        match (theirs, ours) {
            (Some(theirs), Some(ours)) if theirs > ours => Self::Higher,
            (Some(_), Some(_)) => Self::NotHigher,
            _ => Self::Unknown,
        }
    }
}

/// The facts the watchdog decides on.
#[derive(Debug, Clone, Copy)]
pub struct WatchdogFacts {
    /// Time since the last user input of the session.
    pub input_idle: Duration,
    /// Time since the last hook callback or hook installation.
    pub hook_silent: Duration,
    pub foreground: ForegroundPrivilege,
    /// False on the lock screen and the secure desktop, where typing never reaches the hooks.
    pub input_desktop_is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogAction {
    Nothing,
    /// Unhook and hook again.
    Reinstall,
    /// Reinstalling did not help often enough: tell the user instead of trying again.
    WarnUnstable,
}

/// Decides when silent hooks are reinstalled, at most once per 15 seconds and four times an hour.
#[derive(Default)]
pub struct HookWatchdog {
    reinstalls: VecDeque<Instant>,
    warned_at: Option<Instant>,
}

impl HookWatchdog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every condition must hold for the hooks to count as silent: the user is typing now, no hook
    /// callback ran for a while, the input desktop is the default one, and the foreground window
    /// is one whose input must reach the hooks. An unknown privilege never triggers.
    pub fn decide(&mut self, facts: &WatchdogFacts, now: Instant) -> WatchdogAction {
        let silent = hooks_look_silent(facts.input_idle, facts.hook_silent)
            && facts.input_desktop_is_default
            && facts.foreground == ForegroundPrivilege::NotHigher;
        if !silent {
            return WatchdogAction::Nothing;
        }
        while self
            .reinstalls
            .front()
            .is_some_and(|&at| now.saturating_duration_since(at) >= HOUR)
        {
            self.reinstalls.pop_front();
        }
        if self
            .reinstalls
            .back()
            .is_some_and(|&at| now.saturating_duration_since(at) < REINSTALL_MIN_INTERVAL)
        {
            return WatchdogAction::Nothing;
        }
        if self.reinstalls.len() < REINSTALL_PER_HOUR {
            self.reinstalls.push_back(now);
            return WatchdogAction::Reinstall;
        }
        if self
            .warned_at
            .is_none_or(|at| now.saturating_duration_since(at) >= HOUR)
        {
            self.warned_at = Some(now);
            WatchdogAction::WarnUnstable
        } else {
            WatchdogAction::Nothing
        }
    }
}

/// At most two tray updates per second and none while a correction gate holds keys. A shell call
/// can block for a long time when Explorer is unresponsive, and the UI thread must stay available
/// to the hooks. A refused update is not lost: the caller keeps its status comparison and asks
/// again on a later timer tick.
#[derive(Default)]
pub struct TrayThrottle {
    next_allowed: Option<Instant>,
}

impl TrayThrottle {
    pub const MIN_INTERVAL: Duration = Duration::from_millis(500);

    pub fn new() -> Self {
        Self::default()
    }

    /// True when an update may run now; that also starts the next waiting period.
    pub fn allow(&mut self, now: Instant, gate_active: bool) -> bool {
        if gate_active || self.next_allowed.is_some_and(|at| now < at) {
            return false;
        }
        self.next_allowed = Some(now + Self::MIN_INTERVAL);
        true
    }
}

/// Waiting time before the next attempt to install missing hooks or the tray icon after
/// `failures` failed attempts: 1 s, doubling, at most 30 s.
pub fn retry_delay(failures: u32) -> Duration {
    let seconds = 1u64 << failures.saturating_sub(1).min(5);
    Duration::from_secs(seconds.min(30))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typing_with_silent_hooks() -> WatchdogFacts {
        WatchdogFacts {
            input_idle: Duration::from_millis(50),
            hook_silent: Duration::from_secs(10),
            foreground: ForegroundPrivilege::NotHigher,
            input_desktop_is_default: true,
        }
    }

    #[test]
    fn silent_hooks_while_typing_are_reinstalled() {
        let mut watchdog = HookWatchdog::new();
        assert_eq!(
            watchdog.decide(&typing_with_silent_hooks(), Instant::now()),
            WatchdogAction::Reinstall
        );
    }

    #[test]
    fn nothing_happens_while_idle_or_while_the_hooks_are_alive() {
        let now = Instant::now();
        let mut watchdog = HookWatchdog::new();
        let idle = WatchdogFacts {
            input_idle: Duration::from_secs(60),
            ..typing_with_silent_hooks()
        };
        assert_eq!(watchdog.decide(&idle, now), WatchdogAction::Nothing);
        let alive = WatchdogFacts {
            hook_silent: Duration::from_millis(100),
            ..typing_with_silent_hooks()
        };
        assert_eq!(watchdog.decide(&alive, now), WatchdogAction::Nothing);
    }

    #[test]
    fn the_thresholds_are_exact() {
        let now = Instant::now();
        let facts = |input_idle, hook_silent| WatchdogFacts {
            input_idle,
            hook_silent,
            ..typing_with_silent_hooks()
        };
        let mut watchdog = HookWatchdog::new();
        assert_eq!(
            watchdog.decide(
                &facts(Duration::from_millis(401), Duration::from_secs(9)),
                now
            ),
            WatchdogAction::Nothing,
            "input older than 400 ms"
        );
        assert_eq!(
            watchdog.decide(&facts(INPUT_RECENT, HOOK_SILENT), now),
            WatchdogAction::Nothing,
            "silent for exactly 3 s is not more than 3 s"
        );
        assert_eq!(
            watchdog.decide(
                &facts(INPUT_RECENT, HOOK_SILENT + Duration::from_millis(1)),
                now
            ),
            WatchdogAction::Reinstall
        );
    }

    #[test]
    fn a_foreground_that_may_hide_its_input_or_another_desktop_never_triggers() {
        let now = Instant::now();
        for foreground in [
            ForegroundPrivilege::None,
            ForegroundPrivilege::Higher,
            ForegroundPrivilege::Unknown,
        ] {
            let mut watchdog = HookWatchdog::new();
            let facts = WatchdogFacts {
                foreground,
                ..typing_with_silent_hooks()
            };
            assert_eq!(
                watchdog.decide(&facts, now),
                WatchdogAction::Nothing,
                "{foreground:?}"
            );
        }
        let mut watchdog = HookWatchdog::new();
        let secure = WatchdogFacts {
            input_desktop_is_default: false,
            ..typing_with_silent_hooks()
        };
        assert_eq!(watchdog.decide(&secure, now), WatchdogAction::Nothing);
    }

    #[test]
    fn reinstalling_is_limited_to_once_per_fifteen_seconds_and_four_per_hour() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut watchdog = HookWatchdog::new();
        let facts = typing_with_silent_hooks();
        assert_eq!(watchdog.decide(&facts, at(0)), WatchdogAction::Reinstall);
        assert_eq!(
            watchdog.decide(&facts, at(14)),
            WatchdogAction::Nothing,
            "within 15 s"
        );
        assert_eq!(watchdog.decide(&facts, at(15)), WatchdogAction::Reinstall);
        assert_eq!(watchdog.decide(&facts, at(40)), WatchdogAction::Reinstall);
        assert_eq!(watchdog.decide(&facts, at(60)), WatchdogAction::Reinstall);
        // The fifth trigger within the hour is reported once instead of reinstalling again.
        assert_eq!(
            watchdog.decide(&facts, at(90)),
            WatchdogAction::WarnUnstable
        );
        assert_eq!(watchdog.decide(&facts, at(120)), WatchdogAction::Nothing);
        assert_eq!(watchdog.decide(&facts, at(3000)), WatchdogAction::Nothing);
        // An hour after the first reinstall there is room again, and the warning may repeat later.
        assert_eq!(watchdog.decide(&facts, at(3600)), WatchdogAction::Reinstall);
    }

    #[test]
    fn tray_updates_are_limited_to_two_per_second_and_wait_for_a_gate() {
        let start = Instant::now();
        let after = |ms: u64| start + Duration::from_millis(ms);
        let mut throttle = TrayThrottle::new();
        assert!(throttle.allow(start, false));
        assert!(!throttle.allow(after(100), false));
        assert!(!throttle.allow(after(499), false));
        // A gate blocks the update without starting a waiting period.
        assert!(!throttle.allow(after(500), true));
        assert!(throttle.allow(after(500), false));
        assert!(!throttle.allow(after(600), false));
        assert!(throttle.allow(after(1000), false));
    }

    #[test]
    fn integrity_levels_are_compared_and_unknown_levels_stay_unknown() {
        use ForegroundPrivilege::{Higher, NotHigher, Unknown};
        const MEDIUM: u32 = 0x2000;
        const HIGH: u32 = 0x3000;
        assert_eq!(
            ForegroundPrivilege::compare(Some(MEDIUM), Some(MEDIUM)),
            NotHigher
        );
        assert_eq!(
            ForegroundPrivilege::compare(Some(0x1000), Some(MEDIUM)),
            NotHigher
        );
        assert_eq!(
            ForegroundPrivilege::compare(Some(HIGH), Some(MEDIUM)),
            Higher
        );
        assert_eq!(ForegroundPrivilege::compare(None, Some(MEDIUM)), Unknown);
        assert_eq!(ForegroundPrivilege::compare(Some(MEDIUM), None), Unknown);
        assert_eq!(ForegroundPrivilege::compare(None, None), Unknown);
    }

    #[test]
    fn retries_back_off_from_one_second_to_thirty() {
        let seconds: Vec<u64> = (1..=8).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(seconds, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(retry_delay(0).as_secs(), 1);
        assert_eq!(retry_delay(u32::MAX).as_secs(), 30);
    }
}
