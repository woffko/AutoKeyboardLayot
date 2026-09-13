//! Read-only composition/IME diagnostics for one foreground input thread.
//!
//! Prints only counts, flags and handle presence; it never reads or prints typed
//! text, window titles or process paths. Run it once with no composition and once
//! while an IME composition is active on the reviewed test machine.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    windows_probe::run();
}

#[cfg(windows)]
mod windows_probe {
    use std::mem::size_of;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::Ime::{
        GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR, ImmGetCompositionStringW, ImmGetContext,
        ImmIsIME, ImmReleaseContext,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
    use windows::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
    };

    fn thread_of(window: HWND) -> u32 {
        if window.0.is_null() {
            return 0;
        }
        let mut process_id = 0;
        unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) }
    }

    struct ContextReport {
        attached: bool,
        composition_bytes: i32,
        result_bytes: i32,
        cursor: i32,
    }

    fn probe_context(window: HWND) -> ContextReport {
        let mut report = ContextReport {
            attached: false,
            composition_bytes: -1,
            result_bytes: -1,
            cursor: -1,
        };
        if window.0.is_null() {
            return report;
        }
        let context = unsafe { ImmGetContext(window) };
        if context.0.is_null() {
            return report;
        }
        report.attached = true;
        report.composition_bytes =
            unsafe { ImmGetCompositionStringW(context, GCS_COMPSTR, None, 0) };
        report.result_bytes = unsafe { ImmGetCompositionStringW(context, GCS_RESULTSTR, None, 0) };
        report.cursor = unsafe { ImmGetCompositionStringW(context, GCS_CURSORPOS, None, 0) };
        unsafe {
            let _ = ImmReleaseContext(window, context);
        }
        report
    }

    pub fn run() {
        unsafe {
            let hwnd = GetForegroundWindow();
            let process_id = {
                let mut value = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut value));
                value
            };
            let foreground_thread = thread_of(hwnd);
            let mut info = GUITHREADINFO {
                cbSize: size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            let info_ok = GetGUIThreadInfo(foreground_thread, &mut info).is_ok();
            let focus = if info_ok {
                info.hwndFocus
            } else {
                HWND::default()
            };
            let input_thread = if !focus.0.is_null() {
                let focus_thread = thread_of(focus);
                if focus_thread != 0 {
                    focus_thread
                } else {
                    foreground_thread
                }
            } else {
                foreground_thread
            };
            let layout = GetKeyboardLayout(input_thread);
            let is_ime = ImmIsIME(layout).as_bool();
            let top = probe_context(hwnd);
            let focused = probe_context(focus);
            println!(
                "COMPOSITION_PROBE={{\"process_id\":{},\"foreground_thread\":{},\"input_thread\":{},\"gui_info\":{},\"focus_present\":{},\"layout\":{},\"is_ime\":{},\"gui_flags\":{},\"context_top_attached\":{},\"context_focus_attached\":{},\"composition_bytes\":{},\"result_bytes\":{},\"cursor\":{}}}",
                process_id,
                foreground_thread,
                input_thread,
                info_ok,
                usize::from(!focus.0.is_null()),
                layout.0 as usize,
                u8::from(is_ime),
                if info_ok { info.flags.0 } else { 0 },
                u8::from(top.attached),
                u8::from(focused.attached),
                focused.composition_bytes.max(top.composition_bytes),
                focused.result_bytes.max(top.result_bytes),
                focused.cursor.max(top.cursor),
            );
        }
    }
}
