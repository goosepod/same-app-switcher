//! SameAppSwitcher — a tiny Windows tray app that recreates macOS's Cmd+`.
//!
//!   Alt + `          cycle forward through the windows of the foreground app
//!   Alt + Shift + `  cycle backward
//!
//! "Same app" means "same executable", so it works for any program
//! (VS Code, Firefox, Explorer, Excel, terminals, ...).
//! Both shortcuts can be changed independently from the tray icon.
//! The default ` key is found by its physical position (the key above Tab), so it
//! works on non-US keyboard layouts too.

#![windows_subsystem = "windows"]
#![allow(non_snake_case)] // crate is named SameAppSwitcher so the exe is SameAppSwitcher.exe

mod cycle_order;

use cycle_order::CycleOrder;
use std::cell::RefCell;
use std::ffi::c_void;
use std::mem;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const APP_NAME: &str = "SameAppSwitcher";
const RUN_VALUE: &str = "SameAppSwitcher";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const SETTINGS_KEY: &str = r"Software\SameAppSwitcher";
const SHORTCUT_VALUE: &str = "Shortcut"; // legacy next-window setting
const SHORTCUTS_VALUE: &str = "Shortcuts";

const HK_NEXT: i32 = 1;
const HK_PREV: i32 = 2;
const WM_TRAY: u32 = WM_APP + 1;
const TRAY_UID: u32 = 1;

// Menu commands
const ID_SHORTCUT: usize = 100;
const ID_AUTOSTART: usize = 101;
const ID_EXIT: usize = 102;

// Resource IDs (must match assets/app.rc)
const IDI_APP: u16 = 1;
const IDD_SHORTCUT: u16 = 101;
const IDC_HOTKEY: i32 = 1001;
const IDC_WIN: i32 = 1002;
const IDC_REVERSE: i32 = 1003;
const IDC_RESET: i32 = 1004;
const IDC_PREVIOUS: i32 = 1005;
const IDC_PREVIOUS_WIN: i32 = 1006;

// Hotkey control flags (CommCtrl.h)
const HOTKEYF_SHIFT: u32 = 0x01;
const HOTKEYF_CONTROL: u32 = 0x02;
const HOTKEYF_ALT: u32 = 0x04;
const HOTKEYF_EXT: u32 = 0x08;
const HKCOMB_NONE: usize = 0x01;
const HKCOMB_S: usize = 0x02;

static MAIN_HWND: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// Both shortcuts in one atomic snapshot: next in the low DWORD, previous in the high.
static SHORTCUTS: AtomicU64 = AtomicU64::new(0);
static HOTKEY_ACTIVE: AtomicBool = AtomicBool::new(false);
static DIALOG: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn res(id: u16) -> *const u16 {
    id as usize as *const u16 // MAKEINTRESOURCEW
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Shortcut {
    mods: u32, // MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN
    vk: u32,
}

impl Shortcut {
    fn pack(self) -> u32 {
        (self.mods << 16) | (self.vk & 0xFFFF)
    }
    fn unpack(v: u32) -> Self {
        Shortcut {
            mods: v >> 16,
            vk: v & 0xFFFF,
        }
    }
    fn valid(self) -> bool {
        self.vk > 0
            && self.vk <= 0xFE
            && self.mods & !(MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN) == 0
            && self.mods & (MOD_ALT | MOD_CONTROL | MOD_WIN) != 0
    }

    /// Alt + the key above Tab, whatever the keyboard layout calls it.
    unsafe fn default() -> Self {
        let mut vk = MapVirtualKeyW(0x29, MAPVK_VSC_TO_VK);
        if vk == 0 {
            vk = VK_OEM_3 as u32;
        }
        Shortcut { mods: MOD_ALT, vk }
    }

    unsafe fn name(self) -> String {
        let mut s = String::new();
        for (flag, label) in [
            (MOD_WIN, "Win+"),
            (MOD_CONTROL, "Ctrl+"),
            (MOD_ALT, "Alt+"),
            (MOD_SHIFT, "Shift+"),
        ] {
            if self.mods & flag != 0 {
                s.push_str(label);
            }
        }
        s.push_str(&key_name(self.vk));
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Shortcuts {
    next: Shortcut,
    previous: Shortcut,
}

impl Shortcuts {
    fn pack(self) -> u64 {
        self.next.pack() as u64 | ((self.previous.pack() as u64) << 32)
    }
    fn unpack(value: u64) -> Self {
        Self {
            next: Shortcut::unpack(value as u32),
            previous: Shortcut::unpack((value >> 32) as u32),
        }
    }
    fn current() -> Self {
        Self::unpack(SHORTCUTS.load(Ordering::Relaxed))
    }
    fn valid(self) -> bool {
        self.next.valid() && self.previous.valid() && self.next != self.previous
    }
    fn from_legacy(next: Shortcut) -> Self {
        Self {
            next,
            previous: Shortcut {
                mods: next.mods ^ MOD_SHIFT,
                vk: next.vk,
            },
        }
    }
    unsafe fn default() -> Self {
        Self::from_legacy(Shortcut::default())
    }
}

unsafe fn key_name(vk: u32) -> String {
    let sc = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC_EX);
    let mut lparam = ((sc & 0xFF) as i32) << 16;
    if sc & 0xFF00 != 0 {
        lparam |= 1 << 24; // extended key (arrows, Insert, Home, ...)
    }
    let mut buf = [0u16; 64];
    let n = GetKeyNameTextW(lparam, buf.as_mut_ptr(), buf.len() as i32);
    if n > 0 {
        let name = String::from_utf16_lossy(&buf[..n as usize]);
        // GetKeyNameText returns letters in upper case but symbols as-is; tidy single chars.
        if name.chars().count() == 1 {
            return name.to_uppercase();
        }
        // "PAGE UP" -> "Page Up"
        return name
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    format!("Key {vk:#04x}")
}

fn main() {
    unsafe {
        // One instance only.
        CreateMutexW(
            null(),
            0,
            wide("Local\\SameAppSwitcher.SingleInstance").as_ptr(),
        );
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        let icc = INITCOMMONCONTROLSEX {
            dwSize: mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_HOTKEY_CLASS | ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&icc);

        let hinst = GetModuleHandleW(null());
        let class = wide("SameAppSwitcherWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            hIcon: LoadIconW(hinst, res(IDI_APP)),
            ..mem::zeroed()
        };
        RegisterClassW(&wc);

        // A hidden (never shown) top-level window: it receives hotkeys, tray
        // events and the TaskbarCreated broadcast (sent when Explorer restarts).
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide(APP_NAME).as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        MAIN_HWND.store(hwnd, Ordering::Relaxed);
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            Ordering::Relaxed,
        );

        // Keep "Start with Windows" pointing at this exe, even if it was moved or renamed.
        if autostart_enabled() {
            set_autostart(true);
        }

        let shortcuts = load_shortcuts();
        SHORTCUTS.store(shortcuts.pack(), Ordering::Relaxed);
        tray(hwnd, NIM_ADD);
        if let Err(conflict) = register_hotkeys(shortcuts) {
            message_box(&format!("{} could not be registered (already in use or reserved by Windows). Choose another shortcut.", conflict.name()));
            show_shortcut_dialog(hwnd);
        }

        let mut msg: MSG = mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            let dlg = DIALOG.load(Ordering::Relaxed);
            if !dlg.is_null() && IsDialogMessageW(dlg, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            match wparam as i32 {
                HK_NEXT => cycle(false),
                HK_PREV => cycle(true),
                _ => {}
            }
            0
        }
        WM_TRAY => {
            let event = (lparam as u32) & 0xFFFF;
            if event == WM_RBUTTONUP || event == WM_LBUTTONDBLCLK {
                show_menu(hwnd);
            }
            0
        }
        WM_COMMAND => {
            match wparam & 0xFFFF {
                ID_SHORTCUT => show_shortcut_dialog(hwnd),
                ID_AUTOSTART => set_autostart(!autostart_enabled()),
                ID_EXIT => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            tray(hwnd, NIM_DELETE);
            PostQuitMessage(0);
            0
        }
        _ if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            tray(hwnd, NIM_ADD);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ---------------------------------------------------------------------------
// Hotkey registration & settings
// ---------------------------------------------------------------------------

/// Register the pair together. A conflict leaves neither shortcut half-applied.
unsafe fn register_hotkeys(shortcuts: Shortcuts) -> Result<(), Shortcut> {
    unregister_hotkeys();
    let hwnd = MAIN_HWND.load(Ordering::Relaxed);
    for (id, shortcut) in [(HK_NEXT, shortcuts.next), (HK_PREV, shortcuts.previous)] {
        if RegisterHotKey(hwnd, id, shortcut.mods | MOD_NOREPEAT, shortcut.vk) == 0 {
            unregister_hotkeys();
            return Err(shortcut);
        }
    }
    HOTKEY_ACTIVE.store(true, Ordering::Relaxed);
    Ok(())
}

unsafe fn unregister_hotkeys() {
    let hwnd = MAIN_HWND.load(Ordering::Relaxed);
    UnregisterHotKey(hwnd, HK_NEXT);
    UnregisterHotKey(hwnd, HK_PREV);
    HOTKEY_ACTIVE.store(false, Ordering::Relaxed);
}

unsafe fn load_shortcuts() -> Shortcuts {
    let mut bytes = [0u8; 8];
    let mut size = bytes.len() as u32;
    let result = RegGetValueW(
        HKEY_CURRENT_USER,
        wide(SETTINGS_KEY).as_ptr(),
        wide(SHORTCUTS_VALUE).as_ptr(),
        RRF_RT_REG_BINARY,
        null_mut(),
        bytes.as_mut_ptr().cast(),
        &mut size,
    );
    let shortcuts = Shortcuts::unpack(u64::from_le_bytes(bytes));
    if result == ERROR_SUCCESS && size == 8 && shortcuts.valid() {
        return shortcuts;
    }

    let mut legacy = 0u32;
    let mut size = 4;
    let result = RegGetValueW(
        HKEY_CURRENT_USER,
        wide(SETTINGS_KEY).as_ptr(),
        wide(SHORTCUT_VALUE).as_ptr(),
        RRF_RT_REG_DWORD,
        null_mut(),
        (&mut legacy as *mut u32).cast(),
        &mut size,
    );
    let shortcuts = Shortcuts::from_legacy(Shortcut::unpack(legacy));
    if result == ERROR_SUCCESS && shortcuts.valid() {
        shortcuts
    } else {
        Shortcuts::default()
    }
}

unsafe fn save_shortcuts(shortcuts: Shortcuts) -> Result<(), u32> {
    if shortcuts == Shortcuts::default() {
        for name in [SHORTCUTS_VALUE, SHORTCUT_VALUE] {
            let result = RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                wide(SETTINGS_KEY).as_ptr(),
                wide(name).as_ptr(),
            );
            if ![ERROR_SUCCESS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND].contains(&result) {
                return Err(result);
            }
        }
        return Ok(());
    }
    let bytes = shortcuts.pack().to_le_bytes();
    let result = RegSetKeyValueW(
        HKEY_CURRENT_USER,
        wide(SETTINGS_KEY).as_ptr(),
        wide(SHORTCUTS_VALUE).as_ptr(),
        REG_BINARY,
        bytes.as_ptr().cast(),
        bytes.len() as u32,
    );
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(result)
    }
}

// ---------------------------------------------------------------------------
// Shortcut dialog
// ---------------------------------------------------------------------------

unsafe fn show_shortcut_dialog(owner: HWND) {
    let open = DIALOG.load(Ordering::Relaxed);
    if !open.is_null() {
        SetForegroundWindow(open);
        return;
    }
    // Modeless, so the tray menu keeps working; the message loop routes
    // keyboard input to it via IsDialogMessage.
    let dlg = CreateDialogParamW(
        GetModuleHandleW(null()),
        res(IDD_SHORTCUT),
        owner,
        Some(dlg_proc),
        0,
    );
    if dlg.is_null() {
        return;
    }
    DIALOG.store(dlg, Ordering::Relaxed);
    ShowWindow(dlg, SW_SHOW);
    SetForegroundWindow(dlg);
}

unsafe fn close_shortcut_dialog(dlg: HWND) {
    DIALOG.store(null_mut(), Ordering::Relaxed);
    DestroyWindow(dlg);
}

/// Hotkey-control modifier flags -> RegisterHotKey modifier flags.
fn hkf_to_mods(hkf: u32) -> u32 {
    let mut m = 0;
    if hkf & HOTKEYF_ALT != 0 {
        m |= MOD_ALT;
    }
    if hkf & HOTKEYF_CONTROL != 0 {
        m |= MOD_CONTROL;
    }
    if hkf & HOTKEYF_SHIFT != 0 {
        m |= MOD_SHIFT;
    }
    m
}

unsafe fn mods_to_hkf(sc: Shortcut) -> u32 {
    let mut f = 0;
    if sc.mods & MOD_ALT != 0 {
        f |= HOTKEYF_ALT;
    }
    if sc.mods & MOD_CONTROL != 0 {
        f |= HOTKEYF_CONTROL;
    }
    if sc.mods & MOD_SHIFT != 0 {
        f |= HOTKEYF_SHIFT;
    }
    if MapVirtualKeyW(sc.vk, MAPVK_VK_TO_VSC_EX) & 0xFF00 != 0 {
        f |= HOTKEYF_EXT;
    }
    f
}

unsafe fn dlg_set_one(dlg: HWND, field: i32, win_field: i32, shortcut: Shortcut) {
    SendMessageW(
        GetDlgItem(dlg, field),
        HKM_SETHOTKEY,
        ((mods_to_hkf(shortcut) << 8) | (shortcut.vk & 0xFF)) as WPARAM,
        0,
    );
    SendMessageW(
        GetDlgItem(dlg, win_field),
        BM_SETCHECK,
        if shortcut.mods & MOD_WIN != 0 {
            BST_CHECKED
        } else {
            BST_UNCHECKED
        } as WPARAM,
        0,
    );
}

unsafe fn dlg_set(dlg: HWND, shortcuts: Shortcuts) {
    dlg_set_one(dlg, IDC_HOTKEY, IDC_WIN, shortcuts.next);
    dlg_set_one(dlg, IDC_PREVIOUS, IDC_PREVIOUS_WIN, shortcuts.previous);
    dlg_update(dlg);
}

unsafe fn dlg_get_one(dlg: HWND, field: i32, win_field: i32) -> Shortcut {
    let value = SendMessageW(GetDlgItem(dlg, field), HKM_GETHOTKEY, 0, 0) as u32;
    let mut mods = hkf_to_mods((value >> 8) & 0xFF);
    if SendMessageW(GetDlgItem(dlg, win_field), BM_GETCHECK, 0, 0) as u32 == BST_CHECKED {
        mods |= MOD_WIN;
    }
    Shortcut {
        mods,
        vk: value & 0xFF,
    }
}

unsafe fn dlg_get(dlg: HWND) -> Shortcuts {
    Shortcuts {
        next: dlg_get_one(dlg, IDC_HOTKEY, IDC_WIN),
        previous: dlg_get_one(dlg, IDC_PREVIOUS, IDC_PREVIOUS_WIN),
    }
}

unsafe fn dlg_update(dlg: HWND) {
    for (field, win_field) in [(IDC_HOTKEY, IDC_WIN), (IDC_PREVIOUS, IDC_PREVIOUS_WIN)] {
        let win = SendMessageW(GetDlgItem(dlg, win_field), BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
        SendMessageW(
            GetDlgItem(dlg, field),
            HKM_SETRULES,
            if win { 0 } else { HKCOMB_NONE | HKCOMB_S },
            HOTKEYF_ALT as LPARAM,
        );
    }
    let shortcuts = dlg_get(dlg);
    let text = if !shortcuts.next.valid() || !shortcuts.previous.valid() {
        "Choose two shortcuts with Alt, Ctrl, or the Windows key.".to_string()
    } else if shortcuts.next == shortcuts.previous {
        "Next and previous must use different shortcuts.".to_string()
    } else {
        format!(
            "Next: {}    Previous: {}",
            shortcuts.next.name(),
            shortcuts.previous.name()
        )
    };
    EnableWindow(GetDlgItem(dlg, IDOK), shortcuts.valid() as BOOL);
    SetDlgItemTextW(dlg, IDC_REVERSE, wide(&text).as_ptr());
}

unsafe fn cancel_shortcut_dialog(dlg: HWND) {
    if let Err(conflict) = register_hotkeys(Shortcuts::current()) {
        dlg_message(
            dlg,
            &format!(
                "{} is now unavailable. Reopen Change shortcuts to choose another pair.",
                conflict.name()
            ),
        );
    }
    close_shortcut_dialog(dlg);
}

unsafe extern "system" fn dlg_proc(dlg: HWND, msg: u32, wparam: WPARAM, _lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            // Free the current shortcut while editing, so pressing it in the box
            // records it instead of switching windows.
            unregister_hotkeys();

            let hinst = GetModuleHandleW(null());
            let mut big: HICON = null_mut();
            let mut small: HICON = null_mut();
            LoadIconMetric(hinst, res(IDI_APP), LIM_LARGE, &mut big);
            LoadIconMetric(hinst, res(IDI_APP), LIM_SMALL, &mut small);
            SendMessageW(dlg, WM_SETICON, ICON_BIG as WPARAM, big as LPARAM);
            SendMessageW(dlg, WM_SETICON, ICON_SMALL as WPARAM, small as LPARAM);

            dlg_set(dlg, Shortcuts::current());
            SetFocus(GetDlgItem(dlg, IDC_HOTKEY));
            0 // we set the focus ourselves
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            let code = ((wparam >> 16) & 0xFFFF) as u32;
            match id {
                IDC_HOTKEY | IDC_PREVIOUS if code == EN_CHANGE => dlg_update(dlg),
                IDC_WIN | IDC_PREVIOUS_WIN => dlg_update(dlg),
                IDC_RESET => {
                    dlg_set(dlg, Shortcuts::default());
                    SetFocus(GetDlgItem(dlg, IDC_HOTKEY));
                }
                IDOK => {
                    let shortcuts = dlg_get(dlg);
                    if !shortcuts.valid() {
                        return 1;
                    }
                    if let Err(conflict) = register_hotkeys(shortcuts) {
                        dlg_message(dlg, &format!("{} is already in use or reserved by Windows. Please choose another shortcut.", conflict.name()));
                        return 1;
                    }
                    if let Err(error) = save_shortcuts(shortcuts) {
                        unregister_hotkeys();
                        dlg_message(dlg, &format!("Could not save shortcuts (Windows error {error}). Your previous settings are retained."));
                        return 1;
                    }
                    SHORTCUTS.store(shortcuts.pack(), Ordering::Relaxed);
                    tray(MAIN_HWND.load(Ordering::Relaxed), NIM_MODIFY);
                    close_shortcut_dialog(dlg);
                }
                IDCANCEL => cancel_shortcut_dialog(dlg),
                _ => return 0,
            }
            1
        }
        WM_CLOSE => {
            cancel_shortcut_dialog(dlg);
            1
        }
        _ => 0,
    }
}

unsafe fn dlg_message(owner: HWND, text: &str) {
    MessageBoxW(
        owner,
        wide(text).as_ptr(),
        wide(APP_NAME).as_ptr(),
        MB_OK | MB_ICONWARNING,
    );
}

// ---------------------------------------------------------------------------
// Window switching
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
struct WindowId {
    handle: usize,
    pid: u32,
}

thread_local! {
    static CYCLE_ORDER: RefCell<CycleOrder<WindowId>> = const { RefCell::new(CycleOrder::new()) };
}

unsafe fn root_owner(hwnd: HWND) -> HWND {
    let root = GetAncestor(hwnd, GA_ROOTOWNER);
    if root.is_null() {
        hwnd
    } else {
        root
    }
}

/// Keep an app-local ring instead of moving windows behind unrelated apps.
unsafe fn cycle(reverse: bool) {
    let fg = root_owner(GetForegroundWindow());
    if fg.is_null() {
        return;
    }
    let mut fg_pid = 0;
    GetWindowThreadProcessId(fg, &mut fg_pid);
    let Some(exe) = process_path(fg_pid) else {
        return;
    };
    let mut ctx = Enum {
        exe,
        fg_pid,
        cache: Vec::new(),
        windows: Vec::new(),
    };
    if EnumWindows(Some(enum_proc), &mut ctx as *mut Enum as LPARAM) == 0 {
        return;
    }
    let current = WindowId {
        handle: fg as usize,
        pid: fg_pid,
    };
    let target =
        CYCLE_ORDER.with(|order| order.borrow_mut().target(&ctx.windows, current, reverse));
    if let Some(target) = target {
        mask_alt();
        if activate(target.handle as HWND) {
            CYCLE_ORDER.with(|order| order.borrow_mut().activated(target));
        }
    }
}

struct Enum {
    exe: String,
    fg_pid: u32,
    cache: Vec<(u32, bool)>, // pid -> "is the same exe"
    windows: Vec<WindowId>,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam as *mut Enum);
    if !is_switchable(hwnd) {
        return 1;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let same = if pid == ctx.fg_pid {
        true
    } else if let Some(&(_, s)) = ctx.cache.iter().find(|(p, _)| *p == pid) {
        s
    } else {
        let s = process_path(pid).as_deref() == Some(ctx.exe.as_str());
        ctx.cache.push((pid, s));
        s
    };
    if same {
        ctx.windows.push(WindowId {
            handle: hwnd as usize,
            pid,
        });
    }
    1
}

/// Roughly "would this window appear in Alt+Tab?"
unsafe fn is_switchable(hwnd: HWND) -> bool {
    if IsWindowVisible(hwnd) == 0 {
        return false;
    }
    if !GetWindow(hwnd, GW_OWNER).is_null() {
        return false;
    }
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if ex & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0 {
        return false;
    }
    if GetWindowTextLengthW(hwnd) == 0 {
        return false;
    }
    // Cloaked = on another virtual desktop, or a suspended UWP shell window.
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED as u32,
        &mut cloaked as *mut u32 as *mut c_void,
        mem::size_of::<u32>() as u32,
    ) >= 0
        && cloaked != 0
    {
        return false;
    }
    true
}

unsafe fn process_path(pid: u32) -> Option<String> {
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h.is_null() {
        return None; // e.g. an elevated process while we're not elevated
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
    CloseHandle(h);
    if ok == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]).to_lowercase())
}

unsafe fn activate(root: HWND) -> bool {
    let popup = GetLastActivePopup(root);
    let hwnd = if !popup.is_null() && IsWindowVisible(popup) != 0 {
        popup
    } else {
        root
    };
    if IsIconic(root) != 0 {
        ShowWindow(root, SW_RESTORE);
    }
    if SetForegroundWindow(hwnd) == 0 {
        let foreground_thread = GetWindowThreadProcessId(GetForegroundWindow(), null_mut());
        let current_thread = GetCurrentThreadId();
        let attached = foreground_thread != 0
            && foreground_thread != current_thread
            && AttachThreadInput(current_thread, foreground_thread, 1) != 0;
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        if attached {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
    }
    root_owner(GetForegroundWindow()) == root
}

/// Inject a press of an unassigned virtual key (0xE8) so the eventual Alt
/// (or Win) release isn't seen as a lone tap. Same trick AutoHotkey uses.
unsafe fn mask_alt() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0xE8,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    SendInput(
        inputs.len() as u32,
        inputs.as_ptr(),
        mem::size_of::<INPUT>() as i32,
    );
}

// ---------------------------------------------------------------------------
// Tray icon & menu
// ---------------------------------------------------------------------------

unsafe fn tray(hwnd: HWND, op: NOTIFY_ICON_MESSAGE) {
    let mut nid: NOTIFYICONDATAW = mem::zeroed();
    nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    if op != NIM_DELETE {
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        // Sized for the current DPI, from the icon embedded in the exe.
        LoadIconMetric(
            GetModuleHandleW(null()),
            res(IDI_APP),
            LIM_SMALL,
            &mut nid.hIcon,
        );
        if nid.hIcon.is_null() {
            nid.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
        }
        let shortcuts = Shortcuts::current();
        let tip = format!(
            "{APP_NAME}  Next: {}  Previous: {}",
            shortcuts.next.name(),
            shortcuts.previous.name()
        );
        for (dst, src) in nid.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
            *dst = src;
        }
    }
    Shell_NotifyIconW(op, &nid);
}

unsafe fn show_menu(hwnd: HWND) {
    let shortcuts = Shortcuts::current();
    let header =
        if !HOTKEY_ACTIVE.load(Ordering::Relaxed) && DIALOG.load(Ordering::Relaxed).is_null() {
            "Shortcuts unavailable - choose another pair".to_string()
        } else {
            format!(
                "Next: {}    Previous: {}",
                shortcuts.next.name(),
                shortcuts.previous.name()
            )
        };

    let menu = CreatePopupMenu();
    AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, wide(&header).as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(
        menu,
        MF_STRING,
        ID_SHORTCUT,
        wide("&Change shortcuts...").as_ptr(),
    );
    let check = if autostart_enabled() {
        MF_CHECKED
    } else {
        MF_UNCHECKED
    };
    AppendMenuW(
        menu,
        MF_STRING | check,
        ID_AUTOSTART,
        wide("&Start with Windows").as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_EXIT, wide("E&xit").as_ptr());
    SetMenuDefaultItem(menu, ID_SHORTCUT as u32, 0);

    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    // Required so the menu closes when you click elsewhere.
    SetForegroundWindow(hwnd);
    TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, null());
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
}

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------

unsafe fn autostart_enabled() -> bool {
    RegGetValueW(
        HKEY_CURRENT_USER,
        wide(RUN_KEY).as_ptr(),
        wide(RUN_VALUE).as_ptr(),
        RRF_RT_REG_SZ,
        null_mut(),
        null_mut(),
        null_mut(),
    ) == ERROR_SUCCESS
}

unsafe fn set_autostart(enable: bool) {
    if enable {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let cmd = wide(&format!("\"{}\"", exe.display()));
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            wide(RUN_VALUE).as_ptr(),
            REG_SZ,
            cmd.as_ptr() as *const c_void,
            (cmd.len() * 2) as u32,
        );
    } else {
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            wide(RUN_VALUE).as_ptr(),
        );
    }
}

unsafe fn message_box(text: &str) {
    MessageBoxW(
        null_mut(),
        wide(text).as_ptr(),
        wide(APP_NAME).as_ptr(),
        MB_OK | MB_ICONWARNING,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_pair_round_trips_without_losing_windows_or_shift_modifiers() {
        let shortcuts = Shortcuts {
            next: Shortcut {
                mods: MOD_ALT | MOD_CONTROL,
                vk: 0x51,
            },
            previous: Shortcut {
                mods: MOD_WIN | MOD_SHIFT,
                vk: 0x57,
            },
        };
        assert_eq!(Shortcuts::unpack(shortcuts.pack()), shortcuts);
        assert!(shortcuts.valid());
    }

    #[test]
    fn legacy_shortcut_migration_preserves_the_existing_next_combo() {
        let next = Shortcut {
            mods: MOD_CONTROL | MOD_ALT,
            vk: 0x51,
        };
        let migrated = Shortcuts::from_legacy(next);
        assert_eq!(migrated.next, next);
        assert_eq!(
            migrated.previous,
            Shortcut {
                mods: MOD_CONTROL | MOD_ALT | MOD_SHIFT,
                vk: 0x51
            }
        );
        assert!(migrated.valid());
    }

    #[test]
    fn legacy_shift_shortcut_gets_an_independent_reverse_combo() {
        let migrated = Shortcuts::from_legacy(Shortcut {
            mods: MOD_ALT | MOD_SHIFT,
            vk: 0x51,
        });
        assert_eq!(migrated.previous.mods, MOD_ALT);
        assert!(migrated.valid());
    }

    #[test]
    fn reject_duplicate_shortcuts_and_combinations_that_hijack_typing() {
        let next = Shortcut {
            mods: MOD_ALT,
            vk: 0x51,
        };
        assert!(!Shortcuts {
            next,
            previous: next
        }
        .valid());
        for invalid in [
            Shortcut { mods: 0, vk: 0x51 },
            Shortcut {
                mods: MOD_SHIFT,
                vk: 0x51,
            },
            Shortcut {
                mods: MOD_ALT,
                vk: 0,
            },
            Shortcut {
                mods: MOD_ALT,
                vk: 0xFFFF,
            },
            Shortcut {
                mods: MOD_ALT | MOD_NOREPEAT,
                vk: 0x51,
            },
        ] {
            assert!(!invalid.valid());
        }
    }
}
