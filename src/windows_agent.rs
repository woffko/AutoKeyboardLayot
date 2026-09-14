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
use autokeyboardlayot::installed_packages::{InstalledPackages, PackageSource};
use autokeyboardlayot::profile_resolver::ResolvedKeyboardProfiles;
use autokeyboardlayot::tray_visual::{self, TrayVisual};
use autokeyboardlayot::windows_input_profiles::KeyboardProfileCache;
use ui_localization::{tr, tr_format};

mod ui_localization;
use autokeyboardlayot::{
    BackendRules, BackendStrategy, ConfigurationDocument, ConversionTransaction, Detection,
    Detector, ExclusionPolicy, HOTKEY_MOD_ALT, HOTKEY_MOD_CONTROL, HOTKEY_MOD_SHIFT,
    HOTKEY_MOD_WIN, Hotkey, InputEvent, InputSession, Language, PrivacyBlockReason, SessionAction,
    Settings, UserLexicon,
};

mod installer_lifecycle;
mod settings_window;
pub use installer_lifecycle::{initialize_installation, prepare_upgrade};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, GlobalFree, HANDLE, HGLOBAL,
            HINSTANCE, HWND, LPARAM, LRESULT, POINT, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM,
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
                OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{
                CF_UNICODETEXT, OleGetClipboard, OleInitialize, OleUninitialize, ReleaseStgMedium,
            },
            Threading::{
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateMutexW, CreateWaitableTimerExW,
                GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, ReleaseMutex,
                SetWaitableTimerEx, TIMER_ALL_ACCESS, WaitForSingleObject,
            },
        },
        UI::{
            Accessibility::{
                CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement,
                IUIAutomationTextPattern, TextPatternRangeEndpoint_End,
                TextPatternRangeEndpoint_Start, TextUnit_Character, UIA_CONTROLTYPE_ID,
                UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_TextPatternId,
            },
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, GetKeyState, GetKeyboardLayout, GetKeyboardState, HKL, INPUT,
                INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
                KEYEVENTF_SCANCODE, MAPVK_VSC_TO_VK_EX, MapVirtualKeyExW, SendInput, ToUnicodeEx,
                VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_F24,
                VK_HOME, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL,
                VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB,
                VK_UP,
            },
            Shell::{
                NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
                NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NINF_KEY, NOTIFYICON_VERSION_4,
                NOTIFYICONDATAW, Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CallNextHookEx, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
                DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW,
                GUITHREADINFO, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetMessageW,
                GetShellWindow, GetWindowThreadProcessId, HHOOK, HICON, HMENU, ICONINFO, IDYES,
                KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLMHF_INJECTED, MB_ICONERROR,
                MB_ICONINFORMATION, MB_OK, MB_YESNO, MF_CHECKED, MF_GRAYED, MF_SEPARATOR,
                MF_STRING, MF_UNCHECKED, MSG, MSLLHOOKSTRUCT, MessageBoxW, PM_REMOVE, PeekMessageW,
                PostMessageW, PostQuitMessage, RegisterClassW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
                SMTO_ERRORONEXIT, SendMessageTimeoutW, SetForegroundWindow, SetTimer,
                SetWindowTextW, SetWindowsHookExW, TPM_RIGHTBUTTON, TrackPopupMenu,
                TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
                WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU,
                WM_DESTROY, WM_INPUTLANGCHANGEREQUEST, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK,
                WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_NULL, WM_RBUTTONDOWN, WM_SYSKEYDOWN,
                WM_SYSKEYUP, WM_TIMER, WM_XBUTTONDOWN, WNDCLASSW,
            },
        },
    },
    core::{Error, HRESULT, HSTRING, Interface, PCWSTR, PWSTR, Result, w},
};

#[cfg(test)]
use windows::Win32::{
    Foundation::{ERROR_SUCCESS, SetLastError},
    System::DataExchange::{EnumClipboardFormats, GetClipboardData},
    UI::{
        Accessibility::UIA_TextControlTypeId,
        Input::KeyboardAndMouse::{VK_F12, VK_PAUSE},
    },
};

const WINDOW_CLASS: PCWSTR = w!("AutoKeyboardLayot.ObserverWindow");
const WINDOW_TITLE: PCWSTR = w!("AutoKeyboardLayot");
const TRAY_ID: u32 = 1;
const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
// Composite macro from Shellapi.h (not emitted by windows-rs metadata).
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
const GATE_ARM_MESSAGE: u32 = WM_APP + 2;
const GATE_RELEASE_MESSAGE: u32 = WM_APP + 3;
const GATE_DRAIN_ACK_MESSAGE: u32 = WM_APP + 4;
const GATE_FAIL_OPEN_MESSAGE: u32 = WM_APP + 5;
const GATE_HANDOFF_BUDGET_MESSAGE: u32 = WM_APP + 6;
const LEXICON_OFFER_MESSAGE: u32 = WM_APP + 7;
const SETTINGS_APPLIED_MESSAGE: u32 = WM_APP + 8;
const RECOVERY_NOTICE_MESSAGE: u32 = WM_APP + 9;
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
const NOTEPAD_REPLAY_STEP_DELAY_MS: u64 = 3;
const NOTEPAD_REPLAY_GATE_MARGIN_MS: u64 = 200;
const CLIPBOARD_OPEN_RETRIES: usize = 10;
const CLIPBOARD_RETRY_DELAY_MS: u64 = 2;
const CLIPBOARD_BROKER_TIMEOUT_MS: u64 = 500;
const VK_V_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
const LAYOUT_SWITCH_TIMEOUT_MS: u64 = 250;
const LAYOUT_STABLE_MS: u64 = 50;
const REPLAY_COMMIT_MS: u64 = 50;
const CORRECTION_GATE_MAX_HOLD_MS: u64 = 1_200;
const CORRECTION_GATE_CAPACITY: usize = 256;
const TO_UNICODE_DO_NOT_CHANGE_KEYBOARD_STATE: u32 = 4;
const PRIVACY_ALLOWED: u8 = 0;
// Worker-only wait: the low-level hook never waits for UI Automation.
// Give ordinary delayed replies more headroom before failing closed.
const PRIVACY_PROBE_WAIT_MS: u64 = 300;
const SLOW_INPUT_DIAGNOSTIC_MS: u64 = 30;
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
}

#[derive(Debug)]
struct ObserverMetrics {
    candidates: AtomicU64,
    dropped_events: AtomicU64,
    input_epoch: AtomicU64,
    configuration_pending: AtomicBool,
    configuration_ack: AtomicU64,
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
    gate_replayed_events: AtomicU64,
    observed_input_sequence: AtomicU64,
    forwarded_input_sequence: AtomicU64,
    active_gate_token: AtomicU64,
    cancelled_gate_token: AtomicU64,
    privacy_refresh_queued: AtomicBool,
    worker_busy: AtomicBool,
    retained_input: Mutex<Option<RetainedInput>>,
    recovery_armed: AtomicBool,
}

impl Default for ObserverMetrics {
    fn default() -> Self {
        Self {
            candidates: AtomicU64::new(0),
            dropped_events: AtomicU64::new(0),
            input_epoch: AtomicU64::new(0),
            configuration_pending: AtomicBool::new(false),
            configuration_ack: AtomicU64::new(0),
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
            gate_replayed_events: AtomicU64::new(0),
            observed_input_sequence: AtomicU64::new(0),
            forwarded_input_sequence: AtomicU64::new(0),
            active_gate_token: AtomicU64::new(0),
            cancelled_gate_token: AtomicU64::new(0),
            privacy_refresh_queued: AtomicBool::new(false),
            worker_busy: AtomicBool::new(false),
            retained_input: Mutex::new(None),
            recovery_armed: AtomicBool::new(false),
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
}

#[derive(Debug, Clone)]
struct RetainedInput {
    token: u64,
    foreground: ForegroundContext,
    events: Vec<RawKeyEvent>,
    delivery_uncertain: bool,
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

    fn handoff(&mut self, old_token: u64, new_token: u64) -> bool {
        if !self.active
            || self.cancelled
            || !self.awaiting_worker_ack
            || self.token != old_token
            || new_token == 0
            || self.handoff_budget_ms == 0
            || self.deadline.saturating_duration_since(Instant::now())
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
    ReloadConfiguration,
    RecoverInput {
        sequence: u64,
        explicit: bool,
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

#[derive(Debug, Clone)]
struct PendingConversion {
    transaction: ConversionTransaction,
    foreground: ForegroundContext,
    source_layout: usize,
    profile_generation: u64,
    space_down_sequence: u64,
    replay_keys: Vec<ReplayKey>,
    forced: bool,
}

/// The most recent space-delimited word, retained so the user can still convert
/// it with the manual hotkey after the space has been typed.
struct LastBoundary {
    replay_keys: Vec<ReplayKey>,
    delimiter: char,
    foreground: ForegroundContext,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextEditAttempt {
    Applied,
    Unsupported,
    Cancelled,
    Failed,
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
    fn open(owner: HWND) -> Option<Self> {
        for _ in 0..CLIPBOARD_OPEN_RETRIES {
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
    Restore,
    Abandon,
}

#[derive(Debug, Clone, Copy)]
enum ClipboardBrokerResponse {
    Installed { sequence: u32, owner: usize },
    Finished(bool),
    Failed,
}

struct ProtectedClipboard {
    owner: HWND,
    temporary_sequence: u32,
    command: SyncSender<ClipboardBrokerCommand>,
    response: Receiver<ClipboardBrokerResponse>,
    worker: Option<JoinHandle<()>>,
    active: bool,
}

impl ProtectedClipboard {
    fn begin(text: &str) -> Option<Self> {
        let (command_sender, command_receiver) = sync_channel(1);
        let (response_sender, response_receiver) = sync_channel(1);
        let text = text.to_owned();
        let worker = thread::spawn(move || {
            protected_clipboard_broker(text, command_receiver, response_sender);
        });
        let installed = response_receiver
            .recv_timeout(Duration::from_millis(CLIPBOARD_BROKER_TIMEOUT_MS))
            .ok();
        let Some(ClipboardBrokerResponse::Installed {
            sequence: temporary_sequence,
            owner,
        }) = installed
        else {
            let _ = command_sender.try_send(ClipboardBrokerCommand::Restore);
            if worker.is_finished() {
                let _ = worker.join();
            }
            return None;
        };

        Some(Self {
            owner: HWND(owner as *mut c_void),
            temporary_sequence,
            command: command_sender,
            response: response_receiver,
            worker: Some(worker),
            active: true,
        })
    }

    fn is_current(&self) -> bool {
        self.active
            && unsafe { GetClipboardSequenceNumber() } == self.temporary_sequence
            && unsafe { GetClipboardOwner().ok() } == Some(self.owner)
    }

    fn restore(&mut self) -> bool {
        if !self.active {
            return true;
        }
        self.active = false;
        let was_current = self.is_current_after_deactivation();
        let command = if was_current {
            ClipboardBrokerCommand::Restore
        } else {
            ClipboardBrokerCommand::Abandon
        };
        if self.command.send(command).is_err() {
            if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
                let _ = self.worker.take().map(JoinHandle::join);
            }
            return false;
        }
        let response = self
            .response
            .recv_timeout(Duration::from_millis(CLIPBOARD_BROKER_TIMEOUT_MS))
            .ok();
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            let _ = self.worker.take().map(JoinHandle::join);
        }
        was_current && matches!(response, Some(ClipboardBrokerResponse::Finished(true)))
    }

    fn is_current_after_deactivation(&self) -> bool {
        (unsafe { GetClipboardSequenceNumber() }) == self.temporary_sequence
            && (unsafe { GetClipboardOwner().ok() }) == Some(self.owner)
    }
}

impl Drop for ProtectedClipboard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

struct ClipboardBrokerWindow(HWND);

impl ClipboardBrokerWindow {
    fn new() -> Option<Self> {
        let module = unsafe { GetModuleHandleW(None).ok()? };
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
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

fn protected_clipboard_broker(
    text: String,
    commands: Receiver<ClipboardBrokerCommand>,
    responses: SyncSender<ClipboardBrokerResponse>,
) {
    if unsafe { OleInitialize(None).is_err() } {
        let _ = responses.send(ClipboardBrokerResponse::Failed);
        return;
    }
    let Some(owner) = ClipboardBrokerWindow::new() else {
        let _ = responses.send(ClipboardBrokerResponse::Failed);
        unsafe {
            OleUninitialize();
        }
        return;
    };
    let owner_handle = owner.0;
    let mut snapshot = materialize_current_clipboard();
    let installed = snapshot.is_some() && install_protected_clipboard(owner_handle, &text);
    if !installed {
        if let Some(snapshot) = &mut snapshot {
            let _ = restore_materialized_clipboard(owner_handle, None, snapshot);
        }
        let _ = responses.send(ClipboardBrokerResponse::Failed);
        unsafe {
            OleUninitialize();
        }
        return;
    }

    let temporary_sequence = unsafe { GetClipboardSequenceNumber() };
    if responses
        .send(ClipboardBrokerResponse::Installed {
            sequence: temporary_sequence,
            owner: owner_handle.0 as usize,
        })
        .is_err()
    {
        if let Some(snapshot) = &mut snapshot {
            let _ =
                restore_materialized_clipboard(owner_handle, Some(temporary_sequence), snapshot);
        }
        unsafe {
            OleUninitialize();
        }
        return;
    }

    let restored = match wait_clipboard_broker_command(&commands) {
        Ok(ClipboardBrokerCommand::Restore)
            if (unsafe { GetClipboardSequenceNumber() }) == temporary_sequence
                && (unsafe { GetClipboardOwner().ok() }) == Some(owner_handle) =>
        {
            snapshot.as_mut().is_some_and(|snapshot| {
                restore_materialized_clipboard(owner_handle, Some(temporary_sequence), snapshot)
            })
        }
        Ok(ClipboardBrokerCommand::Restore | ClipboardBrokerCommand::Abandon) | Err(_) => false,
    };
    let _ = responses.send(ClipboardBrokerResponse::Finished(restored));
    unsafe {
        OleUninitialize();
    }
}

fn wait_clipboard_broker_command(
    commands: &Receiver<ClipboardBrokerCommand>,
) -> core::result::Result<ClipboardBrokerCommand, ()> {
    loop {
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        match commands.try_recv() {
            Ok(command) => return Ok(command),
            Err(TryRecvError::Disconnected) => return Err(()),
            Err(TryRecvError::Empty) => {}
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn install_protected_clipboard(owner: HWND, text: &str) -> bool {
    let Some(_open) = ClipboardOpenGuard::open(owner) else {
        return false;
    };
    let exclusion =
        unsafe { RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")) };
    let history = unsafe { RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")) };
    let cloud = unsafe { RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")) };
    if exclusion == 0 || history == 0 || cloud == 0 {
        return false;
    }
    let Some(mut exclusion_data) = global_bytes(&0u32.to_ne_bytes()) else {
        return false;
    };
    let Some(mut history_data) = global_bytes(&0u32.to_ne_bytes()) else {
        return false;
    };
    let Some(mut cloud_data) = global_bytes(&0u32.to_ne_bytes()) else {
        return false;
    };
    let Some(mut text_data) = global_utf16(text) else {
        return false;
    };
    if unsafe { EmptyClipboard().is_err() } {
        return false;
    }
    exclusion_data.transfer(exclusion)
        && history_data.transfer(history)
        && cloud_data.transfer(cloud)
        && text_data.transfer(u32::from(CF_UNICODETEXT.0))
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
    let Some(_open) = ClipboardOpenGuard::open(owner) else {
        return false;
    };
    if expected_sequence.is_some_and(|sequence| {
        (unsafe { GetClipboardSequenceNumber() }) != sequence
            || (unsafe { GetClipboardOwner().ok() }) != Some(owner)
    }) || unsafe { EmptyClipboard().is_err() }
    {
        return false;
    }
    entries
        .iter_mut()
        .all(MaterializedClipboardEntry::transfer_to_open_clipboard)
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
    #[default]
    None,
    Resolver,
    Selection,
    Caret,
    RangeMove,
    TextRead,
    TextCompared,
    SelectCall,
    SelectionConfirm,
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
    configuration_loader: Option<RuntimeConfigurationLoader>,
    lexicon_candidate: Arc<Mutex<Option<VolatileLexiconCandidate>>>,
    diagnostic_sender: Option<SyncSender<String>>,
    diagnostic_worker: Option<JoinHandle<()>>,
}

impl AppState {
    // Keep pumping window messages until the worker has actually returned: it
    // may be waiting for a synchronous gate request handled by this window.
    // A worker_busy snapshot alone cannot establish that it has terminated.
    fn request_shutdown(&mut self) -> bool {
        if self.shutdown.load(Ordering::Acquire) {
            return true;
        }
        if self.correction_gate.active
            || has_retained_input(&self.metrics)
            || self.metrics.worker_busy.load(Ordering::Acquire)
            || self.metrics.configuration_pending.load(Ordering::Acquire)
            || self.metrics.undo_hotkey_active.load(Ordering::Acquire)
            || self
                .metrics
                .hotkey_waiting_for_release
                .load(Ordering::Acquire)
        {
            return false;
        }
        self.shutdown.store(true, Ordering::Release);
        self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        // Disconnect recv even when there is no queued event to wake it.
        self.input_sender.take();
        true
    }

    fn shutdown_finished(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
            && self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn enqueue(&self, event: RawInputEvent) -> bool {
        self.enqueue_with_configuration(event, None)
    }

    fn enqueue_with_configuration(
        &self,
        event: RawInputEvent,
        configuration: Option<Box<ConfigurationReload>>,
    ) -> bool {
        let Some(sender) = &self.input_sender else {
            return false;
        };
        let privacy_refresh = matches!(event, RawInputEvent::PrivacyRefresh(_));
        if privacy_refresh
            && self
                .metrics
                .privacy_refresh_queued
                .swap(true, Ordering::AcqRel)
        {
            return true;
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
            Ok(()) => true,
            Err(TrySendError::Disconnected(_)) => {
                if privacy_refresh {
                    self.metrics
                        .privacy_refresh_queued
                        .store(false, Ordering::Release);
                }
                false
            }
            Err(TrySendError::Full(_)) => {
                if privacy_refresh {
                    self.metrics
                        .privacy_refresh_queued
                        .store(false, Ordering::Release);
                    return false;
                }
                self.metrics.dropped_events.fetch_add(1, Ordering::Relaxed);
                self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
                false
            }
        }
    }

    fn enqueue_configuration(&mut self, snapshot: RuntimeConfiguration) -> bool {
        if self.shutdown.load(Ordering::Acquire) {
            return false;
        }
        let previous_sources = self
            .pending_configuration
            .as_ref()
            .map_or(self.configuration_sources, |pending| {
                pending.snapshot.sources
            });
        if !previous_sources.accepts_reload(snapshot.sources) {
            return false;
        }
        let previous_packages = self
            .pending_configuration
            .as_ref()
            .map_or(self.package_source, |pending| {
                pending.snapshot.package_source
            });
        if !previous_packages.accepts_reload(snapshot.package_source) {
            return false;
        }
        if self.correction_gate.active
            || has_retained_input(&self.metrics)
            || self
                .next_configuration_revision
                .saturating_sub(self.metrics.configuration_ack.load(Ordering::Acquire))
                >= 2
        {
            return false;
        }
        let Some(revision) = self.next_configuration_revision.checked_add(1) else {
            return false;
        };
        let reload = ConfigurationReload { revision, snapshot };
        self.metrics
            .configuration_pending
            .store(true, Ordering::Release);
        self.metrics.input_epoch.fetch_add(1, Ordering::AcqRel);
        if !self.enqueue_with_configuration(
            RawInputEvent::ReloadConfiguration,
            Some(Box::new(reload.clone())),
        ) {
            // A failed newer request must not cancel an already queued reload.
            self.metrics
                .configuration_pending
                .store(self.pending_configuration.is_some(), Ordering::Release);
            return false;
        }
        self.next_configuration_revision = revision;
        self.pending_configuration = Some(reload);
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
        if !self.correction_gate.active {
            return;
        }
        let token = self.correction_gate.token;
        let held_count = self.correction_gate.held.len();
        let sequence = self.metrics.observed_input_sequence.load(Ordering::Acquire);
        let foreground = self.correction_gate.origin;
        let held = core::mem::take(&mut self.correction_gate.held);
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
                            current_foreground_context()
                                .filter(|current| same_input_target(origin, *current))
                                .unwrap_or(origin)
                        })
                        .unwrap_or(fallback),
                    events: held,
                    delivery_uncertain: cause == GateFailureCause::DownstreamHook,
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
            "cause={cause:?} token={token} held={held_count} released={} in_flight={} awaiting_ack={} worker_busy={} paused={failed}",
            self.correction_gate.released_count, self.correction_gate.drain_in_flight,
            self.correction_gate.awaiting_worker_ack, self.metrics.worker_busy.load(Ordering::Acquire),
        ));
        let _ = self.enqueue(RawInputEvent::GateDrainReady {
            token,
            drained_count: 0,
            replay_succeeded: false,
        });
        if retained
            && matches!(
                cause,
                GateFailureCause::Deadline
                    | GateFailureCause::Capacity
                    | GateFailureCause::WorkerQueue
            )
        {
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

fn request_shutdown(hwnd: HWND) {
    let accepted = APP_STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(AppState::request_shutdown)
    });
    if accepted {
        finish_shutdown(hwnd);
    } else {
        show_tray_information(hwnd, "AutoKeyboardLayot", tr("lifecycle.close_busy"));
    }
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
    }
    requested
}

fn take_app_state() -> Option<AppState> {
    APP_STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        state.take()
    })
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
            let state = slot.borrow();
            let Some(state) = state.as_ref() else {
                return false;
            };
            let same_record = state
                .metrics
                .retained_input
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .is_some_and(|input| input.token == token);
            state
                .metrics
                .recovery_armed
                .store(same_record, Ordering::Release);
            same_record
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
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow().as_ref() {
            state
                .metrics
                .retained_input
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            state.metrics.recovery_armed.store(false, Ordering::Release);
            state.gate_diagnostic("recovery_discarded", "explicit=true".to_owned());
        }
    });
    show_tray_information(
        hwnd,
        tr("recovery.discarded_title"),
        tr("recovery.discarded_body"),
    );
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

fn enqueue_configuration_reload(configuration: RuntimeConfiguration) -> bool {
    APP_STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(|state| state.enqueue_configuration(configuration))
    })
}

fn finish_configuration_reload(hwnd: HWND) {
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

fn finish_configuration_load(hwnd: HWND) {
    let result = APP_STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .and_then(|state| state.configuration_loader.as_mut())
            .and_then(RuntimeConfigurationLoader::poll)
    });
    let Some(result) = result else {
        return;
    };
    let configuration = match result {
        Ok(configuration) => configuration,
        Err(error) => {
            show_configuration_error(
                hwnd,
                &tr_format("error.read_config", &[("error", &error.to_string())]),
            );
            return;
        }
    };
    if !enqueue_configuration_reload(configuration) {
        show_configuration_error(
            hwnd,
            &tr_format(
                "error.read_config",
                &[("error", "configuration_handoff_unavailable")],
            ),
        );
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

fn add_recent_lexicon_offer(hwnd: HWND) {
    let Some(candidate) = valid_lexicon_candidate() else {
        return;
    };
    if let Err(error) = persist_lexicon_candidate(&candidate) {
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
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow().as_ref()
            && let Ok(mut stored) = state.lexicon_candidate.lock()
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
        if let Some(state) = slot.borrow_mut().as_mut()
            && !shell_foreground
            && now >= state.next_privacy_refresh
        {
            state.enqueue(RawInputEvent::PrivacyRefresh(context));
            state.next_privacy_refresh = now + Duration::from_millis(500);
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
        };
        if status == state.last_tray_status {
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
    let mut tray_slot = Some(tray);
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut()
            && state.tray.is_none()
        {
            state.tray = tray_slot.take();
            if updated {
                state.last_tray_status = status;
            }
        }
    });
    if updated && notify_failure {
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
    fn add(hwnd: HWND, indicator: LayoutIndicator) -> Result<Self> {
        let icon = create_indicator_icon(TrayVisual::Disabled, indicator)?;
        let mut data = notify_icon_data(hwnd, icon);
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
        let tooltip = tray_tooltip(TrayStatus::initial(indicator));
        set_tooltip(&mut data, &tooltip);
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            unsafe {
                let _ = DestroyIcon(icon);
            }
            return Err(Error::from_thread());
        }

        let mut version_data = notify_icon_data(hwnd, icon);
        version_data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        if !unsafe { Shell_NotifyIconW(NIM_SETVERSION, &version_data) }.as_bool() {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                let _ = DestroyIcon(icon);
            }
            return Err(Error::from_thread());
        }
        unsafe {
            let _ = SetWindowTextW(hwnd, &HSTRING::from(&tooltip));
        }
        Ok(Self {
            hwnd,
            icon,
            visual: TrayVisual::Disabled,
            indicator,
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
            | if visual_changed {
                NIF_ICON
            } else {
                Default::default()
            };
        let tooltip = tray_tooltip(status);
        set_tooltip(&mut data, &tooltip);
        if !unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
            if let Some(new_icon) = new_icon {
                unsafe {
                    let _ = DestroyIcon(new_icon);
                }
            }
            return Err(Error::from_thread());
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
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let data = notify_icon_data(self.hwnd, self.icon);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            let _ = DestroyIcon(self.icon);
        }
    }
}

pub fn run() -> Result<()> {
    let initial_configuration = load_runtime_configuration().map_err(|error| {
        Error::new(
            windows::core::HRESULT(0x8007000Du32 as i32),
            tr_format("error.read_config", &[("error", &error.to_string())]),
        )
    })?;
    ui_localization::initialize(
        &initial_configuration.ui_language,
        initial_configuration.package_catalogs.as_deref(),
    );
    let Some(_instance_guard) = InstanceGuard::acquire()? else {
        return Ok(());
    };

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

        let initial_indicator = current_foreground_context()
            .map(|context| LayoutIndicator::from_layout(context.layout))
            .unwrap_or(LayoutIndicator::Unavailable);
        installer_lifecycle::advertise_safe_close(hwnd);
        let tray = TrayIcon::add(hwnd, initial_indicator)?;
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
        let diagnostic_worker = thread::spawn(move || diagnostic_worker(diagnostic_receiver));
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
                tray: Some(tray),
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
                configuration_loader: RuntimeConfigurationLoader::spawn().ok(),
                lexicon_candidate,
                diagnostic_sender: Some(diagnostic_sender),
                diagnostic_worker: Some(diagnostic_worker),
            });
        });

        let keyboard_hook =
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), Some(instance), 0) {
                Ok(hook) => hook,
                Err(error) => {
                    drop(take_app_state());
                    let _ = DestroyWindow(hwnd);
                    return Err(error);
                }
            };
        let mouse_hook =
            match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), Some(instance), 0) {
                Ok(hook) => hook,
                Err(error) => {
                    let _ = UnhookWindowsHookEx(keyboard_hook);
                    drop(take_app_state());
                    let _ = DestroyWindow(hwnd);
                    return Err(error);
                }
            };
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.keyboard_hook = Some(keyboard_hook);
                state.mouse_hook = Some(mouse_hook);
            }
        });

        if SetTimer(Some(hwnd), LAYOUT_TIMER_ID, LAYOUT_TIMER_INTERVAL_MS, None) == 0 {
            let error = Error::from_thread();
            let _ = DestroyWindow(hwnd);
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

pub fn run_settings() -> core::result::Result<(), String> {
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
            if finish_shutdown(hwnd) {
                return LRESULT(0);
            }
            finish_configuration_load(hwnd);
            finish_configuration_reload(hwnd);
            expire_correction_gate();
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
            fail_open_correction_gate(wparam.0 as u64);
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
            if APP_STATE.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .is_some_and(|state| has_retained_input(&state.metrics))
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
            match tray_interaction(notification) {
                TrayInteraction::Settings => settings_window::show(hwnd),
                TrayInteraction::QuickMenu => {
                    let _ = show_tray_menu(hwnd, None);
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
                MENU_EXIT_ID => request_shutdown(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            request_shutdown(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayInteraction {
    Settings,
    QuickMenu,
    Ignore,
}

fn tray_interaction(notification: u32) -> TrayInteraction {
    match notification {
        WM_LBUTTONDBLCLK | NIN_KEYSELECT | NIN_SELECT => TrayInteraction::Settings,
        WM_CONTEXTMENU => TrayInteraction::QuickMenu,
        _ => TrayInteraction::Ignore,
    }
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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
                    if event.flags.contains(LLKHF_INJECTED) && !drained {
                        state.break_correction_gate_fail_open(GateFailureCause::ExternalInjection);
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
                    };

                    let mut claimed_hotkey = false;
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
                        if state.correction_gate.hold(raw) {
                            swallow = true;
                            return;
                        }
                        if state.correction_gate.active {
                            let cause = if Instant::now() >= state.correction_gate.deadline {
                                GateFailureCause::Deadline
                            } else {
                                GateFailureCause::Capacity
                            };
                            // Include this edge in recovery as well: forwarding
                            // it now would overtake all previously swallowed input.
                            state.correction_gate.held.push(raw);
                            swallow = true;
                            state.break_correction_gate_fail_open(cause);
                            return;
                        }
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
                        if state.metrics.recovery_armed.swap(false, Ordering::AcqRel) {
                            state.enqueue(RawInputEvent::RecoverInput {
                                sequence,
                                explicit: true,
                            })
                        } else {
                            state.gate_diagnostic(
                                "recovery_not_armed",
                                format!("sequence={sequence}"),
                            );
                            true
                        }
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
    if code >= 0
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state
                    .metrics
                    .observed_input_sequence
                    .fetch_add(1, Ordering::AcqRel);
                if event.flags & LLMHF_INJECTED == 0 {
                    state.break_correction_gate_fail_open(GateFailureCause::Mouse);
                    state.enqueue(RawInputEvent::Mouse);
                } else {
                    state.enqueue(RawInputEvent::ExternalInjection);
                }
            }
        });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
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

fn held_tail_invalidates_undo(held: &[RawKeyEvent]) -> bool {
    held.iter().any(|event| {
        hotkey_modifier_bit(event.virtual_key as u16).is_none()
            && (event.virtual_key as u16 != VK_SPACE.0
                || !matches!(event.message, WM_KEYUP | WM_SYSKEYUP))
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
                || !state.correction_gate.handoff(old_token, new_token)
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
            fail_open_correction_gate(old_token);
        }
    }
    acknowledged
}

fn fail_open_correction_gate(token: u64) {
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut()
            && state.correction_gate.active
            && state.correction_gate.token == token
        {
            state.break_correction_gate_fail_open(GateFailureCause::WorkerAck);
        }
    });
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
                return Some(PrivacyBlockReason::InspectionUnavailable);
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
                Err(_) => Some(PrivacyBlockReason::InspectionUnavailable),
            }
        }
    }

    fn preceding_text_status(
        &self,
        expected_process_id: u32,
        expected_text: &str,
    ) -> TextBarrierStatus {
        self.last_barrier_report.set(TextBarrierReport {
            expected_chars: expected_text.chars().count(),
            ..TextBarrierReport::default()
        });
        let Ok(character_count) = i32::try_from(expected_text.encode_utf16().count()) else {
            return TextBarrierStatus::Unavailable;
        };
        if character_count == 0 || character_count > 128 {
            return TextBarrierStatus::Unavailable;
        }

        unsafe {
            let (_element, pattern) = match self.focused_editable_text_pattern(expected_process_id)
            {
                Ok(value) => value,
                Err(status) => return status,
            };
            let Ok(ranges) = pattern.GetSelection() else {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::Selection,
                    expected_chars: expected_text.chars().count(),
                    ..TextBarrierReport::default()
                });
                return TextBarrierStatus::Unavailable;
            };
            if ranges.Length().ok() != Some(1) {
                return TextBarrierStatus::Mismatch;
            }
            let Ok(caret) = ranges.GetElement(0) else {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::Caret,
                    expected_chars: expected_text.chars().count(),
                    ..TextBarrierReport::default()
                });
                return TextBarrierStatus::Unavailable;
            };
            if caret
                .CompareEndpoints(
                    TextPatternRangeEndpoint_Start,
                    &caret,
                    TextPatternRangeEndpoint_End,
                )
                .ok()
                != Some(0)
            {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::Caret,
                    expected_chars: expected_text.chars().count(),
                    ..TextBarrierReport::default()
                });
                return TextBarrierStatus::Mismatch;
            }
            let Ok(range) = caret.Clone() else {
                return TextBarrierStatus::Unavailable;
            };
            let moved = range
                .MoveEndpointByUnit(
                    TextPatternRangeEndpoint_Start,
                    TextUnit_Character,
                    -character_count,
                )
                .ok();
            if moved != Some(-character_count) {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::RangeMove,
                    expected_chars: expected_text.chars().count(),
                    moved_units: moved.unwrap_or(i32::MIN),
                    ..TextBarrierReport::default()
                });
                return TextBarrierStatus::Mismatch;
            }
            let Ok(actual) = range.GetText(-1) else {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::TextRead,
                    expected_chars: expected_text.chars().count(),
                    moved_units: moved.unwrap_or_default(),
                    ..TextBarrierReport::default()
                });
                return TextBarrierStatus::Unavailable;
            };
            let actual = actual.to_string();
            self.last_barrier_report.set(classify_text_barrier(
                &actual,
                expected_text,
                TextBarrierStage::TextCompared,
                moved.unwrap_or_default(),
            ));
            if text_barrier_matches(&actual, expected_text) {
                TextBarrierStatus::Match
            } else {
                TextBarrierStatus::Mismatch
            }
        }
    }

    fn select_preceding_text(
        &self,
        expected_process_id: u32,
        expected_text: &str,
    ) -> TextBarrierStatus {
        self.last_barrier_report.set(TextBarrierReport {
            expected_chars: expected_text.chars().count(),
            ..TextBarrierReport::default()
        });
        let Ok(character_count) = i32::try_from(expected_text.encode_utf16().count()) else {
            return TextBarrierStatus::Unavailable;
        };
        if character_count == 0 || character_count > 128 {
            return TextBarrierStatus::Unavailable;
        }

        unsafe {
            let (_element, pattern) = match self.focused_editable_text_pattern(expected_process_id)
            {
                Ok(value) => value,
                Err(status) => return status,
            };
            let Ok(ranges) = pattern.GetSelection() else {
                return TextBarrierStatus::Unavailable;
            };
            if ranges.Length().ok() != Some(1) {
                return TextBarrierStatus::Mismatch;
            }
            let Ok(caret) = ranges.GetElement(0) else {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::Caret,
                    expected_chars: expected_text.chars().count(),
                    ..TextBarrierReport::default()
                });
                return TextBarrierStatus::Unavailable;
            };
            if caret
                .CompareEndpoints(
                    TextPatternRangeEndpoint_Start,
                    &caret,
                    TextPatternRangeEndpoint_End,
                )
                .ok()
                != Some(0)
            {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::Caret,
                    expected_chars: expected_text.chars().count(),
                    ..TextBarrierReport::default()
                });
                return TextBarrierStatus::Mismatch;
            }
            let Ok(range) = caret.Clone() else {
                return TextBarrierStatus::Unavailable;
            };
            let moved = range
                .MoveEndpointByUnit(
                    TextPatternRangeEndpoint_Start,
                    TextUnit_Character,
                    -character_count,
                )
                .ok();
            if moved != Some(-character_count) {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::RangeMove,
                    expected_chars: expected_text.chars().count(),
                    moved_units: moved.unwrap_or(i32::MIN),
                    ..TextBarrierReport::default()
                });
                return TextBarrierStatus::Mismatch;
            }
            let Ok(actual) = range.GetText(-1) else {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::TextRead,
                    expected_chars: expected_text.chars().count(),
                    moved_units: moved.unwrap_or_default(),
                    ..TextBarrierReport::default()
                });
                return TextBarrierStatus::Unavailable;
            };
            let actual = actual.to_string();
            self.last_barrier_report.set(classify_text_barrier(
                &actual,
                expected_text,
                TextBarrierStage::TextCompared,
                moved.unwrap_or_default(),
            ));
            if !text_barrier_matches(&actual, expected_text) {
                return TextBarrierStatus::Mismatch;
            }
            if range.Select().is_err() {
                let mut report = self.last_barrier_report.get();
                report.stage = TextBarrierStage::SelectCall;
                self.last_barrier_report.set(report);
                return TextBarrierStatus::Mismatch;
            }

            let deadline = Instant::now() + Duration::from_millis(UIA_SELECTION_CONFIRM_TIMEOUT_MS);
            let mut saw_mismatch = false;
            while Instant::now() < deadline {
                let Ok(selected_ranges) = pattern.GetSelection() else {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                };
                if selected_ranges.Length().ok() != Some(1) {
                    saw_mismatch = true;
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                let Ok(selected) = selected_ranges.GetElement(0) else {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                };
                if selected
                    .CompareEndpoints(
                        TextPatternRangeEndpoint_Start,
                        &selected,
                        TextPatternRangeEndpoint_End,
                    )
                    .ok()
                    == Some(0)
                {
                    saw_mismatch = true;
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                if let Ok(actual) = selected.GetText(-1) {
                    let actual = actual.to_string();
                    self.last_barrier_report.set(classify_text_barrier(
                        &actual,
                        expected_text,
                        TextBarrierStage::SelectionConfirm,
                        moved.unwrap_or_default(),
                    ));
                    if text_barrier_matches(&actual, expected_text) {
                        return TextBarrierStatus::Match;
                    }
                    saw_mismatch = true;
                }
                thread::sleep(Duration::from_millis(1));
            }
            if saw_mismatch {
                TextBarrierStatus::Mismatch
            } else {
                self.last_barrier_report.set(TextBarrierReport {
                    stage: TextBarrierStage::SelectionConfirm,
                    expected_chars: expected_text.chars().count(),
                    moved_units: moved.unwrap_or_default(),
                    ..TextBarrierReport::default()
                });
                TextBarrierStatus::Unavailable
            }
        }
    }

    fn collapse_selection_to_end(&self, expected_process_id: u32) {
        unsafe {
            let Ok((_element, pattern)) = self.focused_editable_text_pattern(expected_process_id)
            else {
                return;
            };
            let Ok(ranges) = pattern.GetSelection() else {
                return;
            };
            if ranges.Length().ok() != Some(1) {
                return;
            }
            let Ok(range) = ranges.GetElement(0) else {
                return;
            };
            if range
                .MoveEndpointByRange(
                    TextPatternRangeEndpoint_Start,
                    &range,
                    TextPatternRangeEndpoint_End,
                )
                .is_ok()
            {
                let _ = range.Select();
            }
        }
    }

    fn focused_editable_text_pattern(
        &self,
        expected_process_id: u32,
    ) -> core::result::Result<(IUIAutomationElement, IUIAutomationTextPattern), TextBarrierStatus>
    {
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
                            .map(|pattern| (element, pattern))
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
    }
}

fn format_text_barrier_report(report: TextBarrierReport) -> String {
    format!(
        "stage={:?},resolver_reason={:?},resolver_depth={},resolver_pid={},expected_chars={},actual_chars={},moved_units={},exact={},nbsp_equiv={},word_at_start={},word_at_end={},drop_first_is_word={},drop_last_is_word={},leading={:?},trailing={:?}",
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
        let (requests, receive) = sync_channel(1);
        let (reply, replies) = sync_channel(1);
        thread::Builder::new()
            .name("autokey-package-loader".into())
            .spawn(move || {
                while receive.recv().is_ok() {
                    if reply.send(load_runtime_configuration()).is_err() {
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

fn configuration_path() -> Option<PathBuf> {
    configuration_directory().map(|directory| directory.join("config.ini"))
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

struct ConfigurationWriteGuard(ProcessHandle);

impl ConfigurationWriteGuard {
    fn acquire() -> std::io::Result<Self> {
        let handle = unsafe {
            CreateMutexW(
                None,
                false,
                w!("Local\\AutoKeyboardLayot.ConfigurationWrite"),
            )
        }
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

fn save_configuration_if_unchanged(
    document: &ConfigurationDocument,
    original: &ConfigurationDocument,
) -> std::io::Result<()> {
    let _guard = ConfigurationWriteGuard::acquire()?;
    let current = try_load_configuration_document()?;
    if &current != original {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "Configuration changed outside this window. Reopen settings before saving; your current draft has not been discarded.",
        ));
    }
    write_configuration_document(document)
}

// Caller holds ConfigurationWriteGuard through its entire read/modify/write.
fn write_configuration_document(document: &ConfigurationDocument) -> std::io::Result<()> {
    let path = configuration_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
    })?;
    let contents = document
        .to_text()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    write_configuration_file_atomically(&path, &contents)
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

fn persist_lexicon_candidate(candidate: &VolatileLexiconCandidate) -> std::io::Result<()> {
    let _guard = ConfigurationWriteGuard::acquire()?;
    let mut document = try_load_configuration_document()?;
    let entries = match candidate.target {
        LexiconOfferTarget::UserDictionary => &mut document.user_dictionary,
        LexiconOfferTarget::WordExclusions => &mut document.word_exclusions,
    };
    let existing = entries.join("\n");
    let merged = merge_lexicon_entry(&existing, candidate).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid exclusion word")
    })?;
    *entries = merged.lines().map(str::to_owned).collect();
    write_configuration_document(&document)
}

fn diagnostic_log_path() -> Option<PathBuf> {
    configuration_directory().map(|directory| directory.join("diagnostics.log"))
}

fn diagnostic_worker(receiver: Receiver<String>) {
    while let Ok(record) = receiver.recv() {
        let _ = append_diagnostic_record(&record);
    }
}

fn append_diagnostic_record(record: &str) -> std::io::Result<()> {
    let Some(path) = diagnostic_log_path() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "LOCALAPPDATA is unavailable",
        ));
    };
    let Some(directory) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "diagnostic path has no parent",
        ));
    };
    fs::create_dir_all(directory)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let record = record.replace(['\r', '\n'], " ");
    let line = format!("{timestamp} {record}\r\n");
    let current_size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
    if current_size.saturating_add(line.len() as u64) > DIAGNOSTIC_LOG_MAX_BYTES {
        let rotated = path.with_extension("1.log");
        if rotated.exists() {
            fs::remove_file(&rotated)?;
        }
        if path.exists() {
            fs::rename(&path, rotated)?;
        }
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.flush()
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
    let mut processor = InputProcessor::new_with_lexicon_candidate(
        metrics,
        gate_window,
        lexicon_candidate,
        Some(diagnostic_sender),
        configuration,
    );
    while let Ok(event) = receiver.recv() {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        processor.metrics.worker_busy.store(true, Ordering::Release);
        processor.process(event);
        processor
            .metrics
            .worker_busy
            .store(false, Ordering::Release);
    }
}

struct InputProcessor {
    detector: Detector,
    input_profiles: KeyboardProfileCache,
    #[cfg(test)]
    profile_override: Option<ResolvedKeyboardProfiles>,
    session: InputSession,
    modifiers: Modifiers,
    privacy_guard: PrivacyGuard,
    privacy_probe: Option<BoundedProbe<(ForegroundContext, u64), Option<PrivacyBlockReason>>>,
    exclusion_policy: ExclusionPolicy,
    backend_rules: BackendRules,
    settings: Settings,
    lexicon_candidate: Arc<Mutex<Option<VolatileLexiconCandidate>>>,
    diagnostic_sender: Option<SyncSender<String>>,
    privacy_needs_check: bool,
    privacy_reason: Option<PrivacyBlockReason>,
    process_reason: Option<PrivacyBlockReason>,
    last_process_id: Option<u32>,
    current_integrity_level: Option<u32>,
    replay_keys: Vec<ReplayKey>,
    last_boundary: Option<LastBoundary>,
    transpose_cycle: Option<TransposeCycle>,
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
            Default::default(),
            configuration.dictionaries,
            &configuration.input_profiles,
        );
        detector.set_resolved_profiles(None);
        detector
            .replace_user_lexicons(configuration.user_dictionary, configuration.word_exclusions);
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
            session,
            modifiers: Modifiers::default(),
            privacy_guard: PrivacyGuard::new(),
            privacy_probe: BoundedProbe::spawn("autokey-privacy", || {
                let guard = PrivacyGuard::new();
                move |(foreground, _epoch): (ForegroundContext, u64)| {
                    if !foreground_identity_matches(foreground) {
                        return Some(PrivacyBlockReason::InspectionUnavailable);
                    }
                    let reason = guard.inspect(foreground.process_id);
                    if !foreground_identity_matches(foreground) {
                        return Some(PrivacyBlockReason::InspectionUnavailable);
                    }
                    reason
                }
            })
            .ok(),
            exclusion_policy: configuration.process_exclusions,
            backend_rules: configuration.backend_rules,
            settings: configuration.settings,
            lexicon_candidate,
            diagnostic_sender,
            privacy_needs_check: true,
            privacy_reason: Some(PrivacyBlockReason::InspectionUnavailable),
            process_reason: None,
            last_process_id: None,
            current_integrity_level: process_integrity_level(unsafe { GetCurrentProcessId() }),
            replay_keys: Vec::new(),
            last_boundary: None,
            transpose_cycle: None,
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
        }
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

    fn pending_profiles_match(&self, pending: &PendingConversion) -> bool {
        pending.profile_generation == self.input_profiles.generation()
            && self.language_for_layout(pending.source_layout)
                == Some(pending.transaction.source_language)
            && self
                .find_layout(pending.transaction.target_language)
                .is_some()
    }

    fn diagnostic(&self, event: &'static str, details: String) {
        if !self.metrics.diagnostics_enabled.load(Ordering::Acquire) {
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

    fn process(&mut self, queued: QueuedInputEvent) {
        let queue_wait_ms = queued.captured_at.elapsed().as_millis();
        if queue_wait_ms >= u128::from(SLOW_INPUT_DIAGNOSTIC_MS)
            || matches!(queued.event, RawInputEvent::GateDrainReady { .. })
        {
            let kind = match queued.event {
                RawInputEvent::Key(_) => "key",
                RawInputEvent::GateDrainReady { .. } => "gate_ack",
                RawInputEvent::PrivacyRefresh(_) => "privacy",
                RawInputEvent::RecoverInput { .. } => "recovery",
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
        if matches!(queued.event, RawInputEvent::PrivacyRefresh(_)) {
            self.metrics
                .privacy_refresh_queued
                .store(false, Ordering::Release);
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
                self.apply_configuration_reload(Ok(reload.snapshot));
                self.configuration_revision = reload.revision;
                self.metrics
                    .configuration_ack
                    .store(reload.revision, Ordering::Release);
            }
            return;
        }
        let current_epoch = self.metrics.input_epoch.load(Ordering::Acquire);
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
            self.invalidate_conversion_state();
            self.session.clear();
            self.replay_keys.clear();
            return;
        }

        match queued.event {
            RawInputEvent::Mouse => {
                self.invalidate_conversion_state();
                self.replay_keys.clear();
                self.last_boundary = None;
                self.transpose_cycle = None;
                self.session.handle(InputEvent::Mouse, None, &self.detector);
                self.mark_privacy_dirty(false);
            }
            RawInputEvent::ExternalInjection => {
                self.invalidate_conversion_state();
                self.suppress_session();
                self.mark_privacy_dirty(true);
            }
            RawInputEvent::Key(event) => self.process_key(event),
            RawInputEvent::RecoverInput { sequence, explicit } => {
                self.recover_retained_input(sequence, explicit)
            }
            RawInputEvent::PrivacyRefresh(foreground) => {
                // Periodic refresh is not a gate-control dependency. The next
                // timer/word retries; it must not delay a fence acknowledgement.
                if self.metrics.active_gate_token.load(Ordering::Acquire) != 0 {
                    return;
                }
                self.refresh_input_profiles();
                if foreground_identity_matches(foreground) {
                    self.evaluate_privacy(foreground);
                } else {
                    self.mark_privacy_dirty(false);
                }
            }
            RawInputEvent::ReloadConfiguration => unreachable!(),
            RawInputEvent::GateDrainReady { .. } => unreachable!(),
        }
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
        self.detector = Detector::with_profile_selections(
            Default::default(),
            configuration.dictionaries,
            &configuration.input_profiles,
        );
        let profiles = self.resolved_profiles().cloned();
        self.detector.set_resolved_profiles(profiles.as_ref());
        self.detector
            .replace_user_lexicons(configuration.user_dictionary, configuration.word_exclusions);
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
            let required_ms = self.pending_conversion.as_ref().and_then(|pending| {
                let step_count = pending
                    .transaction
                    .forward
                    .erase_characters
                    .checked_add(pending.replay_keys.len())?
                    .checked_add(1)?;
                conversion_gate_budget_ms(
                    step_count,
                    self.text_edit_backend.requires_text_barrier(),
                    matches!(self.text_edit_backend, TextEditBackend::CapabilityProbe),
                    matches!(
                        self.text_edit_backend,
                        TextEditBackend::ProtectedPaste | TextEditBackend::CapabilityProbe
                    ),
                    false,
                )
            });
            if pending_matches
                && required_ms.is_some_and(|required_ms| {
                    self.handoff_correction_gate(token, deferred.boundary_sequence, required_ms)
                })
            {
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
        self.invalidate_conversion_state();
        if queue_overflowed {
            self.session
                .handle(InputEvent::QueueOverflow, None, &self.detector);
        } else {
            self.session.clear();
        }
        self.replay_keys.clear();
        self.modifiers = Modifiers::default();
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
        if !self.input_sequence_is_current(sequence)
            || !foreground_identity_matches(record.foreground)
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
        if !self.ensure_privacy(record.foreground) {
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
        let sent = send_input_count(&inputs);
        {
            let mut retained = self
                .metrics
                .retained_input
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(current) = retained.as_mut()
                && current.token == record.token
            {
                if sent == inputs.len() {
                    *retained = None;
                } else if sent != 0 {
                    current.delivery_uncertain = true;
                }
            }
        }
        self.invalidate_conversion_state();
        self.session.clear();
        self.replay_keys.clear();
        self.diagnostic("recovery", format!("token={} explicit={explicit} submitted={sent} requested={} evidence=input-submitted-text-unverified", record.token, inputs.len()));
        // Recovery never silently re-enables automatic conversion.
    }

    fn process_key(&mut self, event: RawKeyEvent) {
        if self.metrics.active_gate_token.load(Ordering::Acquire) == 0 {
            self.refresh_input_profiles();
        }
        let foreground_key = foreground_identity_key(event.foreground);
        let language = self.language_for_layout(event.foreground.layout);
        if self.last_foreground != Some(foreground_key) {
            self.layout_switch_in_flight = None;
            self.invalidate_conversion_state();
            self.replay_keys.clear();
            self.last_boundary = None;
            self.transpose_cycle = None;
            self.session
                .handle(InputEvent::FocusChanged, language, &self.detector);
            self.modifiers = Modifiers::default();
            self.last_foreground = Some(foreground_key);
            self.last_layout = Some(event.foreground.layout);
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
                self.layout_switch_in_flight = None;
                self.invalidate_conversion_state();
                self.replay_keys.clear();
                self.last_boundary = None;
                self.transpose_cycle = None;
                self.handle_switching_rule(
                    InputEvent::LayoutChanged,
                    language,
                    self.settings.suppress_after_manual_layout_change,
                );
                self.last_layout = Some(event.foreground.layout);
            }
        } else if self
            .layout_switch_in_flight
            .is_some_and(|(_, deadline)| Instant::now() > deadline)
        {
            self.layout_switch_in_flight = None;
        }

        let key_down = matches!(event.message, WM_KEYDOWN | WM_SYSKEYDOWN);
        let key_up = matches!(event.message, WM_KEYUP | WM_SYSKEYUP);
        if !key_down && !key_up {
            return;
        }

        let modifier_event = self.modifiers.update(event.virtual_key, key_down, key_up);
        if event.hotkey_trigger {
            let released = self.settings.pause_break_undo && self.wait_for_hotkey_release(event);
            self.diagnostic(
                "hotkey",
                format!(
                    "sequence={} ready={released} undo_available={} buffered_chars={} deferred={}",
                    event.sequence,
                    self.metrics.undo_available.load(Ordering::Acquire),
                    self.session.buffered_character_count(),
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

        self.clear_undo();
        if event.virtual_key as u16 != VK_SPACE.0 {
            self.pending_conversion = None;
            self.deferred_drained_conversion = None;
        } else if self.pending_conversion.is_some() {
            return;
        }

        if self.modifiers.has_shortcut_modifier() {
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
            self.session
                .handle(InputEvent::Shortcut, language, &self.detector);
            self.mark_privacy_dirty(true);
            return;
        }

        match event.virtual_key as u16 {
            key if key == VK_BACK.0 => {
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Backspace,
                    language,
                    self.settings.suppress_after_backspace,
                );
            }
            key if key == VK_DELETE.0 => {
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Delete,
                    language,
                    self.settings.suppress_after_delete,
                );
            }
            key if key == VK_LEFT.0 => {
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Navigation,
                    language,
                    self.settings.suppress_after_left,
                );
            }
            key if key == VK_RIGHT.0 => {
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Navigation,
                    language,
                    self.settings.suppress_after_right,
                );
            }
            key if key == VK_UP.0 => {
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Navigation,
                    language,
                    self.settings.suppress_after_up,
                );
            }
            key if key == VK_DOWN.0 => {
                self.replay_keys.clear();
                self.handle_switching_rule(
                    InputEvent::Navigation,
                    language,
                    self.settings.suppress_after_down,
                );
            }
            key if key == VK_HOME.0 || key == VK_END.0 => {
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
                if let Some((transaction, replay_keys)) =
                    self.handle_boundary(language, Some(' '), false)
                    && self.metrics.auto_enabled.load(Ordering::Acquire)
                    && !self.modifiers.shift()
                    && replay_keys.len() == transaction.original.chars().count()
                    && replay_keys
                        .iter()
                        .all(|key| key.caps_lock == event.caps_lock)
                {
                    self.pending_conversion = Some(PendingConversion {
                        transaction,
                        foreground: event.foreground,
                        source_layout: event.foreground.layout,
                        profile_generation: self.input_profiles.generation(),
                        space_down_sequence: event.sequence,
                        replay_keys,
                        forced: false,
                    });
                }
                if event.drain_token == 0 && self.metrics.auto_enabled.load(Ordering::Acquire) {
                    self.execute_pending_conversion(event.sequence);
                } else if event.drain_token != 0 && self.pending_conversion.is_some() {
                    self.deferred_drained_conversion = Some(DeferredDrainedConversion {
                        gate_token: event.drain_token,
                        boundary_sequence: event.sequence,
                    });
                }
                if !last_keys.is_empty() && language.is_some() {
                    self.last_boundary = Some(LastBoundary {
                        replay_keys: last_keys,
                        delimiter: ' ',
                        foreground: event.foreground,
                    });
                }
            }
            key if key == VK_TAB.0 => {
                self.last_boundary = None;
                self.transpose_cycle = None;
                self.handle_boundary(language, None, true);
            }
            key if key == VK_RETURN.0 => {
                self.last_boundary = None;
                self.transpose_cycle = None;
                self.handle_boundary(language, None, true);
                self.session.mark_line_start();
            }
            _ => {
                if !self.ensure_privacy(event.foreground) {
                    self.diagnostic(
                        "input",
                        format!(
                            "result=suppressed reason=privacy pid={} route={:?}",
                            event.foreground.process_id, self.text_edit_backend
                        ),
                    );
                    self.suppress_session();
                    return;
                }
                if language.is_none() {
                    self.diagnostic("input", "result=suppressed reason=language".to_owned());
                }
                let input_event = translate_printable(event, self.modifiers).map_or(
                    InputEvent::UnsupportedInput,
                    |character| {
                        if language.is_some_and(|language| {
                            self.detector.can_extend_word(character, language)
                        }) {
                            InputEvent::Printable(character)
                        } else if character.is_whitespace() || character.is_ascii_punctuation() {
                            InputEvent::Boundary
                        } else {
                            InputEvent::UnsupportedInput
                        }
                    },
                );
                if input_event == InputEvent::UnsupportedInput {
                    self.diagnostic(
                        "input",
                        format!(
                            "result=suppressed reason=unsupported vk={}",
                            event.virtual_key
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
                let buffered_before = self.session.buffered_character_count();
                let action = self.session.handle(input_event, language, &self.detector);
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
                if boundary {
                    self.replay_keys.clear();
                    self.privacy_needs_check = true;
                }
            }
        }
    }

    fn wait_for_hotkey_release(&self, event: RawKeyEvent) -> bool {
        // The worker can receive a modifier-up event before the hook returns
        // and Windows updates GetAsyncKeyState. Wait for that exact edge to
        // finish instead of sending correction input with Ctrl/Alt still down.
        let deadline = Instant::now() + Duration::from_millis(HOOK_FORWARD_TIMEOUT_MS);
        loop {
            if !self.input_sequence_is_current(event.sequence)
                || !foreground_identity_matches(event.foreground)
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
        let candidates = language
            .map(|language| {
                mapped_layout_candidates(
                    &replay_keys,
                    language,
                    &self.settings,
                    &self.detector,
                    self.resolved_profiles(),
                )
            })
            .unwrap_or_default();
        let transaction = if self.privacy_reason.is_none() {
            if let SessionAction::Candidate(detection) = self
                .session
                .finish_boundary_with_candidates(language, &self.detector, &candidates)
            {
                self.metrics.candidates.fetch_add(1, Ordering::Relaxed);
                self.diagnostic(
                    "candidate",
                    format!(
                        "forced=false source={} target={} original_chars={} replacement_chars={} mapped_targets={} route={:?}",
                        detection.source_language.id(),
                        detection.target_language.id(),
                        detection.original.chars().count(),
                        detection.replacement.chars().count(),
                        candidates.len(),
                        self.text_edit_backend,
                    ),
                );
                delimiter.and_then(|delimiter| ConversionTransaction::new(&detection, delimiter))
            } else {
                None
            }
        } else {
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
            self.session.clear();
        }
    }

    fn execute_forced_conversion(&mut self, event: RawKeyEvent, _language: Option<Language>) {
        // A press while a manual cycle is active advances it to the next layout.
        if let Some(cycle) = self
            .transpose_cycle
            .clone()
            .filter(|cycle| same_input_target(cycle.foreground, event.foreground))
        {
            let Some(source_layout) = self.find_layout(cycle.language) else {
                self.diagnostic(
                    "hotkey",
                    "result=ignored reason=layout-unavailable".to_owned(),
                );
                return;
            };
            self.session.clear();
            if !self.ensure_privacy(event.foreground) {
                self.diagnostic("hotkey", "result=ignored reason=privacy".to_owned());
                return;
            }
            self.advance_layout_cycle(
                cycle.replay_keys,
                cycle.delimiter,
                cycle.foreground,
                source_layout.0 as usize,
                cycle.language,
                cycle.text,
                event.sequence,
            );
            return;
        }
        // The manual hotkey walks the word through every enabled layout, so it
        // works even when the word is not in any dictionary.
        let (replay_keys, delimiter, source_layout) = if !self.replay_keys.is_empty() {
            (self.replay_keys.clone(), None, event.foreground.layout)
        } else if let Some(last) = self
            .last_boundary
            .as_ref()
            .filter(|last| same_input_target(last.foreground, event.foreground))
        {
            (
                last.replay_keys.clone(),
                Some(last.delimiter),
                last.foreground.layout,
            )
        } else {
            self.diagnostic(
                "hotkey",
                "result=ignored reason=no-word-before-caret".to_owned(),
            );
            return;
        };
        let Some(source_language) = self.language_for_layout(source_layout) else {
            self.diagnostic(
                "hotkey",
                "result=ignored reason=unsupported-layout".to_owned(),
            );
            return;
        };
        let Some(current_text) =
            map_replay_keys_to_layout(&replay_keys, HKL(source_layout as *mut c_void))
        else {
            self.diagnostic(
                "hotkey",
                "result=ignored reason=no-word-before-caret".to_owned(),
            );
            return;
        };
        if !self.ensure_privacy(event.foreground) {
            self.diagnostic("hotkey", "result=ignored reason=privacy".to_owned());
            return;
        }
        self.advance_layout_cycle(
            replay_keys,
            delimiter,
            event.foreground,
            source_layout,
            source_language,
            current_text,
            event.sequence,
        );
    }

    /// Convert the word to the next enabled layout on each press.
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
    ) {
        let profiles = self.resolved_profiles().cloned();
        let order: Vec<Language> = self
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
        if order.len() < 2 {
            self.diagnostic("hotkey", "result=ignored reason=single-layout".to_owned());
            return;
        }
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
        let mut target = None;
        for step in 1..order.len() {
            let candidate_index = (position + step) % order.len();
            let candidate_language = order[candidate_index];
            let Some(candidate_layout) = self.find_layout(candidate_language) else {
                continue;
            };
            let Some(candidate_text) = map_replay_keys_to_layout(&replay_keys, candidate_layout)
            else {
                continue;
            };
            if candidate_text != current_text {
                target = Some((candidate_index, candidate_language, candidate_text));
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
                "forced=true cycle_index={next_index} source={} target={} original_chars={} replacement_chars={} route={:?}",
                detection.source_language.id(),
                detection.target_language.id(),
                detection.original.chars().count(),
                detection.replacement.chars().count(),
                self.text_edit_backend,
            ),
        );
        self.session.clear();
        self.replay_keys.clear();
        self.transpose_cycle = Some(TransposeCycle {
            replay_keys: replay_keys.clone(),
            delimiter,
            foreground,
            language: target_language,
            text: next_text,
        });
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
        });
        self.execute_pending_conversion(sequence);
    }

    fn ensure_privacy(&mut self, foreground: ForegroundContext) -> bool {
        self.refresh_process_policy(foreground.process_id);
        if self.privacy_needs_check {
            self.evaluate_privacy(foreground);
        }
        self.privacy_reason.is_none()
    }

    fn evaluate_privacy(&mut self, foreground: ForegroundContext) {
        self.refresh_process_policy(foreground.process_id);
        let started = Instant::now();
        let epoch = self.metrics.input_epoch.load(Ordering::Acquire);
        let (mut reason, mut source) = if let Some(reason) = self.process_reason {
            (Some(reason), "process-policy")
        } else {
            match self.privacy_probe.as_mut() {
                Some(probe) => match probe.query_result(
                    (foreground, epoch),
                    Duration::from_millis(PRIVACY_PROBE_WAIT_MS),
                    Duration::from_millis(500),
                ) {
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
                },
                None => (
                    Some(PrivacyBlockReason::InspectionUnavailable),
                    "probe-missing",
                ),
            }
        };
        // A late safe reply cannot authorize a different focus or input epoch.
        if self.metrics.input_epoch.load(Ordering::Acquire) != epoch
            || !foreground_identity_matches(foreground)
        {
            reason = Some(PrivacyBlockReason::InspectionUnavailable);
            source = "context-changed";
        }
        if reason != self.privacy_reason
            || started.elapsed() >= Duration::from_millis(SLOW_INPUT_DIAGNOSTIC_MS)
        {
            self.diagnostic(
                "privacy_probe",
                format!(
                    "elapsed_ms={} result={reason:?} source={source} wait_budget_ms={PRIVACY_PROBE_WAIT_MS}",
                    started.elapsed().as_millis()
                ),
            );
        }
        self.privacy_needs_check = false;
        self.set_privacy_reason(reason);
    }

    fn refresh_process_policy(&mut self, process_id: u32) {
        if self.last_process_id == Some(process_id) {
            return;
        }
        self.last_process_id = Some(process_id);
        let image = process_image_name(process_id);
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
        self.process_reason = match (
            image,
            process_integrity_level(process_id),
            self.current_integrity_level,
        ) {
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
        self.diagnostic(
            "context",
            format!(
                "pid={process_id} process={process_name} route={:?} privacy={:?}",
                self.text_edit_backend, self.process_reason
            ),
        );
    }

    fn mark_privacy_dirty(&mut self, pause_now: bool) {
        self.privacy_needs_check = true;
        if pause_now {
            self.set_privacy_reason(
                self.process_reason
                    .or(Some(PrivacyBlockReason::InspectionUnavailable)),
            );
        }
    }

    fn set_privacy_reason(&mut self, reason: Option<PrivacyBlockReason>) {
        self.privacy_reason = reason;
        self.metrics
            .privacy_reason
            .store(privacy_reason_code(reason), Ordering::Release);
        if reason.is_some() {
            self.invalidate_conversion_state();
            self.suppress_session();
        }
    }

    fn suppress_session(&mut self) {
        self.replay_keys.clear();
        self.session
            .handle(InputEvent::UnsupportedInput, None, &self.detector);
    }

    fn execute_pending_conversion(&mut self, boundary_sequence: u64) {
        self.execute_pending_conversion_inner(boundary_sequence);
        self.request_gate_release(boundary_sequence);
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
        if (!pending.forced && !self.metrics.auto_enabled.load(Ordering::Acquire))
            || self.privacy_reason.is_some()
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
            ReplayAttempt::Failed => {
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
        let edit_strategy = match self.text_edit_backend {
            TextEditBackend::ObserveOnly => return,
            TextEditBackend::ProtectedPaste => {
                match self.perform_protected_paste(
                    pending.foreground,
                    boundary_sequence,
                    pending.source_layout,
                    target_layout,
                    &pending.transaction.undo.insert_text,
                    &pending.transaction.forward.insert_text,
                ) {
                    TextEditAttempt::Applied => EditStrategy::ProtectedPaste,
                    TextEditAttempt::Cancelled => return,
                    TextEditAttempt::Unsupported | TextEditAttempt::Failed => {
                        self.record_conversion_failure(ConversionFailureReason::ClipboardEdit);
                        return;
                    }
                }
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
                        match attempt {
                            TextEditAttempt::Applied => {
                                self.metrics
                                    .backend_status
                                    .store(BACKEND_PROTECTED_PASTE, Ordering::Release);
                                EditStrategy::ProtectedPaste
                            }
                            TextEditAttempt::Cancelled => {
                                self.metrics
                                    .backend_status
                                    .store(BACKEND_SELECTION_MISMATCH, Ordering::Release);
                                return;
                            }
                            TextEditAttempt::Unsupported => {
                                self.metrics
                                    .backend_status
                                    .store(BACKEND_SELECTION_UNSUPPORTED, Ordering::Release);
                                return;
                            }
                            TextEditAttempt::Failed => {
                                self.record_conversion_failure(
                                    ConversionFailureReason::ClipboardEdit,
                                );
                                return;
                            }
                        }
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

        self.commit_confirmed_layout(target_layout.0 as usize);
        self.session.clear();
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
            ReplayAttempt::Failed => {
                self.record_conversion_failure(ConversionFailureReason::PhysicalEdit);
                return None;
            }
        }
        match self.wait_for_replay_commit(
            pending.foreground,
            input_sequence,
            target_layout.0 as usize,
        ) {
            ReplayAttempt::Applied => Some(EditStrategy::PhysicalReplay),
            ReplayAttempt::Cancelled => {
                self.layout_switch_in_flight = Some((
                    target_layout.0 as usize,
                    Instant::now() + Duration::from_secs(1),
                ));
                self.session.clear();
                None
            }
            ReplayAttempt::Failed => {
                self.record_conversion_failure(ConversionFailureReason::ReplayCommit);
                None
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
                || !foreground_identity_matches(foreground)
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
            if current_foreground_context().is_none_or(|current| current.layout != source_layout) {
                return ReplayAttempt::Cancelled;
            }
            if self.text_edit_backend.requires_text_barrier()
                && matches!(
                    self.privacy_guard
                        .preceding_text_status(foreground.process_id, expected_text),
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
                self.privacy_guard
                    .preceding_text_status(foreground.process_id, expected_text),
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
            match self
                .privacy_guard
                .preceding_text_status(foreground.process_id, expected_text)
            {
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
        self.request_gate_release(pause_sequence);
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
        if !foreground_identity_matches(record.foreground)
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
                match self.perform_protected_paste(
                    record.foreground,
                    pause_sequence,
                    current_layout,
                    source_layout,
                    &record.transaction.forward.insert_text,
                    &record.transaction.undo.insert_text,
                ) {
                    TextEditAttempt::Applied => {}
                    TextEditAttempt::Cancelled => return,
                    TextEditAttempt::Unsupported | TextEditAttempt::Failed => {
                        self.record_conversion_failure(ConversionFailureReason::ClipboardEdit);
                        return;
                    }
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
                    ReplayAttempt::Failed => {
                        self.record_conversion_failure(ConversionFailureReason::PhysicalEdit);
                        return;
                    }
                }
                match self.wait_for_replay_commit(
                    record.foreground,
                    pause_sequence,
                    record.source_layout,
                ) {
                    ReplayAttempt::Applied => {}
                    ReplayAttempt::Cancelled => {
                        self.layout_switch_in_flight = Some((
                            record.source_layout,
                            Instant::now() + Duration::from_secs(1),
                        ));
                        self.session.clear();
                        return;
                    }
                    ReplayAttempt::Failed => {
                        self.record_conversion_failure(ConversionFailureReason::ReplayCommit);
                        return;
                    }
                }
            }
        }

        self.commit_confirmed_layout(record.source_layout);
        self.session.clear();
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

    fn perform_protected_paste(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
        source_layout: usize,
        target_layout: HKL,
        expected_source: &str,
        replacement: &str,
    ) -> TextEditAttempt {
        let started = Instant::now();
        if !self.edit_guard_is_current(foreground, input_sequence) {
            self.diagnostic(
                "paste",
                format!("sequence={input_sequence} phase=guard result=cancelled"),
            );
            return TextEditAttempt::Cancelled;
        }
        let selection = self
            .privacy_guard
            .select_preceding_text(foreground.process_id, expected_source);
        self.diagnostic("paste", format!("sequence={input_sequence} phase=selection result={selection:?} elapsed_ms={} report={}",
            started.elapsed().as_millis(), format_text_barrier_report(self.privacy_guard.last_barrier_report())));
        match selection {
            TextBarrierStatus::Match => {}
            TextBarrierStatus::Mismatch => return TextEditAttempt::Cancelled,
            TextBarrierStatus::Unavailable => return TextEditAttempt::Unsupported,
        }
        if !self.edit_guard_is_current(foreground, input_sequence) {
            self.privacy_guard
                .collapse_selection_to_end(foreground.process_id);
            return TextEditAttempt::Cancelled;
        }

        let Some(mut clipboard) = ProtectedClipboard::begin(replacement) else {
            self.diagnostic(
                "paste",
                format!(
                    "sequence={input_sequence} phase=clipboard_prepare result=failed elapsed_ms={}",
                    started.elapsed().as_millis()
                ),
            );
            self.privacy_guard
                .collapse_selection_to_end(foreground.process_id);
            return TextEditAttempt::Failed;
        };
        let clipboard_current = clipboard.is_current();
        self.diagnostic("paste", format!("sequence={input_sequence} phase=clipboard_prepare result=ready current={clipboard_current} elapsed_ms={}", started.elapsed().as_millis()));
        if !clipboard_current || !self.edit_guard_is_current(foreground, input_sequence) {
            let restored = clipboard.restore();
            self.diagnostic("paste", format!("sequence={input_sequence} phase=before_submit result=cancelled restored={restored}"));
            self.privacy_guard
                .collapse_selection_to_end(foreground.process_id);
            return TextEditAttempt::Cancelled;
        }

        let paste_inputs = build_paste_inputs();
        let submitted = send_input_count(&paste_inputs);
        self.diagnostic("paste", format!("sequence={input_sequence} phase=submit submitted={submitted} requested={} elapsed_ms={}", paste_inputs.len(), started.elapsed().as_millis()));
        if submitted != paste_inputs.len() {
            let control_up = [keyboard_input(VK_LCONTROL, 0, KEYEVENTF_KEYUP)];
            let _ = send_input_batch(&control_up);
            let _ = clipboard.restore();
            self.privacy_guard
                .collapse_selection_to_end(foreground.process_id);
            return TextEditAttempt::Failed;
        }

        let paste_commit =
            self.wait_for_text_commit(foreground, input_sequence, source_layout, replacement);
        self.diagnostic("paste", format!("sequence={input_sequence} phase=text_commit result={paste_commit:?} elapsed_ms={} report={}",
            started.elapsed().as_millis(), format_text_barrier_report(self.privacy_guard.last_barrier_report())));
        let owned_before_restore = clipboard.is_current();
        let clipboard_restored = clipboard.restore();
        self.diagnostic("paste", format!("sequence={input_sequence} phase=clipboard_restore result={clipboard_restored} owned_before={owned_before_restore} elapsed_ms={}", started.elapsed().as_millis()));
        if !matches!(paste_commit, ReplayAttempt::Applied) {
            return TextEditAttempt::Failed;
        }
        let switched = self.switch_layout_and_wait(foreground, target_layout, input_sequence);
        self.diagnostic(
            "paste",
            format!(
                "sequence={input_sequence} phase=layout result={switched:?} elapsed_ms={}",
                started.elapsed().as_millis()
            ),
        );
        match switched {
            ReplayAttempt::Applied => {}
            ReplayAttempt::Cancelled | ReplayAttempt::Failed => return TextEditAttempt::Failed,
        }
        let target_commit = self.wait_for_text_commit(
            foreground,
            input_sequence,
            target_layout.0 as usize,
            replacement,
        );
        self.diagnostic("paste", format!("sequence={input_sequence} phase=target_commit result={target_commit:?} elapsed_ms={} report={}",
            started.elapsed().as_millis(), format_text_barrier_report(self.privacy_guard.last_barrier_report())));
        let target_confirmed = matches!(target_commit, ReplayAttempt::Applied);
        if clipboard_restored && target_confirmed {
            TextEditAttempt::Applied
        } else {
            TextEditAttempt::Failed
        }
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
            result @ (ReplayAttempt::Cancelled | ReplayAttempt::Failed) => return result,
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
        sent.0 != 0 && message_result == 1
    }

    fn handoff_correction_gate(&self, old_token: u64, new_token: u64, required_ms: u64) -> bool {
        self.prepare_gate_handoff_budget(old_token, required_ms)
            && self.send_gate_drain_ack(old_token, new_token)
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
        if !self.send_gate_drain_ack(token, 0) && self.gate_window != 0 {
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

    fn send_gate_drain_ack(&self, old_token: u64, new_token: u64) -> bool {
        if old_token == 0 || self.gate_window == 0 {
            return false;
        }
        let Ok(new_token) = isize::try_from(new_token) else {
            return false;
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
        sent.0 != 0 && message_result == 1
    }

    fn request_gate_release(&mut self, token: u64) {
        if token == 0 || self.gate_window == 0 {
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

    fn edit_guard_is_current(&self, foreground: ForegroundContext, input_sequence: u64) -> bool {
        self.input_guard_is_current(input_sequence) && foreground_identity_matches(foreground)
    }

    fn pre_forward_guard_is_current(
        &self,
        foreground: ForegroundContext,
        input_sequence: u64,
    ) -> bool {
        self.pre_forward_input_guard_is_current(input_sequence)
            && foreground_identity_matches(foreground)
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
        self.clear_undo();
    }

    fn input_sequence_is_current(&self, expected: u64) -> bool {
        self.metrics.observed_input_sequence.load(Ordering::Acquire) == expected
    }

    fn record_conversion_failure(&mut self, reason: ConversionFailureReason) {
        self.diagnostic("conversion", format!("result=failed reason={reason:?}"));
        self.invalidate_conversion_state();
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

fn foreground_identity_matches(expected: ForegroundContext) -> bool {
    current_foreground_context().is_some_and(|current| same_input_target(current, expected))
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
    [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .all(|key| unsafe { GetAsyncKeyState(key.0 as i32) as u16 & 0x8000 == 0 })
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

fn map_replay_key_to_layout(key: ReplayKey, layout: HKL) -> Option<char> {
    let mapping_scan_code = u32::from(key.scan_code) | if key.extended { 0xE000 } else { 0 };
    let virtual_key =
        unsafe { MapVirtualKeyExW(mapping_scan_code, MAPVK_VSC_TO_VK_EX, Some(layout)) };
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

fn mapped_layout_candidates(
    replay_keys: &[ReplayKey],
    current_language: Language,
    settings: &Settings,
    detector: &Detector,
    profiles: Option<&ResolvedKeyboardProfiles>,
) -> Vec<(Language, String)> {
    mapped_layout_candidates_with(current_language, settings, detector, |target_language| {
        let handle = profiles?.unique_layout(detector.input_profile(target_language)?)?;
        let layout = HKL(handle as *mut c_void);
        map_replay_keys_to_layout(replay_keys, layout)
    })
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
    conversion_gate_budget_ms(step_count, true, false, false, true)
        .is_some_and(|total| total <= CORRECTION_GATE_MAX_HOLD_MS)
}

fn conversion_gate_budget_ms(
    step_count: usize,
    requires_text_barrier: bool,
    capability_probe: bool,
    selection_confirmation: bool,
    include_forward_wait: bool,
) -> Option<u64> {
    let mut total = if include_forward_wait {
        HOOK_FORWARD_TIMEOUT_MS
    } else {
        0
    };
    if requires_text_barrier {
        let pacing_ms = u64::try_from(step_count.saturating_sub(1))
            .ok()?
            .checked_mul(NOTEPAD_REPLAY_STEP_DELAY_MS)?;
        total = total
            .checked_add(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS)?
            .checked_add(LAYOUT_SWITCH_TIMEOUT_MS)?
            .checked_add(pacing_ms)?
            .checked_add(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS)?;
        if selection_confirmation {
            total = total.checked_add(UIA_SELECTION_CONFIRM_TIMEOUT_MS)?;
        }
    } else if capability_probe {
        total = total
            .checked_add(INPUT_SETTLE_AFTER_FORWARD_MS)?
            .checked_add(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS)?
            .checked_add(UIA_SELECTION_CONFIRM_TIMEOUT_MS)?
            .checked_add(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS)?
            .checked_add(LAYOUT_SWITCH_TIMEOUT_MS)?
            .checked_add(NOTEPAD_TEXT_BARRIER_TIMEOUT_MS)?;
    } else {
        total = total
            .checked_add(INPUT_SETTLE_AFTER_FORWARD_MS)?
            .checked_add(LAYOUT_SWITCH_TIMEOUT_MS)?
            .checked_add(REPLAY_COMMIT_MS)?;
    }
    total.checked_add(NOTEPAD_REPLAY_GATE_MARGIN_MS)
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

fn build_raw_replay_inputs(held: &[RawKeyEvent], marker: usize) -> Option<Vec<INPUT>> {
    let mut inputs = Vec::with_capacity(held.len() + 2);
    for event in held {
        let mut flags = if event.scan_code == 0 {
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
        let virtual_key = if event.scan_code == 0 {
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
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    set_wide_text(&mut data.szInfoTitle, title.as_ref());
    set_wide_text(&mut data.szInfo, message.as_ref());
    unsafe {
        data.Anonymous.uTimeout = 5_000;
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

const fn privacy_reason_code(reason: Option<PrivacyBlockReason>) -> u8 {
    match reason {
        None => PRIVACY_ALLOWED,
        Some(PrivacyBlockReason::PasswordField) => PRIVACY_PASSWORD,
        Some(PrivacyBlockReason::ExcludedProcess) => PRIVACY_EXCLUDED,
        Some(PrivacyBlockReason::ElevatedProcess) => PRIVACY_ELEVATED,
        Some(PrivacyBlockReason::InspectionUnavailable) => PRIVACY_UNAVAILABLE,
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
    tooltip
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
        unsafe {
            let _ = DeleteObject(HGDIOBJ(color_bitmap.0));
        }
        return Err(Error::from_thread());
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

#[cfg(test)]
fn point_from_packed_word(value: WPARAM) -> POINT {
    let packed = value.0 as u32;
    POINT {
        x: (packed as u16 as i16) as i32,
        y: ((packed >> 16) as u16 as i16) as i32,
    }
}

fn show_tray_menu(hwnd: HWND, interaction_point: Option<POINT>) -> Result<()> {
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
    let result = show_tray_menu_inner(hwnd, interaction_point);
    APP_STATE.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            state.menu_open = false;
        }
    });
    refresh_tray_state();
    result
}

fn show_tray_menu_inner(hwnd: HWND, interaction_point: Option<POINT>) -> Result<()> {
    unsafe {
        let menu: HMENU = CreatePopupMenu()?;
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
            })
        });
        let Some(status) = snapshot else {
            DestroyMenu(menu)?;
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
        let retained_count = APP_STATE.with(|slot| {
            slot.borrow().as_ref().and_then(|state| {
                state
                    .metrics
                    .retained_input
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_ref()
                    .map(|input| input.events.len())
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
            AppendMenuW(
                menu,
                MF_STRING,
                MENU_RECOVER_INPUT_ID,
                &HSTRING::from(tr_format(
                    "menu.recover_input",
                    &[("count", &count.to_string())],
                )),
            )?;
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

        let point = if let Some(point) = interaction_point {
            point
        } else {
            let mut point = POINT::default();
            GetCursorPos(&mut point)?;
            point
        };
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(
            menu,
            ui_localization::popup_menu_style(TPM_RIGHTBUTTON),
            point.x,
            point.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        DestroyMenu(menu)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(state.enqueue_configuration(snapshot));
        assert!(!state.enqueue_configuration(test_reload_snapshot("lost-store.exe")));
        let mut fork = test_reload_snapshot("fork.exe");
        fork.package_source = PackageSource::Managed {
            generation: 2,
            state_sha256: [2; 32],
        };
        assert!(!state.enqueue_configuration(fork));
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        processor.process(receiver.try_recv().unwrap());
        let published = state.publish_acknowledged_configuration().unwrap();
        assert_eq!(published.package_source, source);
        assert_eq!(state.package_source, source);
        assert!(!state.enqueue_configuration(test_reload_snapshot("lost-store.exe")));
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

    // Requires the legacy-bundled RU/ET data; the English-only base variant omits it.
    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn worker_dictionary_snapshot_follows_validated_selection_and_reload() {
        let mut processor = InputProcessor::new(Arc::new(ObserverMetrics::default()), 0);
        assert!(
            processor
                .detector
                .detect("ghbdtn", Language::English)
                .is_some()
        );
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
        assert!(
            processor
                .detector
                .force_mapped_candidates(
                    "ghbdtn",
                    Language::English,
                    &[(Language::Russian, "привет".to_owned())]
                )
                .is_none()
        );
        processor.apply_configuration_reload(Err(std::io::Error::other("invalid configuration")));
        assert!(
            processor
                .detector
                .detect("ghbdtn", Language::English)
                .is_none()
        );
        for selected in [vec!["ru-RU"], vec!["de-DE"], vec!["en", "ru-RU"]] {
            let mut snapshot = test_reload_snapshot("private.exe");
            snapshot.settings.enabled_input_packs = selected
                .into_iter()
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
        }
        processor.apply_configuration_reload(Ok(test_reload_snapshot("private.exe")));
        assert!(
            processor
                .detector
                .detect("ghbdtn", Language::English)
                .is_some()
        );
    }

    #[test]
    fn missing_sources_cannot_replace_active_or_pending_configuration() {
        use autokeyboardlayot::configuration::ConfigurationSources;
        for pending in [false, true] {
            let (mut state, receiver) = test_gate_state(2);
            if pending {
                let mut snapshot = test_reload_snapshot("private.exe");
                snapshot.sources = ConfigurationSources::UNIFIED;
                assert!(state.enqueue_configuration(snapshot));
            } else {
                state.configuration_sources = ConfigurationSources::UNIFIED;
            }
            let previous_revision = state.next_configuration_revision;
            assert!(!state.enqueue_configuration(RuntimeConfiguration::default()));
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
        assert!(state.enqueue_configuration(test_reload_snapshot("new-policy.exe")));
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
        assert!(state.enqueue_configuration(test_reload_snapshot("first.exe")));
        assert!(!state.enqueue_configuration(test_reload_snapshot("second.exe")));
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
        assert!(!state.enqueue_configuration(test_reload_snapshot("new-policy.exe")));
        assert_eq!(state.settings, original);
        assert!(state.pending_configuration.is_none());
        assert!(!state.metrics.configuration_pending.load(Ordering::Acquire));
    }

    #[test]
    fn rapid_reloads_require_latest_ack_and_bound_outstanding_snapshots() {
        let (mut state, receiver) = test_gate_state(4);
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        assert!(state.enqueue_configuration(test_reload_snapshot("first.exe")));
        assert!(state.enqueue_configuration(test_reload_snapshot("second.exe")));
        assert!(!state.enqueue_configuration(test_reload_snapshot("third.exe")));
        let first = receiver.try_recv().unwrap();
        processor.process(first.clone());
        assert!(state.take_acknowledged_configuration().is_none());
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
    fn pending_configuration_passes_hotkeys_and_suppresses_worker_buffering() {
        let (mut state, receiver) = test_gate_state(2);
        let mut processor = InputProcessor::new(Arc::clone(&state.metrics), 0);
        assert!(state.enqueue_configuration(test_reload_snapshot("new-policy.exe")));
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
        assert!(!state.enqueue_configuration(test_reload_snapshot("new-policy.exe")));
        assert!(state.correction_gate.active);
        assert_eq!(state.correction_gate.token, 9);
        assert!(receiver.try_recv().is_err());
        assert!(!state.metrics.configuration_pending.load(Ordering::Acquire));
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
            configuration_loader: None,
            lexicon_candidate: Arc::new(Mutex::new(None)),
            diagnostic_sender: None,
            diagnostic_worker: None,
        };
        (state, receiver)
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
        let retained = state.metrics.retained_input.lock().unwrap();
        assert_eq!(retained.as_ref().unwrap().events.len(), 2);
        assert!(!state.correction_gate.active);
        assert!(!state.metrics.auto_enabled.load(Ordering::Acquire));
        assert_eq!(state.metrics.conversion_failures.load(Ordering::Acquire), 1);
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
        let last = test_raw_key(WM_KEYUP, VIRTUAL_KEY(0x48), 0, 0);
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
        let last = test_raw_key(WM_KEYUP, VIRTUAL_KEY(0x48), 0, 0);
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
        let _open = ClipboardOpenGuard::open(owner)?;
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
        let _open = ClipboardOpenGuard::open(owner)?;
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
    fn tray_glyph_renderer_changes_pixels() {
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

    #[test]
    fn right_click_opens_menu_and_left_click_opens_settings() {
        assert_eq!(tray_interaction(WM_CONTEXTMENU), TrayInteraction::QuickMenu);
        assert_eq!(tray_interaction(NIN_KEYSELECT), TrayInteraction::Settings);
        assert_eq!(tray_interaction(NIN_SELECT), TrayInteraction::Settings);
        assert_eq!(tray_interaction(WM_TIMER), TrayInteraction::Ignore);
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

    #[test]
    fn protected_clipboard_round_trip_when_explicitly_enabled() {
        if std::env::var_os("AUTOKEYBOARDLAYOT_CLIPBOARD_PROBE").is_none() {
            return;
        }
        let _com = PrivacyGuard::new();
        let module = unsafe { GetModuleHandleW(None).expect("module") };
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("AutoKeyboardLayot clipboard probe"),
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
            .expect("probe window")
        };
        let original_formats = test_clipboard_formats(window).expect("clipboard formats");
        let original_text = test_clipboard_unicode(window);

        let mut transaction = ProtectedClipboard::begin("akl-clipboard-probe")
            .expect("protected clipboard transaction");
        assert!(transaction.is_current());
        assert_eq!(
            test_clipboard_unicode(window).as_deref(),
            Some("akl-clipboard-probe")
        );
        let temporary_formats = test_clipboard_formats(window).expect("temporary formats");
        for name in [
            w!("ExcludeClipboardContentFromMonitorProcessing"),
            w!("CanIncludeInClipboardHistory"),
            w!("CanUploadToCloudClipboard"),
        ] {
            let format = unsafe { RegisterClipboardFormatW(name) };
            assert!(format != 0 && temporary_formats.contains(&format));
        }

        assert!(transaction.restore());
        assert_eq!(test_clipboard_formats(window), Some(original_formats));
        assert_eq!(test_clipboard_unicode(window), original_text);
        unsafe {
            DestroyWindow(window).expect("destroy probe window");
        }
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
            conversion_gate_budget_ms(plan.step_ends.len(), true, false, true, false),
            Some(1_187)
        );
        assert_eq!(
            conversion_gate_budget_ms(14, true, false, true, false),
            Some(839)
        );
        assert_eq!(
            conversion_gate_budget_ms(14, false, true, true, false),
            Some(985)
        );

        let mut gate = CorrectionGate::default();
        gate.activate(10);
        gate.awaiting_worker_ack = true;
        assert!(gate.prepare_handoff_budget(10, 839));
        gate.deadline = Instant::now() + Duration::from_millis(838);
        assert!(!gate.handoff(10, 20));

        gate.activate(10);
        gate.awaiting_worker_ack = true;
        assert!(gate.prepare_handoff_budget(10, 789));
        gate.deadline = Instant::now() + Duration::from_millis(850);
        assert!(gate.handoff(10, 20));
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

        assert!(gate.prepare_handoff_budget(10, 100));
        assert!(gate.handoff(10, 20));
        assert!(gate.active);
        assert_eq!(gate.token, 20);
        assert_eq!(gate.fence_marker, GATE_FENCE_MARKER_BASE | 20);
        assert_eq!(gate.held.len(), 1);
        assert_eq!(gate.released_count, 0);
        assert_eq!(gate.current_drain_count, 0);
        assert!(!gate.awaiting_worker_ack);
        assert_eq!(gate.deadline, deadline);
        assert!(!gate.handoff(10, 30));
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
        gate.deadline = Instant::now() + Duration::from_millis(199);
        assert!(!gate.handoff(10, 20));
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
        // The mapped-candidate path needs real installed layouts for the resolved
        // handles; test_profiles() supplies synthetic handles, so only assert on a
        // machine that can actually map the replay keys to the target layout.
        if mapped_layout_candidates(
            &processor.replay_keys,
            Language::English,
            &processor.settings,
            &processor.detector,
            processor.resolved_profiles(),
        )
        .is_empty()
        {
            return;
        }
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
        let metrics = Arc::new(ObserverMetrics::default());
        assert!(!metrics.auto_enabled.load(Ordering::Acquire));
        metrics.auto_enabled.store(true, Ordering::Release);
        assert!(metrics.auto_enabled.load(Ordering::Acquire));
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
}
