//! Rules that keep the UI thread, which owns the low-level hooks, healthy.
//!
//! Windows silently removes a low-level hook whose thread is too slow, and some session changes
//! leave a hook dead without any notice. The agent compares user input with each hook's callbacks
//! to notice that, and limits the work that can block the thread. The decisions are pure so that
//! every platform tests them; the Windows adapter supplies the facts.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Input newer than this counts as "the user is giving input right now".
pub const INPUT_RECENT: Duration = Duration::from_millis(400);
/// No callback of a hook for longer than this while the user gives it input means that it is gone.
pub const HOOK_SILENT: Duration = Duration::from_secs(3);
/// Shortest time between two reinstallations triggered by the watchdog.
pub const REINSTALL_MIN_INTERVAL: Duration = Duration::from_secs(15);
/// Reinstallations the watchdog may trigger per hour before it reports instability instead.
pub const REINSTALL_PER_HOUR: usize = 4;
const HOUR: Duration = Duration::from_secs(60 * 60);
/// Programs that take the keyboard for a guest or remote session with a low-level hook of their
/// own, ahead of the agent's: the VMware, VirtualBox and Hyper-V consoles and the Remote Desktop
/// clients.
const KEYBOARD_CAPTURING_PROGRAMS: [&str; 6] = [
    "vmware.exe",
    "vmplayer.exe",
    "virtualboxvm.exe",
    "vmconnect.exe",
    "mstsc.exe",
    "msrdc.exe",
];

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

/// A hook that missed input it must have seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilentHook {
    /// Input came while the pointer stood still, and neither hook saw any: typing, or a click or
    /// a scroll that the mouse hook missed as well.
    Keyboard,
    /// The pointer kept moving, and the mouse hook saw none of it.
    Mouse,
}

/// Each hook's time since its last callback or its installation.
#[derive(Debug, Clone, Copy)]
pub struct HookSilence {
    pub keyboard: Duration,
    pub mouse: Duration,
}

/// Which hook looks dead while the user gives input, if one does. Each hook is judged by input
/// that it must see, so one hook's callbacks never hide the other's silence: the mouse hook by a
/// moving pointer, the keyboard hook by input without pointer movement that the mouse hook did
/// not see either (a click or a scroll it saw is no typing). The Windows adapter checks this
/// cheap condition first and gathers the other facts only when it holds.
pub fn silent_hook(
    input_idle: Duration,
    silence: HookSilence,
    pointer_moving: bool,
) -> Option<SilentHook> {
    if input_idle > INPUT_RECENT {
        return None;
    }
    if pointer_moving {
        return (silence.mouse > HOOK_SILENT).then_some(SilentHook::Mouse);
    }
    (silence.keyboard > HOOK_SILENT && silence.mouse > HOOK_SILENT).then_some(SilentHook::Keyboard)
}

/// Follows the pointer from one timer tick to the next. It counts as moving after moves on two
/// ticks in a row: one jump, such as a program placing the pointer, is no movement by the user.
#[derive(Debug, Default)]
pub struct PointerMotion {
    last: Option<(i32, i32)>,
    moves: u32,
}

impl PointerMotion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the pointer's position at this tick (`None`: unknown) and tells whether it moves.
    pub fn observe(&mut self, position: Option<(i32, i32)>) -> bool {
        let moved = position.is_some() && self.last.is_some() && position != self.last;
        self.last = position;
        self.moves = if moved {
            self.moves.saturating_add(1)
        } else {
            0
        };
        self.moves >= 2
    }
}

/// Whether the program at `image_path` (a full path or a file name) takes the keyboard itself.
pub fn captures_keyboard(image_path: &str) -> bool {
    let name = image_path.rsplit(['/', '\\']).next().unwrap_or(image_path);
    KEYBOARD_CAPTURING_PROGRAMS
        .iter()
        .any(|program| name.eq_ignore_ascii_case(program))
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
    /// Each hook's time since its last callback or installation.
    pub silence: HookSilence,
    /// The pointer kept moving over the last timer ticks (`PointerMotion`).
    pub pointer_moving: bool,
    pub foreground: ForegroundPrivilege,
    /// False on the lock screen and the secure desktop, where typing never reaches the hooks.
    pub input_desktop_is_default: bool,
    /// A full-screen program (most games), a virtual machine console or a Remote Desktop window
    /// is in the foreground. These often take the keyboard with a hook of their own before the
    /// agent's hooks see it, so silence proves nothing there, and reinstalling would only fight
    /// them. The hooks are judged again once another window is in the foreground.
    pub foreground_captures_input: bool,
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

    /// Every condition must hold for the hooks to count as silent: the user gives input now that a
    /// hook missed for a while (`silent_hook`), the input desktop is the default one, and the
    /// foreground window is one whose input must reach the hooks: not a higher integrity level,
    /// and not a program that takes the keyboard itself. An unknown privilege never triggers.
    pub fn decide(&mut self, facts: &WatchdogFacts, now: Instant) -> WatchdogAction {
        let silent = silent_hook(facts.input_idle, facts.silence, facts.pointer_moving).is_some()
            && facts.input_desktop_is_default
            && facts.foreground == ForegroundPrivilege::NotHigher
            && !facts.foreground_captures_input;
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

    fn silence(keyboard: Duration, mouse: Duration) -> HookSilence {
        HookSilence { keyboard, mouse }
    }

    fn typing_with_silent_hooks() -> WatchdogFacts {
        WatchdogFacts {
            input_idle: Duration::from_millis(50),
            silence: silence(Duration::from_secs(10), Duration::from_secs(10)),
            pointer_moving: false,
            foreground: ForegroundPrivilege::NotHigher,
            input_desktop_is_default: true,
            foreground_captures_input: false,
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
            silence: silence(Duration::from_millis(100), Duration::from_secs(10)),
            ..typing_with_silent_hooks()
        };
        assert_eq!(watchdog.decide(&alive, now), WatchdogAction::Nothing);
    }

    #[test]
    fn a_dead_mouse_hook_is_noticed_while_the_keyboard_hook_is_called() {
        // The user types and moves the mouse; Windows removed only the mouse hook. Keyboard
        // callbacks used to keep one shared mark fresh and hid it.
        let typing = Duration::from_millis(50);
        let moving_without_mouse_callbacks =
            silence(Duration::from_millis(20), Duration::from_secs(10));
        assert_eq!(
            silent_hook(typing, moving_without_mouse_callbacks, true),
            Some(SilentHook::Mouse)
        );
        let mut watchdog = HookWatchdog::new();
        let facts = WatchdogFacts {
            silence: moving_without_mouse_callbacks,
            pointer_moving: true,
            ..typing_with_silent_hooks()
        };
        assert_eq!(
            watchdog.decide(&facts, Instant::now()),
            WatchdogAction::Reinstall
        );
        // A live mouse hook keeps up with the pointer.
        assert_eq!(
            silent_hook(
                typing,
                silence(Duration::from_secs(10), Duration::from_millis(20)),
                true
            ),
            None
        );
    }

    #[test]
    fn a_dead_keyboard_hook_is_noticed_by_input_that_no_hook_saw() {
        let input = Duration::from_millis(50);
        let long = Duration::from_secs(10);
        let recent = Duration::from_millis(100);
        // Typing while the pointer stands still, and neither hook called.
        assert_eq!(
            silent_hook(input, silence(long, long), false),
            Some(SilentHook::Keyboard)
        );
        // A click or a scroll that the mouse hook saw is no typing.
        assert_eq!(silent_hook(input, silence(long, recent), false), None);
        // Typing that the keyboard hook saw.
        assert_eq!(silent_hook(input, silence(recent, long), false), None);
        // No input lately: nothing to judge by.
        assert_eq!(
            silent_hook(Duration::from_secs(5), silence(long, long), true),
            None
        );
    }

    #[test]
    fn the_pointer_moves_only_over_consecutive_ticks() {
        let mut pointer = PointerMotion::new();
        assert!(!pointer.observe(Some((10, 10))));
        // One jump, such as a program placing the pointer, is no movement.
        assert!(!pointer.observe(Some((400, 300))));
        assert!(!pointer.observe(Some((400, 300))));
        // Moves on two ticks in a row are.
        assert!(!pointer.observe(Some((410, 300))));
        assert!(pointer.observe(Some((420, 305))));
        assert!(pointer.observe(Some((430, 310))));
        // A stop, or a position that cannot be read (another desktop), ends it.
        assert!(!pointer.observe(Some((430, 310))));
        assert!(!pointer.observe(Some((440, 310))));
        assert!(!pointer.observe(None));
        assert!(!pointer.observe(Some((450, 310))));
        assert!(!pointer.observe(Some((460, 310))));
        assert!(pointer.observe(Some((470, 310))));
    }

    #[test]
    fn the_thresholds_are_exact() {
        let now = Instant::now();
        let facts = |input_idle, quiet| WatchdogFacts {
            input_idle,
            silence: silence(quiet, quiet),
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
    fn a_program_that_takes_the_keyboard_is_never_fought_and_costs_no_budget() {
        // 2026-10-11: a game and then VMware kept the hooks silent; the watchdog reinstalled
        // them again and again and twice warned about unstable hooks over the game.
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut watchdog = HookWatchdog::new();
        let captured = WatchdogFacts {
            foreground_captures_input: true,
            ..typing_with_silent_hooks()
        };
        for second in [0, 20, 40, 60, 80, 100] {
            assert_eq!(
                watchdog.decide(&captured, at(second)),
                WatchdogAction::Nothing
            );
        }
        // Back in an ordinary window the whole budget is still there.
        let facts = typing_with_silent_hooks();
        assert_eq!(watchdog.decide(&facts, at(120)), WatchdogAction::Reinstall);
        assert_eq!(watchdog.decide(&facts, at(135)), WatchdogAction::Reinstall);
        assert_eq!(watchdog.decide(&facts, at(150)), WatchdogAction::Reinstall);
        assert_eq!(watchdog.decide(&facts, at(165)), WatchdogAction::Reinstall);
    }

    #[test]
    fn keyboard_capturing_programs_are_matched_by_file_name_in_any_case() {
        for path in [
            r"C:\Program Files (x86)\VMware\VMware Workstation\vmware.exe",
            r"C:\Program Files\Oracle\VirtualBox\VirtualBoxVM.exe",
            r"C:\Windows\System32\mstsc.exe",
            "MSRDC.EXE",
            "vmconnect.exe",
            "vmplayer.exe",
        ] {
            assert!(captures_keyboard(path), "{path}");
        }
        for path in [
            r"C:\Program Files\WindowsApps\Terminal\WindowsTerminal.exe",
            r"C:\Program Files (x86)\VMware\VMware Workstation\vmware-tray.exe",
            r"C:\games\notvmware.exe",
            r"C:\games\vmware.exe.bak",
            "",
        ] {
            assert!(!captures_keyboard(path), "{path}");
        }
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
