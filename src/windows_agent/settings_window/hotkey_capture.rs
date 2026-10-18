//! Explicit, short-lived keyboard capture owned by the settings window.
//! Starts only with every keyboard key released; consumed modifier downs and
//! ups stay paired. No synthetic input, text log or global persistent recorder.
use super::super::ui_localization::{tr, tr_format};
use super::super::{acceptance_typing, current_foreground_context, hotkey_modifier_bit};
use super::{Hotkey, SettingsWindow, set_ui_hotkey};
use autokeyboardlayot::{HOTKEY_MOD_ALT, HOTKEY_MOD_CONTROL, HOTKEY_MOD_SHIFT, HOTKEY_MOD_WIN};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentProcessId},
    UI::{
        Input::KeyboardAndMouse::GetAsyncKeyState,
        WindowsAndMessaging::{
            CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, SetWindowsHookExW,
            UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

thread_local! {
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
    static ABORT_CAPTURE: Cell<bool> = const { Cell::new(false) };
    /// Polls for the release of the key that selected Record from the keyboard.
    static WAITING: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// How long a capture selected from the keyboard waits for the Space or Enter that selected it.
const RELEASE_WAIT: Duration = Duration::from_secs(2);

/// What starting a capture does now: begin with every key up, wait while a key is still down (the
/// Space or Enter that selected the button fires on its key-down), give up after `RELEASE_WAIT`
/// or when another program is in the foreground.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartStep {
    Begin,
    Wait,
    GiveUp,
}

fn start_step(owns_foreground: bool, released: bool, waited: Duration) -> StartStep {
    if !owns_foreground {
        StartStep::GiveUp
    } else if released {
        StartStep::Begin
    } else if waited < RELEASE_WAIT {
        StartStep::Wait
    } else {
        StartStep::GiveUp
    }
}

struct Capture {
    hook: HHOOK,
    ui: slint::Weak<SettingsWindow>,
    dispatch: CaptureDispatch,
    deadline: Instant,
    _timer: slint::Timer,
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.hook);
        }
    }
}

#[derive(Default)]
struct CaptureKeys {
    pressed: BTreeSet<u16>,
    chosen: Option<Hotkey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Update {
    Pending,
    Chosen(Hotkey, bool),
    Finished(Hotkey),
    Invalid,
    Refused(Refusal),
    Cancel,
}

/// Why the capture refuses a shortcut that the hotkey rules allow: once applied, the agent would
/// swallow typing with it everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// Shift alone with a letter or digit types a capital letter or a symbol.
    ShiftOnly,
    /// Ctrl+Alt is AltGr on many layouts: with a letter or digit it types a character (€, @).
    AltGr,
}

/// What the capture makes of a shortcut the hotkey rules allow: refused when it is typing, else
/// accepted, and true when Windows and many programs use it themselves.
fn verdict(hotkey: Hotkey) -> Result<bool, Refusal> {
    let letter_or_digit = matches!(hotkey.virtual_key, 0x30..=0x39 | 0x41..=0x5a);
    let ctrl_alt = HOTKEY_MOD_CONTROL | HOTKEY_MOD_ALT;
    if letter_or_digit && hotkey.modifiers == HOTKEY_MOD_SHIFT {
        return Err(Refusal::ShiftOnly);
    }
    if letter_or_digit
        && hotkey.modifiers & HOTKEY_MOD_WIN == 0
        && hotkey.modifiers & ctrl_alt == ctrl_alt
    {
        return Err(Refusal::AltGr);
    }
    Ok(is_common_shortcut(hotkey))
}

/// Alt+F4 closes the window; Ctrl+C, V, X, Z, A and S copy, paste, cut, undo, select all and save.
fn is_common_shortcut(hotkey: Hotkey) -> bool {
    const VK_F4: u16 = 0x73;
    (hotkey.modifiers == HOTKEY_MOD_ALT && hotkey.virtual_key == VK_F4)
        || (hotkey.modifiers == HOTKEY_MOD_CONTROL
            && matches!(hotkey.virtual_key, 0x41 | 0x43 | 0x53 | 0x56 | 0x58 | 0x5a))
}

/// The hint once `hotkey` is chosen, warning about a shortcut that Windows and programs use.
fn chosen_hint(hotkey: Hotkey) -> String {
    let key = if verdict(hotkey) == Ok(true) {
        "hotkey.selected_common"
    } else {
        "hotkey.selected"
    };
    tr_format(key, &[("hotkey", &hotkey.display_name())])
}

/// This hook-side state contains no GUI object or callback. The UI timer
/// consumes one coalesced update after the low-level hook has returned.
#[derive(Default)]
struct CaptureDispatch {
    keys: CaptureKeys,
    pending: Option<Update>,
    finished: bool,
}

impl CaptureDispatch {
    fn accept(&mut self, key: u16, down: bool) -> bool {
        if self.finished {
            return false;
        }
        let update = self.keys.event(key, down);
        if update != Update::Pending {
            self.pending = Some(update);
        }
        self.finished = matches!(
            update,
            Update::Finished(_) | Update::Chosen(_, true) | Update::Cancel
        );
        true
    }
}

impl CaptureKeys {
    fn event(&mut self, key: u16, down: bool) -> Update {
        if key == 0x1b && down {
            return Update::Cancel;
        }
        // Pause/Break may be delivered as a pulse without a key-up. Do not
        // wait forever for that release; still wait for any held modifiers.
        if down && !matches!(key, 0x13 | 0x03) {
            self.pressed.insert(key);
        }
        if !down {
            self.pressed.remove(&key);
        }
        if let Some(hotkey) = self.chosen {
            return if self.pressed.is_empty() {
                Update::Finished(hotkey)
            } else {
                Update::Pending
            };
        }
        if !down || hotkey_modifier_bit(key).is_some() {
            return Update::Pending;
        }
        let modifiers = self
            .pressed
            .iter()
            .filter_map(|key| hotkey_modifier_bit(*key))
            .fold(0, |all, bit| all | bit);
        match Hotkey::new(key, modifiers).map(|hotkey| (hotkey, verdict(hotkey))) {
            Ok((hotkey, Ok(_))) => {
                self.chosen = Some(hotkey);
                Update::Chosen(hotkey, self.pressed.is_empty())
            }
            Ok((_, Err(refusal))) => Update::Refused(refusal),
            Err(_) => Update::Invalid,
        }
    }
}

fn owns_foreground() -> bool {
    current_foreground_context()
        .is_some_and(|context| context.process_id == unsafe { GetCurrentProcessId() })
}

fn keyboard_released() -> bool {
    (0x08..=0xfe).all(|key| unsafe { GetAsyncKeyState(key) as u16 & 0x8000 == 0 })
}

pub(super) fn start(ui: &SettingsWindow) {
    stop();
    ABORT_CAPTURE.with(|flag| flag.set(false));
    match start_step(owns_foreground(), keyboard_released(), Duration::ZERO) {
        StartStep::Begin => begin(ui),
        StartStep::GiveUp => ui.set_hotkey_capture_hint(tr("hotkey.release_retry").into()),
        StartStep::Wait => {
            let began = Instant::now();
            let weak = ui.as_weak();
            let timer = slint::Timer::default();
            timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(25),
                move || {
                    let step = start_step(owns_foreground(), keyboard_released(), began.elapsed());
                    if step == StartStep::Wait {
                        return;
                    }
                    WAITING.with(|slot| slot.borrow_mut().take());
                    let Some(ui) = weak.upgrade() else {
                        return;
                    };
                    if step == StartStep::Begin {
                        begin(&ui);
                    } else {
                        ui.set_hotkey_capture_hint(tr("hotkey.release_retry").into());
                    }
                },
            );
            WAITING.with(|slot| *slot.borrow_mut() = Some(timer));
        }
    }
}

/// Installs the capture hook; every key is up.
fn begin(ui: &SettingsWindow) {
    let hook = unsafe {
        GetModuleHandleW(None).and_then(|module| {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook),
                Some(HINSTANCE(module.0)),
                0,
            )
        })
    };
    let Ok(hook) = hook else {
        ui.set_hotkey_capture_hint(tr("hotkey.start_failed").into());
        return;
    };
    // Close the small installation race before taking ownership of key-ups.
    if !keyboard_released() {
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
        ui.set_hotkey_capture_hint(tr("hotkey.release_repeat").into());
        return;
    }
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(50),
        process_pending_update,
    );
    CAPTURE.with(|slot| {
        *slot.borrow_mut() = Some(Capture {
            hook,
            ui: ui.as_weak(),
            dispatch: CaptureDispatch::default(),
            deadline: Instant::now() + Duration::from_secs(15),
            _timer: timer,
        })
    });
    ui.set_hotkey_recording(true);
    ui.set_hotkey_capture_hint(tr("hotkey.record_hint").into());
}

pub(super) fn stop() {
    WAITING.with(|slot| slot.borrow_mut().take());
    let capture = CAPTURE.with(|slot| slot.borrow_mut().take());
    if let Some(capture) = capture {
        if let Some(ui) = capture.ui.upgrade() {
            ui.set_hotkey_recording(false);
        }
        drop(capture);
    }
}

fn cancel(message: impl AsRef<str>) {
    let ui = CAPTURE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|capture| capture.ui.upgrade())
    });
    stop();
    if let Some(ui) = ui {
        ui.set_hotkey_capture_hint(message.as_ref().into());
    }
}

fn process_pending_update() {
    let snapshot = CAPTURE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let capture = slot.as_mut()?;
        Some((
            capture.ui.clone(),
            capture.deadline,
            capture.dispatch.pending.take(),
        ))
    });
    let Some((weak, deadline, update)) = snapshot else {
        return;
    };
    // Never hold the capture-state borrow while querying or updating Slint.
    let Some(ui) = weak.upgrade() else {
        stop();
        return;
    };
    if ABORT_CAPTURE.with(Cell::get)
        || Instant::now() >= deadline
        || !owns_foreground()
        || ui.get_current_page() != 0
        || !ui.get_hotkey_enabled()
    {
        cancel(tr("hotkey.cancelled"));
        return;
    }
    match update {
        Some(Update::Chosen(hotkey, finished)) => {
            ui.set_hotkey_capture_hint(chosen_hint(hotkey).into());
            if finished {
                set_ui_hotkey(&ui, hotkey);
                stop();
            }
        }
        // The choice may arrive with its release, between two ticks of this timer.
        Some(Update::Finished(hotkey)) => {
            ui.set_hotkey_capture_hint(chosen_hint(hotkey).into());
            set_ui_hotkey(&ui, hotkey);
            stop();
        }
        Some(Update::Cancel) => cancel(tr("hotkey.cancelled")),
        Some(Update::Invalid) => ui.set_hotkey_capture_hint(tr("hotkey.modifier_required").into()),
        Some(Update::Refused(Refusal::ShiftOnly)) => {
            ui.set_hotkey_capture_hint(tr("hotkey.shift_only").into());
        }
        Some(Update::Refused(Refusal::AltGr)) => {
            ui.set_hotkey_capture_hint(tr("hotkey.altgr").into());
        }
        Some(Update::Pending) | None => {}
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    // Test builds take the desktop acceptance's marked typing as the user's.
    if event.flags.contains(LLKHF_INJECTED) && !acceptance_typing(event.dwExtraInfo) {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    if ABORT_CAPTURE.with(Cell::get) || !owns_foreground() {
        ABORT_CAPTURE.with(|flag| flag.set(true));
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let message = wparam.0 as u32;
    let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
    if !down && !matches!(message, WM_KEYUP | WM_SYSKEYUP) {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let swallow = CAPTURE.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            ABORT_CAPTURE.with(|flag| flag.set(true));
            return false;
        };
        let Some(capture) = slot.as_mut() else {
            return false;
        };
        if Instant::now() >= capture.deadline {
            ABORT_CAPTURE.with(|flag| flag.set(true));
            return false;
        }
        capture.dispatch.accept(event.vkCode as u16, down)
    });
    if swallow {
        LRESULT(1)
    } else {
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use autokeyboardlayot::HOTKEY_MOD_CONTROL;

    #[test]
    fn capture_tracks_modifier_release_before_finishing() {
        let mut keys = CaptureKeys::default();
        assert_eq!(keys.event(0xa2, true), Update::Pending);
        assert_eq!(
            keys.event(0x7b, true),
            Update::Chosen(Hotkey::new(0x7b, HOTKEY_MOD_CONTROL).unwrap(), false)
        );
        assert_eq!(keys.event(0x7b, false), Update::Pending);
        assert_eq!(
            keys.event(0xa2, false),
            Update::Finished(Hotkey::new(0x7b, HOTKEY_MOD_CONTROL).unwrap())
        );
    }

    #[test]
    fn capture_pause_does_not_require_a_missing_key_up() {
        let mut keys = CaptureKeys::default();
        assert_eq!(
            keys.event(0x13, true),
            Update::Chosen(Hotkey::default(), true)
        );
    }

    #[test]
    fn capture_rejects_a_plain_letter_and_escape_cancels() {
        let mut keys = CaptureKeys::default();
        assert_eq!(keys.event(0x41, true), Update::Invalid);
        assert_eq!(keys.event(0x41, false), Update::Pending);
        assert_eq!(keys.event(0x1b, true), Update::Cancel);
    }

    #[test]
    fn a_shortcut_that_types_is_refused_and_a_common_one_is_flagged() {
        use autokeyboardlayot::{HOTKEY_MOD_ALT, HOTKEY_MOD_SHIFT, HOTKEY_MOD_WIN};
        let hotkey = |key: u8, modifiers| Hotkey::new(u16::from(key), modifiers).unwrap();
        let ctrl_alt = HOTKEY_MOD_CONTROL | HOTKEY_MOD_ALT;
        for (shortcut, expected) in [
            // Shift types capitals and symbols; Ctrl+Alt is AltGr (€ on E, @ on 2), with Shift too.
            (hotkey(b'Q', HOTKEY_MOD_SHIFT), Err(Refusal::ShiftOnly)),
            (hotkey(b'7', HOTKEY_MOD_SHIFT), Err(Refusal::ShiftOnly)),
            (hotkey(b'E', ctrl_alt), Err(Refusal::AltGr)),
            (
                hotkey(b'2', ctrl_alt | HOTKEY_MOD_SHIFT),
                Err(Refusal::AltGr),
            ),
            // With Win it is no AltGr; Ctrl, Alt or Win make a letter a shortcut, Shift may join.
            (hotkey(b'E', ctrl_alt | HOTKEY_MOD_WIN), Ok(false)),
            (
                hotkey(b'Q', HOTKEY_MOD_CONTROL | HOTKEY_MOD_SHIFT),
                Ok(false),
            ),
            (hotkey(b'Q', HOTKEY_MOD_ALT), Ok(false)),
            (hotkey(0x74, HOTKEY_MOD_SHIFT), Ok(false)),
            (Hotkey::default(), Ok(false)),
            // Shortcuts Windows and programs use: accepted with a warning.
            (hotkey(0x73, HOTKEY_MOD_ALT), Ok(true)),
            (hotkey(b'C', HOTKEY_MOD_CONTROL), Ok(true)),
            (hotkey(b'S', HOTKEY_MOD_CONTROL), Ok(true)),
            (hotkey(b'Z', HOTKEY_MOD_CONTROL), Ok(true)),
            (
                hotkey(b'C', HOTKEY_MOD_CONTROL | HOTKEY_MOD_SHIFT),
                Ok(false),
            ),
        ] {
            assert_eq!(verdict(shortcut), expected, "{}", shortcut.display_name());
        }
        // A refused chord leaves the capture open for another one.
        let mut keys = CaptureKeys::default();
        assert_eq!(keys.event(0xa0, true), Update::Pending);
        assert_eq!(keys.event(0x51, true), Update::Refused(Refusal::ShiftOnly));
        assert_eq!(keys.event(0x51, false), Update::Pending);
        assert_eq!(keys.event(0xa0, false), Update::Pending);
        assert_eq!(keys.event(0xa2, true), Update::Pending);
        assert_eq!(
            keys.event(0x51, true),
            Update::Chosen(hotkey(b'Q', HOTKEY_MOD_CONTROL), false)
        );
        // The hint names the shortcut and warns about a common one.
        let common = chosen_hint(hotkey(b'C', HOTKEY_MOD_CONTROL));
        assert!(
            common.contains("Ctrl+C") && common.contains("programs"),
            "{common}"
        );
        assert!(!chosen_hint(hotkey(b'Q', HOTKEY_MOD_CONTROL)).contains("programs"));
    }

    #[test]
    fn recording_selected_from_the_keyboard_waits_for_that_key_to_be_released() {
        use StartStep::{Begin, GiveUp, Wait};
        let moment = Duration::from_millis(100);
        assert_eq!(start_step(true, true, Duration::ZERO), Begin);
        // Space or Enter selected the button and is still down: wait for its release.
        assert_eq!(start_step(true, false, Duration::ZERO), Wait);
        assert_eq!(start_step(true, false, RELEASE_WAIT - moment), Wait);
        assert_eq!(start_step(true, true, RELEASE_WAIT - moment), Begin);
        // A key held longer, or another program in the foreground, ends it.
        assert_eq!(start_step(true, false, RELEASE_WAIT), GiveUp);
        assert_eq!(start_step(false, true, Duration::ZERO), GiveUp);
        assert_eq!(start_step(false, false, moment), GiveUp);
    }

    #[test]
    fn hook_updates_coalesce_until_the_ui_loop_consumes_them() {
        let mut dispatch = CaptureDispatch::default();
        assert!(dispatch.accept(0xa2, true));
        assert!(dispatch.accept(0x7b, true));
        assert!(dispatch.accept(0x7b, false));
        assert!(dispatch.accept(0xa2, false));
        assert_eq!(
            dispatch.pending.take(),
            Some(Update::Finished(
                Hotkey::new(0x7b, HOTKEY_MOD_CONTROL).unwrap()
            ))
        );
        assert!(!dispatch.accept(0x41, true));
    }

    #[test]
    fn cancellation_opens_input_without_waiting_for_a_gui_callback() {
        let mut dispatch = CaptureDispatch::default();
        assert!(dispatch.accept(0xa2, true));
        assert!(dispatch.accept(0x1b, true));
        assert_eq!(dispatch.pending, Some(Update::Cancel));
        assert!(!dispatch.accept(0xa2, false));
        assert!(!dispatch.accept(0x41, true));
    }
}
