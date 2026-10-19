//! Acceptance of the settings window on the desktop of the test VM: the real window, driven through
//! UI Automation and input as a user drives it. Each test opens the window once, in a profile of
//! its own, so it must run alone by name in an interactive session; other runs skip
//! `on_the_desktop`. Some tests download published packages and need the network.

use super::*;
use autokeyboardlayot::{
    language_package::PackageTrust, package_install::PreparedCatalog, package_store::PackageStore,
};
use std::{
    path::PathBuf,
    sync::atomic::AtomicBool,
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::{
        BI_RGB, BITMAPFILEHEADER, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen,
        CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        GetDIBits, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
    },
    System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
    UI::{
        Accessibility::{
            CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
            IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
            IUIAutomationValuePattern, ToggleState_On, UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID,
            UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId, UIA_EditControlTypeId,
            UIA_InvokePatternId, UIA_SelectionItemPatternId, UIA_TextControlTypeId,
            UIA_TogglePatternId, UIA_ValuePatternId,
        },
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, IsWindowEnabled, KEYBDINPUT,
            KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MOUSEEVENTF_ABSOLUTE,
            MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
            MOUSEEVENTF_WHEEL, MOUSEINPUT, MapVirtualKeyW, VIRTUAL_KEY, VK_A, VK_CONTROL, VK_SPACE,
        },
        WindowsAndMessaging::{
            EnumWindows, GW_OWNER, GetClassNameW, GetClientRect, GetDlgItem, GetPropW,
            GetSystemMetrics, GetWindow, IsWindowVisible, IsZoomed, SM_CXSCREEN,
            SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN, SW_MAXIMIZE, SendMessageW, ShowWindow, WM_CLOSE, WM_COMMAND,
            WM_GETTEXT, WM_SETTEXT,
        },
    },
};

const LEGACY: &str =
    "schema_version=4\n\n[packages]\nmode=legacy\n\n[settings]\nenabled_input_packs=en-US\n";

fn managed_configuration(ui_language: &str) -> String {
    format!(
        "schema_version=4\n\n[packages]\nmode=managed\n\n[settings]\nui_language={ui_language}\n\
         enabled_input_packs=en-US\n"
    )
}

/// A profile of its own for one window.
struct Profile {
    root: PathBuf,
}

impl Profile {
    fn new(configuration: &str, managed: bool) -> Self {
        let root =
            std::env::temp_dir().join(format!("akl-settings-acceptance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let profile = Self { root };
        std::fs::create_dir_all(profile.directory()).expect("profile");
        std::fs::write(profile.directory().join("config.ini"), configuration).expect("config");
        if managed {
            PackageStore::initialize(&profile.directory().join("packages")).expect("store");
        }
        profile
    }

    fn legacy() -> Self {
        Self::new(LEGACY, false)
    }

    fn managed(ui_language: &str) -> Self {
        Self::new(&managed_configuration(ui_language), true)
    }

    fn directory(&self) -> PathBuf {
        self.root.join("AutoKeyboardLayot")
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

/// The published catalog, checked through a store at `packages`.
fn published_catalog(store: &PackageStore, trust: &PackageTrust) -> PreparedCatalog {
    let snapshot = store.load(trust).expect("snapshot");
    let repository = autokeyboardlayot::package_catalog::resolve_repository_source(
        "",
        snapshot.inventory().repository(),
    )
    .expect("repository");
    PreparedCatalog::from_github(store, &repository, trust, &AtomicBool::new(false))
        .expect("published catalog")
}

/// Installs the published packages `ids` into the profile's store, as the Add language dialog does.
fn install_published(profile: &Profile, ids: &[&str]) {
    let trust = PackageTrust::release().expect("trust");
    let store = PackageStore::open(&profile.directory().join("packages")).expect("store");
    let catalog = published_catalog(&store, &trust);
    let ids: BTreeSet<_> = ids
        .iter()
        .map(|id| PackId::parse(id).expect("id"))
        .collect();
    let cancel = AtomicBool::new(false);
    let prepared = catalog
        .select(&ids, &trust, unix_now())
        .expect("selection")
        .accept_and_download_with_progress(
            &store,
            &trust,
            &cancel,
            || Ok(unix_now()),
            |pin| autokeyboardlayot::package_download::download(pin, &trust, &cancel),
            |_, _| {},
        )
        .expect("download");
    prepared
        .confirm(&store, &trust, unix_now())
        .expect("installation");
}

/// Downloads the published packages `ids` as files into `directory`; returns their names.
fn download_published(directory: &Path, ids: &[PackId]) -> Vec<String> {
    let trust = PackageTrust::release().expect("trust");
    let scratch = tempfile::tempdir().expect("scratch store");
    let store = PackageStore::initialize(&scratch.path().join("packages")).expect("store");
    let catalog = published_catalog(&store, &trust);
    let cancel = AtomicBool::new(false);
    let mut names = Vec::new();
    for pin in catalog.packages().filter(|pin| ids.contains(&pin.id())) {
        let package = autokeyboardlayot::package_download::download(pin, &trust, &cancel)
            .expect("package download");
        let name = format!("{}.aklp", pin.id().as_str());
        std::fs::write(directory.join(&name), package.bytes()).expect("package file");
        names.push(name);
    }
    assert_eq!(names.len(), ids.len(), "not every package is published");
    names
}

/// The settings window of this process, run from `profile` on a thread of its own.
struct Window {
    handle: HWND,
    thread: JoinHandle<Result<(), String>>,
    _profile: Profile,
}

impl Window {
    fn open(profile: Profile) -> Self {
        // Run alone by its name, this is the only test in the process, and the window reads the
        // variable after it is set.
        unsafe {
            std::env::set_var("LOCALAPPDATA", &profile.root);
        }
        let thread = thread::spawn(run);
        let handle = within(Duration::from_secs(60), own_settings_window)
            .expect("the settings window did not open");
        // The window exists before its first frame. Its event loop runs once the graceful-close
        // property appears; with software rendering on a busy machine that took 34 s.
        within(Duration::from_secs(120), || {
            graceful_close_advertised(handle).then_some(())
        })
        .expect("the settings window's event loop did not start");
        let window = Self {
            handle,
            thread,
            _profile: profile,
        };
        // The test VM's screen is 800 px high, its taskbar 48 px: the whole window fits at the top.
        window.place(1_080, 740);
        // The first frames take long with software rendering.
        thread::sleep(Duration::from_secs(4));
        window
    }

    fn place(&self, width: i32, height: i32) {
        use windows::Win32::UI::WindowsAndMessaging::{
            HWND_TOPMOST, SET_WINDOW_POS_FLAGS, SetWindowPos,
        };
        // Above the console that runs the test, so that clicks and screenshots reach the window.
        unsafe {
            let _ = SetWindowPos(
                self.handle,
                Some(HWND_TOPMOST),
                40,
                0,
                width,
                height,
                SET_WINDOW_POS_FLAGS(0),
            );
        }
        thread::sleep(Duration::from_millis(500));
    }

    /// A close request as the installer and the title bar send it.
    fn request_close(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.handle), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    fn closed_within(&self, timeout: Duration) -> bool {
        within(timeout, || self.thread.is_finished().then_some(())).is_some()
    }

    /// The window's client area in screen coordinates.
    fn client(&self) -> RECT {
        let mut client = RECT::default();
        let mut origin = POINT::default();
        unsafe {
            GetClientRect(self.handle, &mut client).expect("client rectangle");
            let _ = ClientToScreen(self.handle, &mut origin);
        }
        RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + client.right,
            bottom: origin.y + client.bottom,
        }
    }
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

/// An element that UI Automation shows in the window.
#[derive(Clone)]
struct Element {
    element: IUIAutomationElement,
    name: String,
    kind: i32,
    rect: RECT,
}

impl Element {
    fn center(&self) -> POINT {
        POINT {
            x: (self.rect.left + self.rect.right) / 2,
            y: (self.rect.top + self.rect.bottom) / 2,
        }
    }

    fn width(&self) -> i32 {
        self.rect.right - self.rect.left
    }

    fn height(&self) -> i32 {
        self.rect.bottom - self.rect.top
    }

    fn is(&self, kind: UIA_CONTROLTYPE_ID) -> bool {
        self.kind == kind.0
    }

    /// Whether it is checked or selected, for elements that can be.
    fn checked(&self) -> Option<bool> {
        unsafe {
            if let Ok(toggle) = self
                .element
                .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            {
                return toggle
                    .CurrentToggleState()
                    .ok()
                    .map(|state| state == ToggleState_On);
            }
            self.element
                .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                    UIA_SelectionItemPatternId,
                )
                .ok()
                .and_then(|item| item.CurrentIsSelected().ok())
                .map(|selected| selected.as_bool())
        }
    }

    fn invoke(&self) {
        unsafe {
            self.element
                .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                .and_then(|pattern| pattern.Invoke())
                .unwrap_or_else(|error| panic!("invoke {}: {error}", self.name));
        }
    }

    fn value(&self) -> String {
        unsafe {
            self.element
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .and_then(|pattern| pattern.CurrentValue())
                .map(|value| value.to_string())
                .unwrap_or_default()
        }
    }

    fn focus(&self) {
        unsafe {
            self.element
                .SetFocus()
                .unwrap_or_else(|error| panic!("focus {}: {error}", self.name));
        }
    }
}

struct Automation {
    automation: IUIAutomation,
    walker: IUIAutomationTreeWalker,
}

impl Automation {
    fn new() -> Self {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).expect("automation");
            let walker = automation.ControlViewWalker().expect("walker");
            Self { automation, walker }
        }
    }

    /// Every element of the window's control view.
    fn elements(&self, window: HWND) -> Vec<Element> {
        let mut found = Vec::new();
        let Ok(root) = (unsafe { self.automation.ElementFromHandle(window) }) else {
            return found;
        };
        let mut stack = vec![(root, 0)];
        while let Some((element, depth)) = stack.pop() {
            if found.len() > 5_000 {
                break;
            }
            unsafe {
                let name = element
                    .CurrentName()
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                let kind = element.CurrentControlType().map_or(0, |kind| kind.0);
                let rect = element.CurrentBoundingRectangle().unwrap_or_default();
                if depth < 40 {
                    let mut child = self.walker.GetFirstChildElement(&element).ok();
                    while let Some(current) = child {
                        child = self.walker.GetNextSiblingElement(&current).ok();
                        stack.push((current, depth + 1));
                    }
                }
                found.push(Element {
                    element,
                    name,
                    kind,
                    rect,
                });
            }
        }
        found
    }

    fn find(&self, window: HWND, kind: UIA_CONTROLTYPE_ID, name: &str) -> Option<Element> {
        self.elements(window)
            .into_iter()
            .find(|element| element.is(kind) && element.name == name)
    }

    fn wait(
        &self,
        window: HWND,
        kind: UIA_CONTROLTYPE_ID,
        name: &str,
        timeout: Duration,
    ) -> Option<Element> {
        within(timeout, || self.find(window, kind, name))
    }

    /// The elements named `name` that can be checked or selected.
    fn choices(&self, window: HWND, name: &str) -> Vec<Element> {
        self.elements(window)
            .into_iter()
            .filter(|element| element.name == name && element.checked().is_some())
            .collect()
    }

    fn texts(&self, window: HWND) -> Vec<Element> {
        self.elements(window)
            .into_iter()
            .filter(|element| element.is(UIA_TextControlTypeId))
            .collect()
    }
}

fn send(inputs: &[INPUT]) {
    assert!(super::super::send_input_batch(inputs), "send input");
}

fn mouse(
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    point: POINT,
    data: u32,
) -> INPUT {
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let scale = |value: i32, origin: i32, size: i32| (value - origin) * 65_535 / (size - 1).max(1);
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: scale(point.x, left, width),
                dy: scale(point.y, top, height),
                mouseData: data,
                dwFlags: flags | MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                ..Default::default()
            },
        },
    }
}

fn click(point: POINT) {
    send(&[mouse(Default::default(), point, 0)]);
    thread::sleep(Duration::from_millis(80));
    send(&[mouse(MOUSEEVENTF_LEFTDOWN, point, 0)]);
    thread::sleep(Duration::from_millis(60));
    send(&[mouse(MOUSEEVENTF_LEFTUP, point, 0)]);
    thread::sleep(Duration::from_millis(400));
}

/// Turns the wheel at `point`: negative notches scroll down.
fn wheel(point: POINT, notches: i32) {
    send(&[mouse(MOUSEEVENTF_WHEEL, point, (notches * 120) as u32)]);
    thread::sleep(Duration::from_millis(300));
}

/// Key edges in order, as the window's focused control receives them.
fn keys(edges: &[(VIRTUAL_KEY, bool)]) {
    for &(key, down) in edges {
        let scan = unsafe { MapVirtualKeyW(u32::from(key.0), MAPVK_VK_TO_VSC) } as u16;
        let flags = if down {
            Default::default()
        } else {
            KEYEVENTF_KEYUP
        };
        send(&[super::super::keyboard_input_with_marker(
            key, scan, flags, 0,
        )]);
        thread::sleep(Duration::from_millis(60));
    }
    thread::sleep(Duration::from_millis(300));
}

fn type_text(text: &str) {
    for unit in text.encode_utf16() {
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            send(&[INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wScan: unit,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            }]);
        }
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(300));
}

/// The whole screen as top-down 0xAARRGGBB pixels, with its width; also saved as `name`.
fn screenshot(name: &str) -> (Vec<u32>, i32) {
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
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u32; (width * height) as usize];
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
        let bytes = pixels.len() as u32 * 4;
        let file = BITMAPFILEHEADER {
            bfType: 0x4D42,
            bfSize: offset + bytes,
            bfOffBits: offset,
            ..Default::default()
        };
        let mut data = Vec::with_capacity((offset + bytes) as usize);
        data.extend_from_slice(core::slice::from_raw_parts(
            (&raw const file).cast::<u8>(),
            size_of::<BITMAPFILEHEADER>(),
        ));
        data.extend_from_slice(core::slice::from_raw_parts(
            (&raw const header.bmiHeader).cast::<u8>(),
            size_of::<BITMAPINFOHEADER>(),
        ));
        data.extend_from_slice(core::slice::from_raw_parts(
            pixels.as_ptr().cast::<u8>(),
            pixels.len() * 4,
        ));
        // Beside the test program: an open-file dialog moves the current directory.
        let path = std::env::current_exe()
            .expect("test program")
            .with_file_name(name);
        let _ = std::fs::write(path, data);
        (pixels, width)
    }
}

fn pixel(screen: &(Vec<u32>, i32), point: POINT) -> u32 {
    screen.0[(point.y * screen.1 + point.x) as usize] & 0x00ff_ffff
}

fn close_colors(left: u32, right: u32) -> bool {
    (0..3).all(|shift| {
        let channel = |value: u32| ((value >> (shift * 8)) & 0xff) as i32;
        (channel(left) - channel(right)).abs() <= 24
    })
}

/// The visible top-level dialog windows (class `#32770`) of this process.
fn own_dialogs() -> Vec<HWND> {
    unsafe extern "system" fn collect(window: HWND, list: LPARAM) -> windows::core::BOOL {
        let list = unsafe { &mut *(list.0 as *mut Vec<HWND>) };
        let mut class = [0u16; 32];
        let length = unsafe { GetClassNameW(window, &mut class) }.max(0) as usize;
        let mut process_id = 0;
        unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
        if String::from_utf16_lossy(&class[..length]) == "#32770"
            && process_id == std::process::id()
            && unsafe { IsWindowVisible(window) }.as_bool()
        {
            list.push(window);
        }
        true.into()
    }
    let mut list: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

/// Chooses `names` in the folder `directory` in an open-file dialog and opens them.
fn choose_in_dialog(dialog: HWND, directory: &Path, names: &[String]) {
    const OPEN: i32 = 1;
    // The file-name box of the Explorer-style dialog takes a folder first, then the files in it.
    let edit = file_name_box(dialog).expect("file name box");
    let set = |text: &str| {
        let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
        unsafe {
            SendMessageW(
                edit,
                WM_SETTEXT,
                Some(WPARAM(0)),
                Some(LPARAM(wide.as_ptr() as isize)),
            );
        }
    };
    let open = || unsafe {
        let _ = PostMessageW(Some(dialog), WM_COMMAND, WPARAM(OPEN as usize), LPARAM(0));
        thread::sleep(Duration::from_millis(1_500));
    };
    let text = || {
        let mut buffer = [0u16; 1_024];
        let length = unsafe {
            SendMessageW(
                edit,
                WM_GETTEXT,
                Some(WPARAM(buffer.len())),
                Some(LPARAM(buffer.as_mut_ptr() as isize)),
            )
        }
        .0
        .clamp(0, buffer.len() as isize) as usize;
        String::from_utf16_lossy(&buffer[..length])
    };
    set(&directory.to_string_lossy());
    open();
    // The dialog empties the box once it shows the folder, which takes long on a busy machine.
    let _ = within(Duration::from_secs(30), || text().is_empty().then_some(()));
    let list = names
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(" ");
    set(&list);
    open();
}

/// The edit box of an open-file dialog where a file name is typed.
fn file_name_box(dialog: HWND) -> Option<HWND> {
    unsafe extern "system" fn collect(window: HWND, list: LPARAM) -> windows::core::BOOL {
        let list = unsafe { &mut *(list.0 as *mut Vec<HWND>) };
        let mut class = [0u16; 32];
        let length = unsafe { GetClassNameW(window, &mut class) }.max(0) as usize;
        if String::from_utf16_lossy(&class[..length]) == "Edit" {
            list.push(window);
        }
        true.into()
    }
    // The combo box with id 0x47c (cmb13) holds the file-name edit.
    let combo = unsafe { GetDlgItem(Some(dialog), 0x47c) }.ok()?;
    let mut edits: Vec<HWND> = Vec::new();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
            Some(combo),
            Some(collect),
            LPARAM(&mut edits as *mut _ as isize),
        );
    }
    edits.into_iter().next()
}

fn graceful_close_advertised(window: HWND) -> bool {
    unsafe { GetPropW(window, w!("AutoKeyboardLayot.GracefulClose.v1")) }.0 as usize == 1
}

/// Shows the navigation page named `page`.
fn show_page(ui: &Automation, window: &Window, page: &str) {
    // A click can miss while the window is still drawn for the first time: click until the page
    // is the shown one.
    let shown = within(Duration::from_secs(15), || {
        let item = ui.find(window.handle, UIA_ButtonControlTypeId, page)?;
        if item.checked() == Some(true) {
            return Some(());
        }
        click(item.center());
        thread::sleep(Duration::from_secs(1));
        None
    });
    if shown.is_none() {
        let _ = screenshot("settings-page-failure.bmp");
        panic!("no page {page}");
    }
    thread::sleep(Duration::from_secs(1));
}

/// Scrolls the page until `name` shows up, and returns it.
fn scroll_to(ui: &Automation, window: &Window, kind: UIA_CONTROLTYPE_ID, name: &str) -> Element {
    let client = window.client();
    let content = POINT {
        x: (client.left + client.right) / 2 + 120,
        y: (client.top + client.bottom) / 2,
    };
    for _ in 0..12 {
        if let Some(element) = ui.find(window.handle, kind, name)
            && element.rect.bottom < client.bottom - 70
        {
            return element;
        }
        wheel(content, -3);
    }
    panic!("{name} did not show up")
}

/// Scrolls the page with the pointer at `x`, beside the cards' content where no text box takes the
/// wheel, until `name` shows up within `timeout`.
fn scroll_until(
    ui: &Automation,
    window: &Window,
    x: i32,
    kind: UIA_CONTROLTYPE_ID,
    name: &str,
    timeout: Duration,
) -> Option<Element> {
    let client = window.client();
    let point = POINT {
        x,
        y: (client.top + client.bottom) / 2,
    };
    within(timeout, || {
        let element = ui
            .find(window.handle, kind, name)
            .filter(|element| element.rect.bottom < client.bottom - 70);
        if element.is_none() {
            wheel(point, -3);
            thread::sleep(Duration::from_millis(400));
        }
        element
    })
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_advertises_the_graceful_close_once_it_exists() {
    let window = Window::open(Profile::legacy());
    let advertised = within(Duration::from_secs(10), || {
        graceful_close_advertised(window.handle).then_some(())
    });
    // The installer's close, with nothing changed: granted.
    window.request_close();
    let closed = window.closed_within(Duration::from_secs(30));
    eprintln!("advertised={} closed={closed}", advertised.is_some());
    assert!(advertised.is_some(), "the graceful close is not advertised");
    assert!(closed, "the window did not close");
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_shows_a_long_message_in_full() {
    let window = Window::open(Profile::legacy());
    // At the narrowest width the close guard's message needs two lines.
    window.place(900, 740);
    let ui = Automation::new();
    let auto_start = ui
        .choices(window.handle, &tr("general.auto_start"))
        .into_iter()
        .next()
        .expect("the automatic conversion box");
    click(auto_start.center());
    // A close with an unsaved change is refused with a message that ends with its remedy.
    window.request_close();
    let unsaved = tr("lifecycle.settings_unsaved");
    let message = ui.wait(
        window.handle,
        UIA_TextControlTypeId,
        &unsaved,
        Duration::from_secs(10),
    );
    let _ = screenshot("settings-message.bmp");
    let still_open = !window.closed_within(Duration::from_secs(2));
    let client = window.client();
    // The page area right of the 252 px navigation, without its 30 px paddings.
    let content_width = client.right - client.left - 252 - 60;
    let shown = message
        .as_ref()
        .map(|message| (message.width(), message.height()));
    eprintln!("message={shown:?} content_width={content_width} still_open={still_open}");
    // Undone, the change no longer holds the window.
    click(auto_start.center());
    window.request_close();
    let closed = window.closed_within(Duration::from_secs(30));
    let (width, height) = shown.expect("no message");
    assert!(still_open, "a close with an unsaved change was granted");
    assert!(
        width >= content_width - 8,
        "the message does not span the window: {width} of {content_width}"
    );
    // Two lines of the 14 px text.
    assert!(height >= 26, "the message is not wrapped: {height} px high");
    assert!(closed, "the window did not close without changes");
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_shows_a_refused_save_in_full() {
    let profile = Profile::legacy();
    let configuration = profile.directory().join("config.ini");
    let window = Window::open(profile);
    window.place(900, 740);
    let ui = Automation::new();
    let english = ui
        .choices(window.handle, "English")
        .into_iter()
        .next()
        .expect("English");
    click(english.center());
    // Another writer chooses a third interface language meanwhile, so Apply cannot save the choice.
    std::fs::write(&configuration, format!("{LEGACY}ui_language=et\n")).expect("outside change");
    ui.wait(
        window.handle,
        UIA_ButtonControlTypeId,
        &tr("button.apply"),
        Duration::from_secs(5),
    )
    .expect("Apply")
    .invoke();
    let template = tr("settings.merge_conflict");
    let prefix = template.split("{fields}").next().unwrap_or_default();
    let message = within(Duration::from_secs(10), || {
        ui.texts(window.handle)
            .into_iter()
            .find(|text| text.name.starts_with(prefix))
    });
    let _ = screenshot("settings-refused-save.bmp");
    let client = window.client();
    // The page area right of the 252 px navigation, without its 30 px paddings.
    let content_width = client.right - client.left - 252 - 60;
    let shown = message
        .as_ref()
        .map(|message| (message.width(), message.height(), message.name.clone()));
    eprintln!("message={shown:?} content_width={content_width}");
    // Cancel discards the choice the window still shows.
    if let Some(cancel) = ui.find(window.handle, UIA_ButtonControlTypeId, &tr("button.cancel")) {
        cancel.invoke();
    }
    let closed = window.closed_within(Duration::from_secs(30));
    let (width, height, _) = shown.expect("no message about the refused save");
    assert!(
        width >= content_width - 8,
        "the message does not span the window: {width} of {content_width}"
    );
    // Two lines of the 14 px text.
    assert!(height >= 26, "the message is not wrapped: {height} px high");
    assert!(closed, "the window did not close on Cancel");
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_keeps_one_interface_language_marked() {
    let window = Window::open(Profile::legacy());
    let ui = Automation::new();
    let system = tr("ui_language.system");
    let english = "English";
    let first = |name: &str| ui.choices(window.handle, name).into_iter().next();
    let english_row = first(english).expect("English");
    click(english_row.center());
    let system_row = first(&system).expect("system language");
    click(system_row.center());
    let marked: Vec<String> = [system.as_str(), english]
        .into_iter()
        .filter(|name| first(name).and_then(|row| row.checked()) == Some(true))
        .map(str::to_owned)
        .collect();
    let _ = screenshot("settings-ui-language.bmp");
    eprintln!("marked={marked:?}");
    window.request_close();
    let closed = window.closed_within(Duration::from_secs(30));
    assert_eq!(
        marked,
        [system],
        "the marks do not show the chosen language"
    );
    assert!(closed, "the window did not close");
}

/// Opens the Add language dialog of a managed profile and waits for the published catalog.
fn open_add_language(ui: &Automation, window: &Window) {
    show_page(ui, window, &tr("nav.languages"));
    let add = ui
        .wait(
            window.handle,
            UIA_ButtonControlTypeId,
            &tr("languages.add_button"),
            Duration::from_secs(10),
        )
        .expect("Add language");
    add.invoke();
    within(Duration::from_secs(90), || {
        (!ui.choices(window.handle, "ar-sa").is_empty()).then_some(())
    })
    .expect("the published catalog did not load");
}

/// The dialog's box: centered, at most 700 × 600 px and 30 px from the window's edges.
fn dialog_box(window: &Window) -> RECT {
    let client = window.client();
    let (width, height) = (client.right - client.left, client.bottom - client.top);
    let (box_width, box_height) = ((width - 60).min(700), (height - 60).min(600));
    RECT {
        left: client.left + (width - box_width) / 2,
        top: client.top + (height - box_height) / 2,
        right: client.left + (width + box_width) / 2,
        bottom: client.top + (height + box_height) / 2,
    }
}

/// Scrolls the dialog's list until the element `name` of `kind` shows up, and returns it.
fn scroll_dialog_to(
    ui: &Automation,
    window: &Window,
    kind: UIA_CONTROLTYPE_ID,
    name: &str,
) -> Element {
    let dialog = dialog_box(window);
    let list = POINT {
        x: (dialog.left + dialog.right) / 2,
        y: (dialog.top + dialog.bottom) / 2,
    };
    for _ in 0..20 {
        if let Some(element) = ui.find(window.handle, kind, name)
            && element.rect.bottom < dialog.bottom - 50
        {
            return element;
        }
        wheel(list, -3);
    }
    panic!("{name} did not show up in the dialog")
}

/// A review's package picker at its own height, above its preview and clear of it.
fn apart(picker: &Element, preview: &Element) -> bool {
    picker.height() <= 40 && picker.rect.bottom <= preview.rect.top
}

fn inside(rect: &RECT, area: &RECT) -> bool {
    rect.left >= area.left - 2
        && rect.right <= area.right + 2
        && rect.top >= area.top - 2
        && rect.bottom <= area.bottom + 2
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_shows_each_package_license_in_the_online_review() {
    let window = Window::open(Profile::managed("system"));
    let ui = Automation::new();
    open_add_language(&ui, &window);
    let chosen = ["ar-sa", "ur-pk"];
    for id in chosen {
        let row = scroll_dialog_to(&ui, &window, UIA_CheckBoxControlTypeId, id);
        click(row.center());
    }
    scroll_dialog_to(&ui, &window, UIA_ButtonControlTypeId, &tr("download.start")).invoke();
    let review = |ui: &Automation| {
        ui.find(
            window.handle,
            UIA_EditControlTypeId,
            &tr("download.confirm"),
        )
        .map(|preview| preview.value())
    };
    let first = within(Duration::from_secs(120), || review(&ui)).expect("no review");
    let picker = ui.elements(window.handle).into_iter().find(|element| {
        element.is(UIA_ComboBoxControlTypeId) && inside(&element.rect, &dialog_box(&window))
    });
    let preview = ui.find(
        window.handle,
        UIA_EditControlTypeId,
        &tr("download.confirm"),
    );
    let picker_apart = picker
        .as_ref()
        .zip(preview.as_ref())
        .is_some_and(|(picker, preview)| apart(picker, preview));
    let mut second = None;
    if let Some(picker) = &picker {
        click(picker.center());
        if let Some(item) = within(Duration::from_secs(5), || {
            ui.elements(window.handle)
                .into_iter()
                .find(|element| element.name.starts_with(chosen[1]))
        }) {
            click(item.center());
        }
        second = within(Duration::from_secs(5), || {
            review(&ui).filter(|preview| preview.contains(chosen[1]))
        });
    }
    let _ = screenshot("settings-online-review.bmp");
    eprintln!(
        "first_shows={} picker={:?} preview={:?} second_shows={}",
        first.contains(chosen[0]),
        picker.as_ref().map(|picker| picker.rect),
        preview.as_ref().map(|preview| preview.rect),
        second.is_some()
    );
    // Leave without installing.
    let cancel = ui
        .elements(window.handle)
        .into_iter()
        .find(|element| {
            element.is(UIA_ButtonControlTypeId)
                && element.name == tr("button.cancel")
                && inside(&element.rect, &dialog_box(&window))
        })
        .expect("the review's Cancel");
    cancel.invoke();
    assert!(
        first.contains(chosen[0]),
        "the review does not show {}",
        chosen[0]
    );
    assert!(picker.is_some(), "the review has no package picker");
    assert!(
        picker_apart,
        "the review's package picker is not apart from its preview"
    );
    assert!(second.is_some(), "the review never shows {}", chosen[1]);
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_shows_the_add_language_failures_in_the_dialog() {
    let window = Window::open(Profile::managed("system"));
    let ui = Automation::new();
    open_add_language(&ui, &window);
    scroll_dialog_to(
        &ui,
        &window,
        UIA_ButtonControlTypeId,
        &tr("download.discard"),
    )
    .invoke();
    let repository = ui
        .wait(
            window.handle,
            UIA_EditControlTypeId,
            &tr("download.repository"),
            Duration::from_secs(10),
        )
        .expect("the repository field");
    click(repository.center());
    keys(&[
        (VK_CONTROL, true),
        (VK_A, true),
        (VK_A, false),
        (VK_CONTROL, false),
    ]);
    type_text("example/does-not-exist");
    ui.find(
        window.handle,
        UIA_ButtonControlTypeId,
        &tr("download.check"),
    )
    .expect("Check")
    .invoke();
    let failure = tr_format("download.failed", &[("code", "")]);
    let prefix = failure.split(':').next().expect("prefix").to_owned();
    let messages = within(Duration::from_secs(60), || {
        let found: Vec<_> = ui
            .texts(window.handle)
            .into_iter()
            .filter(|text| text.name.starts_with(&prefix))
            .collect();
        (!found.is_empty()).then_some(found)
    })
    .expect("no failure message");
    let dialog = dialog_box(&window);
    let in_dialog = messages
        .iter()
        .any(|message| inside(&message.rect, &dialog));
    let _ = screenshot("settings-dialog-failure.bmp");
    eprintln!(
        "messages={:?} dialog={dialog:?}",
        messages
            .iter()
            .map(|message| message.rect)
            .collect::<Vec<_>>()
    );
    assert!(in_dialog, "the failure is not shown in the dialog");
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_mirrors_the_language_rows_in_arabic() {
    let profile = Profile::managed("ar");
    install_published(&profile, &["ar-SA"]);
    let window = Window::open(profile);
    let ui = Automation::new();
    assert!(
        super::super::ui_localization::is_rtl(),
        "the interface is not right-to-left"
    );
    show_page(&ui, &window, &tr("nav.languages"));
    let row = ui
        .wait(
            window.handle,
            UIA_CheckBoxControlTypeId,
            "English",
            Duration::from_secs(10),
        )
        .expect("the English row");
    let elements = ui.elements(window.handle);
    let picker = elements
        .iter()
        .find(|element| {
            element.is(UIA_ComboBoxControlTypeId)
                && element.rect.top >= row.rect.top - 2
                && element.rect.bottom <= row.rect.bottom + 2
        })
        .cloned();
    // The navigation item of the shown page carries the accent color, as a ticked mark does.
    let page = elements
        .iter()
        .find(|element| element.is(UIA_ButtonControlTypeId) && element.name == tr("nav.languages"))
        .expect("the page's navigation item")
        .clone();
    let screen = screenshot("settings-arabic-rows.bmp");
    let accent = pixel(
        &screen,
        POINT {
            x: page.rect.left + 6,
            y: (page.rect.top + page.rect.bottom) / 2,
        },
    );
    // A mirrored row has its 20 px mark 12 px from its right edge; an unmirrored one at the left
    // edge, under the profile picker.
    let accent_pixels = |left: i32, right: i32| {
        (row.rect.top..row.rect.bottom)
            .flat_map(|y| (left..right).map(move |x| POINT { x, y }))
            .filter(|point| close_colors(pixel(&screen, *point), accent))
            .count()
    };
    let right_end = accent_pixels(row.rect.right - 40, row.rect.right);
    let left_end = accent_pixels(row.rect.left, row.rect.left + 40);
    eprintln!(
        "row={:?} picker={:?} accent={accent:06x} accent pixels: right end {right_end}, left end {left_end}",
        row.rect,
        picker.as_ref().map(|picker| picker.rect)
    );
    window.request_close();
    if let Some(picker) = &picker {
        assert!(
            picker.rect.left - row.rect.left < 40,
            "the profile picker is not at the row's left edge"
        );
    }
    assert!(
        right_end >= 200,
        "no ticked mark at the row's right edge: {right_end} accent pixels"
    );
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_starts_recording_with_space() {
    let window = Window::open(Profile::legacy());
    let ui = Automation::new();
    let record = scroll_to(
        &ui,
        &window,
        UIA_ButtonControlTypeId,
        &tr("hotkey.record_button"),
    );
    record.focus();
    thread::sleep(Duration::from_millis(500));
    keys(&[(VK_SPACE, true)]);
    thread::sleep(Duration::from_millis(300));
    keys(&[(VK_SPACE, false)]);
    let recording = ui.wait(
        window.handle,
        UIA_TextControlTypeId,
        &tr("hotkey.press"),
        Duration::from_secs(5),
    );
    let refused = ui
        .find(
            window.handle,
            UIA_TextControlTypeId,
            &tr("hotkey.release_retry"),
        )
        .is_some();
    let _ = screenshot("settings-record-space.bmp");
    eprintln!("recording={} refused={refused}", recording.is_some());
    if let Some(cancel) = ui.find(
        window.handle,
        UIA_ButtonControlTypeId,
        &tr("hotkey.cancel_button"),
    ) {
        cancel.invoke();
    }
    window.request_close();
    assert!(recording.is_some(), "Space did not start the recording");
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_refuses_shortcuts_that_type() {
    let window = Window::open(Profile::legacy());
    let ui = Automation::new();
    let record = scroll_to(
        &ui,
        &window,
        UIA_ButtonControlTypeId,
        &tr("hotkey.record_button"),
    );
    record.focus();
    thread::sleep(Duration::from_millis(500));
    keys(&[(VK_SPACE, true)]);
    thread::sleep(Duration::from_millis(300));
    keys(&[(VK_SPACE, false)]);
    let pressing = tr("hotkey.press");
    let recording = ui.wait(
        window.handle,
        UIA_TextControlTypeId,
        &pressing,
        Duration::from_secs(5),
    );
    // The capture ignores injected keys, as it must: each chord reaches it the way its hook hands on
    // keys that are not injected.
    let chord = |keys: Vec<(u16, bool)>| {
        slint::invoke_from_event_loop(move || super::hotkey_capture::deliver(&keys))
            .expect("event loop");
    };
    // Left Shift and Q.
    chord(vec![
        (0xa0, true),
        (0x51, true),
        (0x51, false),
        (0xa0, false),
    ]);
    let shift_refused = ui.wait(
        window.handle,
        UIA_TextControlTypeId,
        &tr("hotkey.shift_only"),
        Duration::from_secs(5),
    );
    let _ = screenshot("settings-shift-refused.bmp");
    let recording_after_shift = ui
        .find(window.handle, UIA_TextControlTypeId, &pressing)
        .is_some();
    // Left Ctrl, left Alt and E: AltGr+E.
    chord(vec![
        (0xa2, true),
        (0xa4, true),
        (0x45, true),
        (0x45, false),
        (0xa4, false),
        (0xa2, false),
    ]);
    let altgr_refused = ui.wait(
        window.handle,
        UIA_TextControlTypeId,
        &tr("hotkey.altgr"),
        Duration::from_secs(5),
    );
    let recording_after_altgr = ui
        .find(window.handle, UIA_TextControlTypeId, &pressing)
        .is_some();
    eprintln!(
        "recording={} shift_refused={} after_shift={recording_after_shift} altgr_refused={} \
         after_altgr={recording_after_altgr}",
        recording.is_some(),
        shift_refused.is_some(),
        altgr_refused.is_some()
    );
    // Escape ends the recording without a choice.
    chord(vec![(0x1b, true), (0x1b, false)]);
    window.request_close();
    let closed = window.closed_within(Duration::from_secs(30));
    assert!(recording.is_some(), "Space did not start the recording");
    assert!(
        shift_refused.is_some(),
        "Shift and a letter was not refused"
    );
    assert!(
        recording_after_shift,
        "the recording ended at Shift and a letter"
    );
    assert!(
        altgr_refused.is_some(),
        "Ctrl+Alt and a letter was not refused"
    );
    assert!(
        recording_after_altgr,
        "the recording ended at Ctrl+Alt and a letter"
    );
    assert!(closed, "the window did not close without changes");
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_shows_the_migration_review_without_opening_the_tools() {
    // Migration needs a package for every input language this build carries.
    let required: Vec<PackId> = autokeyboardlayot::DictionaryRegistry::embedded()
        .installed_ids()
        .copied()
        .filter(|id| *id != autokeyboardlayot::Language::English)
        .collect();
    let mut ids = required.clone();
    if ids.is_empty() {
        ids.push(PackId::parse("ur-PK").expect("id"));
    }
    let files = tempfile::tempdir().expect("package folder");
    let names = download_published(files.path(), &ids);
    let window = Window::open(Profile::legacy());
    let ui = Automation::new();
    show_page(&ui, &window, &tr("nav.languages"));
    // Left of the first language row is the padding of its card.
    let beside = ui
        .wait(
            window.handle,
            UIA_CheckBoxControlTypeId,
            "English",
            Duration::from_secs(10),
        )
        .expect("the English row")
        .rect
        .left
        - 10;
    ui.wait(
        window.handle,
        UIA_ButtonControlTypeId,
        &tr("migration.choose"),
        Duration::from_secs(10),
    )
    .expect("Choose migration packages")
    .invoke();
    let dialog = within(Duration::from_secs(15), || own_dialogs().into_iter().next())
        .expect("no file dialog");
    choose_in_dialog(dialog, files.path(), &names);
    let confirm = scroll_until(
        &ui,
        &window,
        beside,
        UIA_ButtonControlTypeId,
        &tr("migration.confirm"),
        Duration::from_secs(60),
    );
    let tools = scroll_until(
        &ui,
        &window,
        beside,
        UIA_CheckBoxControlTypeId,
        &tr("languages.advanced"),
        Duration::from_secs(10),
    );
    let tools_open = tools.as_ref().and_then(Element::checked);
    let _ = screenshot("settings-migration-review.bmp");
    eprintln!(
        "files={names:?} confirm={} tools_open={tools_open:?}",
        confirm.is_some()
    );
    if confirm.is_none() {
        for text in ui.texts(window.handle) {
            eprintln!("text: {}", text.name);
        }
    }
    let review = ui.elements(window.handle);
    let preview = review.iter().find(|element| {
        element.is(UIA_EditControlTypeId) && element.name == tr("migration.confirm")
    });
    let picker = review
        .iter()
        .find(|element| element.is(UIA_ComboBoxControlTypeId) && element.width() > 400);
    eprintln!(
        "picker={:?} preview={:?}",
        picker.map(|picker| picker.rect),
        preview.map(|preview| preview.rect)
    );
    // Leave without migrating: the review's Cancel is the one above the window's own.
    let cancel = ui
        .elements(window.handle)
        .into_iter()
        .filter(|element| {
            element.is(UIA_ButtonControlTypeId) && element.name == tr("button.cancel")
        })
        .min_by_key(|element| element.rect.top);
    if let Some(cancel) = cancel {
        cancel.invoke();
    }
    window.request_close();
    assert_eq!(tools_open, Some(false), "the tools section was open");
    assert!(confirm.is_some(), "the migration review did not show");
    assert!(
        picker
            .zip(preview)
            .is_some_and(|(picker, preview)| apart(picker, preview)),
        "the review's package picker is not apart from its preview"
    );
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_lists_the_installed_packages_at_opening() {
    let profile = Profile::managed("system");
    install_published(&profile, &["ur-PK"]);
    let window = Window::open(profile);
    let ui = Automation::new();
    show_page(&ui, &window, &tr("nav.languages"));
    let tools = ui
        .choices(window.handle, &tr("languages.advanced"))
        .into_iter()
        .next()
        .expect("the tools box");
    click(tools.center());
    let listed = within(Duration::from_secs(10), || {
        ui.elements(window.handle)
            .into_iter()
            .filter(|element| element.is(UIA_ComboBoxControlTypeId))
            .map(|element| format!("{} {}", element.name, element.value()))
            .find(|shown| shown.contains("ur-pk"))
    });
    let _ = screenshot("settings-installed-list.bmp");
    eprintln!("listed={listed:?}");
    window.request_close();
    assert!(
        listed.is_some(),
        "the installed package is not listed at opening"
    );
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_opens_its_file_dialogs_as_modal() {
    let window = Window::open(Profile::legacy());
    let ui = Automation::new();
    show_page(&ui, &window, &tr("nav.process_exclusions"));
    ui.wait(
        window.handle,
        UIA_ButtonControlTypeId,
        &tr("process_exclusions.browse"),
        Duration::from_secs(10),
    )
    .expect("Browse")
    .invoke();
    let dialog = within(Duration::from_secs(15), || own_dialogs().into_iter().next())
        .expect("no file dialog");
    let owner = unsafe { GetWindow(dialog, GW_OWNER) }.ok();
    let window_enabled = unsafe { IsWindowEnabled(window.handle) }.as_bool();
    let _ = screenshot("settings-file-dialog.bmp");
    unsafe {
        let _ = PostMessageW(Some(dialog), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    let enabled_again = within(Duration::from_secs(10), || {
        unsafe { IsWindowEnabled(window.handle) }
            .as_bool()
            .then_some(())
    });
    eprintln!(
        "owner={:?} window={:?} enabled_while_open={window_enabled}",
        owner.map(|owner| owner.0),
        window.handle.0
    );
    window.request_close();
    assert_eq!(owner, Some(window.handle), "the dialog has no owner");
    assert!(
        !window_enabled,
        "the window took input while the dialog was open"
    );
    assert!(enabled_again.is_some(), "the window stayed disabled");
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_leaves_the_chosen_folder_free_after_a_file_dialog() {
    let folder = tempfile::tempdir().expect("folder");
    let chosen = folder.path().join("chosen");
    std::fs::create_dir(&chosen).expect("chosen folder");
    std::fs::write(chosen.join("tool.exe"), b"MZ").expect("program file");
    let window = Window::open(Profile::legacy());
    let ui = Automation::new();
    show_page(&ui, &window, &tr("nav.process_exclusions"));
    ui.wait(
        window.handle,
        UIA_ButtonControlTypeId,
        &tr("process_exclusions.browse"),
        Duration::from_secs(10),
    )
    .expect("Browse")
    .invoke();
    let dialog = within(Duration::from_secs(15), || own_dialogs().into_iter().next())
        .expect("no file dialog");
    choose_in_dialog(dialog, &chosen, &["tool.exe".to_owned()]);
    let closed = within(Duration::from_secs(10), || {
        own_dialogs().is_empty().then_some(())
    });
    // The window says so when the dialog handed it a program, after the dialog returned.
    let chosen_listed = within(Duration::from_secs(5), || {
        ui.texts(window.handle)
            .into_iter()
            .any(|text| text.name == tr("process_exclusions.added"))
            .then_some(())
    })
    .is_some();
    let current = std::env::current_dir().ok();
    // Windows cannot rename or remove a folder that is a process's current directory: a USB drive
    // chosen there could not be ejected.
    let renamed = std::fs::rename(&chosen, folder.path().join("renamed"));
    eprintln!(
        "closed={} current={current:?} renamed={renamed:?} listed={chosen_listed}",
        closed.is_some()
    );
    window.request_close();
    assert!(closed.is_some(), "the file dialog did not close");
    assert!(chosen_listed, "the chosen program was not added");
    assert!(
        current.is_some_and(|current| !current.ends_with("chosen")),
        "the process stayed in the chosen folder"
    );
    assert!(renamed.is_ok(), "the chosen folder is still in use");
}

#[test]
#[ignore = "drives the settings window on the desktop: run it alone in an interactive session on the test VM"]
fn the_settings_window_on_the_desktop_stays_maximized_when_opened_again() {
    let window = Window::open(Profile::legacy());
    unsafe {
        let _ = ShowWindow(window.handle, SW_MAXIMIZE);
    }
    let maximized = within(Duration::from_secs(5), || {
        unsafe { IsZoomed(window.handle) }.as_bool().then_some(())
    });
    // What a second start does with the window that is open.
    focus_existing_window();
    thread::sleep(Duration::from_secs(1));
    let still = unsafe { IsZoomed(window.handle) }.as_bool();
    eprintln!("maximized={} still={still}", maximized.is_some());
    window.request_close();
    assert!(maximized.is_some(), "the window did not maximize");
    assert!(still, "opening Settings again un-maximized the window");
}
