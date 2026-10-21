//! Win32 adapter for the observation-only user-session agent.

use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use autokeyboardlayot::bounded_probe::{BoundedProbe, ProbeFailure};
use autokeyboardlayot::conversion_sound::{self, SoundCue};
use autokeyboardlayot::installed_packages::{InstalledPackages, PackageSource};
use autokeyboardlayot::panic_guard::{BusyGuard, FaultWindow, WorkerExit, guarded, run_contained};
use autokeyboardlayot::profile_resolver::ResolvedKeyboardProfiles;
use autokeyboardlayot::rate_limit::RateLimit;
use autokeyboardlayot::tray_visual::{self, TrayVisual};
use autokeyboardlayot::ui_guard::{
    ForegroundPrivilege, HookSilence, HookWatchdog, PointerMotion, SilentHook, TrayThrottle,
    WatchdogAction, WatchdogFacts, captures_keyboard, retry_delay, silent_hook,
};
use autokeyboardlayot::windows_input_profiles::KeyboardProfileCache;
use ui_localization::{tr, tr_format};

mod ui_localization;
use autokeyboardlayot::{
    BackendRules, BackendStrategy, ConfigurationDocument, ConversionTransaction, Detection,
    Detector, DetectorConfig, ExclusionPolicy, HOTKEY_MOD_ALT, HOTKEY_MOD_CONTROL,
    HOTKEY_MOD_SHIFT, HOTKEY_MOD_WIN, Hotkey, InputEvent, InputSession, Language,
    PrivacyBlockReason, SessionAction, Settings, UserLexicon,
};

mod installer_lifecycle;
mod settings_window;
pub use installer_lifecycle::{initialize_installation, prepare_upgrade};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_TIMEOUT, GetLastError, GlobalFree, HANDLE,
            HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WAIT_ABANDONED, WAIT_OBJECT_0,
            WPARAM,
        },
        Graphics::Gdi::{CreateBitmap, DeleteObject, HGDIOBJ},
        Security::{
            GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
            TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel,
        },
        Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize, DATADIR_GET, FORMATETC, STGMEDIUM, TYMED_ENHMF,
                TYMED_GDI, TYMED_HGLOBAL, TYMED_MFPICT,
            },
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardOwner, GetClipboardSequenceNumber,
                GetOpenClipboardWindow, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{
                CF_UNICODETEXT, OleGetClipboard, OleInitialize, OleUninitialize, ReleaseStgMedium,
            },
            RemoteDesktop::{
                NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
                WTSUnRegisterSessionNotification,
            },
            StationsAndDesktops::{
                CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
                GetUserObjectInformationW, OpenInputDesktop, UOI_NAME,
            },
            SystemInformation::GetTickCount64,
            Threading::{
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateMutexW, CreateWaitableTimerExW,
                GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, ReleaseMutex,
                SetWaitableTimerEx, TIMER_ALL_ACCESS, WaitForSingleObject,
            },
        },
        UI::{
            Accessibility::{
                CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationTextPattern,
                IUIAutomationTextRange, TextPatternRangeEndpoint_End,
                TextPatternRangeEndpoint_Start, TextUnit_Character, UIA_CONTROLTYPE_ID,
                UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_TextPatternId,
            },
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, GetKeyState, GetKeyboardLayout, GetKeyboardState,
                GetLastInputInfo, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
                KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, LASTINPUTINFO,
                MAPVK_VSC_TO_VK_EX, MapVirtualKeyExW, SendInput, ToUnicodeEx, VIRTUAL_KEY, VK_APPS,
                VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DECIMAL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE,
                VK_F1, VK_F24, VK_HOME, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN,
                VK_MENU, VK_NUMLOCK, VK_NUMPAD0, VK_PAUSE, VK_RCONTROL, VK_RETURN, VK_RIGHT,
                VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
            },
            Shell::{
                NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD,
                NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NINF_KEY, NOTIFY_ICON_MESSAGE,
                NOTIFYICON_VERSION_4, NOTIFYICONDATAW, QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN,
                SHQueryUserNotificationState, Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CallNextHookEx, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
                DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW,
                GUITHREADINFO, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetMessageW,
                GetShellWindow, GetWindowThreadProcessId, HHOOK, HICON, HMENU, ICONINFO, IDYES,
                KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLMHF_INJECTED, MB_ICONERROR,
                MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNO, MF_CHECKED, MF_GRAYED,
                MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, MSLLHOOKSTRUCT, MessageBoxW,
                PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PM_REMOVE, PeekMessageW,
                PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
                SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT, SendMessageTimeoutW,
                SetForegroundWindow, SetTimer, SetWindowTextW, SetWindowsHookExW, TPM_RIGHTBUTTON,
                TrackPopupMenu, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
                WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU,
                WM_DESTROY, WM_INPUTLANGCHANGEREQUEST, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK,
                WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_NULL, WM_POWERBROADCAST, WM_QUIT,
                WM_RBUTTONDOWN, WM_RENDERALLFORMATS, WM_RENDERFORMAT, WM_SYSKEYDOWN, WM_SYSKEYUP,
                WM_TIMER, WM_WTSSESSION_CHANGE, WM_XBUTTONDOWN, WNDCLASSW, WTS_CONSOLE_CONNECT,
                WTS_REMOTE_CONNECT, WTS_SESSION_UNLOCK,
            },
        },
    },
    core::{Error, HRESULT, HSTRING, Interface, PCWSTR, PWSTR, Result, w},
};

#[cfg(test)]
use windows::Win32::{
    Foundation::{ERROR_SUCCESS, SetLastError},
    System::DataExchange::{EnumClipboardFormats, GetClipboardData},
    UI::{Accessibility::UIA_TextControlTypeId, Input::KeyboardAndMouse::VK_F12},
};

const WINDOW_CLASS: PCWSTR = w!("AutoKeyboardLayot.ObserverWindow");
const WINDOW_TITLE: PCWSTR = w!("AutoKeyboardLayot");
/// Same bound as the tracked word; longer tokens are discarded, not converted.
const MANUAL_TOKEN_MAX_KEYS: usize = 64;

/// Package version and short source commit, shown in the settings window.
const APP_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("AUTOKEY_BUILD_COMMIT"),
    ")"
);
const TRAY_ID: u32 = 1;
const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
// Composite macro from Shellapi.h (not emitted by windows-rs metadata).
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
const GATE_ARM_MESSAGE: u32 = WM_APP + 2;
const GATE_RELEASE_MESSAGE: u32 = WM_APP + 3;
const GATE_DRAIN_ACK_MESSAGE: u32 = WM_APP + 4;
const GATE_FAIL_OPEN_MESSAGE: u32 = WM_APP + 5;
/// The LPARAM of a fail-open request whose edit may have left the word selected.
const GATE_FAIL_OPEN_SELECTION_LEFT: isize = 1;
const GATE_HANDOFF_BUDGET_MESSAGE: u32 = WM_APP + 6;
const LEXICON_OFFER_MESSAGE: u32 = WM_APP + 7;
const SETTINGS_APPLIED_MESSAGE: u32 = WM_APP + 8;
const RECOVERY_NOTICE_MESSAGE: u32 = WM_APP + 9;
const HELD_KEYS_RETURN_MESSAGE: u32 = WM_APP + 10;
/// A tray "add word" save finished on its own thread; the window reports it.
const LEXICON_SAVED_MESSAGE: u32 = WM_APP + 11;
const LAYOUT_TIMER_ID: usize = 1;
const LAYOUT_TIMER_INTERVAL_MS: u32 = 250;
const MENU_AUTO_ID: usize = 1002;
const MENU_EXIT_ID: usize = 1003;
const MENU_ADD_LEXICON_ENTRY_ID: usize = 1004;
const MENU_RECOVER_INPUT_ID: usize = 1005;
const MENU_DISCARD_INPUT_ID: usize = 1006;
const MENU_SETTINGS_ID: usize = 1101;
const INPUT_QUEUE_CAPACITY: usize = 512;
const INPUT_SETTLE_AFTER_FORWARD_MS: u64 = 35;
const HOOK_FORWARD_TIMEOUT_MS: u64 = 50;
const NOTEPAD_TEXT_BARRIER_TIMEOUT_MS: u64 = 150;
const UIA_SELECTION_CONFIRM_TIMEOUT_MS: u64 = 50;
/// A Pause press waits this long for the field to show the selection of the word, and selects
/// a second time when it did not. Automatic conversions keep the shorter wait above, which the
/// correction gate's time budget accounts for.
const UIA_MANUAL_SELECTION_CONFIRM_TIMEOUT_MS: u64 = 100;
const UIA_MANUAL_SELECTION_ATTEMPTS: u32 = 2;
/// How often, and for how long at most, the agent's own selection is collapsed and read back
/// when an edit does not go ahead.
const SELECTION_COLLAPSE_ATTEMPTS: u32 = 4;
const SELECTION_COLLAPSE_BUDGET_MS: u64 = 200;
/// How long, and for how many windows, the last word stays available to Pause after the agent
/// stopped tracking it (a window switch, a click, a shortcut). Memory only, never written.
const REMEMBERED_WORD_TTL: Duration = Duration::from_secs(120);
const REMEMBERED_WORD_WINDOWS: usize = 8;
const NOTEPAD_REPLAY_STEP_DELAY_MS: u64 = 3;
const NOTEPAD_REPLAY_GATE_MARGIN_MS: u64 = 200;
const CLIPBOARD_OPEN_RETRIES: usize = 10;
/// Putting the user's clipboard back is worth a longer wait than installing the temporary text
/// (30 x 2 ms instead of 10 x 2 ms): a clipboard manager may hold the clipboard open for a moment.
const CLIPBOARD_RESTORE_OPEN_RETRIES: usize = 30;
const CLIPBOARD_RETRY_DELAY_MS: u64 = 2;
const CLIPBOARD_BROKER_TIMEOUT_MS: u64 = 500;
/// How long a paste waits for the broker to put the user's clipboard back. The broker opens the
/// clipboard for up to 60 ms; a restore that takes longer finishes on its own and is logged when
/// it is over.
const CLIPBOARD_RESTORE_WAIT_MS: u64 = 100;
/// How long the broker of a paste that was sent but not confirmed waits for the paste's target to
/// read the replacement before it leaves the replacement on the clipboard for good.
const UNCONFIRMED_PASTE_WAIT_MS: u64 = 5_000;
/// How long the replacement stays after the target of an unconfirmed paste read it. The read may
/// be the target's own look at the new clipboard (a clipboard listener) rather than its paste, and
/// a busy program handles its queued Ctrl+V right after: that paste still gets the replacement.
const TARGET_READ_GRACE_MS: u64 = 500;
/// The least time worth giving the installation of a paste's replacement.
const PASTE_MIN_INSTALL_MS: u64 = 50;
/// What a protected paste needs after its Ctrl+V: the commit of the replacement, the restore of
/// the user's clipboard, the layout switch, the second commit and the UI Automation calls between.
const PASTE_AFTER_SUBMIT_MS: u64 = 2 * NOTEPAD_TEXT_BARRIER_TIMEOUT_MS
    + CLIPBOARD_RESTORE_WAIT_MS
    + LAYOUT_SWITCH_TIMEOUT_MS
    + 100;
/// An automatic conversion waits this long for a busy clipboard, then leaves the word alone.
const CLIPBOARD_AUTOMATIC_INSTALL_TIMEOUT_MS: u64 = 200;
const VK_V_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
const LAYOUT_SWITCH_TIMEOUT_MS: u64 = 250;
const LAYOUT_STABLE_MS: u64 = 50;
const REPLAY_COMMIT_MS: u64 = 50;
const CORRECTION_GATE_MAX_HOLD_MS: u64 = 1_200;
const CORRECTION_GATE_CAPACITY: usize = 256;
/// Held keys that could not be handed back wait this long in memory for an explicit recovery and
/// are then deleted, so that they cannot keep automatic conversion paused and Exit declined for
/// hours. Preparing a recovery from the tray menu gives them this long again from then.
const RETAINED_INPUT_LIFETIME_MS: u64 = 60_000;
/// Held keys that the worker should hand back automatically are handed back by the UI thread
/// when the worker is still busy this long after they were retained.
const STALLED_WORKER_HAND_BACK_MS: u64 = 2_000;
const TO_UNICODE_DO_NOT_CHANGE_KEYBOARD_STATE: u32 = 4;
const PRIVACY_ALLOWED: u8 = 0;
// Worker-only wait: the low-level hook never waits for UI Automation.
// Give ordinary delayed replies more headroom before failing closed.
const PRIVACY_PROBE_WAIT_MS: u64 = 300;
/// How long a failed field check may lean on the last good one for the same input target. The
/// provider of a busy terminal answers a check late now and then; see `settle_probe_outcome`.
const PRIVACY_RECENT_OK_MS: u64 = 2_000;
/// Periodic field checks run every half second while they are fast. A slow one spaces the next
/// by twenty times its own length, so that periodic checks fill at most a twentieth of the
/// worker's time, and never more than five seconds apart.
const PRIVACY_REFRESH_MIN_MS: u64 = 500;
const PRIVACY_REFRESH_MAX_MS: u64 = 5_000;
const PRIVACY_REFRESH_COST_FACTOR: u32 = 20;
/// A field check at least this slow marks a full-screen program (a game answered in about
/// 150 ms) that is not asked again periodically while its input target stays the same.
const PRIVACY_SLOW_PROVIDER_MS: u64 = 50;
const SLOW_INPUT_DIAGNOSTIC_MS: u64 = 30;
/// A privacy provider that keeps the probe busy this long without answering is replaced.
const PRIVACY_PROBE_STUCK_AFTER_SECS: u64 = 5;
const PRIVACY_PROBE_MAX_RESPAWNS: usize = 3;
/// A text-barrier provider that has not taken a new read for this long is replaced by a fresh
/// one, at most this many times.
const BARRIER_PROBE_STUCK_AFTER_MS: u64 = 2_000;
const BARRIER_PROBE_MAX_RESPAWNS: usize = 3;
/// A hook callback slower than this is recorded; Windows gives hooks a tight time budget.
const HOOK_SLOW_DIAGNOSTIC_MS: u64 = 50;
/// Timer ticks are 250 ms apart; a longer gap means the UI thread was blocked.
const UI_STALL_DIAGNOSTIC_MS: u64 = 700;
const UIA_PROVIDER_TIMEOUT_MS: u32 = 100;
const PRIVACY_PASSWORD: u8 = 1;
const PRIVACY_EXCLUDED: u8 = 2;
const PRIVACY_UNAVAILABLE: u8 = 3;
const PRIVACY_ELEVATED: u8 = 4;
const BACKEND_OBSERVE_ONLY: u8 = 0;
const BACKEND_PROTECTED_PASTE: u8 = 1;
const BACKEND_PHYSICAL_REPLAY: u8 = 2;
const BACKEND_CAPABILITY_PROBE: u8 = 3;
const BACKEND_CAPABILITY_UNSUPPORTED: u8 = 4;
const BACKEND_CAPABILITY_MISMATCH: u8 = 5;
const BACKEND_SELECTION_MISMATCH: u8 = 6;
const BACKEND_SELECTION_UNSUPPORTED: u8 = 7;
const INJECTED_EVENT_MARKER: usize = 0x414B_4C59_5458_0001;
const DRAINED_EVENT_MARKER: usize = 0x414B_4C59_5458_0002;
/// Keys the desktop acceptance types for the agent. Injected like any program's keys, they count as
/// the user's typing in test builds only: the agent itself never takes another program's keys.
#[cfg(test)]
const ACCEPTANCE_TYPING_MARKER: usize = 0x414B_4C59_5458_0003;
const GATE_FENCE_MARKER_BASE: usize = 0x414B_4C59_5458_8000;
const LEXICON_OFFER_SECONDS: u64 = 120;
const MAX_UIA_EDITABLE_ANCESTOR_DEPTH: usize = 32;
const DIAGNOSTIC_QUEUE_CAPACITY: usize = 256;
const DIAGNOSTIC_LOG_MAX_BYTES: u64 = 1024 * 1024;

const AGENT_MUTEX: PCWSTR = w!("Local\\AutoKeyboardLayot.Agent");

struct InstanceGuard {
    handle: HANDLE,
    _installation_fence: std::fs::File,
}

impl InstanceGuard {
    fn acquire() -> Result<Option<Self>> {
        let Some(fence) = autokeyboardlayot::installation_fence::shared_for_current_user()
            .map_err(|error| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
            })?
        else {
            return Ok(None);
        };
        unsafe {
            let handle = CreateMutexW(None, false, AGENT_MUTEX)?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle)?;
                Ok(None)
            } else {
                Ok(Some(Self {
                    handle,
                    _installation_fence: fence,
                }))
            }
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

thread_local! {
    static APP_STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
    static LAST_TIMER_TICK: Cell<Option<Instant>> = const { Cell::new(None) };
    static UI_GUARDS: RefCell<UiGuards> = RefCell::new(UiGuards::new());
}

/// Milliseconds (`GetTickCount64`) of the last callback or installation of the keyboard hook and
/// of the mouse hook. The watchdog compares each with the input that hook must have seen.
static KEYBOARD_HOOK_ACTIVITY_MS: AtomicU64 = AtomicU64::new(0);
static MOUSE_HOOK_ACTIVITY_MS: AtomicU64 = AtomicU64::new(0);
/// The window message registered as "TaskbarCreated"; 0 until it is registered.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// A failed attempt to install hooks or the tray icon, repeated on later timer ticks.
struct Retry {
    failures: u32,
    next_attempt: Instant,
}

impl Retry {
    fn first(now: Instant) -> Self {
        Self {
            failures: 1,
            next_attempt: now + retry_delay(1),
        }
    }

    fn failed_again(&mut self, now: Instant) {
        self.failures += 1;
        self.next_attempt = now + retry_delay(self.failures);
    }
}

/// UI-thread state of the rules that keep the thread responsive for the hooks (see `ui_guard`).
struct UiGuards {
    watchdog: HookWatchdog,
    pointer: PointerMotion,
    tray: TrayThrottle,
    hook_retry: Option<Retry>,
    tray_retry: Option<Retry>,
}

impl UiGuards {
    fn new() -> Self {
        Self {
            watchdog: HookWatchdog::new(),
            pointer: PointerMotion::new(),
            tray: TrayThrottle::new(),
            hook_retry: None,
            tray_retry: None,
        }
    }
}

/// Set when a panic was contained in a hook callback, the window procedure or the input worker.
/// The UI timer reacts to it (see `handle_engine_fault`).
static ENGINE_FAULT: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
struct ObserverMetrics {
    candidates: AtomicU64,
    dropped_events: AtomicU64,
    input_epoch: AtomicU64,
    configuration_pending: AtomicBool,
    configuration_ack: AtomicU64,
    // A revision whose application panicked in the worker; the UI drops it instead of waiting.
    configuration_rejected: AtomicU64,
    last_input_layout: AtomicUsize,
    privacy_reason: AtomicU8,
    auto_enabled: AtomicBool,
    safety_paused: AtomicBool,
    pause_break_undo: AtomicBool,
    force_hotkey_virtual_key: AtomicU32,
    force_hotkey_modifiers: AtomicU8,
    undo_available: AtomicBool,
    undo_hotkey_active: AtomicBool,
    hotkey_waiting_for_release: AtomicBool,
    explicit_hotkey_sequence: AtomicU64,
    conversion_failures: AtomicU64,
    last_failure_reason: AtomicU8,
    backend_status: AtomicU8,
    diagnostics_enabled: AtomicBool,
    dropped_diagnostics: AtomicU64,
    // Records the log writer could not write, and full logs it could not move aside.
    diagnostic_log_errors: AtomicU64,
    gate_replayed_events: AtomicU64,
    observed_input_sequence: AtomicU64,
    forwarded_input_sequence: AtomicU64,
    active_gate_token: AtomicU64,
    cancelled_gate_token: AtomicU64,
    privacy_refresh_queued: AtomicBool,
    profile_refresh_queued: AtomicBool,
    worker_busy: AtomicBool,
    // The worker is inside an edit that may have selected or typed text in the field.
    edit_in_flight: AtomicBool,
    // The gate whose paste may have left the word selected: its held keys come back only when the
    // user asks for them.
    selection_left_token: AtomicU64,
    // Set when the input worker gave up after repeated internal faults.
    worker_dead: AtomicBool,
    // Gate release requests made by the worker, counted before any window is involved.
    gate_release_requests: AtomicU64,
    // Confirmed edits after which the user's previous clipboard could not be put back.
    clipboard_restore_failures: AtomicU64,
    retained_input: Mutex<Option<RetainedInput>>,
    recovery_armed: AtomicBool,
    // How long the last field check took, in milliseconds: the UI thread spaces the periodic
    // checks by it.
    privacy_probe_cost_ms: AtomicU64,
    // The worker's last observation of a full-screen foreground. The hook watchdog runs on the
    // hook thread and must not ask the shell itself.
    foreground_full_screen: AtomicBool,
}

impl Default for ObserverMetrics {
    fn default() -> Self {
        Self {
            candidates: AtomicU64::new(0),
            dropped_events: AtomicU64::new(0),
            input_epoch: AtomicU64::new(0),
            configuration_pending: AtomicBool::new(false),
            configuration_ack: AtomicU64::new(0),
            configuration_rejected: AtomicU64::new(0),
            last_input_layout: AtomicUsize::new(0),
            privacy_reason: AtomicU8::new(PRIVACY_UNAVAILABLE),
            auto_enabled: AtomicBool::new(false),
            safety_paused: AtomicBool::new(false),
            pause_break_undo: AtomicBool::new(true),
            force_hotkey_virtual_key: AtomicU32::new(u32::from(Hotkey::default().virtual_key)),
            force_hotkey_modifiers: AtomicU8::new(Hotkey::default().modifiers),
            undo_available: AtomicBool::new(false),
            undo_hotkey_active: AtomicBool::new(false),
            hotkey_waiting_for_release: AtomicBool::new(false),
            explicit_hotkey_sequence: AtomicU64::new(0),
            conversion_failures: AtomicU64::new(0),
            last_failure_reason: AtomicU8::new(ConversionFailureReason::None as u8),
            backend_status: AtomicU8::new(BACKEND_OBSERVE_ONLY),
            diagnostics_enabled: AtomicBool::new(false),
            dropped_diagnostics: AtomicU64::new(0),
            diagnostic_log_errors: AtomicU64::new(0),
            gate_replayed_events: AtomicU64::new(0),
            observed_input_sequence: AtomicU64::new(0),
            forwarded_input_sequence: AtomicU64::new(0),
            active_gate_token: AtomicU64::new(0),
            cancelled_gate_token: AtomicU64::new(0),
            privacy_refresh_queued: AtomicBool::new(false),
            profile_refresh_queued: AtomicBool::new(false),
            worker_busy: AtomicBool::new(false),
            edit_in_flight: AtomicBool::new(false),
            selection_left_token: AtomicU64::new(0),
            worker_dead: AtomicBool::new(false),
            gate_release_requests: AtomicU64::new(0),
            clipboard_restore_failures: AtomicU64::new(0),
            retained_input: Mutex::new(None),
            recovery_armed: AtomicBool::new(false),
            privacy_probe_cost_ms: AtomicU64::new(0),
            foreground_full_screen: AtomicBool::new(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ForegroundContext {
    hwnd: usize,
    focus: usize,
    input_thread_id: u32,
    process_id: u32,
    layout: usize,
}

/// What asked for a privacy check. Typing and the periodic refresh may keep a recent good verdict
/// through a late provider answer; recovery and manual conversion take every failure at face value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeTrigger {
    Typing,
    Refresh,
    Recovery,
    Manual,
}

impl ProbeTrigger {
    const fn label(self) -> &'static str {
        match self {
            Self::Typing => "typing",
            Self::Refresh => "refresh",
            Self::Recovery => "recovery",
            Self::Manual => "manual",
        }
    }

    const fn keeps_recent_ok(self) -> bool {
        matches!(self, Self::Typing | Self::Refresh)
    }
}

/// The last privacy check that found the input field fine, and for which input target. The
/// keyboard layout is not part of the target: the answer does not depend on it, and a conversion
/// switches it right before the next check.
#[derive(Debug, Clone, Copy)]
struct RecentPrivacyOk {
    foreground: ForegroundContext,
    epoch: u64,
    at: Instant,
}

/// What a privacy probe answered, before the context check and the recent-good rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProbeAnswer {
    reason: Option<PrivacyBlockReason>,
    source: &'static str,
}

/// A finished privacy check after the recent-good rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProbeVerdict {
    reason: Option<PrivacyBlockReason>,
    source: &'static str,
    /// Set when a failure was taken for a late answer: the age of the good check it leaned on.
    recent_ok_age: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
struct RawKeyEvent {
    message: u32,
    virtual_key: u32,
    scan_code: u32,
    caps_lock: bool,
    extended: bool,
    foreground: ForegroundContext,
    sequence: u64,
    drain_token: u64,
    hotkey_trigger: bool,
    /// A key-down of a key that was already down: an auto-repeat. Recorded only for edges a
    /// correction gate holds, where it decides which held key-ups must still be delivered.
    repeat: bool,
    /// The modifier keys physically held once this edge is applied, in the layout of
    /// `PHYSICAL_MODIFIER_KEYS`: the worker takes them over after a focus change or a new input
    /// epoch instead of assuming every modifier released.
    held_modifiers: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateFailureCause {
    Deadline,
    Capacity,
    FocusChanged,
    Mouse,
    ExternalInjection,
    WorkerQueue,
    ReplaySubmit,
    WorkerAck,
    DownstreamHook,
    InternalFault,
    HookReinstall,
    /// The worker's edit may have left the word selected, so the held keys would replace it.
    SelectionLeft,
}

#[derive(Debug, Clone)]
struct RetainedInput {
    token: u64,
    foreground: ForegroundContext,
    events: Vec<RawKeyEvent>,
    delivery_uncertain: bool,
    /// When the keys are deleted unless they were recovered first.
    expires_at: Instant,
    /// The worker is sending these keys right now; the lifetime timer leaves the record alone.
    recovering: bool,
    /// When the keys were retained.
    retained_at: Instant,
    /// The gate broke for a cause that hands its keys back without being asked (see
    /// `hands_held_keys_back_automatically`).
    auto_recover: bool,
    /// `AppState::typed_input` when the keys were retained.
    input_mark: u64,
}

struct CorrectionGate {
    active: bool,
    cancelled: bool,
    token: u64,
    fence_marker: usize,
    released_count: usize,
    current_drain_count: usize,
    drain_failed: bool,
    drain_in_flight: bool,
    awaiting_worker_ack: bool,
    handoff_budget_ms: u64,
    deadline: Instant,
    held: Vec<RawKeyEvent>,
    origin: Option<ForegroundContext>,
    pending_hotkey: Option<RawKeyEvent>,
}

impl Default for CorrectionGate {
    fn default() -> Self {
        Self {
            active: false,
            cancelled: false,
            token: 0,
            fence_marker: 0,
            released_count: 0,
            current_drain_count: 0,
            drain_failed: false,
            drain_in_flight: false,
            awaiting_worker_ack: false,
            handoff_budget_ms: 0,
            deadline: Instant::now(),
            held: Vec::with_capacity(CORRECTION_GATE_CAPACITY),
            origin: None,
            pending_hotkey: None,
        }
    }
}

impl CorrectionGate {
    fn defer_hotkey(&mut self, mut event: RawKeyEvent) {
        event.hotkey_trigger = true;
        self.pending_hotkey.get_or_insert(event);
    }

    fn take_pending_hotkey(&mut self, current: Option<ForegroundContext>) -> Option<RawKeyEvent> {
        let mut event = self.pending_hotkey.take()?;
        let current = current?;
        if !same_input_target(event.foreground, current) {
            return None;
        }
        event.foreground = current;
        event.drain_token = self.token;
        Some(event)
    }

    fn activate(&mut self, token: u64) {
        self.active = true;
        self.cancelled = false;
        self.token = token;
        self.fence_marker = GATE_FENCE_MARKER_BASE | (token as usize & 0x7fff);
        self.released_count = 0;
        self.current_drain_count = 0;
        self.drain_failed = false;
        self.drain_in_flight = false;
        self.awaiting_worker_ack = false;
        self.handoff_budget_ms = 0;
        self.held.clear();
        self.origin = None;
        self.pending_hotkey = None;
        self.deadline = Instant::now() + Duration::from_millis(CORRECTION_GATE_MAX_HOLD_MS);
    }

    fn hold(&mut self, event: RawKeyEvent) -> bool {
        if !self.active {
            return false;
        }
        if Instant::now() >= self.deadline || self.held.len() >= CORRECTION_GATE_CAPACITY {
            // Ownership stays here until the caller transfers every swallowed
            // edge to recovery. Never erase input in a capacity/deadline check.
            return false;
        }
        self.held.push(event);
        true
    }

    fn can_start_drain(&self, token: u64) -> bool {
        self.active
            && self.token == token
            && !self.cancelled
            && !self.drain_in_flight
            && !self.awaiting_worker_ack
    }

    /// Hands the gate to `new_token` when the budget prepared for it still fits before the
    /// deadline at `now`.
    fn handoff(&mut self, old_token: u64, new_token: u64, now: Instant) -> bool {
        if !self.active
            || self.cancelled
            || !self.awaiting_worker_ack
            || self.token != old_token
            || new_token == 0
            || self.handoff_budget_ms == 0
            || self.deadline.saturating_duration_since(now)
                < Duration::from_millis(self.handoff_budget_ms)
        {
            return false;
        }
        self.token = new_token;
        self.fence_marker = GATE_FENCE_MARKER_BASE | (new_token as usize & 0x7fff);
        self.released_count = 0;
        self.current_drain_count = 0;
        self.drain_failed = false;
        self.drain_in_flight = false;
        self.awaiting_worker_ack = false;
        self.handoff_budget_ms = 0;
        true
    }

    fn prepare_handoff_budget(&mut self, old_token: u64, required_ms: u64) -> bool {
        if !self.active
            || self.cancelled
            || !self.awaiting_worker_ack
            || self.token != old_token
            || required_ms == 0
            || required_ms > CORRECTION_GATE_MAX_HOLD_MS
        {
            return false;
        }
        self.handoff_budget_ms = required_ms;
        true
    }
}

#[derive(Debug, Clone, Copy)]
enum RawInputEvent {
    Key(RawKeyEvent),
    Mouse,
    ExternalInjection,
    PrivacyRefresh(ForegroundContext),
    /// Keeps the keyboard profile snapshot fresh on every timer tick, also while no privacy
    /// refresh runs (the shell in the foreground): without it the first key after six quiet
    /// seconds finds the snapshot expired and the word, undo and remembered words are dropped.
    ProfileRefresh,
    ReloadConfiguration,
    RecoverInput {
        sequence: u64,
        explicit: bool,
    },
    /// Held keys nobody recovered were deleted; automatic conversion may resume (policy D1).
    RetainedInputExpired,
    /// The UI thread sent `submitted` of the `requested` inputs of the retained record `token`
    /// past a stalled worker; the worker finishes that recovery.
    RetainedInputHandedBack {
        token: u64,
        submitted: usize,
        requested: usize,
    },
    GateDrainReady {
        token: u64,
        drained_count: usize,
        replay_succeeded: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DeferredDrainedConversion {
    gate_token: u64,
    boundary_sequence: u64,
}

#[derive(Debug, Clone)]
struct QueuedInputEvent {
    epoch: u64,
    captured_at: Instant,
    event: RawInputEvent,
    configuration: Option<Box<ConfigurationReload>>,
}

#[derive(Debug, Clone)]
struct ConfigurationReload {
    revision: u64,
    snapshot: RuntimeConfiguration,
}

/// What became of a configuration offered to the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigurationHandoff {
    Queued,
    /// Not now: a correction gate or retained keys, two revisions the worker has not acknowledged
    /// yet, or a full queue. These pass by themselves, so the same snapshot can be offered again.
    Busy,
    /// Never: the agent is stopping, the worker is gone, or the snapshot would take back a
    /// configuration source or a package generation.
    Refused,
}

/// Why an event did not reach the worker's queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueRefusal {
    Full,
    /// No queue or no worker behind it.
    Closed,
}

#[derive(Debug, Clone)]
struct PendingConversion {
    transaction: ConversionTransaction,
    foreground: ForegroundContext,
    source_layout: usize,
    profile_generation: u64,
    space_down_sequence: u64,
    replay_keys: Vec<ReplayKey>,
    forced: bool,
    // The hotkey's layout order for this word; empty for automatic conversions.
    cycle_order: Vec<Language>,
}

/// The most recent word, retained so the user can still convert it with the manual
/// hotkey: after the space has been typed (the delimiter), or after a manual layout
/// switch ended it with no delimiter. `foreground.layout` is the layout it was typed in.
#[derive(Clone)]
struct LastBoundary {
    replay_keys: Vec<ReplayKey>,
    delimiter: Option<char>,
    foreground: ForegroundContext,
    epoch: u64,
    profile_generation: u64,
}

/// What Pause knew about an input target when the agent stopped tracking it. Only Pause in a
/// field whose edit verifies the text before the caret uses it, so a stale word is refused there
/// instead of being typed over anything.
#[derive(Clone)]
struct RememberedWord {
    target: ForegroundContext,
    cycle: Option<TransposeCycle>,
    word: Option<LastBoundary>,
    at: Instant,
}

/// Forced-conversion cycle state: the manual hotkey walks the word through every
/// enabled layout, one press per layout, independent of dictionary membership.
#[derive(Clone)]
struct TransposeCycle {
    replay_keys: Vec<ReplayKey>,
    delimiter: Option<char>,
    foreground: ForegroundContext,
    language: Language,
    text: String,
    epoch: u64,
    profile_generation: u64,
    // The order the presses walk through, chosen at the first press: see `layout_cycle_order`.
    order: Vec<Language>,
}

#[derive(Debug, Clone)]
struct UndoRecord {
    transaction: ConversionTransaction,
    foreground: ForegroundContext,
    source_layout: usize,
    profile_generation: u64,
    replay_keys: Vec<ReplayKey>,
    edit_strategy: EditStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexiconOfferTarget {
    UserDictionary,
    WordExclusions,
}

#[derive(Debug, Clone)]
struct VolatileLexiconCandidate {
    target: LexiconOfferTarget,
    language: Language,
    word: String,
    expires_at: Instant,
}

impl VolatileLexiconCandidate {
    fn new(target: LexiconOfferTarget, language: Language, word: String, now: Instant) -> Self {
        Self {
            target,
            language,
            word,
            expires_at: now + Duration::from_secs(LEXICON_OFFER_SECONDS),
        }
    }

    fn is_valid_at(&self, now: Instant) -> bool {
        now <= self.expires_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReplayKey {
    scan_code: u16,
    shift: bool,
    caps_lock: bool,
    extended: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayAttempt {
    Applied,
    Cancelled,
    Failed,
    /// The target layout was not confirmed before any text input was sent, so
    /// the text is unchanged. Only perform_physical_edit reports this.
    LayoutSwitchFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextEditAttempt {
    Applied,
    /// The edit is confirmed; only the user's previous clipboard could not be restored.
    AppliedClipboardNotRestored,
    /// Nothing was typed: the field could not be inspected or selected.
    Unsupported,
    Cancelled,
    /// Input reached the field but the edit did not complete, for this reason.
    Failed(ConversionFailureReason),
    /// The edit did not go ahead and the agent's selection of the word could not be shown gone:
    /// keys typed now would replace the word.
    SelectionLeft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UndoHotkeyAction {
    PassThrough,
    Swallow,
    TriggerSwallow,
    TriggerPassThrough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhysicalReplayMode {
    Batch,
    Paced,
}

struct PhysicalReplayPlan {
    inputs: Vec<INPUT>,
    step_ends: Vec<usize>,
}

struct ReplayPacer(HANDLE);

impl ReplayPacer {
    fn new() -> Option<Self> {
        let handle = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .ok()?;
        Some(Self(handle))
    }

    fn wait(&self) {
        let started = Instant::now();
        let delay = Duration::from_millis(NOTEPAD_REPLAY_STEP_DELAY_MS);
        let delay_100ns = NOTEPAD_REPLAY_STEP_DELAY_MS
            .checked_mul(10_000)
            .and_then(|delay| i64::try_from(delay).ok())
            .and_then(i64::checked_neg);
        if let Some(due_time) = delay_100ns
            && unsafe { SetWaitableTimerEx(self.0, &due_time, 0, None, None, None, 0).is_ok() }
        {
            let _ = unsafe {
                WaitForSingleObject(
                    self.0,
                    u32::try_from(NOTEPAD_REPLAY_STEP_DELAY_MS + 100).unwrap_or(u32::MAX),
                )
            };
        }
        if let Some(remaining) = delay.checked_sub(started.elapsed()) {
            thread::sleep(remaining);
        }
    }
}

impl Drop for ReplayPacer {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct ClipboardOpenGuard;

impl ClipboardOpenGuard {
    fn open(owner: HWND, retries: usize) -> Option<Self> {
        for _ in 0..retries {
            if unsafe { OpenClipboard(Some(owner)).is_ok() } {
                return Some(Self);
            }
            thread::sleep(Duration::from_millis(CLIPBOARD_RETRY_DELAY_MS));
        }
        None
    }
}

impl Drop for ClipboardOpenGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

struct OwnedClipboardHandle(Option<HANDLE>);

impl OwnedClipboardHandle {
    fn new(handle: HANDLE) -> Option<Self> {
        (!handle.is_invalid()).then_some(Self(Some(handle)))
    }

    fn transfer(&mut self, format: u32) -> bool {
        let Some(handle) = self.0 else {
            return false;
        };
        if unsafe { SetClipboardData(format, Some(handle)).is_ok() } {
            self.0 = None;
            true
        } else {
            false
        }
    }
}

impl Drop for OwnedClipboardHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            unsafe {
                let _ = GlobalFree(Some(HGLOBAL(handle.0)));
            }
        }
    }
}

struct MaterializedClipboardEntry {
    format: FORMATETC,
    medium: STGMEDIUM,
}

impl Drop for MaterializedClipboardEntry {
    fn drop(&mut self) {
        unsafe {
            ReleaseStgMedium(&mut self.medium);
        }
    }
}

impl MaterializedClipboardEntry {
    fn transfer_to_open_clipboard(&mut self) -> bool {
        let handle = unsafe {
            match self.medium.tymed {
                value if value == TYMED_HGLOBAL.0 as u32 => HANDLE(self.medium.u.hGlobal.0),
                value if value == TYMED_GDI.0 as u32 => HANDLE(self.medium.u.hBitmap.0),
                value if value == TYMED_MFPICT.0 as u32 => HANDLE(self.medium.u.hMetaFilePict),
                value if value == TYMED_ENHMF.0 as u32 => HANDLE(self.medium.u.hEnhMetaFile.0),
                _ => return false,
            }
        };
        if unsafe { SetClipboardData(u32::from(self.format.cfFormat), Some(handle)).is_err() } {
            return false;
        }
        self.medium = STGMEDIUM::default();
        true
    }
}

fn materialize_current_clipboard() -> Option<Vec<MaterializedClipboardEntry>> {
    let source = unsafe { OleGetClipboard().ok()? };
    let enumerator = unsafe { source.EnumFormatEtc(DATADIR_GET.0 as u32).ok()? };
    let mut entries = Vec::new();
    loop {
        let mut format = FORMATETC::default();
        let mut fetched = 0u32;
        let result =
            unsafe { enumerator.Next(core::slice::from_mut(&mut format), Some(&mut fetched)) };
        if result == HRESULT(1) {
            break;
        }
        if result.is_err() || fetched != 1 {
            return None;
        }
        if !format.ptd.is_null() {
            unsafe {
                CoTaskMemFree(Some(format.ptd.cast()));
            }
            return None;
        }
        format.tymed = clipboard_tymed_for_format(format.cfFormat);
        let medium = unsafe { source.GetData(&format).ok()? };
        if medium.tymed != format.tymed || stgmedium_has_custom_releaser(&medium) {
            let mut medium = medium;
            unsafe {
                ReleaseStgMedium(&mut medium);
            }
            return None;
        }
        format.tymed = medium.tymed;
        entries.push(MaterializedClipboardEntry { format, medium });
    }
    Some(entries)
}

const fn clipboard_tymed_for_format(format: u16) -> u32 {
    match format as u32 {
        2 | 9 | 0x0082 => TYMED_GDI.0 as u32,
        3 | 0x0083 => TYMED_MFPICT.0 as u32,
        14 | 0x008e => TYMED_ENHMF.0 as u32,
        _ => TYMED_HGLOBAL.0 as u32,
    }
}

fn stgmedium_has_custom_releaser(medium: &STGMEDIUM) -> bool {
    let release = unsafe {
        &*((&medium.pUnkForRelease
            as *const core::mem::ManuallyDrop<Option<windows::core::IUnknown>>)
            .cast::<Option<windows::core::IUnknown>>())
    };
    release.is_some()
}

#[derive(Debug, Clone, Copy)]
enum ClipboardBrokerCommand {
    /// Put the user's clipboard back, if the clipboard still holds the replacement.
    Restore,
    /// Leave the replacement there and drop the copy of the user's clipboard.
    Abandon,
    /// The Ctrl+V was sent but not confirmed: leave the replacement until `process`, the paste's
    /// target, has read it, then put the user's clipboard back (`TARGET_READ_GRACE_MS` after the
    /// read); leave it for good after `wait`.
    Defer { process: u32, wait: Duration },
}

/// What a broker found while waiting for the worker.
#[derive(Debug, Clone, Copy)]
enum BrokerWait {
    Command(ClipboardBrokerCommand),
    Timeout,
    Disconnected,
}

#[derive(Debug, Clone, Copy)]
enum ClipboardBrokerResponse {
    Installed { sequence: u32, owner: usize },
    Finished(bool),
    Failed,
}

/// How a clipboard broker ended; logged for a broker the worker stopped waiting for.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrokerEnd {
    Running = 0,
    /// The user's clipboard was never changed.
    Untouched = 1,
    /// The user's clipboard was put back.
    Restored = 2,
    /// The user's clipboard could not be put back, or another program had replaced the
    /// replacement meanwhile.
    NotRestored = 3,
    /// The replacement was left there on purpose: its paste was not confirmed.
    Kept = 4,
}

impl BrokerEnd {
    const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Untouched,
            2 => Self::Restored,
            3 => Self::NotRestored,
            4 => Self::Kept,
            _ => Self::Running,
        }
    }
}

/// What the worker and its clipboard broker share besides their channels.
#[derive(Default)]
struct BrokerShared {
    /// Set when the worker stopped waiting for the installation: the broker must not put the
    /// replacement on the clipboard any more.
    cancelled: AtomicBool,
    /// How the broker ended (`BrokerEnd`).
    end: AtomicU8,
}

impl BrokerShared {
    fn finish(&self, end: BrokerEnd) {
        self.end.store(end as u8, Ordering::Release);
    }

    fn end(&self) -> BrokerEnd {
        BrokerEnd::from_code(self.end.load(Ordering::Acquire))
    }
}

/// Clipboard brokers the worker stopped waiting for: an installation or a restore that took too
/// long. Only one broker runs at a time, so no paste starts while one of these still runs, and
/// each one's end is logged once it is over.
#[derive(Default)]
struct ClipboardBrokers {
    lingering: RefCell<Vec<(JoinHandle<()>, Arc<BrokerShared>)>>,
}

impl ClipboardBrokers {
    fn busy(&self) -> bool {
        self.lingering
            .borrow()
            .iter()
            .any(|(worker, _)| !worker.is_finished())
    }

    fn linger(&self, worker: JoinHandle<()>, shared: Arc<BrokerShared>) {
        self.lingering.borrow_mut().push((worker, shared));
    }

    /// Joins the brokers that are over and says how each one ended.
    fn reap(&self) -> Vec<BrokerEnd> {
        let mut lingering = self.lingering.borrow_mut();
        let mut ended = Vec::new();
        let mut index = 0;
        while index < lingering.len() {
            if lingering[index].0.is_finished() {
                let (worker, shared) = lingering.swap_remove(index);
                let _ = worker.join();
                ended.push(shared.end());
            } else {
                index += 1;
            }
        }
        ended
    }
}

/// What an installation did to the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallOutcome {
    /// Nothing changed: the clipboard could not be opened, it changed since the copy was taken,
    /// the worker stopped waiting, or the data could not be prepared.
    Untouched,
    /// The clipboard was emptied, but the replacement could not be put there whole.
    Emptied {
        sequence: u32,
    },
    Installed {
        sequence: u32,
    },
}

/// The clipboard calls of a broker, made on its own thread. `SystemClipboard` makes them on the
/// system clipboard; tests script them.
trait ClipboardSystem {
    type Snapshot;
    /// The clipboard's sequence number.
    fn sequence(&self) -> u32;
    /// The broker's clipboard owner, as the worker compares it.
    fn owner(&self) -> usize;
    /// Whether the broker owns the clipboard.
    fn owned(&self) -> bool;
    /// Renders and copies every format of the current clipboard.
    fn snapshot(&mut self) -> Option<Self::Snapshot>;
    /// Puts `text` on the clipboard with the history, cloud and monitoring opt-outs, unless the
    /// clipboard changed since its sequence was `expected` or the worker stopped waiting.
    fn install(&mut self, text: &str, expected: u32, cancelled: &AtomicBool) -> InstallOutcome;
    /// Puts `snapshot` back, with `expected` only while the clipboard still holds the broker's
    /// item of that sequence, and adds the opt-outs the snapshot lacks.
    fn restore(&mut self, expected: Option<u32>, snapshot: &mut Self::Snapshot) -> bool;
    /// Waits for the worker's command, or without `commands` only lets time pass, at most
    /// `timeout` when one is given. Requests to render the replacement are served meanwhile.
    fn wait(
        &mut self,
        commands: Option<&Receiver<ClipboardBrokerCommand>>,
        timeout: Option<Duration>,
    ) -> BrokerWait;
    /// Whether `process` has read the replacement since it was put on the clipboard.
    fn read_by(&self, process: u32) -> bool;
}

/// The broker's work: copy the user's clipboard, put the replacement there, report it, and on
/// the worker's command put the copy back or leave the replacement. The clipboard is changed only
/// if it did not change since the copy was taken; an installation that emptied the clipboard and
/// failed is undone at once, and only such a one.
fn run_clipboard_broker<S: ClipboardSystem>(
    system: &mut S,
    text: &str,
    shared: &BrokerShared,
    commands: &Receiver<ClipboardBrokerCommand>,
    responses: &SyncSender<ClipboardBrokerResponse>,
) {
    let before = system.sequence();
    let Some(mut snapshot) = system.snapshot() else {
        shared.finish(BrokerEnd::Untouched);
        let _ = responses.send(ClipboardBrokerResponse::Failed);
        return;
    };
    let sequence = match system.install(text, before, &shared.cancelled) {
        InstallOutcome::Installed { sequence } => sequence,
        InstallOutcome::Untouched => {
            shared.finish(BrokerEnd::Untouched);
            let _ = responses.send(ClipboardBrokerResponse::Failed);
            return;
        }
        InstallOutcome::Emptied { sequence } => {
            let end = if system.restore(Some(sequence), &mut snapshot) {
                BrokerEnd::Restored
            } else {
                BrokerEnd::NotRestored
            };
            shared.finish(end);
            let _ = responses.send(ClipboardBrokerResponse::Failed);
            return;
        }
    };
    // A worker that stopped waiting has already sent its command; it is read below.
    let _ = responses.send(ClipboardBrokerResponse::Installed {
        sequence,
        owner: system.owner(),
    });
    let mut waited = system.wait(Some(commands), None);
    let end = loop {
        waited = match waited {
            BrokerWait::Command(ClipboardBrokerCommand::Restore) => {
                break restore_if_current(system, sequence, &mut snapshot);
            }
            BrokerWait::Command(ClipboardBrokerCommand::Defer { process, wait }) => {
                match wait_for_reader(system, commands, sequence, process, wait) {
                    ReaderWait::Read => break restore_if_current(system, sequence, &mut snapshot),
                    ReaderWait::Replaced => break BrokerEnd::NotRestored,
                    ReaderWait::Expired => break BrokerEnd::Kept,
                    ReaderWait::Command(command) => BrokerWait::Command(command),
                }
            }
            BrokerWait::Command(ClipboardBrokerCommand::Abandon)
            | BrokerWait::Timeout
            | BrokerWait::Disconnected => break BrokerEnd::Kept,
        };
    };
    shared.finish(end);
    let _ = responses.send(ClipboardBrokerResponse::Finished(
        end == BrokerEnd::Restored,
    ));
}

/// Puts the user's clipboard back while the clipboard still holds the broker's replacement.
fn restore_if_current<S: ClipboardSystem>(
    system: &mut S,
    sequence: u32,
    snapshot: &mut S::Snapshot,
) -> BrokerEnd {
    if system.sequence() != sequence || !system.owned() {
        return BrokerEnd::NotRestored;
    }
    if system.restore(Some(sequence), snapshot) {
        BrokerEnd::Restored
    } else {
        BrokerEnd::NotRestored
    }
}

/// How the wait for a paste's target to read the replacement ended.
enum ReaderWait {
    Read,
    /// Another program replaced the replacement: theirs stays.
    Replaced,
    Expired,
    /// The worker sent a command meanwhile.
    Command(ClipboardBrokerCommand),
}

/// Waits up to `wait` for `process` to read the replacement, and `TARGET_READ_GRACE_MS` more once
/// it did. Requests to render it are served meanwhile, also once the worker no longer listens.
fn wait_for_reader<S: ClipboardSystem>(
    system: &mut S,
    commands: &Receiver<ClipboardBrokerCommand>,
    sequence: u32,
    process: u32,
    wait: Duration,
) -> ReaderWait {
    let mut deadline = Instant::now() + wait;
    let mut read = false;
    let mut commands = Some(commands);
    loop {
        if system.sequence() != sequence || !system.owned() {
            return ReaderWait::Replaced;
        }
        let now = Instant::now();
        if !read && system.read_by(process) {
            read = true;
            deadline = now + Duration::from_millis(TARGET_READ_GRACE_MS);
        }
        if now >= deadline {
            return if read {
                ReaderWait::Read
            } else {
                ReaderWait::Expired
            };
        }
        let slice = (deadline - now).min(Duration::from_millis(10));
        match system.wait(commands, Some(slice)) {
            BrokerWait::Timeout => {}
            BrokerWait::Command(command) => return ReaderWait::Command(command),
            BrokerWait::Disconnected => commands = None,
        }
    }
}

fn spawn_clipboard_broker<S, F>(
    text: String,
    make: F,
    shared: Arc<BrokerShared>,
    commands: Receiver<ClipboardBrokerCommand>,
    responses: SyncSender<ClipboardBrokerResponse>,
) -> JoinHandle<()>
where
    S: ClipboardSystem,
    F: FnOnce() -> Option<S> + Send + 'static,
{
    thread::spawn(move || {
        let Some(mut system) = make() else {
            shared.finish(BrokerEnd::Untouched);
            let _ = responses.send(ClipboardBrokerResponse::Failed);
            return;
        };
        run_clipboard_broker(&mut system, &text, &shared, &commands, &responses);
    })
}

/// Why a protected paste could not put its replacement on the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardBeginFailure {
    /// A broker the worker stopped waiting for still runs.
    Busy,
    /// The replacement was not on the clipboard in time. The broker does not put it there any
    /// more, and puts the user's clipboard back if it already had.
    NotInstalled,
}

/// The replacement on the clipboard for one paste. The user's clipboard is put back (`restore`)
/// once the paste is confirmed. Once the Ctrl+V was sent and not confirmed (`defer`), the
/// replacement stays until the paste's target has read it: a field that handles the paste late
/// then gets the replacement, never the user's own clipboard, which comes back afterwards.
/// Dropping it restores before the Ctrl+V was sent and defers after.
struct ProtectedClipboard<'a> {
    owner: usize,
    temporary_sequence: u32,
    /// The process the paste goes to, and how long a paste that was not confirmed waits for it.
    target_process: u32,
    deferred_wait: Duration,
    command: SyncSender<ClipboardBrokerCommand>,
    response: Receiver<ClipboardBrokerResponse>,
    worker: Option<JoinHandle<()>>,
    shared: Arc<BrokerShared>,
    brokers: &'a ClipboardBrokers,
    /// Reads the clipboard's sequence number and owner on the worker's side.
    probe: Box<dyn Fn() -> (u32, usize)>,
    active: bool,
    submitted: bool,
}

impl<'a> ProtectedClipboard<'a> {
    fn begin(
        text: &str,
        install_timeout: Duration,
        brokers: &'a ClipboardBrokers,
        target_process: u32,
    ) -> core::result::Result<Self, ClipboardBeginFailure> {
        Self::begin_with(
            text,
            install_timeout,
            brokers,
            (
                target_process,
                Duration::from_millis(UNCONFIRMED_PASTE_WAIT_MS),
            ),
            SystemClipboard::new,
            Box::new(system_clipboard_state),
        )
    }

    /// `target`: the paste's target process, and how long a paste that was not confirmed waits
    /// for it to read the replacement.
    fn begin_with<S, F>(
        text: &str,
        install_timeout: Duration,
        brokers: &'a ClipboardBrokers,
        target: (u32, Duration),
        make: F,
        probe: Box<dyn Fn() -> (u32, usize)>,
    ) -> core::result::Result<Self, ClipboardBeginFailure>
    where
        S: ClipboardSystem,
        F: FnOnce() -> Option<S> + Send + 'static,
    {
        if brokers.busy() {
            return Err(ClipboardBeginFailure::Busy);
        }
        let (command_sender, command_receiver) = sync_channel(1);
        let (response_sender, response_receiver) = sync_channel(1);
        let shared = Arc::new(BrokerShared::default());
        let worker = spawn_clipboard_broker(
            text.to_owned(),
            make,
            Arc::clone(&shared),
            command_receiver,
            response_sender,
        );
        match response_receiver.recv_timeout(install_timeout) {
            Ok(ClipboardBrokerResponse::Installed { sequence, owner }) => Ok(Self {
                owner,
                temporary_sequence: sequence,
                target_process: target.0,
                deferred_wait: target.1,
                command: command_sender,
                response: response_receiver,
                worker: Some(worker),
                shared,
                brokers,
                probe,
                active: true,
                submitted: false,
            }),
            _ => {
                shared.cancelled.store(true, Ordering::Release);
                let _ = command_sender.try_send(ClipboardBrokerCommand::Restore);
                brokers.linger(worker, shared);
                Err(ClipboardBeginFailure::NotInstalled)
            }
        }
    }

    fn is_current(&self) -> bool {
        self.active && (self.probe)() == (self.temporary_sequence, self.owner)
    }

    /// The Ctrl+V is about to be sent: from now on the replacement stays until the paste was
    /// confirmed (`restore`) or its target read it (`defer`).
    fn mark_submitted(&mut self) {
        self.submitted = true;
    }

    /// Puts the user's clipboard back if the clipboard still holds the replacement. Waits
    /// `CLIPBOARD_RESTORE_WAIT_MS` for the broker; a restore that takes longer finishes on its
    /// own and is logged when it is over.
    fn restore(&mut self) -> bool {
        if !self.active {
            return true;
        }
        self.active = false;
        if (self.probe)() == (self.temporary_sequence, self.owner) {
            self.finish(ClipboardBrokerCommand::Restore)
        } else {
            // Another program replaced the replacement: theirs stays, and the broker ends now.
            let _ = self.finish(ClipboardBrokerCommand::Abandon);
            false
        }
    }

    /// The Ctrl+V was sent and not confirmed: the broker leaves the replacement until the paste's
    /// target has read it and then puts the user's clipboard back, or leaves it for good after
    /// the deferred wait. The worker does not wait for that; the broker's end is logged.
    fn defer(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let _ = self.command.send(ClipboardBrokerCommand::Defer {
            process: self.target_process,
            wait: self.deferred_wait,
        });
        if let Some(worker) = self.worker.take() {
            self.brokers.linger(worker, Arc::clone(&self.shared));
        }
    }

    /// Sends the last command and waits for the broker's answer; a broker that does not answer in
    /// time is left to finish and logged later. True when the user's clipboard was put back.
    fn finish(&mut self, command: ClipboardBrokerCommand) -> bool {
        let response = if self.command.send(command).is_ok() {
            self.response
                .recv_timeout(Duration::from_millis(CLIPBOARD_RESTORE_WAIT_MS))
                .ok()
        } else {
            None
        };
        if let Some(worker) = self.worker.take() {
            if response.is_some() {
                let _ = worker.join();
            } else {
                self.brokers.linger(worker, Arc::clone(&self.shared));
            }
        }
        matches!(response, Some(ClipboardBrokerResponse::Finished(true)))
    }
}

impl Drop for ProtectedClipboard<'_> {
    fn drop(&mut self) {
        if self.submitted {
            self.defer();
        } else {
            let _ = self.restore();
        }
    }
}

/// The clipboard's sequence number and owner, as the worker compares them with the broker's.
fn system_clipboard_state() -> (u32, usize) {
    unsafe {
        (
            GetClipboardSequenceNumber(),
            GetClipboardOwner().map_or(0, |owner| owner.0 as usize),
        )
    }
}

thread_local! {
    /// On a clipboard broker's thread: the replacement it renders on request, and the processes
    /// that asked for it.
    static BROKER_RENDER: RefCell<Option<BrokerRender>> = const { RefCell::new(None) };
}

struct BrokerRender {
    text: String,
    readers: Vec<u32>,
}

const CLIPBOARD_BROKER_CLASS: PCWSTR = w!("AutoKeyboardLayotClipboardBroker");

/// The broker window's procedure. The replacement's text is rendered when a program asks for it,
/// and the process of the window holding the clipboard open is noted as a reader; a broker window
/// that goes away renders it first, so a replacement left on the clipboard stays readable.
unsafe extern "system" fn clipboard_broker_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    guarded(
        || match message {
            WM_RENDERFORMAT => {
                if wparam.0 == usize::from(CF_UNICODETEXT.0) {
                    render_replacement(true);
                }
                LRESULT(0)
            }
            WM_RENDERALLFORMATS => {
                if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
                    if unsafe { GetClipboardOwner().ok() } == Some(hwnd) {
                        render_replacement(false);
                    }
                    let _ = unsafe { CloseClipboard() };
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        },
        || LRESULT(0),
    )
}

fn forget_replacement() {
    BROKER_RENDER.with(|render| *render.borrow_mut() = None);
}

/// Puts the replacement's text on the clipboard for a delayed-rendering request; when a program
/// asked for it, the process of the window holding the clipboard open is noted as a reader.
fn render_replacement(requested: bool) {
    BROKER_RENDER.with(|render| {
        let mut render = render.borrow_mut();
        let Some(render) = render.as_mut() else {
            return;
        };
        if requested {
            let mut process = 0;
            if let Ok(reader) = unsafe { GetOpenClipboardWindow() } {
                unsafe {
                    GetWindowThreadProcessId(reader, Some(&mut process));
                }
            }
            if process != 0 {
                render.readers.push(process);
            }
        }
        if let Some(mut data) = global_utf16(&render.text) {
            let _ = data.transfer(u32::from(CF_UNICODETEXT.0));
        }
    });
}

struct ClipboardBrokerWindow(HWND);

impl ClipboardBrokerWindow {
    fn new() -> Option<Self> {
        let module = unsafe { GetModuleHandleW(None).ok()? };
        let class = WNDCLASSW {
            lpfnWndProc: Some(clipboard_broker_procedure),
            hInstance: HINSTANCE(module.0),
            lpszClassName: CLIPBOARD_BROKER_CLASS,
            ..Default::default()
        };
        // Fails harmlessly once the class exists.
        unsafe {
            RegisterClassW(&class);
        }
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLIPBOARD_BROKER_CLASS,
                w!("AutoKeyboardLayot clipboard broker"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
            .ok()?
        };
        Some(Self(window))
    }
}

impl Drop for ClipboardBrokerWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

/// The system clipboard for a broker thread: OLE is initialized on that thread, and a hidden
/// window owns the clipboard while the replacement is on it.
struct SystemClipboard {
    owner: ClipboardBrokerWindow,
}

impl SystemClipboard {
    fn new() -> Option<Self> {
        if unsafe { OleInitialize(None).is_err() } {
            return None;
        }
        let Some(owner) = ClipboardBrokerWindow::new() else {
            unsafe {
                OleUninitialize();
            }
            return None;
        };
        Some(Self { owner })
    }
}

impl Drop for SystemClipboard {
    fn drop(&mut self) {
        unsafe {
            OleUninitialize();
        }
    }
}

impl ClipboardSystem for SystemClipboard {
    type Snapshot = Vec<MaterializedClipboardEntry>;

    fn sequence(&self) -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    fn owner(&self) -> usize {
        self.owner.0.0 as usize
    }

    fn owned(&self) -> bool {
        (unsafe { GetClipboardOwner().ok() }) == Some(self.owner.0)
    }

    fn snapshot(&mut self) -> Option<Self::Snapshot> {
        materialize_current_clipboard()
    }

    fn install(&mut self, text: &str, expected: u32, cancelled: &AtomicBool) -> InstallOutcome {
        install_protected_clipboard(self.owner.0, text, expected, cancelled)
    }

    fn restore(&mut self, expected: Option<u32>, snapshot: &mut Self::Snapshot) -> bool {
        // From here the replacement is not the broker's to render any more, not even when its
        // window, the owner of the restored clipboard, goes away.
        forget_replacement();
        restore_materialized_clipboard(self.owner.0, expected, snapshot)
    }

    fn wait(
        &mut self,
        commands: Option<&Receiver<ClipboardBrokerCommand>>,
        timeout: Option<Duration>,
    ) -> BrokerWait {
        wait_clipboard_broker_command(commands, timeout)
    }

    fn read_by(&self, process: u32) -> bool {
        BROKER_RENDER.with(|render| {
            render
                .borrow()
                .as_ref()
                .is_some_and(|render| render.readers.contains(&process))
        })
    }
}

fn wait_clipboard_broker_command(
    commands: Option<&Receiver<ClipboardBrokerCommand>>,
    timeout: Option<Duration>,
) -> BrokerWait {
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    loop {
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if let Some(commands) = commands {
            match commands.try_recv() {
                Ok(command) => return BrokerWait::Command(command),
                Err(TryRecvError::Disconnected) => return BrokerWait::Disconnected,
                Err(TryRecvError::Empty) => {}
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return BrokerWait::Timeout;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// Announces `format` for delayed rendering. The call returns no handle even when it succeeds.
fn announce_delayed_clipboard_format(format: u32) -> bool {
    unsafe { SetClipboardData(format, None) }
        .map_or_else(|error| error.code() == HRESULT(0), |_| true)
}

/// The formats that keep an item out of clipboard history, cloud sync and clipboard managers:
/// `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory` and
/// `CanUploadToCloudClipboard`.
fn clipboard_opt_out_formats() -> Option<[u32; 3]> {
    let formats = unsafe {
        [
            RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
            RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
            RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")),
        ]
    };
    formats.iter().all(|format| *format != 0).then_some(formats)
}

/// The opt-out formats a restored item does not carry itself.
fn missing_opt_out_formats(present: &[u32], opt_outs: [u32; 3]) -> Vec<u32> {
    opt_outs
        .into_iter()
        .filter(|format| !present.contains(format))
        .collect()
}

fn install_protected_clipboard(
    owner: HWND,
    text: &str,
    expected: u32,
    cancelled: &AtomicBool,
) -> InstallOutcome {
    let installed = {
        let Some(_open) = ClipboardOpenGuard::open(owner, CLIPBOARD_OPEN_RETRIES) else {
            return InstallOutcome::Untouched;
        };
        // Checked with the clipboard open, so that nothing can change it before it is emptied:
        // what another program put there after the copy was taken is never replaced.
        if cancelled.load(Ordering::Acquire) || unsafe { GetClipboardSequenceNumber() } != expected
        {
            return InstallOutcome::Untouched;
        }
        let Some(opt_outs) = clipboard_opt_out_formats() else {
            return InstallOutcome::Untouched;
        };
        let mut opt_out_data = Vec::with_capacity(opt_outs.len());
        for _ in opt_outs {
            let Some(data) = global_bytes(&0u32.to_ne_bytes()) else {
                return InstallOutcome::Untouched;
            };
            opt_out_data.push(data);
        }
        // The text itself is rendered when a program asks for it (`clipboard_broker_procedure`),
        // so the broker learns whether the paste's target read it.
        BROKER_RENDER.with(|render| {
            *render.borrow_mut() = Some(BrokerRender {
                text: text.to_owned(),
                readers: Vec::new(),
            });
        });
        if unsafe { EmptyClipboard().is_err() } {
            return InstallOutcome::Untouched;
        }
        // Emptying waits for the previous owner to let go, which a hung program delays: a worker
        // that stopped waiting meanwhile gets the user's clipboard back instead of the replacement.
        !cancelled.load(Ordering::Acquire)
            && opt_out_data
                .iter_mut()
                .zip(opt_outs)
                .all(|(data, format)| data.transfer(format))
            && announce_delayed_clipboard_format(u32::from(CF_UNICODETEXT.0))
    };
    let sequence = unsafe { GetClipboardSequenceNumber() };
    if installed {
        InstallOutcome::Installed { sequence }
    } else {
        InstallOutcome::Emptied { sequence }
    }
}

fn global_bytes(bytes: &[u8]) -> Option<OwnedClipboardHandle> {
    if bytes.is_empty() {
        return None;
    }
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()? };
    let destination = unsafe { GlobalLock(memory) }.cast::<u8>();
    if destination.is_null() {
        unsafe {
            let _ = GlobalFree(Some(memory));
        }
        return None;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
        let _ = GlobalUnlock(memory);
    }
    OwnedClipboardHandle::new(HANDLE(memory.0))
}

fn global_utf16(text: &str) -> Option<OwnedClipboardHandle> {
    let units = text
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect::<Vec<_>>();
    let bytes = unsafe {
        core::slice::from_raw_parts(units.as_ptr().cast::<u8>(), units.len().checked_mul(2)?)
    };
    global_bytes(bytes)
}

fn restore_materialized_clipboard(
    owner: HWND,
    expected_sequence: Option<u32>,
    entries: &mut [MaterializedClipboardEntry],
) -> bool {
    let Some(_open) = ClipboardOpenGuard::open(owner, CLIPBOARD_RESTORE_OPEN_RETRIES) else {
        return false;
    };
    if expected_sequence.is_some_and(|sequence| {
        (unsafe { GetClipboardSequenceNumber() }) != sequence
            || (unsafe { GetClipboardOwner().ok() }) != Some(owner)
    }) || unsafe { EmptyClipboard().is_err() }
    {
        return false;
    }
    let present: Vec<u32> = entries
        .iter()
        .map(|entry| u32::from(entry.format.cfFormat))
        .collect();
    let restored = entries
        .iter_mut()
        .all(MaterializedClipboardEntry::transfer_to_open_clipboard);
    // The user's item comes back as it was, not as a new copy for clipboard history, cloud sync
    // or clipboard managers: the opt-outs it did not carry itself are added.
    if let Some(opt_outs) = clipboard_opt_out_formats() {
        for format in missing_opt_out_formats(&present, opt_outs) {
            if let Some(mut data) = global_bytes(&0u32.to_ne_bytes()) {
                let _ = data.transfer(format);
            }
        }
    }
    restored
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConversionFailureReason {
    None = 0,
    SourceBarrier = 1,
    LayoutUnavailable = 2,
    PhysicalEdit = 3,
    TextCommit = 4,
    ReplayCommit = 5,
    GateDrain = 6,
    ClipboardEdit = 7,
    InternalFault = 8,
    SelectionLeft = 9,
    /// The replacement was pasted, but the keyboard layout did not switch after it.
    LayoutSwitch = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditStrategy {
    ProtectedPaste,
    PhysicalReplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextEditBackend {
    ProtectedPaste,
    PhysicalReplay,
    CapabilityProbe,
    ObserveOnly,
}

impl TextEditBackend {
    const fn requires_text_barrier(self) -> bool {
        matches!(self, Self::ProtectedPaste)
    }

    const fn status_code(self) -> u8 {
        match self {
            Self::ProtectedPaste => BACKEND_PROTECTED_PASTE,
            Self::PhysicalReplay => BACKEND_PHYSICAL_REPLAY,
            Self::CapabilityProbe => BACKEND_CAPABILITY_PROBE,
            Self::ObserveOnly => BACKEND_OBSERVE_ONLY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextBarrierStatus {
    Match,
    Mismatch,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum BarrierCharacterClass {
    #[default]
    None,
    Space,
    NonBreakingSpace,
    LineBreak,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TextBarrierStage {
    /// Not reached.
    #[default]
    None,
    /// The text is empty or longer than the barrier checks.
    TextLength,
    Resolver,
    /// The selection could not be read.
    Selection,
    /// The selection is not exactly one range.
    SelectionRanges,
    /// The only selection range could not be read.
    Caret,
    /// Text is selected instead of an empty caret.
    CaretNotEmpty,
    /// The caret range could not be copied.
    RangeClone,
    RangeMove,
    TextRead,
    TextCompared,
    SelectCall,
    SelectionConfirm,
    /// The barrier's provider thread did not answer by the deadline, or was still busy with an
    /// earlier read.
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum EditableResolverReason {
    #[default]
    None,
    AutomationUnavailable,
    FocusUnavailable,
    WalkerUnavailable,
    ProcessUnavailable,
    Password,
    PasswordPropertyUnavailable,
    ParentUnavailable,
    NoEditableAncestor,
    OwnershipUnproven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TextBarrierReport {
    stage: TextBarrierStage,
    expected_chars: usize,
    actual_chars: usize,
    moved_units: i32,
    exact: bool,
    trailing_nbsp_equivalent: bool,
    word_at_start: bool,
    word_at_end: bool,
    actual_without_first_is_word: bool,
    actual_without_last_is_word: bool,
    leading: BarrierCharacterClass,
    trailing: BarrierCharacterClass,
    resolver_depth: usize,
    resolver_reason: EditableResolverReason,
    resolver_process_id: u32,
    /// How the selection was confirmed, when one was made.
    confirmation: Option<SelectionConfirmation>,
    /// How many selection attempts ran.
    attempts: u32,
    /// What became of the agent's selection when it was collapsed again.
    cleanup: Option<SelectionCleanup>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditableResolverAction {
    Continue,
    ReturnCurrent,
}

const fn editable_resolver_action(exact_process: bool, editable: bool) -> EditableResolverAction {
    if exact_process && editable {
        EditableResolverAction::ReturnCurrent
    } else {
        EditableResolverAction::Continue
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilityDecision {
    ProtectedPaste,
    PhysicalReplay,
    CancelMismatch,
    Unsupported,
}

const fn capability_decision(
    barrier: TextBarrierStatus,
    physical_fallback_allowed: bool,
) -> CapabilityDecision {
    match barrier {
        TextBarrierStatus::Match => CapabilityDecision::ProtectedPaste,
        TextBarrierStatus::Mismatch => CapabilityDecision::CancelMismatch,
        TextBarrierStatus::Unavailable if physical_fallback_allowed => {
            CapabilityDecision::PhysicalReplay
        }
        TextBarrierStatus::Unavailable => CapabilityDecision::Unsupported,
    }
}

const fn physical_replay_fallback_is_safe(reason: EditableResolverReason) -> bool {
    matches!(
        reason,
        EditableResolverReason::WalkerUnavailable
            | EditableResolverReason::ParentUnavailable
            | EditableResolverReason::NoEditableAncestor
            | EditableResolverReason::OwnershipUnproven
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayoutIndicator {
    English,
    Russian,
    Estonian,
    Japanese,
    Other(u16),
    Unavailable,
}

impl LayoutIndicator {
    fn from_layout(layout: usize) -> Self {
        let language_id = layout as u16;
        match language_id & 0x03ff {
            0x09 => Self::English,
            0x19 => Self::Russian,
            0x25 => Self::Estonian,
            0x11 => Self::Japanese,
            _ if language_id == 0 => Self::Unavailable,
            _ => Self::Other(language_id),
        }
    }

    const fn icon_text(self) -> &'static str {
        match self {
            Self::English => "EN",
            Self::Russian => "RU",
            Self::Estonian => "ET",
            Self::Japanese => "JA",
            Self::Other(_) | Self::Unavailable => "??",
        }
    }

    fn tooltip_code(self) -> String {
        match self {
            Self::English => "ENG".to_owned(),
            Self::Russian => "RUS".to_owned(),
            Self::Estonian => "EST".to_owned(),
            Self::Japanese => "JPN".to_owned(),
            Self::Other(language_id) => format!("0x{language_id:04X}"),
            Self::Unavailable => "Unavailable".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrayStatus {
    indicator: LayoutIndicator,
    ui_revision: u64,
    candidates: u64,
    dropped: u64,
    privacy_reason: u8,
    auto_enabled: bool,
    safety_paused: bool,
    undo_available: bool,
    failures: u64,
    failure_reason: u8,
    backend_status: u8,
    // Diagnostics records dropped from a full queue or not written, and failed rotations.
    diagnostic_errors: u64,
}

impl TrayStatus {
    const fn initial(indicator: LayoutIndicator) -> Self {
        Self {
            indicator,
            ui_revision: 0,
            candidates: 0,
            dropped: 0,
            privacy_reason: PRIVACY_UNAVAILABLE,
            auto_enabled: false,
            safety_paused: false,
            undo_available: false,
            failures: 0,
            failure_reason: ConversionFailureReason::None as u8,
            backend_status: BACKEND_OBSERVE_ONLY,
            diagnostic_errors: 0,
        }
    }
}

struct AppState {
    window: HWND,
    tray: Option<TrayIcon>,
    keyboard_hook: Option<HHOOK>,
    mouse_hook: Option<HHOOK>,
    input_sender: Option<SyncSender<QueuedInputEvent>>,
    worker: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    metrics: Arc<ObserverMetrics>,
    correction_gate: CorrectionGate,
    last_tray_status: TrayStatus,
    next_privacy_refresh: Instant,
    menu_open: bool,
    settings: Settings,
    next_configuration_revision: u64,
    configuration_sources: autokeyboardlayot::configuration::ConfigurationSources,
    package_source: PackageSource,
    pending_configuration: Option<ConfigurationReload>,
    /// A loaded configuration that the worker could not take yet; offered again on timer ticks.
    deferred_configuration: Option<RuntimeConfiguration>,
    configuration_loader: Option<RuntimeConfigurationLoader>,
    lexicon_candidate: Arc<Mutex<Option<VolatileLexiconCandidate>>>,
    lexicon_saves: LexiconSaves,
    diagnostic_sender: Option<SyncSender<String>>,
    diagnostic_worker: Option<JoinHandle<()>>,
    /// Key-ups of keys pressed before a gate, delivered after that gate broke.
    pending_key_releases: Vec<RawKeyEvent>,
    /// Held keys handed back unchanged after a contained internal fault, or past a stalled worker.
    pending_held_replay: Vec<RawKeyEvent>,
    /// The token of the retained record the UI thread handed back past a stalled worker: once its
    /// keys were sent, the worker finishes the recovery.
    pending_hand_back: Option<u64>,
    /// Physical key-downs that reached the application, clicks and input injected by other
    /// programs, counted by the hooks. Retained keys are handed back past a busy worker only when
    /// none came after them, as they would land after that newer input.
    typed_input: u64,
}

impl AppState {
    // Keep pumping window messages until the worker has actually returned: it
    // may be waiting for a synchronous gate request handled by this window.
    // A worker_busy snapshot alone cannot establish that it has terminated.
    fn request_shutdown(&mut self) -> bool {
        if self.shutdown.load(Ordering::Acquire) {
            return true;
        }
        let blockers = self.decline_reasons();
        if !blockers.is_empty() {
            self.gate_diagnostic(
                "shutdown",
                format!("result=declined blockers={}", blockers.join(",")),
            );
            return false;
        }
        self.gate_diagnostic("shutdown", "result=accepted".to_owned());
        self.shutdown.store(true, Ordering::Release);
        self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        // Disconnect recv even when there is no queued event to wake it.
        self.input_sender.take();
        true
    }

    /// Why a graceful Exit would be declined right now; empty when it would be accepted.
    fn decline_reasons(&self) -> Vec<&'static str> {
        shutdown_blockers(&ShutdownFacts {
            correction_gate_active: self.correction_gate.active,
            retained_input: has_retained_input(&self.metrics),
            worker_busy: self.metrics.worker_busy.load(Ordering::Acquire),
            worker_finished: self.worker.as_ref().is_some_and(JoinHandle::is_finished),
            configuration_pending: self.metrics.configuration_pending.load(Ordering::Acquire),
            undo_hotkey_active: self.metrics.undo_hotkey_active.load(Ordering::Acquire),
            hotkey_waiting_for_release: self
                .metrics
                .hotkey_waiting_for_release
                .load(Ordering::Acquire),
        })
    }

    /// A contained panic makes the state untrustworthy: pause automatic conversion, hand held keys
    /// back unchanged (or retain them when the input target changed meanwhile), end hotkey
    /// gestures that may be half tracked and discard queued work of the old epoch.
    fn record_engine_fault(&mut self) {
        self.record_engine_fault_with_target(current_foreground_context());
    }

    fn record_engine_fault_with_target(&mut self, current: Option<ForegroundContext>) {
        // The fault's own pause comes first: breaking the gate must not record it as a gate
        // drain, which policy D1 would lift on its own once the retained keys expire.
        if self.metrics.auto_enabled.swap(false, Ordering::AcqRel) {
            self.metrics.last_failure_reason.store(
                ConversionFailureReason::InternalFault as u8,
                Ordering::Release,
            );
            self.metrics.safety_paused.store(true, Ordering::Release);
            self.metrics
                .conversion_failures
                .fetch_add(1, Ordering::Relaxed);
        }
        self.break_correction_gate(GateFailureCause::InternalFault, current);
        self.metrics
            .undo_hotkey_active
            .store(false, Ordering::Release);
        self.metrics
            .hotkey_waiting_for_release
            .store(false, Ordering::Release);
        self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        if self.metrics.worker_dead.load(Ordering::Acquire) {
            // Nobody can acknowledge a reload any more; it must not keep input and Exit blocked.
            self.pending_configuration = None;
            self.metrics
                .configuration_pending
                .store(false, Ordering::Release);
            // Nor carry out a prepared recovery: Pause must not be swallowed for it.
            self.metrics.recovery_armed.store(false, Ordering::Release);
        }
        self.gate_diagnostic(
            "fault",
            format!(
                "result=engine_fault worker_dead={}",
                self.metrics.worker_dead.load(Ordering::Acquire)
            ),
        );
    }

    fn shutdown_finished(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
            && self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn enqueue(&self, event: RawInputEvent) -> bool {
        self.enqueue_with_configuration(event, None).is_ok()
    }

    fn enqueue_with_configuration(
        &self,
        event: RawInputEvent,
        configuration: Option<Box<ConfigurationReload>>,
    ) -> core::result::Result<(), QueueRefusal> {
        let Some(sender) = &self.input_sender else {
            return Err(QueueRefusal::Closed);
        };
        // At most one refresh of each kind waits in the queue, and a full queue only drops it.
        let refresh_queued = match event {
            RawInputEvent::PrivacyRefresh(_) => Some(&self.metrics.privacy_refresh_queued),
            RawInputEvent::ProfileRefresh => Some(&self.metrics.profile_refresh_queued),
            _ => None,
        };
        if refresh_queued.is_some_and(|queued| queued.swap(true, Ordering::AcqRel)) {
            return Ok(());
        }
        if let RawInputEvent::Key(key) = event {
            self.metrics
                .last_input_layout
                .store(key.foreground.layout, Ordering::Release);
        }
        let queued = QueuedInputEvent {
            epoch: self.metrics.input_epoch.load(Ordering::Acquire),
            captured_at: Instant::now(),
            event,
            configuration,
        };
        match sender.try_send(queued) {
            Ok(()) => Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                if let Some(queued) = refresh_queued {
                    queued.store(false, Ordering::Release);
                }
                Err(QueueRefusal::Closed)
            }
            Err(TrySendError::Full(_)) => {
                if let Some(queued) = refresh_queued {
                    queued.store(false, Ordering::Release);
                    return Err(QueueRefusal::Full);
                }
                self.metrics.dropped_events.fetch_add(1, Ordering::Relaxed);
                self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
                Err(QueueRefusal::Full)
            }
        }
    }

    fn enqueue_configuration(&mut self, snapshot: RuntimeConfiguration) -> ConfigurationHandoff {
        if self.shutdown.load(Ordering::Acquire) || self.metrics.worker_dead.load(Ordering::Acquire)
        {
            return ConfigurationHandoff::Refused;
        }
        let previous_sources = self
            .pending_configuration
            .as_ref()
            .map_or(self.configuration_sources, |pending| {
                pending.snapshot.sources
            });
        if !previous_sources.accepts_reload(snapshot.sources) {
            return ConfigurationHandoff::Refused;
        }
        let previous_packages = self
            .pending_configuration
            .as_ref()
            .map_or(self.package_source, |pending| {
                pending.snapshot.package_source
            });
        if !previous_packages.accepts_reload(snapshot.package_source) {
            return ConfigurationHandoff::Refused;
        }
        // One snapshot at a time: the slot holds it until the UI published or dropped it, and a
        // newer one queued over it would be lost with it when the newer one is rejected. A
        // rejected revision is settled like an acknowledged one.
        let settled = self
            .metrics
            .configuration_ack
            .load(Ordering::Acquire)
            .max(self.metrics.configuration_rejected.load(Ordering::Acquire));
        if self.correction_gate.active
            || has_retained_input(&self.metrics)
            || self.pending_configuration.is_some()
            || self.next_configuration_revision.saturating_sub(settled) >= 2
        {
            return ConfigurationHandoff::Busy;
        }
        let Some(revision) = self.next_configuration_revision.checked_add(1) else {
            return ConfigurationHandoff::Refused;
        };
        let reload = ConfigurationReload { revision, snapshot };
        self.metrics
            .configuration_pending
            .store(true, Ordering::Release);
        self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        if let Err(refusal) = self.enqueue_with_configuration(
            RawInputEvent::ReloadConfiguration,
            Some(Box::new(reload.clone())),
        ) {
            // A failed newer request must not cancel an already queued reload.
            self.metrics
                .configuration_pending
                .store(self.pending_configuration.is_some(), Ordering::Release);
            return match refusal {
                QueueRefusal::Full => ConfigurationHandoff::Busy,
                QueueRefusal::Closed => ConfigurationHandoff::Refused,
            };
        }
        self.next_configuration_revision = revision;
        self.pending_configuration = Some(reload);
        ConfigurationHandoff::Queued
    }

    /// Hands a freshly loaded configuration, or else the one deferred earlier, to the worker. A
    /// snapshot that the worker cannot take yet is kept and offered again on the next timer tick,
    /// unless a newer load replaces it. Returns the message for the user, if any: a load that
    /// failed is a read error, a snapshot the worker can never take an apply error.
    fn hand_off_configuration(
        &mut self,
        loaded: Option<std::io::Result<RuntimeConfiguration>>,
    ) -> Option<String> {
        let snapshot = match loaded {
            Some(Ok(snapshot)) => snapshot,
            Some(Err(error)) => {
                // The files changed again and cannot be read: what was deferred is out of date.
                self.deferred_configuration = None;
                return Some(tr_format(
                    "error.read_config",
                    &[("error", &error.to_string())],
                ));
            }
            None => self.deferred_configuration.take()?,
        };
        match self.enqueue_configuration(snapshot.clone()) {
            ConfigurationHandoff::Queued => None,
            ConfigurationHandoff::Busy => {
                self.deferred_configuration = Some(snapshot);
                None
            }
            ConfigurationHandoff::Refused => Some(tr_format(
                "error.apply_config",
                &[("error", "configuration_handoff_unavailable")],
            )),
        }
    }

    /// Drops the pending configuration when the worker rejected it after a contained panic; the
    /// published settings stay as they were. True when it was dropped.
    fn drop_rejected_configuration(&mut self) -> bool {
        let Some(pending) = self.pending_configuration.as_ref() else {
            return false;
        };
        if self.metrics.configuration_rejected.load(Ordering::Acquire) != pending.revision {
            return false;
        }
        self.pending_configuration = None;
        self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        self.metrics
            .configuration_pending
            .store(false, Ordering::Release);
        true
    }

    fn take_acknowledged_configuration(&mut self) -> Option<RuntimeConfiguration> {
        let pending = self.pending_configuration.as_ref()?;
        if self.metrics.configuration_ack.load(Ordering::Acquire) != pending.revision {
            return None;
        }
        self.pending_configuration
            .take()
            .map(|pending| pending.snapshot)
    }

    fn publish_acknowledged_configuration(&mut self) -> Option<RuntimeConfiguration> {
        let configuration = self.take_acknowledged_configuration()?;
        let settings = &configuration.settings;
        self.configuration_sources = configuration.sources;
        self.package_source = configuration.package_source;
        self.settings = settings.clone();
        self.metrics
            .pause_break_undo
            .store(settings.pause_break_undo, Ordering::Release);
        store_hotkey_configuration(&self.metrics, settings);
        self.metrics
            .diagnostics_enabled
            .store(settings.diagnostics_enabled, Ordering::Release);
        if !settings.offer_word_exclusion_after_undo
            && !settings.offer_dictionary_after_forced_conversion
            && let Ok(mut candidate) = self.lexicon_candidate.lock()
        {
            candidate.take();
        }
        // Main-thread publication is one non-reentrant operation. Transition
        // input is invalidated before hooks can claim another hotkey or gate.
        self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        self.metrics
            .configuration_pending
            .store(false, Ordering::Release);
        Some(configuration)
    }

    fn break_correction_gate_fail_open(&mut self, cause: GateFailureCause) {
        self.break_correction_gate(cause, current_foreground_context());
    }

    /// Ends the gate without the worker. Key-ups of keys pressed before the gate are delivered,
    /// because they type nothing and withholding them would leave the key pressed; the rest is
    /// retained for recovery. After a contained internal fault the held keys go back unchanged
    /// while `current`, the input target now, is still the gate's target.
    fn break_correction_gate(
        &mut self,
        cause: GateFailureCause,
        current: Option<ForegroundContext>,
    ) {
        if !self.correction_gate.active {
            return;
        }
        let token = self.correction_gate.token;
        let held_count = self.correction_gate.held.len();
        let sequence = self.metrics.observed_input_sequence.load(Ordering::Acquire);
        let foreground = self.correction_gate.origin;
        let held = core::mem::take(&mut self.correction_gate.held);
        let replay_unchanged = cause == GateFailureCause::InternalFault
            && foreground
                .is_some_and(|origin| current.is_some_and(|now| same_input_target(origin, now)));
        let replayed = if replay_unchanged { held.len() } else { 0 };
        let (held, releases) = if replay_unchanged {
            self.pending_held_replay.extend(held);
            (Vec::new(), Vec::new())
        } else {
            split_delivered_key_releases(held)
        };
        let returned = releases.len();
        self.pending_key_releases.extend(releases);
        let mut retained = false;
        if !held.is_empty() {
            // There cannot be a second gate while recovery is outstanding.
            // Keep data only in RAM, never in diagnostics or on the clipboard.
            {
                let mut recovery = self
                    .metrics
                    .retained_input
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                debug_assert!(recovery.is_none());
                let fallback = held[0].foreground;
                *recovery = Some(RetainedInput {
                    token,
                    foreground: foreground
                        .map(|origin| {
                            current
                                .filter(|now| same_input_target(origin, *now))
                                .unwrap_or(origin)
                        })
                        .unwrap_or(fallback),
                    events: held,
                    delivery_uncertain: cause == GateFailureCause::DownstreamHook,
                    expires_at: Instant::now() + Duration::from_millis(RETAINED_INPUT_LIFETIME_MS),
                    recovering: false,
                    retained_at: Instant::now(),
                    auto_recover: hands_held_keys_back_automatically(cause),
                    input_mark: self.typed_input,
                });
                retained = true;
            }
        }
        self.correction_gate.cancelled = true;
        self.correction_gate.active = false;
        self.correction_gate.pending_hotkey = None;
        self.metrics.active_gate_token.store(0, Ordering::Release);
        self.metrics
            .cancelled_gate_token
            .store(token, Ordering::Release);
        self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        // An empty decision gate is not a failed edit. In particular a delayed
        // acknowledgement after a successfully drained Space-up must not turn
        // automatic conversion off.
        let failed = retained || self.correction_gate.drain_failed;
        if failed {
            self.record_gate_failure_without_worker();
        }
        self.gate_diagnostic("abort", format!(
            "cause={cause:?} token={token} held={held_count} returned={returned} replayed={replayed} released={} in_flight={} awaiting_ack={} worker_busy={} paused={failed}",
            self.correction_gate.released_count, self.correction_gate.drain_in_flight,
            self.correction_gate.awaiting_worker_ack, self.metrics.worker_busy.load(Ordering::Acquire),
        ));
        if returned != 0 || replayed != 0 {
            self.post_held_keys_return();
        }
        let _ = self.enqueue(RawInputEvent::GateDrainReady {
            token,
            drained_count: 0,
            replay_succeeded: false,
        });
        if retained && hands_held_keys_back_automatically(cause) {
            let _ = self.enqueue(RawInputEvent::RecoverInput {
                sequence,
                explicit: false,
            });
        }
        if retained {
            unsafe {
                let _ = PostMessageW(
                    Some(self.window),
                    RECOVERY_NOTICE_MESSAGE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    /// Queued input from before a hook reinstall is stale. The epoch moves first: the recovery a
    /// broken gate queues for its held keys must carry the epoch the worker sees as current.
    fn invalidate_for_hook_reinstall(&mut self) {
        self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        self.break_correction_gate_fail_open(GateFailureCause::HookReinstall);
    }

    /// Asks the window procedure to send the queued held keys, outside any hook procedure that
    /// may be running now. The timer sends them as well if this message cannot be posted.
    fn post_held_keys_return(&self) {
        unsafe {
            let _ = PostMessageW(
                Some(self.window),
                HELD_KEYS_RETURN_MESSAGE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }

    /// Queues the key-ups that leave with a record of retained input (see
    /// split_delivered_key_releases).
    fn queue_delivered_key_releases(&mut self, releases: Vec<RawKeyEvent>) {
        if releases.is_empty() {
            return;
        }
        self.pending_key_releases.extend(releases);
        self.post_held_keys_return();
    }

    /// Holds an edge that arrived while the gate is active. A gate that is full or past its
    /// deadline breaks, and the edge joins the retained input as well: forwarding it now would
    /// overtake every edge held before it. False when no gate is active, so the edge passes.
    fn hold_gated_edge(&mut self, raw: RawKeyEvent) -> bool {
        if self.correction_gate.hold(raw) {
            return true;
        }
        if !self.correction_gate.active {
            return false;
        }
        let cause = if Instant::now() >= self.correction_gate.deadline {
            GateFailureCause::Deadline
        } else {
            GateFailureCause::Capacity
        };
        self.correction_gate.held.push(raw);
        self.break_correction_gate_fail_open(cause);
        true
    }

    fn close_drained_gate(&mut self) {
        debug_assert!(self.correction_gate.held.is_empty());
        let old_token = self.correction_gate.token;
        self.correction_gate.active = false;
        self.metrics.active_gate_token.store(0, Ordering::Release);
        self.gate_diagnostic(
            "closed",
            format!(
                "token={old_token} released={}",
                self.correction_gate.released_count
            ),
        );
        if let Some(mut hotkey) = self
            .correction_gate
            .take_pending_hotkey(current_foreground_context())
        {
            hotkey.sequence = self
                .metrics
                .observed_input_sequence
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            hotkey.drain_token = old_token; // deferred requests are undo-only
            self.metrics
                .explicit_hotkey_sequence
                .store(hotkey.sequence, Ordering::Release);
            self.metrics
                .forwarded_input_sequence
                .fetch_max(hotkey.sequence, Ordering::AcqRel);
            let _ = self.enqueue(RawInputEvent::Key(hotkey));
        }
    }

    fn retain_failed_drain(&mut self, token: u64, mut prefix: Vec<RawKeyEvent>, sent: usize) {
        if self.correction_gate.token != token {
            return;
        }
        self.gate_diagnostic(
            "submit_failed",
            format!("token={token} submitted={sent} raw_edges={}", prefix.len()),
        );
        if self.correction_gate.active {
            prefix.append(&mut self.correction_gate.held);
            self.correction_gate.held = prefix;
            self.correction_gate.drain_failed = true;
            self.break_correction_gate_fail_open(GateFailureCause::ReplaySubmit);
        } else {
            // A hook can cancel the gate while SendInput is returning. Its
            // suffix has already moved to recovery; the unsent prefix still
            // belongs before it and must not be lost or overwrite that suffix.
            let (mut prefix, releases) = split_delivered_key_releases(prefix);
            self.queue_delivered_key_releases(releases);
            let mut retained = self
                .metrics
                .retained_input
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(retained) = retained.as_mut() {
                debug_assert_eq!(retained.token, token);
                prefix.append(&mut retained.events);
                retained.events = prefix;
            } else if let Some(first) = prefix.first() {
                *retained = Some(RetainedInput {
                    token,
                    foreground: self.correction_gate.origin.unwrap_or(first.foreground),
                    events: prefix,
                    delivery_uncertain: sent != 0,
                    expires_at: Instant::now() + Duration::from_millis(RETAINED_INPUT_LIFETIME_MS),
                    recovering: false,
                    retained_at: Instant::now(),
                    auto_recover: false,
                    input_mark: self.typed_input,
                });
            }
        }
        let mut retained = self
            .metrics
            .retained_input
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(retained) = retained.as_mut() {
            retained.delivery_uncertain |= sent != 0;
        }
        self.record_gate_failure_without_worker();
    }

    fn gate_diagnostic(&self, phase: &str, details: String) {
        if self.metrics.diagnostics_enabled.load(Ordering::Acquire)
            && let Some(sender) = &self.diagnostic_sender
        {
            let record = format!(
                "event=gate phase={phase} captured_ms={} epoch={} {details}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                self.metrics.input_epoch.load(Ordering::Acquire)
            );
            if sender.try_send(record).is_err() {
                self.metrics
                    .dropped_diagnostics
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// A diagnostics record outside the gate protocol, for UI-thread timing.
    fn event_diagnostic(&self, event: &str, details: String) {
        if self.metrics.diagnostics_enabled.load(Ordering::Acquire)
            && let Some(sender) = &self.diagnostic_sender
        {
            let record = format!(
                "event={event} captured_ms={} epoch={} {details}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                self.metrics.input_epoch.load(Ordering::Acquire)
            );
            if sender.try_send(record).is_err() {
                self.metrics
                    .dropped_diagnostics
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn record_gate_failure_without_worker(&self) {
        if !self.metrics.auto_enabled.swap(false, Ordering::AcqRel) {
            return;
        }
        self.metrics
            .last_failure_reason
            .store(ConversionFailureReason::GateDrain as u8, Ordering::Release);
        self.metrics.safety_paused.store(true, Ordering::Release);
        self.metrics
            .conversion_failures
            .fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.correction_gate.active = false;
        self.correction_gate.held.clear();
        self.metrics.active_gate_token.store(0, Ordering::Release);
        if let Some(hook) = self.keyboard_hook.take() {
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
            }
        }
        if let Some(hook) = self.mouse_hook.take() {
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
            }
        }
        self.shutdown.store(true, Ordering::Release);
        self.input_sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.diagnostic_sender.take();
        if let Some(worker) = self.diagnostic_worker.take() {
            let _ = worker.join();
        }
    }
}

/// `explicit` marks the tray menu Exit. A declined explicit request asks whether to exit anyway,
/// naming the coarse reasons; installer and other WM_CLOSE requests keep the non-blocking
/// notification and never force.
fn request_shutdown(hwnd: HWND, explicit: bool) {
    // The reasons are read under the same borrow as the attempt, so they describe the state that
    // declined it.
    let (accepted, reasons) = APP_STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map_or((false, Vec::new()), |state| {
                let accepted = state.request_shutdown();
                let reasons = if accepted {
                    Vec::new()
                } else {
                    state.decline_reasons()
                };
                (accepted, reasons)
            })
    });
    if accepted {
        // Only the tray menu sets the flag: a later implicit close must not clear it.
        if explicit {
            EXPLICIT_SHUTDOWN.store(true, Ordering::Release);
        }
        finish_shutdown(hwnd);
    } else if explicit && !reasons.is_empty() {
        if confirm_exit_anyway(hwnd, &reasons) {
            force_exit();
        }
    } else {
        show_tray_information(hwnd, "AutoKeyboardLayot", tr("lifecycle.close_busy"));
    }
}

/// True when the user started the graceful shutdown from the tray menu.
static EXPLICIT_SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// One exit prompt at a time: the dialog's nested message loop keeps dispatching timer and tray
/// messages, which could otherwise stack further prompts.
static EXIT_PROMPT_OPEN: AtomicBool = AtomicBool::new(false);

/// Text of the "exit anyway?" prompt: the coarse reasons, and a warning when keystrokes that were
/// held back for recovery would be lost.
fn exit_anyway_text(reasons: &[&str]) -> String {
    let mut text = tr_format("lifecycle.exit_anyway", &[("reasons", &reasons.join(", "))]);
    if reasons.contains(&"retained-input") {
        text.push_str("\n\n");
        text.push_str(&tr("lifecycle.exit_anyway_discard"));
    }
    text
}

/// Asks whether to leave although Exit was declined or does not finish.
fn confirm_exit_anyway(hwnd: HWND, reasons: &[&str]) -> bool {
    if EXIT_PROMPT_OPEN.swap(true, Ordering::AcqRel) {
        return false;
    }
    let answer = unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(exit_anyway_text(reasons)),
            WINDOW_TITLE,
            ui_localization::message_box_style(MB_YESNO | MB_ICONWARNING),
        )
    };
    EXIT_PROMPT_OPEN.store(false, Ordering::Release);
    answer == IDYES
}

/// Leaves the process without waiting for the worker. Only for an exit the user confirmed: input
/// held back for recovery is lost. The order follows the graceful path: stop the hooks so no new
/// input is swallowed, remove the tray icon, give the diagnostics queue a moment, then exit.
fn force_exit() -> ! {
    let handles = APP_STATE.with(|slot| {
        slot.borrow_mut().as_mut().map(|state| {
            state.gate_diagnostic("shutdown", "result=forced".to_owned());
            (
                state.keyboard_hook.take(),
                state.mouse_hook.take(),
                state.tray.take(),
            )
        })
    });
    if let Some((keyboard, mouse, tray)) = handles {
        for hook in [keyboard, mouse].into_iter().flatten() {
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
            }
        }
        // Dropping the tray icon removes it from the notification area.
        drop(tray);
    }
    thread::sleep(Duration::from_millis(200));
    std::process::exit(0)
}

// True means shutdown owns this timer tick, including while the worker is
// still running. Do not start a configuration reload during that interval.
fn finish_shutdown(hwnd: HWND) -> bool {
    let (requested, finished) = APP_STATE.with(|slot| {
        slot.borrow().as_ref().map_or((false, false), |state| {
            (
                state.shutdown.load(Ordering::Acquire),
                state.shutdown_finished(),
            )
        })
    });
    if finished {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    } else if requested {
        // Record once if the worker has not stopped a few seconds after an
        // accepted shutdown; the window then stays until it does.
        static SHUTDOWN_STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        static SHUTDOWN_REPORTED: AtomicBool = AtomicBool::new(false);
        let started = *SHUTDOWN_STARTED.get_or_init(Instant::now);
        if started.elapsed() >= Duration::from_secs(3)
            && !SHUTDOWN_REPORTED.swap(true, Ordering::AcqRel)
        {
            APP_STATE.with(|slot| {
                if let Some(state) = slot.borrow().as_ref() {
                    state.gate_diagnostic("shutdown", "result=waiting-for-worker".to_owned());
                }
            });
        }
        // A shutdown the user started that still has not completed is offered as a forced exit,
        // and asked again at most every 30 seconds. Installer-driven closes never force.
        static NEXT_STUCK_PROMPT: Mutex<Option<Instant>> = Mutex::new(None);
        if started.elapsed() >= Duration::from_secs(5) && EXPLICIT_SHUTDOWN.load(Ordering::Acquire)
        {
            let due = {
                let mut next = NEXT_STUCK_PROMPT
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let now = Instant::now();
                let due = next.is_none_or(|at| now >= at);
                if due {
                    *next = Some(now + Duration::from_secs(30));
                }
                due
            };
            if due && confirm_exit_anyway(hwnd, &["worker-not-stopping"]) {
                force_exit();
            }
        }
    }
    requested
}

/// Marks a callback or an installation of the hook whose mark `activity` is.
fn touch_hook_activity(activity: &AtomicU64) {
    activity.store(unsafe { GetTickCount64() }, Ordering::Relaxed);
}

/// Each hook's time since its last callback or installation.
fn hook_silence() -> HookSilence {
    let now = unsafe { GetTickCount64() };
    let since = |activity: &AtomicU64| {
        Duration::from_millis(now.saturating_sub(activity.load(Ordering::Relaxed)))
    };
    HookSilence {
        keyboard: since(&KEYBOARD_HOOK_ACTIVITY_MS),
        mouse: since(&MOUSE_HOOK_ACTIVITY_MS),
    }
}

/// The pointer's position on the screen, when the input desktop lets it be read.
fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }
        .is_ok()
        .then_some((point.x, point.y))
}

/// Time since the last user input of the session, from `GetLastInputInfo`.
fn input_idle() -> Option<Duration> {
    let mut info = LASTINPUTINFO {
        cbSize: core::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return None;
    }
    // `dwTime` holds the low 32 bits of the tick count at the last input.
    let now = unsafe { GetTickCount64() } as u32;
    Some(Duration::from_millis(u64::from(
        now.wrapping_sub(info.dwTime),
    )))
}

/// True when the desktop that receives input is the default one. The lock screen and the UAC
/// secure desktop are others: typing there counts as input but never reaches the hooks.
fn input_desktop_is_default() -> bool {
    let Ok(desktop) =
        (unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) })
    else {
        return false;
    };
    let mut name = [0u16; 32];
    let read = unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            core::mem::size_of_val(&name) as u32,
            None,
        )
    }
    .is_ok();
    unsafe {
        let _ = CloseDesktop(desktop);
    }
    let length = name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(name.len());
    read && String::from_utf16_lossy(&name[..length]).eq_ignore_ascii_case("Default")
}

fn foreground_privilege() -> ForegroundPrivilege {
    foreground_privilege_of(
        current_foreground_context().map(|context| context.process_id),
        unsafe { GetCurrentProcessId() },
        process_integrity_level,
    )
}

/// The privilege of the `foreground` process (`None`: nothing is in the foreground) compared with
/// the `agent` process, from the integrity level `integrity_level` reads for a process id.
fn foreground_privilege_of(
    foreground: Option<u32>,
    agent: u32,
    integrity_level: impl Fn(u32) -> Option<u32>,
) -> ForegroundPrivilege {
    let Some(foreground) = foreground else {
        return ForegroundPrivilege::None;
    };
    ForegroundPrivilege::compare(integrity_level(foreground), integrity_level(agent))
}

/// Why the hooks are reinstalled for a session change notification, if they are.
fn session_reinstall_reason(event: u32) -> Option<&'static str> {
    match event {
        WTS_SESSION_UNLOCK => Some("unlock"),
        WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => Some("connect"),
        _ => None,
    }
}

/// Why the hooks are reinstalled for a power notification, if they are.
fn power_reinstall_reason(event: u32) -> Option<&'static str> {
    match event {
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some("resume"),
        _ => None,
    }
}

/// Notices hooks that Windows removed or that stopped receiving input and reinstalls them. The
/// cheap checks come first; the others run only when the hooks already look silent.
fn hook_watchdog_tick(hwnd: HWND) {
    let running = APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|state| !state.shutdown.load(Ordering::Acquire))
    });
    if !running {
        return;
    }
    // Followed on every tick: the pointer counts as moving only over consecutive ticks.
    let pointer_moving =
        UI_GUARDS.with(|guards| guards.borrow_mut().pointer.observe(cursor_position()));
    let Some(input_idle) = input_idle() else {
        return;
    };
    let silence = hook_silence();
    let Some(hook) = silent_hook(input_idle, silence, pointer_moving) else {
        return;
    };
    let facts = WatchdogFacts {
        input_idle,
        silence,
        pointer_moving,
        foreground: foreground_privilege(),
        input_desktop_is_default: input_desktop_is_default(),
        foreground_captures_input: foreground_captures_input(),
    };
    let action =
        UI_GUARDS.with(|guards| guards.borrow_mut().watchdog.decide(&facts, Instant::now()));
    match action {
        WatchdogAction::Nothing => {}
        WatchdogAction::Reinstall => {
            reinstall_hooks(match hook {
                SilentHook::Keyboard => "silent-keyboard",
                SilentHook::Mouse => "silent-mouse",
            });
        }
        WatchdogAction::WarnUnstable => notify_hooks_unstable(hwnd, "watchdog-limit"),
    }
}

fn notify_hooks_unstable(hwnd: HWND, why: &str) {
    APP_STATE.with(|slot| {
        if let Ok(borrowed) = slot.try_borrow()
            && let Some(state) = borrowed.as_ref()
        {
            state.event_diagnostic("hooks", format!("result=unstable why={why}"));
        }
    });
    show_tray_information(hwnd, tr("hooks.unstable_title"), tr("hooks.unstable_body"));
}

/// Installs whichever of the two hooks is missing; true when both are installed afterwards.
fn install_missing_hooks() -> bool {
    let missing = APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| (state.keyboard_hook.is_none(), state.mouse_hook.is_none()))
    });
    let Some((keyboard_missing, mouse_missing)) = missing else {
        return false;
    };
    let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
        return false;
    };
    let instance = HINSTANCE(module.0);
    let keyboard = keyboard_missing
        .then(|| unsafe {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), Some(instance), 0).ok()
        })
        .flatten();
    let mouse = mouse_missing
        .then(|| unsafe {
            SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), Some(instance), 0).ok()
        })
        .flatten();
    // A fresh hook has had no chance to be called yet.
    if keyboard.is_some() {
        touch_hook_activity(&KEYBOARD_HOOK_ACTIVITY_MS);
    }
    if mouse.is_some() {
        touch_hook_activity(&MOUSE_HOOK_ACTIVITY_MS);
    }
    let stored = APP_STATE.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let state = borrowed.as_mut()?;
        if keyboard.is_some() {
            state.keyboard_hook = keyboard;
        }
        if mouse.is_some() {
            state.mouse_hook = mouse;
        }
        Some(state.keyboard_hook.is_some() && state.mouse_hook.is_some())
    });
    match stored {
        Some(complete) => complete,
        None => {
            // The application state is gone (shutdown): do not leave new hooks behind.
            for hook in [keyboard, mouse].into_iter().flatten() {
                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
            }
            false
        }
    }
}

/// Unhooks both hooks and hooks again. Held keys are released unchanged first, and no borrow is
/// held while Windows is called. A failure is retried on later timer ticks with backoff.
fn reinstall_hooks(reason: &str) -> bool {
    let handles = APP_STATE.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let state = borrowed.as_mut()?;
        if state.shutdown.load(Ordering::Acquire) {
            return None;
        }
        state.invalidate_for_hook_reinstall();
        Some((state.keyboard_hook.take(), state.mouse_hook.take()))
    });
    let Some((keyboard, mouse)) = handles else {
        return false;
    };
    for hook in [keyboard, mouse].into_iter().flatten() {
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
    let installed = install_missing_hooks();
    APP_STATE.with(|slot| {
        if let Ok(borrowed) = slot.try_borrow()
            && let Some(state) = borrowed.as_ref()
        {
            state.event_diagnostic(
                "hooks",
                format!(
                    "result={} reason={reason}",
                    if installed { "reinstalled" } else { "failed" }
                ),
            );
        }
    });
    if !installed {
        UI_GUARDS.with(|guards| {
            let mut guards = guards.borrow_mut();
            if guards.hook_retry.is_none() {
                guards.hook_retry = Some(Retry::first(Instant::now()));
            }
        });
    }
    installed
}

/// Repeats a failed hook installation with backoff, and tells the user after the third failure.
fn retry_missing_hooks(hwnd: HWND) {
    let now = Instant::now();
    let due = UI_GUARDS.with(|guards| {
        guards
            .borrow()
            .hook_retry
            .as_ref()
            .is_some_and(|retry| now >= retry.next_attempt)
    });
    if !due {
        return;
    }
    let installed = install_missing_hooks();
    let failures = UI_GUARDS.with(|guards| {
        let mut guards = guards.borrow_mut();
        if installed {
            guards.hook_retry = None;
            return 0;
        }
        guards.hook_retry.as_mut().map_or(0, |retry| {
            retry.failed_again(now);
            retry.failures
        })
    });
    if installed {
        APP_STATE.with(|slot| {
            if let Ok(borrowed) = slot.try_borrow()
                && let Some(state) = borrowed.as_ref()
            {
                state.event_diagnostic("hooks", "result=reinstalled reason=retry".to_owned());
            }
        });
    } else if failures == 3 {
        notify_hooks_unstable(hwnd, "install-failed");
    }
}

/// Adds the tray icon showing `status`; on failure a retry is scheduled, because Explorer may
/// still be starting.
fn add_tray(hwnd: HWND, status: TrayStatus) -> bool {
    let now = Instant::now();
    match TrayIcon::add(hwnd, status) {
        Ok(tray) => {
            let duplicate = APP_STATE.with(|slot| {
                let mut borrowed = slot.borrow_mut();
                let state = borrowed.as_mut()?;
                if state.tray.is_none() {
                    state.tray = Some(tray);
                    // The next refresh compares with what the new icon shows.
                    state.last_tray_status = status;
                    None
                } else {
                    Some(tray)
                }
            });
            // Dropping an icon deletes it from the shell by id, which would remove the live one.
            if let Some(duplicate) = duplicate {
                duplicate.discard_replaced();
            }
            let failures = UI_GUARDS.with(|guards| {
                guards
                    .borrow_mut()
                    .tray_retry
                    .take()
                    .map_or(0, |retry| retry.failures)
            });
            if failures > 0 {
                APP_STATE.with(|slot| {
                    if let Ok(borrowed) = slot.try_borrow()
                        && let Some(state) = borrowed.as_ref()
                    {
                        state.event_diagnostic("tray", format!("result=added failures={failures}"));
                    }
                });
            }
            true
        }
        Err(error) => {
            let failures = UI_GUARDS.with(|guards| {
                let mut guards = guards.borrow_mut();
                match guards.tray_retry.as_mut() {
                    Some(retry) => retry.failed_again(now),
                    None => guards.tray_retry = Some(Retry::first(now)),
                }
                guards.tray_retry.as_ref().map_or(0, |retry| retry.failures)
            });
            APP_STATE.with(|slot| {
                if let Ok(borrowed) = slot.try_borrow()
                    && let Some(state) = borrowed.as_ref()
                {
                    state.event_diagnostic(
                        "tray",
                        format!(
                            "result=unavailable failures={failures} error={:#010x}",
                            error.code().0
                        ),
                    );
                }
            });
            false
        }
    }
}

/// Explorer restarted (`TaskbarCreated`) and every notification icon is gone: build a new one
/// that shows what the old one showed. Starting from the initial status instead would announce
/// an old conversion failure again at the next refresh.
fn recreate_tray(hwnd: HWND) {
    let taken = APP_STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(|state| (state.tray.take(), state.last_tray_status))
    });
    let Some((old, status)) = taken else {
        return;
    };
    // Dropping the old icon calls the shell, so no borrow is held here.
    drop(old);
    add_tray(hwnd, status);
}

/// Repeats a failed tray registration with backoff while the icon is missing.
fn retry_tray(hwnd: HWND) {
    let now = Instant::now();
    let due = UI_GUARDS.with(|guards| {
        guards
            .borrow()
            .tray_retry
            .as_ref()
            .is_some_and(|retry| now >= retry.next_attempt)
    });
    if !due {
        return;
    }
    let status = APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|state| state.tray.is_none())
            .map(|state| state.last_tray_status)
    });
    match status {
        Some(status) => {
            add_tray(hwnd, status);
        }
        None => UI_GUARDS.with(|guards| guards.borrow_mut().tray_retry = None),
    }
}

/// The gap since the previous timer tick in milliseconds, when it exceeds the stall threshold.
fn timer_stall_ms(previous: Option<Instant>, now: Instant) -> Option<u128> {
    previous
        .map(|previous| now.saturating_duration_since(previous))
        .filter(|gap| *gap > Duration::from_millis(UI_STALL_DIAGNOSTIC_MS))
        .map(|gap| gap.as_millis())
}

/// The UI thread serves the hooks' synchronous requests, so a long gap between its timer ticks
/// explains a gate that timed out.
fn report_timer_stall() {
    let now = Instant::now();
    let previous = LAST_TIMER_TICK.with(|tick| tick.replace(Some(now)));
    if let Some(gap_ms) = timer_stall_ms(previous, now) {
        APP_STATE.with(|slot| {
            if let Ok(borrowed) = slot.try_borrow()
                && let Some(state) = borrowed.as_ref()
            {
                state.event_diagnostic("ui_stall", format!("delta_ms={gap_ms}"));
            }
        });
    }
}

/// Reacts on the UI thread to a contained panic (`ENGINE_FAULT`) or to a worker that gave up:
/// release held keys, pause automatic conversion, and say so once in a while.
fn handle_engine_fault(hwnd: HWND) {
    static WORKER_STOP_REPORTED: AtomicBool = AtomicBool::new(false);
    static LAST_NOTICE: Mutex<Option<Instant>> = Mutex::new(None);
    let faulted = ENGINE_FAULT.swap(false, Ordering::AcqRel);
    let worker_stopped = APP_STATE.with(|slot| {
        slot.borrow().as_ref().is_some_and(|state| {
            state.metrics.worker_dead.load(Ordering::Acquire)
                && !state.shutdown.load(Ordering::Acquire)
        })
    }) && !WORKER_STOP_REPORTED.swap(true, Ordering::AcqRel);
    if !faulted && !worker_stopped {
        return;
    }
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            state.record_engine_fault();
        }
    });
    // A fault that repeats on every key would otherwise show a notice every quarter second.
    let notice_due = {
        let mut last = LAST_NOTICE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        let due = worker_stopped
            || last.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(60));
        if due {
            *last = Some(now);
        }
        due
    };
    if notice_due {
        let body = if worker_stopped {
            tr("fault.worker_stopped")
        } else {
            tr("fault.engine")
        };
        show_tray_information(hwnd, tr("fault.title"), body);
    }
}

fn take_app_state() -> Option<AppState> {
    APP_STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        state.take()
    })
}

/// Facts that can make a graceful Exit wait or be declined.
struct ShutdownFacts {
    correction_gate_active: bool,
    retained_input: bool,
    worker_busy: bool,
    worker_finished: bool,
    configuration_pending: bool,
    undo_hotkey_active: bool,
    hotkey_waiting_for_release: bool,
}

/// The reasons for declining Exit, in reporting order. A finished worker thread cannot be busy,
/// acknowledge a reload or recover held keys, so the stale flags a crashed worker leaves behind
/// are ignored, and so are held keys only it could bring back.
fn shutdown_blockers(facts: &ShutdownFacts) -> Vec<&'static str> {
    [
        ("correction-gate", facts.correction_gate_active),
        (
            "retained-input",
            facts.retained_input && !facts.worker_finished,
        ),
        ("worker-busy", facts.worker_busy && !facts.worker_finished),
        (
            "configuration-pending",
            facts.configuration_pending && !facts.worker_finished,
        ),
        ("undo-hotkey", facts.undo_hotkey_active),
        ("hotkey-release", facts.hotkey_waiting_for_release),
    ]
    .into_iter()
    .filter_map(|(name, active)| active.then_some(name))
    .collect()
}

/// Whether a finished recovery may resume automatic conversion (policy D1): only the automatic
/// recovery after a gate abort, and only when every requested key was submitted.
const fn should_resume_after_recovery(explicit: bool, submitted: usize, requested: usize) -> bool {
    !explicit && requested != 0 && submitted == requested
}

fn has_retained_input(metrics: &ObserverMetrics) -> bool {
    metrics
        .retained_input
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_some()
}

fn arm_input_recovery(hwnd: HWND) {
    let details = APP_STATE.with(|slot| {
        let state = slot.borrow();
        let state = state.as_ref()?;
        // Only the worker replays retained input; a stopped one can never use the hotkey.
        if state.metrics.worker_dead.load(Ordering::Acquire) {
            return None;
        }
        let retained = state
            .metrics
            .retained_input
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let retained = retained.as_ref()?;
        Some((
            retained.token,
            retained.events.len(),
            retained.delivery_uncertain,
            state.settings.force_hotkey().display_name(),
        ))
    });
    if let Some((token, edges, uncertain, hotkey)) = details {
        if uncertain
            && unsafe {
                MessageBoxW(
                    Some(hwnd),
                    &HSTRING::from(tr("recovery.repeat_warning")),
                    WINDOW_TITLE,
                    ui_localization::message_box_style(MB_YESNO | MB_ICONINFORMATION),
                )
            } != IDYES
        {
            return;
        }
        let armed = APP_STATE.with(|slot| {
            slot.borrow().as_ref().is_some_and(|state| {
                prepare_retained_recovery(&state.metrics, token, Instant::now())
            })
        });
        if !armed {
            return;
        }
        show_tray_information(
            hwnd,
            tr("recovery.ready_title"),
            tr_format(
                "recovery.ready_body",
                &[("count", &edges.to_string()), ("hotkey", &hotkey)],
            ),
        );
    }
}

/// Marks the retained record `token` for an explicit recovery with the force hotkey and gives it
/// a full lifetime from `now`, because returning to the field and placing the caret takes time.
/// False when that record is gone (recovered, deleted or replaced meanwhile).
fn prepare_retained_recovery(metrics: &ObserverMetrics, token: u64, now: Instant) -> bool {
    let mut retained = metrics
        .retained_input
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let same_record = match retained.as_mut() {
        Some(input) if input.token == token => {
            input.expires_at = input
                .expires_at
                .max(now + Duration::from_millis(RETAINED_INPUT_LIFETIME_MS));
            true
        }
        _ => false,
    };
    metrics.recovery_armed.store(same_record, Ordering::Release);
    same_record
}

/// How long past its lifetime a record stays while the worker is sending it. A worker that died
/// in the middle must not keep the keys, the pause and the declined Exit forever.
const RETAINED_RECOVERY_GRACE: Duration = Duration::from_secs(5);

/// Marks the retained record `token` as being sent by the worker. False when it is gone (expired,
/// discarded or replaced) or another recovery is already sending it.
fn begin_retained_recovery(metrics: &ObserverMetrics, token: u64) -> bool {
    let mut retained = metrics
        .retained_input
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match retained.as_mut() {
        Some(input) if input.token == token && !input.recovering => {
            input.recovering = true;
            true
        }
        _ => false,
    }
}

/// Ends a recovery that sent `sent` of `requested` inputs: a complete one removes the record, an
/// incomplete one keeps it for another attempt, marked uncertain when part of it went out.
fn finish_retained_recovery(metrics: &ObserverMetrics, token: u64, sent: usize, requested: usize) {
    let mut retained = metrics
        .retained_input
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(input) = retained.as_mut()
        && input.token == token
    {
        if sent == requested {
            *retained = None;
        } else {
            input.recovering = false;
            input.delivery_uncertain |= sent != 0;
        }
    }
}

/// Deletes the held keys the user confirmed deleting. True when they were deleted: their lifetime
/// may have ended while the question was open, or the worker may be typing them right now.
fn discard_confirmed_retained_input(state: &mut AppState) -> bool {
    let discarded = {
        let mut retained = state
            .metrics
            .retained_input
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if retained.as_ref().is_some_and(|input| !input.recovering) {
            retained.take()
        } else {
            None
        }
    };
    state.metrics.recovery_armed.store(false, Ordering::Release);
    let Some(discarded) = discarded else {
        return false;
    };
    state.gate_diagnostic("recovery_discarded", "explicit=true".to_owned());
    // Deleting a key-up whose key-down was typed would leave that key pressed.
    let (_, releases) = split_delivered_key_releases(discarded.events);
    state.queue_delivered_key_releases(releases);
    true
}

fn discard_retained_input(hwnd: HWND) {
    let confirmed = unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(tr("recovery.discard_warning")),
            WINDOW_TITLE,
            ui_localization::message_box_style(MB_YESNO | MB_ICONINFORMATION),
        )
    };
    if confirmed != IDYES {
        return;
    }
    let discarded = APP_STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(discard_confirmed_retained_input)
    });
    if discarded {
        show_tray_information(
            hwnd,
            tr("recovery.discarded_title"),
            tr("recovery.discarded_body"),
        );
    }
}

fn toggle_auto_enabled() {
    let changed = APP_STATE.with(|slot| {
        let state = slot.borrow();
        let state = state.as_ref()?;
        if has_retained_input(&state.metrics) {
            return None;
        }
        state.metrics.safety_paused.store(false, Ordering::Release);
        state.metrics.auto_enabled.fetch_xor(true, Ordering::AcqRel);
        state
            .metrics
            .undo_hotkey_active
            .store(false, Ordering::Release);
        // This atomic is the only mode value. Advancing the epoch makes every
        // event already queued under the previous mode stale.
        state.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        Some(())
    });
    if changed.is_some() {
        refresh_tray_state();
    }
}

fn store_hotkey_configuration(metrics: &ObserverMetrics, settings: &Settings) {
    let hotkey = settings.force_hotkey();
    metrics
        .force_hotkey_virtual_key
        .store(u32::from(hotkey.virtual_key), Ordering::Release);
    metrics
        .force_hotkey_modifiers
        .store(hotkey.modifiers, Ordering::Release);
    metrics.undo_hotkey_active.store(false, Ordering::Release);
    metrics
        .hotkey_waiting_for_release
        .store(false, Ordering::Release);
}

fn finish_configuration_reload(hwnd: HWND) {
    let rejected = APP_STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(AppState::drop_rejected_configuration)
    });
    if rejected {
        show_configuration_error(
            hwnd,
            &tr_format(
                "error.apply_config",
                &[("error", "configuration_apply_failed")],
            ),
        );
        return;
    }
    let configuration = APP_STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .and_then(AppState::publish_acknowledged_configuration)
    });
    let Some(configuration) = configuration else {
        return;
    };
    ui_localization::apply_installed(
        &configuration.ui_language,
        configuration.package_catalogs.as_deref(),
    );
    refresh_tray_state();
    show_tray_information(hwnd, "AutoKeyboardLayot", tr("settings.reloaded"));
}

fn reload_configuration_from_disk(hwnd: HWND) {
    let requested = APP_STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .and_then(|state| state.configuration_loader.as_mut())
            .is_some_and(RuntimeConfigurationLoader::request)
    });
    if !requested {
        show_configuration_error(
            hwnd,
            &tr_format(
                "error.read_config",
                &[("error", "configuration_loader_unavailable")],
            ),
        );
    }
}

/// Hands a configuration the loader finished, or one deferred on an earlier tick, to the worker.
fn finish_configuration_load(hwnd: HWND) {
    let message = APP_STATE.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let state = borrowed.as_mut()?;
        let loaded = state
            .configuration_loader
            .as_mut()
            .and_then(RuntimeConfigurationLoader::poll);
        state.hand_off_configuration(loaded)
    });
    if let Some(message) = message {
        show_configuration_error(hwnd, &message);
    }
}

fn show_configuration_error(hwnd: HWND, message: &str) {
    let message = HSTRING::from(message);
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &message,
            &HSTRING::from(tr("window.settings")),
            ui_localization::message_box_style(MB_OK | MB_ICONERROR),
        );
    }
}

fn valid_lexicon_candidate() -> Option<VolatileLexiconCandidate> {
    let shared = APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| Arc::clone(&state.lexicon_candidate))
    })?;
    let mut candidate = shared.lock().ok()?;
    if candidate
        .as_ref()
        .is_some_and(|candidate| !candidate.is_valid_at(Instant::now()))
    {
        candidate.take();
    }
    candidate.clone()
}

fn expire_lexicon_candidate() {
    let _ = valid_lexicon_candidate();
}

/// Tray "add word" saves finished on their own threads, waiting for the window to report them.
struct LexiconSaves {
    sender: SyncSender<(VolatileLexiconCandidate, std::io::Result<()>)>,
    finished: Receiver<(VolatileLexiconCandidate, std::io::Result<()>)>,
}

impl LexiconSaves {
    fn new() -> Self {
        let (sender, finished) = sync_channel(8);
        Self { sender, finished }
    }
}

/// Saves the word a tray offer names. Reading, merging and writing the configuration waits for
/// its writer lock and for the disk, so it runs on a thread of its own: this thread serves the
/// keyboard hook. The window reports the outcome (`finish_lexicon_saves`).
fn add_recent_lexicon_offer(hwnd: HWND) {
    let Some(candidate) = valid_lexicon_candidate() else {
        return;
    };
    let Some(sender) = APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| state.lexicon_saves.sender.clone())
    }) else {
        return;
    };
    let window = hwnd.0 as usize;
    let started = configuration_directory()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
        })
        .and_then(|directory| {
            save_lexicon_candidate_in_background(
                directory,
                candidate.clone(),
                move |candidate, result| {
                    let _ = sender.send((candidate, result));
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(window as *mut c_void)),
                            LEXICON_SAVED_MESSAGE,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                },
            )
        });
    if let Err(error) = started {
        report_lexicon_save(hwnd, &candidate, Err(error));
    }
}

/// Saves `candidate` into the configuration in `directory` on a thread of its own; `finished`
/// gets the outcome there.
fn save_lexicon_candidate_in_background(
    directory: PathBuf,
    candidate: VolatileLexiconCandidate,
    finished: impl FnOnce(VolatileLexiconCandidate, std::io::Result<()>) + Send + 'static,
) -> std::io::Result<()> {
    thread::Builder::new()
        .name("autokey-lexicon-save".into())
        .spawn(move || {
            let result = persist_lexicon_candidate(&directory, &candidate);
            finished(candidate, result);
        })
        .map(drop)
}

/// Reports the tray "add word" saves that finished.
fn finish_lexicon_saves(hwnd: HWND) {
    let finished: Vec<_> = APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| state.lexicon_saves.finished.try_iter().collect())
            .unwrap_or_default()
    });
    for (candidate, result) in finished {
        report_lexicon_save(hwnd, &candidate, result);
    }
}

fn report_lexicon_save(
    hwnd: HWND,
    candidate: &VolatileLexiconCandidate,
    result: std::io::Result<()>,
) {
    if let Err(error) = result {
        let destination = match candidate.target {
            LexiconOfferTarget::UserDictionary => tr("dictionary.destination"),
            LexiconOfferTarget::WordExclusions => tr("word_exclusions.destination"),
        };
        show_configuration_error(
            hwnd,
            &tr_format(
                "error.add_word",
                &[("destination", &destination), ("error", &error.to_string())],
            ),
        );
        return;
    }
    // A newer offer made while this one was saved stays.
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow().as_ref()
            && let Ok(mut stored) = state.lexicon_candidate.lock()
            && stored.as_ref().is_some_and(|offer| {
                offer.target == candidate.target
                    && offer.language == candidate.language
                    && offer.word == candidate.word
            })
        {
            stored.take();
        }
    });
    reload_configuration_from_disk(hwnd);
    let message = match candidate.target {
        LexiconOfferTarget::UserDictionary => tr("dictionary.added"),
        LexiconOfferTarget::WordExclusions => tr("word_exclusions.added"),
    };
    show_tray_information(hwnd, "AutoKeyboardLayot", message);
}

/// The wait before the next periodic field check, from how long the last check took: half a
/// second while checks are fast, otherwise twenty times the last check, at most five seconds.
fn privacy_refresh_interval(last_check: Duration) -> Duration {
    last_check
        .saturating_mul(PRIVACY_REFRESH_COST_FACTOR)
        .clamp(
            Duration::from_millis(PRIVACY_REFRESH_MIN_MS),
            Duration::from_millis(PRIVACY_REFRESH_MAX_MS),
        )
}

/// Whether a periodic field check can be left out: a full-screen program is in the foreground
/// and the last periodic check of this same input target was slow, which is how games answer.
/// A new input target is checked again, and a click or a change of field is checked on the next
/// typed key as everywhere; only a focus change made by the program itself, without input,
/// waits for that.
fn full_screen_check_is_redundant(
    full_screen: bool,
    last_check: Option<(ForegroundContext, Duration)>,
    foreground: ForegroundContext,
) -> bool {
    full_screen
        && last_check.is_some_and(|(target, took)| {
            same_input_target(target, foreground)
                && took >= Duration::from_millis(PRIVACY_SLOW_PROVIDER_MS)
        })
}

/// Whether the shell reports a full-screen program in the foreground (most games, a full-screen
/// video). Asked on the worker only: the shell can take long to answer while Explorer is busy,
/// and the UI thread serves the hooks.
fn foreground_is_full_screen() -> bool {
    // SAFETY: the function only writes the state it returns and has no preconditions; a failure
    // comes back as an error, which counts as not full-screen.
    let state = unsafe { SHQueryUserNotificationState() };
    matches!(state, Ok(state) if state == QUNS_BUSY || state == QUNS_RUNNING_D3D_FULL_SCREEN)
}

/// Whether the foreground may take the keyboard before the hooks see it: a full-screen program
/// as the worker last saw it (asking the shell here could block the hook thread), or a virtual
/// machine console or Remote Desktop window. Gathered only when the hooks already look silent.
fn foreground_captures_input() -> bool {
    let full_screen = APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|state| state.metrics.foreground_full_screen.load(Ordering::Acquire))
    });
    full_screen
        || current_foreground_context()
            .and_then(|context| process_image_name(context.process_id))
            .is_some_and(|path| captures_keyboard(&path))
}

/// The worker's periodic refreshes for a timer tick: the keyboard profiles on every tick, and the
/// focused field's privacy check at its own interval, except for the shell.
fn queue_periodic_refreshes(
    state: &mut AppState,
    context: ForegroundContext,
    shell_foreground: bool,
    now: Instant,
) {
    state.enqueue(RawInputEvent::ProfileRefresh);
    if shell_foreground {
        // The desktop is no full-screen program, and no refresh will report it.
        state
            .metrics
            .foreground_full_screen
            .store(false, Ordering::Release);
    } else if now >= state.next_privacy_refresh {
        state.enqueue(RawInputEvent::PrivacyRefresh(context));
        let last_check = state.metrics.privacy_probe_cost_ms.load(Ordering::Acquire);
        state.next_privacy_refresh =
            now + privacy_refresh_interval(Duration::from_millis(last_check));
    }
}

fn refresh_tray_state() {
    if APP_STATE.with(|slot| slot.borrow().as_ref().is_some_and(|state| state.menu_open)) {
        return;
    }
    let Some(context) = current_foreground_context() else {
        return;
    };
    let shell_foreground = is_shell_process(context.process_id);
    let now = Instant::now();
    APP_STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if let Some(state) = state.as_mut() {
            queue_periodic_refreshes(state, context, shell_foreground, now);
        }
    });
    let last_input_layout = APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| state.metrics.last_input_layout.load(Ordering::Acquire))
            .unwrap_or_default()
    });
    let indicator = if shell_foreground {
        if last_input_layout == 0 {
            APP_STATE.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|state| state.last_tray_status.indicator)
                    .unwrap_or(LayoutIndicator::Unavailable)
            })
        } else {
            LayoutIndicator::from_layout(last_input_layout)
        }
    } else {
        LayoutIndicator::from_layout(context.layout)
    };
    update_tray(indicator, now);
}

/// Shows the agent's current status with `indicator` in the tray icon when it changed, and
/// announces a new conversion failure.
fn update_tray(indicator: LayoutIndicator, now: Instant) {
    let Some((mut tray, status, notify_failure)) = APP_STATE.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let state = borrowed.as_mut()?;
        let status = TrayStatus {
            indicator,
            ui_revision: ui_localization::revision(),
            candidates: state.metrics.candidates.load(Ordering::Relaxed),
            dropped: state.metrics.dropped_events.load(Ordering::Relaxed),
            privacy_reason: state.metrics.privacy_reason.load(Ordering::Acquire),
            auto_enabled: state.metrics.auto_enabled.load(Ordering::Acquire),
            safety_paused: state.metrics.safety_paused.load(Ordering::Acquire),
            undo_available: state.metrics.undo_available.load(Ordering::Acquire),
            failures: state.metrics.conversion_failures.load(Ordering::Relaxed),
            failure_reason: state.metrics.last_failure_reason.load(Ordering::Acquire),
            backend_status: state.metrics.backend_status.load(Ordering::Acquire),
            diagnostic_errors: diagnostic_error_count(&state.metrics),
        };
        if status == state.last_tray_status {
            return None;
        }
        // The shell call below can block on an unresponsive Explorer while the hooks wait for this
        // thread: at most two updates per second and none while a gate holds keys. The status
        // stays different from the shown one, so a refused update is tried again on a later tick.
        let gate_active = state.correction_gate.active;
        if !UI_GUARDS.with(|guards| guards.borrow_mut().tray.allow(now, gate_active)) {
            return None;
        }
        let notify_failure = should_notify_conversion_failure(state.last_tray_status, status);
        state.tray.take().map(|tray| (tray, status, notify_failure))
    }) else {
        return;
    };

    // No RefCell borrow is held while Shell_NotifyIconW runs. Shell calls may
    // pump messages and re-enter either the window procedure or a hook.
    let updated = tray.update(status).is_ok();
    let window = tray.hwnd;
    // The icon comes back when it cannot be put back: the state is gone (shutdown), or a message
    // dispatched during the update (TaskbarCreated) added a new icon meanwhile.
    let left_over = APP_STATE.with(|slot| match slot.borrow_mut().as_mut() {
        Some(state) if state.tray.is_none() => {
            state.tray = Some(tray);
            if updated {
                state.last_tray_status = status;
            }
            None
        }
        Some(_) => Some((tray, true)),
        None => Some((tray, false)),
    });
    let stored = left_over.is_none();
    match left_over {
        // The shell knows the new icon by the same id: deleting this one would remove it. The
        // next refresh shows the status in the new icon and announces a failure there.
        Some((stale, true)) => stale.discard_replaced(),
        Some((gone, false)) => drop(gone),
        None => {}
    }
    if updated && notify_failure && stored {
        show_tray_information(
            window,
            tr("conversion.paused_title"),
            tr_format(
                "conversion.paused_body",
                &[("reason", conversion_failure_label(status.failure_reason))],
            ),
        );
    }
}

fn should_notify_conversion_failure(previous: TrayStatus, current: TrayStatus) -> bool {
    !current.auto_enabled && current.failures > previous.failures
}

struct TrayIcon {
    hwnd: HWND,
    icon: HICON,
    visual: TrayVisual,
    indicator: LayoutIndicator,
}

impl TrayIcon {
    fn add(hwnd: HWND, status: TrayStatus) -> Result<Self> {
        let visual = tray_visual_state(status);
        let icon = create_indicator_icon(visual, status.indicator)?;
        let mut data = notify_icon_data(hwnd, icon);
        // With NOTIFYICON_VERSION_4 the shell shows the tooltip only when every call asks for it.
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
        let tooltip = tray_tooltip(status);
        set_tooltip(&mut data, &tooltip);
        if !shell_notify_icon(NIM_ADD, &data) {
            return Err(last_error_then(|| unsafe {
                let _ = DestroyIcon(icon);
            }));
        }

        let mut version_data = notify_icon_data(hwnd, icon);
        version_data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        if !shell_notify_icon(NIM_SETVERSION, &version_data) {
            return Err(last_error_then(|| {
                let _ = shell_notify_icon(NIM_DELETE, &data);
                unsafe {
                    let _ = DestroyIcon(icon);
                }
            }));
        }
        unsafe {
            let _ = SetWindowTextW(hwnd, &HSTRING::from(&tooltip));
        }
        Ok(Self {
            hwnd,
            icon,
            visual,
            indicator: status.indicator,
        })
    }

    fn update(&mut self, status: TrayStatus) -> Result<()> {
        let visual = tray_visual_state(status);
        let visual_changed = visual != self.visual || status.indicator != self.indicator;
        let new_icon = if visual_changed {
            Some(create_indicator_icon(visual, status.indicator)?)
        } else {
            None
        };
        let icon = new_icon.unwrap_or(self.icon);
        let mut data = notify_icon_data(self.hwnd, icon);
        data.uFlags = NIF_TIP
            | NIF_SHOWTIP
            | if visual_changed {
                NIF_ICON
            } else {
                Default::default()
            };
        let tooltip = tray_tooltip(status);
        set_tooltip(&mut data, &tooltip);
        if !shell_notify_icon(NIM_MODIFY, &data) {
            return Err(last_error_then(|| {
                if let Some(new_icon) = new_icon {
                    unsafe {
                        let _ = DestroyIcon(new_icon);
                    }
                }
            }));
        }
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(&tooltip));
        }

        if let Some(new_icon) = new_icon {
            let old_icon = core::mem::replace(&mut self.icon, new_icon);
            self.visual = visual;
            self.indicator = status.indicator;
            unsafe {
                let _ = DestroyIcon(old_icon);
            }
        }
        Ok(())
    }

    /// Destroys the image of an icon that a newer one replaced under the same window and id,
    /// and leaves the notification icon alone: deleting it would remove the newer one.
    fn discard_replaced(self) {
        let icon = self.icon;
        core::mem::forget(self);
        unsafe {
            let _ = DestroyIcon(icon);
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let data = notify_icon_data(self.hwnd, self.icon);
        let _ = shell_notify_icon(NIM_DELETE, &data);
        unsafe {
            let _ = DestroyIcon(self.icon);
        }
    }
}

/// `Shell_NotifyIconW`. Tests put a scripted notification area in its place, which records the
/// calls and answers them.
fn shell_notify_icon(message: NOTIFY_ICON_MESSAGE, data: &NOTIFYICONDATAW) -> bool {
    #[cfg(test)]
    if let Some(answer) = tests::scripted_shell_call(message, data) {
        return answer;
    }
    unsafe { Shell_NotifyIconW(message, data) }.as_bool()
}

/// The error of the call that just failed, read before `cleanup` runs: the cleanup's own calls
/// may leave another last error behind.
fn last_error_then(cleanup: impl FnOnce()) -> Error {
    let error = Error::from_thread();
    cleanup();
    error
}

/// Panics of this process are written to crash.log in the profile folder (never their message).
fn install_crash_log() {
    if let Some(directory) = configuration_directory() {
        autokeyboardlayot::crash_log::install(directory, APP_VERSION);
    }
}

/// The startup dialog for a configuration that cannot be read. A document that
/// parses badly (`InvalidData`) also gets the folder and what can be done about
/// it; a failure elsewhere, such as in the installed packages, does not.
fn unreadable_configuration_message(error: &std::io::Error, directory: Option<&Path>) -> String {
    let mut message = tr_format("error.read_config", &[("error", &error.to_string())]);
    if let (std::io::ErrorKind::InvalidData, Some(directory)) = (error.kind(), directory) {
        message.push_str("\n\n");
        message.push_str(&tr_format(
            "error.read_config_hint",
            &[("folder", &directory.to_string_lossy())],
        ));
    }
    message
}

pub fn run() -> Result<()> {
    install_crash_log();
    // The guard comes first: a second instance leaves without reading the
    // configuration or the installed packages, and without a dialog about a
    // file that the running instance has already read.
    let Some(_instance_guard) = InstanceGuard::acquire()? else {
        return Ok(());
    };
    let initial_configuration = load_runtime_configuration().map_err(|error| {
        Error::new(
            windows::core::HRESULT(0x8007000Du32 as i32),
            unreadable_configuration_message(&error, configuration_directory().as_deref()),
        )
    })?;
    ui_localization::initialize(
        &initial_configuration.ui_language,
        initial_configuration.package_catalogs.as_deref(),
    );

    unsafe {
        let module = GetModuleHandleW(None)?;
        let instance = HINSTANCE(module.0);
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        if RegisterClassW(&window_class) == 0 {
            return Err(Error::from_thread());
        }

        // Registered before the message loop starts, so no broadcast can be missed.
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Release,
        );

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            WINDOW_TITLE,
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )?;
        // Unlock, reconnect and resume reinstall the hooks. Not fatal when it is refused: the
        // watchdog still notices hooks that stopped receiving input.
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);

        let initial_indicator = current_foreground_context()
            .map(|context| LayoutIndicator::from_layout(context.layout))
            .unwrap_or(LayoutIndicator::Unavailable);
        installer_lifecycle::advertise_safe_close(hwnd);
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(
            initial_configuration
                .settings
                .automatic_conversion_on_startup,
            Ordering::Release,
        );
        metrics.pause_break_undo.store(
            initial_configuration.settings.pause_break_undo,
            Ordering::Release,
        );
        store_hotkey_configuration(&metrics, &initial_configuration.settings);
        metrics.diagnostics_enabled.store(
            initial_configuration.settings.diagnostics_enabled,
            Ordering::Release,
        );
        let shutdown = Arc::new(AtomicBool::new(false));
        let lexicon_candidate = Arc::new(Mutex::new(None));
        let (diagnostic_sender, diagnostic_receiver) = sync_channel(DIAGNOSTIC_QUEUE_CAPACITY);
        let diagnostic_metrics = Arc::clone(&metrics);
        let diagnostic_worker = thread::spawn(move || {
            diagnostic_worker(
                diagnostic_receiver,
                diagnostic_log_path(),
                &diagnostic_metrics,
            );
        });
        let correction_gate = CorrectionGate::default();
        let (input_sender, input_receiver) = sync_channel(INPUT_QUEUE_CAPACITY);
        let worker_metrics = Arc::clone(&metrics);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_lexicon_candidate = Arc::clone(&lexicon_candidate);
        let worker_diagnostic_sender = diagnostic_sender.clone();
        let gate_window = hwnd.0 as usize;
        let worker_configuration = initial_configuration.clone();
        let worker = thread::spawn(move || {
            input_worker(
                input_receiver,
                worker_metrics,
                worker_shutdown,
                gate_window,
                worker_lexicon_candidate,
                worker_diagnostic_sender,
                worker_configuration,
            );
        });

        APP_STATE.with(|slot| {
            *slot.borrow_mut() = Some(AppState {
                window: hwnd,
                tray: None,
                keyboard_hook: None,
                mouse_hook: None,
                input_sender: Some(input_sender),
                worker: Some(worker),
                shutdown,
                metrics,
                correction_gate,
                last_tray_status: TrayStatus::initial(initial_indicator),
                next_privacy_refresh: Instant::now(),
                menu_open: false,
                settings: initial_configuration.settings,
                next_configuration_revision: 0,
                configuration_sources: initial_configuration.sources,
                package_source: initial_configuration.package_source,
                pending_configuration: None,
                deferred_configuration: None,
                configuration_loader: RuntimeConfigurationLoader::spawn().ok(),
                lexicon_candidate,
                lexicon_saves: LexiconSaves::new(),
                diagnostic_sender: Some(diagnostic_sender),
                diagnostic_worker: Some(diagnostic_worker),
                pending_key_releases: Vec::new(),
                pending_held_replay: Vec::new(),
                pending_hand_back: None,
                typed_input: 0,
            });
        });
        // The agent starts with the session, possibly before the taskbar exists. It works without
        // the icon meanwhile: a refused icon is retried with backoff, and TaskbarCreated adds it as
        // soon as Explorer is ready.
        add_tray(hwnd, TrayStatus::initial(initial_indicator));

        let keyboard_hook =
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), Some(instance), 0) {
                Ok(hook) => hook,
                Err(error) => {
                    drop(take_app_state());
                    abandon_startup(hwnd);
                    return Err(error);
                }
            };
        let mouse_hook =
            match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), Some(instance), 0) {
                Ok(hook) => hook,
                Err(error) => {
                    let _ = UnhookWindowsHookEx(keyboard_hook);
                    drop(take_app_state());
                    abandon_startup(hwnd);
                    return Err(error);
                }
            };
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.keyboard_hook = Some(keyboard_hook);
                state.mouse_hook = Some(mouse_hook);
            }
        });
        touch_hook_activity(&KEYBOARD_HOOK_ACTIVITY_MS);
        touch_hook_activity(&MOUSE_HOOK_ACTIVITY_MS);

        if SetTimer(Some(hwnd), LAYOUT_TIMER_ID, LAYOUT_TIMER_INTERVAL_MS, None) == 0 {
            let error = Error::from_thread();
            abandon_startup(hwnd);
            return Err(error);
        }

        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, None, 0, 0);
            if result.0 == -1 {
                return Err(Error::from_thread());
            }
            if result.0 == 0 {
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        drop(take_app_state());
        Ok(())
    }
}

/// Destroys the agent's window after a start-up failure. Its WM_DESTROY posts WM_QUIT for a
/// message loop that never ran; left in the queue, that message would close the error dialog that
/// `main` shows next before anyone could read it.
fn abandon_startup(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool() {}
    }
}

/// Exercise the real startup configuration/package loader without installing
/// hooks or showing a dialog. Errors are reported as an exit code, not profile
/// text, so automated deployment checks do not disclose user data.
pub fn verify_profile() -> i32 {
    if load_runtime_configuration().is_ok() {
        0
    } else {
        20
    }
}

pub fn run_settings() -> core::result::Result<(), String> {
    install_crash_log();
    settings_window::run()
}

pub fn show_fatal_error(message: &str) {
    let text = HSTRING::from(message);
    unsafe {
        MessageBoxW(
            None,
            &text,
            &HSTRING::from(tr("error.startup_title")),
            ui_localization::message_box_style(MB_OK | MB_ICONERROR),
        );
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    guarded(
        || unsafe { window_proc_body(hwnd, message, wparam, lparam) },
        || {
            ENGINE_FAULT.store(true, Ordering::Release);
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        },
    )
}

unsafe fn window_proc_body(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if matches!(
        message,
        WM_COMMAND | TRAY_CALLBACK_MESSAGE | SETTINGS_APPLIED_MESSAGE
    ) && APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|state| state.shutdown.load(Ordering::Acquire))
    }) {
        return LRESULT(0);
    }
    match message {
        WM_TIMER if wparam.0 == LAYOUT_TIMER_ID => {
            report_timer_stall();
            if finish_shutdown(hwnd) {
                return LRESULT(0);
            }
            handle_engine_fault(hwnd);
            hook_watchdog_tick(hwnd);
            retry_missing_hooks(hwnd);
            retry_tray(hwnd);
            // Publishing first frees the slot for a configuration loaded meanwhile.
            finish_configuration_reload(hwnd);
            finish_configuration_load(hwnd);
            expire_correction_gate();
            expire_retained_input(hwnd, Instant::now());
            hand_back_stalled_retained_input(
                current_foreground_context(),
                physical_modifiers_released(),
                Instant::now(),
            );
            // In case the message that normally sends them could not be posted.
            send_pending_held_keys();
            expire_lexicon_candidate();
            refresh_tray_state();
            LRESULT(0)
        }
        GATE_ARM_MESSAGE => {
            let token = wparam.0 as u64;
            let foreground = current_foreground_context();
            let armed = APP_STATE.with(|slot| {
                let mut borrowed = slot.borrow_mut();
                let Some(state) = borrowed.as_mut() else {
                    return false;
                };
                if state.shutdown.load(Ordering::Acquire)
                    || token == 0
                    || foreground.is_none()
                    || state.correction_gate.active
                    || has_retained_input(&state.metrics)
                    || !gate_request_enabled(&state.metrics, token)
                    || state
                        .metrics
                        .observed_input_sequence
                        .load(Ordering::Acquire)
                        != token
                {
                    return false;
                }
                state.correction_gate.activate(token);
                state.gate_diagnostic("armed", format!("token={token} source=worker"));
                state.correction_gate.origin = foreground;
                state
                    .metrics
                    .active_gate_token
                    .store(token, Ordering::Release);
                state
                    .metrics
                    .cancelled_gate_token
                    .store(0, Ordering::Release);
                true
            });
            LRESULT(armed as isize)
        }
        GATE_RELEASE_MESSAGE => {
            drain_correction_gate(wparam.0 as u64);
            LRESULT(0)
        }
        GATE_DRAIN_ACK_MESSAGE => {
            let old_token = wparam.0 as u64;
            let new_token = u64::try_from(lparam.0).unwrap_or_default();
            LRESULT(acknowledge_correction_gate_drain(old_token, new_token) as isize)
        }
        GATE_FAIL_OPEN_MESSAGE => {
            fail_open_correction_gate(wparam.0 as u64, fail_open_cause(lparam.0));
            LRESULT(0)
        }
        HELD_KEYS_RETURN_MESSAGE => {
            send_pending_held_keys();
            LRESULT(0)
        }
        GATE_HANDOFF_BUDGET_MESSAGE => {
            let old_token = wparam.0 as u64;
            let required_ms = u64::try_from(lparam.0).unwrap_or_default();
            let prepared = APP_STATE.with(|slot| {
                slot.borrow_mut().as_mut().is_some_and(|state| {
                    state
                        .correction_gate
                        .prepare_handoff_budget(old_token, required_ms)
                })
            });
            LRESULT(prepared as isize)
        }
        LEXICON_SAVED_MESSAGE => {
            finish_lexicon_saves(hwnd);
            LRESULT(0)
        }
        LEXICON_OFFER_MESSAGE => {
            if let Some(candidate) = valid_lexicon_candidate() {
                let (title, message) = match candidate.target {
                    LexiconOfferTarget::UserDictionary => {
                        (tr("conversion.forced_title"), tr("conversion.forced_body"))
                    }
                    LexiconOfferTarget::WordExclusions => {
                        (tr("conversion.undone_title"), tr("conversion.undone_body"))
                    }
                };
                show_tray_information(hwnd, title, message);
            }
            LRESULT(0)
        }
        SETTINGS_APPLIED_MESSAGE => {
            reload_configuration_from_disk(hwnd);
            LRESULT(0)
        }
        RECOVERY_NOTICE_MESSAGE => {
            // A stopped worker gets its own notice; recovery would be an empty promise.
            if APP_STATE.with(|slot| {
                slot.borrow().as_ref().is_some_and(|state| {
                    has_retained_input(&state.metrics)
                        && !state.metrics.worker_dead.load(Ordering::Acquire)
                })
            }) {
                show_tray_information(
                    hwnd,
                    tr("recovery.retained_title"),
                    tr("recovery.retained_body"),
                );
            }
            LRESULT(0)
        }
        TRAY_CALLBACK_MESSAGE => {
            let notification = (lparam.0 as u32) & 0xffff;
            match tray_interaction(notification, wparam) {
                TrayInteraction::Settings => settings_window::show(hwnd),
                TrayInteraction::QuickMenu(anchor) => {
                    let _ = show_tray_menu(hwnd, anchor);
                }
                TrayInteraction::Ignore => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            match wparam.0 & 0xffff {
                MENU_AUTO_ID => toggle_auto_enabled(),
                MENU_ADD_LEXICON_ENTRY_ID => add_recent_lexicon_offer(hwnd),
                MENU_RECOVER_INPUT_ID => arm_input_recovery(hwnd),
                MENU_DISCARD_INPUT_ID => discard_retained_input(hwnd),
                MENU_SETTINGS_ID => settings_window::show(hwnd),
                MENU_EXIT_ID => request_shutdown(hwnd, true),
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            request_shutdown(hwnd, false);
            LRESULT(0)
        }
        WM_WTSSESSION_CHANGE => {
            if let Some(reason) = session_reinstall_reason(wparam.0 as u32) {
                reinstall_hooks(reason);
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            if let Some(reason) = power_reinstall_reason(wparam.0 as u32) {
                reinstall_hooks(reason);
            }
            // TRUE grants the request for the messages that ask for one.
            LRESULT(1)
        }
        message if message != 0 && message == TASKBAR_CREATED.load(Ordering::Acquire) => {
            recreate_tray(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                let _ = WTSUnRegisterSessionNotification(hwnd);
            }
            installer_lifecycle::remove_safe_close(hwnd);
            drop(take_app_state());
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TrayInteraction {
    Settings,
    /// The menu, opened at this screen point.
    QuickMenu(POINT),
    Ignore,
}

/// What a notification of the icon asks for. With NOTIFYICON_VERSION_4, `anchor` (wParam) holds
/// where a menu belongs: the pointer after a right click, the icon after the menu key or
/// Shift+F10.
fn tray_interaction(notification: u32, anchor: WPARAM) -> TrayInteraction {
    match notification {
        WM_LBUTTONDBLCLK | NIN_KEYSELECT | NIN_SELECT => TrayInteraction::Settings,
        WM_CONTEXTMENU => TrayInteraction::QuickMenu(point_from_packed_word(anchor)),
        _ => TrayInteraction::Ignore,
    }
}

/// Hook callbacks must not unwind (that aborts the process). A contained panic passes the event
/// on untouched and raises `ENGINE_FAULT`.
unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    touch_hook_activity(&KEYBOARD_HOOK_ACTIVITY_MS);
    let started = Instant::now();
    let result = guarded(
        || unsafe { keyboard_hook_body(code, wparam, lparam) },
        || {
            ENGINE_FAULT.store(true, Ordering::Release);
            unsafe { CallNextHookEx(None, code, wparam, lparam) }
        },
    );
    report_slow_hook(started, "keyboard");
    result
}

/// Records a hook callback that used a large part of the time Windows allows it.
fn report_slow_hook(started: Instant, hook: &str) {
    let elapsed = started.elapsed();
    if elapsed < Duration::from_millis(HOOK_SLOW_DIAGNOSTIC_MS) {
        return;
    }
    APP_STATE.with(|slot| {
        // The callback may run while this thread holds the state; skip the record then.
        if let Ok(borrowed) = slot.try_borrow()
            && let Some(state) = borrowed.as_ref()
            && slow_hook_may_be_logged(hook, state.metrics.privacy_reason.load(Ordering::Acquire))
        {
            state.event_diagnostic(
                "hook_slow",
                format!("hook={hook} elapsed_ms={}", elapsed.as_millis()),
            );
        }
    });
}

/// A slow keyboard callback is one line per key when the system is loaded: while privacy blocks
/// the field it is not written, as it would give a secret's length and rhythm.
const fn slow_hook_may_be_logged(hook: &str, privacy_reason: u8) -> bool {
    privacy_reason == PRIVACY_ALLOWED || !matches!(hook.as_bytes(), b"keyboard")
}

/// Whether an injected key is the desktop acceptance typing for the agent (test builds only).
#[cfg(test)]
const fn acceptance_typing(marker: usize) -> bool {
    marker == ACCEPTANCE_TYPING_MARKER
}

#[cfg(not(test))]
const fn acceptance_typing(_marker: usize) -> bool {
    false
}

unsafe fn keyboard_hook_body(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let mut swallow = false;
    let mut forwarded_sequence = None;
    let mut drained_event = false;
    if code >= 0 {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if event.dwExtraInfo == INJECTED_EVENT_MARKER {
            // Events created by this process are intentionally invisible to
            // the input state machine.
        } else {
            let foreground = current_foreground_context();
            APP_STATE.with(|slot| {
                if let Some(state) = slot.borrow_mut().as_mut() {
                    let message = wparam.0 as u32;
                    let key_down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
                    let key_up = matches!(message, WM_KEYUP | WM_SYSKEYUP);
                    if is_owned_gate_fence_marker(event.dwExtraInfo) {
                        swallow = true;
                        if !state.correction_gate.active
                            || event.dwExtraInfo != state.correction_gate.fence_marker
                        {
                            return;
                        }
                        if key_up {
                            if state.correction_gate.cancelled {
                                state.break_correction_gate_fail_open(
                                    GateFailureCause::DownstreamHook,
                                );
                            } else {
                                let token = state.correction_gate.token;
                                let drained_count = state.correction_gate.current_drain_count;
                                let replay_succeeded = !state.correction_gate.drain_failed
                                    && !state.correction_gate.cancelled;
                                state.correction_gate.current_drain_count = 0;
                                state.correction_gate.drain_in_flight = false;
                                state.correction_gate.awaiting_worker_ack = true;
                                if !state.enqueue(RawInputEvent::GateDrainReady {
                                    token,
                                    drained_count,
                                    replay_succeeded,
                                }) {
                                    state.break_correction_gate_fail_open(
                                        GateFailureCause::WorkerQueue,
                                    );
                                }
                            }
                        }
                        return;
                    }

                    let drained = event.dwExtraInfo == DRAINED_EVENT_MARKER;
                    if state.shutdown.load(Ordering::Acquire) {
                        return;
                    }
                    drained_event = drained;
                    if event.flags.contains(LLKHF_INJECTED)
                        && !drained
                        && !acceptance_typing(event.dwExtraInfo)
                    {
                        state.break_correction_gate_fail_open(GateFailureCause::ExternalInjection);
                        state.typed_input = state.typed_input.wrapping_add(1);
                        state
                            .metrics
                            .observed_input_sequence
                            .fetch_add(1, Ordering::AcqRel);
                        state.enqueue(RawInputEvent::ExternalInjection);
                        return;
                    }

                    let Some(foreground) = foreground else {
                        state.break_correction_gate_fail_open(GateFailureCause::FocusChanged);
                        state.enqueue(RawInputEvent::ExternalInjection);
                        return;
                    };
                    let mut raw = RawKeyEvent {
                        message,
                        virtual_key: event.vkCode,
                        scan_code: event.scanCode,
                        caps_lock: unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 },
                        extended: event.flags.contains(LLKHF_EXTENDED),
                        foreground,
                        sequence: 0,
                        drain_token: if drained {
                            state.correction_gate.token
                        } else {
                            0
                        },
                        hotkey_trigger: false,
                        repeat: false,
                        held_modifiers: held_modifiers_for_event(
                            event.vkCode as u16,
                            key_down,
                            key_up,
                        ),
                    };
                    if !drained {
                        // A newer physical edge of a key supersedes a key-up still queued for it.
                        cancel_superseded_key_release(&mut state.pending_key_releases, &raw);
                    }

                    let mut claimed_hotkey = false;
                    // The claimed hotkey is a modifier key-up that also reaches the application.
                    let mut passes_through = false;
                    if !drained {
                        match contextual_hotkey_action(
                            &state.metrics,
                            message,
                            event.vkCode as u16,
                            pressed_hotkey_modifiers_for_event(
                                event.vkCode as u16,
                                key_down,
                                key_up,
                            ),
                        ) {
                            UndoHotkeyAction::PassThrough => {}
                            UndoHotkeyAction::Swallow => {
                                swallow = true;
                                return;
                            }
                            UndoHotkeyAction::TriggerSwallow => {
                                swallow = true;
                                if state.correction_gate.active {
                                    raw.hotkey_trigger = true;
                                    state.correction_gate.defer_hotkey(raw);
                                    state.gate_diagnostic(
                                        "hotkey_deferred",
                                        format!("token={}", state.correction_gate.token),
                                    );
                                    return;
                                }
                                claimed_hotkey = true;
                                raw.hotkey_trigger = true;
                            }
                            UndoHotkeyAction::TriggerPassThrough => {
                                if state.correction_gate.active {
                                    raw.hotkey_trigger = true;
                                    state.correction_gate.defer_hotkey(raw);
                                    // Modifier-up belongs in the same raw FIFO as
                                    // its held modifier-down; it is not swallowed
                                    // without a corresponding eventual replay.
                                    raw.hotkey_trigger = false;
                                } else {
                                    claimed_hotkey = true;
                                    passes_through = true;
                                    raw.hotkey_trigger = true;
                                }
                            }
                        }
                    }

                    if !drained && !claimed_hotkey && state.correction_gate.active {
                        if state
                            .correction_gate
                            .origin
                            .is_some_and(|origin| !same_input_target(origin, foreground))
                        {
                            state.break_correction_gate_fail_open(GateFailureCause::FocusChanged);
                        }
                        if key_down && hotkey_modifier_bit(event.vkCode as u16).is_none() {
                            state.correction_gate.pending_hotkey = None;
                        }
                        if state.correction_gate.active && key_down {
                            raw.repeat =
                                gated_key_down_repeats(&state.correction_gate.held, &raw, || {
                                    let key_state =
                                        unsafe { GetAsyncKeyState(event.vkCode as i32) };
                                    key_state as u16 & 0x8000 != 0
                                });
                        }
                        if state.hold_gated_edge(raw) {
                            swallow = true;
                            return;
                        }
                    }

                    if key_down && !drained {
                        state.typed_input = state.typed_input.wrapping_add(1);
                    }
                    let sequence = state
                        .metrics
                        .observed_input_sequence
                        .fetch_add(1, Ordering::AcqRel)
                        .wrapping_add(1);
                    forwarded_sequence = Some(sequence);
                    raw.sequence = sequence;
                    if claimed_hotkey {
                        state
                            .metrics
                            .explicit_hotkey_sequence
                            .store(sequence, Ordering::Release);
                    }
                    if should_arm_space_decision_gate(
                        drained,
                        key_down,
                        event.vkCode as u16,
                        state.metrics.auto_enabled.load(Ordering::Acquire),
                        state.metrics.privacy_reason.load(Ordering::Acquire),
                        state.correction_gate.active,
                        shortcut_modifiers_released(),
                    ) && !has_retained_input(&state.metrics)
                        && !state.metrics.configuration_pending.load(Ordering::Acquire)
                    {
                        state.correction_gate.activate(sequence);
                        state.gate_diagnostic("armed", format!("token={sequence}"));
                        state.correction_gate.origin = Some(foreground);
                        state
                            .metrics
                            .active_gate_token
                            .store(sequence, Ordering::Release);
                        state
                            .metrics
                            .cancelled_gate_token
                            .store(0, Ordering::Release);
                    }
                    let queued = if claimed_hotkey && has_retained_input(&state.metrics) {
                        let armed = state.metrics.recovery_armed.swap(false, Ordering::AcqRel);
                        if !armed {
                            state.gate_diagnostic(
                                "recovery_not_armed",
                                format!("sequence={sequence}"),
                            );
                        }
                        let (key, recovery) = retained_hotkey_events(raw, passes_through, armed);
                        let key_queued = key.is_none_or(|key| state.enqueue(key));
                        key_queued && recovery.is_none_or(|recovery| state.enqueue(recovery))
                    } else {
                        state.enqueue(RawInputEvent::Key(raw))
                    };
                    if !queued
                        && state.correction_gate.active
                        && (drained || state.correction_gate.token == sequence)
                    {
                        state.correction_gate.drain_failed = true;
                        state.break_correction_gate_fail_open(GateFailureCause::WorkerQueue);
                    }
                }
            });
        }
    }
    let result = if swallow {
        LRESULT(1)
    } else {
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    };
    if !swallow && let Some(sequence) = forwarded_sequence {
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                if result.0 == 0 {
                    state
                        .metrics
                        .forwarded_input_sequence
                        .store(sequence, Ordering::Release);
                } else {
                    // A downstream hook consumed this event, so the worker's
                    // buffer no longer matches the target application.
                    if !drained_event
                        && state.correction_gate.active
                        && state.correction_gate.token == sequence
                    {
                        state.break_correction_gate_fail_open(GateFailureCause::DownstreamHook);
                    }
                    if drained_event && state.correction_gate.active {
                        state.correction_gate.cancelled = true;
                        state.correction_gate.drain_failed = true;
                        state
                            .metrics
                            .cancelled_gate_token
                            .store(state.correction_gate.token, Ordering::Release);
                    }
                    state.enqueue(RawInputEvent::ExternalInjection);
                }
            }
        });
    }
    result
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    touch_hook_activity(&MOUSE_HOOK_ACTIVITY_MS);
    let started = Instant::now();
    let result = guarded(
        || unsafe { mouse_hook_body(code, wparam, lparam) },
        || {
            ENGINE_FAULT.store(true, Ordering::Release);
            unsafe { CallNextHookEx(None, code, wparam, lparam) }
        },
    );
    report_slow_hook(started, "mouse");
    result
}

unsafe fn mouse_hook_body(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                observe_mouse_button(state, event.flags & LLMHF_INJECTED != 0);
            }
        });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// A mouse button went down. A click, physical or injected by another program, may move the caret,
/// so held keys must not be replayed after it: the gate breaks either way.
fn observe_mouse_button(state: &mut AppState, injected: bool) {
    state.typed_input = state.typed_input.wrapping_add(1);
    state
        .metrics
        .observed_input_sequence
        .fetch_add(1, Ordering::AcqRel);
    if injected {
        state.break_correction_gate_fail_open(GateFailureCause::ExternalInjection);
        state.enqueue(RawInputEvent::ExternalInjection);
    } else {
        state.break_correction_gate_fail_open(GateFailureCause::Mouse);
        state.enqueue(RawInputEvent::Mouse);
    }
}

fn drain_correction_gate(token: u64) {
    let current = current_foreground_context();
    let batch = APP_STATE.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let state = borrowed.as_mut()?;
        if !state.correction_gate.can_start_drain(token) {
            return None;
        }
        if state
            .correction_gate
            .origin
            .is_none_or(|origin| current.is_none_or(|now| !same_input_target(origin, now)))
        {
            state.break_correction_gate_fail_open(GateFailureCause::FocusChanged);
            return None;
        }

        let held = take_held_replay_prefix(&mut state.correction_gate.held);
        state.correction_gate.released_count += held.len();
        state.correction_gate.current_drain_count = held.len();
        state.correction_gate.drain_in_flight = true;
        state.gate_diagnostic(
            "submit",
            format!(
                "token={token} edges={} remaining={}",
                held.len(),
                state.correction_gate.held.len()
            ),
        );
        if held_tail_invalidates_undo(&held) {
            state.metrics.undo_available.store(false, Ordering::Release);
        }
        Some((held, state.correction_gate.fence_marker))
    });
    let Some((held, fence_marker)) = batch else {
        return;
    };
    let Some(inputs) = build_held_replay_inputs(&held, fence_marker) else {
        fail_correction_gate_drain(token, held, 0);
        return;
    };
    let sent = send_input_count(&inputs);
    if sent != inputs.len() {
        // Do not blindly retry an uncertain, partially accepted batch.
        fail_correction_gate_drain(token, held, sent);
    }
}

fn fail_correction_gate_drain(token: u64, unsent: Vec<RawKeyEvent>, sent: usize) {
    APP_STATE.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(state) = borrowed.as_mut() else {
            return;
        };
        state.retain_failed_drain(token, unsent, sent);
    });
}

/// The events a claimed hotkey queues while held input is retained: the explicit recovery when it
/// was armed, and, when the hotkey fired on a modifier key-up that also reaches the application,
/// that key-up as a plain key event, so the worker sees the modifier released.
fn retained_hotkey_events(
    raw: RawKeyEvent,
    passes_through: bool,
    recovery_armed: bool,
) -> (Option<RawInputEvent>, Option<RawInputEvent>) {
    let key = passes_through.then_some(RawInputEvent::Key(RawKeyEvent {
        hotkey_trigger: false,
        ..raw
    }));
    let recovery = recovery_armed.then_some(RawInputEvent::RecoverInput {
        sequence: raw.sequence,
        explicit: true,
    });
    (key, recovery)
}

/// Whether a key-down a gate is about to hold repeats a key that is already down. The last edge of
/// the same key the gate holds decides: the system never saw it, so its key state still shows a
/// key released in the gate as down. Without one, `system_down` reads the async key state, which
/// the hook updates only after this edge: it still tells whether the key was down before it.
fn gated_key_down_repeats(
    held: &[RawKeyEvent],
    raw: &RawKeyEvent,
    system_down: impl FnOnce() -> bool,
) -> bool {
    let key = (raw.virtual_key, raw.scan_code, raw.extended);
    held.iter()
        .rev()
        .find(|event| (event.virtual_key, event.scan_code, event.extended) == key)
        .map_or_else(system_down, |last| {
            matches!(last.message, WM_KEYDOWN | WM_SYSKEYDOWN)
        })
}

/// Splits edges that leave a gate without being typed into the ones that stay retained and the
/// key-ups that must be delivered now. A key-up with no earlier fresh key-down of the same key in
/// `events` belongs to a key whose key-down reached the application before the gate (directly or
/// as the first of an auto-repeat): it types nothing, and withholding it would leave the key
/// logically pressed system-wide. A key-down after a key-up of the same key in `events` is fresh
/// whatever it is marked.
fn split_delivered_key_releases(events: Vec<RawKeyEvent>) -> (Vec<RawKeyEvent>, Vec<RawKeyEvent>) {
    let mut pressed = std::collections::BTreeSet::new();
    let mut released = std::collections::BTreeSet::new();
    let mut retained = Vec::with_capacity(events.len());
    let mut releases = Vec::new();
    for event in events {
        let key = (event.virtual_key, event.scan_code, event.extended);
        if matches!(event.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
            if !event.repeat || released.contains(&key) {
                pressed.insert(key);
            }
            retained.push(event);
        } else {
            released.insert(key);
            if pressed.remove(&key) {
                retained.push(event);
            } else {
                releases.push(event);
            }
        }
    }
    (retained, releases)
}

/// Drops queued key-ups of the key `event` belongs to: a newer physical edge of that key has
/// already set its state, and delivering the old key-up afterwards would release it again.
fn cancel_superseded_key_release(pending: &mut Vec<RawKeyEvent>, event: &RawKeyEvent) {
    if !pending.is_empty() {
        pending.retain(|queued| {
            (queued.virtual_key, queued.scan_code, queued.extended)
                != (event.virtual_key, event.scan_code, event.extended)
        });
    }
}

/// Held keys a broken gate queued for the window procedure, ready to send.
struct HeldKeysReturn {
    inputs: Vec<INPUT>,
    /// The retained record handed back with them past a stalled worker.
    handed_back: Option<u64>,
}

/// Takes what a broken gate queued: the held keys handed back unchanged after a contained fault or
/// past a stalled worker, then the key-ups of keys pressed before the gate. A key they leave down
/// is released at the end, as in a recovery.
fn take_held_keys_return(state: &mut AppState) -> Option<HeldKeysReturn> {
    if state.pending_held_replay.is_empty() && state.pending_key_releases.is_empty() {
        return None;
    }
    let mut events = core::mem::take(&mut state.pending_held_replay);
    events.append(&mut state.pending_key_releases);
    Some(HeldKeysReturn {
        inputs: build_balanced_replay_inputs(&events).unwrap_or_default(),
        handed_back: state.pending_hand_back.take(),
    })
}

/// Records that `sent` of the inputs of `held` were sent; a record handed back past a stalled
/// worker is reported to the worker, which finishes its recovery.
fn finish_held_keys_return(state: &AppState, held: &HeldKeysReturn, sent: usize) {
    state.gate_diagnostic(
        "returned",
        format!("requested={} submitted={sent}", held.inputs.len()),
    );
    if let Some(token) = held.handed_back {
        let _ = state.enqueue(RawInputEvent::RetainedInputHandedBack {
            token,
            submitted: sent,
            requested: held.inputs.len(),
        });
    }
}

/// Sends what a broken gate queued (see take_held_keys_return). Runs in the window procedure,
/// never in a hook.
fn send_pending_held_keys() {
    let Some(held) = APP_STATE.with(|slot| take_held_keys_return(slot.borrow_mut().as_mut()?))
    else {
        return;
    };
    let sent = send_input_count(&held.inputs);
    APP_STATE.with(|slot| {
        if let Ok(borrowed) = slot.try_borrow()
            && let Some(state) = borrowed.as_ref()
        {
            finish_held_keys_return(state, &held, sent);
        }
    });
}

fn take_held_replay_prefix(held: &mut Vec<RawKeyEvent>) -> Vec<RawKeyEvent> {
    let end = held
        .iter()
        .position(|event| {
            event.virtual_key as u16 == VK_SPACE.0
                && matches!(event.message, WM_KEYDOWN | WM_SYSKEYDOWN)
        })
        .map_or(held.len(), |index| index + 1);
    held.drain(..end).collect()
}

/// Whether replaying `held` after a conversion changes the text after the converted word, which
/// ends its undo. Only a key-down of a non-modifier key types; a key-up never does (a letter
/// released after the Space while typing fast, or the Space's own key-up).
fn held_tail_invalidates_undo(held: &[RawKeyEvent]) -> bool {
    held.iter().any(|event| {
        hotkey_modifier_bit(event.virtual_key as u16).is_none()
            && matches!(event.message, WM_KEYDOWN | WM_SYSKEYDOWN)
    })
}

const fn is_owned_gate_fence_marker(marker: usize) -> bool {
    marker & !0x7fff == GATE_FENCE_MARKER_BASE
}

fn acknowledge_correction_gate_drain(old_token: u64, new_token: u64) -> bool {
    let mut continue_drain = false;
    let acknowledged = APP_STATE.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(state) = borrowed.as_mut() else {
            return false;
        };
        if !state.correction_gate.active
            || state.correction_gate.token != old_token
            || !state.correction_gate.awaiting_worker_ack
        {
            return false;
        }
        if Instant::now() >= state.correction_gate.deadline
            && (new_token != 0 || !state.correction_gate.held.is_empty())
        {
            state.break_correction_gate_fail_open(GateFailureCause::Deadline);
            return false;
        }

        if new_token != 0 {
            if state
                .metrics
                .observed_input_sequence
                .load(Ordering::Acquire)
                != new_token
                || !state.metrics.auto_enabled.load(Ordering::Acquire)
                || !state
                    .correction_gate
                    .handoff(old_token, new_token, Instant::now())
            {
                return false;
            }
            state
                .metrics
                .active_gate_token
                .store(new_token, Ordering::Release);
            state
                .metrics
                .cancelled_gate_token
                .store(0, Ordering::Release);
            return true;
        }

        state.correction_gate.awaiting_worker_ack = false;
        state.correction_gate.handoff_budget_ms = 0;
        if state.correction_gate.held.is_empty() {
            state.close_drained_gate();
        } else {
            continue_drain = true;
        }
        true
    });

    if acknowledged && continue_drain {
        let posted = unsafe {
            PostMessageW(
                APP_STATE.with(|slot| slot.borrow().as_ref().map(|state| state.window)),
                GATE_RELEASE_MESSAGE,
                WPARAM(old_token as usize),
                LPARAM(0),
            )
            .is_ok()
        };
        if !posted {
            fail_open_correction_gate(old_token, GateFailureCause::WorkerAck);
        }
    }
    acknowledged
}

fn fail_open_correction_gate(token: u64, cause: GateFailureCause) {
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut()
            && state.correction_gate.active
            && state.correction_gate.token == token
        {
            state.break_correction_gate_fail_open(cause);
        }
    });
}

/// Causes after which held keys come back without being asked: the worker recovers them once it
/// sees the abort, or the UI thread does when the worker stays busy.
const fn hands_held_keys_back_automatically(cause: GateFailureCause) -> bool {
    matches!(
        cause,
        GateFailureCause::Deadline
            | GateFailureCause::Capacity
            | GateFailureCause::WorkerQueue
            | GateFailureCause::HookReinstall
    )
}

/// What the UI thread knows when it considers handing retained keys back past a busy worker.
#[derive(Debug, Clone, Copy)]
struct HandBackFacts {
    now: Instant,
    /// The worker is busy, and not inside an edit that may have selected or typed text.
    worker_stalled: bool,
    /// `AppState::typed_input` now.
    typed_input: u64,
    /// The gate whose paste may have left the word selected (`ObserverMetrics`).
    selection_left_token: u64,
    modifiers_released: bool,
    current: Option<ForegroundContext>,
    privacy: Option<PrivacyBlockReason>,
}

/// Hands held keys back from the UI thread when the worker that should recover them stays busy:
/// a provider call that never returns would otherwise keep them until they are deleted. See
/// `stalled_worker_may_hand_back` for when this is safe.
fn hand_back_stalled_retained_input(
    current: Option<ForegroundContext>,
    modifiers_released: bool,
    now: Instant,
) {
    APP_STATE.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(state) = borrowed.as_mut() else {
            return;
        };
        let facts = HandBackFacts {
            now,
            worker_stalled: state.metrics.worker_busy.load(Ordering::Acquire)
                && !state.metrics.edit_in_flight.load(Ordering::Acquire),
            typed_input: state.typed_input,
            selection_left_token: state.metrics.selection_left_token.load(Ordering::Acquire),
            modifiers_released,
            current,
            privacy: privacy_reason_from_code(state.metrics.privacy_reason.load(Ordering::Acquire)),
        };
        let record = {
            let mut retained = state
                .metrics
                .retained_input
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if retained
                .as_ref()
                .is_some_and(|input| stalled_worker_may_hand_back(input, facts))
            {
                retained.take()
            } else {
                None
            }
        };
        let Some(record) = record else {
            return;
        };
        state.metrics.recovery_armed.store(false, Ordering::Release);
        state.gate_diagnostic(
            "recovery_handed_back",
            format!("token={} held={}", record.token, record.events.len()),
        );
        state.pending_held_replay.extend(record.events);
        state.pending_hand_back = Some(record.token);
        state.post_held_keys_return();
    });
}

/// Whether the UI thread hands a retained record back itself: the record is one that comes back
/// automatically, is not being sent and was certainly never typed; the worker has been busy for
/// `STALLED_WORKER_HAND_BACK_MS` since the keys were retained, but not inside an edit that may
/// have selected or typed text; nothing was typed or clicked since, and no modifier is held; no
/// paste of its gate may have left the word selected; the input target is still the record's;
/// and no positive privacy finding keeps the keys back.
fn stalled_worker_may_hand_back(input: &RetainedInput, facts: HandBackFacts) -> bool {
    input.auto_recover
        && !input.recovering
        && !input.delivery_uncertain
        && facts.worker_stalled
        && facts.typed_input == input.input_mark
        && facts.selection_left_token != input.token
        && facts.modifiers_released
        && facts.now.saturating_duration_since(input.retained_at)
            >= Duration::from_millis(STALLED_WORKER_HAND_BACK_MS)
        && facts
            .current
            .is_some_and(|current| same_input_target(input.foreground, current))
        && recovery_privacy_allows(facts.privacy)
}

/// The cause a worker's fail-open request names in its LPARAM: a selection the edit may have
/// left, or else a drain acknowledgement that was refused.
const fn fail_open_cause(code: isize) -> GateFailureCause {
    if code == GATE_FAIL_OPEN_SELECTION_LEFT {
        GateFailureCause::SelectionLeft
    } else {
        GateFailureCause::WorkerAck
    }
}

/// Deletes held keys that were neither handed back nor recovered within their lifetime. They were
/// never typed, and keeping them would keep automatic conversion paused, the gate disarmed and
/// Exit declined until someone acted on the tray notice. The worker then decides whether
/// automatic conversion resumes, under the budget of a recovered gate abort (policy D1).
fn expire_retained_input(hwnd: HWND, now: Instant) {
    let expired = APP_STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let state = state.as_mut()?;
        let expired = {
            let mut retained = state
                .metrics
                .retained_input
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let deadline = |input: &RetainedInput| {
                if input.recovering {
                    input.expires_at + RETAINED_RECOVERY_GRACE
                } else {
                    input.expires_at
                }
            };
            if retained
                .as_ref()
                .is_some_and(|input| now >= deadline(input))
            {
                retained.take()
            } else {
                None
            }
        }?;
        state.metrics.recovery_armed.store(false, Ordering::Release);
        state.gate_diagnostic(
            "recovery_discarded",
            format!(
                "explicit=false reason=expired token={} held={}",
                expired.token,
                expired.events.len()
            ),
        );
        // Deleting a key-up whose key-down was typed would leave that key pressed.
        let (_, releases) = split_delivered_key_releases(expired.events);
        state.queue_delivered_key_releases(releases);
        let _ = state.enqueue(RawInputEvent::RetainedInputExpired);
        Some(())
    });
    if expired.is_some() {
        show_tray_information(
            hwnd,
            tr("recovery.discarded_title"),
            tr("recovery.expired_body"),
        );
    }
}

fn expire_correction_gate() {
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut()
            && state.correction_gate.active
            && Instant::now() >= state.correction_gate.deadline
        {
            if state.correction_gate.awaiting_worker_ack && state.correction_gate.held.is_empty() {
                state.close_drained_gate();
            } else {
                state.break_correction_gate_fail_open(GateFailureCause::Deadline);
            }
        }
    });
}

fn current_foreground_context() -> Option<ForegroundContext> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut process_id = 0;
        let foreground_thread = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        if foreground_thread == 0 {
            return None;
        }

        let mut input_thread_id = foreground_thread;
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(foreground_thread, &mut info).is_ok() && !info.hwndFocus.0.is_null() {
            let mut focused_process_id = 0;
            let focused_thread =
                GetWindowThreadProcessId(info.hwndFocus, Some(&mut focused_process_id));
            if focused_thread != 0 && focused_process_id == process_id {
                input_thread_id = focused_thread;
            }
        }

        let layout = GetKeyboardLayout(input_thread_id);
        Some(ForegroundContext {
            hwnd: hwnd.0 as usize,
            focus: info.hwndFocus.0 as usize,
            input_thread_id,
            process_id,
            layout: layout.0 as usize,
        })
    }
}

fn is_shell_process(process_id: u32) -> bool {
    unsafe {
        let shell_window = GetShellWindow();
        if !shell_window.0.is_null() {
            let mut shell_process_id = 0;
            GetWindowThreadProcessId(shell_window, Some(&mut shell_process_id));
            return process_id != 0 && process_id == shell_process_id;
        }
    }
    false
}

fn focused_window_for_context(expected: ForegroundContext) -> HWND {
    let top_level = HWND(expected.hwnd as *mut c_void);
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(expected.input_thread_id, &mut info).is_err() {
            return top_level;
        }
        [info.hwndFocus, info.hwndCaret]
            .into_iter()
            .find(|window| window_belongs_to_context(*window, expected))
            .unwrap_or(top_level)
    }
}

fn window_belongs_to_context(window: HWND, expected: ForegroundContext) -> bool {
    if window.0.is_null() {
        return false;
    }
    let mut process_id = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
    thread_id == expected.input_thread_id && process_id == expected.process_id
}

struct PrivacyGuard {
    automation: Option<IUIAutomation>,
    com_initialized: bool,
    last_barrier_report: Cell<TextBarrierReport>,
}

impl PrivacyGuard {
    fn new() -> Self {
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            if !initialized {
                return Self {
                    automation: None,
                    com_initialized: false,
                    last_barrier_report: Cell::new(TextBarrierReport::default()),
                };
            }

            // Windows 8+ exposes bounded provider calls. Do not silently fall
            // back to the legacy client's 20-second transaction timeout.
            let automation: Option<IUIAutomation> = (|| -> Result<IUIAutomation> {
                let client: IUIAutomation2 =
                    CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
                client.SetConnectionTimeout(UIA_PROVIDER_TIMEOUT_MS)?;
                client.SetTransactionTimeout(UIA_PROVIDER_TIMEOUT_MS)?;
                client.cast()
            })()
            .ok();
            if automation.is_none() {
                CoUninitialize();
                return Self {
                    automation: None,
                    com_initialized: false,
                    last_barrier_report: Cell::new(TextBarrierReport::default()),
                };
            }

            Self {
                automation,
                com_initialized: true,
                last_barrier_report: Cell::new(TextBarrierReport::default()),
            }
        }
    }

    fn inspect(&self, expected_process_id: u32) -> Option<PrivacyBlockReason> {
        let Some(automation) = &self.automation else {
            return Some(PrivacyBlockReason::InspectionUnavailable);
        };
        unsafe {
            let Ok(element) = automation.GetFocusedElement() else {
                return Some(PrivacyBlockReason::FieldInspectionUnavailable);
            };
            let Ok(process_id) = element.CurrentProcessId() else {
                return Some(PrivacyBlockReason::InspectionUnavailable);
            };
            if process_id < 0 || process_id as u32 != expected_process_id {
                return Some(PrivacyBlockReason::InspectionUnavailable);
            }
            match element.CurrentIsPassword() {
                Ok(value) if value.as_bool() => Some(PrivacyBlockReason::PasswordField),
                Ok(_) => None,
                Err(_) => Some(PrivacyBlockReason::FieldInspectionUnavailable),
            }
        }
    }

    fn preceding_text_status(
        &self,
        expected_process_id: u32,
        expected_text: &str,
    ) -> TextBarrierStatus {
        let report = &self.last_barrier_report;
        report.set(TextBarrierReport {
            expected_chars: expected_text.chars().count(),
            ..TextBarrierReport::default()
        });
        let Some(units) = barrier_units(expected_text) else {
            set_barrier_stage(report, TextBarrierStage::TextLength);
            return TextBarrierStatus::Unavailable;
        };
        let field = match self.focused_text_field(expected_process_id) {
            Ok(field) => field,
            Err(status) => return status,
        };
        match read_preceding_text(&field, units, expected_text, report) {
            Ok(_) => TextBarrierStatus::Match,
            Err(status) => status,
        }
    }

    /// Selects the text before the caret, trying again only when the field did not show the
    /// selection and it was collapsed. Returns the status and what became of the selection when
    /// it was collapsed again.
    fn select_preceding_text(
        &self,
        expected_process_id: u32,
        expected_text: &str,
        patience: PastePatience,
    ) -> (TextBarrierStatus, Option<SelectionCleanup>) {
        let (attempt, attempts) = retry_unshown_selection(patience.attempts(), |earlier| {
            self.last_barrier_report.set(TextBarrierReport {
                expected_chars: expected_text.chars().count(),
                ..TextBarrierReport::default()
            });
            select_preceding_text_in(
                expected_text,
                patience.confirm_timeout(),
                earlier,
                &self.last_barrier_report,
                || self.focused_text_field(expected_process_id),
            )
        });
        let mut report = self.last_barrier_report.get();
        report.attempts = attempts;
        self.last_barrier_report.set(report);
        (attempt.status, attempt.cleanup)
    }

    /// Collapses the agent's confirmed selection when the edit does not go ahead after all, and
    /// reads it back. The report keeps the step it described and adds only the cleanup.
    fn collapse_selection_to_end(&self, expected_process_id: u32) -> SelectionCleanup {
        let report = self.last_barrier_report.get();
        let cleanup = match self.focused_text_field(expected_process_id) {
            Ok(field) => collapse_selection(&field),
            Err(_) => SelectionCleanup::Unverified,
        };
        self.last_barrier_report.set(TextBarrierReport {
            cleanup: Some(cleanup),
            ..report
        });
        cleanup
    }

    fn focused_text_field(
        &self,
        expected_process_id: u32,
    ) -> core::result::Result<UiaTextField, TextBarrierStatus> {
        let Some(automation) = &self.automation else {
            self.set_resolver_failure(EditableResolverReason::AutomationUnavailable, 0);
            return Err(TextBarrierStatus::Unavailable);
        };
        unsafe {
            let mut element = automation.GetFocusedElement().map_err(|_| {
                self.set_resolver_failure(EditableResolverReason::FocusUnavailable, 0);
                TextBarrierStatus::Unavailable
            })?;
            let walker = automation.RawViewWalker().map_err(|_| {
                self.set_resolver_failure(EditableResolverReason::WalkerUnavailable, 0);
                TextBarrierStatus::Unavailable
            })?;
            let mut last_process_id = 0;
            for depth in 0..MAX_UIA_EDITABLE_ANCESTOR_DEPTH {
                let process_id = element.CurrentProcessId().map_err(|_| {
                    self.set_resolver_failure(EditableResolverReason::ProcessUnavailable, depth);
                    TextBarrierStatus::Unavailable
                })?;
                if process_id < 0 {
                    self.set_resolver_failure(EditableResolverReason::ProcessUnavailable, depth);
                    return Err(TextBarrierStatus::Unavailable);
                }
                let process_id = process_id as u32;
                last_process_id = process_id;
                if process_id != expected_process_id {
                    self.set_resolver_failure(EditableResolverReason::OwnershipUnproven, depth);
                    self.set_resolver_process(process_id);
                    return Err(TextBarrierStatus::Unavailable);
                }
                match element.CurrentIsPassword() {
                    Ok(value) if value.as_bool() => {
                        self.set_resolver_failure(EditableResolverReason::Password, depth);
                        return Err(TextBarrierStatus::Unavailable);
                    }
                    Ok(_) => {}
                    Err(_) => {
                        self.set_resolver_failure(
                            EditableResolverReason::PasswordPropertyUnavailable,
                            depth,
                        );
                        return Err(TextBarrierStatus::Unavailable);
                    }
                }
                let pattern: Option<IUIAutomationTextPattern> = element
                    .CurrentControlType()
                    .ok()
                    .filter(|control_type| supports_protected_paste(*control_type))
                    .and_then(|_| element.GetCurrentPatternAs(UIA_TextPatternId).ok());
                let exact_process = process_id == expected_process_id;
                match editable_resolver_action(exact_process, pattern.is_some()) {
                    EditableResolverAction::Continue => {}
                    EditableResolverAction::ReturnCurrent => {
                        return pattern
                            .map(|pattern| UiaTextField { pattern })
                            .ok_or(TextBarrierStatus::Unavailable);
                    }
                }
                element = match walker.GetParentElement(&element) {
                    Ok(parent) => parent,
                    Err(_) => {
                        self.set_resolver_failure(EditableResolverReason::ParentUnavailable, depth);
                        self.set_resolver_process(process_id);
                        return Err(TextBarrierStatus::Unavailable);
                    }
                };
            }
            self.set_resolver_failure(
                EditableResolverReason::NoEditableAncestor,
                MAX_UIA_EDITABLE_ANCESTOR_DEPTH,
            );
            self.set_resolver_process(last_process_id);
        }
        Err(TextBarrierStatus::Unavailable)
    }

    fn set_resolver_failure(&self, reason: EditableResolverReason, depth: usize) {
        let mut report = self.last_barrier_report.get();
        report.stage = TextBarrierStage::Resolver;
        report.resolver_reason = reason;
        report.resolver_depth = depth;
        self.last_barrier_report.set(report);
    }

    fn set_resolver_process(&self, process_id: u32) {
        let mut report = self.last_barrier_report.get();
        report.resolver_process_id = process_id;
        self.last_barrier_report.set(report);
    }

    fn last_barrier_report(&self) -> TextBarrierReport {
        self.last_barrier_report.get()
    }

    #[cfg(test)]
    fn is_available(&self) -> bool {
        self.automation.is_some()
    }
}

impl Drop for PrivacyGuard {
    fn drop(&mut self) {
        self.automation.take();
        if self.com_initialized {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn process_image_name(process_id: u32) -> Option<String> {
    unsafe {
        let handle =
            ProcessHandle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?);
        let mut buffer = vec![0u16; 32_768];
        let mut length = u32::try_from(buffer.len()).ok()?;
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
        .ok()?;
        String::from_utf16(&buffer[..usize::try_from(length).ok()?]).ok()
    }
}

const fn text_edit_backend_for_strategy(strategy: BackendStrategy) -> TextEditBackend {
    match strategy {
        BackendStrategy::CapabilityProbe => TextEditBackend::CapabilityProbe,
        BackendStrategy::ProtectedPaste => TextEditBackend::ProtectedPaste,
        BackendStrategy::PhysicalReplay => TextEditBackend::PhysicalReplay,
        BackendStrategy::ObserveOnly => TextEditBackend::ObserveOnly,
    }
}

const fn supports_protected_paste(control_type: UIA_CONTROLTYPE_ID) -> bool {
    control_type.0 == UIA_DocumentControlTypeId.0 || control_type.0 == UIA_EditControlTypeId.0
}

fn text_barrier_matches(actual: &str, expected: &str) -> bool {
    if actual == expected {
        return true;
    }
    expected
        .strip_suffix(' ')
        .zip(actual.strip_suffix('\u{00a0}'))
        .is_some_and(|(expected_word, actual_word)| expected_word == actual_word)
}

/// What one poll of the selection found after the selection was made.
enum SelectionPoll {
    /// Reading the selection failed; try again.
    Unreadable,
    /// The selection does not consist of exactly one range.
    NotOneRange,
    /// The selection is empty.
    Empty,
    /// The text of the selected range.
    Text(String),
}

/// How the selection made by `Select()` turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionConfirmation {
    /// The selection held the expected text.
    Confirmed,
    /// The selection held other text.
    OtherText,
    /// The selection stayed empty or was not one range: the field did not show it in time.
    NotShown,
    /// The selection could not be read at all.
    Unreadable,
}

impl SelectionConfirmation {
    const fn status(self) -> TextBarrierStatus {
        match self {
            Self::Confirmed => TextBarrierStatus::Match,
            Self::OtherText | Self::NotShown => TextBarrierStatus::Mismatch,
            Self::Unreadable => TextBarrierStatus::Unavailable,
        }
    }
}

/// How patiently a protected paste waits for the field to confirm the selection of the word, and
/// for the clipboard. An automatic conversion runs while the correction gate holds the keys typed
/// after it, so it looks once and gives up on a busy clipboard soon. A Pause press is waited for
/// by the user, so it waits longer and selects a second time when the field did not show the
/// first selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PastePatience {
    Automatic,
    Manual,
}

impl PastePatience {
    const fn confirm_timeout(self) -> Duration {
        Duration::from_millis(match self {
            Self::Automatic => UIA_SELECTION_CONFIRM_TIMEOUT_MS,
            Self::Manual => UIA_MANUAL_SELECTION_CONFIRM_TIMEOUT_MS,
        })
    }

    const fn attempts(self) -> u32 {
        match self {
            Self::Automatic => 1,
            Self::Manual => UIA_MANUAL_SELECTION_ATTEMPTS,
        }
    }

    /// How long to wait for the clipboard to take the replacement. The user's clipboard is saved
    /// first, and an application that owns it and does not answer (a hung window holding
    /// delayed-rendered data) can block that for as long as it hangs.
    const fn clipboard_install_timeout(self) -> Duration {
        Duration::from_millis(match self {
            Self::Automatic => CLIPBOARD_AUTOMATIC_INSTALL_TIMEOUT_MS,
            Self::Manual => CLIPBOARD_BROKER_TIMEOUT_MS,
        })
    }
}

/// Held keys are the user's own input for the field they were typed in, so handing them back is
/// refused only on a positive finding: a password field, or an excluded or elevated process. A
/// field that cannot be inspected right now (a hung or busy window) does not keep them back, as
/// keeping them back loses them. The target itself is checked separately and exactly.
const fn recovery_privacy_allows(reason: Option<PrivacyBlockReason>) -> bool {
    !matches!(
        reason,
        Some(
            PrivacyBlockReason::PasswordField
                | PrivacyBlockReason::ExcludedProcess
                | PrivacyBlockReason::ElevatedProcess
        )
    )
}

/// Pause conversions are waited for by the user; automatic ones run inside the gate budget.
const fn paste_patience(forced: bool) -> PastePatience {
    if forced {
        PastePatience::Manual
    } else {
        PastePatience::Automatic
    }
}

/// Runs selection attempts until one is not an unshown selection or `attempts` have run. Only a
/// selection the field did not show, and that was collapsed again, is tried again; every attempt
/// reads and checks the text before the caret again before it selects, and is told whether an
/// earlier attempt ran. Returns the last attempt and how many attempts ran.
fn retry_unshown_selection(
    attempts: u32,
    mut attempt: impl FnMut(bool) -> SelectionAttempt,
) -> (SelectionAttempt, u32) {
    let mut ran = 0;
    loop {
        let result = attempt(ran > 0);
        ran += 1;
        let unshown = result.confirmation == Some(SelectionConfirmation::NotShown)
            && result.cleanup == Some(SelectionCleanup::Collapsed);
        if !unshown || ran >= attempts {
            return (result, ran);
        }
    }
}

/// Confirms that the selection made by `Select()` holds the expected text, polling until
/// `expired` says the time for it is over. Every outcome but `Confirmed` collapses the selection, exactly once: the selection
/// is ours, made a moment ago, and must not be left behind to be typed over. This runs only
/// after our own successful `Select()`, so a selection the user made is never collapsed.
/// Returns the outcome and, when the selection was collapsed, what became of it.
fn confirm_selection(
    mut expired: impl FnMut() -> bool,
    mut poll: impl FnMut() -> SelectionPoll,
    text_matches: impl Fn(&str) -> bool,
    mut sleep: impl FnMut(),
    collapse: impl FnOnce() -> SelectionCleanup,
) -> (SelectionConfirmation, Option<SelectionCleanup>) {
    let mut saw_other_text = false;
    let mut saw_not_shown = false;
    let outcome = loop {
        if expired() {
            break if saw_other_text {
                SelectionConfirmation::OtherText
            } else if saw_not_shown {
                SelectionConfirmation::NotShown
            } else {
                SelectionConfirmation::Unreadable
            };
        }
        match poll() {
            SelectionPoll::Unreadable => {}
            SelectionPoll::NotOneRange | SelectionPoll::Empty => saw_not_shown = true,
            SelectionPoll::Text(actual) => {
                if text_matches(&actual) {
                    break SelectionConfirmation::Confirmed;
                }
                saw_other_text = true;
            }
        }
        sleep();
    };
    let cleanup = (outcome != SelectionConfirmation::Confirmed).then(collapse);
    (outcome, cleanup)
}

/// What became of the agent's own selection when it was collapsed again because the edit did
/// not go ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionCleanup {
    /// Collapsed to its end, and read back empty.
    Collapsed,
    /// Not collapsed, or not read back empty in time: the field may still show the word
    /// selected, and keys typed now would replace it.
    Unverified,
}

/// One selection attempt: its status, how the selection was confirmed when one was made, and
/// what became of the selection when it was collapsed again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionAttempt {
    status: TextBarrierStatus,
    confirmation: Option<SelectionConfirmation>,
    cleanup: Option<SelectionCleanup>,
}

/// The selection of a field as one read found it.
enum SelectionRead<R> {
    /// The selection could not be read.
    Unreadable,
    /// The selection is not exactly one range.
    NotOneRange,
    /// The only range could not be read.
    RangeUnreadable,
    One(R),
}

/// The text pattern calls that the text barrier and the selection make on one field. UI
/// Automation makes them on the focused control (`UiaTextField`); tests script them.
trait TextField {
    type Range;
    fn selection(&self) -> SelectionRead<Self::Range>;
    /// Whether the range is empty; `None` when the comparison fails.
    fn is_empty(&self, range: &Self::Range) -> Option<bool>;
    fn clone_range(&self, range: &Self::Range) -> Option<Self::Range>;
    /// Moves the start of the range by `units` characters and returns how far it moved.
    fn move_start(&self, range: &Self::Range, units: i32) -> Option<i32>;
    fn text(&self, range: &Self::Range) -> Option<String>;
    fn select(&self, range: &Self::Range) -> bool;
    /// Moves the start of the range onto its end.
    fn collapse_to_end(&self, range: &Self::Range) -> bool;
}

/// The focused editable control, through its UI Automation text pattern.
struct UiaTextField {
    pattern: IUIAutomationTextPattern,
}

// The calls go to COM interfaces obtained on this thread; a provider that fails or does not
// answer in time returns an error, which each method turns into its failure value.
impl TextField for UiaTextField {
    type Range = IUIAutomationTextRange;

    fn selection(&self) -> SelectionRead<IUIAutomationTextRange> {
        unsafe {
            let Ok(ranges) = self.pattern.GetSelection() else {
                return SelectionRead::Unreadable;
            };
            if ranges.Length().ok() != Some(1) {
                return SelectionRead::NotOneRange;
            }
            ranges
                .GetElement(0)
                .map_or(SelectionRead::RangeUnreadable, SelectionRead::One)
        }
    }

    fn is_empty(&self, range: &IUIAutomationTextRange) -> Option<bool> {
        unsafe {
            range
                .CompareEndpoints(
                    TextPatternRangeEndpoint_Start,
                    range,
                    TextPatternRangeEndpoint_End,
                )
                .ok()
                .map(|order| order == 0)
        }
    }

    fn clone_range(&self, range: &IUIAutomationTextRange) -> Option<IUIAutomationTextRange> {
        unsafe { range.Clone().ok() }
    }

    fn move_start(&self, range: &IUIAutomationTextRange, units: i32) -> Option<i32> {
        unsafe {
            range
                .MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, units)
                .ok()
        }
    }

    fn text(&self, range: &IUIAutomationTextRange) -> Option<String> {
        unsafe { range.GetText(-1).ok().map(|text| text.to_string()) }
    }

    fn select(&self, range: &IUIAutomationTextRange) -> bool {
        unsafe { range.Select().is_ok() }
    }

    fn collapse_to_end(&self, range: &IUIAutomationTextRange) -> bool {
        unsafe {
            range
                .MoveEndpointByRange(
                    TextPatternRangeEndpoint_Start,
                    range,
                    TextPatternRangeEndpoint_End,
                )
                .is_ok()
        }
    }
}

/// The length in UTF-16 units of a text the barrier checks: none for an empty text or one longer
/// than 128 units.
fn barrier_units(expected_text: &str) -> Option<i32> {
    i32::try_from(expected_text.encode_utf16().count())
        .ok()
        .filter(|units| (1..=128).contains(units))
}

fn set_barrier_stage(report: &Cell<TextBarrierReport>, stage: TextBarrierStage) {
    report.set(TextBarrierReport {
        stage,
        ..report.get()
    });
}

/// Checks that the `units` UTF-16 units before the caret of `field` are `expected_text`. Each
/// early exit records its own stage in `report`; a comparison records what it found. Returns
/// the range over that text when it matches.
fn read_preceding_text<F: TextField>(
    field: &F,
    units: i32,
    expected_text: &str,
    report: &Cell<TextBarrierReport>,
) -> core::result::Result<F::Range, TextBarrierStatus> {
    let stop = |stage, status| {
        set_barrier_stage(report, stage);
        status
    };
    let caret = match field.selection() {
        SelectionRead::Unreadable => {
            return Err(stop(
                TextBarrierStage::Selection,
                TextBarrierStatus::Unavailable,
            ));
        }
        SelectionRead::NotOneRange => {
            return Err(stop(
                TextBarrierStage::SelectionRanges,
                TextBarrierStatus::Mismatch,
            ));
        }
        SelectionRead::RangeUnreadable => {
            return Err(stop(
                TextBarrierStage::Caret,
                TextBarrierStatus::Unavailable,
            ));
        }
        SelectionRead::One(caret) => caret,
    };
    if field.is_empty(&caret) != Some(true) {
        return Err(stop(
            TextBarrierStage::CaretNotEmpty,
            TextBarrierStatus::Mismatch,
        ));
    }
    let Some(range) = field.clone_range(&caret) else {
        return Err(stop(
            TextBarrierStage::RangeClone,
            TextBarrierStatus::Unavailable,
        ));
    };
    let moved = field.move_start(&range, -units);
    if moved != Some(-units) {
        report.set(TextBarrierReport {
            moved_units: moved.unwrap_or(i32::MIN),
            ..report.get()
        });
        return Err(stop(
            TextBarrierStage::RangeMove,
            TextBarrierStatus::Mismatch,
        ));
    }
    let Some(actual) = field.text(&range) else {
        report.set(TextBarrierReport {
            moved_units: -units,
            ..report.get()
        });
        return Err(stop(
            TextBarrierStage::TextRead,
            TextBarrierStatus::Unavailable,
        ));
    };
    report.set(classify_text_barrier(
        &actual,
        expected_text,
        TextBarrierStage::TextCompared,
        -units,
    ));
    if text_barrier_matches(&actual, expected_text) {
        Ok(range)
    } else {
        Err(TextBarrierStatus::Mismatch)
    }
}

/// One attempt to select the text before the caret of the field `resolve` finds: read and check
/// the text, select it, confirm the selection and collapse it again unless it was confirmed.
/// After an earlier attempt, text selected at the caret is that attempt's selection shown late,
/// and is collapsed too; a first attempt leaves a selection it finds alone, as the user made it.
fn select_preceding_text_in<F: TextField>(
    expected_text: &str,
    confirm_timeout: Duration,
    earlier_attempt: bool,
    report: &Cell<TextBarrierReport>,
    resolve: impl FnOnce() -> core::result::Result<F, TextBarrierStatus>,
) -> SelectionAttempt {
    let refused = |status| SelectionAttempt {
        status,
        confirmation: None,
        cleanup: None,
    };
    let Some(units) = barrier_units(expected_text) else {
        set_barrier_stage(report, TextBarrierStage::TextLength);
        return refused(TextBarrierStatus::Unavailable);
    };
    let field = match resolve() {
        Ok(field) => field,
        Err(status) => return refused(status),
    };
    let range = match read_preceding_text(&field, units, expected_text, report) {
        Ok(range) => range,
        Err(status) => {
            let late_selection =
                earlier_attempt && report.get().stage == TextBarrierStage::CaretNotEmpty;
            let cleanup = late_selection.then(|| collapse_selection(&field));
            report.set(TextBarrierReport {
                cleanup,
                ..report.get()
            });
            return SelectionAttempt {
                status,
                confirmation: None,
                cleanup,
            };
        }
    };
    if !field.select(&range) {
        // The call can fail after the field applied it (a provider timeout, say): what is
        // selected is unknown, so it is collapsed like any selection that was not confirmed.
        let cleanup = collapse_selection(&field);
        report.set(TextBarrierReport {
            stage: TextBarrierStage::SelectCall,
            cleanup: Some(cleanup),
            ..report.get()
        });
        return SelectionAttempt {
            status: TextBarrierStatus::Mismatch,
            confirmation: None,
            cleanup: Some(cleanup),
        };
    }
    let confirm_deadline = Instant::now() + confirm_timeout;
    let (confirmation, cleanup) = confirm_selection(
        || Instant::now() >= confirm_deadline,
        || match field.selection() {
            SelectionRead::Unreadable | SelectionRead::RangeUnreadable => SelectionPoll::Unreadable,
            SelectionRead::NotOneRange => SelectionPoll::NotOneRange,
            SelectionRead::One(selected) => {
                if field.is_empty(&selected) == Some(true) {
                    return SelectionPoll::Empty;
                }
                match field.text(&selected) {
                    Some(actual) => {
                        report.set(classify_text_barrier(
                            &actual,
                            expected_text,
                            TextBarrierStage::SelectionConfirm,
                            -units,
                        ));
                        SelectionPoll::Text(actual)
                    }
                    None => SelectionPoll::Unreadable,
                }
            }
        },
        |actual| text_barrier_matches(actual, expected_text),
        || thread::sleep(Duration::from_millis(1)),
        || collapse_selection(&field),
    );
    let mut final_report = if confirmation == SelectionConfirmation::Unreadable {
        TextBarrierReport {
            stage: TextBarrierStage::SelectionConfirm,
            expected_chars: expected_text.chars().count(),
            moved_units: -units,
            ..TextBarrierReport::default()
        }
    } else {
        report.get()
    };
    final_report.confirmation = Some(confirmation);
    final_report.cleanup = cleanup;
    report.set(final_report);
    SelectionAttempt {
        status: confirmation.status(),
        confirmation: Some(confirmation),
        cleanup,
    }
}

/// Collapses the selection of `field` to its end and reads it back. A selection read back empty
/// is `Collapsed`; one that cannot be read, or is still not empty after
/// `SELECTION_COLLAPSE_ATTEMPTS` reads or `SELECTION_COLLAPSE_BUDGET_MS`, is `Unverified`.
fn collapse_selection<F: TextField>(field: &F) -> SelectionCleanup {
    let deadline = Instant::now() + Duration::from_millis(SELECTION_COLLAPSE_BUDGET_MS);
    for attempt in 0..SELECTION_COLLAPSE_ATTEMPTS {
        if attempt > 0 {
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        let SelectionRead::One(range) = field.selection() else {
            continue;
        };
        match field.is_empty(&range) {
            Some(true) => return SelectionCleanup::Collapsed,
            Some(false) if field.collapse_to_end(&range) => {
                let _ = field.select(&range);
            }
            _ => {}
        }
    }
    SelectionCleanup::Unverified
}

/// The result of a protected paste whose text edit and clipboard restoration are known.
const fn protected_paste_outcome(
    target_confirmed: bool,
    clipboard_restored: bool,
) -> TextEditAttempt {
    match (target_confirmed, clipboard_restored) {
        (true, true) => TextEditAttempt::Applied,
        (true, false) => TextEditAttempt::AppliedClipboardNotRestored,
        (false, _) => TextEditAttempt::Failed(ConversionFailureReason::TextCommit),
    }
}

fn barrier_character_class(character: Option<char>) -> BarrierCharacterClass {
    match character {
        None => BarrierCharacterClass::None,
        Some(' ') => BarrierCharacterClass::Space,
        Some('\u{00a0}') => BarrierCharacterClass::NonBreakingSpace,
        Some('\r' | '\n') => BarrierCharacterClass::LineBreak,
        Some(_) => BarrierCharacterClass::Other,
    }
}

fn classify_text_barrier(
    actual: &str,
    expected: &str,
    stage: TextBarrierStage,
    moved_units: i32,
) -> TextBarrierReport {
    let expected_word = expected.strip_suffix(' ').unwrap_or(expected);
    let actual_characters: Vec<char> = actual.chars().collect();
    let expected_word_characters: Vec<char> = expected_word.chars().collect();
    TextBarrierReport {
        stage,
        expected_chars: expected.chars().count(),
        actual_chars: actual_characters.len(),
        moved_units,
        exact: actual == expected,
        trailing_nbsp_equivalent: expected
            .strip_suffix(' ')
            .zip(actual.strip_suffix('\u{00a0}'))
            .is_some_and(|(expected, actual)| expected == actual),
        word_at_start: actual.starts_with(expected_word),
        word_at_end: actual.ends_with(expected_word),
        actual_without_first_is_word: actual_characters
            .get(1..)
            .is_some_and(|actual| actual == expected_word_characters),
        actual_without_last_is_word: actual_characters
            .get(..actual_characters.len().saturating_sub(1))
            .is_some_and(|actual| actual == expected_word_characters),
        leading: barrier_character_class(actual_characters.first().copied()),
        trailing: barrier_character_class(actual_characters.last().copied()),
        resolver_depth: 0,
        resolver_reason: EditableResolverReason::None,
        resolver_process_id: 0,
        confirmation: None,
        attempts: 0,
        cleanup: None,
    }
}

/// The text barrier's question: is the text before the caret of the focused field of
/// `process_id` the word an edit is about to replace?
#[derive(Debug, Clone, PartialEq, Eq)]
struct BarrierRead {
    process_id: u32,
    text: String,
}

/// Text-barrier reads on a provider thread of their own, answered with the barrier's report.
type BarrierProbe = BoundedProbe<BarrierRead, (TextBarrierStatus, TextBarrierReport)>;

/// The answer to `read` from `probe` by `deadline`, or `Unavailable` at the stage `Provider` when
/// the provider is busy with an earlier read, stuck or late. The barrier only reads, so a read
/// left behind at the deadline changes nothing in the field.
fn barrier_answer(
    probe: &mut BarrierProbe,
    read: BarrierRead,
    deadline: Instant,
) -> (TextBarrierStatus, TextBarrierReport) {
    let expected_chars = read.text.chars().count();
    let wait = deadline.saturating_duration_since(Instant::now());
    probe
        .query_result(
            read,
            wait,
            Duration::from_millis(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS),
        )
        .unwrap_or_else(|_| unanswered_barrier(expected_chars))
}

fn unanswered_barrier(expected_chars: usize) -> (TextBarrierStatus, TextBarrierReport) {
    (
        TextBarrierStatus::Unavailable,
        TextBarrierReport {
            stage: TextBarrierStage::Provider,
            expected_chars,
            ..TextBarrierReport::default()
        },
    )
}

fn format_text_barrier_report(report: TextBarrierReport) -> String {
    format!(
        "stage={:?},resolver_reason={:?},resolver_depth={},resolver_pid={},expected_chars={},actual_chars={},moved_units={},exact={},nbsp_equiv={},word_at_start={},word_at_end={},drop_first_matches={},drop_last_matches={},leading={:?},trailing={:?},confirm={:?},attempts={},cleanup={:?}",
        report.stage,
        report.resolver_reason,
        report.resolver_depth,
        report.resolver_process_id,
        report.expected_chars,
        report.actual_chars,
        report.moved_units,
        report.exact,
        report.trailing_nbsp_equivalent,
        report.word_at_start,
        report.word_at_end,
        report.actual_without_first_is_word,
        report.actual_without_last_is_word,
        report.leading,
        report.trailing,
        report.confirmation,
        report.attempts,
        report.cleanup,
    )
}

fn process_integrity_level(process_id: u32) -> Option<u32> {
    unsafe {
        let process =
            ProcessHandle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?);
        let mut token = HANDLE::default();
        OpenProcessToken(process.0, TOKEN_QUERY, &mut token).ok()?;
        let token = ProcessHandle(token);

        let mut required = 0u32;
        let _ = GetTokenInformation(token.0, TokenIntegrityLevel, None, 0, &mut required);
        if required == 0 {
            return None;
        }
        let mut buffer = vec![0u8; usize::try_from(required).ok()?];
        GetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr().cast()),
            required,
            &mut required,
        )
        .ok()?;

        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        if sid.0.is_null() {
            return None;
        }
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(sid, u32::from(count - 1)))
    }
}

// One dedicated loader; package signature checks/FST compilation must never
// block the thread that services WH_KEYBOARD_LL and the tray window messages.
struct RuntimeConfigurationLoader {
    requests: SyncSender<()>,
    replies: Receiver<std::io::Result<RuntimeConfiguration>>,
    in_flight: bool,
    refresh_again: bool,
}
impl RuntimeConfigurationLoader {
    fn spawn() -> std::io::Result<Self> {
        Self::spawn_with(load_runtime_configuration)
    }

    /// `load` runs on the loader thread for each request. A load that panics answers as a failed
    /// read, and the loader goes on serving later requests.
    fn spawn_with(
        load: impl Fn() -> std::io::Result<RuntimeConfiguration> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (requests, receive) = sync_channel(1);
        let (reply, replies) = sync_channel(1);
        thread::Builder::new()
            .name("autokey-package-loader".into())
            .spawn(move || {
                while receive.recv().is_ok() {
                    let result = guarded(&load, || {
                        Err(std::io::Error::other("configuration_loader_panicked"))
                    });
                    if reply.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            replies,
            in_flight: false,
            refresh_again: false,
        })
    }
    fn request(&mut self) -> bool {
        if self.in_flight {
            self.refresh_again = true;
            return true;
        }
        if self.requests.try_send(()).is_err() {
            return false;
        }
        self.in_flight = true;
        true
    }
    fn poll(&mut self) -> Option<std::io::Result<RuntimeConfiguration>> {
        if !self.in_flight {
            return None;
        }
        let result = match self.replies.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Err(std::io::Error::other("configuration_loader_disconnected"))
            }
        };
        self.in_flight = false;
        if std::mem::take(&mut self.refresh_again) {
            // A later explicit save/reload supersedes this result (even errors).
            // Coalesce repeated requests and read the latest files only once more.
            if self.request() {
                return None;
            }
            return Some(Err(std::io::Error::other(
                "configuration_loader_unavailable",
            )));
        }
        Some(result)
    }
}

#[derive(Debug, Clone)]
struct RuntimeConfiguration {
    package_source: PackageSource,
    package_catalogs: Option<Arc<autokeyboardlayot::localization::CatalogRegistry>>,
    input_profiles: autokeyboardlayot::input_profile_selection::InputProfileSelections,
    sources: autokeyboardlayot::configuration::ConfigurationSources,
    settings: Settings,
    ui_language: autokeyboardlayot::localization::UiLanguagePreference,
    user_dictionary: UserLexicon,
    word_exclusions: UserLexicon,
    process_exclusions: ExclusionPolicy,
    backend_rules: BackendRules,
    dictionaries: Arc<autokeyboardlayot::DictionaryRegistry>,
}

impl Default for RuntimeConfiguration {
    fn default() -> Self {
        let settings = Settings::default();
        let dictionaries = dictionary_snapshot(&settings);
        Self {
            package_source: PackageSource::LegacyBootstrap,
            package_catalogs: None,
            input_profiles: Default::default(),
            sources: Default::default(),
            settings,
            ui_language: Default::default(),
            user_dictionary: Default::default(),
            word_exclusions: Default::default(),
            process_exclusions: Default::default(),
            backend_rules: Default::default(),
            dictionaries,
        }
    }
}

// Transitional schema-1 selection bridge. Snapshot construction happens before
// enqueueing; the worker never loads or compiles dictionaries during a reload.
fn dictionary_snapshot(settings: &Settings) -> Arc<autokeyboardlayot::DictionaryRegistry> {
    let mut registry = autokeyboardlayot::DictionaryRegistry::embedded();
    registry
        .set_enabled(settings.enabled_input_packs.iter().cloned())
        .expect("validated selections are bounded");
    Arc::new(registry)
}

fn configuration_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("AutoKeyboardLayot"))
}

fn try_load_configuration_document() -> std::io::Result<ConfigurationDocument> {
    load_configuration_snapshot().map(|loaded| loaded.document)
}

fn load_configuration_snapshot()
-> std::io::Result<autokeyboardlayot::configuration::LoadedConfiguration> {
    let directory = configuration_directory().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
    })?;
    autokeyboardlayot::configuration::load_configuration_snapshot(&directory)
}

fn load_runtime_configuration() -> std::io::Result<RuntimeConfiguration> {
    let loaded = load_configuration_snapshot()?;
    let document = loaded.document;
    let packages = load_installed_packages(&document)?;
    Ok(RuntimeConfiguration {
        package_source: packages.source,
        package_catalogs: packages.catalogs,
        input_profiles: document.input_profiles.clone(),
        dictionaries: packages.dictionaries,
        sources: loaded.sources,
        settings: document.settings.clone(),
        ui_language: document.ui_language.clone(),
        user_dictionary: document.user_lexicon(),
        word_exclusions: document.word_exclusion_lexicon(),
        process_exclusions: document.process_exclusion_policy(),
        backend_rules: document.backend_rules,
    })
}

fn load_installed_packages(document: &ConfigurationDocument) -> std::io::Result<InstalledPackages> {
    let directory = configuration_directory().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
    })?;
    let trust = autokeyboardlayot::language_package::PackageTrust::release()
        .map_err(std::io::Error::other)?;
    InstalledPackages::load(
        &directory.join("packages"),
        document.package_mode,
        &trust,
        &document.settings.enabled_input_packs,
    )
    .map_err(std::io::Error::other)
}

fn write_configuration_file_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    let temporary = autokeyboardlayot::configuration::prepare_configuration_write(path, contents)?;
    let temporary_wide = HSTRING::from(temporary.to_string_lossy().as_ref());
    let path_wide = HSTRING::from(path.to_string_lossy().as_ref());
    let result = unsafe {
        MoveFileExW(
            &temporary_wide,
            &path_wide,
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if let Err(error) = result {
        let _ = fs::remove_file(temporary);
        return Err(std::io::Error::other(error.to_string()));
    }
    Ok(())
}

/// Every writer of the configuration, in any process, holds this lock for its whole
/// read-modify-write.
#[cfg(not(test))]
const CONFIGURATION_WRITE_MUTEX: PCWSTR = w!("Local\\AutoKeyboardLayot.ConfigurationWrite");
/// Tests take a lock of their own: they never wait for an agent running in the same session.
#[cfg(test)]
const CONFIGURATION_WRITE_MUTEX: PCWSTR = w!("Local\\AutoKeyboardLayot.ConfigurationWrite.Tests");

struct ConfigurationWriteGuard(ProcessHandle);

impl ConfigurationWriteGuard {
    fn acquire() -> std::io::Result<Self> {
        let handle = unsafe { CreateMutexW(None, false, CONFIGURATION_WRITE_MUTEX) }
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let handle = ProcessHandle(handle);
        match unsafe { WaitForSingleObject(handle.0, 250) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self(handle)),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Configuration is busy; try applying again",
            )),
        }
    }
}

impl Drop for ConfigurationWriteGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0.0);
        }
    }
}

/// The profile folder, or an error when LOCALAPPDATA is unavailable.
fn configuration_directory_or_error() -> std::io::Result<PathBuf> {
    configuration_directory().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
    })
}

/// Saves `document` into the configuration in `directory`, unless the configuration there is no
/// longer `original`, the one the draft was made from (`AlreadyExists`).
fn save_configuration_if_unchanged(
    directory: &Path,
    document: &ConfigurationDocument,
    original: &ConfigurationDocument,
) -> std::io::Result<()> {
    let _guard = ConfigurationWriteGuard::acquire()?;
    let current = autokeyboardlayot::configuration::load_configuration_directory(directory)?;
    if &current != original {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "Configuration changed outside this window. Reopen settings before saving; your current draft has not been discarded.",
        ));
    }
    write_configuration_document(directory, document)
}

// Caller holds ConfigurationWriteGuard through its entire read/modify/write.
fn write_configuration_document(
    directory: &Path,
    document: &ConfigurationDocument,
) -> std::io::Result<()> {
    let contents = document
        .to_text()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    write_configuration_file_atomically(&directory.join("config.ini"), &contents)
}

fn merge_lexicon_entry(existing: &str, candidate: &VolatileLexiconCandidate) -> Option<String> {
    let lexicon = UserLexicon::from_lines(existing.lines());
    if lexicon.contains(candidate.language, &candidate.word) {
        return Some(existing.to_owned());
    }
    let entry = UserLexicon::format_entry(candidate.language, &candidate.word)?;
    let mut merged = existing.to_owned();
    if !merged.is_empty() && !merged.ends_with('\n') {
        merged.push('\n');
    }
    merged.push_str(&entry);
    Some(merged)
}

fn persist_lexicon_candidate(
    directory: &Path,
    candidate: &VolatileLexiconCandidate,
) -> std::io::Result<()> {
    let _guard = ConfigurationWriteGuard::acquire()?;
    let mut document = autokeyboardlayot::configuration::load_configuration_directory(directory)?;
    let entries = match candidate.target {
        LexiconOfferTarget::UserDictionary => &mut document.user_dictionary,
        LexiconOfferTarget::WordExclusions => &mut document.word_exclusions,
    };
    let existing = entries.join("\n");
    let merged = merge_lexicon_entry(&existing, candidate).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid exclusion word")
    })?;
    *entries = merged.lines().map(str::to_owned).collect();
    write_configuration_document(directory, &document)
}

fn diagnostic_log_path() -> Option<PathBuf> {
    configuration_directory().map(|directory| directory.join("diagnostics.log"))
}

fn diagnostic_worker(receiver: Receiver<String>, path: Option<PathBuf>, metrics: &ObserverMetrics) {
    while let Ok(record) = receiver.recv() {
        let failed = match path.as_deref() {
            Some(path) => !matches!(
                append_diagnostic_record(path, &record, DIAGNOSTIC_LOG_MAX_BYTES),
                Ok(DiagnosticAppend::Appended)
            ),
            None => true,
        };
        if failed {
            metrics
                .diagnostic_log_errors
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How one record reached the diagnostics log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticAppend {
    Appended,
    /// The full log could not be moved aside (a viewer may hold it open without sharing delete
    /// access), so the record went past the limit.
    AppendedPastLimit,
    /// The full log could not be moved aside and had reached twice the limit: it was started
    /// again with this record.
    Restarted,
}

/// Appends one record to the log at `path`. A log that would pass `max_bytes` is first moved
/// aside to `<name>.1.log`, which replaces the previous one only when the move succeeds. When it
/// fails, records still land: past the limit up to twice it, then in a restarted log. So the log
/// neither stops nor grows without bound while something holds it.
fn append_diagnostic_record(
    path: &Path,
    record: &str,
    max_bytes: u64,
) -> std::io::Result<DiagnosticAppend> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let record = record.replace(['\r', '\n'], " ");
    let line = format!("{timestamp} {record}\r\n");
    let current_size = fs::metadata(path).map_or(0, |metadata| metadata.len());
    let size_after = current_size.saturating_add(line.len() as u64);
    let mut outcome = DiagnosticAppend::Appended;
    if current_size > 0
        && size_after > max_bytes
        && fs::rename(path, path.with_extension("1.log")).is_err()
    {
        outcome = if size_after > max_bytes.saturating_mul(2) {
            DiagnosticAppend::Restarted
        } else {
            DiagnosticAppend::AppendedPastLimit
        };
    }
    let mut options = OpenOptions::new();
    options.create(true);
    if outcome == DiagnosticAppend::Restarted {
        options.write(true).truncate(true);
    } else {
        options.append(true);
    }
    let mut file = options.open(path)?;
    file.write_all(line.as_bytes())?;
    file.flush()?;
    Ok(outcome)
}

fn input_worker(
    receiver: Receiver<QueuedInputEvent>,
    metrics: Arc<ObserverMetrics>,
    shutdown: Arc<AtomicBool>,
    gate_window: usize,
    lexicon_candidate: Arc<Mutex<Option<VolatileLexiconCandidate>>>,
    diagnostic_sender: SyncSender<String>,
    configuration: RuntimeConfiguration,
) {
    let flags = Arc::clone(&metrics);
    let mut processor = InputProcessor::new_with_lexicon_candidate(
        metrics,
        gate_window,
        lexicon_candidate,
        Some(diagnostic_sender),
        configuration,
    );
    // A panic while handling an event is contained: the busy flag is cleared, conversion state is
    // dropped and automatic conversion pauses. A run of panics stops the worker instead of
    // spinning on a bug; the UI then reports it and Exit no longer waits for it.
    let mut faults = FaultWindow::new(3, Duration::from_secs(60));
    let exit = run_contained(
        &mut processor,
        &receiver,
        &flags.worker_busy,
        &shutdown,
        &mut faults,
        |processor, event| processor.process(event),
        |processor| {
            ENGINE_FAULT.store(true, Ordering::Release);
            processor.reject_faulted_configuration();
            processor.record_conversion_failure(ConversionFailureReason::InternalFault);
        },
    );
    if exit == WorkerExit::TooManyFaults {
        flags.worker_dead.store(true, Ordering::Release);
    }
}

struct InputProcessor {
    detector: Detector,
    input_profiles: KeyboardProfileCache,
    #[cfg(test)]
    profile_override: Option<ResolvedKeyboardProfiles>,
    // Tests pin the input target that has the focus, instead of the real foreground window.
    #[cfg(test)]
    foreground_override: Option<ForegroundContext>,
    // Tests describe the process behind the input target: its image path and integrity level.
    #[cfg(test)]
    process_override: Option<(Option<String>, Option<u32>)>,
    // Tests script the steps of a protected paste that reach UI Automation, the clipboard,
    // SendInput and the keyboard layout.
    #[cfg(test)]
    paste_seam: Option<RefCell<Box<dyn PasteSeam + Send>>>,
    // Tests give what each key prints in each layout (by handle), instead of the layouts Windows
    // has loaded.
    #[cfg(test)]
    test_keyboard: Option<fn(ReplayKey, usize) -> Option<char>>,
    session: InputSession,
    modifiers: Modifiers,
    // After a focus change, a new input epoch or the start the tracked modifiers are unknown: the
    // next key event's held modifiers replace them.
    resync_modifiers: bool,
    privacy_guard: PrivacyGuard,
    // Keyed by the input target, the input epoch and the privacy generation.
    privacy_probe: Option<BoundedProbe<(ForegroundContext, u64, u64), Option<PrivacyBlockReason>>>,
    // The text barrier's reads, bounded by the barrier's own deadline. Selecting and collapsing
    // change the field and stay on the worker: abandoned at a deadline they could still take
    // effect later, under keys the user types meanwhile.
    barrier_probe: RefCell<Option<BarrierProbe>>,
    exclusion_policy: ExclusionPolicy,
    backend_rules: BackendRules,
    settings: Settings,
    lexicon_candidate: Arc<Mutex<Option<VolatileLexiconCandidate>>>,
    diagnostic_sender: Option<SyncSender<String>>,
    privacy_needs_check: bool,
    // Moves on with every input that may move the focus (mark_privacy_dirty). It is part of the
    // probe's key, so an answer requested before such input never decides the field after it.
    privacy_generation: u64,
    privacy_reason: Option<PrivacyBlockReason>,
    recent_privacy_ok: Option<RecentPrivacyOk>,
    // The input target of the last periodic field check and how long it took.
    last_refresh_check: Option<(ForegroundContext, Duration)>,
    process_reason: Option<PrivacyBlockReason>,
    last_process_id: Option<u32>,
    current_integrity_level: Option<u32>,
    replay_keys: Vec<ReplayKey>,
    // Field-inspection fallback authorizes manual input only. Recovery of UIA
    // must not promote a word captured under that exception to automatic input.
    // A word resumed by erasing its delimiter is also manual-only, so erasing
    // and retyping a space never re-triggers automatic conversion.
    manual_only_word: bool,
    // Whether the active manual token began at a known word start. A token
    // typed after a shortcut has an unknown left context; erasing it must not
    // re-enable automatic conversion for the following text.
    manual_token_start_known: bool,
    // Coarse category of the last event that discarded or blocked the word
    // buffer. It never contains typed text and only explains ignored hotkeys.
    last_word_reset: &'static str,
    // Languages (never text) of the most recent completed words, for the
    // optional layout model. Cleared when the input target changes.
    recent_languages: Vec<Language>,
    last_boundary: Option<LastBoundary>,
    transpose_cycle: Option<TransposeCycle>,
    // The last word of up to REMEMBERED_WORD_WINDOWS input targets the agent stopped tracking,
    // oldest first; see RememberedWord.
    remembered_words: Vec<RememberedWord>,
    // The input target of the last key event, to remember its word when tracking moves on.
    last_key_foreground: Option<ForegroundContext>,
    text_edit_backend: TextEditBackend,
    pending_conversion: Option<PendingConversion>,
    deferred_drained_conversion: Option<DeferredDrainedConversion>,
    undo_record: Option<UndoRecord>,
    layout_switch_in_flight: Option<(usize, Instant)>,
    last_foreground: Option<(usize, usize, u32)>,
    last_layout: Option<usize>,
    last_input_epoch: u64,
    configuration_revision: u64,
    last_dropped_events: u64,
    metrics: Arc<ObserverMetrics>,
    gate_window: usize,
    // Token of the most recent gate release request (0 when none yet).
    last_gate_release_token: u64,
    // Token of the most recent gate ended with its held keys kept for recovery (0 when none).
    last_gate_retention_token: u64,
    // The gate whose held keys are kept for recovery when it ends, instead of replayed.
    held_keys_kept_for: Option<u64>,
    // When the gate holding the keys typed after the word being converted lets them go.
    edit_deadline: Cell<Option<Instant>>,
    // Clipboard brokers the worker stopped waiting for.
    clipboard_brokers: ClipboardBrokers,
    // Policy D1: how often automatic conversion may resume after a recovered gate abort.
    auto_resumes: RateLimit,
    // Replacements of the privacy provider that were already logged.
    reported_probe_respawns: u32,
    // Plays the conversion sounds; started on the first cue while they are enabled.
    sound_player: Option<SyncSender<SoundCue>>,
    // Held keys were replayed without the hooks seeing them, so the word at the caret is unknown:
    // until that word ends (a boundary, a click, a focus change), an epoch reset keeps the
    // session suppressed instead of clearing it.
    recovered_mid_word: bool,
    // The cancelled gate token at the time an automatic edit last failed (0 when none). An edit
    // that failed because of a gate abort forbids resuming after that abort (policy D1).
    failed_edit_gate_token: u64,
    // The configuration revision being applied; a contained panic rejects exactly this one.
    applying_configuration: Option<u64>,
}

impl InputProcessor {
    #[cfg(test)]
    fn new(metrics: Arc<ObserverMetrics>, gate_window: usize) -> Self {
        let mut processor = Self::new_with_lexicon_candidate(
            metrics,
            gate_window,
            Arc::new(Mutex::new(None)),
            None,
            RuntimeConfiguration::default(),
        );
        processor.profile_override = Some(tests::test_profiles());
        processor
            .detector
            .set_resolved_profiles(processor.profile_override.as_ref());
        processor
    }

    fn new_with_lexicon_candidate(
        metrics: Arc<ObserverMetrics>,
        gate_window: usize,
        lexicon_candidate: Arc<Mutex<Option<VolatileLexiconCandidate>>>,
        diagnostic_sender: Option<SyncSender<String>>,
        configuration: RuntimeConfiguration,
    ) -> Self {
        let mut detector = Detector::with_profile_selections(
            DetectorConfig {
                single_letter_words: configuration.settings.single_letter_words,
                ..Default::default()
            },
            configuration.dictionaries,
            &configuration.input_profiles,
        );
        detector.set_resolved_profiles(None);
        detector
            .replace_user_lexicons(configuration.user_dictionary, configuration.word_exclusions);
        detector.set_layout_model(
            configuration
                .settings
                .layout_model
                .then(autokeyboardlayot::layout_model::embedded)
                .flatten(),
        );
        metrics
            .pause_break_undo
            .store(configuration.settings.pause_break_undo, Ordering::Release);
        store_hotkey_configuration(&metrics, &configuration.settings);
        metrics.diagnostics_enabled.store(
            configuration.settings.diagnostics_enabled,
            Ordering::Release,
        );
        let mut session = InputSession::default();
        session.set_recheck_first_word_after_erasing(
            configuration.settings.recheck_first_word_after_erasing,
        );
        Self {
            detector,
            input_profiles: KeyboardProfileCache::default(),
            #[cfg(test)]
            profile_override: None,
            #[cfg(test)]
            foreground_override: None,
            #[cfg(test)]
            process_override: None,
            #[cfg(test)]
            paste_seam: None,
            #[cfg(test)]
            test_keyboard: None,
            session,
            modifiers: Modifiers::default(),
            resync_modifiers: true,
            privacy_guard: PrivacyGuard::new(),
            // A UIA call that never returns would leave every later check "queue busy" and
            // privacy unavailable for good. A provider stuck for 5 s is replaced by a fresh one,
            // built on a new thread (its COM objects are created there), at most three times.
            privacy_probe: BoundedProbe::spawn_resilient(
                "autokey-privacy",
                || {
                    let guard = PrivacyGuard::new();
                    move |(foreground, _epoch, _generation): (ForegroundContext, u64, u64)| {
                        if !foreground_identity_matches(foreground) {
                            return Some(PrivacyBlockReason::InspectionUnavailable);
                        }
                        let reason = guard.inspect(foreground.process_id);
                        if !foreground_identity_matches(foreground) {
                            return Some(PrivacyBlockReason::InspectionUnavailable);
                        }
                        reason
                    }
                },
                Duration::from_secs(PRIVACY_PROBE_STUCK_AFTER_SECS),
                PRIVACY_PROBE_MAX_RESPAWNS,
            )
            .ok(),
            // A barrier read that never returns would hold the worker, which waits for it, for
            // good. On a thread of its own it holds no longer than the barrier's deadline.
            barrier_probe: RefCell::new(
                BoundedProbe::spawn_resilient(
                    "autokey-barrier",
                    || {
                        let guard = PrivacyGuard::new();
                        move |read: BarrierRead| {
                            let status = guard.preceding_text_status(read.process_id, &read.text);
                            (status, guard.last_barrier_report.get())
                        }
                    },
                    Duration::from_millis(BARRIER_PROBE_STUCK_AFTER_MS),
                    BARRIER_PROBE_MAX_RESPAWNS,
                )
                .ok(),
            ),
            exclusion_policy: configuration.process_exclusions,
            backend_rules: configuration.backend_rules,
            settings: configuration.settings,
            lexicon_candidate,
            diagnostic_sender,
            privacy_needs_check: true,
            privacy_generation: 0,
            privacy_reason: Some(PrivacyBlockReason::InspectionUnavailable),
            recent_privacy_ok: None,
            last_refresh_check: None,
            process_reason: None,
            last_process_id: None,
            current_integrity_level: process_integrity_level(unsafe { GetCurrentProcessId() }),
            replay_keys: Vec::new(),
            manual_only_word: false,
            manual_token_start_known: false,
            last_word_reset: "none",
            recent_languages: Vec::new(),
            last_boundary: None,
            transpose_cycle: None,
            remembered_words: Vec::new(),
            last_key_foreground: None,
            text_edit_backend: TextEditBackend::ObserveOnly,
            pending_conversion: None,
            deferred_drained_conversion: None,
            undo_record: None,
            layout_switch_in_flight: None,
            last_foreground: None,
            last_layout: None,
            last_input_epoch: 0,
            configuration_revision: 0,
            last_dropped_events: 0,
            metrics,
            gate_window,
            last_gate_release_token: 0,
            last_gate_retention_token: 0,
            held_keys_kept_for: None,
            edit_deadline: Cell::new(None),
            clipboard_brokers: ClipboardBrokers::default(),
            auto_resumes: RateLimit::new(3, Duration::from_secs(60 * 60)),
            reported_probe_respawns: 0,
            sound_player: None,
            recovered_mid_word: false,
            failed_edit_gate_token: 0,
            applying_configuration: None,
        }
    }

    /// Whether the input target `expected` still has the focus. Tests pin the focused target.
    fn foreground_matches(&self, expected: ForegroundContext) -> bool {
        #[cfg(test)]
        if let Some(current) = self.foreground_override {
            return same_input_target(current, expected);
        }
        foreground_identity_matches(expected)
    }

    /// The input target that has the focus now, with its layout. Tests pin it.
    fn current_foreground(&self) -> Option<ForegroundContext> {
        #[cfg(test)]
        if self.foreground_override.is_some() {
            return self.foreground_override;
        }
        current_foreground_context()
    }

    /// Whether the text before the caret is the word an edit is about to replace, as its settle
    /// step reads it. Tests script it.
    fn settle_text_status(
        &self,
        process_id: u32,
        expected_text: &str,
        deadline: Instant,
    ) -> TextBarrierStatus {
        #[cfg(test)]
        if let Some(seam) = &self.paste_seam {
            return seam.borrow_mut().settle();
        }
        self.barrier_text_status(process_id, expected_text, deadline)
    }

    /// Whether the text before the caret is `expected_text`, as the barrier's provider thread
    /// reads it, waited for until `deadline` at most. Without a provider the barrier is
    /// unavailable, and no edit that needs it goes ahead.
    fn barrier_text_status(
        &self,
        process_id: u32,
        expected_text: &str,
        deadline: Instant,
    ) -> TextBarrierStatus {
        let read = BarrierRead {
            process_id,
            text: expected_text.to_owned(),
        };
        let (status, report) = match self.barrier_probe.borrow_mut().as_mut() {
            Some(probe) => barrier_answer(probe, read, deadline),
            None => unanswered_barrier(expected_text.chars().count()),
        };
        self.privacy_guard.last_barrier_report.set(report);
        status
    }

    /// The image path and the integrity level of `process_id`. Tests describe the process.
    fn process_facts(&self, process_id: u32) -> (Option<String>, Option<u32>) {
        #[cfg(test)]
        if let Some((image, integrity)) = &self.process_override {
            return (image.clone(), *integrity);
        }
        (
            process_image_name(process_id),
            process_integrity_level(process_id),
        )
    }

    /// Selects the text before the caret for a paste. Tests script it.
    fn paste_select(
        &self,
        process_id: u32,
        expected_text: &str,
        patience: PastePatience,
    ) -> (TextBarrierStatus, Option<SelectionCleanup>) {
        #[cfg(test)]
        if let Some(seam) = &self.paste_seam {
            return seam.borrow_mut().select();
        }
        self.privacy_guard
            .select_preceding_text(process_id, expected_text, patience)
    }

    /// Puts a paste's replacement on the clipboard. Tests give it a clipboard of their own.
    fn paste_clipboard(
        &self,
        text: &str,
        install_timeout: Duration,
        target_process: u32,
    ) -> core::result::Result<ProtectedClipboard<'_>, ClipboardBeginFailure> {
        #[cfg(test)]
        if let Some(seam) = &self.paste_seam {
            return seam.borrow_mut().begin(
                text,
                install_timeout,
                &self.clipboard_brokers,
                target_process,
            );
        }
        ProtectedClipboard::begin(
            text,
            install_timeout,
            &self.clipboard_brokers,
            target_process,
        )
    }

    /// Sends a paste's keys and says how many SendInput took. Tests script it.
    fn paste_submit(&self, inputs: &[INPUT]) -> usize {
        #[cfg(test)]
        if let Some(seam) = &self.paste_seam {
            return seam.borrow_mut().submit(inputs.len());
        }
        send_input_count(inputs)
    }

    /// Waits for the field to show a paste's text. Tests script it.
    fn paste_commit(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
        expected_layout: usize,
        expected_text: &str,
    ) -> ReplayAttempt {
        #[cfg(test)]
        if let Some(seam) = &self.paste_seam {
            return seam.borrow_mut().commit();
        }
        self.wait_for_text_commit(foreground, input_sequence, expected_layout, expected_text)
    }

    /// Switches the layout after a paste. Tests script it.
    fn paste_switch_layout(
        &self,
        foreground: ForegroundContext,
        target_layout: HKL,
        input_sequence: u64,
    ) -> ReplayAttempt {
        #[cfg(test)]
        if let Some(seam) = &self.paste_seam {
            return seam.borrow_mut().switch_layout();
        }
        self.switch_layout_and_wait(foreground, target_layout, input_sequence)
    }

    /// Collapses a paste's selection. Tests script it.
    fn paste_collapse(&self, process_id: u32) -> SelectionCleanup {
        #[cfg(test)]
        if let Some(seam) = &self.paste_seam {
            return seam.borrow_mut().collapse();
        }
        self.privacy_guard.collapse_selection_to_end(process_id)
    }

    fn resolved_profiles(&self) -> Option<&ResolvedKeyboardProfiles> {
        #[cfg(test)]
        if self.profile_override.is_some() {
            return self.profile_override.as_ref();
        }
        self.input_profiles.snapshot()
    }

    fn refresh_input_profiles(&mut self) {
        #[cfg(test)]
        if self.profile_override.is_some() {
            return;
        }
        let changed = self.input_profiles.poll();
        self.apply_profile_refresh(changed);
    }

    fn apply_profile_refresh(&mut self, changed: bool) {
        if changed {
            let snapshot = self.resolved_profiles().cloned();
            self.detector.set_resolved_profiles(snapshot.as_ref());
            self.invalidate_conversion_state();
            if self.session.buffered_character_count() != 0 {
                self.suppress_session();
            }
            self.replay_keys.clear();
            self.layout_switch_in_flight = None;
            self.diagnostic(
                "input_profiles",
                format!(
                    "generation={} available={}",
                    self.input_profiles.generation(),
                    snapshot.is_some()
                ),
            );
        }
    }

    fn language_for_layout(&self, layout: usize) -> Option<Language> {
        self.detector
            .language_for_profile(self.resolved_profiles()?.profile(layout)?)
            .filter(|&language| self.settings.language_enabled(language))
    }

    fn profile_inventory_is_current(&self) -> bool {
        #[cfg(test)]
        if self.profile_override.is_some() {
            return true;
        }
        self.input_profiles.inventory_is_current()
    }

    fn find_layout(&self, language: Language) -> Option<HKL> {
        let profile = self.detector.input_profile(language)?;
        let handle = self.resolved_profiles()?.unique_layout(profile)?;
        Some(HKL(handle as *mut c_void))
    }

    /// The source layout is the field's layout when the edit starts: after a manual switch Pause
    /// replaces a word typed in another layout, so it need not be the source language's own. Any
    /// change of the profiles since the edit was planned changes the generation.
    fn pending_profiles_match(&self, pending: &PendingConversion) -> bool {
        pending.profile_generation == self.input_profiles.generation()
            && self.language_for_layout(pending.source_layout).is_some()
            && self
                .find_layout(pending.transaction.target_language)
                .is_some()
    }

    fn diagnostic(&self, event: &'static str, details: String) {
        if !self.metrics.diagnostics_enabled.load(Ordering::Acquire) {
            return;
        }
        // The owner's decision: while privacy blocks the input target nothing about the typing
        // there is written. Even a counter or a timestamp per key tells a secret's length and
        // rhythm.
        if self.privacy_reason.is_some() && describes_typing(event) {
            return;
        }
        let Some(sender) = &self.diagnostic_sender else {
            return;
        };
        let record = format!(
            "event={event} gate={} epoch={} observed={} captured_ms={} {details}",
            self.metrics.active_gate_token.load(Ordering::Acquire),
            self.metrics.input_epoch.load(Ordering::Acquire),
            self.metrics.observed_input_sequence.load(Ordering::Acquire),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        );
        if sender.try_send(record).is_err() {
            self.metrics
                .dropped_diagnostics
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Handles one queued event. When the event is the Space key-down that armed a correction
    /// gate, the gate is released afterwards on every path: the normal paths do it themselves, but
    /// early returns (stale epoch, pending configuration, automatic mode off, a pending
    /// conversion, a shortcut modifier) used to leave the held keys waiting for the deadline and
    /// aborting the gate. Releasing is always safe: it only replays held keys unchanged.
    fn process(&mut self, queued: QueuedInputEvent) {
        self.log_finished_clipboard_brokers();
        let armed = self.armed_gate_token(&queued);
        let reason = if armed == 0 {
            ""
        } else {
            self.gate_bypass_reason(&queued)
        };
        // The gate armed with this Space lets the held keys go this long after it was armed.
        self.edit_deadline.set(
            (armed != 0)
                .then(|| queued.captured_at + Duration::from_millis(CORRECTION_GATE_MAX_HOLD_MS)),
        );
        self.process_event(queued);
        if armed != 0 && self.last_gate_release_token != armed {
            self.diagnostic(
                "gate",
                format!("phase=release_forced token={armed} reason={reason}"),
            );
            self.request_gate_release(armed);
        }
    }

    /// Logs how each clipboard broker the worker stopped waiting for ended, once it is over.
    fn log_finished_clipboard_brokers(&self) {
        for end in self.clipboard_brokers.reap() {
            self.diagnostic("paste", format!("phase=broker_end result={end:?}"));
        }
    }

    /// The gate token when this event is the Space key-down that armed the decision gate, else 0.
    fn armed_gate_token(&self, queued: &QueuedInputEvent) -> u64 {
        let RawInputEvent::Key(key) = &queued.event else {
            return 0;
        };
        let is_space_down = matches!(key.message, WM_KEYDOWN | WM_SYSKEYDOWN)
            && key.virtual_key == u32::from(VK_SPACE.0);
        if is_space_down
            && key.drain_token == 0
            && key.sequence != 0
            && self.metrics.active_gate_token.load(Ordering::Acquire) == key.sequence
        {
            key.sequence
        } else {
            0
        }
    }

    /// Which condition would bypass the normal release, for the diagnostics of a forced one.
    fn gate_bypass_reason(&self, queued: &QueuedInputEvent) -> &'static str {
        if queued.epoch != self.metrics.input_epoch.load(Ordering::Acquire) {
            "stale-epoch"
        } else if self.metrics.configuration_pending.load(Ordering::Acquire) {
            "configuration-pending"
        } else if !self.metrics.auto_enabled.load(Ordering::Acquire) {
            "automatic-off"
        } else if self.pending_conversion.is_some() {
            "pending-conversion"
        } else if self.modifiers.has_shortcut_modifier() {
            "shortcut-modifier"
        } else {
            "unhandled"
        }
    }

    fn process_event(&mut self, queued: QueuedInputEvent) {
        let queue_wait_ms = queued.captured_at.elapsed().as_millis();
        if queue_wait_ms >= u128::from(SLOW_INPUT_DIAGNOSTIC_MS)
            || matches!(queued.event, RawInputEvent::GateDrainReady { .. })
        {
            let kind = match queued.event {
                RawInputEvent::Key(_) => "key",
                RawInputEvent::GateDrainReady { .. } => "gate_ack",
                RawInputEvent::PrivacyRefresh(_) => "privacy",
                RawInputEvent::ProfileRefresh => "profiles",
                RawInputEvent::RecoverInput { .. } => "recovery",
                RawInputEvent::RetainedInputExpired => "recovery_expired",
                RawInputEvent::RetainedInputHandedBack { .. } => "recovery_handed_back",
                RawInputEvent::Mouse => "mouse",
                RawInputEvent::ExternalInjection => "external",
                RawInputEvent::ReloadConfiguration => "configuration",
            };
            self.diagnostic(
                "input_queue",
                format!(
                    "kind={kind} wait_ms={queue_wait_ms} queued_epoch={}",
                    queued.epoch
                ),
            );
        }
        match queued.event {
            RawInputEvent::PrivacyRefresh(_) => self
                .metrics
                .privacy_refresh_queued
                .store(false, Ordering::Release),
            RawInputEvent::ProfileRefresh => self
                .metrics
                .profile_refresh_queued
                .store(false, Ordering::Release),
            _ => {}
        }
        if matches!(queued.event, RawInputEvent::ReloadConfiguration) {
            // Configuration revisions are independent of input overflow epochs.
            if let Some(reload) = queued.configuration
                && reload.revision > self.configuration_revision
            {
                // Source monotonicity was checked against active AND pending
                // state before enqueue. This private FIFO carries only accepted
                // snapshots; adding another rejection path here would require
                // a negative-ack protocol.
                self.applying_configuration = Some(reload.revision);
                self.apply_configuration_reload(Ok(reload.snapshot));
                self.applying_configuration = None;
                self.configuration_revision = reload.revision;
                self.metrics
                    .configuration_ack
                    .store(reload.revision, Ordering::Release);
            }
            return;
        }
        if matches!(queued.event, RawInputEvent::RetainedInputExpired) {
            // Not input: a newer input epoch must not drop the decision to resume.
            self.resume_after_recovered_abort(Instant::now());
            return;
        }
        if let RawInputEvent::RetainedInputHandedBack {
            token,
            submitted,
            requested,
        } = queued.event
        {
            // Not input either: the keys were typed whatever epoch the queue is in now.
            self.finish_recovery(token, false, submitted, requested);
            return;
        }
        let current_epoch = self.metrics.input_epoch.load(Ordering::Acquire);
        if let RawInputEvent::RecoverInput { sequence, explicit } = queued.event {
            // Held keys are no stale input: a published configuration or a hook reinstall since
            // the request, or a pending reload, must not leave them waiting until they expire.
            if self.last_input_epoch != current_epoch {
                self.reset_for_epoch(current_epoch);
            }
            self.recover_retained_input(sequence, explicit);
            return;
        }
        if queued.epoch != current_epoch {
            if self.last_input_epoch != current_epoch {
                self.reset_for_epoch(current_epoch);
            }
            if let RawInputEvent::GateDrainReady {
                token,
                drained_count,
                replay_succeeded,
            } = queued.event
            {
                self.process_gate_drain_ready(token, drained_count, replay_succeeded);
            }
            return;
        }
        if queued.epoch != self.last_input_epoch {
            self.reset_for_epoch(queued.epoch);
        }
        if let RawInputEvent::GateDrainReady {
            token,
            drained_count,
            replay_succeeded,
        } = queued.event
        {
            self.process_gate_drain_ready(token, drained_count, replay_succeeded);
            return;
        }

        if self.metrics.configuration_pending.load(Ordering::Acquire) {
            self.log_word_discard("configuration", WordDiscardDetail::None);
            self.invalidate_conversion_state();
            self.session.clear();
            self.replay_keys.clear();
            return;
        }

        match queued.event {
            RawInputEvent::Mouse => {
                if let Some(target) = self.last_key_foreground {
                    self.remember_word(target, self.last_layout, Instant::now());
                }
                self.log_word_discard("mouse", WordDiscardDetail::None);
                self.last_word_reset = "mouse";
                self.recovered_mid_word = false;
                self.invalidate_conversion_state();
                self.replay_keys.clear();
                self.last_boundary = None;
                self.transpose_cycle = None;
                self.session.handle(InputEvent::Mouse, None, &self.detector);
                self.mark_privacy_dirty(false);
            }
            RawInputEvent::ExternalInjection => {
                self.last_word_reset = "external-input";
                self.invalidate_conversion_state();
                self.suppress_session();
                self.mark_privacy_dirty(true);
            }
            RawInputEvent::Key(event) => self.process_key(event),
            RawInputEvent::ProfileRefresh => {
                // Like the privacy refresh, never in the way of a fence acknowledgement.
                if self.metrics.active_gate_token.load(Ordering::Acquire) == 0 {
                    self.refresh_input_profiles();
                }
            }
            RawInputEvent::PrivacyRefresh(foreground) => {
                // Periodic refresh is not a gate-control dependency. The next
                // timer/word retries; it must not delay a fence acknowledgement.
                if self.metrics.active_gate_token.load(Ordering::Acquire) != 0 {
                    return;
                }
                self.refresh_input_profiles();
                if !self.foreground_matches(foreground) {
                    self.mark_privacy_dirty(false);
                    return;
                }
                self.observe_full_screen(foreground_is_full_screen());
                if full_screen_check_is_redundant(
                    self.metrics.foreground_full_screen.load(Ordering::Acquire),
                    self.last_refresh_check,
                    foreground,
                ) {
                    return;
                }
                let started = Instant::now();
                self.evaluate_privacy(foreground, ProbeTrigger::Refresh);
                self.last_refresh_check = Some((foreground, started.elapsed()));
            }
            RawInputEvent::ReloadConfiguration => unreachable!(),
            RawInputEvent::RecoverInput { .. } => unreachable!(),
            RawInputEvent::RetainedInputExpired => unreachable!(),
            RawInputEvent::RetainedInputHandedBack { .. } => unreachable!(),
            RawInputEvent::GateDrainReady { .. } => unreachable!(),
        }
    }

    /// After a contained panic: a reload that was being applied is rejected, so the UI stops
    /// waiting for it and keeps the published settings that match the policy still in force.
    fn reject_faulted_configuration(&mut self) {
        let Some(revision) = self.applying_configuration.take() else {
            return;
        };
        self.configuration_revision = revision;
        self.metrics
            .configuration_rejected
            .store(revision, Ordering::Release);
        self.diagnostic(
            "configuration",
            format!("result=rejected revision={revision} reason=internal-fault"),
        );
    }

    fn apply_configuration_reload(&mut self, result: std::io::Result<RuntimeConfiguration>) {
        self.invalidate_conversion_state();
        self.session.clear();
        self.replay_keys.clear();
        let configuration = match result {
            Ok(configuration) => configuration,
            Err(_) => {
                // Retain the last validated policy, including every exclusion.
                // Discard in-flight input without logging user configuration.
                self.diagnostic(
                    "configuration",
                    "result=read_failed retained=last_known_good".to_owned(),
                );
                return;
            }
        };
        // Everything that can fail is built before anything is replaced: a contained panic here
        // leaves the previous policy, including every exclusion, in force.
        let mut detector = Detector::with_profile_selections(
            DetectorConfig {
                single_letter_words: configuration.settings.single_letter_words,
                ..Default::default()
            },
            configuration.dictionaries,
            &configuration.input_profiles,
        );
        let profiles = self.resolved_profiles().cloned();
        detector.set_resolved_profiles(profiles.as_ref());
        detector
            .replace_user_lexicons(configuration.user_dictionary, configuration.word_exclusions);
        detector.set_layout_model(
            configuration
                .settings
                .layout_model
                .then(autokeyboardlayot::layout_model::embedded)
                .flatten(),
        );
        self.detector = detector;
        self.recent_languages.clear();
        self.exclusion_policy = configuration.process_exclusions;
        self.backend_rules = configuration.backend_rules;
        self.settings = configuration.settings;
        self.session
            .set_recheck_first_word_after_erasing(self.settings.recheck_first_word_after_erasing);
        // Hook-side settings are published by the main thread only after this
        // snapshot is acknowledged; an older reload must not overwrite them.
        if !self.settings.offer_word_exclusion_after_undo
            && !self.settings.offer_dictionary_after_forced_conversion
            && let Ok(mut candidate) = self.lexicon_candidate.lock()
        {
            candidate.take();
        }
        self.last_process_id = None;
        self.process_reason = None;
        self.mark_privacy_dirty(false);
        self.diagnostic(
            "configuration",
            format!(
                "result=reloaded hotkey_enabled={} shortcut={}",
                self.settings.pause_break_undo,
                self.settings.force_hotkey().display_name()
            ),
        );
    }

    fn process_gate_drain_ready(
        &mut self,
        token: u64,
        drained_count: usize,
        replay_succeeded: bool,
    ) {
        if !replay_succeeded {
            // The hook thread already classified and recorded this abort.
            // Do not turn an empty/cancelled decision gate into a second failure.
            self.invalidate_conversion_state();
            self.diagnostic("gate_ack", format!("token={token} result=cancelled"));
            return;
        }
        self.metrics
            .gate_replayed_events
            .fetch_add(drained_count as u64, Ordering::Relaxed);

        let deferred = self
            .deferred_drained_conversion
            .take()
            .filter(|deferred| deferred.gate_token == token);
        if let Some(deferred) = deferred {
            let pending_matches = self
                .pending_conversion
                .as_ref()
                .is_some_and(|pending| pending.space_down_sequence == deferred.boundary_sequence);
            let required_ms =
                self.pending_conversion
                    .as_ref()
                    .and_then(|pending| match self.text_edit_backend {
                        TextEditBackend::ProtectedPaste => Some(paste_gate_budget_ms(false)),
                        TextEditBackend::CapabilityProbe => Some(paste_gate_budget_ms(true)),
                        TextEditBackend::PhysicalReplay | TextEditBackend::ObserveOnly => {
                            let step_count = pending
                                .transaction
                                .forward
                                .erase_characters
                                .checked_add(pending.replay_keys.len())?
                                .checked_add(1)?;
                            conversion_gate_budget_ms(step_count, false, false)
                        }
                    });
            if pending_matches
                && required_ms.is_some_and(|required_ms| {
                    self.handoff_correction_gate(token, deferred.boundary_sequence, required_ms)
                })
            {
                // The handed-over gate had at least `required_ms` left.
                self.edit_deadline.set(
                    required_ms
                        .map(|required_ms| Instant::now() + Duration::from_millis(required_ms)),
                );
                self.execute_pending_conversion(deferred.boundary_sequence);
                return;
            }
            self.pending_conversion = None;
        }
        self.acknowledge_correction_gate_drain(token);
    }

    fn reset_for_epoch(&mut self, epoch: u64) {
        let dropped_events = self.metrics.dropped_events.load(Ordering::Acquire);
        let queue_overflowed = dropped_events != self.last_dropped_events;
        self.log_word_discard(
            "epoch",
            WordDiscardDetail::Epoch {
                overflow: queue_overflowed,
            },
        );
        self.invalidate_conversion_state();
        // Input the worker never saw (a queue overflow, or keys a recovery replayed past the
        // hooks) leaves the current word unknown until the next boundary.
        if queue_overflowed || core::mem::take(&mut self.recovered_mid_word) {
            self.session
                .handle(InputEvent::QueueOverflow, None, &self.detector);
        } else {
            self.session.clear();
        }
        self.replay_keys.clear();
        self.modifiers = Modifiers::default();
        self.resync_modifiers = true;
        self.metrics
            .undo_hotkey_active
            .store(false, Ordering::Release);
        self.metrics
            .hotkey_waiting_for_release
            .store(false, Ordering::Release);
        self.mark_privacy_dirty(queue_overflowed);
        self.last_input_epoch = epoch;
        self.last_dropped_events = dropped_events;
    }

    fn recover_retained_input(&mut self, sequence: u64, explicit: bool) {
        let record = self
            .metrics
            .retained_input
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(record) = record else {
            return;
        };
        if record.delivery_uncertain && !explicit {
            return;
        }
        // The paste of this gate may have left the word selected: replaying the keys would type
        // over it, so only the user brings them back.
        if !explicit && self.metrics.selection_left_token.load(Ordering::Acquire) == record.token {
            self.diagnostic(
                "recovery",
                format!(
                    "token={} result=retained reason=selection-left",
                    record.token
                ),
            );
            return;
        }
        if !self.input_sequence_is_current(sequence) || !self.foreground_matches(record.foreground)
        {
            self.diagnostic(
                "recovery",
                format!(
                    "token={} result=retained reason=context-or-new-input",
                    record.token
                ),
            );
            return;
        }
        let release_deadline = Instant::now() + Duration::from_millis(HOOK_FORWARD_TIMEOUT_MS);
        while !physical_modifiers_released() {
            if Instant::now() >= release_deadline || !self.input_sequence_is_current(sequence) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        self.privacy_needs_check = true;
        self.ensure_privacy(record.foreground, ProbeTrigger::Recovery);
        if !recovery_privacy_allows(self.privacy_reason) {
            self.diagnostic(
                "recovery",
                format!("token={} result=retained reason=privacy", record.token),
            );
            return;
        }
        let Some(inputs) = build_recovery_inputs(&record.events) else {
            return;
        };
        if !self.input_sequence_is_current(sequence)
            || self.metrics.input_epoch.load(Ordering::Acquire) != self.last_input_epoch
            || current_foreground_context().is_none_or(|current| current != record.foreground)
            || !physical_modifiers_released()
        {
            return;
        }
        // Marked under the lock before sending, so that the lifetime timer cannot report keys as
        // deleted while they are being typed.
        if !begin_retained_recovery(&self.metrics, record.token) {
            self.diagnostic(
                "recovery",
                format!("token={} result=retained reason=gone", record.token),
            );
            return;
        }
        let sent = send_input_count(&inputs);
        finish_retained_recovery(&self.metrics, record.token, sent, inputs.len());
        self.finish_recovery(record.token, explicit, sent, inputs.len());
    }

    /// Bookkeeping after a recovery sent `sent` of `requested` inputs. The replayed keys bypass
    /// the hooks, so the word they may have started is unknown: the session stays suppressed until
    /// the next boundary, also across the epoch reset that an automatic resume causes.
    fn finish_recovery(&mut self, token: u64, explicit: bool, sent: usize, requested: usize) {
        self.invalidate_conversion_state();
        self.suppress_session();
        self.recovered_mid_word = true;
        self.diagnostic("recovery", format!("token={token} explicit={explicit} submitted={sent} requested={requested} evidence=input-submitted-text-unverified"));
        // Only the automatic recovery of a gate abort, and only when every held key was handed
        // back, may resume automatic conversion; explicit or partial recovery leaves it paused.
        if should_resume_after_recovery(explicit, sent, requested) {
            self.resume_after_recovered_abort(Instant::now());
        }
    }

    /// Policy D1: a gate abort whose held input came back in full, or was deleted after nobody
    /// recovered it within its lifetime, is no reason to stay off. Automatic conversion resumes
    /// when that abort was the only reason for the pause, at most three times an hour; any other
    /// pause, including one the user chose, stays as it is.
    fn resume_after_recovered_abort(&mut self, now: Instant) {
        let metrics = &self.metrics;
        let paused_by_the_abort = !metrics.auto_enabled.load(Ordering::Acquire)
            && metrics.safety_paused.load(Ordering::Acquire)
            && metrics.last_failure_reason.load(Ordering::Acquire)
                == ConversionFailureReason::GateDrain as u8;
        if !paused_by_the_abort {
            return;
        }
        let aborted_gate = metrics.cancelled_gate_token.load(Ordering::Acquire);
        if aborted_gate != 0 && self.failed_edit_gate_token == aborted_gate {
            // An automatic edit failed because of this abort, so the text is in an unknown
            // state: that pause is not the abort's alone.
            self.diagnostic(
                "gate",
                "phase=auto_resume_declined reason=failed-edit".to_owned(),
            );
            return;
        }
        if !self.auto_resumes.try_take(now) {
            self.diagnostic(
                "gate",
                "phase=auto_resume_declined reason=budget".to_owned(),
            );
            return;
        }
        let metrics = &self.metrics;
        metrics
            .last_failure_reason
            .store(ConversionFailureReason::None as u8, Ordering::Release);
        metrics.safety_paused.store(false, Ordering::Release);
        metrics.auto_enabled.store(true, Ordering::Release);
        // Events queued while paused are stale now, as after a manual toggle.
        metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        self.diagnostic("gate", "phase=auto_resumed".to_owned());
    }

    fn process_key(&mut self, event: RawKeyEvent) {
        if self.metrics.active_gate_token.load(Ordering::Acquire) == 0 {
            self.refresh_input_profiles();
        }
        let foreground_key = foreground_identity_key(event.foreground);
        let language = self.language_for_layout(event.foreground.layout);
        if self.last_foreground != Some(foreground_key) {
            if let Some(previous) = self.last_key_foreground {
                self.remember_word(previous, self.last_layout, Instant::now());
            }
            if let Some((hwnd, focus, process_id)) = self.last_foreground {
                // Handles are opaque identities; only which parts changed is
                // recorded, to explain words split by transient focus changes.
                self.log_word_discard(
                    "focus",
                    WordDiscardDetail::Focus {
                        window_changed: hwnd != foreground_key.0,
                        focus_changed: focus != foreground_key.1,
                        process_changed: process_id != foreground_key.2,
                        focus_was_null: focus == 0,
                        focus_is_null: foreground_key.1 == 0,
                    },
                );
            }
            self.last_word_reset = "focus";
            self.recovered_mid_word = false;
            self.recent_languages.clear();
            self.layout_switch_in_flight = None;
            self.invalidate_conversion_state();
            self.replay_keys.clear();
            self.last_boundary = None;
            self.transpose_cycle = None;
            self.session
                .handle(InputEvent::FocusChanged, language, &self.detector);
            // Keys released in the other window never reached this worker, and keys held across
            // the switch stay held: the modifiers come from this event.
            self.resync_modifiers = true;
            self.last_foreground = Some(foreground_key);
            self.last_layout = Some(event.foreground.layout);
            // A new window may belong to a new process that reuses the previous one's ID: its
            // image, integrity and exclusion are read again instead of taken from the cache.
            self.last_process_id = None;
            self.refresh_process_policy(event.foreground.process_id);
            self.mark_privacy_dirty(false);
        } else if self.last_layout != Some(event.foreground.layout) {
            let expected_switch = self
                .layout_switch_in_flight
                .is_some_and(|(target, deadline)| {
                    target == event.foreground.layout && Instant::now() <= deadline
                });
            if expected_switch {
                self.last_layout = Some(event.foreground.layout);
                self.layout_switch_in_flight = None;
            } else {
                self.last_word_reset = "layout";
                self.layout_switch_in_flight = None;
                // A manual switch ends automatic conversion of this word, not Pause: the word
                // typed so far, or else the last completed word, stays available to the hotkey
                // and is read in the layout it was typed in. Any other key drops it as usual.
                let typed = core::mem::take(&mut self.replay_keys);
                let kept = match self.last_layout {
                    Some(typed_layout) if !typed.is_empty() => Some(LastBoundary {
                        replay_keys: typed,
                        delimiter: None,
                        foreground: ForegroundContext {
                            layout: typed_layout,
                            ..event.foreground
                        },
                        epoch: self.last_input_epoch,
                        profile_generation: self.input_profiles.generation(),
                    }),
                    _ => self.last_boundary.take(),
                };
                self.invalidate_conversion_state();
                self.transpose_cycle = None;
                self.handle_switching_rule(
                    InputEvent::LayoutChanged,
                    language,
                    self.settings.suppress_after_manual_layout_change,
                );
                self.last_boundary = kept;
                self.last_layout = Some(event.foreground.layout);
            }
        } else if self
            .layout_switch_in_flight
            .is_some_and(|(_, deadline)| Instant::now() > deadline)
        {
            self.layout_switch_in_flight = None;
        }

        self.last_key_foreground = Some(event.foreground);

        let key_down = matches!(event.message, WM_KEYDOWN | WM_SYSKEYDOWN);
        let key_up = matches!(event.message, WM_KEYUP | WM_SYSKEYUP);
        if !key_down && !key_up {
            return;
        }

        if core::mem::take(&mut self.resync_modifiers) {
            self.modifiers = Modifiers::from_held(event.held_modifiers);
        }
        let modifier_event = self.modifiers.update(event.virtual_key, key_down, key_up);
        if event.hotkey_trigger {
            let released = self.settings.pause_break_undo && self.wait_for_hotkey_release(event);
            self.diagnostic(
                "hotkey",
                format!(
                    "sequence={} ready={released} undo_available={} buffered_chars={} replay_keys={} deferred={}",
                    event.sequence,
                    self.metrics.undo_available.load(Ordering::Acquire),
                    self.session.buffered_character_count(),
                    self.replay_keys.len(),
                    event.drain_token != 0
                ),
            );
            if released {
                let cycling = self
                    .transpose_cycle
                    .as_ref()
                    .is_some_and(|cycle| same_input_target(cycle.foreground, event.foreground));
                if cycling && event.drain_token == 0 {
                    // Keep walking the word through the layouts on every press.
                    self.execute_forced_conversion(event, language);
                } else if self.metrics.undo_available.load(Ordering::Acquire) {
                    self.execute_undo(event.sequence);
                } else if event.drain_token == 0 {
                    self.execute_forced_conversion(event, language);
                }
            }
            return;
        }
        if modifier_event {
            return;
        }
        if key_up && event.virtual_key as u16 == VK_SPACE.0 {
            return;
        }
        if !key_down {
            return;
        }
        if switches_keyboard_layout(event.virtual_key, self.modifiers) {
            // Win+Space changes the layout and types nothing. The word stays, so that once the
            // next key shows the new layout the manual-switch rule keeps it for Pause, as after
            // Alt+Shift, whose keys are all modifiers.
            return;
        }

        if self.modifiers.has_shortcut_modifier() {
            // Alt+Tab, Ctrl+Shift+Esc and the like end tracking below; keep the word for Pause.
            self.remember_word(
                event.foreground,
                Some(event.foreground.layout),
                Instant::now(),
            );
        }
        self.clear_undo();
        // Any non-modifier, non-hotkey key can change text or caret position,
        // including punctuation and otherwise unsupported application keys.
        // Only a plain Backspace may resume the word before an erased delimiter.
        let previous_boundary = self.last_boundary.take();
        self.transpose_cycle = None;
        if event.virtual_key as u16 != VK_SPACE.0 {
            self.pending_conversion = None;
            self.deferred_drained_conversion = None;
        } else if self.pending_conversion.is_some() {
            return;
        }

        if self.modifiers.has_shortcut_modifier() {
            self.last_word_reset = "shortcut";
            self.replay_keys.clear();
            self.last_boundary = None;
            self.transpose_cycle = None;
            if event.virtual_key as u16 == VK_BACK.0
                && self.modifiers.control()
                && !self.modifiers.alt()
                && self.session.erase_first_word_by_shortcut()
            {
                self.mark_privacy_dirty(false);
                return;
            }
            let kind = shortcut_kind(event.virtual_key, self.modifiers);
            self.diagnostic(
                "input",
                format!("result=suppressed reason=shortcut kind={kind}"),
            );
            self.session
                .handle(InputEvent::Shortcut, language, &self.detector);
            self.mark_privacy_dirty(true);
            return;
        }

        match event.virtual_key as u16 {
            key if key == VK_BACK.0 => {
                if self.settings.suppress_after_backspace
                    || !self.erase_or_resume_word(previous_boundary, event, language)
                {
                    self.last_word_reset = "backspace";
                    self.replay_keys.clear();
                    self.handle_switching_rule(
                        InputEvent::Backspace,
                        language,
                        self.settings.suppress_after_backspace,
                    );
                }
            }
            key if key == VK_DELETE.0 => {
                self.last_word_reset = "delete";
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Delete,
                    language,
                    self.settings.suppress_after_delete,
                );
            }
            key if key == VK_LEFT.0 => {
                self.last_word_reset = "navigation";
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Navigation,
                    language,
                    self.settings.suppress_after_left,
                );
            }
            key if key == VK_RIGHT.0 => {
                self.last_word_reset = "navigation";
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Navigation,
                    language,
                    self.settings.suppress_after_right,
                );
            }
            key if key == VK_UP.0 => {
                self.last_word_reset = "navigation";
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Navigation,
                    language,
                    self.settings.suppress_after_up,
                );
            }
            key if key == VK_DOWN.0 => {
                self.last_word_reset = "navigation";
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Navigation,
                    language,
                    self.settings.suppress_after_down,
                );
            }
            key if key == VK_HOME.0 || key == VK_END.0 => {
                self.last_word_reset = "navigation";
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Navigation,
                    language,
                    self.settings.suppress_after_home_end,
                );
            }
            key if key == VK_SPACE.0 => {
                let last_keys = self.replay_keys.clone();
                self.transpose_cycle = None;
                // A second space invalidates adjacency too. Record the source
                // before attempting an edit so a partial failure cannot restore
                // stale boundary state after the failure path has revoked it.
                self.last_boundary = if !last_keys.is_empty() && language.is_some() {
                    Some(LastBoundary {
                        replay_keys: last_keys,
                        delimiter: Some(' '),
                        foreground: event.foreground,
                        epoch: self.last_input_epoch,
                        profile_generation: self.input_profiles.generation(),
                    })
                } else {
                    None
                };
                let completed = self.handle_boundary(language, Some(' '), false);
                // handle_boundary explains an unconverted word by what cancelled it, so the
                // boundary itself is recorded only after it.
                self.last_word_reset = "boundary";
                if let Some((transaction, replay_keys)) = completed {
                    // A candidate that is not converted says why, never what it was.
                    let not_attempted = if !self.metrics.auto_enabled.load(Ordering::Acquire) {
                        Some("automatic-off")
                    } else if self.modifiers.shift() {
                        Some("shift")
                    } else if replay_keys.len() != transaction.original.chars().count() {
                        Some("keys")
                    } else if replay_keys
                        .iter()
                        .any(|key| key.caps_lock != event.caps_lock)
                    {
                        Some("caps-lock")
                    } else {
                        None
                    };
                    if let Some(reason) = not_attempted {
                        self.diagnostic("word", format!("result=not-attempted reason={reason}"));
                    } else {
                        self.pending_conversion = Some(PendingConversion {
                            transaction,
                            foreground: event.foreground,
                            source_layout: event.foreground.layout,
                            profile_generation: self.input_profiles.generation(),
                            space_down_sequence: event.sequence,
                            replay_keys,
                            forced: false,
                            cycle_order: Vec::new(),
                        });
                    }
                }
                if event.drain_token == 0 && self.metrics.auto_enabled.load(Ordering::Acquire) {
                    self.execute_pending_conversion(event.sequence);
                } else if event.drain_token != 0 && self.pending_conversion.is_some() {
                    self.deferred_drained_conversion = Some(DeferredDrainedConversion {
                        gate_token: event.drain_token,
                        boundary_sequence: event.sequence,
                    });
                }
            }
            key if key == VK_TAB.0 => {
                self.last_boundary = None;
                self.transpose_cycle = None;
                self.handle_boundary(language, None, true);
                self.last_word_reset = "boundary";
            }
            key if may_move_focus_within_window(key) => {
                // These keys type nothing but can move the focus inside the window (closing a
                // dialog, switching panes, opening a menu): the next key gets a fresh privacy
                // check, and no earlier good check stands for it.
                self.last_word_reset = "navigation";
                self.replay_keys.clear();
                self.handle_switching_rule(InputEvent::Navigation, language, true);
                self.mark_privacy_dirty(false);
            }
            key if key == VK_RETURN.0 => {
                self.last_boundary = None;
                self.transpose_cycle = None;
                self.handle_boundary(language, None, true);
                self.last_word_reset = "boundary";
                self.session.mark_line_start();
            }
            _ => {
                if !self.ensure_privacy(event.foreground, ProbeTrigger::Typing)
                    && !self.manual_privacy_allows_current(event.foreground)
                {
                    // Nothing is logged here: one line per key would give a secret's length.
                    self.last_word_reset = "privacy";
                    self.suppress_session();
                    return;
                }
                if language.is_none() {
                    self.diagnostic("input", "result=suppressed reason=language".to_owned());
                }
                let printable = self.printable_character(event);
                // Only a key the offline tables cannot place needs its readings in the target
                // layouts, such as the Estonian "ü" typed as "[" in the English layout.
                let readings = match (
                    printable,
                    language,
                    replay_key_from_event(event, self.modifiers),
                ) {
                    (Some(character), Some(language), Some(key))
                        if !self.detector.can_extend_word(character, language) =>
                    {
                        self.key_readings(key, language, &mut self.key_mapper())
                    }
                    _ => Vec::new(),
                };
                let input_event = printable.map_or(InputEvent::UnsupportedInput, |character| {
                    if language.is_some_and(|language| {
                        self.detector
                            .can_extend_word_with_readings(character, language, &readings)
                    }) {
                        InputEvent::Printable(character)
                    } else if character.is_whitespace() || character.is_ascii_punctuation() {
                        InputEvent::Boundary
                    } else {
                        InputEvent::UnsupportedInput
                    }
                });
                if input_event == InputEvent::UnsupportedInput {
                    self.last_word_reset = "unsupported";
                    self.diagnostic(
                        "input",
                        format!(
                            "result=suppressed reason=unsupported class={}",
                            unsupported_key_class(printable)
                        ),
                    );
                }
                if matches!(input_event, InputEvent::Printable(_)) {
                    // A new word has started; the previous word is no longer the
                    // one adjacent to the caret.
                    self.last_boundary = None;
                    self.transpose_cycle = None;
                }
                let boundary = input_event == InputEvent::Boundary;
                if boundary {
                    self.last_word_reset = "boundary";
                    self.recovered_mid_word = false;
                }
                let buffered_before = self.session.buffered_character_count();
                let token_keys =
                    self.take_manual_token_keys(input_event, printable.is_some(), buffered_before);
                if matches!(input_event, InputEvent::Printable(_)) && buffered_before == 0 {
                    // Bind the manual-only mark to the word being started. Resets
                    // such as focus, mouse or layout changes discard the previous
                    // word without passing through a boundary or suppression.
                    self.manual_only_word = self.privacy_reason.is_some();
                }
                let action = self.session.handle_with_readings(
                    input_event,
                    language,
                    &self.detector,
                    &readings,
                );
                match action {
                    SessionAction::Candidate(_) => {
                        // Deliberately retain only a counter. The candidate's
                        // original and replacement text are dropped without logs.
                        self.metrics.candidates.fetch_add(1, Ordering::Relaxed);
                    }
                    SessionAction::Reset(_) => self.replay_keys.clear(),
                    SessionAction::None => {}
                }
                if matches!(input_event, InputEvent::Printable(_))
                    && self.session.buffered_character_count() == buffered_before + 1
                {
                    if let Some(key) = replay_key_from_event(event, self.modifiers) {
                        self.replay_keys.push(key);
                    } else {
                        self.suppress_session();
                    }
                }
                if let Some(keys) = token_keys {
                    self.extend_manual_token(keys, replay_key_from_event(event, self.modifiers));
                }
                if boundary {
                    self.replay_keys.clear();
                    self.privacy_needs_check = true;
                }
            }
        }
    }

    /// Pause replays physical keys instead of looking words up, so a token
    /// such as "IPv6" typed in the wrong layout stays available for manual
    /// conversion. Its first unsupported character (for example a digit) still
    /// suppresses automatic conversion until the next boundary; afterwards the
    /// token is represented only by replay keys over a suppressed session.
    ///
    /// Text typed right after a shortcut (for example Ctrl+C in a terminal) or a
    /// manual layout switch is also kept as a manual token: its left context is
    /// unknown, so automatic conversion stays suppressed, but Pause replaces
    /// exactly the keys typed since the shortcut or the switch.
    /// Returns the keys to extend when this printable key continues the token.
    fn take_manual_token_keys(
        &mut self,
        input_event: InputEvent,
        printable: bool,
        buffered_before: usize,
    ) -> Option<Vec<ReplayKey>> {
        if !printable || input_event == InputEvent::Boundary {
            return None;
        }
        let suppressed = self.session.is_suppressed();
        let continues = suppressed && !self.replay_keys.is_empty();
        let starts_at_word = input_event == InputEvent::UnsupportedInput
            && !suppressed
            && self.replay_keys.len() == buffered_before;
        let starts_after_shortcut = suppressed
            && self.replay_keys.is_empty()
            && matches!(self.last_word_reset, "shortcut" | "layout");
        if starts_at_word || starts_after_shortcut {
            self.manual_token_start_known = starts_at_word;
        }
        (continues || starts_at_word || starts_after_shortcut)
            .then(|| core::mem::take(&mut self.replay_keys))
    }

    fn extend_manual_token(&mut self, mut keys: Vec<ReplayKey>, key: Option<ReplayKey>) {
        match key {
            Some(key) if keys.len() < MANUAL_TOKEN_MAX_KEYS => {
                keys.push(key);
                self.replay_keys = keys;
            }
            _ => self.replay_keys.clear(),
        }
    }

    fn wait_for_hotkey_release(&self, event: RawKeyEvent) -> bool {
        // The worker can receive a modifier-up event before the hook returns
        // and Windows updates GetAsyncKeyState. Wait for that exact edge to
        // finish instead of sending correction input with Ctrl/Alt still down.
        let deadline = Instant::now() + Duration::from_millis(HOOK_FORWARD_TIMEOUT_MS);
        loop {
            if !self.input_sequence_is_current(event.sequence)
                || !self.foreground_matches(event.foreground)
            {
                return false;
            }
            let forwarded = hotkey_modifier_bit(event.virtual_key as u16).is_none()
                || self
                    .metrics
                    .forwarded_input_sequence
                    .load(Ordering::Acquire)
                    >= event.sequence;
            if forwarded && physical_modifiers_released() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn handle_boundary(
        &mut self,
        language: Option<Language>,
        delimiter: Option<char>,
        may_change_focus: bool,
    ) -> Option<(ConversionTransaction, Vec<ReplayKey>)> {
        let replay_keys = core::mem::take(&mut self.replay_keys);
        let manual_only = core::mem::take(&mut self.manual_only_word);
        // A boundary ends the unknown word a recovery may have left.
        self.recovered_mid_word = false;
        // What happened to a word that is not converted is logged as a category only, so a
        // missed conversion can be explained from the log without recording the word.
        let suppressed = self.session.is_suppressed();
        let had_word = !replay_keys.is_empty() || self.session.buffered_character_count() != 0;
        let map_to_layout = self.key_mapper();
        let candidates = language
            .map(|language| {
                mapped_layout_candidates_with(
                    language,
                    &self.settings,
                    &self.detector,
                    |target_language| {
                        let profile = self.detector.input_profile(target_language)?;
                        let handle = self.resolved_profiles()?.unique_layout(profile)?;
                        map_to_layout(&replay_keys, handle)
                    },
                )
            })
            .unwrap_or_default();
        let transaction = if self.privacy_reason.is_none() && !manual_only {
            if let SessionAction::Candidate(detection) = self.session.finish_boundary_in_context(
                language,
                &self.detector,
                &candidates,
                &self.recent_languages,
            ) {
                self.metrics.candidates.fetch_add(1, Ordering::Relaxed);
                // Only the stage is logged, never the word.
                let stage = if self.detector.layout_model_enabled()
                    && self
                        .detector
                        .detect_mapped_candidates(
                            &detection.original,
                            detection.source_language,
                            &candidates,
                        )
                        .is_none()
                {
                    "model"
                } else {
                    "dictionary"
                };
                self.diagnostic(
                    "candidate",
                    format!(
                        "forced=false source={} target={} original_chars={} replacement_chars={} mapped_targets={} route={:?} stage={stage}",
                        detection.source_language.id(),
                        detection.target_language.id(),
                        detection.original.chars().count(),
                        detection.replacement.chars().count(),
                        candidates.len(),
                        self.text_edit_backend,
                    ),
                );
                let transaction = delimiter
                    .and_then(|delimiter| ConversionTransaction::new(&detection, delimiter));
                if transaction.is_none() {
                    // Tab or Enter: the edit could not put the word back before them.
                    self.diagnostic("word", "result=not-attempted reason=boundary".to_owned());
                }
                // The following words see this one as typed until its conversion is applied
                // (finish_applied_conversion).
                self.remember_word_language(detection.source_language);
                transaction
            } else {
                if !replay_keys.is_empty()
                    && let Some(language) = language
                {
                    self.remember_word_language(language);
                }
                if suppressed {
                    self.diagnostic(
                        "word",
                        format!("result=suppressed reset={}", self.last_word_reset),
                    );
                } else if had_word {
                    let result = if language.is_none() {
                        "no-language"
                    } else {
                        "no-candidate"
                    };
                    self.diagnostic("word", format!("result={result}"));
                }
                None
            }
        } else {
            if had_word {
                let result = if manual_only {
                    "manual-only"
                } else {
                    "privacy"
                };
                self.diagnostic("word", format!("result={result}"));
            }
            self.session.clear();
            None
        };
        self.privacy_needs_check = true;
        if may_change_focus {
            // Enter/Tab can move focus, but the next printable event already
            // carries a fresh foreground identity. Recheck privacy then
            // without suppressing the first word in the new context.
            self.mark_privacy_dirty(false);
        }
        transaction.map(|transaction| (transaction, replay_keys))
    }

    fn remember_word_language(&mut self, language: Language) {
        const RECENT_LANGUAGES: usize = 4;
        if self.recent_languages.len() == RECENT_LANGUAGES {
            self.recent_languages.remove(0);
        }
        self.recent_languages.push(language);
    }

    fn erase_or_resume_word(
        &mut self,
        previous_boundary: Option<LastBoundary>,
        event: RawKeyEvent,
        language: Option<Language>,
    ) -> bool {
        let map_to_layout = self.key_mapper();
        self.erase_or_resume_word_with(previous_boundary, event, language, map_to_layout)
    }

    /// Maps keys to what they print in a layout (by handle), as for the word's candidates.
    fn key_mapper(&self) -> impl Fn(&[ReplayKey], usize) -> Option<String> + use<> {
        #[cfg(test)]
        let keyboard = self.test_keyboard;
        move |keys, layout| {
            #[cfg(test)]
            if let Some(keyboard) = keyboard {
                return keys.iter().map(|&key| keyboard(key, layout)).collect();
            }
            map_replay_keys_to_layout(keys, HKL(layout as *mut c_void))
        }
    }

    /// The character a key event prints in its foreground layout.
    fn printable_character(&self, event: RawKeyEvent) -> Option<char> {
        #[cfg(test)]
        if let Some(keyboard) = self.test_keyboard {
            return keyboard(
                replay_key_from_event(event, self.modifiers)?,
                event.foreground.layout,
            );
        }
        translate_printable(event, self.modifiers)
    }

    /// What `key` prints in the layout of each enabled automatic target of `language`, from the
    /// same mapping that gives the word's candidates at its boundary.
    fn key_readings(
        &self,
        key: ReplayKey,
        language: Language,
        map_to_layout: &mut impl FnMut(&[ReplayKey], usize) -> Option<String>,
    ) -> Vec<(Language, char)> {
        let mut readings = Vec::new();
        for target in self.detector.automatic_targets(language) {
            if !self.settings.language_enabled(target) {
                continue;
            }
            let Some(layout) = self.find_layout(target) else {
                continue;
            };
            let Some(text) = map_to_layout(core::slice::from_ref(&key), layout.0 as usize) else {
                continue;
            };
            let mut characters = text.chars();
            if let (Some(character), None) = (characters.next(), characters.next()) {
                readings.push((target, character));
            }
        }
        readings
    }

    /// With Backspace suppression disabled, a plain Backspace edits the tracked
    /// word instead of discarding it. Erasing the delimiter directly after a
    /// word resumes that word for manual conversion when its input target,
    /// layout, epoch and profile generation are unchanged. Returns false when
    /// the caller must discard the word as before.
    fn erase_or_resume_word_with(
        &mut self,
        previous_boundary: Option<LastBoundary>,
        event: RawKeyEvent,
        language: Option<Language>,
        mut map_to_layout: impl FnMut(&[ReplayKey], usize) -> Option<String>,
    ) -> bool {
        if self.session.is_suppressed() && !self.replay_keys.is_empty() {
            // Manual token (see take_manual_token_keys). Erasing all of it
            // returns to the token start; only a known word start reopens
            // automatic conversion.
            self.replay_keys.pop();
            if self.replay_keys.is_empty() && self.manual_token_start_known {
                self.session.clear();
            }
            return true;
        }
        let buffered = self.session.buffered_character_count();
        if buffered != 0 {
            if self.replay_keys.len() != buffered || !self.session.erase_last_character() {
                return false;
            }
            self.replay_keys.pop();
            return true;
        }
        if self.session.is_suppressed() || !self.replay_keys.is_empty() {
            return false;
        }
        let (Some(last), Some(language)) = (previous_boundary, language) else {
            return false;
        };
        if last.delimiter.is_none()
            || !same_input_target(last.foreground, event.foreground)
            || last.foreground.layout != event.foreground.layout
            || last.epoch != self.metrics.input_epoch.load(Ordering::Acquire)
            || last.profile_generation != self.input_profiles.generation()
            || (self.privacy_reason.is_some()
                && !self.manual_privacy_allows_current(event.foreground))
        {
            return false;
        }
        let Some(word) = map_to_layout(&last.replay_keys, last.foreground.layout) else {
            return false;
        };
        if word.chars().count() != last.replay_keys.len()
            || !word
                .chars()
                .zip(&last.replay_keys)
                .all(|(character, &key)| {
                    self.detector.can_extend_word(character, language) || {
                        let readings = self.key_readings(key, language, &mut map_to_layout);
                        self.detector
                            .can_extend_word_with_readings(character, language, &readings)
                    }
                })
            || !self.session.restore_word(&word)
        {
            return false;
        }
        self.replay_keys = last.replay_keys;
        self.manual_only_word = true;
        true
    }

    fn handle_switching_rule(
        &mut self,
        event: InputEvent,
        language: Option<Language>,
        suppress_until_boundary: bool,
    ) {
        self.last_boundary = None;
        self.transpose_cycle = None;
        if suppress_until_boundary {
            self.session.handle(event, language, &self.detector);
        } else {
            self.log_word_discard("switching-rule", WordDiscardDetail::Rule(event.category()));
            self.session.clear();
        }
    }

    /// Record that a buffered word was dropped without suppressing the rest of
    /// it, so the following letters would start a new word. Only the category
    /// and length are logged, never text: the detail is typed and holds no string.
    fn log_word_discard(&self, reason: &'static str, detail: WordDiscardDetail) {
        let chars = self
            .session
            .buffered_character_count()
            .max(self.replay_keys.len());
        if chars != 0 {
            self.diagnostic(
                "word_discard",
                format!("reason={reason} chars={chars} {}", detail.describe()),
            );
        }
    }

    /// Keeps what Pause knew about `target` before the agent stops tracking it (a window switch,
    /// a click, a shortcut): the active cycle and the last word, or else the keys typed so far,
    /// read later in `typed_layout`. With nothing to keep, an earlier entry for the target stays.
    fn remember_word(
        &mut self,
        target: ForegroundContext,
        typed_layout: Option<usize>,
        now: Instant,
    ) {
        let word = if self.replay_keys.is_empty() {
            self.last_boundary.clone()
        } else {
            typed_layout.map(|layout| LastBoundary {
                replay_keys: self.replay_keys.clone(),
                delimiter: None,
                foreground: ForegroundContext { layout, ..target },
                epoch: self.last_input_epoch,
                profile_generation: self.input_profiles.generation(),
            })
        }
        .filter(|word| same_input_target(word.foreground, target));
        let cycle = self
            .transpose_cycle
            .clone()
            .filter(|cycle| same_input_target(cycle.foreground, target));
        self.remembered_words
            .retain(|entry| now.saturating_duration_since(entry.at) < REMEMBERED_WORD_TTL);
        if cycle.is_none() && word.is_none() {
            return;
        }
        self.remembered_words
            .retain(|entry| !same_input_target(entry.target, target));
        if self.remembered_words.len() >= REMEMBERED_WORD_WINDOWS {
            self.remembered_words.remove(0);
        }
        self.remembered_words.push(RememberedWord {
            target,
            cycle,
            word,
            at: now,
        });
    }

    /// When Pause finds nothing for this field, restores what was remembered for its input
    /// target, once, and only where the edit verifies the text before the caret (protected
    /// paste): anywhere else a stale word could be typed over other text. Returns the age.
    fn restore_remembered_word(
        &mut self,
        target: ForegroundContext,
        now: Instant,
    ) -> Option<Duration> {
        self.refresh_process_policy(target.process_id);
        if self.text_edit_backend != TextEditBackend::ProtectedPaste
            || self.transpose_cycle.is_some()
            || self.last_boundary.is_some()
            || !self.replay_keys.is_empty()
        {
            return None;
        }
        self.remembered_words
            .retain(|entry| now.saturating_duration_since(entry.at) < REMEMBERED_WORD_TTL);
        let position = self
            .remembered_words
            .iter()
            .position(|entry| same_input_target(entry.target, target))?;
        let entry = self.remembered_words.remove(position);
        // The paste checks the text before the caret, so an entry outlives input epochs (a
        // settings Apply, a mode toggle, an aborted gate). Only changed keyboard profiles make it
        // stale, and then it goes without touching the word being typed.
        let generation = self.input_profiles.generation();
        if entry
            .cycle
            .as_ref()
            .is_some_and(|cycle| cycle.profile_generation != generation)
            || entry
                .word
                .as_ref()
                .is_some_and(|word| word.profile_generation != generation)
        {
            self.diagnostic("hotkey", "source=remembered result=stale".to_owned());
            return None;
        }
        let epoch = self.metrics.input_epoch.load(Ordering::Acquire);
        self.transpose_cycle = entry.cycle.map(|cycle| TransposeCycle { epoch, ..cycle });
        self.last_boundary = entry.word.map(|word| LastBoundary { epoch, ..word });
        Some(now.saturating_duration_since(entry.at))
    }

    /// The word a Pause press acts on when no cycle is active: the keys typed since the last
    /// boundary (read in the current layout), or else the last word kept for the hotkey, with its
    /// delimiter and the layout it was typed in.
    fn forced_conversion_source(
        &self,
        event: RawKeyEvent,
    ) -> Option<(Vec<ReplayKey>, Option<char>, usize)> {
        if !self.replay_keys.is_empty() {
            return Some((self.replay_keys.clone(), None, event.foreground.layout));
        }
        self.last_boundary
            .as_ref()
            .filter(|last| same_input_target(last.foreground, event.foreground))
            .map(|last| {
                (
                    last.replay_keys.clone(),
                    last.delimiter,
                    last.foreground.layout,
                )
            })
    }

    fn execute_forced_conversion(&mut self, event: RawKeyEvent, _language: Option<Language>) {
        if let Some(age) = self.restore_remembered_word(event.foreground, Instant::now()) {
            self.diagnostic(
                "hotkey",
                format!("source=remembered age_ms={}", age.as_millis()),
            );
        }
        let epoch = self.metrics.input_epoch.load(Ordering::Acquire);
        let generation = self.input_profiles.generation();
        if self
            .transpose_cycle
            .as_ref()
            .is_some_and(|cycle| cycle.epoch != epoch || cycle.profile_generation != generation)
            || self
                .last_boundary
                .as_ref()
                .is_some_and(|last| last.epoch != epoch || last.profile_generation != generation)
        {
            self.diagnostic("hotkey", "result=ignored reason=stale-word".to_owned());
            self.invalidate_conversion_state();
            self.suppress_session();
            return;
        }
        // A press while a manual cycle is active advances it to the next layout.
        if let Some(cycle) = self
            .transpose_cycle
            .clone()
            .filter(|cycle| same_input_target(cycle.foreground, event.foreground))
        {
            if self.find_layout(cycle.language).is_none() {
                self.diagnostic(
                    "hotkey",
                    "result=ignored reason=layout-unavailable".to_owned(),
                );
                return;
            }
            if !self.forced_privacy_allows(event.foreground) {
                self.diagnostic("hotkey", "result=ignored reason=privacy".to_owned());
                return;
            }
            // The cycle knows its text; the edit starts from the layout the field has now, which
            // the user may have switched since the last press.
            self.advance_layout_cycle(
                cycle.replay_keys,
                cycle.delimiter,
                cycle.foreground,
                event.foreground.layout,
                cycle.language,
                cycle.text,
                event.sequence,
                Some(cycle.order),
            );
            return;
        }
        // The manual hotkey walks the word through every enabled layout, so it
        // works even when the word is not in any dictionary.
        let Some((replay_keys, delimiter, typed_layout)) = self.forced_conversion_source(event)
        else {
            self.diagnostic(
                "hotkey",
                format!(
                    "result=ignored reason=no-word-before-caret suppressed={} privacy={:?} manual_only={} last_reset={}",
                    self.session.is_suppressed(), self.privacy_reason, self.manual_only_word, self.last_word_reset,
                ),
            );
            return;
        };
        let Some(source_language) = self.language_for_layout(typed_layout) else {
            self.diagnostic(
                "hotkey",
                "result=ignored reason=unsupported-layout".to_owned(),
            );
            return;
        };
        let Some(current_text) = (self.key_mapper())(&replay_keys, typed_layout) else {
            // A key of the word gives no single character in the layout it was typed in, so
            // the text on screen cannot be read back and replaced safely.
            self.diagnostic(
                "hotkey",
                "result=ignored reason=key-without-character".to_owned(),
            );
            return;
        };
        if !self.forced_privacy_allows(event.foreground) {
            self.diagnostic("hotkey", "result=ignored reason=privacy".to_owned());
            return;
        }
        // The keys are read in the layout they were typed in; the edit starts from the layout the
        // field has now, which differs after a manual switch.
        self.advance_layout_cycle(
            replay_keys,
            delimiter,
            event.foreground,
            event.foreground.layout,
            source_language,
            current_text,
            event.sequence,
            None,
        );
    }

    /// Convert the word to the next enabled layout on each press. A first press (no
    /// `cycle_order`) chooses the order: see `layout_cycle_order`.
    #[allow(clippy::too_many_arguments)]
    fn advance_layout_cycle(
        &mut self,
        replay_keys: Vec<ReplayKey>,
        delimiter: Option<char>,
        foreground: ForegroundContext,
        source_layout: usize,
        source_language: Language,
        current_text: String,
        sequence: u64,
        cycle_order: Option<Vec<Language>>,
    ) {
        let profiles = self.resolved_profiles().cloned();
        let enabled: Vec<Language> = self
            .settings
            .enabled_input_packs
            .iter()
            .copied()
            .filter(|language| {
                let Some(profile) = self.detector.input_profile(*language) else {
                    return false;
                };
                profiles
                    .as_ref()
                    .and_then(|profiles| profiles.unique_layout(profile))
                    .is_some()
            })
            .collect();
        if enabled.len() < 2 {
            self.diagnostic("hotkey", "result=ignored reason=single-layout".to_owned());
            return;
        }
        if !enabled.contains(&source_language) {
            self.diagnostic(
                "hotkey",
                "result=ignored reason=unsupported-layout".to_owned(),
            );
            return;
        }
        let map_to_layout = self.key_mapper();
        let order = cycle_order
            .filter(|order| {
                order.len() == enabled.len()
                    && enabled.iter().all(|language| order.contains(language))
            })
            .unwrap_or_else(|| {
                let readings: Vec<(Language, String)> = enabled
                    .iter()
                    .copied()
                    .filter(|language| *language != source_language)
                    .filter_map(|language| {
                        let layout = self.find_layout(language)?;
                        Some((language, map_to_layout(&replay_keys, layout.0 as usize)?))
                    })
                    .collect();
                layout_cycle_order(
                    source_language,
                    &enabled,
                    &self.detector.rank_candidates(&readings),
                )
            });
        let Some(position) = order.iter().position(|entry| *entry == source_language) else {
            self.diagnostic(
                "hotkey",
                "result=ignored reason=unsupported-layout".to_owned(),
            );
            return;
        };
        // Skip enabled languages whose layout produces the same text (for
        // example Latin Estonian keeps a Latin English word unchanged) so one
        // press always reaches a visibly different representation when one
        // exists.
        // The presses always pass through the original layout: a layout that reads the keys as
        // the original text (Estonian for Latin letters typed in English) stands for it, so they
        // never leave the user in a lookalike layout that types ö or ü for ; and [.
        let original_text = self
            .find_layout(order[0])
            .and_then(|layout| map_to_layout(&replay_keys, layout.0 as usize));
        let mut target = None;
        for step in 1..order.len() {
            let candidate_index = (position + step) % order.len();
            let candidate_language = order[candidate_index];
            let Some(candidate_layout) = self.find_layout(candidate_language) else {
                continue;
            };
            let Some(candidate_text) = map_to_layout(&replay_keys, candidate_layout.0 as usize)
            else {
                continue;
            };
            if candidate_text != current_text {
                target = Some(if original_text.as_ref() == Some(&candidate_text) {
                    (0, order[0], candidate_text)
                } else {
                    (candidate_index, candidate_language, candidate_text)
                });
                break;
            }
        }
        let Some((next_index, target_language, next_text)) = target else {
            self.diagnostic("hotkey", "result=ignored reason=identical".to_owned());
            return;
        };
        let detection = Detection {
            source_language,
            target_language,
            original: current_text,
            replacement: next_text.clone(),
            source_score: 0.0,
            target_score: 1.0,
        };
        let transaction = match delimiter {
            Some(delimiter) => ConversionTransaction::new(&detection, delimiter),
            None => ConversionTransaction::without_delimiter(&detection),
        };
        let Some(transaction) = transaction else {
            self.diagnostic("hotkey", "result=ignored reason=edit-unit".to_owned());
            return;
        };
        self.diagnostic(
            "candidate",
            format!(
                "forced=true cycle_index={next_index} cycle={} source={} target={} original_chars={} replacement_chars={} route={:?}",
                order
                    .iter()
                    .map(|language| language.id())
                    .collect::<Vec<_>>()
                    .join(","),
                detection.source_language.id(),
                detection.target_language.id(),
                detection.original.chars().count(),
                detection.replacement.chars().count(),
                self.text_edit_backend,
            ),
        );
        self.metrics.candidates.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .forwarded_input_sequence
            .fetch_max(sequence, Ordering::AcqRel);
        self.pending_conversion = Some(PendingConversion {
            transaction,
            foreground,
            source_layout,
            profile_generation: self.input_profiles.generation(),
            space_down_sequence: sequence,
            replay_keys,
            forced: true,
            cycle_order: order,
        });
        self.execute_pending_conversion(sequence);
    }

    fn ensure_privacy(&mut self, foreground: ForegroundContext, trigger: ProbeTrigger) -> bool {
        self.refresh_process_policy(foreground.process_id);
        if self.privacy_needs_check {
            self.evaluate_privacy(foreground, trigger);
        }
        self.privacy_reason.is_none()
    }

    /// Revalidate process and focus before considering the explicit terminal
    /// exception. Cached transport failures never authorize a manual edit.
    fn forced_privacy_allows(&mut self, foreground: ForegroundContext) -> bool {
        let epoch = self.metrics.input_epoch.load(Ordering::Acquire);
        self.last_process_id = None;
        self.refresh_process_policy(foreground.process_id);
        self.evaluate_privacy(foreground, ProbeTrigger::Manual);
        epoch == self.metrics.input_epoch.load(Ordering::Acquire)
            && self.foreground_matches(foreground)
            && self.manual_privacy_allows_current(foreground)
    }

    fn manual_privacy_allows_current(&self, foreground: ForegroundContext) -> bool {
        self.manual_privacy_allows_reason(self.privacy_reason, foreground)
    }

    fn manual_privacy_allows_reason(
        &self,
        reason: Option<PrivacyBlockReason>,
        foreground: ForegroundContext,
    ) -> bool {
        let terminal = self.process_reason.is_none()
            && self.last_process_id == Some(foreground.process_id)
            && self.text_edit_backend == TextEditBackend::PhysicalReplay
            && self
                .process_facts(foreground.process_id)
                .0
                .is_some_and(|path| {
                    matches!(
                        path.rsplit(['/', '\\'])
                            .next()
                            .unwrap_or("")
                            .to_ascii_lowercase()
                            .as_str(),
                        "windowsterminal.exe" | "openconsole.exe" | "conhost.exe"
                    )
                });
        autokeyboardlayot::privacy::manual_conversion_allowed(
            reason,
            self.settings.manual_terminal_uia_fallback,
            terminal,
        )
    }

    /// Records each replacement of a stuck privacy provider once.
    fn report_probe_respawn(&mut self) {
        let Some(respawns) = self.privacy_probe.as_ref().map(BoundedProbe::respawn_count) else {
            return;
        };
        if respawns != self.reported_probe_respawns {
            self.reported_probe_respawns = respawns;
            self.diagnostic("probe", format!("respawned={respawns}"));
        }
    }

    fn evaluate_privacy(&mut self, foreground: ForegroundContext, trigger: ProbeTrigger) {
        self.refresh_process_policy(foreground.process_id);
        let started = Instant::now();
        let epoch = self.metrics.input_epoch.load(Ordering::Acquire);
        let (reason, source) = if let Some(reason) = self.process_reason {
            (Some(reason), "process-policy")
        } else {
            match self.privacy_probe.as_mut() {
                Some(probe) => probe_verdict(probe.query_result(
                    (foreground, epoch, self.privacy_generation),
                    Duration::from_millis(PRIVACY_PROBE_WAIT_MS),
                    Duration::from_millis(500),
                )),
                None => (
                    Some(PrivacyBlockReason::InspectionUnavailable),
                    "probe-missing",
                ),
            }
        };
        self.report_probe_respawn();
        // A late safe reply cannot authorize a different focus or input epoch.
        let context_current = self.metrics.input_epoch.load(Ordering::Acquire) == epoch
            && self.foreground_matches(foreground);
        self.apply_probe_answer(
            ProbeAnswer { reason, source },
            context_current,
            foreground,
            epoch,
            started,
            trigger,
        );
    }

    /// Everything that follows a probe answer: the context check, the recent-good rule, the log
    /// line and the new privacy state of the word being typed.
    fn apply_probe_answer(
        &mut self,
        answer: ProbeAnswer,
        context_current: bool,
        foreground: ForegroundContext,
        epoch: u64,
        started: Instant,
        trigger: ProbeTrigger,
    ) {
        let ProbeAnswer {
            mut reason,
            mut source,
        } = answer;
        if !context_current {
            reason = Some(PrivacyBlockReason::InspectionUnavailable);
            source = "context-changed";
        }
        let verdict =
            self.settle_probe_outcome(reason, source, foreground, epoch, Instant::now(), trigger);
        let (reason, source) = (verdict.reason, verdict.source);
        let changed = reason != self.privacy_reason;
        self.metrics.privacy_probe_cost_ms.store(
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            Ordering::Release,
        );
        self.privacy_needs_check = false;
        let manual_exception = reason == Some(PrivacyBlockReason::FieldInspectionUnavailable)
            && self.manual_privacy_allows_reason(reason, foreground);
        self.set_privacy_reason_with_manual_exception(reason, manual_exception);
        // Logged under the new verdict, so a check that blocks the field leaves no line.
        if changed
            || verdict.recent_ok_age.is_some()
            || started.elapsed() >= Duration::from_millis(SLOW_INPUT_DIAGNOSTIC_MS)
        {
            let leaned_on = verdict
                .recent_ok_age
                .map_or_else(String::new, |age| format!(" age_ms={}", age.as_millis()));
            self.diagnostic(
                "privacy_probe",
                format!(
                    "elapsed_ms={} result={reason:?} source={source} trigger={} wait_budget_ms={PRIVACY_PROBE_WAIT_MS}{leaned_on}",
                    started.elapsed().as_millis(),
                    trigger.label()
                ),
            );
        }
    }

    fn refresh_process_policy(&mut self, process_id: u32) {
        if self.last_process_id == Some(process_id) {
            return;
        }
        self.last_process_id = Some(process_id);
        let (image, integrity) = self.process_facts(process_id);
        let process_name = image
            .as_deref()
            .and_then(|path| path.rsplit(['/', '\\']).next())
            .filter(|name| !name.is_empty())
            .unwrap_or("unavailable")
            .to_owned();
        self.text_edit_backend = image
            .as_deref()
            .map_or(TextEditBackend::ObserveOnly, |path| {
                text_edit_backend_for_strategy(self.backend_rules.resolve(path))
            });
        self.metrics
            .backend_status
            .store(self.text_edit_backend.status_code(), Ordering::Release);
        self.process_reason = match (image, integrity, self.current_integrity_level) {
            (Some(path), _, _) if self.exclusion_policy.is_excluded(&path) => {
                Some(PrivacyBlockReason::ExcludedProcess)
            }
            (Some(_), Some(target), Some(current)) if target > current => {
                Some(PrivacyBlockReason::ElevatedProcess)
            }
            (Some(_), Some(_), Some(_)) => None,
            _ => Some(PrivacyBlockReason::InspectionUnavailable),
        };
        self.privacy_needs_check = true;
        // A blocked process leaves no line: it would mark when typing there began.
        if self.process_reason.is_none() {
            self.diagnostic(
                "context",
                format!(
                    "pid={process_id} process={process_name} route={:?}",
                    self.text_edit_backend
                ),
            );
        }
    }

    /// Applies the recent-good rule to one finished check.
    ///
    /// The provider of a busy window (Windows Terminal while the user types and our own
    /// replacement edits arrive) sometimes answers a check late, and the probe reports that as
    /// `FieldInspectionUnavailable`. Taken at face value it makes the word being typed
    /// manual-only, so fast typing in a terminal loses the automatic conversion of a word now and
    /// then. When the same input target (window, focus, thread, epoch) passed a check less than
    /// `PRIVACY_RECENT_OK_MS` ago, and nothing that can move the focus has happened since (that
    /// clears the record), one such failure keeps the earlier verdict.
    ///
    /// Only the provider's own field-inspection failure is softened. A password field, an
    /// excluded or unreadable process, a changed context and every transport failure stand, and a
    /// failure never renews the good check, so a window that keeps failing counts as failing
    /// after two seconds.
    fn settle_probe_outcome(
        &mut self,
        reason: Option<PrivacyBlockReason>,
        source: &'static str,
        foreground: ForegroundContext,
        epoch: u64,
        now: Instant,
        trigger: ProbeTrigger,
    ) -> ProbeVerdict {
        let from_provider = source == "provider";
        if from_provider && reason.is_none() {
            self.recent_privacy_ok = Some(RecentPrivacyOk {
                foreground,
                epoch,
                at: now,
            });
        } else if from_provider
            && reason == Some(PrivacyBlockReason::FieldInspectionUnavailable)
            && trigger.keeps_recent_ok()
            && let Some(recent) = self.recent_privacy_ok
            && same_input_target(recent.foreground, foreground)
            && recent.epoch == epoch
            && let Some(age) = now.checked_duration_since(recent.at)
            && age < Duration::from_millis(PRIVACY_RECENT_OK_MS)
        {
            return ProbeVerdict {
                reason: None,
                source: "recent-ok",
                recent_ok_age: Some(age),
            };
        } else {
            self.recent_privacy_ok = None;
        }
        ProbeVerdict {
            reason,
            source,
            recent_ok_age: None,
        }
    }

    /// Asks for a conversion sound when the user enabled them. The sound plays on a thread of its
    /// own; a cue that arrives while one plays is dropped, never queued.
    fn request_sound(&mut self, cue: SoundCue) {
        if !self.settings.conversion_sound {
            return;
        }
        if self.sound_player.is_none() {
            self.sound_player = start_sound_player();
        }
        if let Some(player) = &self.sound_player {
            let _ = player.try_send(cue);
        }
    }

    /// Publishes whether a full-screen program is in the foreground, for the hook watchdog, and
    /// logs each change.
    fn observe_full_screen(&mut self, full_screen: bool) {
        let previous = self
            .metrics
            .foreground_full_screen
            .swap(full_screen, Ordering::AcqRel);
        if previous != full_screen {
            self.diagnostic("foreground", format!("full_screen={full_screen}"));
        }
    }

    fn mark_privacy_dirty(&mut self, pause_now: bool) {
        self.privacy_needs_check = true;
        // A check still running for the field before this input must not decide the next one.
        self.privacy_generation = self.privacy_generation.wrapping_add(1);
        // Tab, Enter, a click, a shortcut, injected input and focus changes all come through
        // here: the field may be another one now, so no earlier good check applies to it.
        self.recent_privacy_ok = None;
        if pause_now {
            // Keep the triggering category (shortcut, external input) visible
            // in diagnostics and to manual-token handling.
            let reset = self.last_word_reset;
            self.set_privacy_reason(
                self.process_reason
                    .or(Some(PrivacyBlockReason::InspectionUnavailable)),
            );
            self.last_word_reset = reset;
        }
    }

    fn set_privacy_reason(&mut self, reason: Option<PrivacyBlockReason>) {
        self.set_privacy_reason_with_manual_exception(reason, false);
    }

    // Only evaluate_privacy supplies an exception after validating the input
    // context, process policy and explicit terminal opt-in. Other callers deny.
    fn set_privacy_reason_with_manual_exception(
        &mut self,
        reason: Option<PrivacyBlockReason>,
        manual_exception: bool,
    ) {
        self.privacy_reason = reason;
        self.metrics
            .privacy_reason
            .store(privacy_reason_code(reason), Ordering::Release);
        if reason.is_some() {
            if manual_exception && reason == Some(PrivacyBlockReason::FieldInspectionUnavailable) {
                self.manual_only_word = true;
                self.pending_conversion = None;
                self.deferred_drained_conversion = None;
                self.clear_undo();
            } else {
                self.last_word_reset = "privacy";
                self.invalidate_conversion_state();
                self.suppress_session();
            }
        }
    }

    fn suppress_session(&mut self) {
        self.replay_keys.clear();
        self.manual_only_word = false;
        self.session
            .handle(InputEvent::UnsupportedInput, None, &self.detector);
    }

    fn execute_pending_conversion(&mut self, boundary_sequence: u64) {
        self.execute_pending_conversion_inner(boundary_sequence);
        self.end_conversion_gate(boundary_sequence);
    }

    /// Ends the gate of a conversion or an undo. Its held keys are replayed, unless the edit may
    /// have left the word selected: then they are kept for recovery, as they would type over it.
    fn end_conversion_gate(&mut self, token: u64) {
        if self.held_keys_kept_for.take() == Some(token) {
            self.request_gate_retention(token);
        } else {
            self.request_gate_release(token);
        }
    }

    fn execute_pending_conversion_inner(&mut self, boundary_sequence: u64) {
        let Some(pending) = self.pending_conversion.take() else {
            return;
        };
        if boundary_sequence != pending.space_down_sequence
            && boundary_sequence != pending.space_down_sequence.wrapping_add(1)
        {
            return;
        }
        let privacy_blocks = if pending.forced {
            !self.manual_privacy_allows_current(pending.foreground)
        } else {
            self.privacy_reason.is_some()
        };
        if (!pending.forced && !self.metrics.auto_enabled.load(Ordering::Acquire))
            || privacy_blocks
            || !self.pending_profiles_match(&pending)
            || !self.pre_forward_guard_is_current(pending.foreground, boundary_sequence)
        {
            return;
        }
        let source_settle = self.wait_for_source_input_settle(
            pending.foreground,
            boundary_sequence,
            pending.source_layout,
            &pending.transaction.undo.insert_text,
        );
        self.diagnostic(
            "source_settle",
            format!(
                "sequence={boundary_sequence} result={source_settle:?} route={:?} forced={}",
                self.text_edit_backend, pending.forced
            ),
        );
        match source_settle {
            ReplayAttempt::Applied => {}
            ReplayAttempt::Cancelled => return,
            ReplayAttempt::Failed | ReplayAttempt::LayoutSwitchFailed => {
                self.record_conversion_failure(ConversionFailureReason::SourceBarrier);
                return;
            }
        }
        let target_layout = self
            .profile_inventory_is_current()
            .then(|| self.find_layout(pending.transaction.target_language))
            .flatten();
        let Some(target_layout) = target_layout else {
            self.diagnostic(
                "layout",
                format!(
                    "result=unavailable target={}",
                    pending.transaction.target_language.id()
                ),
            );
            self.record_conversion_failure(ConversionFailureReason::LayoutUnavailable);
            return;
        };
        if !self.edit_guard_is_current(pending.foreground, boundary_sequence) {
            return;
        }
        if self.text_edit_backend == TextEditBackend::ObserveOnly {
            return;
        }
        // From here an edit can submit input or fail partially. Retained text
        // must not survive an unknown outcome. Publish the next cycle only at
        // the successful completion point below; preflight refusals keep it.
        self.invalidate_conversion_state();
        self.session.clear();
        self.replay_keys.clear();
        let edit_strategy = match self.text_edit_backend {
            TextEditBackend::ObserveOnly => return,
            TextEditBackend::ProtectedPaste => {
                let attempt = self.perform_protected_paste(
                    pending.foreground,
                    boundary_sequence,
                    pending.source_layout,
                    target_layout,
                    &pending.transaction.undo.insert_text,
                    &pending.transaction.forward.insert_text,
                    paste_patience(pending.forced),
                );
                let Some(strategy) = self.apply_paste_attempt_result(
                    attempt,
                    boundary_sequence,
                    PasteRoute::Backend,
                ) else {
                    return;
                };
                strategy
            }
            TextEditBackend::PhysicalReplay => {
                let Some(strategy) =
                    self.complete_physical_conversion(&pending, boundary_sequence, target_layout)
                else {
                    return;
                };
                strategy
            }
            TextEditBackend::CapabilityProbe => {
                let barrier = self.wait_for_capability_text_barrier(
                    pending.foreground,
                    boundary_sequence,
                    pending.source_layout,
                    &pending.transaction.undo.insert_text,
                );
                let barrier_report = self.privacy_guard.last_barrier_report();
                let physical_fallback_allowed =
                    self.settings.physical_fallback_for_unsupported_apps
                        && physical_replay_fallback_is_safe(barrier_report.resolver_reason);
                self.diagnostic(
                    "capability_barrier",
                    format!(
                        "result={barrier:?} fallback_allowed={physical_fallback_allowed} expected_utf16={} delimiter={} report={}",
                        pending.transaction.undo.insert_text.encode_utf16().count(),
                        pending.transaction.delimiter.is_some(),
                        format_text_barrier_report(barrier_report),
                    ),
                );
                match capability_decision(barrier, physical_fallback_allowed) {
                    CapabilityDecision::ProtectedPaste => {
                        let attempt = self.perform_protected_paste(
                            pending.foreground,
                            boundary_sequence,
                            pending.source_layout,
                            target_layout,
                            &pending.transaction.undo.insert_text,
                            &pending.transaction.forward.insert_text,
                            paste_patience(pending.forced),
                        );
                        self.diagnostic(
                            "protected_paste",
                            format!(
                                "result={attempt:?} route=CapabilityProbe report={}",
                                format_text_barrier_report(
                                    self.privacy_guard.last_barrier_report()
                                )
                            ),
                        );
                        let Some(strategy) = self.apply_paste_attempt_result(
                            attempt,
                            boundary_sequence,
                            PasteRoute::CapabilityProbe,
                        ) else {
                            return;
                        };
                        strategy
                    }
                    CapabilityDecision::CancelMismatch => {
                        self.metrics
                            .backend_status
                            .store(BACKEND_CAPABILITY_MISMATCH, Ordering::Release);
                        return;
                    }
                    CapabilityDecision::Unsupported => {
                        self.metrics
                            .backend_status
                            .store(BACKEND_CAPABILITY_UNSUPPORTED, Ordering::Release);
                        return;
                    }
                    CapabilityDecision::PhysicalReplay => {
                        let Some(strategy) = self.complete_physical_conversion(
                            &pending,
                            boundary_sequence,
                            target_layout,
                        ) else {
                            return;
                        };
                        self.metrics
                            .backend_status
                            .store(BACKEND_PHYSICAL_REPLAY, Ordering::Release);
                        strategy
                    }
                }
            }
        };

        self.finish_applied_conversion(pending, boundary_sequence, target_layout, edit_strategy);
    }

    /// The bookkeeping of a conversion whose edit was applied: the target layout is current, the
    /// word may be resumed, cycled, undone or added to the dictionary, and the following words
    /// see an automatically converted word as the target language. Its boundary recorded it as
    /// typed, so a conversion that ends without an edit leaves the model's context as typed.
    fn finish_applied_conversion(
        &mut self,
        pending: PendingConversion,
        boundary_sequence: u64,
        target_layout: HKL,
        edit_strategy: EditStrategy,
    ) {
        if !pending.forced
            && let Some(last) = self.recent_languages.last_mut()
            && *last == pending.transaction.source_language
        {
            *last = pending.transaction.target_language;
        }
        self.commit_confirmed_layout(target_layout.0 as usize);
        self.session.clear();
        if let Some(delimiter) = pending.transaction.delimiter {
            self.last_boundary = Some(LastBoundary {
                replay_keys: pending.replay_keys.clone(),
                delimiter: Some(delimiter),
                foreground: ForegroundContext {
                    layout: target_layout.0 as usize,
                    ..pending.foreground
                },
                epoch: self.last_input_epoch,
                profile_generation: pending.profile_generation,
            });
        }
        if pending.forced {
            self.transpose_cycle = Some(TransposeCycle {
                replay_keys: pending.replay_keys.clone(),
                delimiter: pending.transaction.delimiter,
                foreground: pending.foreground,
                language: pending.transaction.target_language,
                text: pending.transaction.replacement.clone(),
                epoch: self.last_input_epoch,
                profile_generation: pending.profile_generation,
                order: pending.cycle_order.clone(),
            });
        }
        self.request_sound(SoundCue::Converted);
        self.diagnostic(
            "conversion",
            format!(
                "sequence={boundary_sequence} result=applied evidence={} strategy={edit_strategy:?} source={} target={} forced={}",
                if edit_strategy == EditStrategy::PhysicalReplay { "input-submitted-text-unverified" } else { "text-verified" },
                pending.transaction.source_language.id(),
                pending.transaction.target_language.id(),
                pending.forced,
            ),
        );
        if pending.forced && self.settings.offer_dictionary_after_forced_conversion {
            self.set_lexicon_offer(
                LexiconOfferTarget::UserDictionary,
                pending.transaction.target_language,
                pending.transaction.replacement.clone(),
            );
        }
        // Forced conversions are part of a manual cycle, so they do not offer a
        // one-press undo; the user cycles back through the layouts instead.
        if !pending.forced {
            self.undo_record = Some(UndoRecord {
                transaction: pending.transaction,
                foreground: pending.foreground,
                source_layout: pending.source_layout,
                profile_generation: pending.profile_generation,
                replay_keys: pending.replay_keys,
                edit_strategy,
            });
            self.metrics.undo_available.store(true, Ordering::Release);
        }
    }

    fn complete_physical_conversion(
        &mut self,
        pending: &PendingConversion,
        input_sequence: u64,
        target_layout: HKL,
    ) -> Option<EditStrategy> {
        match self.perform_physical_edit(
            pending.foreground,
            input_sequence,
            target_layout,
            pending.transaction.forward.erase_characters,
            &pending.replay_keys,
            pending.transaction.delimiter,
        ) {
            ReplayAttempt::Applied => {}
            ReplayAttempt::Cancelled => return None,
            ReplayAttempt::LayoutSwitchFailed if pending.forced => {
                self.record_manual_edit_failure("layout-switch");
                return None;
            }
            ReplayAttempt::Failed | ReplayAttempt::LayoutSwitchFailed => {
                self.record_conversion_failure(ConversionFailureReason::PhysicalEdit);
                return None;
            }
        }
        let commit = self.wait_for_replay_commit(
            pending.foreground,
            input_sequence,
            target_layout.0 as usize,
        );
        self.finish_replay_commit(
            commit,
            input_sequence,
            target_layout.0 as usize,
            "conversion",
        )
        .then_some(EditStrategy::PhysicalReplay)
    }

    /// What the commit wait of a physical replay found. Every key was sent by then: a guard lost
    /// while waiting (new input, a click) leaves the edit applied but unconfirmed, which is logged
    /// as `applied-uncertain` under `event`. True when the edit is confirmed.
    fn finish_replay_commit(
        &mut self,
        commit: ReplayAttempt,
        sequence: u64,
        layout: usize,
        event: &'static str,
    ) -> bool {
        match commit {
            ReplayAttempt::Applied => true,
            ReplayAttempt::Cancelled => {
                self.diagnostic(
                    event,
                    format!(
                        "sequence={sequence} result=applied-uncertain strategy=PhysicalReplay evidence=input-submitted-guard-lost"
                    ),
                );
                self.layout_switch_in_flight =
                    Some((layout, Instant::now() + Duration::from_secs(1)));
                self.session.clear();
                false
            }
            ReplayAttempt::Failed | ReplayAttempt::LayoutSwitchFailed => {
                self.record_conversion_failure(ConversionFailureReason::ReplayCommit);
                false
            }
        }
    }

    fn wait_for_source_input_settle(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
        source_layout: usize,
        expected_text: &str,
    ) -> ReplayAttempt {
        let forward_deadline = Instant::now() + Duration::from_millis(HOOK_FORWARD_TIMEOUT_MS);
        while self
            .metrics
            .forwarded_input_sequence
            .load(Ordering::Acquire)
            < input_sequence
        {
            let active_gate = self.metrics.active_gate_token.load(Ordering::Acquire);
            if self.metrics.cancelled_gate_token.load(Ordering::Acquire) == input_sequence
                || !self.foreground_matches(foreground)
                || (active_gate != input_sequence
                    && (!self.input_sequence_is_current(input_sequence)
                        || !physical_modifiers_released()))
            {
                return ReplayAttempt::Cancelled;
            }
            if Instant::now() >= forward_deadline {
                return ReplayAttempt::Cancelled;
            }
            thread::sleep(Duration::from_millis(1));
        }

        if !self.arm_correction_gate(input_sequence) {
            return ReplayAttempt::Cancelled;
        }

        let settle_ms = if self.text_edit_backend.requires_text_barrier() {
            NOTEPAD_TEXT_BARRIER_TIMEOUT_MS
        } else {
            INPUT_SETTLE_AFTER_FORWARD_MS
        };
        let settle_deadline = Instant::now() + Duration::from_millis(settle_ms);
        while Instant::now() < settle_deadline {
            if !self.edit_guard_is_current(foreground, input_sequence) {
                return ReplayAttempt::Cancelled;
            }
            if self
                .current_foreground()
                .is_none_or(|current| current.layout != source_layout)
            {
                return ReplayAttempt::Cancelled;
            }
            if self.text_edit_backend.requires_text_barrier()
                && matches!(
                    self.settle_text_status(foreground.process_id, expected_text, settle_deadline),
                    TextBarrierStatus::Match
                )
            {
                return ReplayAttempt::Applied;
            }
            thread::sleep(Duration::from_millis(2));
        }
        if self.text_edit_backend.requires_text_barrier() {
            ReplayAttempt::Cancelled
        } else {
            ReplayAttempt::Applied
        }
    }

    fn wait_for_text_commit(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
        expected_layout: usize,
        expected_text: &str,
    ) -> ReplayAttempt {
        let deadline = Instant::now() + Duration::from_millis(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS);
        while Instant::now() < deadline {
            if !self.edit_guard_is_current(foreground, input_sequence) {
                self.diagnostic(
                    "paste_guard",
                    format!("sequence={input_sequence} reason=input-or-focus-changed"),
                );
                return ReplayAttempt::Cancelled;
            }
            let current = current_foreground_context();
            if current.is_none_or(|current| current.layout != expected_layout) {
                self.diagnostic("paste_guard", format!("sequence={input_sequence} reason=layout-changed expected={expected_layout:x} actual={:x}", current.map_or(0, |context| context.layout)));
                return ReplayAttempt::Failed;
            }
            if matches!(
                self.barrier_text_status(foreground.process_id, expected_text, deadline),
                TextBarrierStatus::Match
            ) {
                return ReplayAttempt::Applied;
            }
            thread::sleep(Duration::from_millis(2));
        }
        ReplayAttempt::Failed
    }

    fn wait_for_capability_text_barrier(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
        expected_layout: usize,
        expected_text: &str,
    ) -> TextBarrierStatus {
        let deadline = Instant::now() + Duration::from_millis(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS);
        let mut saw_mismatch = false;
        while Instant::now() < deadline {
            if !self.edit_guard_is_current(foreground, input_sequence)
                || current_foreground_context()
                    .is_none_or(|current| current.layout != expected_layout)
            {
                return TextBarrierStatus::Mismatch;
            }
            match self.barrier_text_status(foreground.process_id, expected_text, deadline) {
                TextBarrierStatus::Match => return TextBarrierStatus::Match,
                TextBarrierStatus::Mismatch => saw_mismatch = true,
                TextBarrierStatus::Unavailable => {}
            }
            thread::sleep(Duration::from_millis(2));
        }
        if saw_mismatch {
            TextBarrierStatus::Mismatch
        } else {
            TextBarrierStatus::Unavailable
        }
    }

    fn wait_for_replay_commit(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
        expected_layout: usize,
    ) -> ReplayAttempt {
        let deadline = Instant::now() + Duration::from_millis(REPLAY_COMMIT_MS);
        while Instant::now() < deadline {
            if !self.edit_guard_is_current(foreground, input_sequence) {
                return ReplayAttempt::Cancelled;
            }
            if current_foreground_context().is_none_or(|current| current.layout != expected_layout)
            {
                return ReplayAttempt::Failed;
            }
            thread::sleep(Duration::from_millis(2));
        }
        ReplayAttempt::Applied
    }

    fn execute_undo(&mut self, pause_sequence: u64) {
        self.diagnostic(
            "undo",
            format!("sequence={pause_sequence} result=requested"),
        );
        // Pause is intentionally swallowed by our hook. Treat its release as
        // resolved for the gate guard even though it was not forwarded to the
        // target application.
        self.metrics
            .forwarded_input_sequence
            .fetch_max(pause_sequence, Ordering::AcqRel);
        self.execute_undo_inner(pause_sequence);
        self.end_conversion_gate(pause_sequence);
    }

    fn execute_undo_inner(&mut self, pause_sequence: u64) {
        let Some(record) = self.undo_record.clone() else {
            return;
        };
        if !self.input_sequence_is_current(pause_sequence)
            || self.privacy_reason.is_some()
            || record.profile_generation != self.input_profiles.generation()
            || self.language_for_layout(record.source_layout)
                != Some(record.transaction.source_language)
            || !self.profile_inventory_is_current()
            || !physical_modifiers_released()
        {
            return;
        }
        if !self.foreground_matches(record.foreground)
            || current_foreground_context()
                .and_then(|current| self.language_for_layout(current.layout))
                != Some(record.transaction.target_language)
        {
            return;
        }
        if !self.arm_correction_gate(pause_sequence) {
            return;
        }
        let Some(record) = self.undo_record.take() else {
            return;
        };
        self.metrics.undo_available.store(false, Ordering::Release);
        let source_layout = HKL(record.source_layout as *mut c_void);
        match record.edit_strategy {
            EditStrategy::ProtectedPaste => {
                let Some(current_layout) =
                    current_foreground_context().map(|current| current.layout)
                else {
                    return;
                };
                let attempt = self.perform_protected_paste(
                    record.foreground,
                    pause_sequence,
                    current_layout,
                    source_layout,
                    &record.transaction.forward.insert_text,
                    &record.transaction.undo.insert_text,
                    PastePatience::Manual,
                );
                if self
                    .apply_paste_attempt_result(attempt, pause_sequence, PasteRoute::Backend)
                    .is_none()
                {
                    return;
                }
            }
            EditStrategy::PhysicalReplay => {
                match self.perform_physical_edit(
                    record.foreground,
                    pause_sequence,
                    source_layout,
                    record.transaction.undo.erase_characters,
                    &record.replay_keys,
                    record.transaction.delimiter,
                ) {
                    ReplayAttempt::Applied => {}
                    ReplayAttempt::Cancelled => return,
                    ReplayAttempt::LayoutSwitchFailed => {
                        self.record_manual_edit_failure("layout-switch");
                        return;
                    }
                    ReplayAttempt::Failed => {
                        self.record_conversion_failure(ConversionFailureReason::PhysicalEdit);
                        return;
                    }
                }
                let commit = self.wait_for_replay_commit(
                    record.foreground,
                    pause_sequence,
                    record.source_layout,
                );
                if !self.finish_replay_commit(commit, pause_sequence, record.source_layout, "undo")
                {
                    return;
                }
            }
        }

        self.commit_confirmed_layout(record.source_layout);
        self.session.clear();
        self.transpose_cycle = None;
        self.last_boundary = record.transaction.delimiter.map(|delimiter| LastBoundary {
            replay_keys: record.replay_keys.clone(),
            delimiter: Some(delimiter),
            foreground: ForegroundContext {
                layout: record.source_layout,
                ..record.foreground
            },
            epoch: self.last_input_epoch,
            profile_generation: record.profile_generation,
        });
        self.request_sound(SoundCue::Undone);
        self.diagnostic(
            "undo",
            format!(
                "sequence={pause_sequence} result=applied strategy={:?} evidence={}",
                record.edit_strategy,
                if record.edit_strategy == EditStrategy::PhysicalReplay {
                    "input-submitted-text-unverified"
                } else {
                    "text-verified"
                }
            ),
        );
        if self.settings.offer_word_exclusion_after_undo {
            self.set_lexicon_offer(
                LexiconOfferTarget::WordExclusions,
                record.transaction.source_language,
                record.transaction.original,
            );
        }
    }

    fn set_lexicon_offer(&self, target: LexiconOfferTarget, language: Language, word: String) {
        if let Ok(mut candidate) = self.lexicon_candidate.lock() {
            *candidate = Some(VolatileLexiconCandidate::new(
                target,
                language,
                word,
                Instant::now(),
            ));
        }
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.gate_window as *mut c_void)),
                LEXICON_OFFER_MESSAGE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn perform_protected_paste(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
        source_layout: usize,
        target_layout: HKL,
        expected_source: &str,
        replacement: &str,
        patience: PastePatience,
    ) -> TextEditAttempt {
        let started = Instant::now();
        // From the selection on, the field may show the word selected or replaced: the UI thread
        // does not hand held keys back past a worker stuck in here.
        let _edit = BusyGuard::new(&self.metrics.edit_in_flight);
        let budget = PasteBudget {
            deadline: self.edit_deadline.get(),
        };
        if !self.edit_guard_is_current(foreground, input_sequence) {
            self.diagnostic(
                "paste",
                format!("sequence={input_sequence} phase=guard result=cancelled"),
            );
            return TextEditAttempt::Cancelled;
        }
        if !budget.allows_selection(Instant::now(), patience) {
            self.note_paste_budget(input_sequence, "selection", budget);
            return TextEditAttempt::Cancelled;
        }
        let (selection, cleanup) =
            self.paste_select(foreground.process_id, expected_source, patience);
        self.diagnostic("paste", format!("sequence={input_sequence} phase=selection result={selection:?} elapsed_ms={} report={}",
            started.elapsed().as_millis(), format_text_barrier_report(self.privacy_guard.last_barrier_report())));
        if cleanup == Some(SelectionCleanup::Unverified) {
            return TextEditAttempt::SelectionLeft;
        }
        match selection {
            TextBarrierStatus::Match => {}
            TextBarrierStatus::Mismatch => return TextEditAttempt::Cancelled,
            TextBarrierStatus::Unavailable => return TextEditAttempt::Unsupported,
        }
        let Some(install_timeout) = budget.install_timeout(Instant::now(), patience) else {
            self.note_paste_budget(input_sequence, "install", budget);
            return self.abandon_selection(foreground, input_sequence, TextEditAttempt::Cancelled);
        };
        let mut clipboard = match self.paste_clipboard(
            replacement,
            install_timeout,
            foreground.process_id,
        ) {
            Ok(clipboard) => clipboard,
            Err(failure) => {
                self.diagnostic(
                        "paste",
                        format!(
                            "sequence={input_sequence} phase=clipboard_prepare result={failure:?} elapsed_ms={}",
                            started.elapsed().as_millis()
                        ),
                    );
                // Nothing was typed and the user's clipboard was not replaced: a busy
                // clipboard is a word left alone, not a failed edit, so automatic conversion
                // stays on.
                return self.abandon_selection(
                    foreground,
                    input_sequence,
                    TextEditAttempt::Cancelled,
                );
            }
        };
        let clipboard_current = clipboard.is_current();
        self.diagnostic("paste", format!("sequence={input_sequence} phase=clipboard_prepare result=ready current={clipboard_current} elapsed_ms={}", started.elapsed().as_millis()));
        let submit_fits = budget.allows_submit(Instant::now());
        if !clipboard_current
            || !submit_fits
            || !self.edit_guard_is_current(foreground, input_sequence)
        {
            // Nothing was sent yet, so putting the user's clipboard back cannot reach the field.
            let restored = clipboard.restore();
            self.diagnostic("paste", format!("sequence={input_sequence} phase=before_submit result=cancelled restored={restored} budget_ok={submit_fits}"));
            return self.abandon_selection(foreground, input_sequence, TextEditAttempt::Cancelled);
        }

        clipboard.mark_submitted();
        let paste_inputs = build_paste_inputs();
        let submitted = self.paste_submit(&paste_inputs);
        self.diagnostic("paste", format!("sequence={input_sequence} phase=submit submitted={submitted} requested={} elapsed_ms={}", paste_inputs.len(), started.elapsed().as_millis()));
        if submitted != paste_inputs.len() {
            let control_up = [keyboard_input(VK_LCONTROL, 0, KEYEVENTF_KEYUP)];
            let _ = self.paste_submit(&control_up);
            // Part of the chord reached the field and may still paste: the replacement stays until
            // the field reads it.
            clipboard.defer();
            self.diagnostic(
                "paste",
                format!("sequence={input_sequence} phase=clipboard_restore result=deferred reason=partial-submit"),
            );
            return self.abandon_selection(
                foreground,
                input_sequence,
                TextEditAttempt::Failed(ConversionFailureReason::ClipboardEdit),
            );
        }

        let paste_commit =
            self.paste_commit(foreground, input_sequence, source_layout, replacement);
        self.diagnostic("paste", format!("sequence={input_sequence} phase=text_commit result={paste_commit:?} elapsed_ms={} report={}",
            started.elapsed().as_millis(), format_text_barrier_report(self.privacy_guard.last_barrier_report())));
        if clipboard_after_paste(paste_commit) == ClipboardAfterPaste::Defer {
            clipboard.defer();
            self.diagnostic(
                "paste",
                format!("sequence={input_sequence} phase=clipboard_restore result=deferred reason=paste-unconfirmed"),
            );
            // The paste may not have replaced the selected word: do not leave our selection
            // behind, unless the user has already moved on.
            return self.abandon_selection(
                foreground,
                input_sequence,
                TextEditAttempt::Failed(ConversionFailureReason::TextCommit),
            );
        }
        let owned_before_restore = clipboard.is_current();
        let clipboard_restored = clipboard.restore();
        self.diagnostic("paste", format!("sequence={input_sequence} phase=clipboard_restore result={clipboard_restored} owned_before={owned_before_restore} elapsed_ms={}", started.elapsed().as_millis()));
        let switched = self.paste_switch_layout(foreground, target_layout, input_sequence);
        self.diagnostic(
            "paste",
            format!(
                "sequence={input_sequence} phase=layout result={switched:?} elapsed_ms={}",
                started.elapsed().as_millis()
            ),
        );
        match switched {
            ReplayAttempt::Applied => {}
            ReplayAttempt::Cancelled
            | ReplayAttempt::Failed
            | ReplayAttempt::LayoutSwitchFailed => {
                return TextEditAttempt::Failed(ConversionFailureReason::LayoutSwitch);
            }
        }
        let target_commit = self.paste_commit(
            foreground,
            input_sequence,
            target_layout.0 as usize,
            replacement,
        );
        self.diagnostic("paste", format!("sequence={input_sequence} phase=target_commit result={target_commit:?} elapsed_ms={} report={}",
            started.elapsed().as_millis(), format_text_barrier_report(self.privacy_guard.last_barrier_report())));
        protected_paste_outcome(
            matches!(target_commit, ReplayAttempt::Applied),
            clipboard_restored,
        )
    }

    /// A paste step the gate's remaining time does not cover: the paste stops before it.
    fn note_paste_budget(&self, input_sequence: u64, step: &'static str, budget: PasteBudget) {
        self.diagnostic(
            "paste",
            format!(
                "sequence={input_sequence} phase=budget result=cancelled step={step} remaining_ms={}",
                budget
                    .remaining(Instant::now())
                    .map_or(0, |remaining| remaining.as_millis())
            ),
        );
    }

    /// Ends a paste that does not go ahead after the word was selected. While the input target
    /// and its input are still the edit's, the selection is collapsed: the attempt ends as
    /// `outcome` when the field shows it gone, and as `SelectionLeft` when it cannot be shown
    /// gone. Once the user acted (new input, a click, another field), what is selected may be
    /// theirs and is left alone; it may still be the agent's selection, so the attempt ends as
    /// `SelectionLeft` too, which keeps the held keys from being typed over it.
    fn abandon_selection(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
        outcome: TextEditAttempt,
    ) -> TextEditAttempt {
        if !self.edit_guard_is_current(foreground, input_sequence) {
            self.diagnostic(
                "paste",
                format!("sequence={input_sequence} phase=cleanup result=skipped reason=input-or-focus-changed"),
            );
            return TextEditAttempt::SelectionLeft;
        }
        let cleanup = self.paste_collapse(foreground.process_id);
        self.diagnostic(
            "paste",
            format!("sequence={input_sequence} phase=cleanup result={cleanup:?}"),
        );
        match cleanup {
            SelectionCleanup::Collapsed => outcome,
            SelectionCleanup::Unverified => TextEditAttempt::SelectionLeft,
        }
    }

    /// Decides what a finished paste attempt means for the conversion, on both routes. `Some`
    /// carries the strategy to record when the text edit was made. A confirmed edit whose clipboard
    /// could not be restored still counts as success (policy D2): it is logged and counted, and
    /// automatic conversion stays on. Nothing typed (`Cancelled`, `Unsupported`) is no failure;
    /// every failure after input reached the field is recorded with its own reason. The capability
    /// route also records the backend state it found.
    fn apply_paste_attempt_result(
        &mut self,
        attempt: TextEditAttempt,
        sequence: u64,
        route: PasteRoute,
    ) -> Option<EditStrategy> {
        if route == PasteRoute::CapabilityProbe {
            let status = match attempt {
                TextEditAttempt::Applied | TextEditAttempt::AppliedClipboardNotRestored => {
                    Some(BACKEND_PROTECTED_PASTE)
                }
                TextEditAttempt::Cancelled => Some(BACKEND_SELECTION_MISMATCH),
                TextEditAttempt::Unsupported => Some(BACKEND_SELECTION_UNSUPPORTED),
                TextEditAttempt::Failed(_) | TextEditAttempt::SelectionLeft => None,
            };
            if let Some(status) = status {
                self.metrics.backend_status.store(status, Ordering::Release);
            }
        }
        match attempt {
            TextEditAttempt::Applied => Some(EditStrategy::ProtectedPaste),
            TextEditAttempt::AppliedClipboardNotRestored => {
                self.note_clipboard_not_restored(sequence);
                Some(EditStrategy::ProtectedPaste)
            }
            TextEditAttempt::Cancelled | TextEditAttempt::Unsupported => {
                // Nothing was typed; the paste lines of the same sequence say at which step.
                self.diagnostic(
                    "conversion",
                    format!(
                        "sequence={sequence} result=cancelled strategy=ProtectedPaste edit=none unsupported={}",
                        attempt == TextEditAttempt::Unsupported
                    ),
                );
                None
            }
            TextEditAttempt::Failed(reason) => {
                self.record_conversion_failure(reason);
                None
            }
            TextEditAttempt::SelectionLeft => {
                // The field may still show the word selected: the keys the gate holds would type
                // over it, so they are kept for recovery instead of being replayed, and come back
                // only when the user asks for them.
                self.held_keys_kept_for = Some(sequence);
                self.metrics
                    .selection_left_token
                    .store(sequence, Ordering::Release);
                self.record_conversion_failure(ConversionFailureReason::SelectionLeft);
                None
            }
        }
    }

    fn note_clipboard_not_restored(&self, sequence: u64) {
        self.metrics
            .clipboard_restore_failures
            .fetch_add(1, Ordering::Relaxed);
        self.diagnostic(
            "paste",
            format!("sequence={sequence} clipboard_restored=false edit=confirmed"),
        );
    }

    fn perform_physical_edit(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
        target_layout: HKL,
        erase_characters: usize,
        replay_keys: &[ReplayKey],
        delimiter: Option<char>,
    ) -> ReplayAttempt {
        // Erasing and retyping may stop halfway: see perform_protected_paste.
        let _edit = BusyGuard::new(&self.metrics.edit_in_flight);
        if !self.edit_guard_is_current(foreground, input_sequence) {
            return ReplayAttempt::Cancelled;
        }
        let Some(caps_lock) = replay_keys.first().map(|key| key.caps_lock) else {
            return ReplayAttempt::Failed;
        };
        if replay_keys.iter().any(|key| key.caps_lock != caps_lock) {
            return ReplayAttempt::Failed;
        }
        let Some(plan) = build_physical_replay_plan(erase_characters, replay_keys, delimiter)
        else {
            return ReplayAttempt::Failed;
        };
        let mode = if self.text_edit_backend.requires_text_barrier() {
            PhysicalReplayMode::Paced
        } else {
            PhysicalReplayMode::Batch
        };
        if mode == PhysicalReplayMode::Paced && !paced_replay_fits_gate(plan.step_ends.len()) {
            return ReplayAttempt::Failed;
        }
        let pacer = (mode == PhysicalReplayMode::Paced)
            .then(ReplayPacer::new)
            .flatten();
        match self.switch_layout_and_wait(foreground, target_layout, input_sequence) {
            ReplayAttempt::Applied => {}
            ReplayAttempt::Cancelled => return ReplayAttempt::Cancelled,
            ReplayAttempt::Failed | ReplayAttempt::LayoutSwitchFailed => {
                return ReplayAttempt::LayoutSwitchFailed;
            }
        }
        if !self.edit_guard_is_current(foreground, input_sequence) {
            return ReplayAttempt::Failed;
        }
        match dispatch_physical_replay_plan(
            &plan,
            mode,
            || self.edit_guard_is_current(foreground, input_sequence),
            send_input_batch,
            || {
                if let Some(pacer) = &pacer {
                    pacer.wait();
                } else {
                    thread::sleep(Duration::from_millis(NOTEPAD_REPLAY_STEP_DELAY_MS));
                }
            },
        ) {
            // The target HKL has already been established. A guard loss from
            // this point is no longer a side-effect-free cancellation.
            ReplayAttempt::Cancelled => ReplayAttempt::Failed,
            result => result,
        }
    }

    fn switch_layout_and_wait(
        &self,
        foreground: ForegroundContext,
        target_layout: HKL,
        input_sequence: u64,
    ) -> ReplayAttempt {
        let target = target_layout.0 as usize;
        if !self.edit_guard_is_current(foreground, input_sequence) {
            return ReplayAttempt::Cancelled;
        }
        if current_foreground_context().is_some_and(|current| current.layout == target) {
            return ReplayAttempt::Applied;
        }
        if !post_layout_switch(foreground, target_layout) {
            return ReplayAttempt::Failed;
        }

        let deadline = Instant::now() + Duration::from_millis(LAYOUT_SWITCH_TIMEOUT_MS);
        let mut target_seen_at = None;
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
            if !self.edit_guard_is_current(foreground, input_sequence) {
                return ReplayAttempt::Failed;
            }
            if current_foreground_context().is_some_and(|current| current.layout == target) {
                let seen_at = target_seen_at.get_or_insert_with(Instant::now);
                if seen_at.elapsed() >= Duration::from_millis(LAYOUT_STABLE_MS) {
                    return ReplayAttempt::Applied;
                }
            } else {
                target_seen_at = None;
            }
        }
        ReplayAttempt::Failed
    }

    fn arm_correction_gate(&self, token: u64) -> bool {
        if token == 0 || self.gate_window == 0 {
            return false;
        }
        if self.metrics.active_gate_token.load(Ordering::Acquire) == token
            && self.metrics.cancelled_gate_token.load(Ordering::Acquire) != token
        {
            return true;
        }
        let mut message_result = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                HWND(self.gate_window as *mut c_void),
                GATE_ARM_MESSAGE,
                WPARAM(token as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                100,
                Some(&mut message_result),
            )
        };
        let armed = sent.0 != 0 && message_result == 1;
        if armed {
            self.edit_deadline.set(Some(
                Instant::now() + Duration::from_millis(CORRECTION_GATE_MAX_HOLD_MS),
            ));
        }
        armed
    }

    fn handoff_correction_gate(&self, old_token: u64, new_token: u64, required_ms: u64) -> bool {
        self.prepare_gate_handoff_budget(old_token, required_ms)
            && self.send_gate_drain_ack(old_token, new_token) == GateAckOutcome::Accepted
    }

    fn prepare_gate_handoff_budget(&self, old_token: u64, required_ms: u64) -> bool {
        if old_token == 0 || required_ms == 0 || self.gate_window == 0 {
            return false;
        }
        let Ok(required_ms) = isize::try_from(required_ms) else {
            return false;
        };
        let mut message_result = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                HWND(self.gate_window as *mut c_void),
                GATE_HANDOFF_BUDGET_MESSAGE,
                WPARAM(old_token as usize),
                LPARAM(required_ms),
                SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                100,
                Some(&mut message_result),
            )
        };
        sent.0 != 0 && message_result == 1
    }

    fn acknowledge_correction_gate_drain(&self, token: u64) {
        if fail_open_after_ack(self.send_gate_drain_ack(token, 0)) && self.gate_window != 0 {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.gate_window as *mut c_void)),
                    GATE_FAIL_OPEN_MESSAGE,
                    WPARAM(token as usize),
                    LPARAM(0),
                );
            }
        }
    }

    fn send_gate_drain_ack(&self, old_token: u64, new_token: u64) -> GateAckOutcome {
        if old_token == 0 || self.gate_window == 0 {
            return GateAckOutcome::Refused;
        }
        let Ok(new_token) = isize::try_from(new_token) else {
            return GateAckOutcome::Refused;
        };
        let mut message_result = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                HWND(self.gate_window as *mut c_void),
                GATE_DRAIN_ACK_MESSAGE,
                WPARAM(old_token as usize),
                LPARAM(new_token),
                SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                100,
                Some(&mut message_result),
            )
        };
        let timed_out = sent.0 == 0 && unsafe { GetLastError() } == ERROR_TIMEOUT;
        gate_ack_outcome(sent.0 != 0, message_result, timed_out)
    }

    fn request_gate_release(&mut self, token: u64) {
        if token == 0 {
            return;
        }
        // Recorded before the window check so that tests, which run without a window, see it.
        self.last_gate_release_token = token;
        self.metrics
            .gate_release_requests
            .fetch_add(1, Ordering::Relaxed);
        if self.gate_window == 0 {
            return;
        }
        let posted = unsafe {
            PostMessageW(
                Some(HWND(self.gate_window as *mut c_void)),
                GATE_RELEASE_MESSAGE,
                WPARAM(token as usize),
                LPARAM(0),
            )
            .is_ok()
        };
        if !posted {
            self.record_conversion_failure(ConversionFailureReason::GateDrain);
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.gate_window as *mut c_void)),
                    GATE_FAIL_OPEN_MESSAGE,
                    WPARAM(token as usize),
                    LPARAM(0),
                );
            }
        }
    }

    /// Asks the UI thread to end the gate `token` keeping its held keys for recovery. It counts
    /// as the gate's release, so `process` does not release it as well.
    fn request_gate_retention(&mut self, token: u64) {
        if token == 0 {
            return;
        }
        // Recorded before the window check so that tests, which run without a window, see it.
        self.last_gate_release_token = token;
        self.last_gate_retention_token = token;
        if self.gate_window == 0 {
            return;
        }
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.gate_window as *mut c_void)),
                GATE_FAIL_OPEN_MESSAGE,
                WPARAM(token as usize),
                LPARAM(GATE_FAIL_OPEN_SELECTION_LEFT),
            );
        }
    }

    fn edit_guard_is_current(&self, foreground: ForegroundContext, input_sequence: u64) -> bool {
        self.input_guard_is_current(input_sequence) && self.foreground_matches(foreground)
    }

    fn pre_forward_guard_is_current(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
    ) -> bool {
        self.pre_forward_input_guard_is_current(input_sequence)
            && self.foreground_matches(foreground)
    }

    fn pre_forward_input_guard_is_current(&self, input_sequence: u64) -> bool {
        if self.metrics.cancelled_gate_token.load(Ordering::Acquire) == input_sequence
            || self.metrics.input_epoch.load(Ordering::Acquire) != self.last_input_epoch
        {
            return false;
        }
        let active_gate = self.metrics.active_gate_token.load(Ordering::Acquire);
        if active_gate == input_sequence {
            self.metrics.cancelled_gate_token.load(Ordering::Acquire) != input_sequence
        } else {
            self.input_sequence_is_current(input_sequence) && physical_modifiers_released()
        }
    }

    fn input_guard_is_current(&self, input_sequence: u64) -> bool {
        if self.metrics.cancelled_gate_token.load(Ordering::Acquire) == input_sequence
            || self.metrics.input_epoch.load(Ordering::Acquire) != self.last_input_epoch
        {
            return false;
        }
        let active_gate = self.metrics.active_gate_token.load(Ordering::Acquire);
        if active_gate == input_sequence {
            self.metrics.cancelled_gate_token.load(Ordering::Acquire) != input_sequence
                && self
                    .metrics
                    .forwarded_input_sequence
                    .load(Ordering::Acquire)
                    >= input_sequence
        } else {
            self.input_sequence_is_current(input_sequence) && physical_modifiers_released()
        }
    }

    fn clear_undo(&mut self) {
        self.undo_record = None;
        self.metrics.undo_available.store(false, Ordering::Release);
    }

    fn commit_confirmed_layout(&mut self, layout: usize) {
        self.last_layout = Some(layout);
        self.layout_switch_in_flight = None;
    }

    fn invalidate_conversion_state(&mut self) {
        self.pending_conversion = None;
        self.deferred_drained_conversion = None;
        self.last_boundary = None;
        self.transpose_cycle = None;
        self.clear_undo();
    }

    fn input_sequence_is_current(&self, expected: u64) -> bool {
        self.metrics.observed_input_sequence.load(Ordering::Acquire) == expected
    }

    /// A Pause conversion or undo failed before any text input was sent. The
    /// user is watching and can press Pause again; unlike an automatic edit
    /// failure, this says nothing about text integrity, so automatic
    /// conversion stays enabled.
    fn record_manual_edit_failure(&mut self, reason: &str) {
        self.diagnostic(
            "conversion",
            format!("result=failed reason={reason} manual=true edit=none automatic=kept"),
        );
        self.invalidate_conversion_state();
        self.suppress_session();
        self.layout_switch_in_flight = None;
    }

    fn record_conversion_failure(&mut self, reason: ConversionFailureReason) {
        self.diagnostic("conversion", format!("result=failed reason={reason:?}"));
        // Remembered even when automatic conversion is already off: an edit that failed because a
        // gate was aborted must keep that abort from resuming automatic conversion.
        self.failed_edit_gate_token = self.metrics.cancelled_gate_token.load(Ordering::Acquire);
        self.invalidate_conversion_state();
        // What the failed edit left in the document is uncertain, so the recent
        // words no longer describe it.
        self.recent_languages.clear();
        self.suppress_session();
        self.layout_switch_in_flight = None;
        if !self.metrics.auto_enabled.swap(false, Ordering::AcqRel) {
            return;
        }
        self.metrics
            .last_failure_reason
            .store(reason as u8, Ordering::Release);
        self.metrics.safety_paused.store(true, Ordering::Release);
        self.last_input_epoch = self
            .metrics
            .input_epoch
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.metrics
            .conversion_failures
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// What a synchronous gate acknowledgement achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateAckOutcome {
    Accepted,
    Refused,
    /// Not answered in time. The message can still be handled later, so this is no refusal.
    Unanswered,
}

/// Classifies a SendMessageTimeoutW acknowledgement: `returned` is its non-zero return,
/// `timed_out` a zero return with ERROR_TIMEOUT.
const fn gate_ack_outcome(
    returned: bool,
    message_result: usize,
    timed_out: bool,
) -> GateAckOutcome {
    if returned {
        if message_result == 1 {
            GateAckOutcome::Accepted
        } else {
            GateAckOutcome::Refused
        }
    } else if timed_out {
        GateAckOutcome::Unanswered
    } else {
        GateAckOutcome::Refused
    }
}

/// Only a refused acknowledgement fails the gate open. An unanswered one may still be handled,
/// and a fail-open queued behind it would break a gate that is healthy by then; the gate's
/// deadline ends it if it is never handled.
const fn fail_open_after_ack(outcome: GateAckOutcome) -> bool {
    matches!(outcome, GateAckOutcome::Refused)
}

/// Diagnostics that describe typing in the current input target. None of them is written while
/// privacy blocks that target (a password field, or an excluded, elevated or uninspectable
/// program or field).
fn describes_typing(event: &str) -> bool {
    matches!(
        event,
        "input"
            | "input_queue"
            | "word"
            | "word_discard"
            | "candidate"
            | "hotkey"
            | "privacy_probe"
    )
}

/// What a privacy probe's result means for the field: the provider's answer, or for every
/// transport failure that the field cannot be inspected, with the failure as the source.
const fn probe_verdict(
    result: core::result::Result<Option<PrivacyBlockReason>, ProbeFailure>,
) -> (Option<PrivacyBlockReason>, &'static str) {
    match result {
        Ok(reason) => (reason, "provider"),
        Err(failure) => (
            Some(PrivacyBlockReason::InspectionUnavailable),
            match failure {
                ProbeFailure::Timeout => "wait-timeout",
                ProbeFailure::Busy => "queue-busy",
                ProbeFailure::Disconnected => "provider-disconnected",
                ProbeFailure::Expired => "reply-expired",
            },
        ),
    }
}

/// What a word-discard line adds to its reason: flags and fixed labels only, so no text can
/// reach the log through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordDiscardDetail {
    None,
    /// Which parts of the input target changed (handles are opaque identities).
    Focus {
        window_changed: bool,
        focus_changed: bool,
        process_changed: bool,
        focus_was_null: bool,
        focus_is_null: bool,
    },
    /// An input epoch change, and whether the input queue had overflowed.
    Epoch {
        overflow: bool,
    },
    /// The category of the input event that ended the word.
    Rule(&'static str),
}

impl WordDiscardDetail {
    fn describe(self) -> String {
        match self {
            Self::None => String::new(),
            Self::Focus {
                window_changed,
                focus_changed,
                process_changed,
                focus_was_null,
                focus_is_null,
            } => format!(
                "window_changed={window_changed} focus_changed={focus_changed} process_changed={process_changed} focus_was_null={focus_was_null} focus_is_null={focus_is_null}"
            ),
            Self::Epoch { overflow } => format!("overflow={overflow}"),
            Self::Rule(rule) => format!("rule={rule}"),
        }
    }
}

/// Keys that type nothing but can move the focus within one window: Esc, the function keys and
/// the menu key. Tab, Enter and shortcuts with Ctrl, Alt or Win are handled on their own paths.
fn may_move_focus_within_window(virtual_key: u16) -> bool {
    virtual_key == VK_ESCAPE.0
        || virtual_key == VK_APPS.0
        || (VK_F1.0..=VK_F24.0).contains(&virtual_key)
}

fn foreground_identity_matches(expected: ForegroundContext) -> bool {
    current_foreground_context().is_some_and(|current| same_input_target(current, expected))
}

/// The order in which the hotkey walks a word through the layouts: the language it was typed
/// in, then the languages from `ranked` (the most plausible reading first), then any other
/// enabled language in its configured order. The first press goes to the second entry, so a
/// word typed in English reaches Russian first when it reads as Russian, although Estonian
/// sorts first; the last press returns to the original.
fn layout_cycle_order(
    source: Language,
    enabled: &[Language],
    ranked: &[Language],
) -> Vec<Language> {
    let mut order = Vec::with_capacity(enabled.len());
    order.push(source);
    for language in ranked.iter().chain(enabled) {
        if !order.contains(language) {
            order.push(*language);
        }
    }
    order
}

fn same_input_target(first: ForegroundContext, second: ForegroundContext) -> bool {
    foreground_identity_key(first) == foreground_identity_key(second)
        && first.input_thread_id == second.input_thread_id
}

const fn foreground_identity_key(context: ForegroundContext) -> (usize, usize, u32) {
    (context.hwnd, context.focus, context.process_id)
}

fn physical_modifiers_released() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .all(|key| unsafe { GetAsyncKeyState(key.0 as i32) as u16 & 0x8000 == 0 })
}

fn shortcut_modifiers_released() -> bool {
    shortcut_modifiers_released_by(|key| unsafe {
        GetAsyncKeyState(i32::from(key.0)) as u16 & 0x8000 != 0
    })
}

/// No Ctrl, Alt or Windows key is down, as `held` reports each one. With one of them a Space is a
/// shortcut (Win+Space switches the layout), not the end of a word.
fn shortcut_modifiers_released_by(held: impl Fn(VIRTUAL_KEY) -> bool) -> bool {
    [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .all(|key| !held(key))
}

/// The modifier keys of `RawKeyEvent::held_modifiers`, one bit each from bit 0: left and right
/// Shift, Ctrl, Alt and Windows, every pair in the layout of the `Modifiers` masks.
const PHYSICAL_MODIFIER_KEYS: [VIRTUAL_KEY; 8] = [
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
];

/// The modifier keys physically held once this key edge is applied. In a low-level hook the
/// asynchronous key state does not show the edge itself yet.
fn held_modifiers_for_event(virtual_key: u16, key_down: bool, key_up: bool) -> u8 {
    let mut held = 0;
    for (bit, key) in PHYSICAL_MODIFIER_KEYS.into_iter().enumerate() {
        let pressed = if key.0 == virtual_key {
            key_down && !key_up
        } else {
            unsafe { GetAsyncKeyState(i32::from(key.0)) as u16 & 0x8000 != 0 }
        };
        if pressed {
            held |= 1 << bit;
        }
    }
    held
}

fn pressed_hotkey_modifiers_for_event(virtual_key: u16, key_down: bool, key_up: bool) -> u8 {
    let mut modifiers = 0;
    for (key, bit) in [
        (VK_CONTROL, HOTKEY_MOD_CONTROL),
        (VK_MENU, HOTKEY_MOD_ALT),
        (VK_SHIFT, HOTKEY_MOD_SHIFT),
    ] {
        if unsafe { GetAsyncKeyState(key.0 as i32) as u16 & 0x8000 != 0 } {
            modifiers |= bit;
        }
    }
    if [VK_LWIN, VK_RWIN]
        .into_iter()
        .any(|key| unsafe { GetAsyncKeyState(key.0 as i32) as u16 & 0x8000 != 0 })
    {
        modifiers |= HOTKEY_MOD_WIN;
    }
    if let Some(bit) = hotkey_modifier_bit(virtual_key) {
        if key_down {
            modifiers |= bit;
        } else if key_up {
            let other_key = match virtual_key {
                key if key == VK_LCONTROL.0 => Some(VK_RCONTROL),
                key if key == VK_RCONTROL.0 => Some(VK_LCONTROL),
                key if key == VK_LMENU.0 => Some(VK_RMENU),
                key if key == VK_RMENU.0 => Some(VK_LMENU),
                key if key == VK_LSHIFT.0 => Some(VK_RSHIFT),
                key if key == VK_RSHIFT.0 => Some(VK_LSHIFT),
                key if key == VK_LWIN.0 => Some(VK_RWIN),
                key if key == VK_RWIN.0 => Some(VK_LWIN),
                _ => None,
            };
            if !other_key
                .is_some_and(|key| unsafe { GetAsyncKeyState(key.0 as i32) as u16 & 0x8000 != 0 })
            {
                modifiers &= !bit;
            }
        }
    }
    modifiers
}

const fn hotkey_modifier_bit(virtual_key: u16) -> Option<u8> {
    match virtual_key {
        key if key == VK_CONTROL.0 || key == VK_LCONTROL.0 || key == VK_RCONTROL.0 => {
            Some(HOTKEY_MOD_CONTROL)
        }
        key if key == VK_MENU.0 || key == VK_LMENU.0 || key == VK_RMENU.0 => Some(HOTKEY_MOD_ALT),
        key if key == VK_SHIFT.0 || key == VK_LSHIFT.0 || key == VK_RSHIFT.0 => {
            Some(HOTKEY_MOD_SHIFT)
        }
        key if key == VK_LWIN.0 || key == VK_RWIN.0 => Some(HOTKEY_MOD_WIN),
        _ => None,
    }
}

fn contextual_hotkey_action(
    metrics: &ObserverMetrics,
    message: u32,
    virtual_key: u16,
    modifiers: u8,
) -> UndoHotkeyAction {
    if metrics.configuration_pending.load(Ordering::Acquire) {
        let configured_key = metrics.force_hotkey_virtual_key.load(Ordering::Acquire) as u16;
        let configured = Hotkey {
            virtual_key: configured_key,
            modifiers: 0,
        };
        if configured.matches_key(virtual_key) && metrics.undo_hotkey_active.load(Ordering::Acquire)
        {
            if matches!(message, WM_KEYUP | WM_SYSKEYUP) {
                metrics.undo_hotkey_active.store(false, Ordering::Release);
                metrics
                    .hotkey_waiting_for_release
                    .store(false, Ordering::Release);
            }
            return UndoHotkeyAction::Swallow;
        }
        return UndoHotkeyAction::PassThrough;
    }
    if !metrics.pause_break_undo.load(Ordering::Acquire)
        && !metrics.recovery_armed.load(Ordering::Acquire)
        && !metrics.undo_hotkey_active.load(Ordering::Acquire)
    {
        metrics.undo_hotkey_active.store(false, Ordering::Release);
        metrics
            .hotkey_waiting_for_release
            .store(false, Ordering::Release);
        return UndoHotkeyAction::PassThrough;
    }
    let key_down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
    let key_up = matches!(message, WM_KEYUP | WM_SYSKEYUP);
    if !key_down && !key_up {
        return UndoHotkeyAction::PassThrough;
    }
    let configured_key = metrics.force_hotkey_virtual_key.load(Ordering::Acquire) as u16;
    let configured_modifiers = metrics.force_hotkey_modifiers.load(Ordering::Acquire);
    let configured_hotkey = Hotkey {
        virtual_key: configured_key,
        modifiers: configured_modifiers,
    };

    if configured_hotkey.matches_key(virtual_key) {
        if key_down {
            let already_down = metrics.undo_hotkey_active.swap(true, Ordering::AcqRel);
            // Pause can arrive without a key-up. Once its previous action no
            // longer awaits modifiers, a new make is a new pulse. Keep the
            // up-swallow state intact for a possible delayed release.
            let fresh_pause_pulse = configured_key == 0x13
                && !metrics.hotkey_waiting_for_release.load(Ordering::Acquire);
            if already_down && !fresh_pause_pulse {
                return UndoHotkeyAction::Swallow;
            }
            if modifiers != configured_modifiers {
                metrics.undo_hotkey_active.store(false, Ordering::Release);
                return UndoHotkeyAction::PassThrough;
            }
            if configured_modifiers == 0 {
                return UndoHotkeyAction::TriggerSwallow;
            }
            metrics
                .hotkey_waiting_for_release
                .store(true, Ordering::Release);
            return UndoHotkeyAction::Swallow;
        }
        if metrics.undo_hotkey_active.swap(false, Ordering::AcqRel) {
            if modifiers == 0
                && metrics
                    .hotkey_waiting_for_release
                    .swap(false, Ordering::AcqRel)
            {
                return UndoHotkeyAction::TriggerSwallow;
            }
            return UndoHotkeyAction::Swallow;
        }
    }

    if metrics.hotkey_waiting_for_release.load(Ordering::Acquire) {
        if key_down && hotkey_modifier_bit(virtual_key).is_none() {
            metrics
                .hotkey_waiting_for_release
                .store(false, Ordering::Release);
        } else if key_up
            && modifiers == 0
            && hotkey_modifier_bit(virtual_key).is_some()
            && metrics
                .hotkey_waiting_for_release
                .swap(false, Ordering::AcqRel)
        {
            return UndoHotkeyAction::TriggerPassThrough;
        }
    }
    UndoHotkeyAction::PassThrough
}

fn gate_request_enabled(metrics: &ObserverMetrics, token: u64) -> bool {
    !metrics.configuration_pending.load(Ordering::Acquire)
        && (metrics.auto_enabled.load(Ordering::Acquire)
            || (token != 0
                && metrics.pause_break_undo.load(Ordering::Acquire)
                && metrics.explicit_hotkey_sequence.load(Ordering::Acquire) == token))
}

const fn should_arm_space_decision_gate(
    drained: bool,
    key_down: bool,
    virtual_key: u16,
    auto_enabled: bool,
    privacy_reason: u8,
    gate_active: bool,
    shortcut_modifiers_released: bool,
) -> bool {
    !drained
        && key_down
        && virtual_key == VK_SPACE.0
        && auto_enabled
        && privacy_reason == PRIVACY_ALLOWED
        && !gate_active
        && shortcut_modifiers_released
}

fn post_layout_switch(expected: ForegroundContext, target_layout: HKL) -> bool {
    let Some(current) = current_foreground_context() else {
        return false;
    };
    if current.hwnd != expected.hwnd || current.process_id != expected.process_id {
        return false;
    }
    if current.layout == target_layout.0 as usize {
        return true;
    }
    unsafe {
        PostMessageW(
            Some(focused_window_for_context(expected)),
            WM_INPUTLANGCHANGEREQUEST,
            WPARAM(0),
            LPARAM(target_layout.0 as isize),
        )
        .is_ok()
    }
}

fn replay_key_from_event(event: RawKeyEvent, modifiers: Modifiers) -> Option<ReplayKey> {
    let scan_code = u16::try_from(event.scan_code).ok()?;
    if scan_code == 0 {
        return None;
    }
    Some(ReplayKey {
        scan_code,
        shift: modifiers.shift(),
        caps_lock: event.caps_lock,
        extended: event.extended,
    })
}

fn map_replay_keys_to_layout(replay_keys: &[ReplayKey], layout: HKL) -> Option<String> {
    if replay_keys.is_empty() {
        return None;
    }
    replay_keys
        .iter()
        .map(|key| map_replay_key_to_layout(*key, layout))
        .collect()
}

/// The keypad meaning of a numeric keypad scan code. The scan code alone also names a navigation
/// key (keypad 1 is End), but only the digit can be part of a word or token: with Num Lock off, or
/// with Shift, the key navigates and never reaches the replay keys.
const fn keypad_digit_key(scan_code: u16) -> Option<u16> {
    let digit = match scan_code {
        0x52 => 0,
        0x4f => 1,
        0x50 => 2,
        0x51 => 3,
        0x4b => 4,
        0x4c => 5,
        0x4d => 6,
        0x47 => 7,
        0x48 => 8,
        0x49 => 9,
        0x53 => return Some(VK_DECIMAL.0),
        _ => return None,
    };
    Some(VK_NUMPAD0.0 + digit)
}

fn map_replay_key_to_layout(key: ReplayKey, layout: HKL) -> Option<char> {
    let mapping_scan_code = u32::from(key.scan_code) | if key.extended { 0xE000 } else { 0 };
    let keypad = if key.extended {
        None
    } else {
        keypad_digit_key(key.scan_code)
    };
    let virtual_key = match keypad {
        Some(keypad) => u32::from(keypad),
        None => unsafe { MapVirtualKeyExW(mapping_scan_code, MAPVK_VSC_TO_VK_EX, Some(layout)) },
    };
    if virtual_key == 0 {
        return None;
    }

    let mut keyboard_state = [0_u8; 256];
    set_pressed(&mut keyboard_state, VK_SHIFT.0 as usize, key.shift);
    keyboard_state[VK_CAPITAL.0 as usize] = u8::from(key.caps_lock);
    let mut output = [0_u16; 8];
    let count = unsafe {
        ToUnicodeEx(
            virtual_key,
            u32::from(key.scan_code),
            &keyboard_state,
            &mut output,
            TO_UNICODE_DO_NOT_CHANGE_KEYBOARD_STATE,
            Some(layout),
        )
    };
    if count != 1 {
        return None;
    }
    char::decode_utf16(output[..1].iter().copied()).next()?.ok()
}

fn mapped_layout_candidates_with(
    current_language: Language,
    settings: &Settings,
    detector: &Detector,
    mut map_target: impl FnMut(Language) -> Option<String>,
) -> Vec<(Language, String)> {
    detector
        .automatic_targets(current_language)
        .filter(|&target_language| settings.language_enabled(target_language))
        .filter_map(|target_language| {
            let replacement = map_target(target_language)?;
            Some((target_language, replacement))
        })
        .collect()
}

fn send_input_batch(inputs: &[INPUT]) -> bool {
    if inputs.is_empty() {
        return false;
    }
    send_input_count(inputs) == inputs.len()
}

fn send_input_count(inputs: &[INPUT]) -> usize {
    if inputs.is_empty() {
        return 0;
    }
    unsafe { SendInput(inputs, size_of::<INPUT>() as i32) as usize }
}

fn build_paste_inputs() -> [INPUT; 4] {
    [
        keyboard_input(VK_LCONTROL, 0, Default::default()),
        keyboard_input(VK_V_KEY, 0, Default::default()),
        keyboard_input(VK_V_KEY, 0, KEYEVENTF_KEYUP),
        keyboard_input(VK_LCONTROL, 0, KEYEVENTF_KEYUP),
    ]
}

fn build_physical_replay_plan(
    erase_characters: usize,
    replay_keys: &[ReplayKey],
    delimiter: Option<char>,
) -> Option<PhysicalReplayPlan> {
    if erase_characters > 128
        || replay_keys.is_empty()
        || replay_keys.len() > 128
        || delimiter.is_some_and(|delimiter| delimiter != ' ')
    {
        return None;
    }

    let delimiter_steps = usize::from(delimiter.is_some());
    let mut inputs =
        Vec::with_capacity(erase_characters * 2 + replay_keys.len() * 4 + delimiter_steps * 2);
    let mut step_ends = Vec::with_capacity(erase_characters + replay_keys.len() + delimiter_steps);
    for _ in 0..erase_characters {
        inputs.push(keyboard_input(VK_BACK, 0, Default::default()));
        inputs.push(keyboard_input(VK_BACK, 0, KEYEVENTF_KEYUP));
        step_ends.push(inputs.len());
    }
    for key in replay_keys {
        // A keypad digit goes out as its virtual key. As a bare scan code it would arrive as the
        // navigation key that shares it (keypad 2 is Down), which recalls history or clears the
        // line in a terminal. Digits are never shifted: Shift turns keypad keys into navigation.
        let keypad = if key.extended {
            None
        } else {
            keypad_digit_key(key.scan_code)
        };
        if let Some(keypad) = keypad {
            inputs.push(keyboard_input(
                VIRTUAL_KEY(keypad),
                key.scan_code,
                Default::default(),
            ));
            inputs.push(keyboard_input(
                VIRTUAL_KEY(keypad),
                key.scan_code,
                KEYEVENTF_KEYUP,
            ));
            step_ends.push(inputs.len());
            continue;
        }
        if key.shift {
            inputs.push(keyboard_input(VK_LSHIFT, 0, Default::default()));
        }
        let scan_flags = KEYEVENTF_SCANCODE
            | if key.extended {
                KEYEVENTF_EXTENDEDKEY
            } else {
                Default::default()
            };
        inputs.push(keyboard_input(VIRTUAL_KEY(0), key.scan_code, scan_flags));
        inputs.push(keyboard_input(
            VIRTUAL_KEY(0),
            key.scan_code,
            scan_flags | KEYEVENTF_KEYUP,
        ));
        if key.shift {
            inputs.push(keyboard_input(VK_LSHIFT, 0, KEYEVENTF_KEYUP));
        }
        step_ends.push(inputs.len());
    }
    if delimiter.is_some() {
        inputs.push(keyboard_input(VK_SPACE, 0, Default::default()));
        inputs.push(keyboard_input(VK_SPACE, 0, KEYEVENTF_KEYUP));
        step_ends.push(inputs.len());
    }

    Some(PhysicalReplayPlan { inputs, step_ends })
}

fn dispatch_physical_replay_plan<Guard, Send, Pace>(
    plan: &PhysicalReplayPlan,
    mode: PhysicalReplayMode,
    mut guard: Guard,
    mut send: Send,
    mut pace: Pace,
) -> ReplayAttempt
where
    Guard: FnMut() -> bool,
    Send: FnMut(&[INPUT]) -> bool,
    Pace: FnMut(),
{
    if plan.inputs.is_empty() || plan.step_ends.last().copied() != Some(plan.inputs.len()) {
        return ReplayAttempt::Failed;
    }
    if !guard() {
        return ReplayAttempt::Cancelled;
    }
    if mode == PhysicalReplayMode::Batch {
        return if send(&plan.inputs) {
            ReplayAttempt::Applied
        } else {
            ReplayAttempt::Failed
        };
    }

    let mut start = 0;
    for (index, end) in plan.step_ends.iter().copied().enumerate() {
        if end <= start || end > plan.inputs.len() {
            return ReplayAttempt::Failed;
        }
        if !guard() {
            return if start == 0 {
                ReplayAttempt::Cancelled
            } else {
                ReplayAttempt::Failed
            };
        }
        if !send(&plan.inputs[start..end]) {
            return ReplayAttempt::Failed;
        }
        start = end;
        if index + 1 < plan.step_ends.len() {
            pace();
        }
    }
    ReplayAttempt::Applied
}

fn paced_replay_fits_gate(step_count: usize) -> bool {
    conversion_gate_budget_ms(step_count, true, true)
        .is_some_and(|total| total <= CORRECTION_GATE_MAX_HOLD_MS)
}

/// The time a physical replay of `step_count` steps holds the correction gate: paced between text
/// barriers (Notepad) or sent at once.
fn conversion_gate_budget_ms(
    step_count: usize,
    paced: bool,
    include_forward_wait: bool,
) -> Option<u64> {
    let mut total = if include_forward_wait {
        HOOK_FORWARD_TIMEOUT_MS
    } else {
        0
    };
    if paced {
        let pacing_ms = u64::try_from(step_count.saturating_sub(1))
            .ok()?
            .checked_mul(NOTEPAD_REPLAY_STEP_DELAY_MS)?;
        total = total
            .checked_add(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS)?
            .checked_add(LAYOUT_SWITCH_TIMEOUT_MS)?
            .checked_add(pacing_ms)?
            .checked_add(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS)?;
    } else {
        total = total
            .checked_add(INPUT_SETTLE_AFTER_FORWARD_MS)?
            .checked_add(LAYOUT_SWITCH_TIMEOUT_MS)?
            .checked_add(REPLAY_COMMIT_MS)?;
    }
    total.checked_add(NOTEPAD_REPLAY_GATE_MARGIN_MS)
}

/// The time an automatic protected paste needs from the gate: its settle (and a capability
/// probe's text barrier), one confirmed selection, the least installation of the replacement and
/// everything after the Ctrl+V. The paste shortens its own waits to what the gate has left and
/// never sends the Ctrl+V without `PASTE_AFTER_SUBMIT_MS` in hand (`PasteBudget`).
const fn paste_gate_budget_ms(capability_probe: bool) -> u64 {
    let settle = if capability_probe {
        INPUT_SETTLE_AFTER_FORWARD_MS + NOTEPAD_TEXT_BARRIER_TIMEOUT_MS
    } else {
        NOTEPAD_TEXT_BARRIER_TIMEOUT_MS
    };
    settle + UIA_SELECTION_CONFIRM_TIMEOUT_MS + PASTE_MIN_INSTALL_MS + PASTE_AFTER_SUBMIT_MS
}

/// The steps of a protected paste that reach UI Automation, the clipboard, SendInput and the
/// keyboard layout, as tests script them.
#[cfg(test)]
trait PasteSeam {
    fn settle(&mut self) -> TextBarrierStatus;
    fn select(&mut self) -> (TextBarrierStatus, Option<SelectionCleanup>);
    fn begin<'a>(
        &mut self,
        text: &str,
        install_timeout: Duration,
        brokers: &'a ClipboardBrokers,
        target_process: u32,
    ) -> core::result::Result<ProtectedClipboard<'a>, ClipboardBeginFailure>;
    fn submit(&mut self, requested: usize) -> usize;
    fn commit(&mut self) -> ReplayAttempt;
    fn switch_layout(&mut self) -> ReplayAttempt;
    fn collapse(&mut self) -> SelectionCleanup;
}

/// The time a protected paste may take: until the gate holding the keys typed after the word lets
/// them go. Without such a gate (nothing is held) it is unbounded.
#[derive(Debug, Clone, Copy)]
struct PasteBudget {
    deadline: Option<Instant>,
}

impl PasteBudget {
    fn remaining(self, now: Instant) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(now))
    }

    /// Whether the selection may start: its confirmation, the least installation and everything
    /// after the Ctrl+V still fit.
    fn allows_selection(self, now: Instant, patience: PastePatience) -> bool {
        let needed = patience.confirm_timeout() * patience.attempts()
            + Duration::from_millis(PASTE_MIN_INSTALL_MS + PASTE_AFTER_SUBMIT_MS);
        self.remaining(now)
            .is_none_or(|remaining| remaining >= needed)
    }

    /// How long the installation of the replacement may take: the patience's limit, shortened so
    /// that everything after the Ctrl+V still fits; none when less than `PASTE_MIN_INSTALL_MS`
    /// would be left for it.
    fn install_timeout(self, now: Instant, patience: PastePatience) -> Option<Duration> {
        let limit = patience.clipboard_install_timeout();
        let Some(remaining) = self.remaining(now) else {
            return Some(limit);
        };
        let left = remaining.checked_sub(Duration::from_millis(PASTE_AFTER_SUBMIT_MS))?;
        (left >= Duration::from_millis(PASTE_MIN_INSTALL_MS)).then_some(left.min(limit))
    }

    /// Whether the Ctrl+V may be sent: everything after it fits before the gate lets the keys go.
    fn allows_submit(self, now: Instant) -> bool {
        self.remaining(now)
            .is_none_or(|remaining| remaining >= Duration::from_millis(PASTE_AFTER_SUBMIT_MS))
    }
}

/// What becomes of the user's clipboard once the Ctrl+V was sent: put back at once after the
/// field showed the replacement, otherwise only after the paste's target read the replacement. A
/// paste a busy field handles late then gets the replacement, never the user's own clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardAfterPaste {
    Restore,
    Defer,
}

const fn clipboard_after_paste(commit: ReplayAttempt) -> ClipboardAfterPaste {
    match commit {
        ReplayAttempt::Applied => ClipboardAfterPaste::Restore,
        ReplayAttempt::Cancelled | ReplayAttempt::Failed | ReplayAttempt::LayoutSwitchFailed => {
            ClipboardAfterPaste::Defer
        }
    }
}

/// Which conversion route made a protected paste: the protected-paste backend, or a capability
/// probe whose text barrier matched (it records the backend state it found as well).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PasteRoute {
    Backend,
    CapabilityProbe,
}

fn build_held_replay_inputs(held: &[RawKeyEvent], fence_marker: usize) -> Option<Vec<INPUT>> {
    if held.len() > CORRECTION_GATE_CAPACITY || fence_marker == 0 {
        return None;
    }
    let mut inputs = build_raw_replay_inputs(held, DRAINED_EVENT_MARKER)?;
    inputs.push(keyboard_input_with_marker(
        VK_F24,
        0,
        Default::default(),
        fence_marker,
    ));
    inputs.push(keyboard_input_with_marker(
        VK_F24,
        0,
        KEYEVENTF_KEYUP,
        fence_marker,
    ));
    Some(inputs)
}

fn build_recovery_inputs(held: &[RawKeyEvent]) -> Option<Vec<INPUT>> {
    if held.is_empty() || held.len() > CORRECTION_GATE_CAPACITY * 2 + 1 {
        return None;
    }
    build_balanced_replay_inputs(held)
}

/// Replays `held` and then releases every key it leaves down.
fn build_balanced_replay_inputs(held: &[RawKeyEvent]) -> Option<Vec<INPUT>> {
    let mut inputs = build_raw_replay_inputs(held, INJECTED_EVENT_MARKER)?;
    // Physical key-ups may have passed through after the gate was cancelled.
    // Balance every remaining down edge, so restoring cannot latch Win/Ctrl.
    let mut down = std::collections::BTreeMap::new();
    for event in held {
        let key = (event.virtual_key, event.scan_code, event.extended);
        if matches!(event.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
            down.insert(key, *event);
        } else {
            down.remove(&key);
        }
    }
    for mut event in down.into_values() {
        event.message = WM_KEYUP;
        inputs.extend(build_raw_replay_inputs(&[event], INJECTED_EVENT_MARKER)?);
    }
    Some(inputs)
}

/// Whether a virtual key is a keypad digit or the keypad decimal key.
fn is_keypad_digit_key(virtual_key: u32) -> bool {
    u16::try_from(virtual_key)
        .is_ok_and(|key| (VK_NUMPAD0.0..=VK_NUMPAD0.0 + 9).contains(&key) || key == VK_DECIMAL.0)
}

/// Pause and Num Lock: the hook reports both as scan code 0x45, so a replay by scan code alone
/// could produce the other key.
fn shares_scan_code_with_another_key(virtual_key: u32) -> bool {
    u16::try_from(virtual_key).is_ok_and(|key| key == VK_PAUSE.0 || key == VK_NUMLOCK.0)
}

fn build_raw_replay_inputs(held: &[RawKeyEvent], marker: usize) -> Option<Vec<INPUT>> {
    let mut inputs = Vec::with_capacity(held.len() + 2);
    for event in held {
        // A keypad digit goes out as the virtual key it arrived with; as a bare scan code it
        // would arrive as the navigation key that shares it (see build_physical_replay_plan).
        // Pause and Num Lock share scan code 0x45 as well, Pause without the E1 prefix the
        // hook cannot report.
        let by_virtual_key = event.scan_code == 0
            || is_keypad_digit_key(event.virtual_key)
            || shares_scan_code_with_another_key(event.virtual_key);
        let mut flags = if by_virtual_key {
            Default::default()
        } else {
            KEYEVENTF_SCANCODE
        };
        if event.extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if matches!(event.message, WM_KEYUP | WM_SYSKEYUP) {
            flags |= KEYEVENTF_KEYUP;
        } else if !matches!(event.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
            return None;
        }
        let virtual_key = if by_virtual_key {
            VIRTUAL_KEY(u16::try_from(event.virtual_key).ok()?)
        } else {
            VIRTUAL_KEY(0)
        };
        inputs.push(keyboard_input_with_marker(
            virtual_key,
            u16::try_from(event.scan_code).ok()?,
            flags,
            marker,
        ));
    }
    Some(inputs)
}

#[link(name = "winmm")]
unsafe extern "system" {
    fn PlaySoundW(sound: *const u16, module: *mut c_void, flags: u32) -> i32;
}

const SND_NODEFAULT: u32 = 0x0002;
const SND_MEMORY: u32 = 0x0004;

/// Starts the thread that plays conversion sounds, so a conversion never waits for audio.
fn start_sound_player() -> Option<SyncSender<SoundCue>> {
    let (sender, receiver) = sync_channel::<SoundCue>(1);
    thread::Builder::new()
        .name("autokey-sound".to_owned())
        .spawn(move || {
            let converted = conversion_sound::wav(SoundCue::Converted);
            let undone = conversion_sound::wav(SoundCue::Undone);
            while let Ok(cue) = receiver.recv() {
                let image = match cue {
                    SoundCue::Converted => &converted,
                    SoundCue::Undone => &undone,
                };
                // SAFETY: with SND_MEMORY the first argument is the WAV image itself, owned by
                // this thread and alive across the call. Without SND_ASYNC the call returns after
                // playback, so the image is not read afterwards. SND_NODEFAULT keeps Windows from
                // playing its default sound when the image cannot be played; nothing is played then.
                unsafe {
                    PlaySoundW(
                        image.as_ptr().cast(),
                        std::ptr::null_mut(),
                        SND_MEMORY | SND_NODEFAULT,
                    );
                }
            }
        })
        .ok()?;
    Some(sender)
}

fn keyboard_input(
    virtual_key: VIRTUAL_KEY,
    scan_code: u16,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
) -> INPUT {
    keyboard_input_with_marker(virtual_key, scan_code, flags, INJECTED_EVENT_MARKER)
}

fn keyboard_input_with_marker(
    virtual_key: VIRTUAL_KEY,
    scan_code: u16,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
    marker: usize,
) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: marker,
            },
        },
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Modifiers {
    shift_mask: u8,
    control_mask: u8,
    alt_mask: u8,
    win_mask: u8,
}

impl Modifiers {
    /// The modifiers the hook found held for an event (`RawKeyEvent::held_modifiers`).
    const fn from_held(held: u8) -> Self {
        Self {
            shift_mask: held & 0b11,
            control_mask: (held >> 2) & 0b11,
            alt_mask: (held >> 4) & 0b11,
            win_mask: (held >> 6) & 0b11,
        }
    }

    fn update(&mut self, virtual_key: u32, key_down: bool, key_up: bool) -> bool {
        let pressed = key_down && !key_up;
        match virtual_key as u16 {
            key if key == VK_SHIFT.0 || key == VK_LSHIFT.0 || key == VK_RSHIFT.0 => {
                update_modifier_mask(
                    &mut self.shift_mask,
                    key,
                    VK_SHIFT.0,
                    VK_LSHIFT.0,
                    VK_RSHIFT.0,
                    pressed,
                );
                true
            }
            key if key == VK_CONTROL.0 || key == VK_LCONTROL.0 || key == VK_RCONTROL.0 => {
                update_modifier_mask(
                    &mut self.control_mask,
                    key,
                    VK_CONTROL.0,
                    VK_LCONTROL.0,
                    VK_RCONTROL.0,
                    pressed,
                );
                true
            }
            key if key == VK_MENU.0 || key == VK_LMENU.0 || key == VK_RMENU.0 => {
                update_modifier_mask(
                    &mut self.alt_mask,
                    key,
                    VK_MENU.0,
                    VK_LMENU.0,
                    VK_RMENU.0,
                    pressed,
                );
                true
            }
            key if key == VK_LWIN.0 || key == VK_RWIN.0 => {
                let bit = if key == VK_LWIN.0 { 1 } else { 2 };
                set_modifier_bit(&mut self.win_mask, bit, pressed);
                true
            }
            _ => false,
        }
    }

    const fn shift(self) -> bool {
        self.shift_mask != 0
    }

    const fn control(self) -> bool {
        self.control_mask != 0
    }

    const fn alt(self) -> bool {
        self.alt_mask != 0
    }

    const fn has_shortcut_modifier(self) -> bool {
        self.control() || self.alt() || self.win_mask != 0
    }
}

/// Win+Space or Win+Shift+Space, the Windows chords that switch the keyboard layout.
fn switches_keyboard_layout(virtual_key: u32, modifiers: Modifiers) -> bool {
    virtual_key == u32::from(VK_SPACE.0)
        && modifiers.win_mask != 0
        && !modifiers.control()
        && !modifiers.alt()
}

/// Which kind of shortcut ended a word, for the diagnostics log: the editing command, never a
/// character the user typed.
fn shortcut_kind(virtual_key: u32, modifiers: Modifiers) -> &'static str {
    let control_only = modifiers.control() && !modifiers.alt();
    match u16::try_from(virtual_key).unwrap_or(0) {
        key if control_only && key == VK_BACK.0 => "erase-word",
        0x56 if control_only => "paste",
        0x43 if control_only => "copy",
        0x58 if control_only => "cut",
        0x5a | 0x59 if control_only => "undo",
        _ if control_only => "ctrl",
        _ if modifiers.alt() && !modifiers.control() => "alt",
        _ if modifiers.control() => "ctrl-alt",
        _ => "win",
    }
}

fn update_modifier_mask(
    mask: &mut u8,
    key: u16,
    generic: u16,
    left: u16,
    right: u16,
    pressed: bool,
) {
    let bit = if key == left {
        1
    } else if key == right {
        2
    } else if key == generic {
        4
    } else {
        return;
    };
    set_modifier_bit(mask, bit, pressed);
}

fn set_modifier_bit(mask: &mut u8, bit: u8, pressed: bool) {
    if pressed {
        *mask |= bit;
    } else {
        *mask &= !bit;
    }
}

/// What kind of key ended up unsupported, for the diagnostics log: one that prints nothing
/// (function, navigation, Caps Lock and dead keys) or one that prints a character a word cannot
/// contain (a digit, a symbol). Never the key or the character.
const fn unsupported_key_class(printable: Option<char>) -> &'static str {
    if printable.is_some() {
        "other-character"
    } else {
        "no-character"
    }
}

fn translate_printable(event: RawKeyEvent, modifiers: Modifiers) -> Option<char> {
    let mut keyboard_state = [0u8; 256];
    unsafe {
        let _ = GetKeyboardState(&mut keyboard_state);
    }
    set_pressed(&mut keyboard_state, event.virtual_key as usize, true);
    set_pressed(&mut keyboard_state, VK_SHIFT.0 as usize, modifiers.shift());
    set_pressed(
        &mut keyboard_state,
        VK_CONTROL.0 as usize,
        modifiers.control(),
    );
    set_pressed(&mut keyboard_state, VK_MENU.0 as usize, modifiers.alt());
    keyboard_state[VK_CAPITAL.0 as usize] = u8::from(event.caps_lock);

    let mut output = [0u16; 8];
    let count = unsafe {
        ToUnicodeEx(
            event.virtual_key,
            event.scan_code,
            &keyboard_state,
            &mut output,
            TO_UNICODE_DO_NOT_CHANGE_KEYBOARD_STATE,
            Some(HKL(event.foreground.layout as *mut c_void)),
        )
    };
    if count <= 0 {
        return None;
    }
    let units = usize::try_from(count).ok()?;
    let text = String::from_utf16(&output[..units]).ok()?;
    let mut characters = text.chars();
    let character = characters.next()?;
    characters.next().is_none().then_some(character)
}

fn set_pressed(keyboard_state: &mut [u8; 256], index: usize, pressed: bool) {
    if index < keyboard_state.len() {
        if pressed {
            keyboard_state[index] |= 0x80;
        } else {
            keyboard_state[index] &= 0x7f;
        }
    }
}

fn notify_icon_data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        hIcon: icon,
        ..Default::default()
    }
}

fn set_tooltip(data: &mut NOTIFYICONDATAW, text: &str) {
    set_wide_text(&mut data.szTip, text);
}

fn set_wide_text<const N: usize>(buffer: &mut [u16; N], text: &str) {
    buffer.fill(0);
    let maximum_units = buffer.len().saturating_sub(1);
    let mut written = 0;
    for character in text.chars() {
        let mut units = [0u16; 2];
        let encoded = character.encode_utf16(&mut units);
        if written + encoded.len() > maximum_units {
            break;
        }
        buffer[written..written + encoded.len()].copy_from_slice(encoded);
        written += encoded.len();
    }
}

fn show_tray_information(hwnd: HWND, title: impl AsRef<str>, message: impl AsRef<str>) {
    let icon = APP_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|state| state.tray.as_ref())
            .map(|tray| tray.icon)
    });
    let Some(icon) = icon else {
        return;
    };
    let mut data = notify_icon_data(hwnd, icon);
    data.uFlags = NIF_INFO | NIF_SHOWTIP;
    data.dwInfoFlags = NIIF_INFO;
    set_wide_text(&mut data.szInfoTitle, title.as_ref());
    set_wide_text(&mut data.szInfo, message.as_ref());
    data.Anonymous.uTimeout = 5_000;
    let _ = shell_notify_icon(NIM_MODIFY, &data);
}

const fn privacy_reason_code(reason: Option<PrivacyBlockReason>) -> u8 {
    match reason {
        None => PRIVACY_ALLOWED,
        Some(PrivacyBlockReason::PasswordField) => PRIVACY_PASSWORD,
        Some(PrivacyBlockReason::ExcludedProcess) => PRIVACY_EXCLUDED,
        Some(PrivacyBlockReason::ElevatedProcess) => PRIVACY_ELEVATED,
        Some(
            PrivacyBlockReason::InspectionUnavailable
            | PrivacyBlockReason::FieldInspectionUnavailable,
        ) => PRIVACY_UNAVAILABLE,
    }
}

/// The privacy finding a metric code stands for. Both kinds of unavailable inspection share one
/// code, read back as the process-wide one.
const fn privacy_reason_from_code(code: u8) -> Option<PrivacyBlockReason> {
    match code {
        PRIVACY_ALLOWED => None,
        PRIVACY_PASSWORD => Some(PrivacyBlockReason::PasswordField),
        PRIVACY_EXCLUDED => Some(PrivacyBlockReason::ExcludedProcess),
        PRIVACY_ELEVATED => Some(PrivacyBlockReason::ElevatedProcess),
        _ => Some(PrivacyBlockReason::InspectionUnavailable),
    }
}

const fn privacy_reason_label(reason: u8) -> Option<&'static str> {
    match reason {
        PRIVACY_PASSWORD => Some("password"),
        PRIVACY_EXCLUDED => Some("excluded"),
        PRIVACY_UNAVAILABLE => Some("unavailable"),
        PRIVACY_ELEVATED => Some("elevated"),
        _ => None,
    }
}

const fn conversion_failure_label(reason: u8) -> &'static str {
    match reason {
        value if value == ConversionFailureReason::SourceBarrier as u8 => "source-barrier",
        value if value == ConversionFailureReason::LayoutUnavailable as u8 => "layout-unavailable",
        value if value == ConversionFailureReason::PhysicalEdit as u8 => "physical-edit",
        value if value == ConversionFailureReason::TextCommit as u8 => "text-commit",
        value if value == ConversionFailureReason::ReplayCommit as u8 => "replay-commit",
        value if value == ConversionFailureReason::GateDrain as u8 => "gate-drain",
        value if value == ConversionFailureReason::ClipboardEdit as u8 => "clipboard-edit",
        value if value == ConversionFailureReason::InternalFault as u8 => "internal-fault",
        value if value == ConversionFailureReason::SelectionLeft as u8 => "selection-left",
        value if value == ConversionFailureReason::LayoutSwitch as u8 => "layout-switch",
        _ => "unknown",
    }
}

const fn backend_status_label(status: u8) -> &'static str {
    match status {
        BACKEND_PROTECTED_PASTE => "uia-paste",
        BACKEND_PHYSICAL_REPLAY => "physical-replay",
        BACKEND_CAPABILITY_PROBE => "capability-probe",
        BACKEND_CAPABILITY_UNSUPPORTED => "unsupported",
        BACKEND_CAPABILITY_MISMATCH => "text-mismatch",
        BACKEND_SELECTION_MISMATCH => "selection-mismatch",
        BACKEND_SELECTION_UNSUPPORTED => "selection-unsupported",
        _ => "observe-only",
    }
}

fn tray_visual_state(status: TrayStatus) -> TrayVisual {
    if status.auto_enabled {
        TrayVisual::Active
    } else if status.safety_paused {
        TrayVisual::SafetyPaused
    } else {
        TrayVisual::Disabled
    }
}

fn tray_tooltip(status: TrayStatus) -> String {
    let mode = match tray_visual_state(status) {
        TrayVisual::Active => tr("tray.active"),
        TrayVisual::Disabled => tr("tray.disabled"),
        TrayVisual::SafetyPaused => tr_format(
            "tray.safety_pause",
            &[("reason", conversion_failure_label(status.failure_reason))],
        ),
    };
    let mut tooltip = format!(
        "{mode} — {}; {}",
        status.indicator.tooltip_code(),
        tr_format(
            "tray.candidates",
            &[("count", &status.candidates.to_string())]
        )
    );
    if status.dropped > 0 {
        tooltip.push_str("; ");
        tooltip.push_str(&tr_format(
            "tray.dropped",
            &[("count", &status.dropped.to_string())],
        ));
    }
    if let Some(label) = privacy_reason_label(status.privacy_reason) {
        tooltip.push_str("; ");
        tooltip.push_str(&tr_format("tray.privacy_paused", &[("reason", label)]));
    }
    tooltip.push_str("; ");
    tooltip.push_str(&tr_format(
        "tray.backend",
        &[("backend", backend_status_label(status.backend_status))],
    ));
    if status.undo_available {
        tooltip.push_str("; ");
        tooltip.push_str(&tr("tray.undo_ready"));
    }
    if status.failures > 0 {
        tooltip.push_str("; ");
        tooltip.push_str(&tr_format(
            "tray.failures",
            &[
                ("count", &status.failures.to_string()),
                ("reason", conversion_failure_label(status.failure_reason)),
            ],
        ));
    }
    if status.diagnostic_errors > 0 {
        tooltip.push_str("; ");
        tooltip.push_str(&tr_format(
            "tray.diagnostic_errors",
            &[("count", &status.diagnostic_errors.to_string())],
        ));
    }
    tooltip
}

/// Diagnostics records that never reached the log, plus failed log rotations: shown in the tray
/// while diagnostics are on, so a log with gaps is not taken for a complete one.
fn diagnostic_error_count(metrics: &ObserverMetrics) -> u64 {
    metrics
        .dropped_diagnostics
        .load(Ordering::Relaxed)
        .saturating_add(metrics.diagnostic_log_errors.load(Ordering::Relaxed))
}

fn create_indicator_icon(visual: TrayVisual, indicator: LayoutIndicator) -> Result<HICON> {
    const SIZE: usize = 32;
    let pixels = tray_visual::pixels(visual, indicator.icon_text());

    let color_bitmap = unsafe {
        CreateBitmap(
            SIZE as i32,
            SIZE as i32,
            1,
            32,
            Some(pixels.as_ptr().cast()),
        )
    };
    if color_bitmap.0.is_null() {
        return Err(Error::from_thread());
    }

    // Preserve transparency for both alpha-aware and legacy mask consumers.
    let mut mask_bits = [0u8; SIZE * SIZE / 8];
    for (index, pixel) in pixels.iter().enumerate() {
        if pixel >> 24 == 0 {
            mask_bits[index / 8] |= 0x80 >> (index % 8);
        }
    }
    let mask_bitmap = unsafe {
        CreateBitmap(
            SIZE as i32,
            SIZE as i32,
            1,
            1,
            Some(mask_bits.as_ptr().cast()),
        )
    };
    if mask_bitmap.0.is_null() {
        return Err(last_error_then(|| unsafe {
            let _ = DeleteObject(HGDIOBJ(color_bitmap.0));
        }));
    }

    let icon_info = ICONINFO {
        fIcon: true.into(),
        hbmMask: mask_bitmap,
        hbmColor: color_bitmap,
        ..Default::default()
    };
    let icon = unsafe { CreateIconIndirect(&icon_info) };
    unsafe {
        let _ = DeleteObject(HGDIOBJ(mask_bitmap.0));
        let _ = DeleteObject(HGDIOBJ(color_bitmap.0));
    }
    icon
}

fn point_from_packed_word(value: WPARAM) -> POINT {
    let packed = value.0 as u32;
    POINT {
        x: (packed as u16 as i16) as i32,
        y: ((packed >> 16) as u16 as i16) as i32,
    }
}

fn show_tray_menu(hwnd: HWND, anchor: POINT) -> Result<()> {
    let entered = APP_STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        match state.as_mut() {
            Some(state) if !state.menu_open => {
                state.menu_open = true;
                true
            }
            _ => false,
        }
    });
    if !entered {
        return Ok(());
    }
    let result = show_tray_menu_inner(hwnd, anchor);
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            state.menu_open = false;
        }
    });
    refresh_tray_state();
    result
}

/// A popup menu, destroyed when it goes out of scope: on every return path.
struct PopupMenu(HMENU);

impl PopupMenu {
    fn new() -> Result<Self> {
        Ok(Self(unsafe { CreatePopupMenu() }?))
    }
}

impl Drop for PopupMenu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

fn show_tray_menu_inner(hwnd: HWND, anchor: POINT) -> Result<()> {
    unsafe {
        let popup = PopupMenu::new()?;
        let menu = popup.0;
        let snapshot = APP_STATE.with(|slot| {
            let state = slot.borrow();
            let state = state.as_ref()?;
            Some(TrayStatus {
                indicator: state.last_tray_status.indicator,
                ui_revision: ui_localization::revision(),
                candidates: state.metrics.candidates.load(Ordering::Relaxed),
                dropped: state.metrics.dropped_events.load(Ordering::Relaxed),
                privacy_reason: state.metrics.privacy_reason.load(Ordering::Acquire),
                auto_enabled: state.metrics.auto_enabled.load(Ordering::Acquire),
                safety_paused: state.metrics.safety_paused.load(Ordering::Acquire),
                undo_available: state.metrics.undo_available.load(Ordering::Acquire),
                failures: state.metrics.conversion_failures.load(Ordering::Relaxed),
                failure_reason: state.metrics.last_failure_reason.load(Ordering::Acquire),
                backend_status: state.metrics.backend_status.load(Ordering::Acquire),
                diagnostic_errors: diagnostic_error_count(&state.metrics),
            })
        });
        let Some(status) = snapshot else {
            return Ok(());
        };
        let auto_flags = MF_STRING
            | if status.auto_enabled {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        let auto_label = HSTRING::from(if status.auto_enabled {
            tr("menu.auto_on")
        } else {
            tr("menu.auto_off")
        });
        let (retained_count, worker_dead) = APP_STATE.with(|slot| {
            slot.borrow().as_ref().map_or((None, false), |state| {
                (
                    state
                        .metrics
                        .retained_input
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .as_ref()
                        .map(|input| input.events.len()),
                    state.metrics.worker_dead.load(Ordering::Acquire),
                )
            })
        });
        AppendMenuW(
            menu,
            auto_flags
                | if retained_count.is_some() {
                    MF_GRAYED
                } else {
                    MF_UNCHECKED
                },
            MENU_AUTO_ID,
            &auto_label,
        )?;
        if let Some(count) = retained_count {
            // A stopped worker cannot replay anything; only deleting is still possible.
            if !worker_dead {
                AppendMenuW(
                    menu,
                    MF_STRING,
                    MENU_RECOVER_INPUT_ID,
                    &HSTRING::from(tr_format(
                        "menu.recover_input",
                        &[("count", &count.to_string())],
                    )),
                )?;
            }
            AppendMenuW(
                menu,
                MF_STRING,
                MENU_DISCARD_INPUT_ID,
                &HSTRING::from(tr("menu.discard_input")),
            )?;
        }
        let lexicon_candidate = valid_lexicon_candidate();
        let recent_flags = MF_STRING
            | if lexicon_candidate.is_some() {
                MF_UNCHECKED
            } else {
                MF_GRAYED
            };
        let recent_label =
            HSTRING::from(match lexicon_candidate.map(|candidate| candidate.target) {
                Some(LexiconOfferTarget::UserDictionary) => tr("menu.dictionary_add"),
                Some(LexiconOfferTarget::WordExclusions) => tr("menu.exclude_word"),
                None => tr("menu.no_dictionary_offer"),
            });
        AppendMenuW(menu, recent_flags, MENU_ADD_LEXICON_ENTRY_ID, &recent_label)?;
        AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;

        AppendMenuW(
            menu,
            MF_STRING,
            MENU_SETTINGS_ID,
            &HSTRING::from(tr("menu.settings")),
        )?;
        AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
        AppendMenuW(
            menu,
            MF_STRING,
            MENU_EXIT_ID,
            &HSTRING::from(tr("menu.exit")),
        )?;

        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(
            menu,
            ui_localization::popup_menu_style(TPM_RIGHTBUTTON),
            anchor.x,
            anchor.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    pub(super) fn test_profiles() -> ResolvedKeyboardProfiles {
        use autokeyboardlayot::profile_resolver::{
            KeyboardProfileProbe, resolve_keyboard_profiles,
        };
        struct Fixture(usize);
        impl KeyboardProfileProbe for Fixture {
            fn loaded_layouts(&mut self) -> Option<Vec<usize>> {
                Some(vec![0x0409, 0x0419, 0x0425, 0xf0020409])
            }
            fn current_layout(&mut self) -> Option<usize> {
                Some(self.0)
            }
            fn is_ime(&mut self, _: usize) -> bool {
                false
            }
            fn activate_layout(&mut self, layout: usize) -> Option<usize> {
                Some(std::mem::replace(&mut self.0, layout))
            }
            fn current_layout_name(&mut self) -> Option<[u16; 9]> {
                let name = match self.0 {
                    0x0409 => "00000409",
                    0x0419 => "00000419",
                    0x0425 => "00000425",
                    0xf0020409 => "00020409",
                    _ => return None,
                };
                let mut result = [0; 9];
                for (slot, value) in result.iter_mut().zip(name.encode_utf16()) {
                    *slot = value;
                }
                Some(result)
            }
        }
        resolve_keyboard_profiles(&mut Fixture(0x0409)).unwrap()
    }

    /// What a key prints, by set-1 scan code, in the US (0x0409), Russian (0x0419) and Estonian
    /// (0x0425) layouts of `test_profiles()`. Shift or Caps Lock capitalize a letter; a shifted
    /// punctuation key prints nothing here.
    pub(super) fn test_keyboard(key: ReplayKey, layout: usize) -> Option<char> {
        const KEYS: [(u16, [char; 3]); 44] = [
            (0x02, ['1', '1', '1']),
            (0x03, ['2', '2', '2']),
            (0x04, ['3', '3', '3']),
            (0x05, ['4', '4', '4']),
            (0x06, ['5', '5', '5']),
            (0x07, ['6', '6', '6']),
            (0x08, ['7', '7', '7']),
            (0x09, ['8', '8', '8']),
            (0x0a, ['9', '9', '9']),
            (0x0b, ['0', '0', '0']),
            (0x10, ['q', 'й', 'q']),
            (0x11, ['w', 'ц', 'w']),
            (0x12, ['e', 'у', 'e']),
            (0x13, ['r', 'к', 'r']),
            (0x14, ['t', 'е', 't']),
            (0x15, ['y', 'н', 'y']),
            (0x16, ['u', 'г', 'u']),
            (0x17, ['i', 'ш', 'i']),
            (0x18, ['o', 'щ', 'o']),
            (0x19, ['p', 'з', 'p']),
            (0x1a, ['[', 'х', 'ü']),
            (0x1b, [']', 'ъ', 'õ']),
            (0x1e, ['a', 'ф', 'a']),
            (0x1f, ['s', 'ы', 's']),
            (0x20, ['d', 'в', 'd']),
            (0x21, ['f', 'а', 'f']),
            (0x22, ['g', 'п', 'g']),
            (0x23, ['h', 'р', 'h']),
            (0x24, ['j', 'о', 'j']),
            (0x25, ['k', 'л', 'k']),
            (0x26, ['l', 'д', 'l']),
            (0x27, [';', 'ж', 'ö']),
            (0x28, ['\'', 'э', 'ä']),
            (0x2c, ['z', 'я', 'z']),
            (0x2d, ['x', 'ч', 'x']),
            (0x2e, ['c', 'с', 'c']),
            (0x2f, ['v', 'м', 'v']),
            (0x30, ['b', 'и', 'b']),
            (0x31, ['n', 'т', 'n']),
            (0x32, ['m', 'ь', 'm']),
            (0x33, [',', 'б', ',']),
            (0x34, ['.', 'ю', '.']),
            (0x35, ['/', '.', '-']),
            (0x39, [' ', ' ', ' ']),
        ];
        let column = match layout {
            0x0409 => 0,
            0x0419 => 1,
            0x0425 => 2,
            _ => return None,
        };
        let (_, characters) = KEYS
            .iter()
            .find(|(scan_code, _)| *scan_code == key.scan_code)?;
        let character = characters[column];
        if key.shift == key.caps_lock {
            Some(character)
        } else if character.is_alphabetic() {
            character.to_uppercase().next()
        } else {
            None
        }
    }

    #[test]
    fn an_unreadable_document_names_the_folder_and_other_failures_do_not() {
        let directory = Path::new("C:\\profile\\AutoKeyboardLayot");
        let base = |error: &std::io::Error| {
            tr_format("error.read_config", &[("error", &error.to_string())])
        };
        let unreadable = std::io::Error::new(std::io::ErrorKind::InvalidData, "line 3: bad value");
        let message = unreadable_configuration_message(&unreadable, Some(directory));
        assert!(message.starts_with(&base(&unreadable)));
        assert!(
            message.contains("C:\\profile\\AutoKeyboardLayot"),
            "{message}"
        );
        assert!(message.len() > base(&unreadable).len() + 20, "{message}");
        // Without a folder, or when the failure is not about the document (for
        // example the installed packages), only the base message is shown.
        assert_eq!(
            unreadable_configuration_message(&unreadable, None),
            base(&unreadable)
        );
        let elsewhere = std::io::Error::other("package set is invalid");
        assert_eq!(
            unreadable_configuration_message(&elsewhere, Some(directory)),
            base(&elsewhere)
        );
    }

    #[test]
    fn retained_words_are_revoked_by_uncertain_contexts_but_not_unchanged_profiles() {
        for reset in 0..4 {
            let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
            let foreground = test_raw_key(WM_KEYDOWN, VK_SPACE, 7, 0).foreground;
            processor.last_boundary = Some(LastBoundary {
                replay_keys: Vec::new(),
                delimiter: Some(' '),
                foreground,
                epoch: 0,
                profile_generation: processor.input_profiles.generation(),
            });
            processor.transpose_cycle = Some(TransposeCycle {
                replay_keys: Vec::new(),
                delimiter: Some(' '),
                foreground,
                language: Language::English,
                text: "hello".into(),
                epoch: 0,
                profile_generation: processor.input_profiles.generation(),
                order: Vec::new(),
            });
            processor.apply_profile_refresh(false);
            assert!(processor.last_boundary.is_some());
            assert!(processor.transpose_cycle.is_some());
            match reset {
                0 => processor.reset_for_epoch(1),
                1 => processor.apply_configuration_reload(Err(std::io::Error::other("test"))),
                2 => processor.set_privacy_reason(Some(PrivacyBlockReason::PasswordField)),
                _ => processor.invalidate_conversion_state(),
            }
            assert!(processor.last_boundary.is_none());
            assert!(processor.transpose_cycle.is_none());
        }
    }

    #[test]
    fn another_space_revokes_the_previous_word_boundary() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        let event = test_raw_key(WM_KEYDOWN, VK_SPACE, 7, 0);
        processor.last_foreground = Some(foreground_identity_key(event.foreground));
        processor.last_layout = Some(event.foreground.layout);
        processor.last_process_id = Some(event.foreground.process_id);
        processor.last_boundary = Some(LastBoundary {
            replay_keys: Vec::new(),
            delimiter: Some(' '),
            foreground: event.foreground,
            epoch: 0,
            profile_generation: processor.input_profiles.generation(),
        });
        processor.process_key(event);
        assert!(processor.last_boundary.is_none());
        assert!(processor.transpose_cycle.is_none());
    }

    #[test]
    fn profile_refresh_preserves_unchanged_words_and_suppresses_only_an_invalidated_tail() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        processor.apply_profile_refresh(true);
        for character in "hello".chars() {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
        }
        assert_eq!(processor.session.buffered_character_count(), 5);
        processor.apply_profile_refresh(false);
        assert_eq!(processor.session.buffered_character_count(), 5);
        processor.apply_profile_refresh(true);
        assert_eq!(processor.session.buffered_character_count(), 0);
        for character in "tail".chars() {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
        }
        assert_eq!(processor.session.buffered_character_count(), 0);
        processor.session.handle(
            InputEvent::Boundary,
            Some(Language::English),
            &processor.detector,
        );
        for character in "word".chars() {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
        }
        assert_eq!(processor.session.buffered_character_count(), 4);
    }

    // Requires the legacy-bundled RU/ET data; the English-only base variant omits it.
    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn pending_conversion_is_bound_to_profile_generation_source_and_target() {
        let processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        let detection = processor
            .detector
            .detect("ghbdtn", Language::English)
            .unwrap();
        let mut pending = PendingConversion {
            transaction: ConversionTransaction::without_delimiter(&detection).unwrap(),
            foreground: test_raw_key(WM_KEYDOWN, VK_SPACE, 7, 0).foreground,
            source_layout: 0x0409,
            profile_generation: processor.input_profiles.generation(),
            space_down_sequence: 7,
            replay_keys: Vec::new(),
            forced: false,
            cycle_order: Vec::new(),
        };
        assert!(processor.pending_profiles_match(&pending));
        pending.profile_generation = pending.profile_generation.wrapping_add(1);
        assert!(!processor.pending_profiles_match(&pending));
        pending.profile_generation = processor.input_profiles.generation();
        pending.source_layout = 0xf0020409;
        assert!(!processor.pending_profiles_match(&pending));
        pending.source_layout = 0x0409;
        pending.transaction.target_language = Language::Japanese;
        assert!(!processor.pending_profiles_match(&pending));
    }

    // Requires the legacy-bundled RU/ET data; the English-only base variant omits it.
    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn worker_resolves_exact_profiles_without_primary_language_fallback() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        assert_eq!(
            processor.language_for_layout(0x0409),
            Some(Language::English)
        );
        assert_eq!(
            processor.language_for_layout(0x0419),
            Some(Language::Russian)
        );
        assert_eq!(
            processor.language_for_layout(0x0425),
            Some(Language::Estonian)
        );
        assert_eq!(processor.language_for_layout(0xf0020409), None);
        assert_eq!(processor.language_for_layout(4), None);
        assert_eq!(
            processor.find_layout(Language::English).unwrap().0 as usize,
            0x0409
        );
        processor.profile_override = None;
        assert_eq!(processor.language_for_layout(0x0409), None);
        assert!(processor.find_layout(Language::English).is_none());
    }

    // Requires the legacy-bundled RU/ET data; the English-only base variant omits it.
    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn candidate_mapping_obeys_runtime_requirements_before_invoking_platform_mapper() {
        let mut registry = autokeyboardlayot::DictionaryRegistry::embedded();
        registry.remove(&Language::English).unwrap();
        registry
            .insert(
                autokeyboardlayot::DictionaryPack::from_words(Language::English, ["hello"], [])
                    .unwrap(),
            )
            .unwrap();
        let detector = Detector::with_registry(Default::default(), Arc::new(registry));
        let settings = Settings {
            enabled_input_packs: [Language::English, Language::Russian].into_iter().collect(),
            ..Default::default()
        };
        for source in [Language::English, Language::Russian] {
            assert!(
                mapped_layout_candidates_with(source, &settings, &detector, |_| {
                    panic!("incompatible or disabled pack reached platform mapper")
                })
                .is_empty()
            );
        }
        let detector = Detector::default();
        let mut calls = Vec::new();
        let candidates =
            mapped_layout_candidates_with(Language::English, &settings, &detector, |target| {
                calls.push(target);
                Some("привет".to_owned())
            });
        assert_eq!(calls, [Language::Russian]);
        assert_eq!(candidates, [(Language::Russian, "привет".to_owned())]);
    }

    fn test_reload_snapshot(name: &str) -> RuntimeConfiguration {
        let mut configuration = RuntimeConfiguration::default();
        configuration.process_exclusions.extend_lines([name]);
        configuration.settings.diagnostics_enabled = true;
        configuration
    }

    #[test]
    fn configuration_loader_coalesces_requests_and_never_blocks_the_caller() {
        let (requests, receive) = sync_channel(1);
        let (reply, replies) = sync_channel(1);
        let mut loader = RuntimeConfigurationLoader {
            requests,
            replies,
            in_flight: false,
            refresh_again: false,
        };
        assert!(loader.poll().is_none());
        assert!(loader.request());
        receive.try_recv().unwrap();
        assert!(loader.poll().is_none()); // no reply: no blocking wait
        assert!(loader.request());
        assert!(loader.request());
        assert!(receive.try_recv().is_err()); // coalesced, not queued again yet
        reply.send(Ok(test_reload_snapshot("older.exe"))).unwrap();
        assert!(loader.poll().is_none()); // superseded snapshot is not adopted
        receive.try_recv().unwrap();
        assert!(receive.try_recv().is_err());
        let latest = test_reload_snapshot("latest.exe");
        let policy = latest.process_exclusions.clone();
        reply.send(Ok(latest)).unwrap();
        assert_eq!(loader.poll().unwrap().unwrap().process_exclusions, policy);
        assert!(!loader.in_flight);
        assert!(loader.request());
        receive.try_recv().unwrap();
        drop(reply);
        assert!(loader.poll().unwrap().is_err());
        assert!(loader.poll().is_none());
    }

    #[test]
    fn managed_package_source_is_guarded_against_active_and_pending_reload() {
        let (mut state, receiver) = test_gate_state(2);
        let source = PackageSource::Managed {
            generation: 2,
            state_sha256: [1; 32],
        };
        let mut snapshot = test_reload_snapshot("private.exe");
        snapshot.package_source = source;
        assert_eq!(
            state.enqueue_configuration(snapshot),
            ConfigurationHandoff::Queued
        );
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("lost-store.exe")),
            ConfigurationHandoff::Refused
        );
        let mut fork = test_reload_snapshot("fork.exe");
        fork.package_source = PackageSource::Managed {
            generation: 2,
            state_sha256: [2; 32],
        };
        assert_eq!(
            state.enqueue_configuration(fork),
            ConfigurationHandoff::Refused
        );
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        processor.process(receiver.try_recv().unwrap());
        let published = state.publish_acknowledged_configuration().unwrap();
        assert_eq!(published.package_source, source);
        assert_eq!(state.package_source, source);
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("lost-store.exe")),
            ConfigurationHandoff::Refused
        );
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn worker_keeps_managed_base_and_policy_when_a_read_fails() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(metrics, 0);
        let mut snapshot = test_reload_snapshot("private.exe");
        snapshot.package_source = PackageSource::Managed {
            generation: 1,
            state_sha256: [1; 32],
        };
        snapshot.dictionaries = Arc::new(autokeyboardlayot::DictionaryRegistry::english_base());
        let policy = snapshot.process_exclusions.clone();
        processor.apply_configuration_reload(Ok(snapshot));
        assert!(
            processor
                .detector
                .detect("ghbdtn", Language::English)
                .is_none()
        );
        processor.apply_configuration_reload(Err(std::io::Error::other("store read failed")));
        assert_eq!(processor.exclusion_policy, policy);
        assert!(
            processor
                .detector
                .detect("ghbdtn", Language::English)
                .is_none()
        );
    }

    #[test]
    fn worker_reload_applies_exact_profile_choices_and_retains_them_on_read_error() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        assert!(
            processor
                .detector
                .input_profile(Language::English)
                .is_some()
        );
        let mut configuration = test_reload_snapshot("private.exe");
        configuration.input_profiles =
            autokeyboardlayot::input_profile_selection::InputProfileSelections::parse([(
                "en-US",
                "0409:00020409",
            )])
            .unwrap();
        processor.apply_configuration_reload(Ok(configuration));
        assert_eq!(processor.detector.input_profile(Language::English), None);
        assert!(processor.exclusion_policy.is_excluded("private.exe"));
        processor.apply_configuration_reload(Err(std::io::Error::other("test read failure")));
        assert_eq!(processor.detector.input_profile(Language::English), None);
        assert!(processor.exclusion_policy.is_excluded("private.exe"));
        processor.apply_configuration_reload(Ok(test_reload_snapshot("private.exe")));
        assert!(
            processor
                .detector
                .input_profile(Language::English)
                .is_some()
        );
    }

    /// Presses Pause on "ghbdtn" ("привет" on the Russian layout) typed in the US layout and returns
    /// the language the processor converts it to, or the reason it ignores the key.
    #[cfg(feature = "legacy-bundled-input")]
    fn pause_result(processor: &mut InputProcessor, diagnostics: &Receiver<String>) -> String {
        let replay_keys = [0x22_u16, 0x23, 0x30, 0x20, 0x14, 0x31]
            .map(|scan_code| ReplayKey {
                scan_code,
                shift: false,
                caps_lock: false,
                extended: false,
            })
            .to_vec();
        let foreground = ForegroundContext {
            layout: 0x0409,
            ..test_foreground()
        };
        diagnostics.try_iter().for_each(drop);
        processor.advance_layout_cycle(
            replay_keys,
            Some(' '),
            foreground,
            0x0409,
            Language::English,
            "ghbdtn".to_owned(),
            41,
            None,
        );
        diagnostics
            .try_iter()
            .find_map(|line| {
                let field = |name: &str| {
                    line.split(' ')
                        .find_map(|field| field.strip_prefix(name).map(str::to_owned))
                };
                if line.starts_with("event=candidate ") {
                    field("target=")
                } else if line.starts_with("event=hotkey ") {
                    field("reason=")
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }

    // Requires the legacy-bundled RU/ET data; the English-only base variant omits it.
    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn worker_dictionary_snapshot_follows_validated_selection_and_reload() {
        let (mut processor, diagnostics) = processor_with_diagnostics();
        processor.test_keyboard = Some(test_keyboard);
        assert!(
            processor
                .detector
                .detect("ghbdtn", Language::English)
                .is_some()
        );
        assert_eq!(pause_result(&mut processor, &diagnostics), "ru-RU");
        let mut snapshot = test_reload_snapshot("private.exe");
        snapshot
            .settings
            .set_pack_enabled(autokeyboardlayot::PackId::parse("ru-RU").unwrap(), false);
        snapshot.dictionaries = dictionary_snapshot(&snapshot.settings);
        processor.apply_configuration_reload(Ok(snapshot));
        assert!(
            processor
                .detector
                .detect("ghbdtn", Language::English)
                .is_none()
        );
        // Pause cycles only through the enabled languages: the Estonian reading of the keys is
        // the typed text itself.
        assert_eq!(pause_result(&mut processor, &diagnostics), "identical");
        processor.apply_configuration_reload(Err(std::io::Error::other("invalid configuration")));
        assert!(
            processor
                .detector
                .detect("ghbdtn", Language::English)
                .is_none()
        );
        // Pause needs two enabled languages with a layout, the typed one among them.
        for (selected, pause) in [
            (vec!["ru-RU"], "single-layout"),
            (vec!["de-DE"], "single-layout"),
            (vec!["en", "ru-RU"], "single-layout"),
            (vec!["ru-RU", "et-EE"], "unsupported-layout"),
        ] {
            let mut snapshot = test_reload_snapshot("private.exe");
            snapshot.settings.enabled_input_packs = selected
                .iter()
                .map(|id| autokeyboardlayot::PackId::parse(id).unwrap())
                .collect();
            snapshot.dictionaries = dictionary_snapshot(&snapshot.settings);
            assert_eq!(
                snapshot
                    .dictionaries
                    .enabled_ids()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                snapshot.settings.enabled_input_packs
            );
            processor.apply_configuration_reload(Ok(snapshot));
            assert!(
                processor
                    .detector
                    .detect("ghbdtn", Language::English)
                    .is_none()
            );
            assert!(
                processor
                    .detector
                    .detect("руддщ", Language::Russian)
                    .is_none()
            );
            assert!(processor.exclusion_policy.is_excluded("private.exe"));
            assert_eq!(
                pause_result(&mut processor, &diagnostics),
                pause,
                "{selected:?}"
            );
        }
        processor.apply_configuration_reload(Ok(test_reload_snapshot("private.exe")));
        assert!(
            processor
                .detector
                .detect("ghbdtn", Language::English)
                .is_some()
        );
        assert_eq!(pause_result(&mut processor, &diagnostics), "ru-RU");
    }

    #[test]
    fn missing_sources_cannot_replace_active_or_pending_configuration() {
        use autokeyboardlayot::configuration::ConfigurationSources;
        for pending in [false, true] {
            let (mut state, receiver) = test_gate_state(2);
            if pending {
                let mut snapshot = test_reload_snapshot("private.exe");
                snapshot.sources = ConfigurationSources::UNIFIED;
                assert_eq!(
                    state.enqueue_configuration(snapshot),
                    ConfigurationHandoff::Queued
                );
            } else {
                state.configuration_sources = ConfigurationSources::UNIFIED;
            }
            let previous_revision = state.next_configuration_revision;
            assert_eq!(
                state.enqueue_configuration(RuntimeConfiguration::default()),
                ConfigurationHandoff::Refused
            );
            assert_eq!(state.next_configuration_revision, previous_revision);
            assert_eq!(
                state.metrics.configuration_pending.load(Ordering::Acquire),
                pending
            );
            if pending {
                let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
                processor.process(receiver.try_recv().unwrap());
                assert!(state.publish_acknowledged_configuration().is_some());
                assert_eq!(state.configuration_sources, ConfigurationSources::UNIFIED);
                assert!(processor.exclusion_policy.is_excluded("private.exe"));
            }
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn input_overflow_does_not_cancel_a_queued_configuration() {
        let (mut state, receiver) = test_gate_state(1);
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("new-policy.exe")),
            ConfigurationHandoff::Queued
        );
        let queued_epoch = state.metrics.input_epoch.load(Ordering::Acquire);
        assert!(!state.enqueue(RawInputEvent::Mouse));
        assert!(state.metrics.input_epoch.load(Ordering::Acquire) > queued_epoch);
        assert!(!state.settings.diagnostics_enabled);
        processor.process(receiver.try_recv().unwrap());
        assert!(processor.exclusion_policy.is_excluded("new-policy.exe"));
        assert_eq!(state.metrics.configuration_ack.load(Ordering::Acquire), 1);
        assert!(
            state
                .take_acknowledged_configuration()
                .unwrap()
                .settings
                .diagnostics_enabled
        );
        // Worker acknowledgement alone must not publish hook settings.
        assert!(!state.metrics.diagnostics_enabled.load(Ordering::Acquire));
    }

    #[test]
    fn a_failed_newer_reload_keeps_the_previous_pending_snapshot() {
        let (mut state, receiver) = test_gate_state(1);
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("first.exe")),
            ConfigurationHandoff::Queued
        );
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("second.exe")),
            ConfigurationHandoff::Busy
        );
        assert!(state.metrics.configuration_pending.load(Ordering::Acquire));
        processor.process(receiver.try_recv().unwrap());
        let acknowledged = state.take_acknowledged_configuration().unwrap();
        assert!(acknowledged.process_exclusions.is_excluded("first.exe"));
        assert!(!acknowledged.process_exclusions.is_excluded("second.exe"));
    }

    #[test]
    fn disconnected_reload_queue_does_not_publish_or_stay_pending() {
        let (mut state, receiver) = test_gate_state(1);
        drop(receiver);
        let original = state.settings.clone();
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("new-policy.exe")),
            ConfigurationHandoff::Refused
        );
        assert_eq!(state.settings, original);
        assert!(state.pending_configuration.is_none());
        assert!(!state.metrics.configuration_pending.load(Ordering::Acquire));
    }

    #[test]
    fn rapid_reloads_wait_for_the_pending_one_and_old_snapshots_never_apply() {
        let (mut state, receiver) = test_gate_state(4);
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("first.exe")),
            ConfigurationHandoff::Queued
        );
        // One snapshot at a time: the next one waits until this one is published or dropped.
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("second.exe")),
            ConfigurationHandoff::Busy
        );
        let first = receiver.try_recv().unwrap();
        processor.process(first.clone());
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("second.exe")),
            ConfigurationHandoff::Busy,
            "acknowledged but not published yet"
        );
        assert!(
            state
                .publish_acknowledged_configuration()
                .unwrap()
                .process_exclusions
                .is_excluded("first.exe")
        );
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("second.exe")),
            ConfigurationHandoff::Queued
        );
        processor.process(receiver.try_recv().unwrap());
        assert!(
            state
                .take_acknowledged_configuration()
                .unwrap()
                .process_exclusions
                .is_excluded("second.exe")
        );
        processor.process(first);
        assert_eq!(state.metrics.configuration_ack.load(Ordering::Acquire), 2);
        assert!(processor.exclusion_policy.is_excluded("second.exe"));
        assert!(!processor.exclusion_policy.is_excluded("first.exe"));
    }

    #[test]
    fn reloads_the_worker_rejected_do_not_keep_later_ones_busy() {
        let (mut state, _receiver) = test_gate_state(4);
        for (revision, program) in [(1, "first.exe"), (2, "second.exe")] {
            assert_eq!(
                state.enqueue_configuration(test_reload_snapshot(program)),
                ConfigurationHandoff::Queued
            );
            // The worker panicked applying it (see reject_faulted_configuration).
            state
                .metrics
                .configuration_rejected
                .store(revision, Ordering::Release);
            assert!(state.drop_rejected_configuration());
        }
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("third.exe")),
            ConfigurationHandoff::Queued
        );
    }

    #[test]
    fn a_reload_never_replaces_one_the_worker_runs_but_the_ui_has_not_published() {
        let (mut state, receiver) = test_gate_state(4);
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("first.exe")),
            ConfigurationHandoff::Queued
        );
        processor.process(receiver.try_recv().unwrap());
        assert_eq!(state.metrics.configuration_ack.load(Ordering::Acquire), 1);
        // Replacing it now would lose it: were the newer one rejected, the UI would keep the
        // settings before both while the worker runs the first.
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("second.exe")),
            ConfigurationHandoff::Busy
        );
        assert!(
            state
                .publish_acknowledged_configuration()
                .unwrap()
                .process_exclusions
                .is_excluded("first.exe")
        );
    }

    #[test]
    fn pending_configuration_passes_hotkeys_and_suppresses_worker_buffering() {
        let (mut state, receiver) = test_gate_state(2);
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("new-policy.exe")),
            ConfigurationHandoff::Queued
        );
        assert_eq!(
            contextual_hotkey_action(&state.metrics, WM_KEYDOWN, VK_PAUSE.0, 0),
            UndoHotkeyAction::PassThrough
        );
        assert!(!gate_request_enabled(&state.metrics, 1));
        processor.process(receiver.try_recv().unwrap());
        assert!(state.enqueue(RawInputEvent::Key(test_raw_key(
            WM_KEYDOWN,
            VIRTUAL_KEY(0x41),
            1,
            0
        ))));
        processor.process(receiver.try_recv().unwrap());
        assert_eq!(processor.session.buffered_character_count(), 0);
        assert!(state.metrics.configuration_pending.load(Ordering::Acquire));
        let transition_epoch = state.metrics.input_epoch.load(Ordering::Acquire);
        assert!(state.publish_acknowledged_configuration().is_some());
        assert!(!state.metrics.configuration_pending.load(Ordering::Acquire));
        assert!(state.metrics.diagnostics_enabled.load(Ordering::Acquire));
        assert!(state.settings.diagnostics_enabled);
        assert!(state.metrics.input_epoch.load(Ordering::Acquire) > transition_epoch);
        assert!(gate_request_enabled(&state.metrics, 1));
        assert!(state.publish_acknowledged_configuration().is_none());
    }

    #[test]
    fn an_active_gate_prevents_configuration_handoff_without_losing_held_state() {
        let (mut state, receiver) = test_gate_state(2);
        state.correction_gate.activate(9);
        assert_eq!(
            state.enqueue_configuration(test_reload_snapshot("new-policy.exe")),
            ConfigurationHandoff::Busy
        );
        assert!(state.correction_gate.active);
        assert_eq!(state.correction_gate.token, 9);
        assert!(receiver.try_recv().is_err());
        assert!(!state.metrics.configuration_pending.load(Ordering::Acquire));
    }

    #[test]
    fn a_configuration_the_worker_cannot_take_yet_is_handed_over_on_a_later_tick() {
        // Apply in the settings while a correction gate holds keys: nothing to report, and the
        // newest snapshot reaches the worker once the gate is over.
        let (mut state, receiver) = test_gate_state(2);
        state.correction_gate.activate(9);
        let loaded = |name: &str| Some(Ok(test_reload_snapshot(name)));
        assert_eq!(state.hand_off_configuration(loaded("first.exe")), None);
        assert_eq!(state.hand_off_configuration(None), None);
        // A newer load replaces the deferred snapshot.
        assert_eq!(state.hand_off_configuration(loaded("second.exe")), None);
        assert!(receiver.try_recv().is_err());
        state.correction_gate.active = false;
        assert_eq!(state.hand_off_configuration(None), None);
        assert!(state.deferred_configuration.is_none());
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        processor.process(receiver.try_recv().expect("the deferred reload"));
        let published = state
            .publish_acknowledged_configuration()
            .expect("published");
        assert!(published.process_exclusions.is_excluded("second.exe"));
        assert!(!published.process_exclusions.is_excluded("first.exe"));
        assert!(receiver.try_recv().is_err());
        // Nothing deferred, nothing loaded: nothing happens.
        assert_eq!(state.hand_off_configuration(None), None);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn only_a_failed_read_is_a_read_error_and_a_refused_snapshot_an_apply_error() {
        let (mut state, receiver) = test_gate_state(2);
        state.correction_gate.activate(9);
        assert_eq!(
            state.hand_off_configuration(Some(Ok(test_reload_snapshot("first.exe")))),
            None
        );
        // The files changed again and cannot be read: the deferred snapshot is out of date.
        let failed = std::io::Error::other("unreadable");
        assert_eq!(
            state.hand_off_configuration(Some(Err(failed))),
            Some(tr_format("error.read_config", &[("error", "unreadable")]))
        );
        assert!(state.deferred_configuration.is_none());
        state.correction_gate.active = false;
        assert_eq!(state.hand_off_configuration(None), None);
        assert!(receiver.try_recv().is_err());
        // A worker that stopped for good can never take a configuration.
        state.metrics.worker_dead.store(true, Ordering::Release);
        assert_eq!(
            state.hand_off_configuration(Some(Ok(test_reload_snapshot("second.exe")))),
            Some(tr_format(
                "error.apply_config",
                &[("error", "configuration_handoff_unavailable")]
            ))
        );
        assert!(state.deferred_configuration.is_none());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn a_reload_without_a_snapshot_cannot_acknowledge_configuration() {
        let (state, receiver) = test_gate_state(1);
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        state
            .metrics
            .configuration_pending
            .store(true, Ordering::Release);
        assert!(state.enqueue(RawInputEvent::ReloadConfiguration));
        processor.process(receiver.try_recv().unwrap());
        assert_eq!(state.metrics.configuration_ack.load(Ordering::Acquire), 0);
        assert_eq!(processor.configuration_revision, 0);
        assert!(state.metrics.configuration_pending.load(Ordering::Acquire));
    }

    #[test]
    fn initial_worker_configuration_is_the_supplied_snapshot() {
        let mut configuration = RuntimeConfiguration::default();
        configuration.settings.diagnostics_enabled = true;
        configuration
            .process_exclusions
            .extend_lines(["private-test.exe"]);
        let metrics = Arc::new(ObserverMetrics::default());
        let processor = InputProcessor::new_with_lexicon_candidate(
            Arc::clone(&metrics),
            0,
            Arc::new(Mutex::new(None)),
            None,
            configuration.clone(),
        );
        assert_eq!(processor.settings, configuration.settings);
        assert_eq!(processor.exclusion_policy, configuration.process_exclusions);
        assert_eq!(processor.backend_rules, configuration.backend_rules);
        assert!(metrics.diagnostics_enabled.load(Ordering::Acquire));
    }

    #[test]
    fn failed_worker_reload_preserves_policy_and_clears_pending_input() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        processor
            .exclusion_policy
            .extend_lines(["private-test.exe"]);
        processor.settings.diagnostics_enabled = true;
        let old_settings = processor.settings.clone();
        let old_policy = processor.exclusion_policy.clone();
        let old_rules = processor.backend_rules.clone();
        processor.session.handle(
            InputEvent::Printable('a'),
            Some(Language::English),
            &processor.detector,
        );
        assert_eq!(processor.session.buffered_character_count(), 1);
        processor.apply_configuration_reload(Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "test failure",
        )));
        assert_eq!(processor.settings, old_settings);
        assert_eq!(processor.exclusion_policy, old_policy);
        assert_eq!(processor.backend_rules, old_rules);
        assert_eq!(processor.session.buffered_character_count(), 0);
        assert!(processor.pending_conversion.is_none());
        assert!(processor.undo_record.is_none());
    }

    fn test_raw_key(
        message: u32,
        virtual_key: VIRTUAL_KEY,
        sequence: u64,
        drain_token: u64,
    ) -> RawKeyEvent {
        RawKeyEvent {
            message,
            virtual_key: u32::from(virtual_key.0),
            scan_code: 0x20,
            caps_lock: false,
            extended: false,
            foreground: ForegroundContext {
                hwnd: 1,
                focus: 1,
                input_thread_id: 2,
                process_id: 3,
                layout: 4,
            },
            sequence,
            drain_token,
            hotkey_trigger: false,
            repeat: false,
            held_modifiers: 0,
        }
    }

    fn test_gate_state(capacity: usize) -> (AppState, Receiver<QueuedInputEvent>) {
        let (sender, receiver) = sync_channel(capacity);
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(true, Ordering::Release);
        let state = AppState {
            window: HWND::default(),
            tray: None,
            keyboard_hook: None,
            mouse_hook: None,
            input_sender: Some(sender),
            worker: None,
            shutdown: Arc::new(AtomicBool::new(false)),
            metrics,
            correction_gate: CorrectionGate::default(),
            last_tray_status: TrayStatus::initial(LayoutIndicator::Unavailable),
            next_privacy_refresh: Instant::now(),
            menu_open: false,
            settings: Settings::default(),
            next_configuration_revision: 0,
            configuration_sources: autokeyboardlayot::configuration::ConfigurationSources::default(
            ),
            package_source: PackageSource::LegacyBootstrap,
            pending_configuration: None,
            deferred_configuration: None,
            configuration_loader: None,
            lexicon_candidate: Arc::new(Mutex::new(None)),
            lexicon_saves: LexiconSaves::new(),
            diagnostic_sender: None,
            diagnostic_worker: None,
            pending_key_releases: Vec::new(),
            pending_held_replay: Vec::new(),
            pending_hand_back: None,
            typed_input: 0,
        };
        (state, receiver)
    }

    #[test]
    fn a_finished_worker_with_a_stuck_busy_flag_does_not_block_exit() {
        let (mut state, _receiver) = test_gate_state(4);
        // A worker that panicked while busy leaves the flag set and its thread finished.
        state.metrics.worker_busy.store(true, Ordering::Release);
        let worker = thread::spawn(|| {});
        while !worker.is_finished() {
            thread::sleep(Duration::from_millis(1));
        }
        state.worker = Some(worker);
        assert!(
            state.request_shutdown(),
            "a stale busy flag of a finished worker must not refuse Exit"
        );
    }

    /// A Space key-down whose hook armed a gate with this sequence as its token.
    fn armed_space(metrics: &Arc<ObserverMetrics>, sequence: u64, epoch: u64) -> QueuedInputEvent {
        metrics.active_gate_token.store(sequence, Ordering::Release);
        QueuedInputEvent {
            epoch,
            captured_at: Instant::now(),
            event: RawInputEvent::Key(test_raw_key(WM_KEYDOWN, VK_SPACE, sequence, 0)),
            configuration: None,
        }
    }

    #[test]
    fn every_way_of_handling_an_armed_space_requests_exactly_one_gate_release() {
        type Prepare = fn(&ObserverMetrics);
        // (path, prepare the metrics, epoch of the queued event)
        let cases: [(&str, Prepare, u64); 4] = [
            (
                "normal path",
                |metrics| metrics.auto_enabled.store(true, Ordering::Release),
                0,
            ),
            (
                "stale epoch",
                |metrics| {
                    metrics.auto_enabled.store(true, Ordering::Release);
                    metrics.input_epoch.store(5, Ordering::Release);
                },
                4,
            ),
            (
                "configuration pending",
                |metrics| {
                    metrics.auto_enabled.store(true, Ordering::Release);
                    metrics.configuration_pending.store(true, Ordering::Release);
                },
                0,
            ),
            ("automatic conversion off", |_| {}, 0),
        ];
        for (path, prepare, epoch) in cases {
            let metrics = Arc::new(ObserverMetrics::default());
            prepare(&metrics);
            let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
            processor.set_privacy_reason(None);
            let queued = armed_space(&metrics, 7, epoch);
            processor.process(queued);
            assert_eq!(
                metrics.gate_release_requests.load(Ordering::Acquire),
                1,
                "{path}: an armed gate needs exactly one release request"
            );
            assert_eq!(processor.last_gate_release_token, 7, "{path}");
        }
    }

    #[test]
    fn a_recovered_gate_abort_resumes_automatic_conversion_at_most_three_times_an_hour() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        let start = Instant::now();
        let minute = |n: u64| start + Duration::from_secs(60 * n);
        let pause = |reason: ConversionFailureReason| {
            metrics.auto_enabled.store(false, Ordering::Release);
            metrics.safety_paused.store(true, Ordering::Release);
            metrics
                .last_failure_reason
                .store(reason as u8, Ordering::Release);
        };
        for n in 0..3 {
            pause(ConversionFailureReason::GateDrain);
            let epoch = metrics.input_epoch.load(Ordering::Acquire);
            processor.resume_after_recovered_abort(minute(n));
            assert!(metrics.auto_enabled.load(Ordering::Acquire), "resume {n}");
            assert!(!metrics.safety_paused.load(Ordering::Acquire));
            assert_eq!(
                metrics.last_failure_reason.load(Ordering::Acquire),
                ConversionFailureReason::None as u8
            );
            assert!(metrics.input_epoch.load(Ordering::Acquire) > epoch);
        }
        // The fourth abort within the hour stays paused.
        pause(ConversionFailureReason::GateDrain);
        processor.resume_after_recovered_abort(minute(30));
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
        assert!(metrics.safety_paused.load(Ordering::Acquire));
        // Once the first resume is an hour old, there is room again.
        processor.resume_after_recovered_abort(minute(60));
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
    }

    #[test]
    fn only_a_pause_caused_by_the_gate_abort_is_resumed() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        let now = Instant::now();
        // (auto enabled, safety paused, reason, resumed): every other pause stays as it is.
        let cases = [
            (false, true, ConversionFailureReason::PhysicalEdit, false),
            (false, true, ConversionFailureReason::ClipboardEdit, false),
            (false, true, ConversionFailureReason::InternalFault, false),
            // The user switched automatic conversion off: no safety pause is recorded.
            (false, false, ConversionFailureReason::GateDrain, false),
            // Already on: nothing to resume and no budget is consumed.
            (true, false, ConversionFailureReason::GateDrain, true),
            (false, true, ConversionFailureReason::GateDrain, true),
        ];
        for (auto_enabled, paused, reason, resumed) in cases {
            metrics.auto_enabled.store(auto_enabled, Ordering::Release);
            metrics.safety_paused.store(paused, Ordering::Release);
            metrics
                .last_failure_reason
                .store(reason as u8, Ordering::Release);
            processor.resume_after_recovered_abort(now);
            assert_eq!(
                metrics.auto_enabled.load(Ordering::Acquire),
                resumed,
                "{auto_enabled} {paused} {reason:?}"
            );
        }
    }

    #[test]
    fn deleted_held_keys_resume_automatic_conversion_even_from_an_older_input_epoch() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        metrics.auto_enabled.store(false, Ordering::Release);
        metrics.safety_paused.store(true, Ordering::Release);
        metrics
            .last_failure_reason
            .store(ConversionFailureReason::GateDrain as u8, Ordering::Release);
        let queued_epoch = metrics.input_epoch.load(Ordering::Acquire);
        // Input that arrived after the expiry was queued moved the epoch on.
        metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        processor.process_event(QueuedInputEvent {
            epoch: queued_epoch,
            captured_at: Instant::now(),
            event: RawInputEvent::RetainedInputExpired,
            configuration: None,
        });
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
        assert!(!metrics.safety_paused.load(Ordering::Acquire));
        assert_eq!(
            metrics.last_failure_reason.load(Ordering::Acquire),
            ConversionFailureReason::None as u8
        );
    }

    #[test]
    fn periodic_field_checks_are_spaced_by_their_cost() {
        let interval = |ms: u64| privacy_refresh_interval(Duration::from_millis(ms)).as_millis();
        // Fast providers (a terminal answers in a few milliseconds) keep the half second.
        assert_eq!(interval(0), 500);
        assert_eq!(interval(3), 500);
        assert_eq!(interval(25), 500);
        assert_eq!(interval(26), 520);
        // The game of 2026-10-11 answered in about 144 ms: about every three seconds instead
        // of twice a second.
        assert_eq!(interval(144), 2_880);
        // Never more than five seconds apart, even after a check that timed out.
        assert_eq!(interval(250), 5_000);
        assert_eq!(interval(300), 5_000);
        assert_eq!(interval(u64::MAX), 5_000);
    }

    #[test]
    fn a_slow_full_screen_program_is_checked_once_per_input_target() {
        let game = test_raw_key(WM_KEYDOWN, VK_SPACE, 7, 0).foreground;
        let slow = Duration::from_millis(PRIVACY_SLOW_PROVIDER_MS);
        let fast = slow - Duration::from_millis(1);
        assert!(full_screen_check_is_redundant(
            true,
            Some((game, slow)),
            game
        ));
        // A layout switch is no new input target.
        let other_layout = ForegroundContext {
            layout: game.layout + 1,
            ..game
        };
        assert!(full_screen_check_is_redundant(
            true,
            Some((game, slow)),
            other_layout
        ));
        // Not full-screen, never checked, a fast provider, or another input target: check.
        assert!(!full_screen_check_is_redundant(
            false,
            Some((game, slow)),
            game
        ));
        assert!(!full_screen_check_is_redundant(true, None, game));
        assert!(!full_screen_check_is_redundant(
            true,
            Some((game, fast)),
            game
        ));
        let other_focus = ForegroundContext {
            focus: game.focus + 1,
            ..game
        };
        assert!(!full_screen_check_is_redundant(
            true,
            Some((game, slow)),
            other_focus
        ));
    }

    #[test]
    fn the_hotkey_cycle_starts_with_the_most_plausible_reading() {
        let enabled = [Language::English, Language::Estonian, Language::Russian];
        // 2026-10-12 05:39: a word typed in the English layout went to Estonian first, because
        // the enabled languages are kept in alphabetical order, and needed a second press.
        assert_eq!(
            layout_cycle_order(
                Language::English,
                &enabled,
                &[Language::Russian, Language::Estonian]
            ),
            [Language::English, Language::Russian, Language::Estonian]
        );
        // Without a ranking the configured order follows the source, as before.
        assert_eq!(
            layout_cycle_order(Language::Russian, &enabled, &[]),
            [Language::Russian, Language::English, Language::Estonian]
        );
        // A language the word cannot be read in comes last, and nothing is repeated.
        assert_eq!(
            layout_cycle_order(Language::English, &enabled, &[Language::Estonian]),
            [Language::English, Language::Estonian, Language::Russian]
        );
    }

    #[test]
    fn only_a_complete_automatic_recovery_may_resume() {
        assert!(should_resume_after_recovery(false, 12, 12));
        assert!(!should_resume_after_recovery(true, 12, 12), "explicit");
        assert!(!should_resume_after_recovery(false, 7, 12), "partial");
        assert!(!should_resume_after_recovery(false, 0, 12), "nothing sent");
        assert!(
            !should_resume_after_recovery(false, 0, 0),
            "nothing requested"
        );
    }

    #[test]
    fn a_long_gap_between_timer_ticks_is_reported_as_a_ui_stall() {
        let start = Instant::now();
        let after = |ms: u64| start + Duration::from_millis(ms);
        assert_eq!(timer_stall_ms(None, after(5_000)), None);
        assert_eq!(timer_stall_ms(Some(start), after(250)), None);
        assert_eq!(timer_stall_ms(Some(start), after(700)), None);
        assert_eq!(timer_stall_ms(Some(start), after(701)), Some(701));
        assert_eq!(
            timer_stall_ms(Some(after(10)), start),
            None,
            "clock went back"
        );
    }

    /// The polls of `polls` in order, and a clock that runs out once every one has been read:
    /// the outcome never depends on how fast the test runs. A poll after the clock ran out fails
    /// the test.
    fn scripted_polls(
        polls: Vec<SelectionPoll>,
    ) -> (impl FnMut() -> bool, impl FnMut() -> SelectionPoll) {
        let polls = std::rc::Rc::new(RefCell::new(VecDeque::from(polls)));
        let script = std::rc::Rc::clone(&polls);
        (
            move || script.borrow().is_empty(),
            move || {
                polls
                    .borrow_mut()
                    .pop_front()
                    .expect("polled after the time for confirmation ran out")
            },
        )
    }

    #[test]
    fn a_confirmed_selection_is_kept_and_every_other_outcome_collapses_it_exactly_once() {
        let text = |value: &str| SelectionPoll::Text(value.to_owned());
        // (path, scripted polls; the time runs out when the script does, expected status, collapses)
        let cases = [
            (
                "match at once",
                vec![text("ghbdtn")],
                SelectionConfirmation::Confirmed,
                0,
            ),
            (
                "unreadable, then match",
                vec![
                    SelectionPoll::Unreadable,
                    SelectionPoll::Unreadable,
                    text("ghbdtn"),
                ],
                SelectionConfirmation::Confirmed,
                0,
            ),
            (
                "mismatch, then match",
                vec![text("other"), text("ghbdtn")],
                SelectionConfirmation::Confirmed,
                0,
            ),
            (
                "the selected text never matches",
                vec![text("other")],
                SelectionConfirmation::OtherText,
                1,
            ),
            (
                "the selection is empty",
                vec![SelectionPoll::Empty],
                SelectionConfirmation::NotShown,
                1,
            ),
            (
                "the selection is not one range",
                vec![SelectionPoll::NotOneRange],
                SelectionConfirmation::NotShown,
                1,
            ),
            (
                "empty, then other text",
                vec![SelectionPoll::Empty, text("other")],
                SelectionConfirmation::OtherText,
                1,
            ),
            (
                "the selection stays unreadable",
                vec![],
                SelectionConfirmation::Unreadable,
                1,
            ),
        ];
        for (path, polls, expected, collapses) in cases {
            let collapsed = Cell::new(0);
            let (expired, poll) = scripted_polls(polls);
            let (status, cleanup) = confirm_selection(
                expired,
                poll,
                |actual| actual == "ghbdtn",
                || {},
                || {
                    collapsed.set(collapsed.get() + 1);
                    SelectionCleanup::Unverified
                },
            );
            assert_eq!(status, expected, "{path}");
            assert_eq!(collapsed.get(), collapses, "{path}: collapses");
            // The result of the collapse is passed on as it was found.
            assert_eq!(
                cleanup,
                (collapses == 1).then_some(SelectionCleanup::Unverified),
                "{path}: cleanup"
            );
        }
    }

    #[test]
    fn a_selection_outcome_maps_to_the_old_status() {
        assert_eq!(
            SelectionConfirmation::Confirmed.status(),
            TextBarrierStatus::Match
        );
        assert_eq!(
            SelectionConfirmation::OtherText.status(),
            TextBarrierStatus::Mismatch
        );
        assert_eq!(
            SelectionConfirmation::NotShown.status(),
            TextBarrierStatus::Mismatch
        );
        assert_eq!(
            SelectionConfirmation::Unreadable.status(),
            TextBarrierStatus::Unavailable
        );
    }

    #[test]
    fn only_pause_waits_longer_and_selects_again() {
        // Automatic conversions keep the wait that the correction gate's budget accounts for.
        assert_eq!(
            PastePatience::Automatic.confirm_timeout(),
            Duration::from_millis(UIA_SELECTION_CONFIRM_TIMEOUT_MS)
        );
        assert_eq!(PastePatience::Automatic.attempts(), 1);
        assert_eq!(
            PastePatience::Manual.confirm_timeout(),
            Duration::from_millis(100)
        );
        assert_eq!(PastePatience::Manual.attempts(), 2);
        assert_eq!(paste_patience(true), PastePatience::Manual);
        assert_eq!(paste_patience(false), PastePatience::Automatic);
    }

    #[test]
    fn only_a_selection_the_field_did_not_show_is_tried_again() {
        use SelectionConfirmation::{Confirmed, NotShown, OtherText, Unreadable};
        let run = |attempts: u32, script: Vec<SelectionAttempt>| {
            let mut script = std::collections::VecDeque::from(script);
            let mut earlier = Vec::new();
            let result = retry_unshown_selection(attempts, |earlier_attempt| {
                earlier.push(earlier_attempt);
                script.pop_front().expect("no more attempts than scripted")
            });
            // Every attempt after the first is told that an earlier one ran.
            assert!(
                earlier
                    .iter()
                    .enumerate()
                    .all(|(index, value)| *value == (index > 0))
            );
            result
        };
        let attempt = |status, confirmation, cleanup| SelectionAttempt {
            status,
            confirmation,
            cleanup,
        };
        let not_shown = attempt(
            TextBarrierStatus::Mismatch,
            Some(NotShown),
            Some(SelectionCleanup::Collapsed),
        );
        let confirmed = attempt(TextBarrierStatus::Match, Some(Confirmed), None);
        // A second selection that the field shows is used.
        assert_eq!(run(2, vec![not_shown, confirmed]), (confirmed, 2));
        // Two unshown selections give up as before.
        assert_eq!(run(2, vec![not_shown, not_shown]), (not_shown, 2));
        // An automatic conversion looks once.
        assert_eq!(run(1, vec![not_shown]), (not_shown, 1));
        // Other text, unreadable selections, text that never matched, and an unshown selection
        // that could not be collapsed are not retried.
        for once in [
            attempt(
                TextBarrierStatus::Mismatch,
                Some(OtherText),
                Some(SelectionCleanup::Collapsed),
            ),
            attempt(
                TextBarrierStatus::Unavailable,
                Some(Unreadable),
                Some(SelectionCleanup::Collapsed),
            ),
            attempt(TextBarrierStatus::Mismatch, None, None),
            attempt(TextBarrierStatus::Unavailable, None, None),
            attempt(
                TextBarrierStatus::Mismatch,
                Some(NotShown),
                Some(SelectionCleanup::Unverified),
            ),
            confirmed,
        ] {
            assert_eq!(run(2, vec![once]), (once, 1), "{once:?}");
        }
    }

    #[test]
    fn the_barrier_report_says_how_the_selection_was_confirmed() {
        let report = TextBarrierReport {
            stage: TextBarrierStage::TextCompared,
            confirmation: Some(SelectionConfirmation::NotShown),
            attempts: 2,
            ..TextBarrierReport::default()
        };
        let line = format_text_barrier_report(report);
        assert!(
            line.contains(",confirm=Some(NotShown),attempts=2"),
            "{line}"
        );
        assert!(
            format_text_barrier_report(TextBarrierReport::default())
                .ends_with(",confirm=None,attempts=0,cleanup=None")
        );
    }

    #[test]
    fn a_cancelled_protected_paste_leaves_a_final_line() {
        let (mut processor, receiver) = processor_with_diagnostics();
        assert!(
            processor
                .apply_paste_attempt_result(TextEditAttempt::Cancelled, 42, PasteRoute::Backend)
                .is_none()
        );
        let lines: Vec<String> = receiver
            .try_iter()
            .filter(|record| record.starts_with("event=conversion "))
            .collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("sequence=42 result=cancelled strategy=ProtectedPaste edit=none"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_cancelled_paste_keeps_automatic_conversion_and_a_failed_one_pauses_it() {
        // A busy clipboard ends as Cancelled: nothing was typed, so automatic conversion stays.
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(true, Ordering::Release);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        assert!(
            processor
                .apply_paste_attempt_result(TextEditAttempt::Cancelled, 7, PasteRoute::Backend)
                .is_none()
        );
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
        assert!(
            processor
                .apply_paste_attempt_result(
                    TextEditAttempt::Failed(ConversionFailureReason::ClipboardEdit),
                    8,
                    PasteRoute::Backend
                )
                .is_none()
        );
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
    }

    #[test]
    fn an_automatic_paste_gives_up_on_a_busy_clipboard_sooner_than_pause() {
        assert_eq!(
            PastePatience::Automatic.clipboard_install_timeout(),
            Duration::from_millis(200)
        );
        assert_eq!(
            PastePatience::Manual.clipboard_install_timeout(),
            Duration::from_millis(CLIPBOARD_BROKER_TIMEOUT_MS)
        );
    }

    #[test]
    fn held_keys_are_kept_back_only_on_a_positive_privacy_finding() {
        for (reason, allowed) in [
            (None, true),
            (Some(PrivacyBlockReason::FieldInspectionUnavailable), true),
            (Some(PrivacyBlockReason::InspectionUnavailable), true),
            (Some(PrivacyBlockReason::PasswordField), false),
            (Some(PrivacyBlockReason::ExcludedProcess), false),
            (Some(PrivacyBlockReason::ElevatedProcess), false),
        ] {
            assert_eq!(recovery_privacy_allows(reason), allowed, "{reason:?}");
        }
    }

    #[test]
    fn a_confirmed_edit_with_an_unrestored_clipboard_is_not_a_failure() {
        assert_eq!(
            protected_paste_outcome(true, true),
            TextEditAttempt::Applied
        );
        assert_eq!(
            protected_paste_outcome(true, false),
            TextEditAttempt::AppliedClipboardNotRestored
        );
        assert_eq!(
            protected_paste_outcome(false, true),
            TextEditAttempt::Failed(ConversionFailureReason::TextCommit)
        );
        assert_eq!(
            protected_paste_outcome(false, false),
            TextEditAttempt::Failed(ConversionFailureReason::TextCommit)
        );
        // Restoring the user's clipboard waits three times as long as installing the temporary text.
        assert_eq!(CLIPBOARD_RESTORE_OPEN_RETRIES, 3 * CLIPBOARD_OPEN_RETRIES);
        assert_eq!(
            CLIPBOARD_RESTORE_OPEN_RETRIES as u64 * CLIPBOARD_RETRY_DELAY_MS,
            60
        );
    }

    #[test]
    fn an_unrestored_clipboard_keeps_automatic_conversion_on_but_a_failed_edit_pauses_it() {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(true, Ordering::Release);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        assert_eq!(
            processor.apply_paste_attempt_result(TextEditAttempt::Applied, 5, PasteRoute::Backend),
            Some(EditStrategy::ProtectedPaste)
        );
        assert_eq!(
            metrics.clipboard_restore_failures.load(Ordering::Acquire),
            0
        );
        assert_eq!(
            processor.apply_paste_attempt_result(
                TextEditAttempt::AppliedClipboardNotRestored,
                6,
                PasteRoute::Backend
            ),
            Some(EditStrategy::ProtectedPaste)
        );
        assert_eq!(
            metrics.clipboard_restore_failures.load(Ordering::Acquire),
            1
        );
        assert_eq!(
            processor.apply_paste_attempt_result(
                TextEditAttempt::Cancelled,
                7,
                PasteRoute::Backend
            ),
            None
        );
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
        assert_eq!(
            processor.apply_paste_attempt_result(
                TextEditAttempt::Failed(ConversionFailureReason::ClipboardEdit),
                8,
                PasteRoute::Backend
            ),
            None
        );
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
        assert_eq!(
            metrics.last_failure_reason.load(Ordering::Acquire),
            ConversionFailureReason::ClipboardEdit as u8
        );
    }

    #[test]
    fn session_and_power_notifications_choose_the_reinstall_reason() {
        assert_eq!(session_reinstall_reason(WTS_SESSION_UNLOCK), Some("unlock"));
        assert_eq!(
            session_reinstall_reason(WTS_CONSOLE_CONNECT),
            Some("connect")
        );
        assert_eq!(
            session_reinstall_reason(WTS_REMOTE_CONNECT),
            Some("connect")
        );
        // Console disconnect, remote disconnect, logon, logoff and lock change nothing.
        for event in [2, 4, 5, 6, 7] {
            assert_eq!(
                session_reinstall_reason(event),
                None,
                "session event {event}"
            );
        }
        assert_eq!(
            power_reinstall_reason(PBT_APMRESUMEAUTOMATIC),
            Some("resume")
        );
        assert_eq!(power_reinstall_reason(PBT_APMRESUMESUSPEND), Some("resume"));
        // Suspend and power status changes are not a resume.
        for event in [0x4, 0xA, 0x8013] {
            assert_eq!(
                power_reinstall_reason(event),
                None,
                "power event {event:#x}"
            );
        }
    }

    #[test]
    fn a_failed_attempt_is_retried_with_a_growing_delay() {
        let now = Instant::now();
        let mut retry = Retry::first(now);
        assert_eq!(retry.failures, 1);
        assert_eq!(retry.next_attempt, now + Duration::from_secs(1));
        retry.failed_again(now);
        assert_eq!(retry.failures, 2);
        assert_eq!(retry.next_attempt, now + Duration::from_secs(2));
        for _ in 0..10 {
            retry.failed_again(now);
        }
        assert_eq!(retry.next_attempt, now + Duration::from_secs(30));
    }

    #[test]
    fn each_hook_s_silence_is_measured_from_its_own_callbacks() {
        let ten_seconds_ago = || unsafe { GetTickCount64() }.saturating_sub(10_000);
        let about_ten_seconds = |silence: Duration| {
            silence >= Duration::from_secs(10) && silence < Duration::from_secs(12)
        };
        // A keyboard callback (code < 0: passed on untouched) refreshes the keyboard hook's mark
        // only.
        KEYBOARD_HOOK_ACTIVITY_MS.store(ten_seconds_ago(), Ordering::Relaxed);
        MOUSE_HOOK_ACTIVITY_MS.store(ten_seconds_ago(), Ordering::Relaxed);
        unsafe { keyboard_hook_proc(-1, WPARAM(0), LPARAM(0)) };
        let silence = hook_silence();
        assert!(silence.keyboard < Duration::from_secs(2), "{silence:?}");
        assert!(about_ten_seconds(silence.mouse), "{silence:?}");
        // And a mouse callback the mouse hook's.
        KEYBOARD_HOOK_ACTIVITY_MS.store(ten_seconds_ago(), Ordering::Relaxed);
        unsafe { mouse_hook_proc(-1, WPARAM(0), LPARAM(0)) };
        let silence = hook_silence();
        assert!(about_ten_seconds(silence.keyboard), "{silence:?}");
        assert!(silence.mouse < Duration::from_secs(2), "{silence:?}");
        // The pointer's position can be read on the test's desktop or not at all.
        let _ = cursor_position();
    }

    #[test]
    fn the_input_probes_answer_without_crashing() {
        // The values depend on the session (a locked screen has another input desktop), so only
        // the calls themselves are exercised: sizes, handles and buffers must be right.
        if let Some(idle) = input_idle() {
            assert!(idle < Duration::from_secs(60 * 60 * 24 * 50), "{idle:?}");
        }
        let _ = input_desktop_is_default();
        let _ = foreground_privilege();
    }

    #[test]
    fn the_foreground_privilege_compares_the_foreground_process_with_the_agent() {
        use ForegroundPrivilege::{Higher, NotHigher, Unknown};
        const LOW: u32 = 0x1000;
        const MEDIUM: u32 = 0x2000;
        const HIGH: u32 = 0x3000;
        // Process 10 is the agent at medium integrity, 20 an elevated program, 30 a process whose
        // token cannot be read, and every other one runs at low integrity.
        let level = |process: u32| match process {
            10 => Some(MEDIUM),
            20 => Some(HIGH),
            30 => None,
            _ => Some(LOW),
        };
        assert_eq!(
            foreground_privilege_of(None, 10, level),
            ForegroundPrivilege::None
        );
        assert_eq!(foreground_privilege_of(Some(20), 10, level), Higher);
        assert_eq!(foreground_privilege_of(Some(10), 10, level), NotHigher);
        assert_eq!(foreground_privilege_of(Some(40), 10, level), NotHigher);
        assert_eq!(foreground_privilege_of(Some(30), 10, level), Unknown);
        // An agent that cannot read its own level knows nothing either.
        assert_eq!(foreground_privilege_of(Some(40), 30, level), Unknown);
        // The real lookup: the agent is never above itself, and the idle process cannot be opened.
        let agent = unsafe { GetCurrentProcessId() };
        assert_eq!(
            foreground_privilege_of(Some(agent), agent, process_integrity_level),
            NotHigher
        );
        assert_eq!(
            foreground_privilege_of(Some(0), agent, process_integrity_level),
            Unknown
        );
    }

    #[test]
    fn events_that_did_not_arm_a_gate_request_no_release() {
        let cases = [
            // A drained Space replay carries its gate token: the gate is already being drained.
            (VK_SPACE, WM_KEYDOWN, 7, 3),
            // Space key-up, and keys other than Space, never arm the decision gate.
            (VK_SPACE, WM_KEYUP, 7, 0),
            (VIRTUAL_KEY(0x47), WM_KEYDOWN, 7, 0),
        ];
        for (key, message, sequence, drain_token) in cases {
            let metrics = Arc::new(ObserverMetrics::default());
            metrics.auto_enabled.store(true, Ordering::Release);
            metrics.active_gate_token.store(sequence, Ordering::Release);
            let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
            processor.set_privacy_reason(None);
            processor.process(QueuedInputEvent {
                epoch: 0,
                captured_at: Instant::now(),
                event: RawInputEvent::Key(test_raw_key(message, key, sequence, drain_token)),
                configuration: None,
            });
            assert_eq!(
                metrics.gate_release_requests.load(Ordering::Acquire),
                0,
                "{key:?} {message} drain_token={drain_token}"
            );
        }
        // A Space that belongs to a different gate token is not this gate's decision event.
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.active_gate_token.store(9, Ordering::Release);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        processor.set_privacy_reason(None);
        processor.process(QueuedInputEvent {
            epoch: 0,
            captured_at: Instant::now(),
            event: RawInputEvent::Key(test_raw_key(WM_KEYDOWN, VK_SPACE, 7, 0)),
            configuration: None,
        });
        assert_eq!(metrics.gate_release_requests.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_running_worker_still_blocks_exit_until_its_thread_has_finished() {
        let (mut state, _receiver) = test_gate_state(4);
        state.metrics.worker_busy.store(true, Ordering::Release);
        let (release, wait) = std::sync::mpsc::channel::<()>();
        state.worker = Some(thread::spawn(move || {
            let _ = wait.recv();
        }));
        assert!(!state.request_shutdown());
        assert_eq!(state.decline_reasons(), ["worker-busy"]);
        release.send(()).unwrap();
        while !state.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            thread::sleep(Duration::from_millis(1));
        }
        // The flag is still set (nobody cleared it), but the thread is gone.
        assert!(state.metrics.worker_busy.load(Ordering::Acquire));
        assert!(state.request_shutdown());
    }

    #[test]
    fn shutdown_blockers_name_every_reason_in_order_and_ignore_a_finished_workers_flag() {
        let all = ShutdownFacts {
            correction_gate_active: true,
            retained_input: true,
            worker_busy: true,
            worker_finished: false,
            configuration_pending: true,
            undo_hotkey_active: true,
            hotkey_waiting_for_release: true,
        };
        assert_eq!(
            shutdown_blockers(&all),
            [
                "correction-gate",
                "retained-input",
                "worker-busy",
                "configuration-pending",
                "undo-hotkey",
                "hotkey-release",
            ]
        );
        let finished = ShutdownFacts {
            worker_finished: true,
            ..all
        };
        // A finished worker can neither be busy, acknowledge the pending reload nor recover the
        // held keys.
        assert_eq!(
            shutdown_blockers(&finished),
            ["correction-gate", "undo-hotkey", "hotkey-release"]
        );
        let idle = ShutdownFacts {
            correction_gate_active: false,
            retained_input: false,
            worker_busy: false,
            worker_finished: false,
            configuration_pending: false,
            undo_hotkey_active: false,
            hotkey_waiting_for_release: false,
        };
        assert!(shutdown_blockers(&idle).is_empty());
    }

    #[test]
    fn the_exit_prompt_names_the_reasons_and_warns_only_about_retained_input() {
        let plain = exit_anyway_text(&["worker-busy", "correction-gate"]);
        assert!(plain.contains("worker-busy, correction-gate"), "{plain}");
        assert!(!plain.contains("held back"), "{plain}");
        let retained = exit_anyway_text(&["retained-input", "worker-busy"]);
        assert!(
            retained.contains("retained-input, worker-busy"),
            "{retained}"
        );
        assert!(retained.contains("held back"), "{retained}");
        assert!(retained.starts_with(plain.split("worker-busy").next().unwrap()));
    }

    #[test]
    fn an_implicit_close_never_sets_the_explicit_exit_mark_and_never_clears_it() {
        EXPLICIT_SHUTDOWN.store(false, Ordering::Release);
        // Accepted every time: an installer's close first, then the tray menu Exit, then another
        // close. Only the tray menu sets the mark, and nothing clears it.
        for (explicit, marked) in [(false, false), (true, true), (false, true)] {
            let (state, _receiver) = test_gate_state(4);
            APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
            request_shutdown(HWND::default(), explicit);
            assert_eq!(
                EXPLICIT_SHUTDOWN.load(Ordering::Acquire),
                marked,
                "explicit={explicit}"
            );
            drop(take_app_state());
        }
        EXPLICIT_SHUTDOWN.store(false, Ordering::Release);
    }

    #[test]
    fn an_engine_fault_pauses_automatic_conversion_and_ends_hotkey_gestures() {
        let (mut state, _receiver) = test_gate_state(4);
        state
            .metrics
            .undo_hotkey_active
            .store(true, Ordering::Release);
        state
            .metrics
            .hotkey_waiting_for_release
            .store(true, Ordering::Release);
        state.correction_gate.activate(10);
        let epoch = state.metrics.input_epoch.load(Ordering::Acquire);
        state.record_engine_fault();
        let metrics = &state.metrics;
        assert!(!state.correction_gate.active);
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
        assert!(metrics.safety_paused.load(Ordering::Acquire));
        assert_eq!(
            metrics.last_failure_reason.load(Ordering::Acquire),
            ConversionFailureReason::InternalFault as u8
        );
        assert_eq!(
            conversion_failure_label(ConversionFailureReason::InternalFault as u8),
            "internal-fault"
        );
        assert_eq!(metrics.conversion_failures.load(Ordering::Acquire), 1);
        assert!(!metrics.undo_hotkey_active.load(Ordering::Acquire));
        assert!(!metrics.hotkey_waiting_for_release.load(Ordering::Acquire));
        assert!(metrics.input_epoch.load(Ordering::Acquire) > epoch);
        // Repeating the fault does not count the same pause twice.
        state.record_engine_fault();
        assert_eq!(state.metrics.conversion_failures.load(Ordering::Acquire), 1);
        assert!(state.decline_reasons().is_empty());
    }

    #[test]
    fn the_ui_timer_turns_a_contained_panic_into_a_pause_and_clears_the_flag() {
        let (state, _receiver) = test_gate_state(4);
        let metrics = Arc::clone(&state.metrics);
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        ENGINE_FAULT.store(true, Ordering::Release);
        handle_engine_fault(HWND::default());
        assert!(!ENGINE_FAULT.load(Ordering::Acquire));
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
        assert!(metrics.safety_paused.load(Ordering::Acquire));
        // Without a fault the handler changes nothing.
        metrics.safety_paused.store(false, Ordering::Release);
        handle_engine_fault(HWND::default());
        assert!(!metrics.safety_paused.load(Ordering::Acquire));
        drop(take_app_state());
    }

    #[test]
    fn graceful_shutdown_refuses_input_and_configuration_operations() {
        let (mut state, _receiver) = test_gate_state(4);
        state.correction_gate.activate(10);
        assert!(!state.request_shutdown());
        assert!(state.correction_gate.active);
        state.correction_gate.active = false;
        let retained_key = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0);
        *state.metrics.retained_input.lock().unwrap() = Some(RetainedInput {
            token: 10,
            foreground: retained_key.foreground,
            events: vec![retained_key],
            delivery_uncertain: false,
            expires_at: Instant::now() + Duration::from_millis(RETAINED_INPUT_LIFETIME_MS),
            recovering: false,
            retained_at: Instant::now(),
            auto_recover: false,
            input_mark: 0,
        });
        assert!(!state.request_shutdown());
        assert_eq!(
            state
                .metrics
                .retained_input
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .events
                .len(),
            1
        );
        state.metrics.retained_input.lock().unwrap().take();
        for index in 0..4 {
            let metrics = Arc::clone(&state.metrics);
            let flag = [
                &metrics.worker_busy,
                &metrics.configuration_pending,
                &metrics.undo_hotkey_active,
                &metrics.hotkey_waiting_for_release,
            ][index];
            flag.store(true, Ordering::Release);
            assert!(!state.request_shutdown());
            assert!(flag.load(Ordering::Acquire));
            assert!(!state.shutdown.load(Ordering::Acquire));
            assert!(state.input_sender.is_some());
            flag.store(false, Ordering::Release);
        }
        assert!(state.request_shutdown());
        assert!(state.request_shutdown());
        assert!(state.shutdown_finished());
        assert!(state.input_sender.is_none());
    }

    #[test]
    fn graceful_shutdown_waits_for_actual_worker_return() {
        let (mut state, _receiver) = test_gate_state(4);
        let (release, wait) = sync_channel::<()>(1);
        state.worker = Some(thread::spawn(move || {
            let _ = wait.recv_timeout(Duration::from_secs(2));
        }));
        assert!(state.request_shutdown());
        assert!(!state.shutdown_finished());
        release.send(()).unwrap();
        state.worker.take().unwrap().join().unwrap();
        assert!(state.shutdown_finished());
    }

    #[test]
    fn expired_hold_preserves_swallowed_edges_for_recovery() {
        let mut gate = CorrectionGate::default();
        gate.activate(10);
        let edge = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0);
        assert!(gate.hold(edge));
        gate.deadline = Instant::now() - Duration::from_millis(1);
        assert!(!gate.hold(edge));
        assert!(gate.active);
        assert_eq!(gate.held.len(), 1);
        assert_eq!(gate.held[0].virtual_key, edge.virtual_key);
    }

    #[test]
    fn full_hold_preserves_every_edge_including_modifier_ups() {
        let mut gate = CorrectionGate::default();
        gate.activate(10);
        let edge = test_raw_key(WM_KEYUP, VK_LCONTROL, 0, 0);
        for _ in 0..CORRECTION_GATE_CAPACITY {
            assert!(gate.hold(edge));
        }
        assert!(!gate.hold(edge));
        assert_eq!(gate.held.len(), CORRECTION_GATE_CAPACITY);
        assert!(gate.held.iter().all(|event| event.message == WM_KEYUP));
    }

    #[test]
    fn abort_retains_all_held_input_without_a_worker() {
        let (mut state, _receiver) = test_gate_state(1);
        state.correction_gate.activate(10);
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0));
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYUP, VK_LCONTROL, 0, 0));
        state.break_correction_gate_fail_open(GateFailureCause::Deadline);
        // The Ctrl key-down reached the application before the gate, so its key-up is delivered
        // rather than retained; only the typed key waits for recovery.
        let retained = state.metrics.retained_input.lock().unwrap();
        assert_eq!(retained.as_ref().unwrap().events.len(), 1);
        assert_eq!(
            retained.as_ref().unwrap().events[0].virtual_key,
            u32::from(VIRTUAL_KEY(0x47).0)
        );
        assert_eq!(state.pending_key_releases.len(), 1);
        assert_eq!(
            state.pending_key_releases[0].virtual_key,
            u32::from(VK_LCONTROL.0)
        );
        assert!(!state.correction_gate.active);
        assert!(!state.metrics.auto_enabled.load(Ordering::Acquire));
        assert_eq!(state.metrics.conversion_failures.load(Ordering::Acquire), 1);
    }

    #[test]
    fn held_keys_nobody_recovers_are_deleted_after_their_lifetime() {
        let (state, receiver) = test_gate_state(4);
        let metrics = Arc::clone(&state.metrics);
        let key = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0);
        let lifetime = Duration::from_millis(RETAINED_INPUT_LIFETIME_MS);
        let start = Instant::now();
        *metrics.retained_input.lock().unwrap() = Some(RetainedInput {
            token: 10,
            foreground: key.foreground,
            events: vec![key],
            delivery_uncertain: false,
            expires_at: start + lifetime,
            recovering: false,
            retained_at: Instant::now(),
            auto_recover: false,
            input_mark: 0,
        });
        metrics.recovery_armed.store(true, Ordering::Release);
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        expire_retained_input(HWND::default(), start + lifetime - Duration::from_millis(1));
        assert!(has_retained_input(&metrics), "kept for its whole lifetime");
        assert!(metrics.recovery_armed.load(Ordering::Acquire));
        assert!(receiver.try_recv().is_err());
        expire_retained_input(HWND::default(), start + lifetime);
        assert!(!has_retained_input(&metrics));
        assert!(!metrics.recovery_armed.load(Ordering::Acquire));
        // The worker is told, so that automatic conversion can resume, and Exit is no longer
        // declined for held input.
        assert!(matches!(
            receiver.try_recv().unwrap().event,
            RawInputEvent::RetainedInputExpired
        ));
        let state = take_app_state().unwrap();
        assert!(state.decline_reasons().is_empty());
        // Nothing is deleted or reported twice.
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        expire_retained_input(HWND::default(), start + lifetime * 2);
        assert!(receiver.try_recv().is_err());
        drop(take_app_state());
    }

    #[test]
    fn preparing_a_recovery_gives_the_held_keys_a_full_lifetime_from_then() {
        let metrics = ObserverMetrics::default();
        let key = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0);
        let lifetime = Duration::from_millis(RETAINED_INPUT_LIFETIME_MS);
        let start = Instant::now();
        *metrics.retained_input.lock().unwrap() = Some(RetainedInput {
            token: 10,
            foreground: key.foreground,
            events: vec![key],
            delivery_uncertain: false,
            expires_at: start + lifetime,
            recovering: false,
            retained_at: Instant::now(),
            auto_recover: false,
            input_mark: 0,
        });
        let expiry = || {
            metrics
                .retained_input
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .expires_at
        };
        // A notice about another record prepares nothing.
        assert!(!prepare_retained_recovery(
            &metrics,
            11,
            start + lifetime / 2
        ));
        assert!(!metrics.recovery_armed.load(Ordering::Acquire));
        assert_eq!(expiry(), start + lifetime);
        assert!(prepare_retained_recovery(
            &metrics,
            10,
            start + lifetime / 2
        ));
        assert!(metrics.recovery_armed.load(Ordering::Acquire));
        assert_eq!(expiry(), start + lifetime / 2 + lifetime);
        // Preparing again never shortens it.
        assert!(prepare_retained_recovery(&metrics, 10, start));
        assert_eq!(expiry(), start + lifetime / 2 + lifetime);
        metrics.retained_input.lock().unwrap().take();
        assert!(!prepare_retained_recovery(&metrics, 10, start));
        assert!(!metrics.recovery_armed.load(Ordering::Acquire));
    }

    #[test]
    fn empty_expired_gate_does_not_disable_automatic_conversion() {
        let (mut state, _receiver) = test_gate_state(4);
        state.correction_gate.activate(10);
        state.break_correction_gate_fail_open(GateFailureCause::Deadline);
        assert!(state.metrics.auto_enabled.load(Ordering::Acquire));
        assert_eq!(state.metrics.conversion_failures.load(Ordering::Acquire), 0);
        assert!(!has_retained_input(&state.metrics));
    }

    #[test]
    fn late_empty_ack_closes_without_resetting_epoch_or_undo() {
        let (mut state, _receiver) = test_gate_state(4);
        state.correction_gate.activate(10);
        state.correction_gate.awaiting_worker_ack = true;
        state.metrics.undo_available.store(true, Ordering::Release);
        state.correction_gate.deadline = Instant::now() - Duration::from_secs(5);
        state.close_drained_gate();
        assert_eq!(state.metrics.input_epoch.load(Ordering::Acquire), 0);
        assert!(state.metrics.undo_available.load(Ordering::Acquire));
        assert!(state.metrics.auto_enabled.load(Ordering::Acquire));
    }

    #[test]
    fn repeated_pause_is_coalesced_and_deferred_request_is_undo_only() {
        let mut gate = CorrectionGate::default();
        gate.activate(10);
        let first = test_raw_key(WM_KEYDOWN, VK_PAUSE, 20, 0);
        gate.defer_hotkey(first);
        gate.defer_hotkey(test_raw_key(WM_KEYDOWN, VK_PAUSE, 21, 0));
        let pending = gate.take_pending_hotkey(Some(first.foreground)).unwrap();
        assert_eq!(pending.sequence, 20);
        assert_eq!(pending.drain_token, 10);
        assert!(pending.hotkey_trigger);
        assert!(gate.take_pending_hotkey(Some(first.foreground)).is_none());
    }

    #[test]
    fn deferred_pause_never_moves_to_a_different_control() {
        let mut gate = CorrectionGate::default();
        gate.activate(10);
        let event = test_raw_key(WM_KEYDOWN, VK_PAUSE, 20, 0);
        gate.defer_hotkey(event);
        let changed = ForegroundContext {
            focus: 99,
            ..event.foreground
        };
        assert!(gate.take_pending_hotkey(Some(changed)).is_none());
        assert!(gate.pending_hotkey.is_none());
    }

    #[test]
    fn periodic_privacy_requests_are_coalesced_and_do_not_overflow_input() {
        let (state, receiver) = test_gate_state(1);
        let foreground = test_raw_key(WM_KEYDOWN, VK_SPACE, 0, 0).foreground;
        for _ in 0..100 {
            assert!(state.enqueue(RawInputEvent::PrivacyRefresh(foreground)));
        }
        assert!(matches!(
            receiver.try_recv().unwrap().event,
            RawInputEvent::PrivacyRefresh(_)
        ));
        assert!(receiver.try_recv().is_err());
        assert_eq!(state.metrics.dropped_events.load(Ordering::Acquire), 0);
        assert_eq!(state.metrics.input_epoch.load(Ordering::Acquire), 0);
    }

    #[test]
    fn held_keypad_digits_are_replayed_as_their_virtual_keys() {
        // A digit typed on the keypad while the gate holds keys after a Space is replayed later.
        // As a bare scan code it would arrive as Down, End and the like.
        let mut digit = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(VK_NUMPAD0.0 + 2), 0, 0);
        digit.scan_code = 0x50;
        let mut digit_up = digit;
        digit_up.message = WM_KEYUP;
        let mut letter = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x41), 0, 0);
        letter.scan_code = 0x1e;
        let inputs = build_raw_replay_inputs(&[digit, digit_up, letter], INJECTED_EVENT_MARKER)
            .expect("replayable");
        let input = |index: usize| unsafe { inputs[index].Anonymous.ki };
        assert_eq!(input(0).wVk, VIRTUAL_KEY(VK_NUMPAD0.0 + 2));
        assert_eq!(input(0).wScan, 0x50);
        assert_eq!(input(0).dwFlags, Default::default());
        assert_eq!(input(1).wVk, VIRTUAL_KEY(VK_NUMPAD0.0 + 2));
        assert_eq!(input(1).dwFlags, KEYEVENTF_KEYUP);
        // Every other key keeps its scan code.
        assert_eq!(input(2).wVk, VIRTUAL_KEY(0));
        assert_eq!(input(2).dwFlags, KEYEVENTF_SCANCODE);
        for keypad in (0..=9)
            .map(|digit| VK_NUMPAD0.0 + digit)
            .chain([VK_DECIMAL.0])
        {
            assert!(is_keypad_digit_key(u32::from(keypad)), "{keypad:#x}");
        }
        for other in [0x30, 0x41, VK_END.0, VK_DOWN.0, 0x6b, 0x6d] {
            assert!(!is_keypad_digit_key(u32::from(other)), "{other:#x}");
        }
    }

    #[test]
    fn recovery_balances_an_unmatched_modifier_and_never_adds_a_fence() {
        let mut edge = test_raw_key(WM_KEYDOWN, VK_LWIN, 0, 0);
        edge.scan_code = 0x5b;
        edge.extended = true;
        let inputs = build_recovery_inputs(&[edge]).unwrap();
        assert_eq!(inputs.len(), 2);
        let up = unsafe { inputs[1].Anonymous.ki };
        assert_eq!(
            up.dwFlags,
            KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP
        );
        assert!(
            inputs
                .iter()
                .all(|input| unsafe { input.Anonymous.ki.dwExtraInfo } == INJECTED_EVENT_MARKER)
        );
    }

    #[test]
    fn cancelled_token_cannot_become_current_again_after_gate_is_opened() {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.observed_input_sequence.store(10, Ordering::Release);
        metrics
            .forwarded_input_sequence
            .store(10, Ordering::Release);
        metrics.cancelled_gate_token.store(10, Ordering::Release);
        let processor = InputProcessor::new(metrics, 0);
        assert!(!processor.pre_forward_input_guard_is_current(10));
        assert!(!processor.input_guard_is_current(10));
    }

    #[test]
    fn partial_submission_preserves_prefix_and_suffix_but_marks_delivery_uncertain() {
        let (mut state, _receiver) = test_gate_state(8);
        state.correction_gate.activate(10);
        let first = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0);
        // A key-down: a lone key-up of a key pressed before the gate would be delivered instead.
        let last = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x48), 0, 0);
        state.correction_gate.held.push(last);
        state.retain_failed_drain(10, vec![first], 1);
        let retained = state.metrics.retained_input.lock().unwrap();
        let retained = retained.as_ref().unwrap();
        assert_eq!(retained.events.len(), 2);
        assert_eq!(retained.events[0].virtual_key, first.virtual_key);
        assert_eq!(retained.events[1].virtual_key, last.virtual_key);
        assert!(retained.delivery_uncertain);
    }

    #[test]
    fn cancellation_during_send_does_not_lose_the_unsent_prefix() {
        let (mut state, _receiver) = test_gate_state(8);
        state.correction_gate.activate(10);
        let first = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0);
        // A key-down: a lone key-up of a key pressed before the gate would be delivered instead.
        let last = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x48), 0, 0);
        state.correction_gate.held.push(last);
        state.break_correction_gate_fail_open(GateFailureCause::Mouse);
        state.retain_failed_drain(10, vec![first], 0);
        let retained = state.metrics.retained_input.lock().unwrap();
        let retained = retained.as_ref().unwrap();
        assert_eq!(retained.events.len(), 2);
        assert_eq!(retained.events[0].virtual_key, first.virtual_key);
        assert_eq!(retained.events[1].virtual_key, last.virtual_key);
        assert!(!retained.delivery_uncertain);
    }

    #[test]
    fn gate_cancellation_notification_does_not_count_or_pause_twice() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        metrics.auto_enabled.store(true, Ordering::Release);
        processor.process_gate_drain_ready(10, 0, false);
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
        assert_eq!(metrics.conversion_failures.load(Ordering::Acquire), 0);
    }

    fn test_clipboard_formats(owner: HWND) -> Option<Vec<u32>> {
        let _open = ClipboardOpenGuard::open(owner, CLIPBOARD_OPEN_RETRIES)?;
        let mut formats = Vec::new();
        let mut previous = 0;
        loop {
            unsafe {
                SetLastError(ERROR_SUCCESS);
            }
            let format = unsafe { EnumClipboardFormats(previous) };
            if format == 0 {
                return (unsafe { GetLastError() } == ERROR_SUCCESS).then_some(formats);
            }
            formats.push(format);
            previous = format;
        }
    }

    fn test_clipboard_unicode(owner: HWND) -> Option<String> {
        let _open = ClipboardOpenGuard::open(owner, CLIPBOARD_OPEN_RETRIES)?;
        let handle = unsafe { GetClipboardData(u32::from(CF_UNICODETEXT.0)).ok()? };
        let memory = HGLOBAL(handle.0);
        let text = unsafe { GlobalLock(memory) }.cast::<u16>();
        if text.is_null() {
            return None;
        }
        let mut length = 0usize;
        while length < 32_768 && unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        let value = (length < 32_768)
            .then(|| String::from_utf16(unsafe { core::slice::from_raw_parts(text, length) }).ok())
            .flatten();
        unsafe {
            let _ = GlobalUnlock(memory);
        }
        value
    }

    #[test]
    fn decodes_signed_version_four_interaction_coordinates() {
        let x = -12i16;
        let y = 345i16;
        let packed = u32::from(x as u16) | (u32::from(y as u16) << 16);
        let point = point_from_packed_word(WPARAM(packed as usize));
        assert_eq!(point.x, i32::from(x));
        assert_eq!(point.y, i32::from(y));
    }

    #[test]
    fn observes_queue_epoch_before_processing_the_next_event() {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.input_epoch.store(7, Ordering::Release);
        let mut processor = InputProcessor::new(metrics, 0);
        processor.process(QueuedInputEvent {
            configuration: None,
            captured_at: Instant::now(),
            epoch: 7,
            event: RawInputEvent::Mouse,
        });
        assert_eq!(processor.last_input_epoch, 7);
    }

    #[test]
    fn mode_epoch_reset_does_not_suppress_the_first_new_word() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        processor.set_privacy_reason(None);

        metrics.input_epoch.store(1, Ordering::Release);
        processor.reset_for_epoch(1);

        assert!(!processor.session.is_suppressed());
        assert!(processor.privacy_reason.is_none());
        assert!(processor.privacy_needs_check);
    }

    // Requires the legacy-bundled RU/ET data; the English-only base variant omits it.
    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn privacy_recovery_never_converts_a_partial_word_but_allows_the_next_word() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(metrics, 0);
        processor.set_privacy_reason(None);
        for character in "gh".chars() {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
        }
        processor.set_privacy_reason(Some(PrivacyBlockReason::InspectionUnavailable));
        assert_eq!(processor.session.buffered_character_count(), 0);
        assert!(processor.replay_keys.is_empty());
        processor.set_privacy_reason(None);
        for character in "bdtn".chars() {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
        }
        assert_eq!(
            processor.session.handle(
                InputEvent::Boundary,
                Some(Language::English),
                &processor.detector
            ),
            SessionAction::None
        );
        for character in "ghbdtn".chars() {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
        }
        assert!(matches!(
            processor.session.handle(
                InputEvent::Boundary,
                Some(Language::English),
                &processor.detector
            ),
            SessionAction::Candidate(_)
        ));
    }

    #[test]
    fn real_queue_overflow_keeps_the_current_word_suppressed() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        processor.set_privacy_reason(None);

        metrics.dropped_events.store(1, Ordering::Release);
        metrics.input_epoch.store(1, Ordering::Release);
        processor.reset_for_epoch(1);

        assert!(processor.session.is_suppressed());
        assert!(processor.privacy_reason.is_some());
    }

    #[test]
    fn the_tray_status_selects_the_icon_visual() {
        let initial = TrayStatus::initial(LayoutIndicator::English);
        assert_eq!(tray_visual_state(initial), TrayVisual::Disabled);
        let active = TrayStatus {
            auto_enabled: true,
            ..initial
        };
        assert_eq!(tray_visual_state(active), TrayVisual::Active);
        for privacy_reason in [PRIVACY_PASSWORD, PRIVACY_UNAVAILABLE, PRIVACY_EXCLUDED] {
            assert_eq!(
                tray_visual_state(TrayStatus {
                    privacy_reason,
                    ..active
                }),
                TrayVisual::Active
            );
        }
        assert_eq!(
            tray_visual_state(TrayStatus {
                safety_paused: true,
                ..initial
            }),
            TrayVisual::SafetyPaused
        );
        assert_eq!(
            tray_visual_state(TrayStatus {
                failures: 5,
                ..initial
            }),
            TrayVisual::Disabled
        );
    }

    #[test]
    fn frameless_icons_can_be_created_and_destroyed_in_every_state() {
        for state in [
            TrayVisual::Active,
            TrayVisual::Disabled,
            TrayVisual::SafetyPaused,
        ] {
            for (indicator, label) in [
                (LayoutIndicator::English, "EN"),
                (LayoutIndicator::Russian, "RU"),
                (LayoutIndicator::Estonian, "ET"),
                (LayoutIndicator::Japanese, "JA"),
                (LayoutIndicator::Unavailable, "??"),
                (LayoutIndicator::Other(0x407), "??"),
            ] {
                assert_eq!(indicator.icon_text(), label);
                let icon = create_indicator_icon(state, indicator).expect("icon should be created");
                unsafe {
                    DestroyIcon(icon).expect("test icon should be destroyable");
                }
            }
        }
    }

    /// The color pixels (top-down, 0xAARRGGBB) and the mask bytes of a 32×32 icon.
    fn icon_rasters(icon: HICON) -> (Vec<u32>, Vec<u8>) {
        use windows::Win32::Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetBitmapBits, GetDC, GetDIBits,
            ReleaseDC,
        };
        use windows::Win32::UI::WindowsAndMessaging::GetIconInfo;
        let mut info = ICONINFO::default();
        unsafe { GetIconInfo(icon, &mut info) }.expect("icon info");
        let mut color = vec![0u32; 32 * 32];
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: 32,
                biHeight: -32,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut mask = vec![0u8; 32 * 32 / 8];
        unsafe {
            let screen = GetDC(None);
            let rows = GetDIBits(
                screen,
                info.hbmColor,
                0,
                32,
                Some(color.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, screen);
            assert_eq!(rows, 32, "color rows");
            // A monochrome bitmap's rows are padded to 16 bits; 32 pixels are four bytes.
            let read = GetBitmapBits(info.hbmMask, mask.len() as i32, mask.as_mut_ptr().cast());
            assert_eq!(read, mask.len() as i32, "mask bytes");
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        }
        (color, mask)
    }

    #[test]
    fn the_tray_icon_carries_the_rendered_pixels_and_a_mask_that_matches_them() {
        let mut rasters = Vec::new();
        for (visual, indicator) in [
            (TrayVisual::Active, LayoutIndicator::English),
            (TrayVisual::Disabled, LayoutIndicator::English),
            (TrayVisual::SafetyPaused, LayoutIndicator::Russian),
        ] {
            let expected = tray_visual::pixels(visual, indicator.icon_text());
            let icon = create_indicator_icon(visual, indicator).expect("icon");
            let (color, mask) = icon_rasters(icon);
            unsafe {
                DestroyIcon(icon).expect("destroy icon");
            }
            let case = format!("{visual:?} {indicator:?}");
            assert_eq!(color, expected, "{case}");
            // A set mask bit is a transparent pixel: exactly those whose alpha is zero. The badge
            // is opaque, so a set bit would punch a hole into it.
            for (index, pixel) in expected.iter().enumerate() {
                let transparent = mask[index / 8] & (0x80 >> (index % 8)) != 0;
                assert_eq!(transparent, pixel >> 24 == 0, "{case} pixel {index}");
            }
            rasters.push(color);
        }
        // The mode and the layout show in the pixels.
        assert_ne!(rasters[0], rasters[1]);
        assert_ne!(rasters[1], rasters[2]);
    }

    /// One call to the scripted notification area: the message, its flags, the icon image, the
    /// tooltip and the answer.
    #[derive(Debug, Clone)]
    struct ShellCall {
        message: NOTIFY_ICON_MESSAGE,
        flags: windows::Win32::UI::Shell::NOTIFY_ICON_DATA_FLAGS,
        icon: usize,
        tip: String,
        answer: bool,
    }

    /// A notification area for the tray code of the calling thread. It records every call, keeps
    /// the ids it shows, fails calls on request, and can run a message in the middle of a call.
    #[derive(Default)]
    struct ScriptedShell {
        calls: Vec<ShellCall>,
        icons: std::collections::BTreeSet<u32>,
        /// The next calls of these messages fail, in order, with these errors.
        failures: VecDeque<(NOTIFY_ICON_MESSAGE, windows::Win32::Foundation::WIN32_ERROR)>,
        /// Runs inside the next call of the message, like a message that the call dispatches.
        during: Option<(NOTIFY_ICON_MESSAGE, Box<dyn FnOnce()>)>,
        /// Passes every call on to the real notification area and records its answer.
        real: bool,
    }

    thread_local! {
        static SCRIPTED_SHELL: RefCell<Option<ScriptedShell>> = const { RefCell::new(None) };
    }

    fn wide_text(buffer: &[u16]) -> String {
        let length = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..length])
    }

    /// The scripted notification area's answer, or `None` when the thread uses the real one.
    pub(super) fn scripted_shell_call(
        message: NOTIFY_ICON_MESSAGE,
        data: &NOTIFYICONDATAW,
    ) -> Option<bool> {
        let (real, failure, during) = SCRIPTED_SHELL.with(|slot| {
            let mut slot = slot.borrow_mut();
            let shell = slot.as_mut()?;
            let during = shell
                .during
                .take_if(|(expected, _)| *expected == message)
                .map(|(_, run)| run);
            let failure = shell
                .failures
                .iter()
                .position(|(failing, _)| *failing == message)
                .and_then(|index| shell.failures.remove(index))
                .map(|(_, error)| error);
            Some((shell.real, failure, during))
        })?;
        // The real shell may dispatch messages that call it again: no borrow is held meanwhile.
        let answer = failure.is_none()
            && if real {
                unsafe { Shell_NotifyIconW(message, data) }.as_bool()
            } else {
                scripted_shell(|shell| match message {
                    NIM_ADD => shell.icons.insert(data.uID),
                    NIM_DELETE => shell.icons.remove(&data.uID),
                    _ => shell.icons.contains(&data.uID),
                })
            };
        scripted_shell(|shell| {
            shell.calls.push(ShellCall {
                message,
                flags: data.uFlags,
                icon: data.hIcon.0 as usize,
                tip: wide_text(&data.szTip),
                answer,
            });
        });
        // A message dispatched during the call runs with the shell released: it may call it.
        if let Some(during) = during {
            during();
        }
        if let Some(error) = failure {
            unsafe { windows::Win32::Foundation::SetLastError(error) };
        }
        Some(answer)
    }

    /// Runs `body` with `shell` as this thread's notification area, and gives the shell back.
    fn with_scripted_shell<T>(
        shell: ScriptedShell,
        body: impl FnOnce() -> T,
    ) -> (T, ScriptedShell) {
        SCRIPTED_SHELL.with(|slot| *slot.borrow_mut() = Some(shell));
        let result = body();
        let shell = SCRIPTED_SHELL
            .with(|slot| slot.borrow_mut().take())
            .expect("scripted shell");
        (result, shell)
    }

    #[test]
    fn a_failed_call_reports_its_own_error_and_not_the_cleanup_s() {
        use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, SetLastError};
        unsafe { SetLastError(ERROR_TIMEOUT) };
        let error = last_error_then(|| unsafe { SetLastError(ERROR_ACCESS_DENIED) });
        assert_eq!(error.code(), ERROR_TIMEOUT.to_hresult());
        // The shell refuses the icon's version, and deleting the half-added icon leaves another
        // error behind: the registration reports the refusal, and no icon is left.
        let shell = ScriptedShell {
            failures: VecDeque::from([(NIM_SETVERSION, ERROR_TIMEOUT)]),
            during: Some((
                NIM_DELETE,
                Box::new(|| unsafe { SetLastError(ERROR_ACCESS_DENIED) }),
            )),
            ..ScriptedShell::default()
        };
        let (added, shell) = with_scripted_shell(shell, || {
            TrayIcon::add(
                HWND::default(),
                TrayStatus::initial(LayoutIndicator::English),
            )
            .map(|tray| tray.icon)
        });
        assert_eq!(
            added.err().map(|error| error.code()),
            Some(ERROR_TIMEOUT.to_hresult())
        );
        assert!(shell.icons.is_empty());
        let messages: Vec<_> = shell.calls.iter().map(|call| call.message).collect();
        assert_eq!(messages, [NIM_ADD, NIM_SETVERSION, NIM_DELETE]);
    }

    #[test]
    fn every_icon_call_asks_the_shell_for_the_standard_tooltip() {
        let ((), shell) = with_scripted_shell(ScriptedShell::default(), || {
            let mut tray = TrayIcon::add(
                HWND::default(),
                TrayStatus::initial(LayoutIndicator::English),
            )
            .expect("add");
            let status = TrayStatus {
                auto_enabled: true,
                ..TrayStatus::initial(LayoutIndicator::Russian)
            };
            tray.update(status).expect("update");
            // A balloon is a modify call too.
            let (mut state, _receiver) = test_gate_state(1);
            state.tray = Some(tray);
            APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
            show_tray_information(HWND::default(), "AutoKeyboardLayot", "notice");
            drop(take_app_state());
        });
        let calls: Vec<_> = shell
            .calls
            .iter()
            .filter(|call| [NIM_ADD, NIM_MODIFY].contains(&call.message))
            .map(|call| (call.message, call.flags.contains(NIF_SHOWTIP)))
            .collect();
        assert_eq!(
            calls,
            [(NIM_ADD, true), (NIM_MODIFY, true), (NIM_MODIFY, true)]
        );
    }

    #[test]
    fn a_re_added_icon_shows_the_last_status_and_announces_no_old_failure() {
        // A conversion failed and paused automatic conversion, which the tray announced once.
        // Explorer restarts: the new icon shows the failure in its tooltip, and the next refresh
        // has nothing to announce.
        let (state, _receiver) = test_gate_state(1);
        state.metrics.auto_enabled.store(false, Ordering::Release);
        state
            .metrics
            .conversion_failures
            .store(1, Ordering::Release);
        state
            .metrics
            .last_failure_reason
            .store(ConversionFailureReason::TextCommit as u8, Ordering::Release);
        let indicator = LayoutIndicator::English;
        let start = Instant::now();
        let ((), shell) = with_scripted_shell(ScriptedShell::default(), || {
            APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
            assert!(add_tray(HWND::default(), TrayStatus::initial(indicator)));
            update_tray(indicator, start);
            recreate_tray(HWND::default());
            update_tray(indicator, start + Duration::from_secs(1));
            drop(take_app_state());
        });
        let balloons = shell
            .calls
            .iter()
            .filter(|call| call.flags.contains(NIF_INFO))
            .count();
        assert_eq!(balloons, 1, "{:?}", shell.calls);
        let added: Vec<_> = shell
            .calls
            .iter()
            .filter(|call| call.message == NIM_ADD)
            .map(|call| call.tip.as_str())
            .collect();
        assert_eq!(added.len(), 2);
        assert!(!added[0].contains("conversion failures"), "{}", added[0]);
        assert!(added[1].contains("conversion failures: 1"), "{}", added[1]);
    }

    /// Runs `body` with this thread's scripted notification area.
    fn scripted_shell<T>(body: impl FnOnce(&mut ScriptedShell) -> T) -> T {
        SCRIPTED_SHELL.with(|slot| body(slot.borrow_mut().as_mut().expect("scripted shell")))
    }

    fn icon_exists(icon: usize) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::GetIconInfo;
        let mut info = ICONINFO::default();
        let exists = unsafe { GetIconInfo(HICON(icon as *mut c_void), &mut info) }.is_ok();
        if exists {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
                let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
            }
        }
        exists
    }

    #[test]
    fn an_icon_added_again_during_an_update_stays_in_the_shell() {
        // Explorer restarts while an update's shell call runs, and the new taskbar's message adds
        // the icon again in the middle of it. The updated, stale icon must not delete the new
        // one, which the shell knows by the same id.
        let (state, _receiver) = test_gate_state(1);
        let indicator = LayoutIndicator::English;
        let ((shown, stored), shell) = with_scripted_shell(ScriptedShell::default(), || {
            APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
            assert!(add_tray(HWND::default(), TrayStatus::initial(indicator)));
            scripted_shell(|shell| {
                shell.during = Some((
                    NIM_MODIFY,
                    Box::new(|| {
                        scripted_shell(|shell| shell.icons.clear());
                        recreate_tray(HWND::default());
                    }),
                ));
            });
            update_tray(indicator, Instant::now());
            let shown = scripted_shell(|shell| shell.icons.contains(&TRAY_ID));
            let stored = APP_STATE.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .and_then(|state| state.tray.as_ref())
                    .map(|tray| tray.icon.0 as usize)
            });
            drop(take_app_state());
            (shown, stored)
        });
        let messages: Vec<_> = shell.calls.iter().map(|call| call.message).collect();
        assert_eq!(
            messages,
            [
                NIM_ADD,
                NIM_SETVERSION,
                NIM_MODIFY,
                NIM_ADD,
                NIM_SETVERSION,
                NIM_DELETE
            ],
            "only the final teardown deletes"
        );
        assert!(shown, "the new icon was deleted");
        // The state keeps the new icon, and the stale one's image is destroyed.
        assert_eq!(stored, Some(shell.calls[3].icon));
        assert!(!icon_exists(shell.calls[2].icon));
    }

    #[test]
    fn an_icon_added_while_another_is_stored_gives_up_its_image_only() {
        // An addition that finds an icon stored already (a nested addition won) keeps that one:
        // the shell shows one icon per id, and deleting the extra one would remove it.
        let (state, _receiver) = test_gate_state(1);
        let indicator = LayoutIndicator::English;
        let ((), shell) = with_scripted_shell(ScriptedShell::default(), || {
            APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
            assert!(add_tray(HWND::default(), TrayStatus::initial(indicator)));
            // A restarted shell shows nothing yet, so the second addition succeeds.
            scripted_shell(|shell| shell.icons.clear());
            assert!(add_tray(HWND::default(), TrayStatus::initial(indicator)));
            assert!(scripted_shell(|shell| shell.icons.contains(&TRAY_ID)));
            drop(take_app_state());
        });
        let added: Vec<_> = shell
            .calls
            .iter()
            .filter(|call| call.message == NIM_ADD)
            .map(|call| call.icon)
            .collect();
        assert_eq!(added.len(), 2);
        assert!(!icon_exists(added[1]), "the extra icon's image leaked");
        let deletes = shell
            .calls
            .iter()
            .filter(|call| call.message == NIM_DELETE)
            .count();
        assert_eq!(deletes, 1, "only the final teardown deletes");
    }

    #[test]
    fn an_icon_the_shell_refuses_is_added_by_a_later_retry() {
        use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
        // The agent starts before the taskbar exists: the shell refuses the icon, and the agent
        // goes on without it until a later timer tick adds it.
        let (state, _receiver) = test_gate_state(1);
        let shell = ScriptedShell {
            failures: VecDeque::from([(NIM_ADD, ERROR_FILE_NOT_FOUND)]),
            ..ScriptedShell::default()
        };
        let stored = || APP_STATE.with(|slot| slot.borrow().as_ref().unwrap().tray.is_some());
        let ((), shell) = with_scripted_shell(shell, || {
            APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
            assert!(!add_tray(
                HWND::default(),
                TrayStatus::initial(LayoutIndicator::English)
            ));
            assert!(!stored());
            // Not due yet: nothing is tried.
            retry_tray(HWND::default());
            assert_eq!(scripted_shell(|shell| shell.calls.len()), 1);
            UI_GUARDS.with(|guards| {
                guards
                    .borrow_mut()
                    .tray_retry
                    .as_mut()
                    .expect("retry")
                    .next_attempt = Instant::now();
            });
            retry_tray(HWND::default());
            assert!(stored());
            assert!(UI_GUARDS.with(|guards| guards.borrow().tray_retry.is_none()));
            drop(take_app_state());
        });
        let messages: Vec<_> = shell.calls.iter().map(|call| call.message).collect();
        assert_eq!(messages, [NIM_ADD, NIM_ADD, NIM_SETVERSION, NIM_DELETE]);
    }

    /// A window of the calling thread with the agent's window procedure. It goes with the thread.
    fn agent_procedure_window() -> HWND {
        let module = unsafe { GetModuleHandleW(None).expect("module") };
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: w!("AutoKeyboardLayotProcedureTest"),
            ..Default::default()
        };
        // A second registration fails harmlessly: the class is the same.
        unsafe { RegisterClassW(&class) };
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("AutoKeyboardLayotProcedureTest"),
                w!("AutoKeyboardLayot procedure test"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
            .expect("window")
        }
    }

    /// Takes a WM_QUIT waiting in this thread's queue; true when there was one.
    fn take_quit_message() -> bool {
        let mut message = MSG::default();
        unsafe { PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE) }.as_bool()
    }

    #[test]
    fn a_failed_start_leaves_no_quit_message_for_the_error_dialog() {
        // Destroying the agent's window posts WM_QUIT for its message loop.
        let window = agent_procedure_window();
        unsafe { DestroyWindow(window).expect("destroy window") };
        assert!(take_quit_message());
        // A failed start never ran that loop: it takes the message back, so the dialog that main
        // shows next is not closed by it.
        abandon_startup(agent_procedure_window());
        assert!(!take_quit_message());
    }

    #[test]
    fn right_click_opens_menu_and_left_click_opens_settings() {
        // The menu opens where the shell says: the icon's place on a monitor left of the primary
        // one, as for the menu key, never wherever the pointer happens to be.
        let anchor = WPARAM((usize::from(860u16) << 16) | usize::from((-120i16) as u16));
        assert_eq!(
            tray_interaction(WM_CONTEXTMENU, anchor),
            TrayInteraction::QuickMenu(POINT { x: -120, y: 860 })
        );
        assert_eq!(
            tray_interaction(NIN_KEYSELECT, anchor),
            TrayInteraction::Settings
        );
        assert_eq!(
            tray_interaction(NIN_SELECT, anchor),
            TrayInteraction::Settings
        );
        assert_eq!(tray_interaction(WM_TIMER, anchor), TrayInteraction::Ignore);
    }

    #[test]
    fn the_tray_menu_is_destroyed_on_every_return() {
        use windows::Win32::System::Threading::{
            GR_USEROBJECTS, GetCurrentProcess, GetGuiResources,
        };
        use windows::Win32::UI::WindowsAndMessaging::IsMenu;
        let menu = PopupMenu::new().expect("menu");
        let handle = menu.0;
        assert!(unsafe { IsMenu(handle) }.as_bool());
        drop(menu);
        assert!(!unsafe { IsMenu(handle) }.as_bool());
        // Without the agent's state (it is stopping) the menu returns early, leaving nothing.
        let objects = || unsafe { GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS) };
        let before = objects();
        show_tray_menu_inner(HWND::default(), POINT::default()).expect("early return");
        assert_eq!(objects(), before);
    }

    #[test]
    fn failure_notice_is_not_repeated_or_shown_for_manual_disable() {
        let before = TrayStatus::initial(LayoutIndicator::English);
        let failed = TrayStatus {
            failures: 1,
            failure_reason: ConversionFailureReason::ClipboardEdit as u8,
            ..before
        };
        assert!(should_notify_conversion_failure(before, failed));
        assert!(!should_notify_conversion_failure(failed, failed));
        assert!(!should_notify_conversion_failure(before, before));
        assert!(!should_notify_conversion_failure(
            before,
            TrayStatus {
                auto_enabled: true,
                ..failed
            }
        ));
    }

    #[test]
    fn tray_tooltip_exposes_privacy_pause_without_text_content() {
        let paused = tray_tooltip(TrayStatus {
            indicator: LayoutIndicator::English,
            ui_revision: 0,
            candidates: 2,
            dropped: 0,
            privacy_reason: PRIVACY_PASSWORD,
            auto_enabled: false,
            safety_paused: false,
            undo_available: false,
            failures: 0,
            failure_reason: ConversionFailureReason::None as u8,
            backend_status: BACKEND_CAPABILITY_UNSUPPORTED,
            diagnostic_errors: 0,
        });
        let active = tray_tooltip(TrayStatus {
            indicator: LayoutIndicator::English,
            ui_revision: 0,
            candidates: 2,
            dropped: 0,
            privacy_reason: PRIVACY_ALLOWED,
            auto_enabled: true,
            safety_paused: false,
            undo_available: true,
            failures: 0,
            failure_reason: ConversionFailureReason::None as u8,
            backend_status: BACKEND_PROTECTED_PASTE,
            diagnostic_errors: 0,
        });
        let failed = tray_tooltip(TrayStatus {
            indicator: LayoutIndicator::English,
            ui_revision: 0,
            candidates: 11,
            dropped: 0,
            privacy_reason: PRIVACY_ALLOWED,
            auto_enabled: false,
            safety_paused: true,
            undo_available: false,
            failures: 1,
            failure_reason: ConversionFailureReason::TextCommit as u8,
            backend_status: BACKEND_PHYSICAL_REPLAY,
            diagnostic_errors: 2,
        });
        assert!(paused.contains("privacy paused (password)"));
        assert!(!active.contains("privacy paused"));
        assert!(paused.contains("candidates: 2"));
        assert!(active.starts_with("Automatic conversion is on"));
        assert!(paused.starts_with("Automatic conversion is off"));
        assert!(failed.starts_with("Safety pause: text-commit"));
        assert!(active.contains("undo ready"));
        assert!(paused.contains("backend: unsupported"));
        assert!(active.contains("backend: uia-paste"));
        assert!(failed.contains("backend: physical-replay"));
        assert!(failed.contains("conversion failures: 1 (last: text-commit)"));
        assert!(failed.ends_with("; log errors: 2"));
        assert!(!active.contains("log errors"));
        assert_eq!(
            conversion_failure_label(ConversionFailureReason::ClipboardEdit as u8),
            "clipboard-edit"
        );
    }

    #[test]
    fn localized_tray_text_never_truncates_half_a_surrogate_pair() {
        let mut short = [42u16; 3];
        set_wide_text(&mut short, "A😀");
        assert_eq!(short, [b'A' as u16, 0, 0]);
        let mut complete = [0u16; 4];
        set_wide_text(&mut complete, "A😀B");
        assert_eq!(String::from_utf16(&complete[..3]).unwrap(), "A😀");
        assert_eq!(complete[3], 0);
        set_wide_text(&mut [0u16; 0], "text");
    }

    #[test]
    fn a_full_diagnostics_log_is_moved_aside_over_the_previous_one() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("diagnostics.log");
        let rotated = directory.path().join("diagnostics.1.log");
        fs::write(&rotated, "previous rotated log\r\n").unwrap();
        // Each line is 30 bytes: a 13-digit timestamp, a space, the record and CR LF.
        for index in 0..4 {
            assert_eq!(
                append_diagnostic_record(&path, &format!("event=test n={index}"), 64).unwrap(),
                DiagnosticAppend::Appended
            );
        }
        let rotated_text = fs::read_to_string(&rotated).unwrap();
        let active_text = fs::read_to_string(&path).unwrap();
        assert!(!rotated_text.contains("previous rotated log"));
        assert!(rotated_text.contains("n=0") && rotated_text.contains("n=1"));
        assert!(active_text.contains("n=2") && active_text.contains("n=3"));
        assert!(!active_text.contains("n=1"));
    }

    #[test]
    fn a_log_that_cannot_be_moved_aside_keeps_taking_records_within_twice_the_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("diagnostics.log");
        // A directory where the rotated log belongs makes every move fail, as a viewer that
        // holds the log open without sharing delete access does.
        fs::create_dir_all(directory.path().join("diagnostics.1.log").join("blocker")).unwrap();
        let mut outcomes = Vec::new();
        for index in 0..6 {
            outcomes.push(
                append_diagnostic_record(&path, &format!("event=test n={index}"), 64).unwrap(),
            );
            assert!(fs::metadata(&path).unwrap().len() <= 128);
        }
        assert_eq!(
            outcomes,
            [
                DiagnosticAppend::Appended,
                DiagnosticAppend::Appended,
                DiagnosticAppend::AppendedPastLimit,
                DiagnosticAppend::AppendedPastLimit,
                DiagnosticAppend::Restarted,
                DiagnosticAppend::Appended,
            ]
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("n=4") && text.contains("n=5"));
        assert!(!text.contains("n=3"));
    }

    #[test]
    fn the_diagnostics_writer_counts_what_it_could_not_write_for_the_tray() {
        let metrics = ObserverMetrics::default();
        let directory = tempfile::tempdir().unwrap();
        // The log's directory is a file: no record can be written.
        let blocked = directory.path().join("blocked");
        fs::write(&blocked, "").unwrap();
        let (sender, receiver) = sync_channel(4);
        sender.send("event=test n=1".to_owned()).unwrap();
        sender.send("event=test n=2".to_owned()).unwrap();
        drop(sender);
        diagnostic_worker(receiver, Some(blocked.join("diagnostics.log")), &metrics);
        assert_eq!(metrics.diagnostic_log_errors.load(Ordering::Relaxed), 2);

        // A full log that cannot be moved aside still takes the record, and the failed move is
        // counted.
        let path = directory.path().join("diagnostics.log");
        fs::write(&path, vec![b'x'; DIAGNOSTIC_LOG_MAX_BYTES as usize]).unwrap();
        fs::create_dir_all(directory.path().join("diagnostics.1.log").join("blocker")).unwrap();
        let (sender, receiver) = sync_channel(1);
        sender.send("event=test n=3".to_owned()).unwrap();
        drop(sender);
        diagnostic_worker(receiver, Some(path.clone()), &metrics);
        assert_eq!(metrics.diagnostic_log_errors.load(Ordering::Relaxed), 3);
        assert!(fs::read_to_string(&path).unwrap().contains("n=3"));

        // Without a log path every record is lost.
        let (sender, receiver) = sync_channel(1);
        sender.send("event=test n=4".to_owned()).unwrap();
        drop(sender);
        diagnostic_worker(receiver, None, &metrics);
        assert_eq!(metrics.diagnostic_log_errors.load(Ordering::Relaxed), 4);

        metrics.dropped_diagnostics.store(5, Ordering::Relaxed);
        assert_eq!(diagnostic_error_count(&metrics), 9);
    }

    #[test]
    fn ui_language_revision_invalidates_an_otherwise_unchanged_tray_status() {
        let before = TrayStatus::initial(LayoutIndicator::English);
        let after = TrayStatus {
            ui_revision: 1,
            ..before
        };
        assert_ne!(before, after);
        assert_eq!(tray_visual_state(before), tray_visual_state(after));
        assert!(!should_notify_conversion_failure(before, after));
    }

    #[test]
    fn protected_paste_chord_is_marked_and_balanced() {
        let inputs = build_paste_inputs();
        let control_down = unsafe { inputs[0].Anonymous.ki };
        let paste_down = unsafe { inputs[1].Anonymous.ki };
        let paste_up = unsafe { inputs[2].Anonymous.ki };
        let control_up = unsafe { inputs[3].Anonymous.ki };

        assert_eq!(control_down.wVk, VK_LCONTROL);
        assert_eq!(control_down.dwFlags, Default::default());
        assert_eq!(paste_down.wVk, VK_V_KEY);
        assert_eq!(paste_down.dwFlags, Default::default());
        assert_eq!(paste_up.wVk, VK_V_KEY);
        assert_eq!(paste_up.dwFlags, KEYEVENTF_KEYUP);
        assert_eq!(control_up.wVk, VK_LCONTROL);
        assert_eq!(control_up.dwFlags, KEYEVENTF_KEYUP);
        for input in inputs {
            assert_eq!(
                unsafe { input.Anonymous.ki }.dwExtraInfo,
                INJECTED_EVENT_MARKER
            );
        }
    }

    /// An invisible window owned by this thread, to open the clipboard with.
    fn clipboard_test_window(title: PCWSTR) -> HWND {
        let module = unsafe { GetModuleHandleW(None).expect("module") };
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                title,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
            .expect("clipboard test window")
        }
    }

    /// Puts `text` on the clipboard, owned by `owner`.
    fn put_test_clipboard_text(owner: HWND, text: &str) {
        let _open = ClipboardOpenGuard::open(owner, CLIPBOARD_OPEN_RETRIES).expect("open");
        unsafe { EmptyClipboard() }.expect("empty");
        let mut data = global_utf16(text).expect("text");
        assert!(data.transfer(u32::from(CF_UNICODETEXT.0)));
    }

    fn sorted(mut formats: Vec<u32>) -> Vec<u32> {
        formats.sort_unstable();
        formats.dedup();
        formats
    }

    #[test]
    #[ignore = "uses the real clipboard: run on the test VM with --include-ignored"]
    fn protected_clipboard_round_trip_on_the_real_clipboard() {
        let _com = PrivacyGuard::new();
        // The clipboard's owner must answer when the clipboard is emptied; this thread waits for
        // the broker meanwhile, so the original comes from a window that is gone by then.
        let source = clipboard_test_window(w!("AutoKeyboardLayot clipboard source"));
        put_test_clipboard_text(source, "akl-original");
        unsafe {
            DestroyWindow(source).expect("destroy source window");
        }
        let window = clipboard_test_window(w!("AutoKeyboardLayot clipboard probe"));
        let original_formats = test_clipboard_formats(window).expect("clipboard formats");

        let brokers = ClipboardBrokers::default();
        let mut transaction = ProtectedClipboard::begin(
            "akl-clipboard-probe",
            Duration::from_millis(CLIPBOARD_BROKER_TIMEOUT_MS),
            &brokers,
            unsafe { GetCurrentProcessId() },
        )
        .expect("protected clipboard transaction");
        assert!(transaction.is_current());
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some("akl-clipboard-probe")
        );
        let opt_outs = clipboard_opt_out_formats().expect("opt-out formats");
        let temporary_formats = test_clipboard_formats(window).expect("temporary formats");
        for format in opt_outs {
            assert!(temporary_formats.contains(&format));
        }

        assert!(transaction.restore());
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some("akl-original")
        );
        // The user's item comes back with the opt-outs it lacked, and nothing else.
        let mut expected = original_formats.clone();
        expected.extend(opt_outs);
        assert_eq!(
            sorted(test_clipboard_formats(window).expect("restored formats")),
            sorted(expected)
        );
        drop(transaction);
        assert!(brokers.reap().is_empty());
        unsafe {
            DestroyWindow(window).expect("destroy probe window");
        }
    }

    #[test]
    #[ignore = "uses the real clipboard: run on the test VM with --include-ignored"]
    fn the_real_clipboard_is_not_replaced_once_it_changed_since_the_copy() {
        let source = clipboard_test_window(w!("AutoKeyboardLayot changed source"));
        put_test_clipboard_text(source, "akl-newer");
        unsafe {
            DestroyWindow(source).expect("destroy source window");
        }
        let window = clipboard_test_window(w!("AutoKeyboardLayot changed probe"));
        let owner = clipboard_test_window(w!("AutoKeyboardLayot changed owner"));
        // The copy was taken before another program wrote: its sequence is an older one.
        let stale = unsafe { GetClipboardSequenceNumber() }.wrapping_sub(1);
        let cancelled = AtomicBool::new(false);
        assert_eq!(
            install_protected_clipboard(owner, "akl-replacement", stale, &cancelled),
            InstallOutcome::Untouched
        );
        assert_eq!(test_clipboard_unicode(window).as_deref(), Some("akl-newer"));
        // A worker that stopped waiting is not served either, whatever the sequence.
        cancelled.store(true, Ordering::Release);
        let current = unsafe { GetClipboardSequenceNumber() };
        assert_eq!(
            install_protected_clipboard(owner, "akl-replacement", current, &cancelled),
            InstallOutcome::Untouched
        );
        assert_eq!(test_clipboard_unicode(window).as_deref(), Some("akl-newer"));
        unsafe {
            DestroyWindow(owner).expect("destroy owner window");
            DestroyWindow(window).expect("destroy probe window");
        }
    }

    const DELAYED_TEXT: &str = "akl-delayed-original";

    unsafe extern "system" fn delayed_render_procedure(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        use windows::Win32::UI::WindowsAndMessaging::{WM_RENDERALLFORMATS, WM_RENDERFORMAT};
        let render = || {
            if let Some(mut data) = global_utf16(DELAYED_TEXT) {
                let _ = data.transfer(u32::from(CF_UNICODETEXT.0));
            }
        };
        match message {
            WM_RENDERFORMAT => {
                render();
                LRESULT(0)
            }
            WM_RENDERALLFORMATS => {
                if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
                    if unsafe { GetClipboardOwner().ok() } == Some(hwnd) {
                        render();
                    }
                    let _ = unsafe { CloseClipboard() };
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    #[test]
    #[ignore = "uses the real clipboard: run on the test VM with --include-ignored"]
    fn delayed_rendered_clipboard_content_survives_a_protected_paste() {
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        // Programs such as Excel or browsers put formats on the clipboard that they render only
        // when asked. Rendering them for the copy must not count as a change of the clipboard,
        // or no paste could ever replace such content.
        let (ready_sender, ready) = sync_channel::<u32>(1);
        let owner = thread::spawn(move || {
            let module = unsafe { GetModuleHandleW(None).expect("module") };
            let class = WNDCLASSW {
                lpfnWndProc: Some(delayed_render_procedure),
                hInstance: HINSTANCE(module.0),
                lpszClassName: w!("AutoKeyboardLayotDelayedRenderTest"),
                ..Default::default()
            };
            unsafe { RegisterClassW(&class) };
            let window = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("AutoKeyboardLayotDelayedRenderTest"),
                    w!("AutoKeyboardLayot delayed render test"),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(HINSTANCE(module.0)),
                    None,
                )
                .expect("owner window")
            };
            {
                let _open = ClipboardOpenGuard::open(window, CLIPBOARD_OPEN_RETRIES).expect("open");
                unsafe { EmptyClipboard() }.expect("empty");
                // Without data the call returns no handle even when it succeeds.
                let _ = unsafe { SetClipboardData(u32::from(CF_UNICODETEXT.0), None) };
            }
            let _ = ready_sender.send(unsafe { GetCurrentThreadId() });
            let mut message = MSG::default();
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
                unsafe {
                    DispatchMessageW(&message);
                }
            }
            unsafe {
                let _ = DestroyWindow(window);
            }
        });
        let owner_thread = ready.recv().expect("owner ready");

        let _com = PrivacyGuard::new();
        let window = clipboard_test_window(w!("AutoKeyboardLayot delayed render probe"));
        let brokers = ClipboardBrokers::default();
        let mut transaction = ProtectedClipboard::begin(
            "akl-delayed-probe",
            Duration::from_millis(CLIPBOARD_BROKER_TIMEOUT_MS),
            &brokers,
            unsafe { GetCurrentProcessId() },
        )
        .expect("the replacement goes over delayed-rendered content");
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some("akl-delayed-probe")
        );
        assert!(transaction.restore());
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some(DELAYED_TEXT)
        );
        drop(transaction);
        unsafe {
            PostThreadMessageW(owner_thread, WM_QUIT, WPARAM(0), LPARAM(0))
                .expect("stop the owner");
            DestroyWindow(window).expect("destroy probe window");
        }
        owner.join().expect("owner thread");
    }

    #[test]
    fn keypad_digits_are_replayed_as_keypad_keys_not_as_navigation_scan_codes() {
        // Sent as a bare scan code, keypad 2 arrives as Down, which recalls history or clears the
        // line in a terminal: a Pause conversion of a word with a keypad digit deleted it on the
        // host. Digits are never shifted (Shift turns keypad keys into navigation).
        let key = |scan_code, shift, extended| ReplayKey {
            scan_code,
            shift,
            caps_lock: false,
            extended,
        };
        let keys = [
            key(0x1e, false, false),
            key(0x50, false, false),
            key(0x53, true, false),
            key(0x50, false, true),
        ];
        let plan = build_physical_replay_plan(0, &keys, None).expect("bounded plan");
        let input = |index: usize| unsafe { plan.inputs[index].Anonymous.ki };
        // A letter stays a scan code.
        assert_eq!(input(0).wVk, VIRTUAL_KEY(0));
        assert_eq!(input(0).dwFlags, KEYEVENTF_SCANCODE);
        // Keypad 2 is its virtual key, down and up, with its scan code for the application.
        assert_eq!(input(2).wVk, VIRTUAL_KEY(VK_NUMPAD0.0 + 2));
        assert_eq!(input(2).wScan, 0x50);
        assert_eq!(input(2).dwFlags, Default::default());
        assert_eq!(input(3).wVk, VIRTUAL_KEY(VK_NUMPAD0.0 + 2));
        assert_eq!(input(3).dwFlags, KEYEVENTF_KEYUP);
        // The keypad decimal key, and no Shift around it.
        assert_eq!(input(4).wVk, VK_DECIMAL);
        assert_eq!(input(5).wVk, VK_DECIMAL);
        assert_eq!(input(5).dwFlags, KEYEVENTF_KEYUP);
        // The extended key with the same scan code is the Down arrow, not a keypad digit.
        assert_eq!(input(6).wVk, VIRTUAL_KEY(0));
        assert_eq!(input(6).dwFlags, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY);
        assert_eq!(plan.inputs.len(), 8);
        assert_eq!(plan.step_ends, [2, 4, 6, 8]);
    }

    #[test]
    fn builds_one_marked_physical_replay_plan() {
        let keys = [
            ReplayKey {
                scan_code: 0x23,
                shift: false,
                caps_lock: false,
                extended: false,
            },
            ReplayKey {
                scan_code: 0x12,
                shift: true,
                caps_lock: false,
                extended: false,
            },
        ];
        let plan = build_physical_replay_plan(2, &keys, Some(' '))
            .expect("bounded physical replay should build");
        let inputs = &plan.inputs;
        assert_eq!(inputs.len(), 12);
        for input in inputs {
            assert_eq!(input.r#type, INPUT_KEYBOARD);
            let keyboard = unsafe { input.Anonymous.ki };
            assert_eq!(keyboard.dwExtraInfo, INJECTED_EVENT_MARKER);
        }
        let first = unsafe { inputs[0].Anonymous.ki };
        let second = unsafe { inputs[1].Anonymous.ki };
        assert_eq!(first.wVk, VK_BACK);
        assert_eq!(first.dwFlags, Default::default());
        assert_eq!(second.dwFlags, KEYEVENTF_KEYUP);
        let first_replay = unsafe { inputs[4].Anonymous.ki };
        assert_eq!(first_replay.wVk, VIRTUAL_KEY(0));
        assert_eq!(first_replay.wScan, 0x23);
        assert_eq!(first_replay.dwFlags, KEYEVENTF_SCANCODE);
        let shift_down = unsafe { inputs[6].Anonymous.ki };
        let shifted_down = unsafe { inputs[7].Anonymous.ki };
        let shifted_up = unsafe { inputs[8].Anonymous.ki };
        let shift_up = unsafe { inputs[9].Anonymous.ki };
        assert_eq!(shift_down.wVk, VK_LSHIFT);
        assert_eq!(shift_down.dwFlags, Default::default());
        assert_eq!(shifted_down.wScan, 0x12);
        assert_eq!(shifted_down.dwFlags, KEYEVENTF_SCANCODE);
        assert_eq!(shifted_up.wScan, 0x12);
        assert_eq!(shifted_up.dwFlags, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP);
        assert_eq!(shift_up.wVk, VK_LSHIFT);
        assert_eq!(shift_up.dwFlags, KEYEVENTF_KEYUP);
        let delimiter = unsafe { inputs[10].Anonymous.ki };
        assert_eq!(delimiter.wVk, VK_SPACE);
        assert_eq!(plan.step_ends, [2, 4, 6, 10, 12]);
    }

    #[test]
    fn maps_buffered_scan_codes_through_a_windows_layout_with_caps_toggle() {
        // This mapping test is read-only and only runs for an exact US layout
        // already current on its own thread. Resolver activation is tested by
        // the separate explicit diagnostic, not hidden in this unit fixture.
        let english = unsafe { GetKeyboardLayout(0) };
        let mut name = [0u16; 9];
        if unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutNameW(&mut name) }
            .is_err()
        {
            return;
        }
        let Ok(name) = String::from_utf16(&name[..8]) else {
            return;
        };
        if format!("{:04X}:{name}", english.0 as usize & 0xffff) != "0409:00000409" {
            return;
        }
        let hello = [0x23_u16, 0x12, 0x26, 0x26, 0x18].map(|scan_code| ReplayKey {
            scan_code,
            shift: false,
            caps_lock: false,
            extended: false,
        });
        assert_eq!(
            map_replay_keys_to_layout(&hello, english).as_deref(),
            Some("hello")
        );

        let capital_h = ReplayKey {
            caps_lock: true,
            ..hello[0]
        };
        assert_eq!(map_replay_key_to_layout(capital_h, english), Some('H'));
    }

    #[test]
    fn extended_scan_code_is_preserved_in_physical_replay() {
        let key = ReplayKey {
            scan_code: 0x1c,
            shift: false,
            caps_lock: false,
            extended: true,
        };
        let plan = build_physical_replay_plan(1, &[key], Some(' ')).expect("valid replay plan");
        let down = unsafe { plan.inputs[2].Anonymous.ki };
        let up = unsafe { plan.inputs[3].Anonymous.ki };
        assert_eq!(down.dwFlags, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY);
        assert_eq!(
            up.dwFlags,
            KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP
        );
    }

    #[test]
    fn hotkey_replay_does_not_insert_an_artificial_space() {
        let key = ReplayKey {
            scan_code: 0x15,
            shift: false,
            caps_lock: false,
            extended: false,
        };
        let plan = build_physical_replay_plan(1, &[key], None).expect("valid hotkey replay");
        assert_eq!(plan.step_ends, [2, 4]);
        assert_eq!(plan.inputs.len(), 4);
        let final_key = unsafe { plan.inputs[3].Anonymous.ki };
        assert_eq!(final_key.wScan, key.scan_code);
        assert_eq!(final_key.dwFlags, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP);
    }

    #[test]
    fn terminal_dispatch_keeps_one_full_batch() {
        let keys = [ReplayKey {
            scan_code: 0x23,
            shift: false,
            caps_lock: false,
            extended: false,
        }];
        let plan = build_physical_replay_plan(1, &keys, Some(' ')).expect("valid plan");
        let mut send_lengths = Vec::new();
        let mut pace_calls = 0;

        let result = dispatch_physical_replay_plan(
            &plan,
            PhysicalReplayMode::Batch,
            || true,
            |inputs| {
                send_lengths.push(inputs.len());
                true
            },
            || {
                pace_calls += 1;
            },
        );

        assert_eq!(result, ReplayAttempt::Applied);
        assert_eq!(send_lengths, [plan.inputs.len()]);
        assert_eq!(pace_calls, 0);
    }

    #[test]
    fn notepad_dispatch_sends_atomic_steps_with_pacing_between_them() {
        let keys = [
            ReplayKey {
                scan_code: 0x23,
                shift: false,
                caps_lock: false,
                extended: false,
            },
            ReplayKey {
                scan_code: 0x12,
                shift: true,
                caps_lock: false,
                extended: false,
            },
        ];
        let plan = build_physical_replay_plan(2, &keys, Some(' ')).expect("valid plan");
        let mut send_lengths = Vec::new();
        let mut pace_calls = 0;

        let result = dispatch_physical_replay_plan(
            &plan,
            PhysicalReplayMode::Paced,
            || true,
            |inputs| {
                send_lengths.push(inputs.len());
                true
            },
            || {
                pace_calls += 1;
            },
        );

        assert_eq!(result, ReplayAttempt::Applied);
        assert_eq!(send_lengths, [2, 2, 2, 4, 2]);
        assert_eq!(pace_calls, 4);
    }

    #[test]
    fn paced_dispatch_stops_after_a_failed_logical_step() {
        let keys = [ReplayKey {
            scan_code: 0x23,
            shift: false,
            caps_lock: false,
            extended: false,
        }];
        let plan = build_physical_replay_plan(2, &keys, Some(' ')).expect("valid plan");
        let mut send_calls = 0;
        let mut pace_calls = 0;

        let result = dispatch_physical_replay_plan(
            &plan,
            PhysicalReplayMode::Paced,
            || true,
            |_| {
                send_calls += 1;
                send_calls != 3
            },
            || {
                pace_calls += 1;
            },
        );

        assert_eq!(result, ReplayAttempt::Failed);
        assert_eq!(send_calls, 3);
        assert_eq!(pace_calls, 2);
    }

    #[test]
    fn paced_dispatch_reports_guard_loss_after_edit_as_failure() {
        let keys = [ReplayKey {
            scan_code: 0x23,
            shift: false,
            caps_lock: false,
            extended: false,
        }];
        let plan = build_physical_replay_plan(1, &keys, Some(' ')).expect("valid plan");
        let mut guard_calls = 0;
        let mut send_calls = 0;

        let result = dispatch_physical_replay_plan(
            &plan,
            PhysicalReplayMode::Paced,
            || {
                guard_calls += 1;
                guard_calls <= 2
            },
            |_| {
                send_calls += 1;
                true
            },
            || {},
        );

        assert_eq!(result, ReplayAttempt::Failed);
        assert_eq!(send_calls, 1);
    }

    #[test]
    fn maximum_session_word_fits_the_paced_gate_budget() {
        let keys = vec![
            ReplayKey {
                scan_code: 0x23,
                shift: false,
                caps_lock: false,
                extended: false,
            };
            64
        ];
        let plan = build_physical_replay_plan(65, &keys, Some(' ')).expect("valid maximum plan");
        assert_eq!(plan.step_ends.len(), 130);
        assert!(paced_replay_fits_gate(plan.step_ends.len()));
        assert!(!paced_replay_fits_gate(135));
        assert_eq!(
            conversion_gate_budget_ms(plan.step_ends.len(), true, true),
            Some(1_187)
        );
        assert_eq!(
            conversion_gate_budget_ms(plan.step_ends.len(), true, false),
            Some(1_137)
        );
        assert_eq!(conversion_gate_budget_ms(14, false, false), Some(535));
        // A protected paste is budgeted from its own waits, not as a paced replay: settle,
        // selection, the least installation and everything after the Ctrl+V.
        assert_eq!(paste_gate_budget_ms(false), 1_000);
        assert_eq!(paste_gate_budget_ms(true), 1_035);
        assert!(paste_gate_budget_ms(true) <= CORRECTION_GATE_MAX_HOLD_MS);

        // The handoff is judged at a fixed instant, so it fits exactly when the budget does.
        let now = Instant::now();
        let mut gate = CorrectionGate::default();
        gate.activate(10);
        gate.awaiting_worker_ack = true;
        assert!(gate.prepare_handoff_budget(10, 839));
        gate.deadline = now + Duration::from_millis(838);
        assert!(!gate.handoff(10, 20, now));

        gate.activate(10);
        gate.awaiting_worker_ack = true;
        assert!(gate.prepare_handoff_budget(10, 789));
        gate.deadline = now + Duration::from_millis(789);
        assert!(gate.handoff(10, 20, now));
    }

    #[test]
    fn high_resolution_replay_pacer_wait_is_bounded() {
        let pacer = ReplayPacer::new().expect("Windows 11 should provide a high-resolution timer");
        let started = Instant::now();
        pacer.wait();
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(1));
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn gate_holds_full_raw_edges_and_replay_ends_with_a_fence() {
        let foreground = ForegroundContext {
            hwnd: 1,
            focus: 1,
            input_thread_id: 2,
            process_id: 3,
            layout: 4,
        };
        let down = RawKeyEvent {
            message: WM_KEYDOWN,
            virtual_key: u32::from(VK_RIGHT.0),
            scan_code: 0x4d,
            caps_lock: false,
            extended: true,
            foreground,
            sequence: 0,
            drain_token: 0,
            hotkey_trigger: false,
            repeat: false,
            held_modifiers: 0,
        };
        let up = RawKeyEvent {
            message: WM_KEYUP,
            ..down
        };
        let mut gate = CorrectionGate::default();
        gate.activate(42);
        assert!(gate.hold(down));
        assert!(gate.hold(up));
        assert_eq!(gate.held.len(), 2);

        let inputs = build_held_replay_inputs(&gate.held, gate.fence_marker)
            .expect("held edges and fence should build");
        assert_eq!(inputs.len(), 4);
        let replay_down = unsafe { inputs[0].Anonymous.ki };
        let replay_up = unsafe { inputs[1].Anonymous.ki };
        let fence_down = unsafe { inputs[2].Anonymous.ki };
        let fence_up = unsafe { inputs[3].Anonymous.ki };
        assert_eq!(replay_down.dwExtraInfo, DRAINED_EVENT_MARKER);
        assert_eq!(
            replay_down.dwFlags,
            KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY
        );
        assert_eq!(
            replay_up.dwFlags,
            KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP
        );
        assert_eq!(fence_down.wVk, VK_F24);
        assert_eq!(fence_down.dwExtraInfo, gate.fence_marker);
        assert_eq!(fence_up.dwFlags, KEYEVENTF_KEYUP);
    }

    #[test]
    fn held_replay_prefix_stops_after_first_space_down() {
        let mut held = vec![
            test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0),
            test_raw_key(WM_KEYUP, VIRTUAL_KEY(0x47), 0, 0),
            test_raw_key(WM_KEYDOWN, VK_SPACE, 0, 0),
            test_raw_key(WM_KEYUP, VK_SPACE, 0, 0),
            test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x48), 0, 0),
            test_raw_key(WM_KEYUP, VIRTUAL_KEY(0x48), 0, 0),
        ];

        let prefix = take_held_replay_prefix(&mut held);

        assert_eq!(prefix.len(), 3);
        assert_eq!(prefix.last().map(|event| event.virtual_key), Some(0x20));
        assert!(matches!(
            prefix.last().map(|event| event.message),
            Some(WM_KEYDOWN)
        ));
        assert_eq!(held.len(), 3);
        assert_eq!(held[0].virtual_key, u32::from(VK_SPACE.0));
        assert_eq!(held[0].message, WM_KEYUP);
    }

    #[test]
    fn matching_space_up_tail_preserves_undo() {
        let space_up = [test_raw_key(WM_KEYUP, VK_SPACE, 0, 0)];
        assert!(!held_tail_invalidates_undo(&space_up));

        let space_down = [test_raw_key(WM_KEYDOWN, VK_SPACE, 0, 0)];
        assert!(held_tail_invalidates_undo(&space_down));

        let next_key = [test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0)];
        assert!(held_tail_invalidates_undo(&next_key));
    }

    #[test]
    fn gate_handoff_preserves_suffix_and_absolute_deadline() {
        let mut gate = CorrectionGate::default();
        gate.activate(10);
        gate.awaiting_worker_ack = true;
        gate.current_drain_count = 4;
        gate.released_count = 4;
        gate.held
            .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x48), 0, 0));
        let deadline = gate.deadline;
        let now = deadline - Duration::from_secs(1);

        assert!(gate.prepare_handoff_budget(10, 100));
        assert!(gate.handoff(10, 20, now));
        assert!(gate.active);
        assert_eq!(gate.token, 20);
        assert_eq!(gate.fence_marker, GATE_FENCE_MARKER_BASE | 20);
        assert_eq!(gate.held.len(), 1);
        assert_eq!(gate.released_count, 0);
        assert_eq!(gate.current_drain_count, 0);
        assert!(!gate.awaiting_worker_ack);
        assert_eq!(gate.deadline, deadline);
        assert!(!gate.handoff(10, 30, now));
    }

    #[test]
    fn gate_rejects_duplicate_drain_and_near_deadline_handoff() {
        let mut gate = CorrectionGate::default();
        gate.activate(10);
        assert!(gate.can_start_drain(10));
        gate.drain_in_flight = true;
        assert!(!gate.can_start_drain(10));
        gate.drain_in_flight = false;
        gate.awaiting_worker_ack = true;
        assert!(!gate.can_start_drain(10));
        assert!(gate.prepare_handoff_budget(10, 200));
        let now = Instant::now();
        gate.deadline = now + Duration::from_millis(199);
        assert!(!gate.handoff(10, 20, now));
        assert_eq!(gate.token, 10);
    }

    #[test]
    fn owned_fence_namespace_is_always_recognized() {
        assert!(is_owned_gate_fence_marker(GATE_FENCE_MARKER_BASE));
        assert!(is_owned_gate_fence_marker(GATE_FENCE_MARKER_BASE | 0x7fff));
        assert!(!is_owned_gate_fence_marker(INJECTED_EVENT_MARKER));
        assert!(!is_owned_gate_fence_marker(DRAINED_EVENT_MARKER));
    }

    #[test]
    #[cfg_attr(
        not(feature = "legacy-bundled-input"),
        ignore = "the keys spell \"привет\" in Russian, and the modular base has no Russian dictionary"
    )]
    fn drained_space_down_defers_candidate_until_fence_ack() {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(true, Ordering::Release);
        let mut processor = InputProcessor::new(metrics, 0);
        let foreground = ForegroundContext {
            hwnd: 1,
            focus: 1,
            input_thread_id: 2,
            process_id: 3,
            layout: 0x0409,
        };
        processor.last_foreground = Some(foreground_identity_key(foreground));
        processor.last_layout = Some(foreground.layout);
        processor.set_privacy_reason(None);
        // US set-1 scan codes for "ghbdtn", verified with MapVirtualKeyEx on the
        // exact US layout.
        for (character, scan_code) in "ghbdtn"
            .chars()
            .zip([0x22_u16, 0x23, 0x30, 0x20, 0x14, 0x31])
        {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
            processor.replay_keys.push(ReplayKey {
                scan_code,
                shift: false,
                caps_lock: false,
                extended: false,
            });
        }
        // test_profiles() supplies synthetic layout handles; the test keyboard maps the replay
        // keys in them.
        processor.test_keyboard = Some(test_keyboard);
        let event = RawKeyEvent {
            foreground,
            ..test_raw_key(WM_KEYDOWN, VK_SPACE, 40, 7)
        };

        processor.process_key(event);

        assert!(processor.pending_conversion.is_some());
        assert_eq!(
            processor.deferred_drained_conversion,
            Some(DeferredDrainedConversion {
                gate_token: 7,
                boundary_sequence: 40,
            })
        );
    }

    /// A processor typing in an allowed field of an editor whose foreground layout is `layout`,
    /// with the test keyboard and automatic conversion on.
    fn processor_typing_in(layout: usize) -> (InputProcessor, ForegroundContext) {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(true, Ordering::Release);
        let mut processor = InputProcessor::new(metrics, 0);
        let foreground = ForegroundContext {
            layout,
            ..test_foreground()
        };
        processor.test_keyboard = Some(test_keyboard);
        processor.last_foreground = Some(foreground_identity_key(foreground));
        processor.last_layout = Some(foreground.layout);
        processor.foreground_override = Some(foreground);
        processor.process_override = Some((Some(EDITOR.to_owned()), Some(0x2000)));
        processor.current_integrity_level = Some(0x2000);
        processor.privacy_probe = Some(answering_probe(None));
        processor.privacy_needs_check = true;
        (processor, foreground)
    }

    /// Types the keys with these scan codes.
    fn type_keys(
        processor: &mut InputProcessor,
        foreground: ForegroundContext,
        scan_codes: &[u32],
    ) {
        for (index, &scan_code) in scan_codes.iter().enumerate() {
            processor.process_key(RawKeyEvent {
                scan_code,
                foreground,
                ..letter(0x41, index as u64 + 1)
            });
        }
    }

    /// A Space; a drained one (`drain_token` other than 0) defers the edit.
    #[cfg(feature = "legacy-bundled-input")]
    fn space_key(foreground: ForegroundContext, sequence: u64, drain_token: u64) -> RawKeyEvent {
        RawKeyEvent {
            scan_code: 0x39,
            foreground,
            ..test_raw_key(WM_KEYDOWN, VK_SPACE, sequence, drain_token)
        }
    }

    /// Types the keys with these scan codes, then a drained Space that defers the edit.
    #[cfg(feature = "legacy-bundled-input")]
    fn type_keys_and_drained_space(
        processor: &mut InputProcessor,
        foreground: ForegroundContext,
        scan_codes: &[u32],
    ) {
        type_keys(processor, foreground, scan_codes);
        processor.process_key(space_key(foreground, scan_codes.len() as u64 + 1, 9));
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn a_key_that_prints_a_letter_of_a_target_layout_continues_the_word() {
        // Russian typed with the Estonian layout active: the comma key prints "б" in Russian, so
        // ",skj" is one word, "было".
        let (mut processor, foreground) = processor_typing_in(0x0425);
        type_keys_and_drained_space(&mut processor, foreground, &[0x33, 0x1f, 0x25, 0x24]);
        let pending = processor
            .pending_conversion
            .as_ref()
            .expect("the word is converted");
        assert_eq!(pending.transaction.original, ",skj");
        assert_eq!(pending.transaction.replacement, "было");
        assert_eq!(pending.transaction.target_language, Language::Russian);
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn backspace_resumes_a_word_with_a_key_that_prints_a_letter_of_a_target_layout() {
        let (mut processor, foreground) = processor_typing_in(0x0425);
        type_keys_and_drained_space(&mut processor, foreground, &[0x33, 0x1f, 0x25, 0x24]);
        let last = processor.last_boundary.take();
        let backspace = RawKeyEvent {
            foreground,
            ..test_raw_key(WM_KEYDOWN, VK_BACK, 6, 0)
        };
        assert!(processor.erase_or_resume_word(last, backspace, Some(Language::Estonian)));
        assert_eq!(processor.session.buffered_character_count(), 4);
        assert_eq!(processor.replay_keys.len(), 4);
    }

    #[test]
    fn the_hook_counts_the_edge_of_its_own_modifier_key() {
        // No modifier key is physically held while the tests run; the asynchronous key state
        // does not show the event's own edge yet, so the hook adds it.
        assert_eq!(held_modifiers_for_event(0x41, true, false), 0);
        assert_eq!(
            held_modifiers_for_event(VK_LSHIFT.0, true, false),
            held_keys(&[VK_LSHIFT])
        );
        assert_eq!(
            held_modifiers_for_event(VK_RCONTROL.0, true, false),
            held_keys(&[VK_RCONTROL])
        );
        assert_eq!(held_modifiers_for_event(VK_LSHIFT.0, false, true), 0);
        assert_eq!(held_modifiers_for_event(VK_RWIN.0, false, true), 0);
    }

    #[test]
    fn the_hooks_held_modifiers_become_the_tracked_ones() {
        let none = Modifiers::from_held(0);
        assert!(!none.shift() && !none.has_shortcut_modifier());
        let shift = Modifiers::from_held(held_keys(&[VK_RSHIFT]));
        assert!(shift.shift() && !shift.has_shortcut_modifier());
        // Each side is tracked apart: letting one Shift go keeps the other held.
        let mut both = Modifiers::from_held(held_keys(&[VK_LSHIFT, VK_RSHIFT]));
        both.update(u32::from(VK_LSHIFT.0), false, true);
        assert!(both.shift());
        both.update(u32::from(VK_RSHIFT.0), false, true);
        assert!(!both.shift());
        for key in [
            VK_LCONTROL,
            VK_RCONTROL,
            VK_LMENU,
            VK_RMENU,
            VK_LWIN,
            VK_RWIN,
        ] {
            let modifiers = Modifiers::from_held(held_keys(&[key]));
            assert!(
                modifiers.has_shortcut_modifier() && !modifiers.shift(),
                "{key:?}"
            );
        }
        let alt_control = Modifiers::from_held(held_keys(&[VK_RMENU, VK_LCONTROL]));
        assert!(alt_control.alt() && alt_control.control());
    }

    #[test]
    fn modifiers_held_across_a_focus_change_come_from_the_key_event() {
        let (mut processor, foreground) = processor_typing_in(0x0409);
        // Shift+Tab moves the focus while Shift stays held: the worker saw Shift go down, and
        // the next key in the other field still has it held.
        processor.process_key(RawKeyEvent {
            scan_code: 0x2a,
            held_modifiers: held_keys(&[VK_LSHIFT]),
            foreground,
            ..test_raw_key(WM_KEYDOWN, VK_LSHIFT, 1, 0)
        });
        let field = ForegroundContext {
            focus: 9,
            ..foreground
        };
        processor.foreground_override = Some(field);
        processor.process_key(RawKeyEvent {
            scan_code: 0x30,
            held_modifiers: held_keys(&[VK_LSHIFT]),
            foreground: field,
            ..letter(0x42, 2)
        });
        assert_eq!(processor.session.buffered_character_count(), 1);
        assert!(
            processor.replay_keys[0].shift,
            "B was typed with Shift held"
        );
        // Ctrl held across a switch makes the next key a shortcut, not a typed letter.
        let other = ForegroundContext {
            focus: 10,
            ..foreground
        };
        processor.foreground_override = Some(other);
        processor.process_key(RawKeyEvent {
            scan_code: 0x2f,
            held_modifiers: held_keys(&[VK_LCONTROL]),
            foreground: other,
            ..letter(0x56, 3)
        });
        assert_eq!(processor.last_word_reset, "shortcut");
        assert_eq!(processor.session.buffered_character_count(), 0);
    }

    #[test]
    fn modifiers_held_into_a_new_input_epoch_come_from_the_key_event() {
        let (mut processor, foreground) = processor_typing_in(0x0409);
        type_keys(&mut processor, foreground, &[0x22]);
        processor.process_key(RawKeyEvent {
            scan_code: 0x2a,
            held_modifiers: held_keys(&[VK_LSHIFT]),
            foreground,
            ..test_raw_key(WM_KEYDOWN, VK_LSHIFT, 2, 0)
        });
        // A settings Apply starts a new input epoch while Shift stays held.
        let epoch = processor.metrics.input_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        processor.process(QueuedInputEvent {
            epoch,
            captured_at: Instant::now(),
            event: RawInputEvent::Key(RawKeyEvent {
                scan_code: 0x30,
                held_modifiers: held_keys(&[VK_LSHIFT]),
                foreground,
                ..letter(0x42, 3)
            }),
            configuration: None,
        });
        assert_eq!(processor.session.buffered_character_count(), 1);
        assert!(
            processor.replay_keys[0].shift,
            "B was typed with Shift held"
        );
    }

    /// Presses and releases `keys` in order, each event with the modifiers held at that moment.
    fn press_chord(
        processor: &mut InputProcessor,
        foreground: ForegroundContext,
        keys: &[(VIRTUAL_KEY, u32)],
        sequence: u64,
    ) {
        let modifier_keys: Vec<VIRTUAL_KEY> = keys
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| PHYSICAL_MODIFIER_KEYS.contains(key))
            .collect();
        let mut held = Vec::new();
        let edges = keys
            .iter()
            .map(|key| (key, WM_KEYDOWN))
            .chain(keys.iter().rev().map(|key| (key, WM_KEYUP)));
        for (index, (&(key, scan_code), message)) in edges.enumerate() {
            if modifier_keys.contains(&key) {
                if message == WM_KEYDOWN {
                    held.push(key);
                } else {
                    held.retain(|other| *other != key);
                }
            }
            processor.process_key(RawKeyEvent {
                scan_code,
                held_modifiers: held_keys(&held),
                foreground,
                ..test_raw_key(message, key, sequence + index as u64, 0)
            });
        }
    }

    #[test]
    fn a_layout_chord_keeps_the_word_for_pause() {
        for chord in [
            &[(VK_LWIN, 0x5b), (VK_SPACE, 0x39)][..],
            &[(VK_LWIN, 0x5b), (VK_LSHIFT, 0x2a), (VK_SPACE, 0x39)],
            &[(VK_LMENU, 0x38), (VK_LSHIFT, 0x2a)],
        ] {
            let (mut processor, foreground) = processor_typing_in(0x0409);
            type_keys(&mut processor, foreground, &GHBDTN);
            press_chord(&mut processor, foreground, chord, 7);
            assert_eq!(
                processor.replay_keys.len(),
                6,
                "{chord:?}: the word is kept"
            );
            // The next key shows the Russian layout: the word typed so far stays for Pause, read
            // in the layout it was typed in.
            let russian = ForegroundContext {
                layout: 0x0419,
                ..foreground
            };
            processor.foreground_override = Some(russian);
            press_chord(&mut processor, russian, &[(VK_LSHIFT, 0x2a)], 20);
            let kept = processor
                .last_boundary
                .as_ref()
                .unwrap_or_else(|| panic!("{chord:?}: no word for Pause"));
            assert_eq!(kept.replay_keys.len(), 6, "{chord:?}");
            assert_eq!(kept.foreground.layout, 0x0409, "{chord:?}");
        }
    }

    #[test]
    fn gate_drain_control_event_bypasses_stale_input_epoch() {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.input_epoch.store(9, Ordering::Release);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        processor.deferred_drained_conversion = Some(DeferredDrainedConversion {
            gate_token: 7,
            boundary_sequence: 40,
        });

        processor.process(QueuedInputEvent {
            configuration: None,
            captured_at: Instant::now(),
            epoch: 1,
            event: RawInputEvent::GateDrainReady {
                token: 7,
                drained_count: 4,
                replay_succeeded: true,
            },
        });

        assert_eq!(metrics.gate_replayed_events.load(Ordering::Acquire), 4);
        assert_eq!(processor.last_input_epoch, 9);
        assert!(processor.deferred_drained_conversion.is_none());
        assert!(processor.pending_conversion.is_none());
    }

    #[test]
    fn auto_mode_has_one_shared_atomic_source_of_truth() {
        // The tray menu's toggle writes the one value that the worker and the window's gate
        // requests read: nothing keeps a copy of its own.
        let (state, _receiver) = test_gate_state(4);
        let metrics = Arc::clone(&state.metrics);
        metrics.auto_enabled.store(false, Ordering::Release);
        let processor = InputProcessor::new(Arc::clone(&metrics), 0);
        let space = |epoch| QueuedInputEvent {
            epoch,
            captured_at: Instant::now(),
            event: RawInputEvent::Key(test_raw_key(WM_KEYDOWN, VK_SPACE, 1, 0)),
            configuration: None,
        };
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        for enabled in [true, false] {
            toggle_auto_enabled();
            // The toggle makes queued events stale; a new one is judged by the mode alone.
            let current = space(metrics.input_epoch.load(Ordering::Acquire));
            assert_eq!(
                processor.gate_bypass_reason(&current) == "automatic-off",
                !enabled
            );
            assert_eq!(gate_request_enabled(&metrics, 0), enabled);
        }
        drop(take_app_state());
    }

    #[test]
    fn physical_space_down_arms_the_decision_gate_before_space_up() {
        assert!(should_arm_space_decision_gate(
            false,
            true,
            VK_SPACE.0,
            true,
            PRIVACY_ALLOWED,
            false,
            true,
        ));
        assert!(!should_arm_space_decision_gate(
            true,
            true,
            VK_SPACE.0,
            true,
            PRIVACY_ALLOWED,
            false,
            true,
        ));
        assert!(!should_arm_space_decision_gate(
            false,
            false,
            VK_SPACE.0,
            true,
            PRIVACY_ALLOWED,
            false,
            true,
        ));
        assert!(!should_arm_space_decision_gate(
            false,
            true,
            VK_SPACE.0,
            true,
            PRIVACY_PASSWORD,
            false,
            true,
        ));
        assert!(!should_arm_space_decision_gate(
            false,
            true,
            VK_SPACE.0,
            true,
            PRIVACY_ALLOWED,
            true,
            true,
        ));
        assert!(!should_arm_space_decision_gate(
            false,
            true,
            VK_SPACE.0,
            true,
            PRIVACY_ALLOWED,
            false,
            false,
        ));
    }

    #[test]
    fn win_space_never_arms_or_holds_the_decision_gate() {
        // The hook asks for the generic Ctrl and Alt keys, and for each Windows key: there is no
        // generic one. Shift is part of typing.
        for (held_key, arms) in [
            (None, true),
            (Some(VK_SHIFT), true),
            (Some(VK_LWIN), false),
            (Some(VK_RWIN), false),
            (Some(VK_CONTROL), false),
            (Some(VK_MENU), false),
        ] {
            let released = shortcut_modifiers_released_by(|key| Some(key) == held_key);
            assert_eq!(
                should_arm_space_decision_gate(
                    false,
                    true,
                    VK_SPACE.0,
                    true,
                    PRIVACY_ALLOWED,
                    false,
                    released,
                ),
                arms,
                "{held_key:?}"
            );
        }
    }

    #[test]
    fn configurable_hotkey_triggers_once_and_waits_for_modifier_release() {
        let metrics = ObserverMetrics::default();
        metrics
            .force_hotkey_virtual_key
            .store(u32::from(VK_F12.0), Ordering::Release);
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYDOWN, VK_F12.0, 0),
            UndoHotkeyAction::TriggerSwallow
        );
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYDOWN, VK_F12.0, 0),
            UndoHotkeyAction::Swallow
        );
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYUP, VK_F12.0, 0),
            UndoHotkeyAction::Swallow
        );

        metrics
            .force_hotkey_virtual_key
            .store(u32::from(VK_F12.0), Ordering::Release);
        metrics
            .force_hotkey_modifiers
            .store(HOTKEY_MOD_CONTROL, Ordering::Release);
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYDOWN, VK_F12.0, HOTKEY_MOD_CONTROL),
            UndoHotkeyAction::Swallow
        );
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYUP, VK_F12.0, HOTKEY_MOD_CONTROL),
            UndoHotkeyAction::Swallow
        );
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYUP, VK_CONTROL.0, 0),
            UndoHotkeyAction::TriggerPassThrough
        );

        metrics.pause_break_undo.store(false, Ordering::Release);
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYDOWN, VK_F12.0, HOTKEY_MOD_CONTROL),
            UndoHotkeyAction::PassThrough
        );
    }

    #[test]
    fn explicit_hotkey_can_arm_gate_with_automatic_conversion_disabled() {
        let metrics = ObserverMetrics::default();
        assert!(!gate_request_enabled(&metrics, 10));
        metrics
            .explicit_hotkey_sequence
            .store(10, Ordering::Release);
        assert!(gate_request_enabled(&metrics, 10));
        assert!(!gate_request_enabled(&metrics, 11));
        metrics.pause_break_undo.store(false, Ordering::Release);
        assert!(!gate_request_enabled(&metrics, 10));
    }

    #[test]
    fn pause_pulses_and_ctrl_break_rearm_without_a_pause_key_up() {
        let metrics = ObserverMetrics::default();
        for _ in 0..3 {
            assert_eq!(
                contextual_hotkey_action(&metrics, WM_KEYDOWN, VK_PAUSE.0, 0),
                UndoHotkeyAction::TriggerSwallow
            );
        }
        metrics.undo_hotkey_active.store(false, Ordering::Release);
        metrics
            .force_hotkey_modifiers
            .store(HOTKEY_MOD_CONTROL, Ordering::Release);
        for _ in 0..3 {
            assert_eq!(
                contextual_hotkey_action(&metrics, WM_KEYDOWN, 0x03, HOTKEY_MOD_CONTROL),
                UndoHotkeyAction::Swallow
            );
            assert_eq!(
                contextual_hotkey_action(&metrics, WM_KEYUP, VK_LCONTROL.0, 0),
                UndoHotkeyAction::TriggerPassThrough
            );
        }
    }

    #[test]
    fn hotkey_handles_modifier_first_release_and_unrelated_shortcuts() {
        let metrics = ObserverMetrics::default();
        metrics
            .force_hotkey_virtual_key
            .store(u32::from(VK_F12.0), Ordering::Release);
        metrics
            .force_hotkey_modifiers
            .store(HOTKEY_MOD_CONTROL, Ordering::Release);
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYDOWN, VK_F12.0, HOTKEY_MOD_ALT),
            UndoHotkeyAction::PassThrough,
        );
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYUP, VK_F12.0, HOTKEY_MOD_ALT),
            UndoHotkeyAction::PassThrough,
        );
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYDOWN, VK_F12.0, HOTKEY_MOD_CONTROL),
            UndoHotkeyAction::Swallow,
        );
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYUP, VK_LCONTROL.0, 0),
            UndoHotkeyAction::TriggerPassThrough,
        );
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYDOWN, VK_F12.0, 0),
            UndoHotkeyAction::Swallow,
        );
        assert_eq!(
            contextual_hotkey_action(&metrics, WM_KEYUP, VK_F12.0, 0),
            UndoHotkeyAction::Swallow,
        );
    }

    #[test]
    fn lexicon_offer_expires_and_merges_idempotently() {
        let now = Instant::now();
        let candidate = VolatileLexiconCandidate::new(
            LexiconOfferTarget::WordExclusions,
            Language::English,
            "HF,JNFTN".to_owned(),
            now,
        );
        assert!(candidate.is_valid_at(now));
        assert!(!candidate.is_valid_at(now + Duration::from_secs(LEXICON_OFFER_SECONDS + 1)));

        let template = "# Source-layout words that must never be converted.\n";
        let merged = merge_lexicon_entry(template, &candidate).expect("valid exclusion");
        assert!(merged.contains("en-US: hf,jnftn"));
        assert_eq!(
            merge_lexicon_entry(&merged, &candidate),
            Some(merged.clone())
        );
    }

    #[test]
    fn one_failed_attempt_is_not_counted_twice_during_gate_cleanup() {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(true, Ordering::Release);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        processor.record_conversion_failure(ConversionFailureReason::TextCommit);
        processor.record_conversion_failure(ConversionFailureReason::GateDrain);
        assert_eq!(metrics.conversion_failures.load(Ordering::Acquire), 1);
        assert_eq!(
            metrics.last_failure_reason.load(Ordering::Acquire),
            ConversionFailureReason::TextCommit as u8
        );
    }

    #[test]
    fn active_gate_token_keeps_the_trigger_valid_while_tail_is_held() {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.observed_input_sequence.store(13, Ordering::Release);
        metrics
            .forwarded_input_sequence
            .store(10, Ordering::Release);
        metrics.active_gate_token.store(11, Ordering::Release);
        let processor = InputProcessor::new(Arc::clone(&metrics), 0);
        assert!(processor.pre_forward_input_guard_is_current(11));
        metrics
            .forwarded_input_sequence
            .store(11, Ordering::Release);
        assert!(processor.input_guard_is_current(11));
    }

    #[test]
    fn successful_gate_drain_preserves_replayed_word_state() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(metrics, 0);
        processor.set_privacy_reason(None);
        processor.session.handle(
            InputEvent::Printable('g'),
            Some(Language::English),
            &processor.detector,
        );

        processor.process(QueuedInputEvent {
            configuration: None,
            captured_at: Instant::now(),
            epoch: 0,
            event: RawInputEvent::GateDrainReady {
                token: 7,
                drained_count: 4,
                replay_succeeded: true,
            },
        });

        assert_eq!(processor.session.buffered_character_count(), 1);
        assert!(!processor.session.is_suppressed());
    }

    #[test]
    fn releasing_one_shift_does_not_clear_the_other_shift() {
        let mut modifiers = Modifiers::default();
        assert!(modifiers.update(u32::from(VK_LSHIFT.0), true, false));
        assert!(modifiers.update(u32::from(VK_RSHIFT.0), true, false));
        assert!(modifiers.update(u32::from(VK_LSHIFT.0), false, true));
        assert!(modifiers.shift());
        assert!(modifiers.update(u32::from(VK_RSHIFT.0), false, true));
        assert!(!modifiers.shift());
    }

    #[test]
    fn routes_text_edit_backend_before_any_uia_or_clipboard_work() {
        let rules = BackendRules::default();
        assert_eq!(
            text_edit_backend_for_strategy(rules.resolve("C:\\Windows\\System32\\notepad.exe")),
            TextEditBackend::ProtectedPaste
        );
        for terminal in [
            "WindowsTerminal.exe",
            "C:\\Program Files\\WindowsApps\\Microsoft.WindowsTerminal\\OpenConsole.exe",
            "C:\\Windows\\System32\\conhost.exe",
        ] {
            assert_eq!(
                text_edit_backend_for_strategy(rules.resolve(terminal)),
                TextEditBackend::PhysicalReplay
            );
        }
        assert_eq!(
            text_edit_backend_for_strategy(rules.resolve("")),
            TextEditBackend::ObserveOnly
        );
        for unknown in ["Code.exe", "Notepad++.exe", "WindowsTerminal.exe.bak"] {
            assert_eq!(
                text_edit_backend_for_strategy(rules.resolve(unknown)),
                TextEditBackend::CapabilityProbe
            );
        }
        assert_eq!(
            text_edit_backend_for_strategy(rules.resolve("firefox.exe")),
            TextEditBackend::PhysicalReplay
        );
        assert_eq!(
            text_edit_backend_for_strategy(rules.resolve("Viber.exe")),
            TextEditBackend::ProtectedPaste
        );
        assert!(TextEditBackend::ProtectedPaste.requires_text_barrier());
        assert!(!TextEditBackend::PhysicalReplay.requires_text_barrier());
        assert!(!TextEditBackend::CapabilityProbe.requires_text_barrier());
        assert!(!TextEditBackend::ObserveOnly.requires_text_barrier());
    }

    #[test]
    fn capability_fallback_never_replays_a_text_mismatch() {
        assert_eq!(
            capability_decision(TextBarrierStatus::Match, false),
            CapabilityDecision::ProtectedPaste
        );
        assert_eq!(
            capability_decision(TextBarrierStatus::Mismatch, true),
            CapabilityDecision::CancelMismatch
        );
        assert_eq!(
            capability_decision(TextBarrierStatus::Unavailable, false),
            CapabilityDecision::Unsupported
        );
        assert_eq!(
            capability_decision(TextBarrierStatus::Unavailable, true),
            CapabilityDecision::PhysicalReplay
        );
        assert!(physical_replay_fallback_is_safe(
            EditableResolverReason::OwnershipUnproven
        ));
        assert!(physical_replay_fallback_is_safe(
            EditableResolverReason::ParentUnavailable
        ));
        assert!(!physical_replay_fallback_is_safe(
            EditableResolverReason::Password
        ));
        assert!(!physical_replay_fallback_is_safe(
            EditableResolverReason::PasswordPropertyUnavailable
        ));
    }

    #[test]
    fn editable_resolver_accepts_only_an_exact_foreground_process() {
        assert_eq!(
            editable_resolver_action(false, true),
            EditableResolverAction::Continue
        );
        assert_eq!(
            editable_resolver_action(true, false),
            EditableResolverAction::Continue
        );
        assert_eq!(
            editable_resolver_action(true, true),
            EditableResolverAction::ReturnCurrent
        );
    }

    #[test]
    fn protected_paste_requires_an_editable_uia_control_type() {
        assert!(supports_protected_paste(UIA_DocumentControlTypeId));
        assert!(supports_protected_paste(UIA_EditControlTypeId));
        assert!(!supports_protected_paste(UIA_TextControlTypeId));
    }

    #[test]
    fn text_barrier_allows_only_a_trailing_gecko_nbsp_equivalent() {
        assert!(text_barrier_matches("ghbdtn ", "ghbdtn "));
        assert!(text_barrier_matches("ghbdtn\u{00a0}", "ghbdtn "));
        assert!(!text_barrier_matches("ghb\u{00a0}dtn", "ghb dtn"));
        assert!(!text_barrier_matches("ghbdtn", "ghbdtn "));
        assert!(!text_barrier_matches("other\u{00a0}", "ghbdtn "));

        let shifted =
            classify_text_barrier(" ghbdtn", "ghbdtn ", TextBarrierStage::TextCompared, -7);
        assert_eq!(shifted.actual_chars, 7);
        assert!(shifted.word_at_end);
        assert!(shifted.actual_without_first_is_word);
        assert_eq!(shifted.leading, BarrierCharacterClass::Space);
        assert_eq!(shifted.trailing, BarrierCharacterClass::Other);
        let diagnostic = format_text_barrier_report(shifted);
        assert!(diagnostic.contains("stage=TextCompared"));
        assert!(!diagnostic.contains("ghbdtn"));
    }

    #[test]
    fn focused_input_thread_changes_do_not_change_session_identity() {
        let first = ForegroundContext {
            hwnd: 11,
            focus: 11,
            input_thread_id: 21,
            process_id: 31,
            layout: 41,
        };
        let second = ForegroundContext {
            input_thread_id: 22,
            ..first
        };
        assert_eq!(
            foreground_identity_key(first),
            foreground_identity_key(second)
        );
    }

    fn buffer_test_word(processor: &mut InputProcessor, word: &str) {
        for character in word.chars() {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
            processor.replay_keys.push(ReplayKey {
                scan_code: 0x22,
                shift: false,
                caps_lock: false,
                extended: false,
            });
        }
    }

    fn test_last_boundary(processor: &InputProcessor, length: usize) -> LastBoundary {
        LastBoundary {
            replay_keys: vec![
                ReplayKey {
                    scan_code: 0x22,
                    shift: false,
                    caps_lock: false,
                    extended: false,
                };
                length
            ],
            delimiter: Some(' '),
            foreground: test_raw_key(WM_KEYDOWN, VK_BACK, 1, 0).foreground,
            epoch: processor.metrics.input_epoch.load(Ordering::Acquire),
            profile_generation: processor.input_profiles.generation(),
        }
    }

    #[test]
    fn digit_keeps_the_token_for_manual_conversion_only() {
        let (mut processor, foreground) = processor_typing_in(0x0409);
        // "ШЗм6" typed in the Russian layout for "IPv6"; typed here as Latin letters because only
        // the key sequence matters.
        type_keys(&mut processor, foreground, &[0x17, 0x19, 0x2f, 0x07, 0x2d]);
        assert_eq!(processor.replay_keys.len(), 5);
        assert_eq!(processor.session.buffered_character_count(), 0);
        // Automatic conversion stays suppressed for the token.
        assert!(processor.session.is_suppressed());

        // Backspace edits the token; erasing all of it reopens the boundary.
        let event = test_raw_key(WM_KEYDOWN, VK_BACK, 1, 0);
        for expected in (0..5).rev() {
            assert!(processor.erase_or_resume_word_with(
                None,
                event,
                Some(Language::English),
                |_, _| unreachable!(),
            ));
            assert_eq!(processor.replay_keys.len(), expected);
        }
        assert!(!processor.session.is_suppressed());

        // A token may also start with the unsupported character.
        type_keys(&mut processor, foreground, &[0x07, 0x14]);
        assert_eq!(processor.replay_keys.len(), 2);
        assert!(
            processor
                .take_manual_token_keys(InputEvent::Boundary, true, 0)
                .is_none()
        );
    }

    #[test]
    fn recent_word_languages_keep_only_the_last_four() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        for language in [
            Language::English,
            Language::Russian,
            Language::Estonian,
            Language::Russian,
            Language::English,
        ] {
            processor.remember_word_language(language);
        }
        assert_eq!(
            processor.recent_languages,
            [
                Language::Russian,
                Language::Estonian,
                Language::Russian,
                Language::English
            ]
        );
    }

    // Needs the bundled Russian dictionary: the modular base installs it as a package.
    #[cfg(feature = "legacy-bundled-input")]
    fn type_wrong_layout_word(processor: &mut InputProcessor) {
        for (character, scan_code) in "ghbdtn".chars().zip([0x22, 0x23, 0x30, 0x20, 0x14, 0x31]) {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
            processor.replay_keys.push(ReplayKey {
                scan_code,
                shift: false,
                caps_lock: false,
                extended: false,
            });
        }
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn a_finished_word_counts_as_typed_until_its_conversion_is_applied() {
        // (automatic conversion on, delimiter)
        for (automatic, delimiter) in [
            (true, Some(' ')),
            (false, Some(' ')),
            (true, None),
            (false, None),
        ] {
            let metrics = Arc::new(ObserverMetrics::default());
            metrics.auto_enabled.store(automatic, Ordering::Release);
            let mut processor = InputProcessor::new(metrics, 0);
            processor.set_privacy_reason(None);
            processor.settings.conversion_sound = false;
            processor.settings.offer_dictionary_after_forced_conversion = false;
            type_wrong_layout_word(&mut processor);
            let conversion = processor.handle_boundary(Some(Language::English), delimiter, false);
            assert_eq!(conversion.is_some(), delimiter.is_some());
            assert_eq!(
                processor.recent_languages,
                [Language::English],
                "automatic={automatic} delimiter={delimiter:?}"
            );
            // Only an applied automatic conversion makes the word count as Russian.
            let Some((transaction, replay_keys)) = conversion else {
                continue;
            };
            for (forced, expected) in [(true, Language::English), (false, Language::Russian)] {
                let pending = PendingConversion {
                    transaction: transaction.clone(),
                    foreground: test_foreground(),
                    source_layout: 0x0409,
                    profile_generation: processor.input_profiles.generation(),
                    space_down_sequence: 1,
                    replay_keys: replay_keys.clone(),
                    forced,
                    cycle_order: Vec::new(),
                };
                processor.finish_applied_conversion(
                    pending,
                    1,
                    HKL(0x0419 as *mut c_void),
                    EditStrategy::ProtectedPaste,
                );
                assert_eq!(processor.recent_languages, [expected], "forced={forced}");
            }
        }
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn a_word_whose_conversion_is_not_applied_counts_as_typed() {
        // "ghbdtn", "привет" in Russian, ended by a Space with Shift held, with Caps Lock changed
        // since the word, deferred by the gate, or handled at once by a conversion that cannot
        // reach the field here.
        for case in ["shift", "caps", "deferred", "unapplied"] {
            let (mut processor, foreground) = processor_typing_in(0x0409);
            type_keys(&mut processor, foreground, &GHBDTN);
            let mut space = space_key(foreground, 7, 0);
            match case {
                "shift" => {
                    processor
                        .modifiers
                        .update(u32::from(VK_LSHIFT.0), true, false);
                }
                "caps" => space.caps_lock = true,
                "deferred" => space.drain_token = 9,
                _ => {}
            }
            processor.process_key(space);
            assert_eq!(
                processor.pending_conversion.is_some(),
                case == "deferred",
                "{case}"
            );
            assert_eq!(processor.recent_languages, [Language::English], "{case}");
        }
    }

    #[test]
    fn a_conversion_failure_forgets_the_recent_word_languages() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        processor.remember_word_language(Language::Russian);
        processor.remember_word_language(Language::English);
        processor.record_conversion_failure(ConversionFailureReason::PhysicalEdit);
        assert!(processor.recent_languages.is_empty());
    }

    #[test]
    fn manual_layout_switch_failure_keeps_automatic_conversion() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(metrics.clone(), 0);
        metrics.auto_enabled.store(true, Ordering::Release);
        buffer_test_word(&mut processor, "ghb");
        processor.record_manual_edit_failure("layout-switch");
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
        assert!(!metrics.safety_paused.load(Ordering::Acquire));
        assert!(processor.replay_keys.is_empty());
        assert_eq!(processor.session.buffered_character_count(), 0);

        // Automatic edit failures still pause automatic conversion.
        processor.record_conversion_failure(ConversionFailureReason::PhysicalEdit);
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
        assert!(metrics.safety_paused.load(Ordering::Acquire));
    }

    #[test]
    fn text_after_a_shortcut_is_a_manual_token_only() {
        let (mut processor, foreground) = processor_typing_in(0x0409);
        // Ctrl+C in a terminal: the shortcut suppresses the session and pauses
        // privacy until the next key rechecks it.
        press_chord(
            &mut processor,
            foreground,
            &[(VK_LCONTROL, 0x1d), (VIRTUAL_KEY(0x43), 0x2e)],
            1,
        );
        assert_eq!(processor.last_word_reset, "shortcut");
        type_keys(&mut processor, foreground, &[0x22, 0x30, 0x07]);
        assert_eq!(processor.replay_keys.len(), 3);
        assert!(processor.session.is_suppressed());
        assert!(!processor.manual_token_start_known);

        // Erasing it keeps automatic conversion suppressed: the left context
        // is still unknown.
        let event = test_raw_key(WM_KEYDOWN, VK_BACK, 1, 0);
        for _ in 0..3 {
            assert!(processor.erase_or_resume_word_with(
                None,
                event,
                Some(Language::English),
                |_, _| unreachable!(),
            ));
        }
        assert!(processor.replay_keys.is_empty());
        assert!(processor.session.is_suppressed());

        // Other suppression causes still do not start a token.
        processor.last_word_reset = "external-input";
        type_keys(&mut processor, foreground, &[0x1e]);
        assert!(processor.replay_keys.is_empty());
    }

    #[test]
    fn manual_token_never_starts_inside_unknown_text_and_is_bounded() {
        let (mut processor, foreground) = processor_typing_in(0x0409);
        // After navigation the caret position inside the text is unknown.
        processor.process_key(RawKeyEvent {
            scan_code: 0x4b,
            extended: true,
            foreground,
            ..test_raw_key(WM_KEYDOWN, VK_LEFT, 1, 0)
        });
        type_keys(&mut processor, foreground, &[0x07, 0x1e]);
        assert!(processor.replay_keys.is_empty());
        // A key without a replay form, or an overlong token, discards it.
        processor.session.clear();
        type_keys(&mut processor, foreground, &[0x07]);
        let keys = processor.take_manual_token_keys(InputEvent::UnsupportedInput, true, 0);
        processor.extend_manual_token(keys.unwrap(), None);
        assert!(processor.replay_keys.is_empty());
        processor.session.clear();
        type_keys(&mut processor, foreground, &[0x07; MANUAL_TOKEN_MAX_KEYS]);
        assert_eq!(processor.replay_keys.len(), MANUAL_TOKEN_MAX_KEYS);
        type_keys(&mut processor, foreground, &[0x07]);
        assert!(processor.replay_keys.is_empty());
        // Non-printable keys never extend a token.
        assert!(
            processor
                .take_manual_token_keys(InputEvent::UnsupportedInput, false, 0)
                .is_none()
        );
    }

    #[test]
    fn backspace_edits_the_tracked_word_and_its_replay_keys() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        processor.set_privacy_reason(None);
        buffer_test_word(&mut processor, "ghbdnb");
        let event = test_raw_key(WM_KEYDOWN, VK_BACK, 1, 0);
        for expected in [5, 4] {
            assert!(processor.erase_or_resume_word_with(
                None,
                event,
                Some(Language::English),
                |_, _| unreachable!("a buffered word is edited in place"),
            ));
            assert_eq!(processor.session.buffered_character_count(), expected);
            assert_eq!(processor.replay_keys.len(), expected);
            assert!(!processor.session.is_suppressed());
        }
        assert!(!processor.manual_only_word);

        // A misaligned buffer is never edited; the caller discards it instead.
        processor.replay_keys.pop();
        assert!(!processor.erase_or_resume_word_with(
            None,
            event,
            Some(Language::English),
            |_, _| None,
        ));
    }

    #[test]
    fn backspace_over_the_delimiter_resumes_the_word_for_manual_conversion_only() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        processor.set_privacy_reason(None);
        let event = test_raw_key(WM_KEYDOWN, VK_BACK, 1, 0);
        let last = test_last_boundary(&processor, 6);
        assert!(processor.erase_or_resume_word_with(
            Some(last),
            event,
            Some(Language::English),
            |keys, layout| {
                assert_eq!((keys.len(), layout), (6, event.foreground.layout));
                Some("ghbdtn".to_owned())
            },
        ));
        assert_eq!(processor.session.buffered_character_count(), 6);
        assert_eq!(processor.replay_keys.len(), 6);
        assert!(processor.manual_only_word);
        // Retyping the delimiter does not re-trigger automatic conversion.
        assert!(
            processor
                .handle_boundary(Some(Language::English), Some(' '), false)
                .is_none()
        );
    }

    #[test]
    fn backspace_does_not_resume_a_stale_or_different_word() {
        let event = test_raw_key(WM_KEYDOWN, VK_BACK, 1, 0);
        let cases: [fn(&mut LastBoundary); 4] = [
            |last| last.foreground.layout += 1,
            |last| last.foreground.focus += 1,
            |last| last.epoch += 1,
            |last| last.profile_generation += 1,
        ];
        for change in cases {
            let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
            processor.set_privacy_reason(None);
            let mut last = test_last_boundary(&processor, 6);
            change(&mut last);
            assert!(!processor.erase_or_resume_word_with(
                Some(last),
                event,
                Some(Language::English),
                |_, _| Some("ghbdtn".to_owned()),
            ));
            assert_eq!(processor.session.buffered_character_count(), 0);
        }
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        processor.set_privacy_reason(None);
        for word in [None, Some("ghbdt"), Some("gh1dtn")] {
            let last = test_last_boundary(&processor, 6);
            assert!(!processor.erase_or_resume_word_with(
                Some(last),
                event,
                Some(Language::English),
                |_, _| word.map(str::to_owned),
            ));
        }
        processor.suppress_session();
        let last = test_last_boundary(&processor, 6);
        assert!(!processor.erase_or_resume_word_with(
            Some(last),
            event,
            Some(Language::English),
            |_, _| Some("ghbdtn".to_owned()),
        ));
        assert!(!processor.manual_only_word);
    }

    #[test]
    fn terminal_field_failure_keeps_manual_cycle_and_next_word_available() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        let foreground = test_raw_key(WM_KEYDOWN, VK_SPACE, 7, 0).foreground;
        processor.set_privacy_reason(None);
        processor.transpose_cycle = Some(TransposeCycle {
            replay_keys: Vec::new(),
            delimiter: None,
            foreground,
            language: Language::English,
            text: "hello".into(),
            epoch: 0,
            profile_generation: processor.input_profiles.generation(),
            order: Vec::new(),
        });

        processor.set_privacy_reason_with_manual_exception(
            Some(PrivacyBlockReason::FieldInspectionUnavailable),
            true,
        );
        assert!(processor.transpose_cycle.is_some());
        assert!(!processor.session.is_suppressed());

        // Successive words, including a one-letter word, remain bufferable
        // without a trailing Space. A periodic field failure may occur midway.
        for word in ["a", "hello", "ghbdtn"] {
            processor.handle_boundary(Some(Language::English), Some(' '), false);
            for character in word.chars() {
                processor.session.handle(
                    InputEvent::Printable(character),
                    Some(Language::English),
                    &processor.detector,
                );
                processor.replay_keys.push(ReplayKey {
                    scan_code: 0x22,
                    shift: false,
                    caps_lock: false,
                    extended: false,
                });
                processor.set_privacy_reason_with_manual_exception(
                    Some(PrivacyBlockReason::FieldInspectionUnavailable),
                    true,
                );
            }
            assert_eq!(processor.session.buffered_character_count(), word.len());
            assert_eq!(processor.replay_keys.len(), word.len());
            assert!(!processor.session.is_suppressed());
            assert!(processor.manual_only_word);
        }
    }

    #[test]
    fn manual_exception_does_not_preserve_input_for_hard_privacy_failures() {
        for reason in [
            PrivacyBlockReason::PasswordField,
            PrivacyBlockReason::ExcludedProcess,
            PrivacyBlockReason::ElevatedProcess,
            PrivacyBlockReason::InspectionUnavailable,
            PrivacyBlockReason::FieldInspectionUnavailable,
        ] {
            let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
            processor.session.handle(
                InputEvent::Printable('a'),
                Some(Language::English),
                &processor.detector,
            );
            processor.replay_keys.push(ReplayKey {
                scan_code: 0x1e,
                shift: false,
                caps_lock: false,
                extended: false,
            });
            // Field failures without a verified exception also stay denied.
            processor.set_privacy_reason_with_manual_exception(
                Some(reason),
                reason != PrivacyBlockReason::FieldInspectionUnavailable,
            );
            assert_eq!(processor.session.buffered_character_count(), 0);
            assert!(processor.replay_keys.is_empty());
            assert!(processor.session.is_suppressed());
        }
    }

    #[test]
    #[cfg(feature = "legacy-bundled-input")]
    fn recovered_field_inspection_does_not_promote_manual_word_to_auto() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        for manual_only in [true, false] {
            processor.set_privacy_reason(None);
            for (character, scan_code) in "ghbdtn".chars().zip([0x22, 0x23, 0x30, 0x20, 0x14, 0x31])
            {
                processor.session.handle(
                    InputEvent::Printable(character),
                    Some(Language::English),
                    &processor.detector,
                );
                processor.replay_keys.push(ReplayKey {
                    scan_code,
                    shift: false,
                    caps_lock: false,
                    extended: false,
                });
            }
            if manual_only {
                processor.set_privacy_reason_with_manual_exception(
                    Some(PrivacyBlockReason::FieldInspectionUnavailable),
                    true,
                );
                processor.set_privacy_reason(None);
            }
            let conversion = processor.handle_boundary(Some(Language::English), Some(' '), false);
            assert_eq!(conversion.is_none(), manual_only);
            assert!(!processor.manual_only_word);
        }
    }

    const FIELD_FAILURE: Option<PrivacyBlockReason> =
        Some(PrivacyBlockReason::FieldInspectionUnavailable);

    fn recent_ok_foreground() -> ForegroundContext {
        test_raw_key(WM_KEYDOWN, VK_SPACE, 7, 0).foreground
    }

    /// One finished check for the standard target and epoch 5, as typing reports it.
    fn settle(
        processor: &mut InputProcessor,
        reason: Option<PrivacyBlockReason>,
        source: &'static str,
        at: Instant,
    ) -> ProbeVerdict {
        processor.settle_probe_outcome(
            reason,
            source,
            recent_ok_foreground(),
            5,
            at,
            ProbeTrigger::Typing,
        )
    }

    #[test]
    fn a_late_provider_answer_keeps_a_good_verdict_for_two_seconds() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        let start = Instant::now();
        let good = settle(&mut processor, None, "provider", start);
        assert_eq!((good.reason, good.source), (None, "provider"));
        assert_eq!(good.recent_ok_age, None);
        for milliseconds in [0, 120, 400, 1_999] {
            let age = Duration::from_millis(milliseconds);
            let verdict = settle(&mut processor, FIELD_FAILURE, "provider", start + age);
            assert_eq!(verdict.reason, None, "{milliseconds} ms after a good check");
            assert_eq!(verdict.source, "recent-ok");
            assert_eq!(verdict.recent_ok_age, Some(age));
        }
    }

    #[test]
    fn a_failure_that_lasts_is_taken_at_face_value() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        let start = Instant::now();
        let at = |milliseconds| start + Duration::from_millis(milliseconds);
        // Without any good check there is nothing to lean on.
        let verdict = settle(&mut processor, FIELD_FAILURE, "provider", at(0));
        assert_eq!(
            (verdict.reason, verdict.source),
            (FIELD_FAILURE, "provider")
        );
        settle(&mut processor, None, "provider", at(10));
        // Two seconds after the good check the window is over.
        let verdict = settle(&mut processor, FIELD_FAILURE, "provider", at(2_010));
        assert_eq!(verdict.reason, FIELD_FAILURE);
        assert_eq!(verdict.recent_ok_age, None);
        // A failure never renews the good check: leaning on it for 1.5 s must not extend it.
        settle(&mut processor, None, "provider", at(3_000));
        assert_eq!(
            settle(&mut processor, FIELD_FAILURE, "provider", at(4_500)).reason,
            None
        );
        assert_eq!(
            settle(&mut processor, FIELD_FAILURE, "provider", at(5_100)).reason,
            FIELD_FAILURE
        );
    }

    #[test]
    fn the_rule_never_applies_to_another_input_target() {
        let start = Instant::now();
        let later = start + Duration::from_millis(300);
        let foreground = recent_ok_foreground();
        let variants = [
            ForegroundContext {
                hwnd: 9,
                ..foreground
            },
            ForegroundContext {
                focus: 9,
                ..foreground
            },
            ForegroundContext {
                input_thread_id: 9,
                ..foreground
            },
            ForegroundContext {
                process_id: 9,
                ..foreground
            },
        ];
        for other in variants {
            let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
            settle(&mut processor, None, "provider", start);
            let verdict = processor.settle_probe_outcome(
                FIELD_FAILURE,
                "provider",
                other,
                5,
                later,
                ProbeTrigger::Typing,
            );
            assert_eq!(verdict.reason, FIELD_FAILURE, "{other:?}");
        }
        // The input epoch moves on queue overflow, hook reinstalls, configuration changes,
        // toggling automatic conversion and a gate that gave up.
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        settle(&mut processor, None, "provider", start);
        let verdict = processor.settle_probe_outcome(
            FIELD_FAILURE,
            "provider",
            foreground,
            6,
            later,
            ProbeTrigger::Typing,
        );
        assert_eq!(verdict.reason, FIELD_FAILURE);
    }

    #[test]
    fn a_layout_switch_does_not_end_the_window() {
        // A conversion switches the keyboard layout right before the next check, and the user
        // types in alternating layouts. The field is the same, so the rule must still apply.
        let start = Instant::now();
        let foreground = recent_ok_foreground();
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        settle(&mut processor, None, "provider", start);
        let switched = ForegroundContext {
            layout: foreground.layout + 1,
            ..foreground
        };
        let verdict = processor.settle_probe_outcome(
            FIELD_FAILURE,
            "provider",
            switched,
            5,
            start + Duration::from_millis(300),
            ProbeTrigger::Refresh,
        );
        assert_eq!((verdict.reason, verdict.source), (None, "recent-ok"));
    }

    #[test]
    fn only_a_field_inspection_failure_from_the_provider_is_softened() {
        let start = Instant::now();
        let later = start + Duration::from_millis(300);
        for hard in [
            PrivacyBlockReason::PasswordField,
            PrivacyBlockReason::ExcludedProcess,
            PrivacyBlockReason::ElevatedProcess,
            PrivacyBlockReason::InspectionUnavailable,
        ] {
            let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
            settle(&mut processor, None, "provider", start);
            assert_eq!(
                settle(&mut processor, Some(hard), "provider", later).reason,
                Some(hard)
            );
            // A password verdict supersedes the good check: nothing is left to lean on.
            assert_eq!(
                settle(&mut processor, FIELD_FAILURE, "provider", later).reason,
                FIELD_FAILURE,
                "after {hard:?}"
            );
        }
        for source in [
            "wait-timeout",
            "queue-busy",
            "provider-disconnected",
            "reply-expired",
            "context-changed",
            "process-policy",
            "probe-missing",
        ] {
            let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
            settle(&mut processor, None, "provider", start);
            let verdict = settle(&mut processor, FIELD_FAILURE, source, later);
            assert_eq!((verdict.reason, verdict.source), (FIELD_FAILURE, source));
        }
    }

    #[test]
    fn anything_that_can_move_the_focus_ends_the_window() {
        let start = Instant::now();
        let later = start + Duration::from_millis(100);
        for pause_now in [false, true] {
            let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
            settle(&mut processor, None, "provider", start);
            // Tab, Enter, a click, a shortcut, injected input and a focus change all end here.
            processor.mark_privacy_dirty(pause_now);
            let verdict = settle(&mut processor, FIELD_FAILURE, "provider", later);
            assert_eq!(verdict.reason, FIELD_FAILURE, "pause_now={pause_now}");
        }
    }

    #[test]
    fn recovery_and_manual_conversion_take_a_failure_at_face_value() {
        let start = Instant::now();
        let later = start + Duration::from_millis(100);
        for (trigger, softened) in [
            (ProbeTrigger::Typing, true),
            (ProbeTrigger::Refresh, true),
            (ProbeTrigger::Recovery, false),
            (ProbeTrigger::Manual, false),
        ] {
            let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
            settle(&mut processor, None, "provider", start);
            let verdict = processor.settle_probe_outcome(
                FIELD_FAILURE,
                "provider",
                recent_ok_foreground(),
                5,
                later,
                trigger,
            );
            assert_eq!(verdict.reason.is_none(), softened, "{trigger:?}");
        }
    }

    #[test]
    #[cfg(feature = "legacy-bundled-input")]
    fn a_word_typed_through_a_late_privacy_answer_is_still_converted() {
        let start = Instant::now();
        for (good_check_first, converted) in [(true, true), (false, false)] {
            let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
            processor.set_privacy_reason(None);
            if good_check_first {
                settle(&mut processor, None, "provider", start);
            }
            for (character, scan_code) in "ghbdtn".chars().zip([0x22, 0x23, 0x30, 0x20, 0x14, 0x31])
            {
                processor.session.handle(
                    InputEvent::Printable(character),
                    Some(Language::English),
                    &processor.detector,
                );
                processor.replay_keys.push(ReplayKey {
                    scan_code,
                    shift: false,
                    caps_lock: false,
                    extended: false,
                });
            }
            // The late answer arrives in the middle of the word.
            processor.apply_probe_answer(
                ProbeAnswer {
                    reason: FIELD_FAILURE,
                    source: "provider",
                },
                true,
                recent_ok_foreground(),
                5,
                Instant::now(),
                ProbeTrigger::Typing,
            );
            // The next check succeeds, but a word that met a failure never becomes automatic.
            processor.set_privacy_reason(None);
            let conversion = processor.handle_boundary(Some(Language::English), Some(' '), false);
            assert_eq!(
                conversion.is_some(),
                converted,
                "good check first: {good_check_first}"
            );
        }
    }

    fn probe_lines(receiver: &Receiver<String>) -> Vec<String> {
        receiver
            .try_iter()
            .filter(|record| record.starts_with("event=privacy_probe "))
            .collect()
    }

    fn apply(
        processor: &mut InputProcessor,
        reason: Option<PrivacyBlockReason>,
        context_current: bool,
    ) {
        processor.apply_probe_answer(
            ProbeAnswer {
                reason,
                source: "provider",
            },
            context_current,
            recent_ok_foreground(),
            5,
            Instant::now(),
            ProbeTrigger::Typing,
        );
    }

    #[test]
    fn a_late_answer_after_a_good_check_leaves_the_state_and_the_word_alone() {
        let (mut processor, receiver) = processor_with_diagnostics();
        apply(&mut processor, None, true);
        type_letters(&mut processor, "hel");
        apply(&mut processor, FIELD_FAILURE, true);
        assert!(processor.privacy_reason.is_none());
        assert!(!processor.manual_only_word);
        assert!(!processor.privacy_needs_check);
        assert_eq!(processor.session.buffered_character_count(), 3);
        let lines = probe_lines(&receiver);
        let line = lines.last().expect("the softened answer is logged");
        for expected in [
            "result=None",
            "source=recent-ok",
            "trigger=typing",
            " age_ms=",
        ] {
            assert!(line.contains(expected), "{expected}: {line}");
        }
    }

    #[test]
    fn a_late_answer_after_a_layout_switch_is_still_softened() {
        let (mut processor, receiver) = processor_with_diagnostics();
        apply(&mut processor, None, true);
        type_letters(&mut processor, "hel");
        let foreground = recent_ok_foreground();
        processor.apply_probe_answer(
            ProbeAnswer {
                reason: FIELD_FAILURE,
                source: "provider",
            },
            true,
            ForegroundContext {
                layout: foreground.layout + 1,
                ..foreground
            },
            5,
            Instant::now(),
            ProbeTrigger::Refresh,
        );
        assert!(processor.privacy_reason.is_none());
        assert!(!processor.manual_only_word);
        assert_eq!(processor.session.buffered_character_count(), 3);
        let lines = probe_lines(&receiver);
        let line = lines.last().expect("the softened answer is logged");
        assert!(line.contains("source=recent-ok"), "{line}");
        assert!(line.contains("trigger=refresh"), "{line}");
    }

    #[test]
    fn a_failure_without_a_good_check_stands_and_leaves_no_line() {
        let (mut processor, receiver) = processor_with_diagnostics();
        type_letters(&mut processor, "hel");
        apply(&mut processor, FIELD_FAILURE, true);
        assert_eq!(processor.privacy_reason, FIELD_FAILURE);
        assert!(processor.session.is_suppressed());
        // The owner's decision: a check that blocks the field is not written.
        let lines = probe_lines(&receiver);
        assert!(lines.is_empty(), "{lines:?}");
    }

    #[test]
    fn a_changed_context_is_never_softened() {
        let (mut processor, receiver) = processor_with_diagnostics();
        apply(&mut processor, None, true);
        type_letters(&mut processor, "hel");
        apply(&mut processor, FIELD_FAILURE, false);
        assert_eq!(
            processor.privacy_reason,
            Some(PrivacyBlockReason::InspectionUnavailable)
        );
        assert!(processor.session.is_suppressed());
        let lines = probe_lines(&receiver);
        assert!(lines.is_empty(), "{lines:?}");
    }

    fn processor_with_diagnostics() -> (InputProcessor, Receiver<String>) {
        let mut configuration = RuntimeConfiguration::default();
        configuration.settings.diagnostics_enabled = true;
        let (sender, receiver) = sync_channel(64);
        let mut processor = InputProcessor::new_with_lexicon_candidate(
            Arc::new(ObserverMetrics::default()),
            0,
            Arc::new(Mutex::new(None)),
            Some(sender),
            configuration,
        );
        processor.profile_override = Some(test_profiles());
        processor
            .detector
            .set_resolved_profiles(processor.profile_override.as_ref());
        processor.set_privacy_reason(None);
        (processor, receiver)
    }

    fn type_letters(processor: &mut InputProcessor, word: &str) {
        for character in word.chars() {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
            processor.replay_keys.push(ReplayKey {
                scan_code: 0x1e,
                shift: false,
                caps_lock: false,
                extended: false,
            });
        }
    }

    fn word_lines(receiver: &Receiver<String>) -> Vec<String> {
        receiver
            .try_iter()
            .filter(|record| record.starts_with("event=word "))
            .collect()
    }

    #[test]
    fn a_word_that_is_not_converted_leaves_one_line_with_the_reason() {
        type Prepare = fn(&mut InputProcessor);
        let cases: [(&str, Prepare, Option<Language>); 5] = [
            ("no-candidate", |_| {}, Some(Language::English)),
            (
                "manual-only",
                |processor| processor.manual_only_word = true,
                Some(Language::English),
            ),
            (
                "privacy",
                |processor| processor.privacy_reason = Some(PrivacyBlockReason::PasswordField),
                Some(Language::English),
            ),
            (
                "suppressed",
                |processor| {
                    processor.session.handle(
                        InputEvent::UnsupportedInput,
                        Some(Language::English),
                        &processor.detector,
                    );
                    processor.last_word_reset = "unsupported";
                },
                Some(Language::English),
            ),
            ("no-language", |_| {}, None),
        ];
        for (expected, prepare, language) in cases {
            let (mut processor, receiver) = processor_with_diagnostics();
            type_letters(&mut processor, "hello");
            prepare(&mut processor);
            processor.handle_boundary(language, Some(' '), false);
            let lines = word_lines(&receiver);
            // The owner's decision: nothing is written about a word in a blocked field.
            if expected == "privacy" {
                assert!(lines.is_empty(), "{lines:?}");
                continue;
            }
            assert_eq!(lines.len(), 1, "{expected}: {lines:?}");
            assert!(
                lines[0].contains(&format!("result={expected}")),
                "{expected}: {lines:?}"
            );
        }
    }

    #[test]
    fn a_word_cancelled_before_its_boundary_names_what_cancelled_it() {
        // The boundary arms used to record "boundary" before the word was explained, so every
        // suppressed word was reported as reset=boundary. Drive the real arms.
        for (key, name) in [(VK_SPACE, "space"), (VK_TAB, "tab"), (VK_RETURN, "return")] {
            for reset in ["unsupported", "layout", "shortcut", "mouse"] {
                let (mut processor, receiver) = processor_with_diagnostics();
                let event = test_raw_key(WM_KEYDOWN, key, 7, 0);
                processor.last_foreground = Some(foreground_identity_key(event.foreground));
                processor.last_layout = Some(event.foreground.layout);
                processor.last_process_id = Some(event.foreground.process_id);
                type_letters(&mut processor, "hello");
                processor.session.handle(
                    InputEvent::UnsupportedInput,
                    Some(Language::English),
                    &processor.detector,
                );
                processor.last_word_reset = reset;
                processor.process_key(event);
                let lines = word_lines(&receiver);
                assert_eq!(lines.len(), 1, "{name} {reset}: {lines:?}");
                assert!(
                    lines[0].contains(&format!("result=suppressed reset={reset}")),
                    "{name} {reset}: {lines:?}"
                );
                assert_eq!(processor.last_word_reset, "boundary", "{name} {reset}");
            }
        }
    }

    #[test]
    fn an_unsupported_key_is_logged_by_class_only() {
        assert_eq!(unsupported_key_class(None), "no-character");
        for printed in ['1', '\u{2116}', '\u{ab}', ' '] {
            assert_eq!(unsupported_key_class(Some(printed)), "other-character");
        }
    }

    #[test]
    fn numpad_digits_in_a_token_read_back_as_digits_in_every_loaded_layout() {
        // A digit typed on the numeric keypad joins a manual token. Its scan code alone names a
        // navigation key (End, Down and so on), so reading the token back must use the keypad
        // meaning, or Pause cannot convert the token.
        use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutList;
        let count = unsafe { GetKeyboardLayoutList(None) };
        let mut layouts = vec![HKL::default(); usize::try_from(count).unwrap_or(0)];
        let filled = unsafe { GetKeyboardLayoutList(Some(&mut layouts)) };
        layouts.truncate(usize::try_from(filled).unwrap_or(0));
        assert!(!layouts.is_empty(), "no keyboard layout is loaded");
        for layout in layouts {
            for (scan_code, digit) in [
                (0x52, '0'),
                (0x4f, '1'),
                (0x50, '2'),
                (0x51, '3'),
                (0x4b, '4'),
                (0x4c, '5'),
                (0x4d, '6'),
                (0x47, '7'),
                (0x48, '8'),
                (0x49, '9'),
            ] {
                let key = ReplayKey {
                    scan_code,
                    shift: false,
                    caps_lock: false,
                    extended: false,
                };
                assert_eq!(
                    map_replay_key_to_layout(key, layout),
                    Some(digit),
                    "layout {:?}, scan code {scan_code:#x}",
                    layout.0
                );
            }
        }
    }

    #[test]
    fn keypad_scan_codes_read_as_keypad_digits() {
        let digits = [
            (0x52, 0),
            (0x4f, 1),
            (0x50, 2),
            (0x51, 3),
            (0x4b, 4),
            (0x4c, 5),
            (0x4d, 6),
            (0x47, 7),
            (0x48, 8),
            (0x49, 9),
        ];
        for (scan_code, digit) in digits {
            assert_eq!(keypad_digit_key(scan_code), Some(VK_NUMPAD0.0 + digit));
        }
        assert_eq!(keypad_digit_key(0x53), Some(VK_DECIMAL.0));
        // Main-row digits and the keypad operators are not ambiguous and keep their mapping.
        for scan_code in [0x02, 0x0b, 0x4a, 0x4e, 0x37] {
            assert_eq!(keypad_digit_key(scan_code), None, "{scan_code:#x}");
        }
    }

    fn processor_at_the_test_target() -> InputProcessor {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        let event = test_raw_key(WM_KEYDOWN, VK_SPACE, 7, 0);
        processor.last_foreground = Some(foreground_identity_key(event.foreground));
        processor.last_layout = Some(event.foreground.layout);
        processor.last_process_id = Some(event.foreground.process_id);
        processor.set_privacy_reason(None);
        processor
    }

    /// A key event that carries another layout than the last one, as after Alt+Shift.
    fn switch_layout_by_hand(processor: &mut InputProcessor, new_layout: usize) {
        let mut event = test_raw_key(WM_KEYDOWN, VK_SHIFT, 30, 0);
        event.foreground.layout = new_layout;
        processor.process_key(event);
    }

    #[test]
    fn a_manual_layout_switch_keeps_the_word_typed_so_far_for_pause() {
        let mut processor = processor_at_the_test_target();
        let typed_layout = processor.last_layout.expect("set by the fixture");
        type_letters(&mut processor, "ghbdtn");
        switch_layout_by_hand(&mut processor, typed_layout + 1);
        // Automatic conversion of the word ends here; Pause keeps it.
        assert!(processor.session.is_suppressed());
        assert!(processor.replay_keys.is_empty());
        assert_eq!(processor.last_word_reset, "layout");
        let kept = processor
            .last_boundary
            .as_ref()
            .expect("the word stays for Pause");
        assert_eq!(kept.replay_keys.len(), 6);
        assert_eq!(kept.delimiter, None);
        assert_eq!(kept.foreground.layout, typed_layout);
        // Pause reads it in the layout it was typed in, not in the new one.
        let mut pause = test_raw_key(WM_KEYDOWN, VK_PAUSE, 31, 0);
        pause.foreground.layout = typed_layout + 1;
        let source = processor
            .forced_conversion_source(pause)
            .map(|(keys, delimiter, layout)| (keys.len(), delimiter, layout));
        assert_eq!(source, Some((6, None, typed_layout)));
        // Any other key drops it, as it always dropped the last word.
        let mut left = test_raw_key(WM_KEYDOWN, VK_LEFT, 32, 0);
        left.foreground.layout = typed_layout + 1;
        processor.process_key(left);
        assert!(processor.last_boundary.is_none());
    }

    #[test]
    fn a_manual_layout_switch_after_a_space_keeps_the_last_word_for_pause() {
        let mut processor = processor_at_the_test_target();
        let typed_layout = processor.last_layout.expect("set by the fixture");
        processor.last_boundary = Some(test_last_boundary(&processor, 6));
        switch_layout_by_hand(&mut processor, typed_layout + 1);
        let kept = processor
            .last_boundary
            .as_ref()
            .expect("the last word stays for Pause");
        assert_eq!(kept.delimiter, Some(' '));
        assert_eq!(kept.foreground.layout, typed_layout);
        assert_eq!(kept.replay_keys.len(), 6);
    }

    #[test]
    fn text_typed_right_after_a_manual_layout_switch_is_kept_for_pause() {
        let mut processor = processor_at_the_test_target();
        let typed_layout = processor.last_layout.expect("set by the fixture");
        switch_layout_by_hand(&mut processor, typed_layout + 1);
        assert!(processor.session.is_suppressed());
        // The first letter after the switch starts a manual token, as after a shortcut.
        let keys = processor.take_manual_token_keys(InputEvent::Printable('g'), true, 0);
        assert_eq!(keys.map(|keys| keys.len()), Some(0));
        assert!(!processor.manual_token_start_known);
        // Other suppressions still do not start one.
        let mut processor = processor_at_the_test_target();
        processor.session.handle(
            InputEvent::UnsupportedInput,
            Some(Language::English),
            &processor.detector,
        );
        processor.last_word_reset = "backspace";
        assert!(
            processor
                .take_manual_token_keys(InputEvent::Printable('g'), true, 0)
                .is_none()
        );
    }

    #[test]
    fn erasing_after_a_word_kept_by_a_layout_switch_does_not_resume_it() {
        // Without a delimiter there is nothing to erase between the word and the caret: the
        // Backspace removes a letter of the word itself.
        for (delimiter, resumed) in [(Some(' '), true), (None, false)] {
            let mut processor = processor_at_the_test_target();
            let mut kept = test_last_boundary(&processor, 6);
            kept.delimiter = delimiter;
            let event = test_raw_key(WM_KEYDOWN, VK_BACK, 33, 0);
            let result = processor.erase_or_resume_word_with(
                Some(kept),
                event,
                Some(Language::English),
                |_, _| Some("ghbdtn".to_owned()),
            );
            assert_eq!(result, resumed, "{delimiter:?}");
        }
    }

    fn remembered_entry(processor: &InputProcessor) -> &RememberedWord {
        assert_eq!(processor.remembered_words.len(), 1, "one remembered window");
        &processor.remembered_words[0]
    }

    #[test]
    fn a_window_switch_remembers_the_word_typed_so_far() {
        let mut processor = processor_at_the_test_target();
        let target = recent_ok_foreground();
        processor.last_key_foreground = Some(target);
        type_letters(&mut processor, "ghbdtn");
        let mut elsewhere = test_raw_key(WM_KEYDOWN, VK_SHIFT, 40, 0);
        elsewhere.foreground.hwnd = 77;
        processor.process_key(elsewhere);
        let entry = remembered_entry(&processor);
        assert!(same_input_target(entry.target, target));
        let word = entry.word.as_ref().expect("the word typed so far");
        assert_eq!(word.replay_keys.len(), 6);
        assert_eq!(word.delimiter, None);
        assert_eq!(word.foreground.layout, target.layout);
        assert!(entry.cycle.is_none());
        // Tracking itself still starts over in the new window.
        assert!(processor.replay_keys.is_empty());
    }

    #[test]
    fn a_shortcut_remembers_the_last_word() {
        let mut processor = processor_at_the_test_target();
        processor.last_boundary = Some(test_last_boundary(&processor, 6));
        processor.process_key(RawKeyEvent {
            held_modifiers: held_keys(&[VK_LCONTROL]),
            ..test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x43), 41, 0)
        });
        let word = remembered_entry(&processor)
            .word
            .as_ref()
            .expect("the last word");
        assert_eq!(word.delimiter, Some(' '));
        assert_eq!(word.replay_keys.len(), 6);
        assert!(processor.last_boundary.is_none());
    }

    #[test]
    fn a_click_remembers_the_word_typed_so_far() {
        let mut processor = processor_at_the_test_target();
        processor.last_key_foreground = Some(recent_ok_foreground());
        type_letters(&mut processor, "ghbd");
        processor.process(QueuedInputEvent {
            epoch: 0,
            captured_at: Instant::now(),
            event: RawInputEvent::Mouse,
            configuration: None,
        });
        let word = remembered_entry(&processor)
            .word
            .as_ref()
            .expect("the word typed so far");
        assert_eq!(word.replay_keys.len(), 4);
        assert!(processor.replay_keys.is_empty());
    }

    #[test]
    fn remembering_nothing_keeps_the_earlier_word() {
        // Alt+Tab clears the state, and the key that then arrives in another window finds
        // nothing to remember for the window that was left: the word must survive that.
        let mut processor = processor_at_the_test_target();
        let target = recent_ok_foreground();
        let now = Instant::now();
        processor.last_boundary = Some(test_last_boundary(&processor, 6));
        processor.remember_word(target, Some(target.layout), now);
        processor.last_boundary = None;
        processor.remember_word(target, Some(target.layout), now);
        assert_eq!(
            remembered_entry(&processor)
                .word
                .as_ref()
                .map(|word| word.replay_keys.len()),
            Some(6)
        );
    }

    #[test]
    fn only_eight_windows_are_remembered_and_only_for_two_minutes() {
        let mut processor = processor_at_the_test_target();
        let now = Instant::now();
        for hwnd in 1..=9 {
            let target = ForegroundContext {
                hwnd,
                ..recent_ok_foreground()
            };
            let mut word = test_last_boundary(&processor, 3);
            word.foreground = target;
            processor.last_boundary = Some(word);
            processor.remember_word(target, Some(target.layout), now);
        }
        assert_eq!(processor.remembered_words.len(), 8);
        assert_eq!(processor.remembered_words[0].target.hwnd, 2);
        // An entry older than two minutes is gone.
        processor.last_boundary = None;
        processor.text_edit_backend = TextEditBackend::ProtectedPaste;
        let last = ForegroundContext {
            hwnd: 9,
            ..recent_ok_foreground()
        };
        assert_eq!(
            processor.restore_remembered_word(last, now + Duration::from_secs(121)),
            None
        );
        assert!(processor.remembered_words.is_empty());
    }

    #[test]
    fn pause_restores_a_remembered_word_once_and_only_in_a_verified_field() {
        let target = recent_ok_foreground();
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let remembered = |backend: TextEditBackend| {
            let mut processor = processor_at_the_test_target();
            processor.text_edit_backend = backend;
            processor.last_boundary = Some(test_last_boundary(&processor, 6));
            processor.remember_word(target, Some(target.layout), now);
            processor.last_boundary = None;
            processor
        };
        // A protected-paste field checks the text before the caret, so the word is restored.
        let mut processor = remembered(TextEditBackend::ProtectedPaste);
        assert_eq!(
            processor.restore_remembered_word(target, later),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            processor
                .last_boundary
                .as_ref()
                .map(|word| word.replay_keys.len()),
            Some(6)
        );
        // Used once.
        processor.last_boundary = None;
        assert_eq!(processor.restore_remembered_word(target, later), None);
        // Physical replay types blind: never.
        for backend in [
            TextEditBackend::PhysicalReplay,
            TextEditBackend::CapabilityProbe,
            TextEditBackend::ObserveOnly,
        ] {
            let mut processor = remembered(backend);
            assert_eq!(
                processor.restore_remembered_word(target, later),
                None,
                "{backend:?}"
            );
            assert!(processor.last_boundary.is_none());
        }
        // Something known for the field wins over the memory.
        let mut processor = remembered(TextEditBackend::ProtectedPaste);
        type_letters(&mut processor, "a");
        assert_eq!(processor.restore_remembered_word(target, later), None);
        // Another window has nothing remembered.
        let mut processor = remembered(TextEditBackend::ProtectedPaste);
        let other = ForegroundContext { hwnd: 78, ..target };
        assert_eq!(processor.restore_remembered_word(other, later), None);
    }

    fn held_modifiers(keys: &[VIRTUAL_KEY]) -> Modifiers {
        let mut modifiers = Modifiers::default();
        for key in keys {
            modifiers.update(u32::from(key.0), true, false);
        }
        modifiers
    }

    /// `RawKeyEvent::held_modifiers` for these physical modifier keys held.
    fn held_keys(keys: &[VIRTUAL_KEY]) -> u8 {
        keys.iter()
            .map(|key| {
                let bit = PHYSICAL_MODIFIER_KEYS
                    .iter()
                    .position(|candidate| candidate == key)
                    .expect("a left or right modifier key");
                1 << bit
            })
            .fold(0, |held, bit| held | bit)
    }

    #[test]
    fn a_shortcut_is_logged_by_its_editing_command_only() {
        let control = held_modifiers(&[VK_CONTROL]);
        for (key, kind) in [
            (VK_BACK.0, "erase-word"),
            (0x56, "paste"),
            (0x43, "copy"),
            (0x58, "cut"),
            (0x5a, "undo"),
            (0x59, "undo"),
            (0x41, "ctrl"),
        ] {
            assert_eq!(shortcut_kind(u32::from(key), control), kind, "{key:#x}");
        }
        assert_eq!(
            shortcut_kind(u32::from(VK_TAB.0), held_modifiers(&[VK_MENU])),
            "alt"
        );
        assert_eq!(
            shortcut_kind(0x56, held_modifiers(&[VK_CONTROL, VK_MENU])),
            "ctrl-alt"
        );
        assert_eq!(
            shortcut_kind(u32::from(VK_SPACE.0), held_modifiers(&[VK_LWIN])),
            "win"
        );
    }

    #[test]
    fn a_word_ended_by_a_shortcut_leaves_its_kind_in_the_log() {
        let (mut processor, receiver) = processor_with_diagnostics();
        let event = RawKeyEvent {
            held_modifiers: held_keys(&[VK_LCONTROL]),
            ..test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x56), 7, 0)
        };
        processor.last_foreground = Some(foreground_identity_key(event.foreground));
        processor.last_layout = Some(event.foreground.layout);
        processor.last_process_id = Some(event.foreground.process_id);
        processor.process_key(event);
        let lines: Vec<String> = receiver
            .try_iter()
            .filter(|record| record.starts_with("event=input "))
            .collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("result=suppressed reason=shortcut kind=paste"),
            "{lines:?}"
        );
        assert_eq!(processor.last_word_reset, "shortcut");
    }

    #[test]
    fn a_conversion_sound_is_requested_only_when_enabled_and_never_queued() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        let (sender, receiver) = sync_channel(1);
        processor.sound_player = Some(sender);
        processor.request_sound(SoundCue::Converted);
        assert!(receiver.try_recv().is_err(), "off by default");
        processor.settings.conversion_sound = true;
        processor.request_sound(SoundCue::Converted);
        // A cue that arrives while one is waiting is dropped instead of blocking the worker.
        processor.request_sound(SoundCue::Undone);
        assert_eq!(receiver.try_recv().ok(), Some(SoundCue::Converted));
        assert!(receiver.try_recv().is_err());
        processor.request_sound(SoundCue::Undone);
        assert_eq!(receiver.try_recv().ok(), Some(SoundCue::Undone));
    }

    #[test]
    fn a_boundary_without_a_word_leaves_no_word_line() {
        let (mut processor, receiver) = processor_with_diagnostics();
        processor.handle_boundary(Some(Language::English), Some(' '), false);
        assert!(word_lines(&receiver).is_empty());
    }

    #[test]
    #[cfg(feature = "legacy-bundled-input")]
    fn a_converted_word_leaves_its_candidate_line_and_no_word_line() {
        let (mut processor, receiver) = processor_with_diagnostics();
        for (character, scan_code) in "ghbdtn".chars().zip([0x22, 0x23, 0x30, 0x20, 0x14, 0x31]) {
            processor.session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &processor.detector,
            );
            processor.replay_keys.push(ReplayKey {
                scan_code,
                shift: false,
                caps_lock: false,
                extended: false,
            });
        }
        assert!(
            processor
                .handle_boundary(Some(Language::English), Some(' '), false)
                .is_some()
        );
        let records: Vec<String> = receiver.try_iter().collect();
        assert!(
            records
                .iter()
                .any(|record| record.starts_with("event=candidate "))
        );
        assert!(
            !records
                .iter()
                .any(|record| record.starts_with("event=word "))
        );
        assert_no_record_contains(&records, &["ghbdtn", "привет"]);
    }

    #[test]
    fn ordinary_focus_refresh_does_not_suppress_the_first_word() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(metrics, 0);
        processor.set_privacy_reason(None);
        processor.session.handle(
            InputEvent::Printable('g'),
            Some(Language::English),
            &processor.detector,
        );

        processor.mark_privacy_dirty(false);

        assert_eq!(processor.session.buffered_character_count(), 1);
        assert!(!processor.session.is_suppressed());
        assert!(processor.privacy_needs_check);
    }

    #[test]
    fn enter_boundary_rechecks_privacy_without_suppressing_next_word() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(metrics, 0);
        processor.set_privacy_reason(None);
        processor.session.handle(
            InputEvent::Printable('x'),
            Some(Language::English),
            &processor.detector,
        );

        processor.handle_boundary(Some(Language::English), None, true);

        assert!(processor.privacy_needs_check);
        assert!(processor.privacy_reason.is_none());
        assert!(!processor.session.is_suppressed());
    }

    #[test]
    fn external_injection_invalidates_an_existing_undo_record() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        let detection = autokeyboardlayot::Detection {
            source_language: Language::English,
            target_language: Language::Russian,
            original: "ghbdtn".to_owned(),
            replacement: "привет".to_owned(),
            source_score: 0.0,
            target_score: 20.0,
        };
        let transaction = ConversionTransaction::new(&detection, ' ').unwrap();
        let foreground = ForegroundContext {
            hwnd: 1,
            focus: 1,
            input_thread_id: 1,
            process_id: 1,
            layout: 1,
        };
        processor.undo_record = Some(UndoRecord {
            transaction,
            foreground,
            source_layout: 1,
            profile_generation: processor.input_profiles.generation(),
            replay_keys: vec![ReplayKey {
                scan_code: 0x22,
                shift: false,
                caps_lock: false,
                extended: false,
            }],
            edit_strategy: EditStrategy::PhysicalReplay,
        });
        metrics.undo_available.store(true, Ordering::Release);

        processor.process(QueuedInputEvent {
            configuration: None,
            captured_at: Instant::now(),
            epoch: 0,
            event: RawInputEvent::ExternalInjection,
        });
        assert!(processor.undo_record.is_none());
        assert!(!metrics.undo_available.load(Ordering::Acquire));
    }

    #[test]
    fn ordinary_conversion_invalidation_preserves_a_posted_layout_transition() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(metrics, 0);
        processor.layout_switch_in_flight = Some((1, Instant::now() + Duration::from_secs(1)));

        processor.invalidate_conversion_state();

        assert!(processor.layout_switch_in_flight.is_some());
    }

    #[test]
    fn confirmed_layout_transition_does_not_expire_before_next_word() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(metrics, 0);
        processor.last_layout = Some(1);
        processor.layout_switch_in_flight = Some((2, Instant::now() - Duration::from_secs(2)));

        processor.commit_confirmed_layout(2);

        assert_eq!(processor.last_layout, Some(2));
        assert!(processor.layout_switch_in_flight.is_none());
        assert!(!processor.session.is_suppressed());
    }

    #[test]
    fn initializes_the_uia_privacy_client() {
        let guard = PrivacyGuard::new();
        assert!(guard.is_available());
    }

    #[test]
    fn resolves_the_current_process_image_name() {
        let process_id = unsafe { windows::Win32::System::Threading::GetCurrentProcessId() };
        let image = process_image_name(process_id).expect("current process image should resolve");
        assert!(!image.is_empty());
        assert!(process_integrity_level(process_id).is_some());
    }

    fn key_with_repeat(message: u32, virtual_key: VIRTUAL_KEY, repeat: bool) -> RawKeyEvent {
        RawKeyEvent {
            repeat,
            ..test_raw_key(message, virtual_key, 0, 0)
        }
    }

    fn virtual_keys(events: &[RawKeyEvent]) -> Vec<(u32, u32)> {
        events
            .iter()
            .map(|event| (event.message, event.virtual_key))
            .collect()
    }

    #[test]
    fn key_ups_of_keys_pressed_before_the_gate_are_delivered_not_retained() {
        let a = VIRTUAL_KEY(0x41);
        let g = VIRTUAL_KEY(0x47);
        let (retained, releases) = split_delivered_key_releases(vec![
            test_raw_key(WM_KEYDOWN, g, 0, 0),
            test_raw_key(WM_KEYUP, VK_LCONTROL, 0, 0),
        ]);
        assert_eq!(virtual_keys(&retained), [(WM_KEYDOWN, u32::from(g.0))]);
        assert_eq!(
            virtual_keys(&releases),
            [(WM_KEYUP, u32::from(VK_LCONTROL.0))]
        );

        // A key typed completely while the gate held it stays retained as a whole.
        let (retained, releases) = split_delivered_key_releases(vec![
            test_raw_key(WM_KEYDOWN, a, 0, 0),
            test_raw_key(WM_KEYUP, a, 0, 0),
        ]);
        assert_eq!(retained.len(), 2);
        assert!(releases.is_empty());

        // Released, then pressed again: the first key-up leaves, the new press stays.
        let (retained, releases) = split_delivered_key_releases(vec![
            test_raw_key(WM_KEYUP, a, 0, 0),
            test_raw_key(WM_KEYDOWN, a, 0, 0),
        ]);
        assert_eq!(virtual_keys(&retained), [(WM_KEYDOWN, u32::from(a.0))]);
        assert_eq!(virtual_keys(&releases), [(WM_KEYUP, u32::from(a.0))]);

        // Auto-repeats of a key pressed before the gate do not make its key-up retained.
        let (retained, releases) = split_delivered_key_releases(vec![
            key_with_repeat(WM_KEYDOWN, a, true),
            key_with_repeat(WM_KEYDOWN, a, true),
            test_raw_key(WM_KEYUP, a, 0, 0),
        ]);
        assert_eq!(retained.len(), 2);
        assert_eq!(virtual_keys(&releases), [(WM_KEYUP, u32::from(a.0))]);

        // A fresh press followed by repeats owns its key-up.
        let (retained, releases) = split_delivered_key_releases(vec![
            key_with_repeat(WM_KEYDOWN, a, false),
            key_with_repeat(WM_KEYDOWN, a, true),
            test_raw_key(WM_KEYUP, a, 0, 0),
        ]);
        assert_eq!(retained.len(), 3);
        assert!(releases.is_empty());
    }

    #[test]
    fn a_gate_that_held_only_key_ups_retains_nothing_and_keeps_conversion_on() {
        let (mut state, receiver) = test_gate_state(8);
        state.correction_gate.activate(10);
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYUP, VK_SPACE, 0, 0));
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYUP, VK_LSHIFT, 0, 0));
        state.break_correction_gate_fail_open(GateFailureCause::Mouse);
        assert!(!state.correction_gate.active);
        assert!(!has_retained_input(&state.metrics));
        assert!(state.metrics.auto_enabled.load(Ordering::Acquire));
        assert_eq!(state.metrics.conversion_failures.load(Ordering::Acquire), 0);
        assert_eq!(
            virtual_keys(&state.pending_key_releases),
            [
                (WM_KEYUP, u32::from(VK_SPACE.0)),
                (WM_KEYUP, u32::from(VK_LSHIFT.0))
            ]
        );
        assert!(
            !receiver
                .try_iter()
                .any(|queued| matches!(queued.event, RawInputEvent::RecoverInput { .. }))
        );
        assert!(state.decline_reasons().is_empty());
    }

    #[test]
    fn a_held_edge_of_the_same_key_decides_whether_a_gated_key_down_repeats() {
        let down = test_raw_key(WM_KEYDOWN, VK_LSHIFT, 0, 0);
        let other = test_raw_key(WM_KEYUP, VK_RSHIFT, 0, 0);
        // No held edge of the key: the system's key state answers.
        assert!(gated_key_down_repeats(&[other], &down, || true));
        assert!(!gated_key_down_repeats(&[other], &down, || false));
        // Released in the gate: the swallowed key-up never reached the system's key state.
        let released = [test_raw_key(WM_KEYUP, VK_LSHIFT, 0, 0), other];
        assert!(!gated_key_down_repeats(&released, &down, || true));
        // Pressed in the gate: neither did the swallowed key-down.
        let pressed = [released[0], down, other];
        assert!(gated_key_down_repeats(&pressed, &down, || false));
    }

    #[test]
    fn a_key_down_after_a_key_up_of_the_same_key_owns_the_next_key_up() {
        // A key-down marked as a repeat after the key was released in `events` is a new press.
        let (retained, releases) = split_delivered_key_releases(vec![
            test_raw_key(WM_KEYUP, VK_LSHIFT, 0, 0),
            key_with_repeat(WM_KEYDOWN, VK_LSHIFT, true),
            test_raw_key(WM_KEYUP, VK_LSHIFT, 0, 0),
        ]);
        assert_eq!(
            virtual_keys(&retained),
            [
                (WM_KEYDOWN, u32::from(VK_LSHIFT.0)),
                (WM_KEYUP, u32::from(VK_LSHIFT.0))
            ]
        );
        assert_eq!(
            virtual_keys(&releases),
            [(WM_KEYUP, u32::from(VK_LSHIFT.0))]
        );
    }

    #[test]
    fn shift_released_and_pressed_again_in_the_gate_is_recovered_balanced() {
        let (mut state, _receiver) = test_gate_state(8);
        let shift = u32::from(VK_LSHIFT.0);
        state.correction_gate.activate(10);
        // Shift went down before the gate. The gate swallows its key-up, so the system's key
        // state shows it down for the rest of the gate.
        for (message, key) in [
            (WM_KEYUP, VK_LSHIFT),
            (WM_KEYDOWN, VK_LSHIFT),
            (WM_KEYDOWN, VIRTUAL_KEY(0x41)),
            (WM_KEYUP, VIRTUAL_KEY(0x41)),
            (WM_KEYUP, VK_LSHIFT),
            (WM_KEYDOWN, VIRTUAL_KEY(0x42)),
            (WM_KEYUP, VIRTUAL_KEY(0x42)),
        ] {
            let mut raw = test_raw_key(message, key, 0, 0);
            if message == WM_KEYDOWN {
                raw.repeat =
                    gated_key_down_repeats(&state.correction_gate.held, &raw, || key == VK_LSHIFT);
            }
            assert!(state.hold_gated_edge(raw));
        }
        state.break_correction_gate_fail_open(GateFailureCause::Deadline);
        assert_eq!(
            virtual_keys(&state.pending_key_releases),
            [(WM_KEYUP, shift)]
        );
        let retained = state.metrics.retained_input.lock().unwrap().take().unwrap();
        // "Ab": Shift goes up between the letters, and recovery leaves nothing down.
        assert_eq!(
            virtual_keys(&retained.events),
            [
                (WM_KEYDOWN, shift),
                (WM_KEYDOWN, 0x41),
                (WM_KEYUP, 0x41),
                (WM_KEYUP, shift),
                (WM_KEYDOWN, 0x42),
                (WM_KEYUP, 0x42)
            ]
        );
        assert_eq!(
            build_recovery_inputs(&retained.events).unwrap().len(),
            retained.events.len()
        );
    }

    #[test]
    fn a_newer_physical_edge_of_a_key_cancels_its_queued_key_up() {
        let mut pending = vec![
            test_raw_key(WM_KEYUP, VK_LSHIFT, 0, 0),
            test_raw_key(WM_KEYUP, VIRTUAL_KEY(0x41), 0, 0),
        ];
        cancel_superseded_key_release(&mut pending, &test_raw_key(WM_KEYDOWN, VK_LSHIFT, 0, 0));
        assert_eq!(virtual_keys(&pending), [(WM_KEYUP, 0x41)]);
        cancel_superseded_key_release(&mut pending, &test_raw_key(WM_KEYUP, VK_RSHIFT, 0, 0));
        assert_eq!(pending.len(), 1, "another key cancels nothing");
    }

    #[test]
    fn confirming_the_deletion_leaves_keys_the_worker_is_sending_alone() {
        let (mut state, _receiver) = test_gate_state(4);
        let metrics = Arc::clone(&state.metrics);
        let mut record = stalled_record(Instant::now());
        record.recovering = true;
        *metrics.retained_input.lock().unwrap() = Some(record);
        // The worker began typing them while the question was open.
        assert!(!discard_confirmed_retained_input(&mut state));
        assert!(metrics.retained_input.lock().unwrap().is_some());
        // A recovery that did not finish hands the record back for another attempt or deletion.
        metrics
            .retained_input
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .recovering = false;
        assert!(discard_confirmed_retained_input(&mut state));
        assert!(!has_retained_input(&metrics));
        assert!(!discard_confirmed_retained_input(&mut state));
    }

    #[test]
    fn deleting_retained_input_still_releases_keys_pressed_before_the_gate() {
        let (state, _receiver) = test_gate_state(4);
        let metrics = Arc::clone(&state.metrics);
        let start = Instant::now();
        let lifetime = Duration::from_millis(RETAINED_INPUT_LIFETIME_MS);
        let up = test_raw_key(WM_KEYUP, VIRTUAL_KEY(0x41), 0, 0);
        let down = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x42), 0, 0);
        *metrics.retained_input.lock().unwrap() = Some(RetainedInput {
            token: 10,
            foreground: up.foreground,
            events: vec![up, down],
            delivery_uncertain: false,
            expires_at: start + lifetime,
            recovering: false,
            retained_at: Instant::now(),
            auto_recover: false,
            input_mark: 0,
        });
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        expire_retained_input(HWND::default(), start + lifetime);
        let state = take_app_state().unwrap();
        assert!(!has_retained_input(&metrics));
        assert_eq!(
            virtual_keys(&state.pending_key_releases),
            [(WM_KEYUP, 0x41)]
        );
    }

    #[test]
    fn an_engine_fault_hands_held_keys_back_unchanged_to_the_same_target() {
        let (mut state, _receiver) = test_gate_state(8);
        let target = test_raw_key(WM_KEYDOWN, VK_SPACE, 0, 0).foreground;
        state.correction_gate.activate(10);
        state.correction_gate.origin = Some(target);
        let held = [
            test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0),
            test_raw_key(WM_KEYUP, VIRTUAL_KEY(0x47), 0, 0),
            test_raw_key(WM_KEYUP, VK_LCONTROL, 0, 0),
        ];
        state.correction_gate.held.extend(held);
        state.record_engine_fault_with_target(Some(target));
        assert!(!state.correction_gate.active);
        assert!(!has_retained_input(&state.metrics));
        assert_eq!(
            virtual_keys(&state.pending_held_replay),
            virtual_keys(&held)
        );
        assert!(state.pending_key_releases.is_empty());
        assert_eq!(
            state.metrics.last_failure_reason.load(Ordering::Acquire),
            ConversionFailureReason::InternalFault as u8
        );
    }

    #[test]
    fn a_worker_that_stopped_leaves_no_recovery_armed() {
        for worker_dead in [false, true] {
            let (mut state, _receiver) = test_gate_state(8);
            state.metrics.recovery_armed.store(true, Ordering::Release);
            state
                .metrics
                .worker_dead
                .store(worker_dead, Ordering::Release);
            state.record_engine_fault_with_target(None);
            // Only a living worker can still carry out the recovery Pause would ask for.
            assert_eq!(
                state.metrics.recovery_armed.load(Ordering::Acquire),
                !worker_dead
            );
        }
    }

    #[test]
    fn an_engine_fault_keeps_held_keys_for_recovery_when_the_target_changed() {
        let (mut state, _receiver) = test_gate_state(8);
        let origin = test_raw_key(WM_KEYDOWN, VK_SPACE, 0, 0).foreground;
        let elsewhere = ForegroundContext {
            hwnd: 9,
            focus: 9,
            ..origin
        };
        state.correction_gate.activate(10);
        state.correction_gate.origin = Some(origin);
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0));
        state.record_engine_fault_with_target(Some(elsewhere));
        assert!(has_retained_input(&state.metrics));
        assert!(state.pending_held_replay.is_empty());
        // The fault's own pause is recorded, not a gate drain that policy D1 would lift.
        assert_eq!(
            state.metrics.last_failure_reason.load(Ordering::Acquire),
            ConversionFailureReason::InternalFault as u8
        );
        assert_eq!(state.metrics.conversion_failures.load(Ordering::Acquire), 1);
    }

    #[test]
    fn a_stopped_worker_offers_no_recovery_and_drops_its_pending_reload() {
        let (mut state, _receiver) = test_gate_state(4);
        state.metrics.worker_dead.store(true, Ordering::Release);
        state.pending_configuration = Some(ConfigurationReload {
            revision: 3,
            snapshot: RuntimeConfiguration::default(),
        });
        state
            .metrics
            .configuration_pending
            .store(true, Ordering::Release);
        state.record_engine_fault_with_target(None);
        assert!(state.pending_configuration.is_none());
        assert!(!state.metrics.configuration_pending.load(Ordering::Acquire));

        let key = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0);
        *state.metrics.retained_input.lock().unwrap() = Some(RetainedInput {
            token: 10,
            foreground: key.foreground,
            events: vec![key],
            delivery_uncertain: false,
            expires_at: Instant::now() + Duration::from_millis(RETAINED_INPUT_LIFETIME_MS),
            recovering: false,
            retained_at: Instant::now(),
            auto_recover: false,
            input_mark: 0,
        });
        let metrics = Arc::clone(&state.metrics);
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        arm_input_recovery(HWND::default());
        drop(take_app_state());
        assert!(!metrics.recovery_armed.load(Ordering::Acquire));
    }

    #[test]
    fn a_hook_reinstall_queues_the_recovery_in_the_current_epoch() {
        let (mut state, receiver) = test_gate_state(8);
        state.correction_gate.activate(10);
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0));
        state.invalidate_for_hook_reinstall();
        let epoch = state.metrics.input_epoch.load(Ordering::Acquire);
        let recovery = receiver
            .try_iter()
            .find(|queued| matches!(queued.event, RawInputEvent::RecoverInput { .. }))
            .expect("the held keys are handed back automatically");
        assert_eq!(
            recovery.epoch, epoch,
            "the worker must not drop it as stale"
        );
    }

    #[test]
    fn the_lifetime_timer_leaves_keys_alone_while_the_worker_sends_them() {
        let (state, _receiver) = test_gate_state(4);
        let metrics = Arc::clone(&state.metrics);
        let key = test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0);
        let start = Instant::now();
        let lifetime = Duration::from_millis(RETAINED_INPUT_LIFETIME_MS);
        *metrics.retained_input.lock().unwrap() = Some(RetainedInput {
            token: 10,
            foreground: key.foreground,
            events: vec![key],
            delivery_uncertain: false,
            expires_at: start + lifetime,
            recovering: false,
            retained_at: Instant::now(),
            auto_recover: false,
            input_mark: 0,
        });
        assert!(!begin_retained_recovery(&metrics, 11));
        assert!(begin_retained_recovery(&metrics, 10));
        assert!(
            !begin_retained_recovery(&metrics, 10),
            "one recovery at a time"
        );
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        expire_retained_input(HWND::default(), start + lifetime);
        assert!(has_retained_input(&metrics), "not deleted while being sent");
        // An incomplete send keeps the record for another attempt, marked uncertain.
        finish_retained_recovery(&metrics, 10, 1, 2);
        {
            let retained = metrics.retained_input.lock().unwrap();
            let input = retained.as_ref().unwrap();
            assert!(!input.recovering);
            assert!(input.delivery_uncertain);
        }
        assert!(begin_retained_recovery(&metrics, 10));
        finish_retained_recovery(&metrics, 10, 2, 2);
        assert!(!has_retained_input(&metrics));
        // A worker that died while sending does not keep the keys forever.
        *metrics.retained_input.lock().unwrap() = Some(RetainedInput {
            token: 12,
            foreground: key.foreground,
            events: vec![key],
            delivery_uncertain: false,
            expires_at: start + lifetime,
            recovering: true,
            retained_at: Instant::now(),
            auto_recover: false,
            input_mark: 0,
        });
        expire_retained_input(HWND::default(), start + lifetime + RETAINED_RECOVERY_GRACE);
        assert!(!has_retained_input(&metrics));
        drop(take_app_state());
    }

    #[test]
    fn a_finished_worker_with_a_pending_reload_does_not_block_exit() {
        let facts = ShutdownFacts {
            correction_gate_active: false,
            retained_input: false,
            worker_busy: false,
            worker_finished: true,
            configuration_pending: true,
            undo_hotkey_active: false,
            hotkey_waiting_for_release: false,
        };
        assert!(shutdown_blockers(&facts).is_empty());
        assert_eq!(
            shutdown_blockers(&ShutdownFacts {
                worker_finished: false,
                ..facts
            }),
            ["configuration-pending"]
        );
    }

    #[test]
    fn a_reload_that_panicked_is_rejected_and_the_ui_stops_waiting_for_it() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        processor.reject_faulted_configuration();
        assert_eq!(metrics.configuration_rejected.load(Ordering::Acquire), 0);
        processor.applying_configuration = Some(5);
        processor.reject_faulted_configuration();
        assert_eq!(metrics.configuration_rejected.load(Ordering::Acquire), 5);
        assert_eq!(processor.configuration_revision, 5);
        assert!(processor.applying_configuration.is_none());

        let (mut state, _receiver) = test_gate_state(4);
        state.metrics = Arc::clone(&metrics);
        state.pending_configuration = Some(ConfigurationReload {
            revision: 5,
            snapshot: RuntimeConfiguration::default(),
        });
        metrics.configuration_pending.store(true, Ordering::Release);
        assert!(state.drop_rejected_configuration());
        assert!(state.pending_configuration.is_none());
        assert!(!metrics.configuration_pending.load(Ordering::Acquire));
        assert!(!state.drop_rejected_configuration());
    }

    #[test]
    fn a_recovery_request_is_handled_after_the_epoch_moved_and_while_a_reload_is_pending() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        let (sender, diagnostics) = sync_channel(16);
        processor.diagnostic_sender = Some(sender);
        metrics.diagnostics_enabled.store(true, Ordering::Release);
        // The paste of its gate may have left the word selected: the worker answers without
        // sending anything.
        *metrics.retained_input.lock().unwrap() = Some(stalled_record(Instant::now()));
        metrics.selection_left_token.store(10, Ordering::Release);
        let mut request = |epoch| {
            processor.process_event(QueuedInputEvent {
                epoch,
                captured_at: Instant::now(),
                event: RawInputEvent::RecoverInput {
                    sequence: 0,
                    explicit: false,
                },
                configuration: None,
            });
            diagnostics
                .try_iter()
                .any(|line| line.contains("result=retained reason=selection-left"))
        };
        // A configuration was published after the request was queued.
        let queued_epoch = metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        assert!(request(queued_epoch));
        metrics.configuration_pending.store(true, Ordering::Release);
        assert!(request(metrics.input_epoch.load(Ordering::Acquire)));
        assert_eq!(
            processor.last_input_epoch,
            metrics.input_epoch.load(Ordering::Acquire)
        );
    }

    #[test]
    fn a_recovery_leaves_the_word_at_the_caret_suppressed_until_the_next_boundary() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        processor.finish_recovery(10, true, 2, 2);
        assert!(processor.session.is_suppressed());
        // The epoch reset of an automatic resume keeps it suppressed.
        let epoch = metrics.input_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        processor.reset_for_epoch(epoch);
        assert!(processor.session.is_suppressed());
        // A later reset without a recovery clears the session as usual.
        let epoch = metrics.input_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        processor.reset_for_epoch(epoch);
        assert!(!processor.session.is_suppressed());
    }

    #[test]
    fn the_word_a_recovery_left_ends_at_punctuation_a_click_and_a_focus_change() {
        type EndWord = fn(&mut InputProcessor, ForegroundContext);
        let endings: [(&str, EndWord); 3] = [
            ("punctuation", |processor, foreground| {
                // "/", which no target layout reads as a letter.
                processor.process_key(RawKeyEvent {
                    scan_code: 0x35,
                    foreground,
                    ..letter(0xbf, 1)
                });
            }),
            ("click", |processor, _| {
                processor.process_event(QueuedInputEvent {
                    epoch: processor.metrics.input_epoch.load(Ordering::Acquire),
                    captured_at: Instant::now(),
                    event: RawInputEvent::Mouse,
                    configuration: None,
                });
            }),
            ("focus", |processor, foreground| {
                let elsewhere = ForegroundContext {
                    hwnd: foreground.hwnd + 8,
                    focus: foreground.focus + 8,
                    ..foreground
                };
                processor.foreground_override = Some(elsewhere);
                processor.process_key(RawKeyEvent {
                    scan_code: 0x1e,
                    foreground: elsewhere,
                    ..letter(0x41, 1)
                });
            }),
        ];
        for (ending, end_word) in endings {
            let (mut processor, foreground) = processor_typing_in(0x0409);
            // An explicit recovery: the input epoch stays.
            processor.finish_recovery(10, true, 2, 2);
            end_word(&mut processor, foreground);
            assert!(!processor.recovered_mid_word, "{ending}");
            // Turning automatic conversion back on later must not suppress the next word.
            let epoch = processor.metrics.input_epoch.fetch_add(1, Ordering::AcqRel) + 1;
            processor.reset_for_epoch(epoch);
            assert!(!processor.session.is_suppressed(), "{ending}");
        }
    }

    #[test]
    fn an_edit_that_failed_because_of_the_abort_keeps_conversion_paused() {
        let paused_by_abort = |metrics: &ObserverMetrics| {
            metrics.auto_enabled.store(false, Ordering::Release);
            metrics.safety_paused.store(true, Ordering::Release);
            metrics
                .last_failure_reason
                .store(ConversionFailureReason::GateDrain as u8, Ordering::Release);
            metrics.cancelled_gate_token.store(7, Ordering::Release);
        };
        let metrics = Arc::new(ObserverMetrics::default());
        paused_by_abort(&metrics);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        processor.record_conversion_failure(ConversionFailureReason::PhysicalEdit);
        processor.resume_after_recovered_abort(Instant::now());
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));

        // Without a failed edit the same abort resumes.
        let metrics = Arc::new(ObserverMetrics::default());
        paused_by_abort(&metrics);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        processor.resume_after_recovered_abort(Instant::now());
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
    }

    #[test]
    fn an_injected_click_ends_the_gate_like_a_physical_one() {
        for injected in [false, true] {
            let (mut state, receiver) = test_gate_state(8);
            state.correction_gate.activate(10);
            state
                .correction_gate
                .held
                .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0));
            observe_mouse_button(&mut state, injected);
            assert!(!state.correction_gate.active, "injected={injected}");
            assert!(has_retained_input(&state.metrics));
            let expected = |event: &RawInputEvent| {
                if injected {
                    matches!(event, RawInputEvent::ExternalInjection)
                } else {
                    matches!(event, RawInputEvent::Mouse)
                }
            };
            assert!(receiver.try_iter().any(|queued| expected(&queued.event)));
        }
    }

    #[test]
    fn an_unanswered_gate_acknowledgement_is_no_refusal() {
        assert_eq!(gate_ack_outcome(true, 1, false), GateAckOutcome::Accepted);
        assert_eq!(gate_ack_outcome(true, 0, false), GateAckOutcome::Refused);
        assert_eq!(gate_ack_outcome(false, 0, true), GateAckOutcome::Unanswered);
        assert_eq!(gate_ack_outcome(false, 0, false), GateAckOutcome::Refused);
        assert!(fail_open_after_ack(GateAckOutcome::Refused));
        assert!(!fail_open_after_ack(GateAckOutcome::Unanswered));
        assert!(!fail_open_after_ack(GateAckOutcome::Accepted));
    }

    #[test]
    fn key_ups_released_after_a_conversion_never_end_its_undo() {
        let rollover = [
            test_raw_key(WM_KEYUP, VIRTUAL_KEY(0x4e), 0, 0),
            test_raw_key(WM_KEYUP, VK_SPACE, 0, 0),
        ];
        assert!(!held_tail_invalidates_undo(&rollover));
        let modifier = [test_raw_key(WM_KEYDOWN, VK_LSHIFT, 0, 0)];
        assert!(!held_tail_invalidates_undo(&modifier));
        let typed = [
            test_raw_key(WM_KEYDOWN, VK_LSHIFT, 0, 0),
            test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x41), 0, 0),
        ];
        assert!(held_tail_invalidates_undo(&typed));
    }

    #[test]
    fn held_pause_and_num_lock_are_replayed_as_their_virtual_keys() {
        let pause = RawKeyEvent {
            scan_code: 0x45,
            ..test_raw_key(WM_KEYDOWN, VK_PAUSE, 0, 0)
        };
        let num_lock = RawKeyEvent {
            scan_code: 0x45,
            extended: true,
            ..test_raw_key(WM_KEYDOWN, VK_NUMLOCK, 0, 0)
        };
        let inputs = build_raw_replay_inputs(&[pause, num_lock], INJECTED_EVENT_MARKER).unwrap();
        for (input, key) in inputs.iter().zip([VK_PAUSE, VK_NUMLOCK]) {
            let keyboard = unsafe { input.Anonymous.ki };
            assert_eq!(keyboard.wVk, key);
            assert_eq!(keyboard.dwFlags & KEYEVENTF_SCANCODE, Default::default());
        }
    }

    #[test]
    fn a_hotkey_on_a_modifier_key_up_still_reports_the_key_up_during_recovery() {
        let raw = RawKeyEvent {
            hotkey_trigger: true,
            ..test_raw_key(WM_KEYUP, VK_LCONTROL, 9, 0)
        };
        let (key, recovery) = retained_hotkey_events(raw, true, true);
        assert!(matches!(
            key,
            Some(RawInputEvent::Key(RawKeyEvent {
                hotkey_trigger: false,
                message: WM_KEYUP,
                ..
            }))
        ));
        assert!(matches!(
            recovery,
            Some(RawInputEvent::RecoverInput {
                sequence: 9,
                explicit: true
            })
        ));
        let (key, recovery) = retained_hotkey_events(raw, false, false);
        assert!(key.is_none());
        assert!(recovery.is_none());
    }

    #[test]
    fn the_gate_timer_retains_held_keys_and_closes_an_empty_gate() {
        let (mut state, _receiver) = test_gate_state(8);
        let metrics = Arc::clone(&state.metrics);
        state.correction_gate.activate(10);
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0));
        state.correction_gate.deadline = Instant::now() - Duration::from_millis(1);
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        expire_correction_gate();
        let state = take_app_state().unwrap();
        assert!(!state.correction_gate.active);
        assert_eq!(
            metrics
                .retained_input
                .lock()
                .unwrap()
                .as_ref()
                .map(|input| input.events.len()),
            Some(1)
        );

        let (mut state, _receiver) = test_gate_state(8);
        let metrics = Arc::clone(&state.metrics);
        state.correction_gate.activate(11);
        state.correction_gate.awaiting_worker_ack = true;
        state.correction_gate.deadline = Instant::now() - Duration::from_millis(1);
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        expire_correction_gate();
        let state = take_app_state().unwrap();
        assert!(!state.correction_gate.active);
        assert!(!has_retained_input(&metrics));
        assert!(metrics.auto_enabled.load(Ordering::Acquire));

        // Waiting for the worker does not make held keys disposable: they are retained.
        let (mut state, _receiver) = test_gate_state(8);
        let metrics = Arc::clone(&state.metrics);
        state.correction_gate.activate(12);
        state.correction_gate.awaiting_worker_ack = true;
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0));
        state.correction_gate.deadline = Instant::now() - Duration::from_millis(1);
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        expire_correction_gate();
        drop(take_app_state());
        assert!(has_retained_input(&metrics));
    }

    #[test]
    fn a_late_drain_acknowledgement_with_held_keys_breaks_the_gate() {
        let (mut state, receiver) = test_gate_state(8);
        let metrics = Arc::clone(&state.metrics);
        state.correction_gate.activate(10);
        state.correction_gate.awaiting_worker_ack = true;
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0));
        state.correction_gate.deadline = Instant::now() - Duration::from_millis(1);
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        assert!(!acknowledge_correction_gate_drain(10, 0));
        let state = take_app_state().unwrap();
        assert!(!state.correction_gate.active);
        assert!(has_retained_input(&metrics));
        assert!(receiver.try_iter().any(|queued| matches!(
            queued.event,
            RawInputEvent::RecoverInput {
                explicit: false,
                ..
            }
        )));

        // In time and with nothing left to replay, the acknowledgement closes the gate.
        let (mut state, _receiver) = test_gate_state(8);
        state.correction_gate.activate(12);
        state.correction_gate.awaiting_worker_ack = true;
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        assert!(acknowledge_correction_gate_drain(12, 0));
        let state = take_app_state().unwrap();
        assert!(!state.correction_gate.active);
        assert!(!has_retained_input(&state.metrics));
    }

    #[test]
    fn an_edge_beyond_a_full_gate_joins_the_retained_input() {
        let (mut state, _receiver) = test_gate_state(8);
        assert!(!state.hold_gated_edge(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0)));
        state.correction_gate.activate(10);
        for _ in 0..CORRECTION_GATE_CAPACITY {
            assert!(state.hold_gated_edge(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0)));
        }
        assert!(state.correction_gate.active);
        assert!(state.hold_gated_edge(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x48), 0, 0)));
        assert!(!state.correction_gate.active);
        let retained = state.metrics.retained_input.lock().unwrap();
        let events = &retained.as_ref().unwrap().events;
        assert_eq!(events.len(), CORRECTION_GATE_CAPACITY + 1);
        assert_eq!(events.last().unwrap().virtual_key, 0x48);
    }

    #[test]
    fn held_keys_come_back_automatically_only_after_the_four_recoverable_causes() {
        use GateFailureCause::*;
        for cause in [
            Deadline,
            Capacity,
            FocusChanged,
            Mouse,
            ExternalInjection,
            WorkerQueue,
            ReplaySubmit,
            WorkerAck,
            DownstreamHook,
            InternalFault,
            HookReinstall,
            SelectionLeft,
        ] {
            let (mut state, receiver) = test_gate_state(8);
            state.correction_gate.activate(10);
            state
                .correction_gate
                .held
                .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0));
            state.break_correction_gate(cause, None);
            if cause != InternalFault {
                let retained = state.metrics.retained_input.lock().unwrap().clone();
                assert_eq!(
                    retained.map(|input| input.auto_recover),
                    Some(matches!(
                        cause,
                        Deadline | Capacity | WorkerQueue | HookReinstall
                    )),
                    "{cause:?}"
                );
            }
            let recovers = receiver.try_iter().any(|queued| {
                matches!(
                    queued.event,
                    RawInputEvent::RecoverInput {
                        explicit: false,
                        ..
                    }
                )
            });
            assert_eq!(
                recovers,
                matches!(cause, Deadline | Capacity | WorkerQueue | HookReinstall),
                "{cause:?}"
            );
        }
    }

    #[test]
    fn a_gate_release_request_posts_exactly_one_message_with_its_token() {
        let module = unsafe { GetModuleHandleW(None) }.unwrap();
        // An invisible window owned by this thread, so that its queue can be inspected.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("AutoKeyboardLayot gate release test"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
        }
        .unwrap();
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(true, Ordering::Release);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), window.0 as usize);
        processor.request_gate_release(77);
        let mut message = MSG::default();
        let take = |message: &mut MSG| unsafe {
            PeekMessageW(
                message,
                Some(window),
                GATE_RELEASE_MESSAGE,
                GATE_RELEASE_MESSAGE,
                PM_REMOVE,
            )
            .as_bool()
        };
        assert!(take(&mut message));
        assert_eq!(message.wParam.0, 77);
        assert!(!take(&mut message), "exactly one release request");
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
        // A window that is gone cannot release the gate: the request fails it open instead.
        unsafe { DestroyWindow(window) }.unwrap();
        processor.request_gate_release(78);
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
        assert_eq!(
            metrics.last_failure_reason.load(Ordering::Acquire),
            ConversionFailureReason::GateDrain as u8
        );
    }

    const EDITOR: &str = r"C:\Program Files\Editor\editor.exe";

    const KEEPASS: &str = r"C:\Program Files\KeePass\KeePass.exe";

    fn assert_no_record_contains(records: &[String], texts: &[&str]) {
        for record in records {
            for text in texts {
                assert!(!record.contains(text), "{text:?} in {record}");
            }
        }
    }

    /// The input target of `test_raw_key`.
    fn test_foreground() -> ForegroundContext {
        test_raw_key(WM_KEYDOWN, VK_SPACE, 0, 0).foreground
    }

    fn letter(virtual_key: u16, sequence: u64) -> RawKeyEvent {
        test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(virtual_key), sequence, 0)
    }

    /// A key event queued `waited` ago, so the worker would note a slow queue.
    fn queued_key(
        processor: &InputProcessor,
        event: RawKeyEvent,
        waited: Duration,
    ) -> QueuedInputEvent {
        QueuedInputEvent {
            epoch: processor.metrics.input_epoch.load(Ordering::Acquire),
            captured_at: Instant::now() - waited,
            event: RawInputEvent::Key(event),
            configuration: None,
        }
    }

    type TestProbe = BoundedProbe<(ForegroundContext, u64, u64), Option<PrivacyBlockReason>>;

    /// A privacy probe whose provider answers every request with `answer` at once.
    fn answering_probe(answer: Option<PrivacyBlockReason>) -> TestProbe {
        BoundedProbe::spawn("test-privacy", move || move |_| answer).unwrap()
    }

    /// A privacy probe whose provider waits for a message on `gate` (or for its sender to be
    /// dropped) before each answer. The n-th request it handles is answered with `answer(n)`.
    fn gated_probe(
        gate: Receiver<()>,
        answer: impl Fn(usize) -> Option<PrivacyBlockReason> + Send + 'static,
    ) -> TestProbe {
        BoundedProbe::spawn("test-privacy-gated", move || {
            let mut handled = 0;
            move |_| {
                let _ = gate.recv();
                handled += 1;
                answer(handled)
            }
        })
        .unwrap()
    }

    /// A processor with diagnostics whose input target, `test_raw_key`'s, has the focus, in a
    /// process with `image` at the agent's own integrity level, and whose privacy provider
    /// answers `answer`.
    fn processor_in_field(
        image: &str,
        answer: Option<PrivacyBlockReason>,
    ) -> (InputProcessor, Receiver<String>) {
        let (mut processor, receiver) = processor_with_diagnostics();
        processor.foreground_override = Some(test_foreground());
        processor.process_override = Some((Some(image.to_owned()), Some(0x2000)));
        processor.current_integrity_level = Some(0x2000);
        processor.privacy_probe = Some(answering_probe(answer));
        processor.privacy_needs_check = true;
        (processor, receiver)
    }

    #[test]
    fn typing_in_a_password_field_leaves_no_line_once_the_field_is_known() {
        let (mut processor, receiver) =
            processor_in_field(EDITOR, Some(PrivacyBlockReason::PasswordField));
        processor.process_key(letter(0x53, 1));
        // Only the program the window belongs to; the check that found the password field is
        // not written.
        let first: Vec<String> = receiver.try_iter().collect();
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(first[0].starts_with("event=context "), "{first:?}");
        assert_eq!(
            processor.privacy_reason,
            Some(PrivacyBlockReason::PasswordField)
        );
        // Every further key, slow in the queue or not, and the word's end leave nothing.
        for (index, key) in [0x45u16, 0x43, 0x52, 0x45, 0x54].into_iter().enumerate() {
            let event = letter(key, index as u64 + 2);
            let queued = queued_key(&processor, event, Duration::from_millis(100));
            processor.process(queued);
        }
        assert_eq!(processor.last_word_reset, "privacy");
        let space = test_raw_key(WM_KEYDOWN, VK_SPACE, 7, 0);
        let queued = queued_key(&processor, space, Duration::from_millis(100));
        processor.process(queued);
        let typed: Vec<String> = receiver.try_iter().collect();
        assert!(typed.is_empty(), "{typed:?}");
    }

    #[test]
    fn typing_in_an_excluded_program_leaves_no_line_at_all() {
        let (mut processor, receiver) = processor_in_field(KEEPASS, None);
        for (index, key) in [0x53u16, 0x45, 0x43].into_iter().enumerate() {
            processor.process_key(letter(key, index as u64 + 1));
        }
        processor.process_key(test_raw_key(WM_KEYDOWN, VK_SPACE, 4, 0));
        let records: Vec<String> = receiver.try_iter().collect();
        assert!(records.is_empty(), "{records:?}");
        assert_eq!(
            processor.privacy_reason,
            Some(PrivacyBlockReason::ExcludedProcess)
        );
    }

    #[test]
    fn the_privacy_check_of_a_key_tells_an_allowed_field_from_a_blocked_one() {
        for (image, integrity, answer, blocked) in [
            (EDITOR, Some(0x2000), None, false),
            (
                EDITOR,
                Some(0x2000),
                Some(PrivacyBlockReason::PasswordField),
                true,
            ),
            (
                EDITOR,
                Some(0x2000),
                Some(PrivacyBlockReason::FieldInspectionUnavailable),
                true,
            ),
            (KEEPASS, Some(0x2000), None, true),
            // An elevated program, and one whose integrity cannot be read.
            (EDITOR, Some(0x3000), None, true),
            (EDITOR, None, None, true),
        ] {
            let (mut processor, _receiver) = processor_in_field(image, answer);
            processor.process_override = Some((Some(image.to_owned()), integrity));
            processor.process_key(letter(0x41, 1));
            let case = format!("{image} {integrity:?} {answer:?}");
            assert_eq!(processor.privacy_reason.is_some(), blocked, "{case}");
            assert_eq!(processor.last_word_reset == "privacy", blocked, "{case}");
            // An allowed key goes on to the word (here a key the test layout cannot type).
            assert!(!blocked || processor.session.is_suppressed(), "{case}");
        }
    }

    #[test]
    fn every_failed_privacy_check_blocks_the_field() {
        let unavailable = Some(PrivacyBlockReason::InspectionUnavailable);
        let mut sources = Vec::new();
        for failure in [
            ProbeFailure::Timeout,
            ProbeFailure::Busy,
            ProbeFailure::Disconnected,
            ProbeFailure::Expired,
        ] {
            let (reason, source) = probe_verdict(Err(failure));
            assert_eq!(reason, unavailable, "{failure:?}");
            sources.push(source);
        }
        sources.sort_unstable();
        sources.dedup();
        assert_eq!(sources.len(), 4, "each failure names itself");
        assert_eq!(probe_verdict(Ok(None)), (None, "provider"));
        assert_eq!(
            probe_verdict(Ok(Some(PrivacyBlockReason::PasswordField))),
            (Some(PrivacyBlockReason::PasswordField), "provider")
        );

        // Through the key: a provider that does not answer in time, one that is gone, and no
        // provider at all.
        let (gate, waiting) = sync_channel::<()>(1);
        let gone: TestProbe = BoundedProbe::spawn("test-privacy-gone", || {
            |_: (ForegroundContext, u64, u64)| -> Option<PrivacyBlockReason> {
                panic!("the provider is gone")
            }
        })
        .unwrap();
        for (case, probe) in [
            ("timeout", Some(gated_probe(waiting, |_| None))),
            ("disconnected", Some(gone)),
            ("missing", None),
        ] {
            let (mut processor, _receiver) = processor_in_field(EDITOR, None);
            processor.privacy_probe = probe;
            processor.process_key(letter(0x41, 1));
            assert_eq!(processor.privacy_reason, unavailable, "{case}");
            assert_eq!(processor.last_word_reset, "privacy", "{case}");
            assert!(processor.session.is_suppressed(), "{case}");
        }
        drop(gate);

        // A reply for another input target never authorizes this one.
        let (mut processor, _receiver) = processor_in_field(EDITOR, None);
        processor.foreground_override = Some(ForegroundContext {
            focus: 99,
            ..test_foreground()
        });
        processor.evaluate_privacy(test_foreground(), ProbeTrigger::Typing);
        assert_eq!(processor.privacy_reason, unavailable);
    }

    #[test]
    fn a_pause_check_holds_only_for_the_input_target_and_epoch_it_began_with() {
        let foreground = test_foreground();
        let (mut processor, _receiver) = processor_in_field(EDITOR, None);
        assert!(processor.forced_privacy_allows(foreground));
        // Another control has the focus.
        processor.foreground_override = Some(ForegroundContext {
            focus: 99,
            ..foreground
        });
        assert!(!processor.forced_privacy_allows(foreground));
        // The input epoch moves on while the field is checked.
        processor.foreground_override = Some(foreground);
        let metrics = Arc::clone(&processor.metrics);
        processor.privacy_probe = Some(
            BoundedProbe::spawn("test-privacy-epoch", move || {
                move |_: (ForegroundContext, u64, u64)| {
                    metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
                    None
                }
            })
            .unwrap(),
        );
        assert!(!processor.forced_privacy_allows(foreground));
    }

    #[test]
    fn an_answer_requested_before_tab_never_decides_the_field_after_it() {
        // The provider answers the first field (before Tab) as an ordinary one and every later
        // one as a password field, each only when the test lets it.
        let (release, gate) = sync_channel::<()>(4);
        let (mut processor, _receiver) = processor_in_field(EDITOR, None);
        processor.privacy_probe = Some(gated_probe(gate, |handled| {
            (handled > 1).then_some(PrivacyBlockReason::PasswordField)
        }));
        processor.process_key(letter(0x41, 1));
        assert_eq!(
            processor.privacy_reason,
            Some(PrivacyBlockReason::InspectionUnavailable),
            "no answer in time"
        );
        // The answer for the field before Tab arrives late, and Tab moves to the password field.
        release.send(()).unwrap();
        thread::sleep(Duration::from_millis(20));
        processor.process_key(test_raw_key(WM_KEYDOWN, VK_TAB, 2, 0));
        processor.process_key(letter(0x42, 3));
        assert!(processor.privacy_reason.is_some());
        assert_eq!(processor.last_word_reset, "privacy");
        drop(release);
    }

    #[test]
    fn keys_that_can_move_the_focus_within_a_window_ask_for_a_fresh_privacy_check() {
        // Esc, F1, F6, F24 and the menu key; Tab, Shift+Tab and Enter; Ctrl+Tab and an Alt
        // accelerator.
        let none: &[VIRTUAL_KEY] = &[];
        for (modifiers, key) in [
            (none, VK_ESCAPE),
            (none, VK_APPS),
            (none, VK_F1),
            (none, VIRTUAL_KEY(0x75)),
            (none, VK_F24),
            (none, VK_TAB),
            (&[VK_LSHIFT][..], VK_TAB),
            (none, VK_RETURN),
            (&[VK_CONTROL][..], VK_TAB),
            (&[VK_MENU][..], VIRTUAL_KEY(0x46)),
        ] {
            let (mut processor, _receiver) = processor_in_field(EDITOR, None);
            processor.process_key(letter(0x41, 1));
            assert!(processor.privacy_reason.is_none(), "{key:?}");
            assert!(!processor.privacy_needs_check, "{key:?}");
            processor.recent_privacy_ok = Some(RecentPrivacyOk {
                foreground: test_foreground(),
                epoch: 0,
                at: Instant::now(),
            });
            let generation = processor.privacy_generation;
            processor.modifiers = held_modifiers(modifiers);
            processor.process_key(test_raw_key(WM_KEYDOWN, key, 2, 0));
            assert!(processor.privacy_needs_check, "{key:?}");
            assert!(processor.recent_privacy_ok.is_none(), "{key:?}");
            assert_ne!(processor.privacy_generation, generation, "{key:?}");
        }
        for typing in [VK_SPACE.0, 0x41, VK_LEFT.0, VK_BACK.0] {
            assert!(!may_move_focus_within_window(typing), "{typing:#x}");
        }
    }

    #[test]
    fn a_slow_keyboard_hook_is_not_logged_while_privacy_blocks_the_field() {
        assert!(slow_hook_may_be_logged("keyboard", PRIVACY_ALLOWED));
        assert!(slow_hook_may_be_logged("mouse", PRIVACY_PASSWORD));
        for blocked in [
            PRIVACY_PASSWORD,
            PRIVACY_EXCLUDED,
            PRIVACY_UNAVAILABLE,
            PRIVACY_ELEVATED,
        ] {
            assert!(!slow_hook_may_be_logged("keyboard", blocked), "{blocked}");
        }
    }

    #[test]
    fn a_new_window_reads_its_process_again_even_with_the_same_process_id() {
        let (mut processor, _receiver) = processor_in_field(EDITOR, None);
        processor.process_key(letter(0x41, 1));
        assert!(processor.process_reason.is_none());
        // The program closes, and another one gets its process ID and a new window.
        processor.process_override = Some((Some(KEEPASS.to_owned()), Some(0x2000)));
        let moved = ForegroundContext {
            hwnd: 50,
            focus: 50,
            ..test_foreground()
        };
        processor.foreground_override = Some(moved);
        let mut event = letter(0x42, 2);
        event.foreground = moved;
        processor.process_key(event);
        assert_eq!(
            processor.process_reason,
            Some(PrivacyBlockReason::ExcludedProcess)
        );
        assert_eq!(processor.last_word_reset, "privacy");
    }

    #[test]
    fn a_pending_configuration_drops_the_word_in_an_allowed_field() {
        let (mut processor, receiver) = processor_in_field(EDITOR, None);
        type_letters(&mut processor, "hel");
        processor
            .metrics
            .configuration_pending
            .store(true, Ordering::Release);
        let queued = queued_key(&processor, letter(0x4c, 1), Duration::ZERO);
        processor.process(queued);
        assert_eq!(processor.session.buffered_character_count(), 0);
        assert!(processor.replay_keys.is_empty());
        assert!(processor.privacy_reason.is_none());
        let lines: Vec<String> = receiver.try_iter().collect();
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("event=word_discard ")
                    && line.contains("reason=configuration chars=3")),
            "{lines:?}"
        );
    }

    #[test]
    fn the_worker_takes_a_privacy_refresh_and_lets_the_next_one_be_queued() {
        let (state, receiver) = test_gate_state(4);
        let foreground = test_foreground();
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        processor.foreground_override = Some(foreground);
        processor.process_override = Some((Some(EDITOR.to_owned()), Some(0x2000)));
        processor.current_integrity_level = Some(0x2000);
        let asked = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&asked);
        processor.privacy_probe = Some(
            BoundedProbe::spawn("test-privacy-refresh", move || {
                move |_: (ForegroundContext, u64, u64)| {
                    counter.fetch_add(1, Ordering::AcqRel);
                    None
                }
            })
            .unwrap(),
        );
        assert!(state.enqueue(RawInputEvent::PrivacyRefresh(foreground)));
        assert!(state.metrics.privacy_refresh_queued.load(Ordering::Acquire));
        processor.process(receiver.try_recv().unwrap());
        assert!(!state.metrics.privacy_refresh_queued.load(Ordering::Acquire));
        assert_eq!(asked.load(Ordering::Acquire), 1);
        assert!(processor.privacy_reason.is_none());
        // The next refresh is queued again, and while a gate holds keys it asks nothing.
        assert!(state.enqueue(RawInputEvent::PrivacyRefresh(foreground)));
        state.metrics.active_gate_token.store(9, Ordering::Release);
        processor.process(receiver.try_recv().expect("queued again"));
        assert!(!state.metrics.privacy_refresh_queued.load(Ordering::Acquire));
        assert_eq!(asked.load(Ordering::Acquire), 1);
    }

    /// The kinds of the refresh events waiting in `receiver`.
    fn queued_refreshes(receiver: &Receiver<QueuedInputEvent>) -> Vec<&'static str> {
        receiver
            .try_iter()
            .map(|queued| match queued.event {
                RawInputEvent::ProfileRefresh => "profiles",
                RawInputEvent::PrivacyRefresh(_) => "privacy",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn every_timer_tick_refreshes_the_keyboard_profiles_also_over_the_shell() {
        let (mut state, receiver) = test_gate_state(8);
        let foreground = test_foreground();
        let now = Instant::now();
        // The shell gets no field check, but its keyboard profiles age like any other.
        queue_periodic_refreshes(&mut state, foreground, true, now);
        assert_eq!(queued_refreshes(&receiver), ["profiles"]);
        state
            .metrics
            .profile_refresh_queued
            .store(false, Ordering::Release);
        queue_periodic_refreshes(&mut state, foreground, false, now);
        assert_eq!(queued_refreshes(&receiver), ["profiles", "privacy"]);
        // The field check keeps its own interval; the profiles come with every tick.
        state
            .metrics
            .profile_refresh_queued
            .store(false, Ordering::Release);
        state
            .metrics
            .privacy_refresh_queued
            .store(false, Ordering::Release);
        queue_periodic_refreshes(&mut state, foreground, false, now);
        assert_eq!(queued_refreshes(&receiver), ["profiles"]);
    }

    #[test]
    fn a_profile_refresh_waits_once_in_the_queue_and_a_full_queue_only_drops_it() {
        let (state, receiver) = test_gate_state(1);
        assert!(state.enqueue(RawInputEvent::ProfileRefresh));
        assert!(state.enqueue(RawInputEvent::ProfileRefresh));
        assert_eq!(queued_refreshes(&receiver), ["profiles"]);
        // Still marked as queued until the worker takes it: the next tick sends nothing.
        assert!(state.enqueue(RawInputEvent::ProfileRefresh));
        assert!(receiver.try_recv().is_err());
        state
            .metrics
            .profile_refresh_queued
            .store(false, Ordering::Release);
        assert!(state.enqueue(RawInputEvent::Mouse));
        let epoch = state.metrics.input_epoch.load(Ordering::Acquire);
        assert!(!state.enqueue(RawInputEvent::ProfileRefresh));
        assert_eq!(state.metrics.input_epoch.load(Ordering::Acquire), epoch);
        assert_eq!(state.metrics.dropped_events.load(Ordering::Acquire), 0);
        assert!(!state.metrics.profile_refresh_queued.load(Ordering::Acquire));
    }

    #[test]
    fn a_profile_refresh_polls_the_keyboard_profiles_unless_a_gate_holds_keys() {
        let (state, receiver) = test_gate_state(4);
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        processor.profile_override = None;
        let asked = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&asked);
        let snapshot = test_profiles();
        processor.input_profiles = KeyboardProfileCache::with_resolver(
            BoundedProbe::spawn("test-profiles", move || {
                move |()| {
                    counter.fetch_add(1, Ordering::AcqRel);
                    Ok(snapshot.clone())
                }
            })
            .unwrap(),
        );
        state.metrics.active_gate_token.store(9, Ordering::Release);
        assert!(state.enqueue(RawInputEvent::ProfileRefresh));
        processor.process(receiver.try_recv().unwrap());
        assert!(!state.metrics.profile_refresh_queued.load(Ordering::Acquire));
        thread::sleep(Duration::from_millis(20));
        assert_eq!(asked.load(Ordering::Acquire), 0, "a gate holds keys");
        state.metrics.active_gate_token.store(0, Ordering::Release);
        // The provider answers a request on a later refresh.
        let deadline = Instant::now() + Duration::from_secs(5);
        while processor.input_profiles.generation() == 0 && Instant::now() < deadline {
            assert!(state.enqueue(RawInputEvent::ProfileRefresh));
            processor.process(receiver.try_recv().unwrap());
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(processor.input_profiles.generation(), 1);
        assert!(asked.load(Ordering::Acquire) >= 1);
        assert_eq!(processor.resolved_profiles(), Some(&test_profiles()));
    }

    #[test]
    fn a_pause_press_over_a_word_never_writes_the_word() {
        let (mut processor, receiver) = processor_in_field(EDITOR, None);
        processor.settings.pause_break_undo = true;
        type_letters(&mut processor, "secret");
        let pause = RawKeyEvent {
            hotkey_trigger: true,
            ..test_raw_key(WM_KEYDOWN, VK_PAUSE, 5, 0)
        };
        processor
            .metrics
            .observed_input_sequence
            .store(5, Ordering::Release);
        processor.last_foreground = Some(foreground_identity_key(pause.foreground));
        processor.last_layout = Some(pause.foreground.layout);
        processor.last_process_id = Some(pause.foreground.process_id);
        processor.process_key(pause);
        let records: Vec<String> = receiver.try_iter().collect();
        assert!(
            records
                .iter()
                .any(|record| record.starts_with("event=hotkey ")),
            "{records:?}"
        );
        assert_no_record_contains(&records, &["secret"]);
    }

    #[test]
    fn the_barrier_report_never_carries_the_text_it_compared() {
        let report = classify_text_barrier(
            "привет\u{a0}",
            "привет ",
            TextBarrierStage::TextCompared,
            -7,
        );
        let line = format_text_barrier_report(report);
        assert!(line.contains("nbsp_equiv=true"), "{line}");
        assert_no_record_contains(&[line], &["привет"]);
    }

    /// A scripted text field. The caret is at the end of `text`, and `selected` characters
    /// before it are selected (shared, so a test can look after the field was moved).
    struct FakeField {
        text: Vec<char>,
        selected: std::rc::Rc<Cell<usize>>,
        selection_fails: bool,
        not_one_range: bool,
        range_unreadable: bool,
        clone_fails: bool,
        move_short: bool,
        text_fails: bool,
        /// Select() reports failure; with `select_applies` the field selects all the same.
        select_fails: bool,
        select_applies: bool,
        /// The field selects one character less than asked.
        select_short: bool,
        /// From the first Select() on, the selection cannot be read.
        unreadable_after_select: bool,
        /// Moving a range's start onto its end fails.
        collapse_fails: bool,
        /// Selecting an empty range is accepted but changes nothing.
        ignores_collapse: bool,
        selected_once: Cell<bool>,
    }

    struct FakeRange {
        start: Cell<usize>,
        end: usize,
    }

    fn fake_field(text: &str) -> FakeField {
        FakeField {
            text: text.chars().collect(),
            selected: std::rc::Rc::new(Cell::new(0)),
            selection_fails: false,
            not_one_range: false,
            range_unreadable: false,
            clone_fails: false,
            move_short: false,
            text_fails: false,
            select_fails: false,
            select_applies: false,
            select_short: false,
            unreadable_after_select: false,
            collapse_fails: false,
            ignores_collapse: false,
            selected_once: Cell::new(false),
        }
    }

    impl TextField for FakeField {
        type Range = FakeRange;

        fn selection(&self) -> SelectionRead<FakeRange> {
            if self.selection_fails || (self.unreadable_after_select && self.selected_once.get()) {
                return SelectionRead::Unreadable;
            }
            if self.not_one_range {
                return SelectionRead::NotOneRange;
            }
            if self.range_unreadable {
                return SelectionRead::RangeUnreadable;
            }
            let end = self.text.len();
            SelectionRead::One(FakeRange {
                start: Cell::new(end - self.selected.get()),
                end,
            })
        }

        fn is_empty(&self, range: &FakeRange) -> Option<bool> {
            Some(range.start.get() == range.end)
        }

        fn clone_range(&self, range: &FakeRange) -> Option<FakeRange> {
            (!self.clone_fails).then(|| FakeRange {
                start: Cell::new(range.start.get()),
                end: range.end,
            })
        }

        fn move_start(&self, range: &FakeRange, units: i32) -> Option<i32> {
            let old = i32::try_from(range.start.get()).unwrap();
            let new = (old + units).max(0);
            range.start.set(usize::try_from(new).unwrap());
            Some(if self.move_short {
                units + 1
            } else {
                new - old
            })
        }

        fn text(&self, range: &FakeRange) -> Option<String> {
            (!self.text_fails).then(|| self.text[range.start.get()..range.end].iter().collect())
        }

        fn select(&self, range: &FakeRange) -> bool {
            if self.ignores_collapse && range.start.get() == range.end {
                return true;
            }
            self.selected_once.set(true);
            if !self.select_fails || self.select_applies {
                let length = range.end - range.start.get();
                let short = usize::from(self.select_short && length > 0);
                self.selected.set(length - short);
            }
            !self.select_fails
        }

        fn collapse_to_end(&self, range: &FakeRange) -> bool {
            if self.collapse_fails {
                return false;
            }
            range.start.set(range.end);
            true
        }
    }

    /// One selection attempt for "ghbdtn " on `field`; returns it, its report and how many
    /// characters stay selected.
    fn select_in(field: FakeField, earlier: bool) -> (SelectionAttempt, TextBarrierReport, usize) {
        let expected = "ghbdtn ";
        let selected = std::rc::Rc::clone(&field.selected);
        let report = Cell::new(TextBarrierReport {
            expected_chars: expected.chars().count(),
            ..TextBarrierReport::default()
        });
        let attempt = select_preceding_text_in(
            expected,
            Duration::from_millis(20),
            earlier,
            &report,
            || Ok(field),
        );
        (attempt, report.get(), selected.get())
    }

    #[test]
    fn a_selection_the_field_confirms_is_kept() {
        let (attempt, report, selected) = select_in(fake_field("hello ghbdtn "), false);
        assert_eq!(attempt.status, TextBarrierStatus::Match);
        assert_eq!(attempt.confirmation, Some(SelectionConfirmation::Confirmed));
        assert_eq!(attempt.cleanup, None);
        assert_eq!(report.cleanup, None);
        assert_eq!(selected, 7);
    }

    #[test]
    fn a_select_call_that_fails_is_still_collapsed_and_read_back() {
        // The field applied the selection although the call reported failure.
        let (attempt, report, selected) = select_in(
            FakeField {
                select_fails: true,
                select_applies: true,
                ..fake_field("hello ghbdtn ")
            },
            false,
        );
        assert_eq!(attempt.status, TextBarrierStatus::Mismatch);
        assert_eq!(attempt.cleanup, Some(SelectionCleanup::Collapsed));
        assert_eq!(report.stage, TextBarrierStage::SelectCall);
        assert_eq!(report.cleanup, Some(SelectionCleanup::Collapsed));
        assert_eq!(selected, 0);
        // When it cannot be collapsed, the selection is reported left.
        let (attempt, report, selected) = select_in(
            FakeField {
                select_fails: true,
                select_applies: true,
                collapse_fails: true,
                ..fake_field("hello ghbdtn ")
            },
            false,
        );
        assert_eq!(attempt.cleanup, Some(SelectionCleanup::Unverified));
        assert_eq!(report.cleanup, Some(SelectionCleanup::Unverified));
        assert_eq!(selected, 7);
    }

    #[test]
    fn a_selection_that_cannot_be_read_back_or_collapsed_is_reported_left() {
        // A busy field: the selection is made, then nothing can be read any more.
        let (attempt, report, _) = select_in(
            FakeField {
                unreadable_after_select: true,
                ..fake_field("hello ghbdtn ")
            },
            false,
        );
        assert_eq!(
            attempt.confirmation,
            Some(SelectionConfirmation::Unreadable)
        );
        assert_eq!(attempt.cleanup, Some(SelectionCleanup::Unverified));
        assert_eq!(report.stage, TextBarrierStage::SelectionConfirm);
        assert_eq!(report.cleanup, Some(SelectionCleanup::Unverified));
        // The field shows other text and refuses to collapse it.
        let (attempt, _, selected) = select_in(
            FakeField {
                select_short: true,
                collapse_fails: true,
                ..fake_field("hello ghbdtn ")
            },
            false,
        );
        assert_eq!(attempt.confirmation, Some(SelectionConfirmation::OtherText));
        assert_eq!(attempt.cleanup, Some(SelectionCleanup::Unverified));
        assert_eq!(selected, 6);
        // The same field, collapsing: the selection is gone and that is confirmed.
        let (attempt, report, selected) = select_in(
            FakeField {
                select_short: true,
                ..fake_field("hello ghbdtn ")
            },
            false,
        );
        assert_eq!(attempt.cleanup, Some(SelectionCleanup::Collapsed));
        assert_eq!(report.cleanup, Some(SelectionCleanup::Collapsed));
        assert_eq!(selected, 0);
    }

    #[test]
    fn a_late_selection_found_by_a_second_attempt_is_collapsed_and_a_users_is_not() {
        let late = || {
            let field = fake_field("hello ghbdtn ");
            field.selected.set(7);
            field
        };
        let (attempt, report, selected) = select_in(late(), true);
        assert_eq!(attempt.status, TextBarrierStatus::Mismatch);
        assert_eq!(report.stage, TextBarrierStage::CaretNotEmpty);
        assert_eq!(attempt.cleanup, Some(SelectionCleanup::Collapsed));
        assert_eq!(selected, 0);
        let (attempt, _, selected) = select_in(late(), false);
        assert_eq!(attempt.cleanup, None);
        assert_eq!(
            selected, 7,
            "the first attempt leaves a selection it finds alone"
        );
    }

    /// A barrier answer that names itself through the report's character count.
    fn marked_barrier_answer(mark: usize) -> (TextBarrierStatus, TextBarrierReport) {
        (
            TextBarrierStatus::Match,
            TextBarrierReport {
                stage: TextBarrierStage::TextCompared,
                expected_chars: mark,
                ..TextBarrierReport::default()
            },
        )
    }

    #[test]
    fn a_barrier_read_that_never_returns_holds_the_worker_only_until_its_deadline() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&spawned);
        let (hanging, hung) = std::sync::mpsc::channel::<()>();
        // The first provider hangs in its first read for good, once it has said so; the ones
        // after it answer.
        let mut probe: BarrierProbe = BoundedProbe::spawn_resilient(
            "barrier-hang-test",
            move || {
                let hangs = counter.fetch_add(1, Ordering::SeqCst) == 0;
                let hanging = hanging.clone();
                move |_: BarrierRead| {
                    if hangs {
                        let _ = hanging.send(());
                        loop {
                            thread::park();
                        }
                    }
                    marked_barrier_answer(41)
                }
            },
            Duration::from_millis(300),
            1,
        )
        .expect("barrier provider");
        let read = |text: &str| BarrierRead {
            process_id: 7,
            text: text.to_owned(),
        };
        // A read whose deadline has come does not wait at all, whatever the provider does.
        let started = Instant::now();
        let (status, report) = barrier_answer(&mut probe, read("ghbdtn "), started);
        assert_eq!(status, TextBarrierStatus::Unavailable);
        assert_eq!(report.stage, TextBarrierStage::Provider);
        assert_eq!(report.expected_chars, 7);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "{:?}",
            started.elapsed()
        );
        // The provider has taken that read and hangs in it. The next read waits in the queue no
        // longer than its own deadline.
        hung.recv_timeout(Duration::from_secs(10))
            .expect("the provider took the read");
        let next = Instant::now();
        let (status, _) =
            barrier_answer(&mut probe, read("vbh "), next + Duration::from_millis(50));
        assert_eq!(status, TextBarrierStatus::Unavailable);
        assert!(
            next.elapsed() < Duration::from_millis(400),
            "{:?}",
            next.elapsed()
        );
        // With the queue full, a read is refused at once, however long it may wait.
        let refused = Instant::now();
        let (status, report) =
            barrier_answer(&mut probe, read("lf "), refused + Duration::from_secs(30));
        assert_eq!(
            (status, report.stage),
            (TextBarrierStatus::Unavailable, TextBarrierStage::Provider)
        );
        assert!(
            refused.elapsed() < Duration::from_millis(400),
            "{:?}",
            refused.elapsed()
        );
        // Stuck past its time, the provider is replaced and a fresh one answers.
        thread::sleep(Duration::from_millis(350));
        let (status, report) = barrier_answer(
            &mut probe,
            read("lf "),
            Instant::now() + Duration::from_secs(5),
        );
        assert_eq!(
            (status, report.expected_chars),
            (TextBarrierStatus::Match, 41)
        );
        assert_eq!(probe.respawn_count(), 1);
        assert_eq!(spawned.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_text_barrier_asks_its_provider_and_keeps_its_report() {
        let metrics = Arc::new(ObserverMetrics::default());
        let processor = InputProcessor::new(metrics, 0);
        *processor.barrier_probe.borrow_mut() = Some(
            BoundedProbe::spawn("barrier-answer-test", || {
                |read: BarrierRead| marked_barrier_answer(read.text.chars().count() + 100)
            })
            .expect("barrier provider"),
        );
        let status =
            processor.barrier_text_status(7, "привет ", Instant::now() + Duration::from_secs(5));
        assert_eq!(status, TextBarrierStatus::Match);
        assert_eq!(
            processor.privacy_guard.last_barrier_report().expected_chars,
            107
        );
        // Without a provider the barrier is unavailable, never a match.
        *processor.barrier_probe.borrow_mut() = None;
        let status =
            processor.barrier_text_status(7, "привет ", Instant::now() + Duration::from_secs(5));
        assert_eq!(status, TextBarrierStatus::Unavailable);
        assert_eq!(
            processor.privacy_guard.last_barrier_report().stage,
            TextBarrierStage::Provider
        );
    }

    #[test]
    fn a_collapse_the_field_ignores_gives_up_within_its_budget() {
        // The collapse call fails, or it is accepted and the selection stays: read back, it is
        // never shown gone.
        for field in [
            FakeField {
                collapse_fails: true,
                ..fake_field("hello ghbdtn ")
            },
            FakeField {
                ignores_collapse: true,
                ..fake_field("hello ghbdtn ")
            },
        ] {
            field.selected.set(7);
            let started = Instant::now();
            assert_eq!(collapse_selection(&field), SelectionCleanup::Unverified);
            assert!(started.elapsed() < Duration::from_millis(SELECTION_COLLAPSE_BUDGET_MS + 100));
            assert_eq!(field.selected.get(), 7);
            field.selected.set(0);
            assert_eq!(collapse_selection(&field), SelectionCleanup::Collapsed);
        }
    }

    #[test]
    fn every_early_exit_of_the_barrier_names_its_own_stage() {
        type Break = fn(&mut FakeField);
        let cases: [(Break, TextBarrierStage, TextBarrierStatus); 8] = [
            (
                |field| field.selection_fails = true,
                TextBarrierStage::Selection,
                TextBarrierStatus::Unavailable,
            ),
            (
                |field| field.not_one_range = true,
                TextBarrierStage::SelectionRanges,
                TextBarrierStatus::Mismatch,
            ),
            (
                |field| field.range_unreadable = true,
                TextBarrierStage::Caret,
                TextBarrierStatus::Unavailable,
            ),
            (
                |field| field.selected.set(2),
                TextBarrierStage::CaretNotEmpty,
                TextBarrierStatus::Mismatch,
            ),
            (
                |field| field.clone_fails = true,
                TextBarrierStage::RangeClone,
                TextBarrierStatus::Unavailable,
            ),
            (
                |field| field.move_short = true,
                TextBarrierStage::RangeMove,
                TextBarrierStatus::Mismatch,
            ),
            (
                |field| field.text_fails = true,
                TextBarrierStage::TextRead,
                TextBarrierStatus::Unavailable,
            ),
            (
                |field| field.text = "hello world ".chars().collect(),
                TextBarrierStage::TextCompared,
                TextBarrierStatus::Mismatch,
            ),
        ];
        let mut stages = vec![TextBarrierStage::TextLength, TextBarrierStage::SelectCall];
        for (break_field, stage, status) in cases {
            let mut field = fake_field("hello ghbdtn ");
            break_field(&mut field);
            let report = Cell::new(TextBarrierReport::default());
            let result = read_preceding_text(&field, 7, "ghbdtn ", &report);
            assert_eq!(result.err(), Some(status), "{stage:?}");
            assert_eq!(report.get().stage, stage);
            stages.push(stage);
        }
        // A text the barrier does not check stops before the field is even looked for.
        for text in [String::new(), "x".repeat(129)] {
            let report = Cell::new(TextBarrierReport::default());
            let attempt = select_preceding_text_in(
                &text,
                Duration::from_millis(1),
                false,
                &report,
                || -> core::result::Result<FakeField, TextBarrierStatus> { panic!("resolved") },
            );
            assert_eq!(attempt.status, TextBarrierStatus::Unavailable);
            assert_eq!(report.get().stage, TextBarrierStage::TextLength);
        }
        let count = stages.len();
        stages.sort_by_key(|stage| format!("{stage:?}"));
        stages.dedup();
        assert_eq!(
            stages.len(),
            count,
            "every early exit has a stage of its own"
        );
    }

    #[test]
    fn a_collapse_adds_its_result_to_the_report_without_overwriting_the_step() {
        let guard = PrivacyGuard {
            automation: None,
            com_initialized: false,
            last_barrier_report: Cell::new(TextBarrierReport {
                stage: TextBarrierStage::TextCompared,
                expected_chars: 7,
                ..TextBarrierReport::default()
            }),
        };
        // Without UI Automation the field cannot even be found: the collapse is unverified.
        assert_eq!(
            guard.collapse_selection_to_end(3),
            SelectionCleanup::Unverified
        );
        let report = guard.last_barrier_report();
        assert_eq!(report.stage, TextBarrierStage::TextCompared);
        assert_eq!(report.resolver_reason, EditableResolverReason::None);
        assert_eq!(report.cleanup, Some(SelectionCleanup::Unverified));
        assert!(format_text_barrier_report(report).ends_with(",cleanup=Some(Unverified)"));
    }

    #[test]
    fn a_paste_that_may_leave_the_word_selected_keeps_the_held_keys_for_recovery() {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(true, Ordering::Release);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        assert!(
            processor
                .apply_paste_attempt_result(TextEditAttempt::SelectionLeft, 42, PasteRoute::Backend)
                .is_none()
        );
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
        assert_eq!(
            metrics.last_failure_reason.load(Ordering::Acquire),
            ConversionFailureReason::SelectionLeft as u8
        );
        assert_eq!(
            conversion_failure_label(ConversionFailureReason::SelectionLeft as u8),
            "selection-left"
        );
        processor.end_conversion_gate(42);
        assert_eq!(processor.last_gate_retention_token, 42);
        assert_eq!(processor.last_gate_release_token, 42);
        // The next conversion's gate is released as usual.
        processor.end_conversion_gate(43);
        assert_eq!(processor.last_gate_release_token, 43);
        assert_eq!(processor.last_gate_retention_token, 42);
        // An undo whose paste may leave the word selected keeps its held keys as well.
        assert!(
            processor
                .apply_paste_attempt_result(TextEditAttempt::SelectionLeft, 44, PasteRoute::Backend)
                .is_none()
        );
        processor.execute_undo(44);
        assert_eq!(processor.last_gate_retention_token, 44);
    }

    #[test]
    fn a_retention_request_posts_a_fail_open_that_names_the_left_selection() {
        let module = unsafe { GetModuleHandleW(None) }.unwrap();
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("AutoKeyboardLayot gate retention test"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(module.into()),
                None,
            )
        }
        .unwrap();
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), window.0 as usize);
        processor.request_gate_retention(77);
        let mut message = MSG::default();
        let taken = unsafe {
            PeekMessageW(
                &mut message,
                Some(window),
                GATE_FAIL_OPEN_MESSAGE,
                GATE_FAIL_OPEN_MESSAGE,
                PM_REMOVE,
            )
            .as_bool()
        };
        unsafe { DestroyWindow(window) }.unwrap();
        assert!(taken);
        assert_eq!(message.wParam.0, 77);
        assert_eq!(
            fail_open_cause(message.lParam.0),
            GateFailureCause::SelectionLeft
        );
        assert_eq!(fail_open_cause(0), GateFailureCause::WorkerAck);
    }

    #[test]
    fn a_fail_open_for_a_left_selection_retains_the_keys_without_replaying_them() {
        let (mut state, receiver) = test_gate_state(8);
        let metrics = Arc::clone(&state.metrics);
        state.correction_gate.activate(10);
        state
            .correction_gate
            .held
            .push(test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0));
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        fail_open_correction_gate(10, GateFailureCause::SelectionLeft);
        let state = take_app_state().unwrap();
        assert!(!state.correction_gate.active);
        assert!(state.pending_held_replay.is_empty());
        let retained = metrics.retained_input.lock().unwrap().clone();
        let retained = retained.expect("the held key is retained");
        assert_eq!(retained.events.len(), 1);
        assert!(!retained.auto_recover);
        assert!(!receiver.try_iter().any(|queued| matches!(
            queued.event,
            RawInputEvent::RecoverInput {
                explicit: false,
                ..
            }
        )));
    }

    fn stalled_record(retained_at: Instant) -> RetainedInput {
        RetainedInput {
            token: 10,
            foreground: test_foreground(),
            events: vec![test_raw_key(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0, 0)],
            delivery_uncertain: false,
            expires_at: retained_at + Duration::from_millis(RETAINED_INPUT_LIFETIME_MS),
            recovering: false,
            retained_at,
            auto_recover: true,
            input_mark: 0,
        }
    }

    #[test]
    fn held_keys_come_back_from_the_ui_thread_only_past_a_stalled_worker() {
        let foreground = test_foreground();
        let now = Instant::now();
        let record = stalled_record(now);
        let unavailable = Some(PrivacyBlockReason::InspectionUnavailable);
        let ready = HandBackFacts {
            now: now + Duration::from_millis(STALLED_WORKER_HAND_BACK_MS),
            worker_stalled: true,
            typed_input: 0,
            selection_left_token: 0,
            modifiers_released: true,
            current: Some(foreground),
            privacy: None,
        };
        assert!(stalled_worker_may_hand_back(&record, ready));
        assert!(stalled_worker_may_hand_back(
            &record,
            HandBackFacts {
                privacy: unavailable,
                ..ready
            }
        ));
        // Each condition alone keeps the keys back.
        for (case, facts) in [
            (
                "too early",
                HandBackFacts {
                    now: ready.now - Duration::from_millis(1),
                    ..ready
                },
            ),
            (
                "worker not stalled",
                HandBackFacts {
                    worker_stalled: false,
                    ..ready
                },
            ),
            (
                "typed or clicked since",
                HandBackFacts {
                    typed_input: 1,
                    ..ready
                },
            ),
            (
                "modifier held",
                HandBackFacts {
                    modifiers_released: false,
                    ..ready
                },
            ),
            (
                "its paste may have left the word selected",
                HandBackFacts {
                    selection_left_token: record.token,
                    ..ready
                },
            ),
            (
                "another control",
                HandBackFacts {
                    current: Some(ForegroundContext {
                        focus: 9,
                        ..foreground
                    }),
                    ..ready
                },
            ),
            (
                "no foreground",
                HandBackFacts {
                    current: None,
                    ..ready
                },
            ),
        ] {
            assert!(!stalled_worker_may_hand_back(&record, facts), "{case}");
        }
        for blocked in [
            PrivacyBlockReason::PasswordField,
            PrivacyBlockReason::ExcludedProcess,
            PrivacyBlockReason::ElevatedProcess,
        ] {
            let facts = HandBackFacts {
                privacy: Some(blocked),
                ..ready
            };
            assert!(!stalled_worker_may_hand_back(&record, facts), "{blocked:?}");
            assert_eq!(
                privacy_reason_from_code(privacy_reason_code(Some(blocked))),
                Some(blocked)
            );
        }
        for changed in [
            RetainedInput {
                auto_recover: false,
                ..record.clone()
            },
            RetainedInput {
                recovering: true,
                ..record.clone()
            },
            RetainedInput {
                delivery_uncertain: true,
                ..record.clone()
            },
        ] {
            assert!(!stalled_worker_may_hand_back(&changed, ready));
        }
        assert_eq!(privacy_reason_from_code(PRIVACY_ALLOWED), None);
        assert_eq!(privacy_reason_from_code(PRIVACY_UNAVAILABLE), unavailable);
    }

    /// The scan code and whether it is a key-up, of every keyboard input.
    fn scan_edges(inputs: &[INPUT]) -> Vec<(u16, bool)> {
        inputs
            .iter()
            .map(|input| {
                let key = unsafe { input.Anonymous.ki };
                (key.wScan, key.dwFlags.contains(KEYEVENTF_KEYUP))
            })
            .collect()
    }

    #[test]
    fn held_keys_the_ui_thread_returns_leave_no_key_down() {
        let (mut state, _receiver) = test_gate_state(8);
        let edge = |message, key, scan_code| RawKeyEvent {
            scan_code,
            ..test_raw_key(message, key, 0, 0)
        };
        // Shift is still down when the gate breaks; its physical key-up passes through later.
        state.pending_held_replay.extend([
            edge(WM_KEYDOWN, VK_LSHIFT, 0x2a),
            edge(WM_KEYDOWN, VIRTUAL_KEY(0x47), 0x22),
            edge(WM_KEYUP, VIRTUAL_KEY(0x47), 0x22),
        ]);
        state
            .pending_key_releases
            .push(edge(WM_KEYUP, VK_LCONTROL, 0x1d));
        let held = take_held_keys_return(&mut state).unwrap();
        assert_eq!(
            scan_edges(&held.inputs),
            [
                (0x2a, false),
                (0x22, false),
                (0x22, true),
                (0x1d, true),
                (0x2a, true)
            ]
        );
        assert_eq!(held.handed_back, None);
        assert!(state.pending_held_replay.is_empty());
        assert!(state.pending_key_releases.is_empty());
        assert!(take_held_keys_return(&mut state).is_none());
    }

    #[test]
    fn keys_handed_back_past_a_stalled_worker_disarm_recovery_and_report_to_it() {
        let (state, receiver) = test_gate_state(8);
        let metrics = Arc::clone(&state.metrics);
        let now = Instant::now();
        let retained_at = now - Duration::from_millis(STALLED_WORKER_HAND_BACK_MS);
        *metrics.retained_input.lock().unwrap() = Some(stalled_record(retained_at));
        metrics.worker_busy.store(true, Ordering::Release);
        metrics.recovery_armed.store(true, Ordering::Release);
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        hand_back_stalled_retained_input(Some(test_foreground()), true, now);
        let mut state = take_app_state().unwrap();
        assert!(metrics.retained_input.lock().unwrap().is_none());
        // Pause no longer has anything to recover.
        assert!(!metrics.recovery_armed.load(Ordering::Acquire));
        let held = take_held_keys_return(&mut state).unwrap();
        assert_eq!(held.handed_back, Some(10));
        assert_eq!(held.inputs.len(), 2, "the held G is released as well");
        finish_held_keys_return(&state, &held, 1);
        let reports: Vec<_> = receiver
            .try_iter()
            .filter_map(|queued| match queued.event {
                RawInputEvent::RetainedInputHandedBack {
                    token,
                    submitted,
                    requested,
                } => Some((token, submitted, requested)),
                _ => None,
            })
            .collect();
        assert_eq!(reports, [(10, 1, 2)]);
        // Keys a broken gate returns by itself are no recovery: nothing to report.
        state
            .pending_key_releases
            .push(test_raw_key(WM_KEYUP, VK_LSHIFT, 0, 0));
        let held = take_held_keys_return(&mut state).unwrap();
        finish_held_keys_return(&state, &held, 1);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn the_worker_finishes_the_recovery_of_keys_the_ui_thread_handed_back() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        let pause = || {
            metrics.auto_enabled.store(false, Ordering::Release);
            metrics.safety_paused.store(true, Ordering::Release);
            metrics
                .last_failure_reason
                .store(ConversionFailureReason::GateDrain as u8, Ordering::Release);
        };
        let handed_back = |processor: &mut InputProcessor, submitted| {
            let queued_epoch = metrics.input_epoch.load(Ordering::Acquire);
            // The gate abort and the stale recovery the worker found nothing for moved it on.
            metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
            processor.process_event(QueuedInputEvent {
                epoch: queued_epoch,
                captured_at: Instant::now(),
                event: RawInputEvent::RetainedInputHandedBack {
                    token: 10,
                    submitted,
                    requested: 2,
                },
                configuration: None,
            });
        };
        // Only some of the keys went out: the session is unknown, the pause stays.
        pause();
        handed_back(&mut processor, 1);
        assert!(processor.session.is_suppressed());
        assert!(processor.recovered_mid_word);
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
        // All of them: policy D1 resumes automatic conversion.
        processor.session.clear();
        handed_back(&mut processor, 2);
        assert!(processor.session.is_suppressed());
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
        assert!(!metrics.safety_paused.load(Ordering::Acquire));
    }

    #[test]
    fn the_timer_hands_back_keys_a_stalled_worker_holds_but_not_from_inside_an_edit() {
        let (state, _receiver) = test_gate_state(8);
        let metrics = Arc::clone(&state.metrics);
        let now = Instant::now();
        let retained_at = now - Duration::from_millis(STALLED_WORKER_HAND_BACK_MS);
        *metrics.retained_input.lock().unwrap() = Some(stalled_record(retained_at));
        metrics.worker_busy.store(true, Ordering::Release);
        metrics.edit_in_flight.store(true, Ordering::Release);
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        // Inside an edit the field may show the word selected: the keys stay retained.
        hand_back_stalled_retained_input(Some(test_foreground()), true, now);
        assert!(metrics.retained_input.lock().unwrap().is_some());
        metrics.edit_in_flight.store(false, Ordering::Release);
        // A click since the keys were retained: they would land after it.
        APP_STATE.with(|slot| {
            let mut borrowed = slot.borrow_mut();
            observe_mouse_button(borrowed.as_mut().unwrap(), false);
        });
        hand_back_stalled_retained_input(Some(test_foreground()), true, now);
        assert!(metrics.retained_input.lock().unwrap().is_some());
        APP_STATE.with(|slot| {
            let mut borrowed = slot.borrow_mut();
            borrowed.as_mut().unwrap().typed_input = 0;
        });
        hand_back_stalled_retained_input(Some(test_foreground()), true, now);
        let state = take_app_state().unwrap();
        assert!(metrics.retained_input.lock().unwrap().is_none());
        assert_eq!(state.pending_held_replay.len(), 1);
    }

    #[test]
    #[ignore = "uses the real clipboard: run on the test VM with --include-ignored"]
    fn an_unconfirmed_paste_gives_the_real_clipboard_back_only_after_its_target_read_it() {
        let _com = PrivacyGuard::new();
        let source = clipboard_test_window(w!("AutoKeyboardLayot deferred source"));
        put_test_clipboard_text(source, "akl-user-clipboard");
        unsafe {
            DestroyWindow(source).expect("destroy source window");
        }
        let window = clipboard_test_window(w!("AutoKeyboardLayot deferred probe"));
        let own_process = unsafe { GetCurrentProcessId() };
        let wait_for = |expected: &str| {
            let deadline = Instant::now() + Duration::from_secs(3);
            while test_clipboard_unicode(window).as_deref() != Some(expected)
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(10));
            }
            test_clipboard_unicode(window)
        };

        // This process is the paste's target: reading the replacement is the late paste, and
        // the user's clipboard comes back right after it.
        let brokers = ClipboardBrokers::default();
        let mut transaction = ProtectedClipboard::begin(
            "akl-deferred-probe",
            Duration::from_millis(CLIPBOARD_BROKER_TIMEOUT_MS),
            &brokers,
            own_process,
        )
        .expect("protected clipboard transaction");
        transaction.mark_submitted();
        transaction.defer();
        drop(transaction);
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some("akl-deferred-probe")
        );
        assert_eq!(
            wait_for("akl-user-clipboard").as_deref(),
            Some("akl-user-clipboard")
        );
        assert_eq!(reap_all(&brokers), [BrokerEnd::Restored]);

        // Another process is the target: this one reading the replacement proves nothing, and
        // the replacement stays once the wait is over.
        let mut transaction = ProtectedClipboard::begin_with(
            "akl-deferred-probe",
            Duration::from_millis(CLIPBOARD_BROKER_TIMEOUT_MS),
            &brokers,
            (4, Duration::from_millis(300)),
            SystemClipboard::new,
            Box::new(system_clipboard_state),
        )
        .expect("protected clipboard transaction");
        transaction.mark_submitted();
        transaction.defer();
        drop(transaction);
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some("akl-deferred-probe")
        );
        assert_eq!(reap_all(&brokers), [BrokerEnd::Kept]);
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some("akl-deferred-probe")
        );
        unsafe {
            DestroyWindow(window).expect("destroy probe window");
        }
    }

    /// A scripted clipboard for the broker: its text, sequence and owner, and the failures and
    /// delays to inject.
    #[derive(Default)]
    struct FakeClipboardState {
        text: String,
        sequence: u32,
        owner: usize,
        empties: u32,
        restores: u32,
        snapshot_delay: Duration,
        restore_delay: Duration,
        /// Another program puts this on the clipboard while the copy is taken.
        writer_during_snapshot: Option<String>,
        open_fails: bool,
        install_fails_after_empty: bool,
        /// Processes that read the replacement.
        readers: Vec<u32>,
    }

    const FAKE_BROKER_OWNER: usize = 7;
    const FAKE_TARGET: u32 = 1234;

    struct FakeClipboard(Arc<Mutex<FakeClipboardState>>);

    impl ClipboardSystem for FakeClipboard {
        type Snapshot = String;

        fn sequence(&self) -> u32 {
            self.0.lock().unwrap().sequence
        }

        fn owner(&self) -> usize {
            FAKE_BROKER_OWNER
        }

        fn owned(&self) -> bool {
            self.0.lock().unwrap().owner == FAKE_BROKER_OWNER
        }

        fn snapshot(&mut self) -> Option<String> {
            let delay = self.0.lock().unwrap().snapshot_delay;
            thread::sleep(delay);
            let mut state = self.0.lock().unwrap();
            let copy = state.text.clone();
            if let Some(newer) = state.writer_during_snapshot.take() {
                state.text = newer;
                state.sequence += 1;
                state.owner = 99;
            }
            Some(copy)
        }

        fn install(&mut self, text: &str, expected: u32, cancelled: &AtomicBool) -> InstallOutcome {
            let mut state = self.0.lock().unwrap();
            if state.open_fails || cancelled.load(Ordering::Acquire) || state.sequence != expected {
                return InstallOutcome::Untouched;
            }
            state.empties += 1;
            state.text.clear();
            state.owner = FAKE_BROKER_OWNER;
            state.sequence += 1;
            if state.install_fails_after_empty {
                return InstallOutcome::Emptied {
                    sequence: state.sequence,
                };
            }
            state.text = text.to_owned();
            state.sequence += 1;
            InstallOutcome::Installed {
                sequence: state.sequence,
            }
        }

        fn restore(&mut self, expected: Option<u32>, snapshot: &mut String) -> bool {
            let delay = self.0.lock().unwrap().restore_delay;
            thread::sleep(delay);
            let mut state = self.0.lock().unwrap();
            if expected.is_some_and(|sequence| {
                state.sequence != sequence || state.owner != FAKE_BROKER_OWNER
            }) {
                return false;
            }
            state.restores += 1;
            state.text = snapshot.clone();
            state.sequence += 1;
            true
        }

        fn wait(
            &mut self,
            commands: Option<&Receiver<ClipboardBrokerCommand>>,
            timeout: Option<Duration>,
        ) -> BrokerWait {
            match (commands, timeout) {
                (Some(commands), Some(timeout)) => match commands.recv_timeout(timeout) {
                    Ok(command) => BrokerWait::Command(command),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => BrokerWait::Timeout,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        BrokerWait::Disconnected
                    }
                },
                (Some(commands), None) => commands
                    .recv()
                    .map_or(BrokerWait::Disconnected, BrokerWait::Command),
                (None, timeout) => {
                    thread::sleep(timeout.unwrap_or(Duration::from_millis(10)));
                    BrokerWait::Timeout
                }
            }
        }

        fn read_by(&self, process: u32) -> bool {
            self.0.lock().unwrap().readers.contains(&process)
        }
    }

    fn fake_clipboard(text: &str) -> Arc<Mutex<FakeClipboardState>> {
        Arc::new(Mutex::new(FakeClipboardState {
            text: text.to_owned(),
            sequence: 1,
            owner: 99,
            ..FakeClipboardState::default()
        }))
    }

    /// Begins a protected clipboard on `state` for a paste to `FAKE_TARGET` that, unconfirmed,
    /// waits `deferred_wait` for its target; the worker's checks read the same state.
    fn begin_fake_with<'a>(
        state: &Arc<Mutex<FakeClipboardState>>,
        brokers: &'a ClipboardBrokers,
        install_timeout: Duration,
        deferred_wait: Duration,
    ) -> core::result::Result<ProtectedClipboard<'a>, ClipboardBeginFailure> {
        begin_fake_for(
            state,
            brokers,
            "привет",
            install_timeout,
            (FAKE_TARGET, deferred_wait),
        )
    }

    /// Begins a protected clipboard on `state` that puts `text` there for a paste to `target.0`
    /// which, unconfirmed, waits `target.1` for it; the worker's checks read the same state.
    fn begin_fake_for<'a>(
        state: &Arc<Mutex<FakeClipboardState>>,
        brokers: &'a ClipboardBrokers,
        text: &str,
        install_timeout: Duration,
        target: (u32, Duration),
    ) -> core::result::Result<ProtectedClipboard<'a>, ClipboardBeginFailure> {
        let broker_state = Arc::clone(state);
        let probe_state = Arc::clone(state);
        ProtectedClipboard::begin_with(
            text,
            install_timeout,
            brokers,
            target,
            move || Some(FakeClipboard(broker_state)),
            Box::new(move || {
                let state = probe_state.lock().unwrap();
                (state.sequence, state.owner)
            }),
        )
    }

    fn begin_fake<'a>(
        state: &Arc<Mutex<FakeClipboardState>>,
        brokers: &'a ClipboardBrokers,
        install_timeout: Duration,
    ) -> core::result::Result<ProtectedClipboard<'a>, ClipboardBeginFailure> {
        begin_fake_with(state, brokers, install_timeout, Duration::from_secs(2))
    }

    /// Waits until every broker the worker stopped waiting for is over; says how they ended.
    fn reap_all(brokers: &ClipboardBrokers) -> Vec<BrokerEnd> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while brokers.busy() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        brokers.reap()
    }

    #[test]
    fn a_confirmed_paste_puts_the_users_clipboard_back() {
        let state = fake_clipboard("user text");
        let brokers = ClipboardBrokers::default();
        let mut clipboard = begin_fake(&state, &brokers, Duration::from_secs(2)).unwrap();
        assert!(clipboard.is_current());
        assert_eq!(state.lock().unwrap().text, "привет");
        clipboard.mark_submitted();
        assert!(clipboard.restore());
        drop(clipboard);
        assert_eq!(state.lock().unwrap().text, "user text");
        assert_eq!(state.lock().unwrap().restores, 1);
        assert!(brokers.reap().is_empty(), "the broker was waited for");
    }

    #[test]
    fn a_paste_sent_and_not_confirmed_gives_the_clipboard_back_only_after_its_target_read_it() {
        let state = fake_clipboard("password");
        let brokers = ClipboardBrokers::default();
        let mut clipboard = begin_fake(&state, &brokers, Duration::from_secs(2)).unwrap();
        clipboard.mark_submitted();
        clipboard.defer();
        drop(clipboard);
        // Another program reading it proves nothing: the replacement stays for the late paste.
        state.lock().unwrap().readers.push(99);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(state.lock().unwrap().text, "привет");
        assert!(brokers.busy());
        // The target reads it. That may be its clipboard listener, with the queued Ctrl+V right
        // after: the replacement stays a moment longer.
        state.lock().unwrap().readers.push(FAKE_TARGET);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(state.lock().unwrap().text, "привет");
        assert!(brokers.busy());
        // Then the password comes back.
        assert_eq!(reap_all(&brokers), [BrokerEnd::Restored]);
        assert_eq!(state.lock().unwrap().text, "password");
    }

    #[test]
    fn a_restore_that_finds_the_replacement_replaced_ends_its_broker_at_once() {
        let state = fake_clipboard("password");
        let brokers = ClipboardBrokers::default();
        let mut clipboard = begin_fake(&state, &brokers, Duration::from_secs(2)).unwrap();
        {
            let mut state = state.lock().unwrap();
            state.text = "theirs".to_owned();
            state.sequence += 1;
            state.owner = 99;
        }
        assert!(!clipboard.restore());
        // The broker is over before the next paste could start, and theirs stays.
        assert_eq!(clipboard.shared.end(), BrokerEnd::Kept);
        drop(clipboard);
        assert!(brokers.reap().is_empty());
        assert_eq!(state.lock().unwrap().text, "theirs");
        assert_eq!(state.lock().unwrap().restores, 0);
    }

    #[test]
    fn a_replacement_its_target_never_read_stays_on_the_clipboard() {
        // Dropped after the Ctrl+V (an early return), the transaction defers all the same.
        let state = fake_clipboard("password");
        let brokers = ClipboardBrokers::default();
        let mut clipboard = begin_fake_with(
            &state,
            &brokers,
            Duration::from_secs(2),
            Duration::from_millis(100),
        )
        .unwrap();
        clipboard.mark_submitted();
        drop(clipboard);
        assert_eq!(reap_all(&brokers), [BrokerEnd::Kept]);
        assert_eq!(state.lock().unwrap().text, "привет");
        assert_eq!(state.lock().unwrap().restores, 0);
        // Another program that replaced the replacement keeps its content.
        let state = fake_clipboard("password");
        let mut clipboard = begin_fake(&state, &brokers, Duration::from_secs(2)).unwrap();
        clipboard.mark_submitted();
        clipboard.defer();
        drop(clipboard);
        {
            let mut state = state.lock().unwrap();
            state.text = "theirs".to_owned();
            state.sequence += 1;
            state.owner = 99;
        }
        assert_eq!(reap_all(&brokers), [BrokerEnd::NotRestored]);
        assert_eq!(state.lock().unwrap().text, "theirs");
        // Before the Ctrl+V, dropping it puts the user's clipboard back at once.
        let state = fake_clipboard("user text");
        drop(begin_fake(&state, &brokers, Duration::from_secs(2)).unwrap());
        assert_eq!(state.lock().unwrap().text, "user text");
        assert!(brokers.reap().is_empty());
    }

    #[test]
    fn the_users_clipboard_comes_back_only_after_the_field_showed_the_paste() {
        assert_eq!(
            clipboard_after_paste(ReplayAttempt::Applied),
            ClipboardAfterPaste::Restore
        );
        for unconfirmed in [
            ReplayAttempt::Cancelled,
            ReplayAttempt::Failed,
            ReplayAttempt::LayoutSwitchFailed,
        ] {
            assert_eq!(
                clipboard_after_paste(unconfirmed),
                ClipboardAfterPaste::Defer,
                "{unconfirmed:?}"
            );
        }
    }

    #[test]
    fn a_clipboard_changed_while_it_is_copied_is_left_alone() {
        let state = fake_clipboard("old");
        state.lock().unwrap().writer_during_snapshot = Some("newer".to_owned());
        let brokers = ClipboardBrokers::default();
        assert!(matches!(
            begin_fake(&state, &brokers, Duration::from_secs(2)),
            Err(ClipboardBeginFailure::NotInstalled)
        ));
        assert_eq!(reap_all(&brokers), [BrokerEnd::Untouched]);
        let state = state.lock().unwrap();
        assert_eq!(state.text, "newer");
        assert_eq!(state.empties, 0);
        assert_eq!(state.restores, 0);
    }

    #[test]
    fn a_failed_installation_is_undone_only_when_it_emptied_the_clipboard() {
        let state = fake_clipboard("user text");
        state.lock().unwrap().install_fails_after_empty = true;
        let brokers = ClipboardBrokers::default();
        assert!(begin_fake(&state, &brokers, Duration::from_secs(2)).is_err());
        assert_eq!(reap_all(&brokers), [BrokerEnd::Restored]);
        assert_eq!(state.lock().unwrap().text, "user text");
        assert_eq!(state.lock().unwrap().restores, 1);
        // A clipboard that could not even be opened was never changed: nothing is put back.
        let state = fake_clipboard("user text");
        state.lock().unwrap().open_fails = true;
        assert!(begin_fake(&state, &brokers, Duration::from_secs(2)).is_err());
        assert_eq!(reap_all(&brokers), [BrokerEnd::Untouched]);
        assert_eq!(state.lock().unwrap().empties, 0);
        assert_eq!(state.lock().unwrap().restores, 0);
    }

    #[test]
    fn a_broker_the_worker_stopped_waiting_for_never_installs_and_holds_back_the_next_paste() {
        let state = fake_clipboard("user text");
        state.lock().unwrap().snapshot_delay = Duration::from_millis(300);
        let brokers = ClipboardBrokers::default();
        assert!(matches!(
            begin_fake(&state, &brokers, Duration::from_millis(20)),
            Err(ClipboardBeginFailure::NotInstalled)
        ));
        // One broker at a time: the next paste does not start while that one still runs.
        let other = fake_clipboard("other");
        assert!(matches!(
            begin_fake(&other, &brokers, Duration::from_secs(2)),
            Err(ClipboardBeginFailure::Busy)
        ));
        assert_eq!(reap_all(&brokers), [BrokerEnd::Untouched]);
        assert_eq!(state.lock().unwrap().text, "user text");
        assert_eq!(state.lock().unwrap().empties, 0);
        assert!(begin_fake(&other, &brokers, Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn a_restore_the_worker_stopped_waiting_for_is_logged_when_it_ends() {
        let (processor, receiver) = processor_with_diagnostics();
        let state = fake_clipboard("user text");
        state.lock().unwrap().restore_delay =
            Duration::from_millis(CLIPBOARD_RESTORE_WAIT_MS + 200);
        let mut clipboard =
            begin_fake(&state, &processor.clipboard_brokers, Duration::from_secs(2)).unwrap();
        clipboard.mark_submitted();
        assert!(!clipboard.restore(), "not waited for");
        drop(clipboard);
        let deadline = Instant::now() + Duration::from_secs(5);
        while processor.clipboard_brokers.busy() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        processor.log_finished_clipboard_brokers();
        let lines: Vec<String> = receiver
            .try_iter()
            .filter(|record| record.contains("phase=broker_end"))
            .collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("result=Restored"), "{lines:?}");
        assert_eq!(state.lock().unwrap().text, "user text");
    }

    #[test]
    fn a_restored_item_gets_the_opt_outs_it_lacked_and_keeps_its_own() {
        let opt_outs = [101, 102, 103];
        assert_eq!(missing_opt_out_formats(&[13, 1], opt_outs), [101, 102, 103]);
        assert_eq!(missing_opt_out_formats(&[13, 102], opt_outs), [101, 103]);
        assert!(missing_opt_out_formats(&[101, 102, 103, 13], opt_outs).is_empty());
    }

    #[test]
    fn a_paste_fits_its_waits_into_the_time_the_gate_has_left() {
        let now = Instant::now();
        let at = |ms: u64| PasteBudget {
            deadline: Some(now + Duration::from_millis(ms)),
        };
        assert_eq!(PASTE_AFTER_SUBMIT_MS, 750);
        // Without a gate nothing is cut short.
        let unbounded = PasteBudget { deadline: None };
        assert!(unbounded.allows_selection(now, PastePatience::Manual));
        assert_eq!(
            unbounded.install_timeout(now, PastePatience::Manual),
            Some(Duration::from_millis(CLIPBOARD_BROKER_TIMEOUT_MS))
        );
        assert!(unbounded.allows_submit(now));
        // A gate armed a settle ago: an automatic paste keeps its installation limit, Pause gets
        // what is left after everything that follows the Ctrl+V.
        let fresh = at(CORRECTION_GATE_MAX_HOLD_MS - NOTEPAD_TEXT_BARRIER_TIMEOUT_MS);
        assert!(fresh.allows_selection(now, PastePatience::Automatic));
        assert!(fresh.allows_selection(now, PastePatience::Manual));
        assert_eq!(
            fresh.install_timeout(now, PastePatience::Automatic),
            Some(Duration::from_millis(
                CLIPBOARD_AUTOMATIC_INSTALL_TIMEOUT_MS
            ))
        );
        assert_eq!(
            fresh.install_timeout(now, PastePatience::Manual),
            Some(Duration::from_millis(1_050 - PASTE_AFTER_SUBMIT_MS))
        );
        // The selection starts only with its confirmation, the least installation and
        // everything after the Ctrl+V in hand.
        let automatic =
            UIA_SELECTION_CONFIRM_TIMEOUT_MS + PASTE_MIN_INSTALL_MS + PASTE_AFTER_SUBMIT_MS;
        assert!(at(automatic).allows_selection(now, PastePatience::Automatic));
        assert!(!at(automatic - 1).allows_selection(now, PastePatience::Automatic));
        let manual = 2 * UIA_MANUAL_SELECTION_CONFIRM_TIMEOUT_MS
            + PASTE_MIN_INSTALL_MS
            + PASTE_AFTER_SUBMIT_MS;
        assert!(at(manual).allows_selection(now, PastePatience::Manual));
        assert!(!at(manual - 1).allows_selection(now, PastePatience::Manual));
        // Less than the least installation is no installation.
        let least = PASTE_AFTER_SUBMIT_MS + PASTE_MIN_INSTALL_MS;
        assert_eq!(
            at(least).install_timeout(now, PastePatience::Automatic),
            Some(Duration::from_millis(PASTE_MIN_INSTALL_MS))
        );
        assert_eq!(
            at(least - 1).install_timeout(now, PastePatience::Automatic),
            None
        );
        // The Ctrl+V needs everything after it in hand.
        assert!(at(PASTE_AFTER_SUBMIT_MS).allows_submit(now));
        assert!(!at(PASTE_AFTER_SUBMIT_MS - 1).allows_submit(now));
    }

    /// A gate window that arms a gate, prepares a handoff and accepts a drain whenever asked.
    unsafe extern "system" fn accepting_gate_procedure(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            GATE_ARM_MESSAGE | GATE_HANDOFF_BUDGET_MESSAGE | GATE_DRAIN_ACK_MESSAGE => LRESULT(1),
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    /// A window of the calling thread that accepts every gate request, so that it answers the
    /// worker's messages within the call. It goes with the thread.
    fn accepting_gate_window() -> HWND {
        let module = unsafe { GetModuleHandleW(None).expect("module") };
        let class = WNDCLASSW {
            lpfnWndProc: Some(accepting_gate_procedure),
            hInstance: HINSTANCE(module.0),
            lpszClassName: w!("AutoKeyboardLayotAcceptingGateTest"),
            ..Default::default()
        };
        // A second registration fails harmlessly: the class is the same.
        unsafe { RegisterClassW(&class) };
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("AutoKeyboardLayotAcceptingGateTest"),
                w!("AutoKeyboardLayot gate test"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
            .expect("gate window")
        }
    }

    #[test]
    fn a_gate_the_worker_arms_or_hands_over_gives_the_edit_the_time_the_gate_has() {
        let window = accepting_gate_window();
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        processor.gate_window = window.0 as usize;
        let lets_go_after = |before: Instant, ms: u64, processor: &InputProcessor| {
            let hold = Duration::from_millis(ms);
            processor.edit_deadline.get().is_some_and(|deadline| {
                deadline >= before + hold && deadline <= Instant::now() + hold
            })
        };

        // Armed by the worker (a Pause conversion, a word whose keys it waited for): the keys go
        // after the gate's longest hold.
        let before = Instant::now();
        assert!(processor.arm_correction_gate(9));
        assert!(lets_go_after(
            before,
            CORRECTION_GATE_MAX_HOLD_MS,
            &processor
        ));

        // Handed over to the next word: the gate keeps the keys for what the word's route needs.
        let detection = Detection {
            source_language: Language::English,
            target_language: Language::Russian,
            original: "z".into(),
            replacement: "я".into(),
            source_score: 0.0,
            target_score: 1.0,
        };
        let transaction = ConversionTransaction::without_delimiter(&detection).unwrap();
        let replay_steps = transaction.forward.erase_characters + 1;
        for (backend, required_ms) in [
            (TextEditBackend::ProtectedPaste, paste_gate_budget_ms(false)),
            (TextEditBackend::CapabilityProbe, paste_gate_budget_ms(true)),
            (
                TextEditBackend::PhysicalReplay,
                conversion_gate_budget_ms(replay_steps, false, false).unwrap(),
            ),
        ] {
            processor.edit_deadline.set(None);
            processor.text_edit_backend = backend;
            // Automatic conversion is off: the conversion stops right after the handoff.
            processor.pending_conversion = Some(PendingConversion {
                transaction: transaction.clone(),
                foreground: test_foreground(),
                source_layout: test_foreground().layout,
                profile_generation: processor.input_profiles.generation(),
                space_down_sequence: 7,
                replay_keys: Vec::new(),
                forced: false,
                cycle_order: Vec::new(),
            });
            processor.deferred_drained_conversion = Some(DeferredDrainedConversion {
                gate_token: 6,
                boundary_sequence: 7,
            });
            let before = Instant::now();
            processor.process_gate_drain_ready(6, 0, true);
            assert!(
                lets_go_after(before, required_ms, &processor),
                "{backend:?}"
            );
        }
        // The release the conversion posted is not handled here.
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, Some(window), 0, 0, PM_REMOVE) }.as_bool() {}
        unsafe {
            DestroyWindow(window).expect("destroy gate window");
        }
    }

    /// A configuration folder holding the default document.
    fn configuration_folder() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.ini"),
            ConfigurationDocument::default().to_text().unwrap(),
        )
        .unwrap();
        directory
    }

    fn dictionary_offer(word: &str) -> VolatileLexiconCandidate {
        VolatileLexiconCandidate::new(
            LexiconOfferTarget::UserDictionary,
            Language::English,
            word.to_owned(),
            Instant::now(),
        )
    }

    fn saved_document(directory: &Path) -> ConfigurationDocument {
        autokeyboardlayot::configuration::load_configuration_directory(directory).unwrap()
    }

    #[test]
    fn a_draft_older_than_the_configuration_is_refused_and_a_current_one_is_saved() {
        let folder = configuration_folder();
        let base = ConfigurationDocument::default();
        let mut first = base.clone();
        first.user_dictionary.push("en-US: firefox".to_owned());
        save_configuration_if_unchanged(folder.path(), &first, &base).unwrap();
        let mut stale = base.clone();
        stale.process_exclusions.push("private.exe".to_owned());
        assert_eq!(
            save_configuration_if_unchanged(folder.path(), &stale, &base)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::AlreadyExists
        );
        let saved = saved_document(folder.path());
        assert_eq!(saved.user_dictionary, ["en-US: firefox"]);
        assert!(saved.process_exclusions.is_empty());
    }

    #[test]
    fn writers_saving_at_the_same_time_each_keep_their_entry() {
        let folder = configuration_folder();
        let words = ["alpha", "beta", "gamma", "delta"];
        let start = Arc::new(std::sync::Barrier::new(words.len()));
        let writers: Vec<_> = words
            .into_iter()
            .map(|word| {
                let directory = folder.path().to_owned();
                let start = Arc::clone(&start);
                thread::spawn(move || {
                    start.wait();
                    loop {
                        match persist_lexicon_candidate(&directory, &dictionary_offer(word)) {
                            // Another writer holds the lock: a user would try again.
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                            outcome => break outcome,
                        }
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
        let saved = saved_document(folder.path()).user_dictionary;
        for word in words {
            assert!(
                saved.contains(&format!("en-US: {word}")),
                "{word}: {saved:?}"
            );
        }
    }

    #[test]
    fn a_tray_word_is_saved_off_the_calling_thread_while_another_writer_holds_the_lock() {
        let folder = configuration_folder();
        let held = ConfigurationWriteGuard::acquire().unwrap();
        let (sender, outcome) = sync_channel(1);
        let started = Instant::now();
        save_lexicon_candidate_in_background(
            folder.path().to_owned(),
            dictionary_offer("firefox"),
            move |candidate, result| {
                let _ = sender.send((candidate.word, result));
            },
        )
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "the caller waited for the lock"
        );
        thread::sleep(Duration::from_millis(100));
        assert!(outcome.try_recv().is_err(), "saved past the held lock");
        drop(held);
        let (word, result) = outcome.recv_timeout(Duration::from_secs(5)).unwrap();
        result.unwrap();
        assert_eq!(word, "firefox");
        assert_eq!(
            saved_document(folder.path()).user_dictionary,
            ["en-US: firefox"]
        );
    }

    #[test]
    fn a_configuration_load_that_panics_fails_that_read_and_the_loader_keeps_serving() {
        let loads = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&loads);
        let mut loader = RuntimeConfigurationLoader::spawn_with(move || {
            assert!(
                counter.fetch_add(1, Ordering::AcqRel) > 0,
                "a row the parser did not expect"
            );
            Ok(RuntimeConfiguration::default())
        })
        .unwrap();
        let answer = |loader: &mut RuntimeConfigurationLoader| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(result) = loader.poll() {
                    return result;
                }
                assert!(Instant::now() < deadline, "the loader did not answer");
                thread::sleep(Duration::from_millis(5));
            }
        };
        assert!(loader.request());
        assert_eq!(
            answer(&mut loader).unwrap_err().to_string(),
            "configuration_loader_panicked"
        );
        assert!(loader.request());
        assert!(answer(&mut loader).is_ok());
        assert_eq!(loads.load(Ordering::Acquire), 2);
    }

    #[test]
    fn an_armed_space_gives_its_conversion_the_deadline_of_its_gate() {
        let metrics = Arc::new(ObserverMetrics::default());
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        let queued = armed_space(&metrics, 7, 0);
        let captured_at = queued.captured_at;
        processor.process(queued);
        assert_eq!(
            processor.edit_deadline.get(),
            Some(captured_at + Duration::from_millis(CORRECTION_GATE_MAX_HOLD_MS))
        );
        // An event that armed no gate carries no deadline.
        metrics.active_gate_token.store(0, Ordering::Release);
        processor.process(QueuedInputEvent {
            epoch: metrics.input_epoch.load(Ordering::Acquire),
            captured_at: Instant::now(),
            event: RawInputEvent::Key(test_raw_key(WM_KEYUP, VK_SPACE, 8, 0)),
            configuration: None,
        });
        assert_eq!(processor.edit_deadline.get(), None);
    }

    #[test]
    fn nothing_typed_is_no_failure_and_each_failure_after_the_paste_names_its_reason() {
        let metrics = Arc::new(ObserverMetrics::default());
        metrics.auto_enabled.store(true, Ordering::Release);
        let mut processor = InputProcessor::new(Arc::clone(&metrics), 0);
        for (attempt, status) in [
            (TextEditAttempt::Unsupported, BACKEND_SELECTION_UNSUPPORTED),
            (TextEditAttempt::Cancelled, BACKEND_SELECTION_MISMATCH),
        ] {
            for route in [PasteRoute::Backend, PasteRoute::CapabilityProbe] {
                metrics
                    .backend_status
                    .store(BACKEND_CAPABILITY_PROBE, Ordering::Release);
                assert!(
                    processor
                        .apply_paste_attempt_result(attempt, 5, route)
                        .is_none()
                );
                assert!(
                    metrics.auto_enabled.load(Ordering::Acquire),
                    "{attempt:?} {route:?}"
                );
                let expected = if route == PasteRoute::CapabilityProbe {
                    status
                } else {
                    BACKEND_CAPABILITY_PROBE
                };
                assert_eq!(metrics.backend_status.load(Ordering::Acquire), expected);
            }
        }
        assert_eq!(
            processor.apply_paste_attempt_result(
                TextEditAttempt::Applied,
                6,
                PasteRoute::CapabilityProbe
            ),
            Some(EditStrategy::ProtectedPaste)
        );
        assert_eq!(
            metrics.backend_status.load(Ordering::Acquire),
            BACKEND_PROTECTED_PASTE
        );
        for reason in [
            ConversionFailureReason::TextCommit,
            ConversionFailureReason::LayoutSwitch,
            ConversionFailureReason::ClipboardEdit,
        ] {
            metrics.auto_enabled.store(true, Ordering::Release);
            assert!(
                processor
                    .apply_paste_attempt_result(
                        TextEditAttempt::Failed(reason),
                        7,
                        PasteRoute::Backend
                    )
                    .is_none()
            );
            assert!(!metrics.auto_enabled.load(Ordering::Acquire));
            assert_eq!(
                metrics.last_failure_reason.load(Ordering::Acquire),
                reason as u8
            );
        }
        assert_eq!(
            conversion_failure_label(ConversionFailureReason::LayoutSwitch as u8),
            "layout-switch"
        );
    }

    #[test]
    fn a_replay_that_lost_its_guard_after_sending_is_logged_as_applied_uncertain() {
        let (mut processor, receiver) = processor_with_diagnostics();
        processor
            .metrics
            .auto_enabled
            .store(true, Ordering::Release);
        type_letters(&mut processor, "hel");
        assert!(!processor.finish_replay_commit(ReplayAttempt::Cancelled, 9, 0x0419, "conversion"));
        assert_eq!(processor.session.buffered_character_count(), 0);
        assert_eq!(
            processor.layout_switch_in_flight.map(|(layout, _)| layout),
            Some(0x0419)
        );
        // Every key was sent: this is no failed edit.
        assert!(processor.metrics.auto_enabled.load(Ordering::Acquire));
        assert!(!processor.finish_replay_commit(ReplayAttempt::Cancelled, 10, 0x0409, "undo"));
        let lines: Vec<String> = receiver
            .try_iter()
            .filter(|record| record.contains("result=applied-uncertain"))
            .collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("event=conversion ") && lines[0].contains("sequence=9"));
        assert!(lines[1].starts_with("event=undo ") && lines[1].contains("sequence=10"));
        assert!(processor.finish_replay_commit(ReplayAttempt::Applied, 11, 0x0409, "conversion"));
        assert!(!processor.finish_replay_commit(ReplayAttempt::Failed, 12, 0x0409, "conversion"));
        assert_eq!(
            processor
                .metrics
                .last_failure_reason
                .load(Ordering::Acquire),
            ConversionFailureReason::ReplayCommit as u8
        );
    }

    #[test]
    fn a_paste_abandoned_after_the_user_acted_leaves_the_selection_alone() {
        let (mut processor, receiver) = processor_in_field(EDITOR, None);
        let foreground = test_foreground();
        processor.privacy_guard = PrivacyGuard {
            automation: None,
            com_initialized: false,
            last_barrier_report: Cell::new(TextBarrierReport::default()),
        };
        // The input is still the edit's: the selection is collapsed (here it cannot be shown
        // gone, without UI Automation).
        processor
            .metrics
            .observed_input_sequence
            .store(5, Ordering::Release);
        assert_eq!(
            processor.abandon_selection(foreground, 5, TextEditAttempt::Cancelled),
            TextEditAttempt::SelectionLeft
        );
        assert_eq!(
            processor.privacy_guard.last_barrier_report().cleanup,
            Some(SelectionCleanup::Unverified)
        );
        // The user typed or clicked since: what is selected may be theirs and is not touched.
        processor
            .privacy_guard
            .last_barrier_report
            .set(TextBarrierReport::default());
        processor
            .metrics
            .observed_input_sequence
            .store(6, Ordering::Release);
        assert_eq!(
            processor.abandon_selection(foreground, 5, TextEditAttempt::Cancelled),
            TextEditAttempt::SelectionLeft
        );
        assert_eq!(processor.privacy_guard.last_barrier_report().cleanup, None);
        let cleanups: Vec<String> = receiver
            .try_iter()
            .filter(|record| record.contains("phase=cleanup"))
            .collect();
        assert_eq!(cleanups.len(), 2, "{cleanups:?}");
        assert!(cleanups[0].contains("result=Unverified"), "{cleanups:?}");
        assert!(cleanups[1].contains("result=skipped"), "{cleanups:?}");
    }

    #[test]
    fn keys_whose_paste_may_have_left_the_word_selected_come_back_only_when_asked() {
        let (mut processor, receiver) = processor_in_field(EDITOR, None);
        *processor.metrics.retained_input.lock().unwrap() = Some(RetainedInput {
            token: 42,
            ..stalled_record(Instant::now())
        });
        processor
            .metrics
            .observed_input_sequence
            .store(50, Ordering::Release);
        assert!(
            processor
                .apply_paste_attempt_result(TextEditAttempt::SelectionLeft, 42, PasteRoute::Backend)
                .is_none()
        );
        assert_eq!(
            processor
                .metrics
                .selection_left_token
                .load(Ordering::Acquire),
            42
        );
        processor.recover_retained_input(50, false);
        assert!(processor.metrics.retained_input.lock().unwrap().is_some());
        assert!(
            receiver
                .try_iter()
                .any(|record| record.contains("result=retained reason=selection-left"))
        );
    }

    const PASTE_SEQUENCE: u64 = 5;

    /// What the outside steps of a scripted paste saw: each step in order (`submit:N` for a
    /// SendInput call of N keys), the installation's time limit, and the clipboard's text when
    /// the Ctrl+V went out.
    #[derive(Default)]
    struct PasteSteps {
        steps: Vec<String>,
        install_timeout: Option<Duration>,
        clipboard_at_submit: Option<String>,
    }

    /// The clipboard of a scripted paste: a scripted one whose unconfirmed paste waits the given
    /// time for its target, or the real one (test VM only).
    enum PasteClipboard {
        Fake(Arc<Mutex<FakeClipboardState>>, Duration),
        System,
    }

    /// Something that happens right after a step of a scripted paste: the user clicks, the step
    /// takes long, another program writes the clipboard, the field reads it.
    type PasteEvent = Box<dyn FnOnce() + Send>;

    /// The outside steps of a protected paste as a test scripts them: the selection's finding,
    /// the clipboard, how many keys of a chord SendInput takes (all of them when `None`), the
    /// field's answers to the commit waits (shown once they run out), the layout switch and the
    /// collapse.
    struct ScriptedPaste {
        steps: Arc<Mutex<PasteSteps>>,
        settled: TextBarrierStatus,
        selection: (TextBarrierStatus, Option<SelectionCleanup>),
        clipboard: PasteClipboard,
        taken: Option<usize>,
        commits: VecDeque<ReplayAttempt>,
        layout: ReplayAttempt,
        collapse: SelectionCleanup,
        events: Vec<(&'static str, PasteEvent)>,
    }

    impl ScriptedPaste {
        /// A paste that goes through: the word is selected and the field shows each change.
        fn new(clipboard: PasteClipboard) -> (Self, Arc<Mutex<PasteSteps>>) {
            let steps = Arc::new(Mutex::new(PasteSteps::default()));
            let script = Self {
                steps: Arc::clone(&steps),
                settled: TextBarrierStatus::Match,
                selection: (TextBarrierStatus::Match, None),
                clipboard,
                taken: None,
                commits: VecDeque::new(),
                layout: ReplayAttempt::Applied,
                collapse: SelectionCleanup::Collapsed,
                events: Vec::new(),
            };
            (script, steps)
        }

        fn after(mut self, step: &'static str, event: impl FnOnce() + Send + 'static) -> Self {
            self.events.push((step, Box::new(event)));
            self
        }

        /// Notes `step` and lets what follows it happen.
        fn step(&mut self, step: &str) {
            self.steps.lock().unwrap().steps.push(step.to_owned());
            while let Some(index) = self.events.iter().position(|(name, _)| *name == step) {
                let (_, event) = self.events.remove(index);
                event();
            }
        }
    }

    impl PasteSeam for ScriptedPaste {
        fn settle(&mut self) -> TextBarrierStatus {
            self.step("settle");
            self.settled
        }

        fn select(&mut self) -> (TextBarrierStatus, Option<SelectionCleanup>) {
            self.step("select");
            self.selection
        }

        fn begin<'a>(
            &mut self,
            text: &str,
            install_timeout: Duration,
            brokers: &'a ClipboardBrokers,
            target_process: u32,
        ) -> core::result::Result<ProtectedClipboard<'a>, ClipboardBeginFailure> {
            self.steps.lock().unwrap().install_timeout = Some(install_timeout);
            let clipboard = match &self.clipboard {
                PasteClipboard::Fake(state, deferred_wait) => begin_fake_for(
                    state,
                    brokers,
                    text,
                    install_timeout,
                    (target_process, *deferred_wait),
                ),
                PasteClipboard::System => {
                    ProtectedClipboard::begin(text, install_timeout, brokers, target_process)
                }
            };
            self.step("begin");
            clipboard
        }

        fn submit(&mut self, requested: usize) -> usize {
            // Reading the real clipboard here would count as the field's paste.
            if let PasteClipboard::Fake(state, _) = &self.clipboard {
                let text = state.lock().unwrap().text.clone();
                self.steps
                    .lock()
                    .unwrap()
                    .clipboard_at_submit
                    .get_or_insert(text);
            }
            self.step(&format!("submit:{requested}"));
            self.taken.take().unwrap_or(requested)
        }

        fn commit(&mut self) -> ReplayAttempt {
            self.step("commit");
            self.commits.pop_front().unwrap_or(ReplayAttempt::Applied)
        }

        fn switch_layout(&mut self) -> ReplayAttempt {
            self.step("layout");
            self.layout
        }

        fn collapse(&mut self) -> SelectionCleanup {
            self.step("collapse");
            self.collapse
        }
    }

    /// A scripted paste on a scripted clipboard holding `password`; unconfirmed, it waits two
    /// seconds for its target.
    fn scripted_paste() -> (
        ScriptedPaste,
        Arc<Mutex<PasteSteps>>,
        Arc<Mutex<FakeClipboardState>>,
    ) {
        let state = fake_clipboard("password");
        let (script, steps) = ScriptedPaste::new(PasteClipboard::Fake(
            Arc::clone(&state),
            Duration::from_secs(2),
        ));
        (script, steps, state)
    }

    /// A processor whose edit of input `PASTE_SEQUENCE` still holds its gate in the field of
    /// `foreground`, with the outside steps of its protected paste scripted by `script`.
    fn paste_processor(
        foreground: ForegroundContext,
        script: impl FnOnce(&Arc<ObserverMetrics>) -> ScriptedPaste,
    ) -> InputProcessor {
        let (mut processor, _receiver) = processor_in_field(EDITOR, None);
        processor.foreground_override = Some(foreground);
        let metrics = &processor.metrics;
        metrics
            .active_gate_token
            .store(PASTE_SEQUENCE, Ordering::Release);
        metrics
            .forwarded_input_sequence
            .store(PASTE_SEQUENCE, Ordering::Release);
        let script = script(metrics);
        processor.paste_seam = Some(RefCell::new(Box::new(script)));
        processor
    }

    /// A click: the edit's input is no longer current.
    fn user_clicks(metrics: &Arc<ObserverMetrics>) -> impl FnOnce() + Send + 'static {
        let metrics = Arc::clone(metrics);
        move || {
            metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Converts `ghbdtn` to `привет` by a protected paste in the processor's field.
    fn run_paste(processor: &InputProcessor, patience: PastePatience) -> TextEditAttempt {
        processor.perform_protected_paste(
            processor.foreground_override.unwrap(),
            PASTE_SEQUENCE,
            0x0409,
            HKL(0x0419 as *mut c_void),
            "ghbdtn",
            "привет",
            patience,
        )
    }

    #[test]
    fn a_paste_gives_the_users_clipboard_back_only_once_the_field_showed_the_replacement() {
        let target = test_foreground().process_id;
        // Shown in the field: the user's clipboard comes back before the paste ends.
        let (script, steps, state) = scripted_paste();
        let processor = paste_processor(test_foreground(), |_| script);
        assert_eq!(
            run_paste(&processor, PastePatience::Automatic),
            TextEditAttempt::Applied
        );
        assert_eq!(
            steps.lock().unwrap().steps,
            ["select", "begin", "submit:4", "commit", "layout", "commit"]
        );
        assert_eq!(
            steps.lock().unwrap().clipboard_at_submit.as_deref(),
            Some("привет")
        );
        assert_eq!(state.lock().unwrap().text, "password");
        assert!(processor.clipboard_brokers.reap().is_empty());

        // A busy field that showed nothing in time, and a chord sent only in part (Ctrl is let go):
        // either may still paste, so the replacement stays until the field reads it.
        for (taken, commits, reason, expected) in [
            (
                None,
                vec![ReplayAttempt::Failed],
                ConversionFailureReason::TextCommit,
                &["select", "begin", "submit:4", "commit", "collapse"][..],
            ),
            (
                Some(2),
                Vec::new(),
                ConversionFailureReason::ClipboardEdit,
                &["select", "begin", "submit:4", "submit:1", "collapse"][..],
            ),
        ] {
            let (mut script, steps, state) = scripted_paste();
            script.taken = taken;
            script.commits = commits.into();
            let processor = paste_processor(test_foreground(), |_| script);
            assert_eq!(
                run_paste(&processor, PastePatience::Automatic),
                TextEditAttempt::Failed(reason)
            );
            assert_eq!(steps.lock().unwrap().steps, expected, "{reason:?}");
            // Another program reading it proves nothing.
            state.lock().unwrap().readers.push(99);
            thread::sleep(Duration::from_millis(30));
            assert_eq!(state.lock().unwrap().text, "привет", "{reason:?}");
            assert!(processor.clipboard_brokers.busy(), "{reason:?}");
            // The field handles its Ctrl+V late and gets the replacement; then the password is
            // back.
            state.lock().unwrap().readers.push(target);
            assert_eq!(
                reap_all(&processor.clipboard_brokers),
                [BrokerEnd::Restored]
            );
            assert_eq!(state.lock().unwrap().text, "password");
        }
    }

    /// A processor in an editor's field on the protected-paste backend whose keyboard, settle
    /// step and paste are scripted, so that Pause runs its whole pipeline in a test: the field
    /// shows `layout`, and automatic conversion is off.
    #[cfg(feature = "legacy-bundled-input")]
    fn pause_processor(
        layout: usize,
    ) -> (
        InputProcessor,
        Receiver<String>,
        ForegroundContext,
        Arc<Mutex<PasteSteps>>,
    ) {
        let (mut processor, diagnostics) = processor_in_field(EDITOR, None);
        let foreground = ForegroundContext {
            layout,
            ..test_foreground()
        };
        processor.gate_window = accepting_gate_window().0 as usize;
        processor.foreground_override = Some(foreground);
        processor.last_foreground = Some(foreground_identity_key(foreground));
        processor.last_layout = Some(layout);
        processor.test_keyboard = Some(test_keyboard);
        processor.backend_rules =
            BackendRules::from_lines(["editor.exe = protected-paste"]).unwrap();
        processor.settings.conversion_sound = false;
        processor.settings.offer_dictionary_after_forced_conversion = false;
        let (script, steps, _clipboard) = scripted_paste();
        processor.paste_seam = Some(RefCell::new(Box::new(script)));
        (processor, diagnostics, foreground, steps)
    }

    /// Types the keys with these scan codes and a Space in the processor's field.
    #[cfg(feature = "legacy-bundled-input")]
    fn type_word_and_space(processor: &mut InputProcessor, scan_codes: &[u32]) {
        let foreground = processor.foreground_override.expect("the field");
        type_keys(processor, foreground, scan_codes);
        processor.process_key(space_key(foreground, scan_codes.len() as u64 + 1, 0));
    }

    /// Presses Pause in the processor's field and returns the diagnostics written meanwhile. A
    /// press that converts leaves the field in the target layout, as Windows would.
    #[cfg(feature = "legacy-bundled-input")]
    fn press_pause(
        processor: &mut InputProcessor,
        diagnostics: &Receiver<String>,
        sequence: u64,
    ) -> Vec<String> {
        diagnostics.try_iter().for_each(drop);
        let foreground = processor.foreground_override.expect("the field");
        processor
            .metrics
            .active_gate_token
            .store(sequence, Ordering::Release);
        let event = RawKeyEvent {
            foreground,
            hotkey_trigger: true,
            ..test_raw_key(WM_KEYDOWN, VK_PAUSE, sequence, 0)
        };
        let language = processor.language_for_layout(foreground.layout);
        processor.execute_forced_conversion(event, language);
        if let Some(cycle) = &processor.transpose_cycle {
            let layout = processor
                .find_layout(cycle.language)
                .expect("the target layout")
                .0 as usize;
            processor.foreground_override = Some(ForegroundContext {
                layout,
                ..foreground
            });
            processor.last_layout = Some(layout);
        }
        diagnostics.try_iter().collect()
    }

    /// The language and text of the word after each of three Pause presses.
    #[cfg(feature = "legacy-bundled-input")]
    fn three_pause_presses(
        processor: &mut InputProcessor,
        diagnostics: &Receiver<String>,
    ) -> Vec<(Language, String)> {
        (0..3)
            .map(|press| {
                press_pause(processor, diagnostics, 41 + press);
                let cycle = processor
                    .transpose_cycle
                    .as_ref()
                    .unwrap_or_else(|| panic!("press {} converts", press + 1));
                (cycle.language, cycle.text.clone())
            })
            .collect()
    }

    const GHBDTN: [u32; 6] = [0x22, 0x23, 0x30, 0x20, 0x14, 0x31];

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn pause_converts_the_last_word_after_a_manual_layout_switch() {
        let (mut processor, diagnostics, foreground, steps) = pause_processor(0x0409);
        type_word_and_space(&mut processor, &GHBDTN);
        // Alt+Shift: the field now shows the Russian layout; the word was typed in English.
        processor.foreground_override = Some(ForegroundContext {
            layout: 0x0419,
            ..foreground
        });
        let lines = press_pause(&mut processor, &diagnostics, 41);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("event=source_settle") && line.contains("result=Applied")),
            "{lines:?}"
        );
        let cycle = processor.transpose_cycle.as_ref().expect("converted");
        assert_eq!(cycle.language, Language::Russian);
        assert_eq!(cycle.text, "привет");
        assert_eq!(steps.lock().unwrap().steps[0], "settle");
        // A manual switch between two presses: the next press starts from the layout the field
        // shows (Estonian), not from the one the last press left.
        processor.foreground_override = Some(ForegroundContext {
            layout: 0x0425,
            ..foreground
        });
        let lines = press_pause(&mut processor, &diagnostics, 42);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("event=source_settle") && line.contains("result=Applied")),
            "{lines:?}"
        );
        let cycle = processor.transpose_cycle.as_ref().expect("converted again");
        assert_ne!(cycle.language, Language::Russian, "{lines:?}");
        assert_eq!(cycle.text, "ghbdtn");
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn pause_presses_pass_through_the_original_layout() {
        // English, Russian and Estonian: Estonian reads Latin letters as English does, so the
        // presses go back to English rather than to a lookalike layout.
        let (mut processor, diagnostics, _, _) = pause_processor(0x0409);
        type_word_and_space(&mut processor, &GHBDTN);
        assert_eq!(
            three_pause_presses(&mut processor, &diagnostics),
            [
                (Language::Russian, "привет".to_owned()),
                (Language::English, "ghbdtn".to_owned()),
                (Language::Russian, "привет".to_owned()),
            ]
        );
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn pause_presses_follow_the_stored_order_through_every_enabled_layout() {
        // "k[simus" reads differently in all three layouts; the first press orders them by how
        // plausible each reading is (the Estonian "küsimus" is a dictionary word), and the next
        // presses keep that order.
        let (mut processor, diagnostics, _, _) = pause_processor(0x0409);
        type_word_and_space(&mut processor, &[0x25, 0x1a, 0x1f, 0x17, 0x32, 0x16, 0x1f]);
        let mut orders = Vec::new();
        let mut words = Vec::new();
        for press in 0..3 {
            let lines = press_pause(&mut processor, &diagnostics, 41 + press);
            orders.extend(lines.iter().filter_map(|line| {
                line.split(' ')
                    .find_map(|field| field.strip_prefix("cycle="))
                    .map(str::to_owned)
            }));
            let cycle = processor.transpose_cycle.as_ref().expect("converted");
            words.push((cycle.language, cycle.text.clone()));
        }
        assert_eq!(
            words,
            [
                (Language::Estonian, "küsimus".to_owned()),
                (Language::Russian, "лхышьгы".to_owned()),
                (Language::English, "k[simus".to_owned()),
            ]
        );
        assert_eq!(orders, ["en-US,et-EE,ru-RU"; 3]);
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn a_remembered_word_outlives_a_new_input_epoch_but_not_new_keyboard_profiles() {
        for profiles_changed in [false, true] {
            let (mut processor, diagnostics, field, _) = pause_processor(0x0409);
            type_word_and_space(&mut processor, &GHBDTN);
            // Another window gets the keys; the field's last word is remembered.
            let elsewhere = ForegroundContext {
                hwnd: 8,
                focus: 8,
                ..field
            };
            processor.foreground_override = Some(elsewhere);
            type_keys(&mut processor, elsewhere, &[0x1e]);
            assert_eq!(processor.remembered_words.len(), 1);
            if profiles_changed && let Some(word) = processor.remembered_words[0].word.as_mut() {
                word.profile_generation += 1;
            }
            // A settings Apply starts a new input epoch.
            let epoch = processor.metrics.input_epoch.fetch_add(1, Ordering::AcqRel) + 1;
            processor.process(QueuedInputEvent {
                epoch,
                captured_at: Instant::now(),
                event: RawInputEvent::Mouse,
                configuration: None,
            });
            processor.foreground_override = Some(field);
            let lines = press_pause(&mut processor, &diagnostics, 41);
            assert!(!processor.session.is_suppressed(), "{lines:?}");
            if profiles_changed {
                assert!(processor.transpose_cycle.is_none(), "{lines:?}");
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("source=remembered result=stale")),
                    "{lines:?}"
                );
            } else {
                let cycle = processor.transpose_cycle.as_ref().expect("converted");
                assert_eq!(cycle.text, "привет", "{lines:?}");
            }
        }
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn a_candidate_that_is_not_converted_says_why() {
        // (automatic conversion on, the key that ends "ghbdtn", the reason logged)
        let space = |sequence| (VK_SPACE, 0x39, 0, false, sequence);
        for (automatic, (key, scan_code, held, caps_lock, sequence), reason) in [
            (false, space(7), Some("automatic-off")),
            (
                true,
                (VK_SPACE, 0x39, held_keys(&[VK_LSHIFT]), false, 8),
                Some("shift"),
            ),
            (true, (VK_SPACE, 0x39, 0, true, 7), Some("caps-lock")),
            (true, (VK_RETURN, 0x1c, 0, false, 7), Some("boundary")),
            (true, space(7), None),
        ] {
            let (mut processor, diagnostics, foreground, _) = pause_processor(0x0409);
            processor
                .metrics
                .auto_enabled
                .store(automatic, Ordering::Release);
            type_keys(&mut processor, foreground, &GHBDTN);
            if held != 0 {
                processor.process_key(RawKeyEvent {
                    scan_code: 0x2a,
                    held_modifiers: held,
                    foreground,
                    ..test_raw_key(WM_KEYDOWN, VK_LSHIFT, 7, 0)
                });
            }
            diagnostics.try_iter().for_each(drop);
            processor.process_key(RawKeyEvent {
                scan_code,
                held_modifiers: held,
                caps_lock,
                foreground,
                ..test_raw_key(WM_KEYDOWN, key, sequence, 0)
            });
            let words: Vec<String> = diagnostics
                .try_iter()
                .filter(|line| line.starts_with("event=word "))
                .collect();
            match reason {
                Some(reason) => {
                    assert_eq!(words.len(), 1, "{words:?}");
                    assert!(
                        words[0].ends_with(&format!("result=not-attempted reason={reason}")),
                        "{words:?}"
                    );
                }
                None => assert!(words.is_empty(), "{words:?}"),
            }
        }
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn a_conversion_planned_for_another_space_is_rejected_although_every_later_guard_passes() {
        // (sequence the edit runs for, converts): the Space down or the key after it qualifies.
        for (boundary, converts) in [(41, true), (42, true), (99, false)] {
            let (mut processor, _diagnostics, foreground, steps) = pause_processor(0x0409);
            processor.refresh_process_policy(foreground.process_id);
            let detection = Detection {
                source_language: Language::English,
                target_language: Language::Russian,
                original: "ghbdtn".into(),
                replacement: "привет".into(),
                source_score: 0.0,
                target_score: 1.0,
            };
            let replay_keys = GHBDTN
                .map(|scan_code| ReplayKey {
                    scan_code: scan_code as u16,
                    shift: false,
                    caps_lock: false,
                    extended: false,
                })
                .to_vec();
            processor.pending_conversion = Some(PendingConversion {
                transaction: ConversionTransaction::new(&detection, ' ').unwrap(),
                foreground,
                source_layout: foreground.layout,
                profile_generation: processor.input_profiles.generation(),
                space_down_sequence: 41,
                replay_keys,
                forced: true,
                cycle_order: vec![Language::English, Language::Russian, Language::Estonian],
            });
            processor
                .metrics
                .active_gate_token
                .store(boundary, Ordering::Release);
            processor
                .metrics
                .forwarded_input_sequence
                .store(boundary, Ordering::Release);
            processor.execute_pending_conversion_inner(boundary);
            assert!(processor.pending_conversion.is_none(), "{boundary}");
            assert_eq!(processor.transpose_cycle.is_some(), converts, "{boundary}");
            assert_eq!(
                steps.lock().unwrap().steps.is_empty(),
                !converts,
                "{boundary}"
            );
        }
    }

    #[test]
    fn a_paste_that_panics_after_the_ctrl_v_leaves_the_replacement_for_its_field() {
        let (script, _steps, state) = scripted_paste();
        let script = script.after("submit:4", || panic!("the worker fails after the chord"));
        let processor = paste_processor(test_foreground(), |_| script);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_paste(&processor, PastePatience::Automatic)
        }));
        assert!(unwound.is_err());
        assert_eq!(state.lock().unwrap().text, "привет");
        state
            .lock()
            .unwrap()
            .readers
            .push(test_foreground().process_id);
        assert_eq!(
            reap_all(&processor.clipboard_brokers),
            [BrokerEnd::Restored]
        );
        assert_eq!(state.lock().unwrap().text, "password");
    }

    #[test]
    fn a_confirmed_paste_whose_clipboard_another_program_took_meanwhile_says_so() {
        let (script, _steps, state) = scripted_paste();
        let writer = Arc::clone(&state);
        let script = script.after("commit", move || {
            let mut state = writer.lock().unwrap();
            state.text = "theirs".to_owned();
            state.sequence += 1;
            state.owner = 99;
        });
        let processor = paste_processor(test_foreground(), |_| script);
        assert_eq!(
            run_paste(&processor, PastePatience::Automatic),
            TextEditAttempt::AppliedClipboardNotRestored
        );
        assert_eq!(state.lock().unwrap().text, "theirs");
        assert_eq!(state.lock().unwrap().restores, 0);
    }

    #[test]
    fn a_layout_that_does_not_switch_after_the_paste_fails_with_its_own_reason() {
        let (mut script, steps, state) = scripted_paste();
        script.layout = ReplayAttempt::LayoutSwitchFailed;
        let processor = paste_processor(test_foreground(), |_| script);
        assert_eq!(
            run_paste(&processor, PastePatience::Automatic),
            TextEditAttempt::Failed(ConversionFailureReason::LayoutSwitch)
        );
        assert_eq!(
            steps.lock().unwrap().steps,
            ["select", "begin", "submit:4", "commit", "layout"]
        );
        // The replacement was shown, so the user's clipboard was already back.
        assert_eq!(state.lock().unwrap().text, "password");
    }

    #[test]
    fn a_paste_that_types_nothing_ends_by_what_it_found() {
        use SelectionCleanup::{Collapsed, Unverified};
        for (selection, open_fails, outcome, expected) in [
            (
                (TextBarrierStatus::Unavailable, None),
                false,
                TextEditAttempt::Unsupported,
                &["select"][..],
            ),
            (
                (TextBarrierStatus::Mismatch, Some(Collapsed)),
                false,
                TextEditAttempt::Cancelled,
                &["select"][..],
            ),
            (
                (TextBarrierStatus::Mismatch, Some(Unverified)),
                false,
                TextEditAttempt::SelectionLeft,
                &["select"][..],
            ),
            // The clipboard did not take the replacement: a busy clipboard is a word left alone.
            (
                (TextBarrierStatus::Match, None),
                true,
                TextEditAttempt::Cancelled,
                &["select", "begin", "collapse"][..],
            ),
        ] {
            let (mut script, steps, state) = scripted_paste();
            script.selection = selection;
            state.lock().unwrap().open_fails = open_fails;
            let processor = paste_processor(test_foreground(), |_| script);
            assert_eq!(
                run_paste(&processor, PastePatience::Automatic),
                outcome,
                "{selection:?}"
            );
            assert_eq!(steps.lock().unwrap().steps, expected, "{selection:?}");
            assert_eq!(state.lock().unwrap().text, "password");
            assert_eq!(state.lock().unwrap().empties, 0);
        }
    }

    #[test]
    fn a_paste_stops_before_each_step_the_gate_has_no_time_left_for() {
        let paste = |budget_ms: u64, slow_step: Option<&'static str>| {
            let (mut script, steps, state) = scripted_paste();
            if let Some(step) = slow_step {
                script = script.after(step, || thread::sleep(Duration::from_millis(300)));
            }
            let processor = paste_processor(test_foreground(), |_| script);
            processor
                .edit_deadline
                .set(Some(Instant::now() + Duration::from_millis(budget_ms)));
            let attempt = run_paste(&processor, PastePatience::Automatic);
            let steps = std::mem::take(&mut *steps.lock().unwrap());
            (attempt, steps, state)
        };
        // No time left: not even the selection starts.
        let (attempt, steps, _) = paste(0, None);
        assert_eq!(attempt, TextEditAttempt::Cancelled);
        assert!(steps.steps.is_empty(), "{:?}", steps.steps);
        // Time for the selection and a shortened installation (the gate's 900 ms less everything
        // after the Ctrl+V leave 150 ms of the usual 200).
        let (attempt, steps, _) = paste(900, None);
        assert_eq!(attempt, TextEditAttempt::Applied);
        let install = steps.install_timeout.unwrap();
        assert!(
            install >= Duration::from_millis(PASTE_MIN_INSTALL_MS)
                && install < Duration::from_millis(CLIPBOARD_AUTOMATIC_INSTALL_TIMEOUT_MS),
            "{install:?}"
        );
        // A slow selection leaves no time for the installation: the clipboard is not touched.
        let (attempt, steps, state) = paste(1_000, Some("select"));
        assert_eq!(attempt, TextEditAttempt::Cancelled);
        assert_eq!(steps.steps, ["select", "collapse"]);
        assert_eq!(state.lock().unwrap().empties, 0);
        // A slow installation leaves no time for what follows the Ctrl+V: it is not sent, and the
        // user's clipboard comes back at once.
        let (attempt, steps, state) = paste(1_000, Some("begin"));
        assert_eq!(attempt, TextEditAttempt::Cancelled);
        assert_eq!(steps.steps, ["select", "begin", "collapse"]);
        assert_eq!(state.lock().unwrap().text, "password");
        assert_eq!(state.lock().unwrap().restores, 1);
    }

    #[test]
    fn a_paste_the_user_interrupted_leaves_the_selection_alone() {
        // A click before the Ctrl+V: nothing was sent, so the user's clipboard comes back at once;
        // what is selected may be the user's by now and is not touched.
        let (script, steps, state) = scripted_paste();
        let processor = paste_processor(test_foreground(), |metrics| {
            script.after("begin", user_clicks(metrics))
        });
        assert_eq!(
            run_paste(&processor, PastePatience::Automatic),
            TextEditAttempt::SelectionLeft
        );
        assert_eq!(steps.lock().unwrap().steps, ["select", "begin"]);
        assert_eq!(state.lock().unwrap().text, "password");
        assert_eq!(state.lock().unwrap().restores, 1);

        // A click while the field had not shown the paste: the replacement waits for the field,
        // and the selection is left alone as well.
        let (mut script, steps, state) = scripted_paste();
        script.commits = VecDeque::from([ReplayAttempt::Cancelled]);
        let processor = paste_processor(test_foreground(), |metrics| {
            script.after("commit", user_clicks(metrics))
        });
        assert_eq!(
            run_paste(&processor, PastePatience::Automatic),
            TextEditAttempt::SelectionLeft
        );
        assert_eq!(
            steps.lock().unwrap().steps,
            ["select", "begin", "submit:4", "commit"]
        );
        assert_eq!(state.lock().unwrap().text, "привет");

        // Without a click, a collapse the field does not show ends the same way.
        let (mut script, steps, _state) = scripted_paste();
        script.commits = VecDeque::from([ReplayAttempt::Failed]);
        script.collapse = SelectionCleanup::Unverified;
        let processor = paste_processor(test_foreground(), |_| script);
        assert_eq!(
            run_paste(&processor, PastePatience::Automatic),
            TextEditAttempt::SelectionLeft
        );
        assert_eq!(
            steps.lock().unwrap().steps,
            ["select", "begin", "submit:4", "commit", "collapse"]
        );
    }

    #[test]
    #[ignore = "uses the real clipboard: run on the test VM with --include-ignored"]
    fn a_paste_on_the_real_clipboard_gives_a_late_field_the_replacement_and_the_user_their_clipboard()
     {
        let _com = PrivacyGuard::new();
        let source = clipboard_test_window(w!("AutoKeyboardLayot paste source"));
        put_test_clipboard_text(source, "akl-user-clipboard");
        unsafe {
            DestroyWindow(source).expect("destroy source window");
        }
        let window = clipboard_test_window(w!("AutoKeyboardLayot paste probe"));
        // This process plays the field the paste goes to.
        let foreground = ForegroundContext {
            process_id: unsafe { GetCurrentProcessId() },
            ..test_foreground()
        };

        // A field that pastes at once and shows it: the user's clipboard is back when the paste
        // ends.
        let pasted = Arc::new(Mutex::new(None));
        let field = Arc::clone(&pasted);
        let reader = window.0 as usize;
        let (script, steps) = ScriptedPaste::new(PasteClipboard::System);
        let script = script.after("commit", move || {
            *field.lock().unwrap() = test_clipboard_unicode(HWND(reader as *mut c_void));
        });
        let processor = paste_processor(foreground, |_| script);
        assert_eq!(
            run_paste(&processor, PastePatience::Manual),
            TextEditAttempt::Applied
        );
        assert_eq!(
            steps.lock().unwrap().steps,
            ["select", "begin", "submit:4", "commit", "layout", "commit"]
        );
        assert_eq!(pasted.lock().unwrap().as_deref(), Some("привет"));
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some("akl-user-clipboard")
        );
        assert!(processor.clipboard_brokers.reap().is_empty());

        // A busy field that handles its Ctrl+V only after the paste stopped waiting: it gets the
        // replacement, never the user's clipboard, which comes back right after.
        let (mut script, steps) = ScriptedPaste::new(PasteClipboard::System);
        script.commits = VecDeque::from([ReplayAttempt::Failed]);
        let processor = paste_processor(foreground, |_| script);
        assert_eq!(
            run_paste(&processor, PastePatience::Manual),
            TextEditAttempt::Failed(ConversionFailureReason::TextCommit)
        );
        assert_eq!(
            steps.lock().unwrap().steps,
            ["select", "begin", "submit:4", "commit", "collapse"]
        );
        assert!(processor.clipboard_brokers.busy());
        assert_eq!(test_clipboard_unicode(window).as_deref(), Some("привет"));
        // That read may be the field's own look at the clipboard: a paste right after it still
        // gets the replacement.
        thread::sleep(Duration::from_millis(100));
        assert_eq!(test_clipboard_unicode(window).as_deref(), Some("привет"));
        let deadline = Instant::now() + Duration::from_secs(3);
        while test_clipboard_unicode(window).as_deref() != Some("akl-user-clipboard")
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some("akl-user-clipboard")
        );
        assert_eq!(
            reap_all(&processor.clipboard_brokers),
            [BrokerEnd::Restored]
        );
        unsafe {
            DestroyWindow(window).expect("destroy probe window");
        }
    }

    /// The field of the desktop paste acceptance: a WinForms rich edit in a process of its own,
    /// which Windows PowerShell compiles on the spot. It listens to the clipboard; the first time
    /// the agent's broker puts something there, it stays busy for a while, as a program that
    /// handles its paste late does. `WM_APP + 0x40` sets for the next paste how long it is busy
    /// (wparam, in milliseconds) and, when lparam is not zero, that it then reads the new
    /// clipboard itself (a clipboard listener) and is busy lparam milliseconds more.
    const DESKTOP_FIELD_SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
Add-Type -WarningAction SilentlyContinue -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;
public class AklBusyField : Form {
    [DllImport("user32.dll")] static extern bool AddClipboardFormatListener(IntPtr window);
    [DllImport("user32.dll")] static extern IntPtr GetClipboardOwner();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder name, int size);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, IntPtr process);
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint attach, uint to, bool on);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    readonly RichTextBox box = new RichTextBox();
    int busyMs;
    int readThenBusyMs;
    public AklBusyField() {
        Text = "AutoKeyboardLayot busy field";
        StartPosition = FormStartPosition.Manual;
        SetBounds(80, 80, 640, 140);
        box.Multiline = false;
        box.Dock = DockStyle.Fill;
        box.Font = new System.Drawing.Font("Segoe UI", 16f);
        Controls.Add(box);
        Shown += delegate { TakeForeground(); };
    }
    void TakeForeground() {
        uint other = GetWindowThreadProcessId(GetForegroundWindow(), IntPtr.Zero);
        uint own = GetCurrentThreadId();
        bool attached = other != 0 && other != own && AttachThreadInput(own, other, true);
        SetForegroundWindow(Handle);
        Activate();
        box.Focus();
        if (attached) { AttachThreadInput(own, other, false); }
    }
    protected override void OnHandleCreated(EventArgs e) {
        base.OnHandleCreated(e);
        AddClipboardFormatListener(Handle);
    }
    protected override void WndProc(ref Message m) {
        if (m.Msg == 0x8040) {
            busyMs = m.WParam.ToInt32();
            readThenBusyMs = m.LParam.ToInt32();
            m.Result = new IntPtr(1);
            return;
        }
        if (m.Msg == 0x031D) {
            StringBuilder owner = new StringBuilder(128);
            GetClassName(GetClipboardOwner(), owner, owner.Capacity);
            if (owner.ToString() == "AutoKeyboardLayotClipboardBroker") {
                int busy = busyMs;
                int after = readThenBusyMs;
                busyMs = 0;
                readThenBusyMs = 0;
                if (busy > 0) { Thread.Sleep(busy); }
                if (after > 0) {
                    try { Clipboard.GetText(); } catch (Exception) { }
                    Thread.Sleep(after);
                }
            }
        }
        base.WndProc(ref m);
    }
}
'@
[System.Windows.Forms.Application]::EnableVisualStyles()
[System.Windows.Forms.Application]::Run((New-Object AklBusyField))
"#;

    /// The busy field of the desktop acceptance, in front on the interactive session's desktop.
    /// It is ended however the test ends.
    struct DesktopField {
        child: std::process::Child,
        script: PathBuf,
        context: ForegroundContext,
    }

    impl DesktopField {
        const CONFIGURE: u32 = 0x8040;

        /// Starts the field and brings it to the front. Started by this process it may not take
        /// the foreground by itself, and Start's search window can keep it after a first sign-in:
        /// the search is dismissed and the field is clicked, as the user would.
        fn start() -> Self {
            use windows::Win32::Foundation::RECT;
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
                MOUSEEVENTF_MOVE, MOUSEINPUT,
            };
            use windows::Win32::UI::WindowsAndMessaging::{
                AllowSetForegroundWindow, FindWindowW, GetSystemMetrics, GetWindowRect,
                SM_CXSCREEN, SM_CYSCREEN, SetForegroundWindow,
            };
            let escape = [
                keyboard_input(VK_ESCAPE, 0, Default::default()),
                keyboard_input(VK_ESCAPE, 0, KEYEVENTF_KEYUP),
            ];
            assert!(send_input_batch(&escape));
            thread::sleep(Duration::from_millis(500));
            let script =
                std::env::temp_dir().join(format!("akl-busy-field-{}.ps1", std::process::id()));
            fs::write(&script, DESKTOP_FIELD_SCRIPT).expect("field script");
            let child = {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                std::process::Command::new("powershell.exe")
                    .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                    .arg(&script)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                    .expect("start the field")
            };
            let mut field = Self {
                child,
                script,
                context: test_foreground(),
            };
            unsafe {
                let _ = AllowSetForegroundWindow(field.child.id());
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Some(context) = current_foreground_context()
                    && context.process_id == field.child.id()
                    && context.focus != 0
                {
                    field.context = context;
                    return field;
                }
                if let Ok(window) = unsafe { FindWindowW(None, w!("AutoKeyboardLayot busy field")) }
                {
                    unsafe {
                        let _ = SetForegroundWindow(window);
                    }
                    let mut rect = RECT::default();
                    if unsafe { GetWindowRect(window, &mut rect) }.is_ok() {
                        let width = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(2);
                        let height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(2);
                        let mouse = |flags| INPUT {
                            r#type: INPUT_MOUSE,
                            Anonymous: INPUT_0 {
                                mi: MOUSEINPUT {
                                    dx: (rect.left + rect.right) / 2 * 65_535 / (width - 1),
                                    dy: (rect.top + rect.bottom) / 2 * 65_535 / (height - 1),
                                    dwFlags: flags | MOUSEEVENTF_ABSOLUTE,
                                    ..Default::default()
                                },
                            },
                        };
                        let click = [
                            mouse(MOUSEEVENTF_MOVE),
                            mouse(MOUSEEVENTF_LEFTDOWN),
                            mouse(MOUSEEVENTF_LEFTUP),
                        ];
                        assert!(send_input_batch(&click));
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "the field did not take the foreground: {:?}",
                    current_foreground_context()
                );
                thread::sleep(Duration::from_millis(100));
            }
        }

        fn send(window: usize, message: u32, wparam: usize, lparam: isize) -> usize {
            let mut result = 0usize;
            unsafe {
                let _ = SendMessageTimeoutW(
                    HWND(window as *mut c_void),
                    message,
                    WPARAM(wparam),
                    LPARAM(lparam),
                    SMTO_BLOCK,
                    5_000,
                    Some(&mut result),
                );
            }
            result
        }

        fn text(&self) -> String {
            use windows::Win32::UI::WindowsAndMessaging::WM_GETTEXT;
            let mut buffer = [0u16; 256];
            let copied = Self::send(
                self.context.focus,
                WM_GETTEXT,
                buffer.len(),
                buffer.as_mut_ptr() as isize,
            );
            String::from_utf16_lossy(&buffer[..copied.min(buffer.len())])
        }

        /// Puts `text` in the field with the caret at its end.
        fn set_text(&self, text: &str) {
            use windows::Win32::UI::WindowsAndMessaging::WM_SETTEXT;
            const EM_SETSEL: u32 = 0x00B1;
            let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
            Self::send(self.context.focus, WM_SETTEXT, 0, wide.as_ptr() as isize);
            let end = wide.len() - 1;
            Self::send(self.context.focus, EM_SETSEL, end, end as isize);
        }

        /// How the field behaves when the next paste's replacement arrives (`DESKTOP_FIELD_SCRIPT`).
        fn configure(&self, busy_ms: usize, read_then_busy_ms: isize) {
            Self::send(
                self.context.hwnd,
                Self::CONFIGURE,
                busy_ms,
                read_then_busy_ms,
            );
        }

        /// The field's text once it changed from `before` and stopped changing (a busy field
        /// pastes late), or after five seconds.
        fn settled_text(&self, before: &str) -> String {
            let mut text = self.text();
            let mut changed_at = Instant::now();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline
                && (text == before || changed_at.elapsed() < Duration::from_millis(700))
            {
                thread::sleep(Duration::from_millis(50));
                let now = self.text();
                if now != text {
                    text = now;
                    changed_at = Instant::now();
                }
            }
            text
        }
    }

    impl Drop for DesktopField {
        fn drop(&mut self) {
            use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.context.hwnd as *mut c_void)),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            thread::sleep(Duration::from_millis(500));
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_file(&self.script);
        }
    }

    /// Puts `text` on the clipboard as the user's, from a window that is gone right after.
    fn put_user_clipboard(text: &str) {
        let source = clipboard_test_window(w!("AutoKeyboardLayot desktop clipboard source"));
        put_test_clipboard_text(source, text);
        unsafe {
            DestroyWindow(source).expect("destroy source window");
        }
    }

    /// The clipboard's text once it is `expected`, or after `timeout`.
    fn clipboard_once(probe: HWND, expected: &str, timeout: Duration) -> Option<String> {
        let deadline = Instant::now() + timeout;
        while test_clipboard_unicode(probe).as_deref() != Some(expected)
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(50));
        }
        test_clipboard_unicode(probe)
    }

    #[test]
    #[ignore = "drives a window on the desktop: run it alone in an interactive session on the test VM"]
    fn a_protected_paste_on_the_desktop_gives_a_busy_field_the_replacement_and_never_the_clipboard()
    {
        let field = DesktopField::start();
        let probe = clipboard_test_window(w!("AutoKeyboardLayot desktop paste probe"));
        let mut outcomes = Vec::new();
        // A field that is not busy, one busy for a while or long after the clipboard changed, and
        // one that is busy until the paste stopped waiting, then reads the new clipboard itself
        // and handles the paste a moment later (less than the grace after a target's read).
        for (busy_ms, read_then_busy_ms) in [(0, 0), (300, 0), (1_000, 0), (300, 300)] {
            put_user_clipboard("akl-password");
            field.set_text("ghbdtn");
            field.configure(busy_ms, read_then_busy_ms);
            let foreground = current_foreground_context().expect("foreground");
            assert!(
                same_input_target(foreground, field.context),
                "the field lost the foreground"
            );
            let (mut processor, receiver) = processor_with_diagnostics();
            processor.foreground_override = Some(foreground);
            processor
                .metrics
                .observed_input_sequence
                .store(5, Ordering::Release);
            let attempt = processor.perform_protected_paste(
                foreground,
                5,
                foreground.layout,
                HKL(foreground.layout as *mut c_void),
                "ghbdtn",
                "привет",
                PastePatience::Manual,
            );
            // The clipboard is read only once the field settled: earlier it would be a read of the
            // replacement by another process than the paste's target.
            let text = field.settled_text("ghbdtn");
            let clipboard = clipboard_once(probe, "akl-password", Duration::from_secs(4));
            let brokers = reap_all(&processor.clipboard_brokers);
            eprintln!(
                "busy_ms={busy_ms} read_then_busy_ms={read_then_busy_ms} attempt={attempt:?} \
                 replaced={} clipboard_back={} brokers={brokers:?}",
                text == "привет",
                clipboard.as_deref() == Some("akl-password")
            );
            for line in receiver
                .try_iter()
                .filter(|line| line.starts_with("event=paste"))
            {
                eprintln!("  {line}");
            }
            outcomes.push((busy_ms, read_then_busy_ms, attempt, text, clipboard));
        }
        unsafe {
            DestroyWindow(probe).expect("destroy probe window");
        }
        drop(field);
        for (busy_ms, read_then_busy_ms, attempt, text, clipboard) in outcomes {
            let case = format!(
                "busy_ms={busy_ms} read_then_busy_ms={read_then_busy_ms} attempt={attempt:?}"
            );
            assert_eq!(text, "привет", "{case}");
            assert_eq!(clipboard.as_deref(), Some("akl-password"), "{case}");
        }
    }

    #[test]
    #[ignore = "runs the agent on the desktop: run it alone in an interactive session on the test VM"]
    fn the_agent_on_the_desktop_converts_a_word_in_a_busy_field_and_never_pastes_the_clipboard() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetKeyboardLayoutList, MAPVK_VK_TO_VSC, MapVirtualKeyW,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowW, WM_CLOSE, WM_INPUTLANGCHANGEREQUEST,
        };
        // The word is typed in English and converted to Russian: the account needs both layouts.
        let mut layouts = [HKL::default(); 16];
        let count = unsafe { GetKeyboardLayoutList(Some(&mut layouts)) }.max(0) as usize;
        let languages: Vec<usize> = layouts[..count.min(layouts.len())]
            .iter()
            .map(|layout| layout.0 as usize & 0xffff)
            .collect();
        assert!(
            languages.contains(&0x0409) && languages.contains(&0x0419),
            "the account needs the English and Russian layouts: {languages:x?}"
        );
        // A profile of its own: diagnostics on, and a word typed after the test switches the
        // layout back to English still converted.
        let profile =
            std::env::temp_dir().join(format!("akl-agent-acceptance-{}", std::process::id()));
        let directory = profile.join("AutoKeyboardLayot");
        fs::create_dir_all(&directory).expect("profile");
        let mut document = ConfigurationDocument::default();
        document.settings.diagnostics_enabled = true;
        document.settings.suppress_after_manual_layout_change = false;
        fs::write(
            directory.join("config.ini"),
            document.to_text().expect("configuration"),
        )
        .expect("configuration");
        // Run alone by its name, this is the only test in the process, and the agent's threads,
        // which read the variable, start after it is set.
        unsafe {
            std::env::set_var("LOCALAPPDATA", &profile);
        }
        let field = DesktopField::start();
        let probe = clipboard_test_window(w!("AutoKeyboardLayot agent acceptance probe"));
        let agent = thread::spawn(run);
        let deadline = Instant::now() + Duration::from_secs(10);
        let agent_window = loop {
            if let Ok(window) = unsafe { FindWindowW(WINDOW_CLASS, None) } {
                break window;
            }
            assert!(Instant::now() < deadline, "the agent did not start");
            thread::sleep(Duration::from_millis(50));
        };
        // The agent sends panics to its crash log; the test wants them on screen.
        drop(std::panic::take_hook());
        // The worker reads the configuration and the layouts.
        thread::sleep(Duration::from_secs(2));
        let type_keys = |keys: &[VIRTUAL_KEY]| {
            for key in keys {
                let scan = unsafe { MapVirtualKeyW(u32::from(key.0), MAPVK_VK_TO_VSC) } as u16;
                for flags in [Default::default(), KEYEVENTF_KEYUP] {
                    let input =
                        keyboard_input_with_marker(*key, scan, flags, ACCEPTANCE_TYPING_MARKER);
                    assert!(send_input_batch(&[input]));
                    thread::sleep(Duration::from_millis(40));
                }
            }
        };
        let english = 0x0409_0409usize;
        let mut outcomes = Vec::new();
        // A field that pastes at once, and one busy when the replacement arrives.
        for busy_ms in [0usize, 300] {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(field.context.hwnd as *mut c_void)),
                    WM_INPUTLANGCHANGEREQUEST,
                    WPARAM(0),
                    LPARAM(english as isize),
                );
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while current_foreground_context().is_none_or(|context| context.layout != english)
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(50));
            }
            field.set_text("");
            put_user_clipboard("akl-password");
            field.configure(busy_ms, 0);
            thread::sleep(Duration::from_millis(500));
            // g h b d t n, Space: "привет" typed in the English layout.
            type_keys(&[
                VIRTUAL_KEY(0x47),
                VIRTUAL_KEY(0x48),
                VIRTUAL_KEY(0x42),
                VIRTUAL_KEY(0x44),
                VIRTUAL_KEY(0x54),
                VIRTUAL_KEY(0x4E),
                VK_SPACE,
            ]);
            let text = field.settled_text("ghbdtn ");
            let clipboard = clipboard_once(probe, "akl-password", Duration::from_secs(4));
            eprintln!(
                "busy_ms={busy_ms} converted={} clipboard_back={} text_chars={}",
                text == "привет ",
                clipboard.as_deref() == Some("akl-password"),
                text.chars().count()
            );
            outcomes.push((busy_ms, text, clipboard));
        }
        // The agent's own record: the gates it armed, the conversions and the pastes.
        let record = |filter: &dyn Fn(&str) -> bool| {
            let log = fs::read_to_string(directory.join("diagnostics.log")).unwrap_or_default();
            for line in log.lines().filter(|line| filter(line)) {
                eprintln!("  {line}");
            }
        };
        record(&|line| {
            line.contains("event=paste")
                || line.contains("event=conversion")
                || line.contains("phase=armed")
                || line.contains("event=gate_ack")
        });
        // A graceful close is declined while the agent holds keys back for recovery, which expire
        // after RETAINED_INPUT_LIFETIME_MS: it is asked again until it accepts.
        let deadline = Instant::now() + Duration::from_millis(RETAINED_INPUT_LIFETIME_MS + 10_000);
        while !agent.is_finished() && Instant::now() < deadline {
            unsafe {
                let _ = PostMessageW(Some(agent_window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            thread::sleep(Duration::from_secs(5));
        }
        record(&|line| line.contains("phase=shutdown"));
        assert!(agent.is_finished(), "the agent declined to stop");
        let stopped = agent.join();
        unsafe {
            DestroyWindow(probe).expect("destroy probe window");
        }
        drop(field);
        let _ = fs::remove_dir_all(&profile);
        assert!(
            matches!(stopped, Ok(Ok(()))),
            "the agent did not stop cleanly"
        );
        for (busy_ms, text, clipboard) in outcomes {
            assert_eq!(text, "привет ", "busy_ms={busy_ms}");
            assert_eq!(
                clipboard.as_deref(),
                Some("akl-password"),
                "busy_ms={busy_ms}"
            );
        }
    }

    // Tray acceptance on the desktop of the test VM: the real notification area and Explorer.

    /// The rectangle of `window`'s tray icon on the screen, while the shell shows it.
    fn tray_icon_rect(window: HWND) -> Option<windows::Win32::Foundation::RECT> {
        use windows::Win32::UI::Shell::{NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect};
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: window,
            uID: TRAY_ID,
            ..Default::default()
        };
        unsafe { Shell_NotifyIconGetRect(&identifier) }.ok()
    }

    /// Every visible top-level window: its handle, class name and text.
    fn visible_windows() -> Vec<(usize, String, String)> {
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetClassNameW, GetWindowTextW, IsWindowVisible,
        };
        unsafe extern "system" fn collect(window: HWND, list: LPARAM) -> windows::core::BOOL {
            let list = unsafe { &mut *(list.0 as *mut Vec<(usize, String, String)>) };
            if unsafe { IsWindowVisible(window) }.as_bool() {
                let mut class = [0u16; 128];
                let mut text = [0u16; 512];
                let class_length = unsafe { GetClassNameW(window, &mut class) }.max(0) as usize;
                let text_length = unsafe { GetWindowTextW(window, &mut text) }.max(0) as usize;
                list.push((
                    window.0 as usize,
                    String::from_utf16_lossy(&class[..class_length]),
                    String::from_utf16_lossy(&text[..text_length]),
                ));
            }
            true.into()
        }
        let mut list: Vec<(usize, String, String)> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize));
        }
        list
    }

    /// The names that UI Automation shows under `window`, down to eight levels, as
    /// "control type: name".
    fn automation_texts(window: usize) -> Vec<String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(automation) =
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            else {
                return Vec::new();
            };
            let (Ok(walker), Ok(top)) = (
                automation.RawViewWalker(),
                automation.ElementFromHandle(HWND(window as *mut c_void)),
            ) else {
                return Vec::new();
            };
            let mut texts = Vec::new();
            let mut stack = vec![(top, 0)];
            while let Some((element, depth)) = stack.pop() {
                if texts.len() > 500 {
                    break;
                }
                let kind = element.CurrentControlType().map_or(0, |kind| kind.0);
                let name = element
                    .CurrentName()
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                if !name.is_empty() {
                    texts.push(format!("{kind}: {name}"));
                }
                if depth < 8 {
                    let mut child = walker.GetFirstChildElement(&element).ok();
                    while let Some(current) = child {
                        child = walker.GetNextSiblingElement(&current).ok();
                        stack.push((current, depth + 1));
                    }
                }
            }
            texts
        }
    }

    /// Saves the whole screen as a 32-bit bitmap file.
    fn save_screenshot(path: &Path) {
        use windows::Win32::Graphics::Gdi::{
            BI_RGB, BITMAPFILEHEADER, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap,
            CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, GetDC, GetDIBits, ReleaseDC, SRCCOPY,
            SelectObject,
        };
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        unsafe {
            let (width, height) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            let _ = BitBlt(memory, 0, 0, width, height, Some(screen), 0, 0, SRCCOPY);
            SelectObject(memory, previous);
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            GetDIBits(
                memory,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            let offset = (size_of::<BITMAPFILEHEADER>() + size_of::<BITMAPINFOHEADER>()) as u32;
            let file = BITMAPFILEHEADER {
                bfType: 0x4D42,
                bfSize: offset + pixels.len() as u32,
                bfOffBits: offset,
                ..Default::default()
            };
            let mut bytes = Vec::new();
            bytes.extend_from_slice(core::slice::from_raw_parts(
                (&raw const file).cast::<u8>(),
                size_of::<BITMAPFILEHEADER>(),
            ));
            bytes.extend_from_slice(core::slice::from_raw_parts(
                (&raw const header.bmiHeader).cast::<u8>(),
                size_of::<BITMAPINFOHEADER>(),
            ));
            bytes.extend_from_slice(&pixels);
            let _ = fs::write(path, bytes);
        }
    }

    /// The names of the tooltips that UI Automation finds among the desktop's windows, their
    /// children and grandchildren.
    fn automation_tooltips() -> Vec<String> {
        use windows::Win32::UI::Accessibility::UIA_ToolTipControlTypeId;
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(automation) =
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            else {
                return Vec::new();
            };
            let (Ok(walker), Ok(root)) = (automation.RawViewWalker(), automation.GetRootElement())
            else {
                return Vec::new();
            };
            let mut names = Vec::new();
            let mut stack = vec![(root, 0)];
            let mut visited = 0;
            while let Some((element, depth)) = stack.pop() {
                visited += 1;
                if visited > 3_000 {
                    break;
                }
                if element
                    .CurrentControlType()
                    .is_ok_and(|kind| kind == UIA_ToolTipControlTypeId)
                    && let Ok(name) = element.CurrentName()
                {
                    names.push(name.to_string());
                }
                if depth < 3 {
                    let mut child = walker.GetFirstChildElement(&element).ok();
                    while let Some(current) = child {
                        child = walker.GetNextSiblingElement(&current).ok();
                        stack.push((current, depth + 1));
                    }
                }
            }
            names
        }
    }

    /// Moves the pointer to `point` with real input, so that the window under it sees the move.
    fn move_pointer(point: POINT) {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
            MOUSEINPUT,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        };
        let (left, top, width, height) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        let scale =
            |value: i32, origin: i32, size: i32| (value - origin) * 65_535 / (size - 1).max(1);
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: scale(point.x, left, width),
                    dy: scale(point.y, top, height),
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    ..Default::default()
                },
            },
        };
        assert!(send_input_batch(&[input]), "move the pointer");
    }

    /// The first value `probe` gives within `timeout`, asked every 100 ms.
    fn within<T>(timeout: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(value) = probe() {
                return Some(value);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Lets the calling thread's windows handle what is queued for them.
    fn pump_messages() {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    /// Moves this executable's tray icon out of the overflow area, as the taskbar settings do.
    /// Windows 11 keeps a new icon there, and an icon there cannot be hovered over.
    fn promote_this_executables_tray_icon() -> bool {
        let executable = std::env::current_exe().expect("executable");
        let name = executable
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .to_lowercase();
        let key = within(Duration::from_secs(10), || {
            let output = std::process::Command::new("reg")
                .args(["query", r"HKCU\Control Panel\NotifyIconSettings", "/s"])
                .output()
                .ok()?;
            let listing = String::from_utf8_lossy(&output.stdout).into_owned();
            let mut current = None;
            for line in listing.lines() {
                if line.starts_with("HKEY_CURRENT_USER") {
                    current = Some(line.trim().to_owned());
                } else if line.contains("ExecutablePath") && line.to_lowercase().ends_with(&name) {
                    return current.clone();
                }
            }
            None
        });
        let Some(key) = key else {
            return false;
        };
        std::process::Command::new("reg")
            .args([
                "add",
                &key,
                "/v",
                "IsPromoted",
                "/t",
                "REG_DWORD",
                "/d",
                "1",
                "/f",
            ])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    /// Whether Explorer's taskbar exists.
    fn taskbar_exists() -> bool {
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.is_ok()
    }

    /// The agent's state on this thread, owning a window with the agent's procedure, so that the
    /// tray code runs as in the agent. Explorer's TaskbarCreated reaches that window.
    fn desktop_tray_state() -> (HWND, Receiver<QueuedInputEvent>) {
        TASKBAR_CREATED.store(
            unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            Ordering::Release,
        );
        let window = agent_procedure_window();
        let (mut state, receiver) = test_gate_state(4);
        state.window = window;
        APP_STATE.with(|slot| *slot.borrow_mut() = Some(state));
        (window, receiver)
    }

    thread_local! {
        /// The notifications that the icon of `notification_window` received (lParam's low word).
        static ICON_NOTIFICATIONS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "system" fn notification_procedure(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == TRAY_CALLBACK_MESSAGE {
            ICON_NOTIFICATIONS.with(|list| list.borrow_mut().push((lparam.0 as u32) & 0xffff));
            return LRESULT(0);
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// A top-level window of the calling thread that records its icon's notifications.
    fn notification_window() -> HWND {
        let module = unsafe { GetModuleHandleW(None).expect("module") };
        let class = WNDCLASSW {
            lpfnWndProc: Some(notification_procedure),
            hInstance: HINSTANCE(module.0),
            lpszClassName: w!("AutoKeyboardLayotNotificationTest"),
            ..Default::default()
        };
        unsafe { RegisterClassW(&class) };
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("AutoKeyboardLayotNotificationTest"),
                w!("AutoKeyboardLayot notification test"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
            .expect("window")
        }
    }

    #[test]
    #[ignore = "uses the notification area: run it alone in an interactive session on the test VM"]
    fn the_tray_icon_on_the_desktop_shows_its_status_in_a_tooltip() {
        let (_agent_window, _receiver) = desktop_tray_state();
        // The icon belongs to a window that records what the shell tells it.
        let window = notification_window();
        let status = TrayStatus {
            auto_enabled: true,
            ..TrayStatus::initial(LayoutIndicator::English)
        };
        assert!(add_tray(window, status), "the icon was not added");
        let expected = tray_tooltip(status);
        let mode = expected.split(';').next().expect("mode").to_owned();
        eprintln!("rect before promotion: {:?}", tray_icon_rect(window));
        let promoted = promote_this_executables_tray_icon();
        let rect = within(Duration::from_secs(10), || tray_icon_rect(window));
        eprintln!("promoted={promoted} rect={rect:?} expected={mode:?}");
        let mut shown = None;
        if let Some(rect) = rect {
            let center = POINT {
                x: (rect.left + rect.right) / 2,
                y: (rect.top + rect.bottom) / 2,
            };
            // Hover: from above the icon onto it, a little more after a moment.
            move_pointer(POINT {
                x: center.x,
                y: center.y - 80,
            });
            thread::sleep(Duration::from_millis(300));
            move_pointer(center);
            thread::sleep(Duration::from_millis(100));
            move_pointer(POINT {
                x: center.x + 1,
                y: center.y,
            });
            let mut captured = false;
            let hovered = Instant::now();
            shown = within(Duration::from_secs(6), || {
                pump_messages();
                if !captured && hovered.elapsed() > Duration::from_millis(2_500) {
                    captured = true;
                    save_screenshot(Path::new("tray-tooltip.bmp"));
                }
                // A classic tooltip carries its text; one of the XAML taskbar's popups shows it
                // to UI Automation.
                visible_windows()
                    .into_iter()
                    .flat_map(|(window, class, text)| {
                        let mut seen = vec![format!("window {class}: {text}")];
                        if class.contains("Popup") || class.to_lowercase().contains("tooltip") {
                            seen.extend(
                                automation_texts(window)
                                    .into_iter()
                                    .map(|text| format!("{class} automation {text}")),
                            );
                        }
                        seen
                    })
                    .chain(
                        automation_tooltips()
                            .into_iter()
                            .map(|name| format!("automation tooltip: {name}")),
                    )
                    .find(|seen| seen.contains(&mode))
            });
            if !captured {
                save_screenshot(Path::new("tray-tooltip.bmp"));
            }
        }
        pump_messages();
        let notifications = ICON_NOTIFICATIONS.with(|list| list.borrow().clone());
        eprintln!("notifications: {notifications:x?}");
        eprintln!("tooltip: {shown:?}");
        for (window, class, text) in visible_windows() {
            eprintln!("  visible {window:#x} {class}: {text}");
            if class.contains("Popup") || class.to_lowercase().contains("tooltip") {
                for text in automation_texts(window) {
                    eprintln!("    {text}");
                }
            }
        }
        for name in automation_tooltips() {
            eprintln!("  automation tooltip: {name}");
        }
        drop(take_app_state());
        unsafe {
            let _ = DestroyWindow(window);
        }
        assert!(rect.is_some(), "the shell does not show the icon");
        assert!(shown.is_some(), "no tooltip with the status appeared");
        // Without NIF_SHOWTIP the shell leaves the hover to the program (NIN_POPUPOPEN), and a
        // shell that honours that shows no tooltip at all.
        assert!(
            !notifications.contains(&windows::Win32::UI::Shell::NIN_POPUPOPEN),
            "the shell asked for a popup of the program's own instead of showing the tooltip"
        );
    }

    #[test]
    #[ignore = "restarts Explorer: run it alone in an interactive session on the test VM"]
    fn the_tray_icon_on_the_desktop_returns_after_explorer_restarts_without_a_false_pause_notice() {
        let (window, _receiver) = desktop_tray_state();
        // A conversion failed and paused automatic conversion, and the tray announced it.
        APP_STATE.with(|slot| {
            let borrowed = slot.borrow();
            let metrics = &borrowed.as_ref().expect("state").metrics;
            metrics.auto_enabled.store(false, Ordering::Release);
            metrics.conversion_failures.store(1, Ordering::Release);
            metrics
                .last_failure_reason
                .store(ConversionFailureReason::TextCommit as u8, Ordering::Release);
        });
        let indicator = LayoutIndicator::English;
        assert!(add_tray(window, TrayStatus::initial(indicator)));
        update_tray(indicator, Instant::now());
        // From here on the calls to the notification area are recorded on their way to it.
        SCRIPTED_SHELL.with(|slot| {
            *slot.borrow_mut() = Some(ScriptedShell {
                real: true,
                ..ScriptedShell::default()
            });
        });
        let _ = std::process::Command::new("taskkill")
            .args(["/f", "/im", "explorer.exe"])
            .output();
        let gone = within(Duration::from_secs(10), || {
            (!taskbar_exists()).then_some(())
        });
        // Windows starts the shell again by itself; this does it when it does not.
        thread::sleep(Duration::from_secs(3));
        let restarted_by_windows = taskbar_exists();
        if !restarted_by_windows {
            let _ = std::process::Command::new("explorer.exe").spawn();
        }
        // The agent's timer ticks: TaskbarCreated reaches the window between them.
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut back_since = None;
        while Instant::now() < deadline {
            pump_messages();
            retry_tray(window);
            update_tray(indicator, Instant::now());
            let added = scripted_shell(|shell| {
                shell
                    .calls
                    .iter()
                    .any(|call| call.message == NIM_ADD && call.answer)
            });
            if added && back_since.is_none() {
                back_since = Some(Instant::now());
            }
            if back_since.is_some_and(|since| since.elapsed() > Duration::from_secs(5)) {
                break;
            }
            thread::sleep(Duration::from_millis(250));
        }
        let rect = tray_icon_rect(window);
        let shell = SCRIPTED_SHELL
            .with(|slot| slot.borrow_mut().take())
            .expect("recording shell");
        for call in &shell.calls {
            eprintln!(
                "  {:?} flags={:?} answer={} tip={:?}",
                call.message, call.flags, call.answer, call.tip
            );
        }
        let balloons = shell
            .calls
            .iter()
            .filter(|call| call.flags.contains(NIF_INFO))
            .count();
        eprintln!(
            "gone={:?} restarted_by_windows={restarted_by_windows} back={:?} rect={rect:?} \
             balloons={balloons}",
            gone.is_some(),
            back_since.is_some()
        );
        drop(take_app_state());
        unsafe {
            let _ = DestroyWindow(window);
        }
        assert!(gone.is_some(), "Explorer did not stop");
        assert!(back_since.is_some(), "the icon did not come back");
        assert_eq!(balloons, 0, "an old failure was announced again");
    }

    #[test]
    #[ignore = "runs the agent without Explorer: run it alone in an interactive session on the test VM \
                with AutoRestartShell off"]
    fn the_agent_on_the_desktop_starts_without_a_taskbar_and_adds_its_icon_once_there_is_one() {
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, WM_CLOSE};
        let profile =
            std::env::temp_dir().join(format!("akl-tray-acceptance-{}", std::process::id()));
        let directory = profile.join("AutoKeyboardLayot");
        fs::create_dir_all(&directory).expect("profile");
        let mut document = ConfigurationDocument::default();
        document.settings.diagnostics_enabled = true;
        fs::write(
            directory.join("config.ini"),
            document.to_text().expect("configuration"),
        )
        .expect("configuration");
        // Run alone by its name, this is the only test in the process, and the agent's threads,
        // which read the variable, start after it is set.
        unsafe {
            std::env::set_var("LOCALAPPDATA", &profile);
        }
        let _ = std::process::Command::new("taskkill")
            .args(["/f", "/im", "explorer.exe"])
            .output();
        let gone = within(Duration::from_secs(10), || {
            (!taskbar_exists()).then_some(())
        });
        // With AutoRestartShell off nothing brings the taskbar back on its own.
        thread::sleep(Duration::from_secs(3));
        let still_gone = !taskbar_exists();
        let agent = thread::spawn(run);
        let agent_window = within(Duration::from_secs(10), || {
            unsafe { FindWindowW(WINDOW_CLASS, None) }.ok()
        });
        // The agent sends panics to its crash log; the test wants them on screen.
        drop(std::panic::take_hook());
        thread::sleep(Duration::from_secs(5));
        let running_without_taskbar = !agent.is_finished();
        let _ = std::process::Command::new("explorer.exe").spawn();
        let taskbar = within(Duration::from_secs(60), || taskbar_exists().then_some(()));
        let log = || fs::read_to_string(directory.join("diagnostics.log")).unwrap_or_default();
        let added = within(Duration::from_secs(60), || {
            log()
                .lines()
                .any(|line| line.contains("event=tray") && line.contains("result=added"))
                .then_some(())
        });
        let rect = agent_window.and_then(tray_icon_rect);
        for line in log().lines().filter(|line| line.contains("event=tray")) {
            eprintln!("  {line}");
        }
        eprintln!(
            "gone={} still_gone={still_gone} window={} running={running_without_taskbar} \
             taskbar={} added={} rect={rect:?}",
            gone.is_some(),
            agent_window.is_some(),
            taskbar.is_some(),
            added.is_some()
        );
        // A graceful close is declined while keys are retained; none are here, but ask again.
        let deadline = Instant::now() + Duration::from_secs(30);
        while !agent.is_finished() && Instant::now() < deadline {
            if let Some(window) = agent_window {
                unsafe {
                    let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            thread::sleep(Duration::from_secs(2));
        }
        let stopped = agent.is_finished().then(|| agent.join());
        let _ = fs::remove_dir_all(&profile);
        assert!(gone.is_some() && still_gone, "Explorer was not stopped");
        assert!(agent_window.is_some(), "the agent created no window");
        assert!(
            running_without_taskbar,
            "the agent stopped without a taskbar"
        );
        assert!(taskbar.is_some(), "Explorer did not start again");
        assert!(added.is_some(), "the agent did not add its icon");
        assert!(
            matches!(stopped, Some(Ok(Ok(())))),
            "the agent did not stop cleanly"
        );
    }
}
