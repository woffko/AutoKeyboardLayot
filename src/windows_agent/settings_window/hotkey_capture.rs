//! Explicit, short-lived keyboard capture owned by the settings window.
//! Starts only with every keyboard key released; consumed modifier downs and
//! ups stay paired. No synthetic input, text log or global persistent recorder.
use super::super::{current_foreground_context, hotkey_modifier_bit};
use super::{Hotkey, SettingsWindow, set_ui_hotkey};
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
    Cancel,
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
        match Hotkey::new(key, modifiers) {
            Ok(hotkey) => {
                self.chosen = Some(hotkey);
                Update::Chosen(hotkey, self.pressed.is_empty())
            }
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
    if !owns_foreground() || !keyboard_released() {
        ui.set_hotkey_capture_hint("Отпустите клавиши и снова нажмите «Нажать сочетание».".into());
        return;
    }
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
        ui.set_hotkey_capture_hint(
            "Не удалось начать назначение. Можно выбрать кнопку «Вернуть Pause/Break».".into(),
        );
        return;
    };
    // Close the small installation race before taking ownership of key-ups.
    if !keyboard_released() {
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
        ui.set_hotkey_capture_hint("Отпустите клавиши и повторите назначение.".into());
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
    ui.set_hotkey_capture_hint(
        "Нажмите Pause/Break, F-клавишу или сочетание с Ctrl / Alt / Shift / Win. Esc — отмена."
            .into(),
    );
}

pub(super) fn stop() {
    let capture = CAPTURE.with(|slot| slot.borrow_mut().take());
    if let Some(capture) = capture {
        if let Some(ui) = capture.ui.upgrade() {
            ui.set_hotkey_recording(false);
        }
        drop(capture);
    }
}

fn cancel(message: &str) {
    let ui = CAPTURE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|capture| capture.ui.upgrade())
    });
    stop();
    if let Some(ui) = ui {
        ui.set_hotkey_capture_hint(message.into());
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
        cancel("Назначение отменено; прежнее сочетание сохранено.");
        return;
    }
    match update {
        Some(Update::Chosen(hotkey, finished)) => {
            ui.set_hotkey_capture_hint(format!("Выбрано {}. Отпустите клавиши и нажмите «Применить».", hotkey.display_name()).into());
            if finished { set_ui_hotkey(&ui, hotkey); stop(); }
        }
        Some(Update::Finished(hotkey)) => { set_ui_hotkey(&ui, hotkey); stop(); }
        Some(Update::Cancel) => cancel("Назначение отменено; прежнее сочетание сохранено."),
        Some(Update::Invalid) => ui.set_hotkey_capture_hint("Для буквы или цифры удерживайте Ctrl, Alt, Shift или Win. Также подходят Pause/Break и F1–F24.".into()),
        Some(Update::Pending) | None => {}
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    if event.flags.contains(LLKHF_INJECTED) {
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
