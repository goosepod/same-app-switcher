//! SameAppSwitcher — a tiny Windows tray app that recreates macOS's Cmd+`.
//!
//!   Alt + `          cycle forward through the windows of the foreground app
//!   Alt + Shift + `  cycle backward
//!
//! "Same app" means "same executable", so it works for any program
//! (VS Code, Firefox, Explorer, Excel, terminals, ...).
//! The shortcut can be changed from the tray icon (right-click or double-click).
//! The default ` key is found by its physical position (the key above Tab), so it
//! works on non-US keyboard layouts too.

#![windows_subsystem = "windows"]
#![allow(non_snake_case)] // crate is named SameAppSwitcher so the exe is SameAppSwitcher.exe

use std::ffi::c_void;
use std::mem;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

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
const SHORTCUT_VALUE: &str = "Shortcut";

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

// Hotkey control flags (CommCtrl.h)
const HOTKEYF_SHIFT: u32 = 0x01;
const HOTKEYF_CONTROL: u32 = 0x02;
const HOTKEYF_ALT: u32 = 0x04;
const HOTKEYF_EXT: u32 = 0x08;
const HKCOMB_NONE: usize = 0x01;
const HKCOMB_S: usize = 0x02;

static MAIN_HWND: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// Current shortcut, packed as (MOD_* flags << 16) | virtual-key code.
static SHORTCUT: AtomicU32 = AtomicU32::new(0);
static HOTKEY_ACTIVE: AtomicBool = AtomicBool::new(false);
static DIALOG: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn res(id: u16) -> *const u16 {
    id as usize as *const u16 // MAKEINTRESOURCEW
}

#[derive(Clone, Copy, PartialEq)]
struct Shortcut {
    mods: u32, // MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN
    vk: u32,
}

impl Shortcut {
    fn pack(self) -> u32 {
        (self.mods << 16) | (self.vk & 0xFFFF)
    }
    fn unpack(v: u32) -> Self {
        Shortcut { mods: v >> 16, vk: v & 0xFFFF }
    }
    fn current() -> Self {
        Self::unpack(SHORTCUT.load(Ordering::Relaxed))
    }

    /// Alt + the key above Tab, whatever the keyboard layout calls it.
    unsafe fn default() -> Self {
        let mut vk = MapVirtualKeyW(0x29, MAPVK_VSC_TO_VK);
        if vk == 0 {
            vk = VK_OEM_3 as u32;
        }
        Shortcut { mods: MOD_ALT, vk }
    }

    /// The backward shortcut adds Shift, unless the shortcut already uses it.
    fn reverse(self) -> Option<Shortcut> {
        (self.mods & MOD_SHIFT == 0).then_some(Shortcut { mods: self.mods | MOD_SHIFT, vk: self.vk })
    }

    unsafe fn name(self) -> String {
        let mut s = String::new();
        for (flag, label) in [(MOD_WIN, "Win+"), (MOD_CONTROL, "Ctrl+"), (MOD_ALT, "Alt+"), (MOD_SHIFT, "Shift+")] {
            if self.mods & flag != 0 {
                s.push_str(label);
            }
        }
        s.push_str(&key_name(self.vk));
        s
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
        CreateMutexW(null(), 0, wide("Local\\SameAppSwitcher.SingleInstance").as_ptr());
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
            0, 0, 0, 0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        MAIN_HWND.store(hwnd, Ordering::Relaxed);
        TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);

        // Keep "Start with Windows" pointing at this exe, even if it was moved or renamed.
        if autostart_enabled() {
            set_autostart(true);
        }

        let sc = load_shortcut().unwrap_or_else(|| Shortcut::default());
        SHORTCUT.store(sc.pack(), Ordering::Relaxed);
        tray(hwnd, NIM_ADD);

        match register_hotkeys(sc) {
            Ok(true) => {}
            Ok(false) => message_box(&format!(
                "{} works, but the backward shortcut {} is already used by another program.",
                sc.name(),
                sc.reverse().map(|r| r.name()).unwrap_or_default()
            )),
            Err(()) => {
                message_box(&format!(
                    "{} is already used by another program.\n\nRight-click the SameAppSwitcher tray icon and choose \"Change shortcut...\" to pick a different one.",
                    sc.name()
                ));
                show_shortcut_dialog(hwnd);
            }
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

/// Ok(true) = both registered, Ok(false) = only forward, Err = forward failed.
unsafe fn register_hotkeys(sc: Shortcut) -> Result<bool, ()> {
    let hwnd = MAIN_HWND.load(Ordering::Relaxed);
    unregister_hotkeys();
    if RegisterHotKey(hwnd, HK_NEXT, sc.mods | MOD_NOREPEAT, sc.vk) == 0 {
        return Err(());
    }
    HOTKEY_ACTIVE.store(true, Ordering::Relaxed);
    Ok(match sc.reverse() {
        Some(r) => RegisterHotKey(hwnd, HK_PREV, r.mods | MOD_NOREPEAT, r.vk) != 0,
        None => true,
    })
}

unsafe fn unregister_hotkeys() {
    let hwnd = MAIN_HWND.load(Ordering::Relaxed);
    UnregisterHotKey(hwnd, HK_NEXT);
    UnregisterHotKey(hwnd, HK_PREV);
    HOTKEY_ACTIVE.store(false, Ordering::Relaxed);
}

unsafe fn load_shortcut() -> Option<Shortcut> {
    let mut v = 0u32;
    let mut size = 4u32;
    let r = RegGetValueW(
        HKEY_CURRENT_USER,
        wide(SETTINGS_KEY).as_ptr(),
        wide(SHORTCUT_VALUE).as_ptr(),
        RRF_RT_REG_DWORD,
        null_mut(),
        &mut v as *mut u32 as *mut c_void,
        &mut size,
    );
    let sc = Shortcut::unpack(v);
    (r == ERROR_SUCCESS && sc.vk != 0).then_some(sc)
}

unsafe fn save_shortcut(sc: Shortcut) {
    let v = sc.pack();
    RegSetKeyValueW(
        HKEY_CURRENT_USER,
        wide(SETTINGS_KEY).as_ptr(),
        wide(SHORTCUT_VALUE).as_ptr(),
        REG_DWORD,
        &v as *const u32 as *const c_void,
        4,
    );
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
    let dlg = CreateDialogParamW(GetModuleHandleW(null()), res(IDD_SHORTCUT), owner, Some(dlg_proc), 0);
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
    if hkf & HOTKEYF_ALT != 0 { m |= MOD_ALT; }
    if hkf & HOTKEYF_CONTROL != 0 { m |= MOD_CONTROL; }
    if hkf & HOTKEYF_SHIFT != 0 { m |= MOD_SHIFT; }
    m
}

unsafe fn mods_to_hkf(sc: Shortcut) -> u32 {
    let mut f = 0;
    if sc.mods & MOD_ALT != 0 { f |= HOTKEYF_ALT; }
    if sc.mods & MOD_CONTROL != 0 { f |= HOTKEYF_CONTROL; }
    if sc.mods & MOD_SHIFT != 0 { f |= HOTKEYF_SHIFT; }
    if MapVirtualKeyW(sc.vk, MAPVK_VK_TO_VSC_EX) & 0xFF00 != 0 { f |= HOTKEYF_EXT; }
    f
}

unsafe fn dlg_set(dlg: HWND, sc: Shortcut) {
    let hk = GetDlgItem(dlg, IDC_HOTKEY);
    SendMessageW(hk, HKM_SETHOTKEY, ((mods_to_hkf(sc) << 8) | (sc.vk & 0xFF)) as WPARAM, 0);
    let win = sc.mods & MOD_WIN != 0;
    SendMessageW(GetDlgItem(dlg, IDC_WIN), BM_SETCHECK, if win { BST_CHECKED } else { BST_UNCHECKED } as WPARAM, 0);
    dlg_update(dlg);
}

unsafe fn dlg_get(dlg: HWND) -> Shortcut {
    let v = SendMessageW(GetDlgItem(dlg, IDC_HOTKEY), HKM_GETHOTKEY, 0, 0) as u32;
    let mut mods = hkf_to_mods((v >> 8) & 0xFF);
    if SendMessageW(GetDlgItem(dlg, IDC_WIN), BM_GETCHECK, 0, 0) as u32 == BST_CHECKED {
        mods |= MOD_WIN;
    }
    Shortcut { mods, vk: v & 0xFF }
}

/// Keep the hint under the box in sync, and require a modifier unless Win is ticked.
unsafe fn dlg_update(dlg: HWND) {
    let hk = GetDlgItem(dlg, IDC_HOTKEY);
    let win = SendMessageW(GetDlgItem(dlg, IDC_WIN), BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
    if win {
        SendMessageW(hk, HKM_SETRULES, 0, 0);
    } else {
        // A bare key or Shift+key would hijack normal typing: turn those into Alt+key.
        SendMessageW(hk, HKM_SETRULES, HKCOMB_NONE | HKCOMB_S, HOTKEYF_ALT as LPARAM);
    }

    let sc = dlg_get(dlg);
    let text = if sc.vk == 0 {
        "Press a key combination, e.g. Alt+` or Ctrl+Alt+Q.".to_string()
    } else {
        match sc.reverse() {
            Some(r) => format!("{} = next window,  {} = previous window.", sc.name(), r.name()),
            None => format!("{} = next window. (It already uses Shift, so there's no previous-window shortcut.)", sc.name()),
        }
    };
    SetDlgItemTextW(dlg, IDC_REVERSE, wide(&text).as_ptr());
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

            let reset_label = format!("&Reset to {}", Shortcut::default().name());
            SetDlgItemTextW(dlg, IDC_RESET, wide(&reset_label).as_ptr());

            dlg_set(dlg, Shortcut::current());
            SetFocus(GetDlgItem(dlg, IDC_HOTKEY));
            0 // we set the focus ourselves
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            let code = ((wparam >> 16) & 0xFFFF) as u32;
            match id {
                IDC_HOTKEY if code == EN_CHANGE => dlg_update(dlg),
                IDC_WIN => dlg_update(dlg),
                IDC_RESET => {
                    dlg_set(dlg, Shortcut::default());
                    SetFocus(GetDlgItem(dlg, IDC_HOTKEY));
                }
                IDOK => {
                    let sc = dlg_get(dlg);
                    if sc.vk == 0 {
                        SetFocus(GetDlgItem(dlg, IDC_HOTKEY));
                        return 1;
                    }
                    match register_hotkeys(sc) {
                        Err(()) => {
                            dlg_message(dlg, &format!("{} is already used by another program (or reserved by Windows). Please pick a different shortcut.", sc.name()));
                            SetFocus(GetDlgItem(dlg, IDC_HOTKEY));
                            return 1;
                        }
                        Ok(both) => {
                            if !both {
                                dlg_message(dlg, &format!(
                                    "{} is set, but {} is already used by another program, so there won't be a previous-window shortcut.",
                                    sc.name(),
                                    sc.reverse().map(|r| r.name()).unwrap_or_default()
                                ));
                            }
                            SHORTCUT.store(sc.pack(), Ordering::Relaxed);
                            if sc == Shortcut::default() {
                                RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(SETTINGS_KEY).as_ptr(), wide(SHORTCUT_VALUE).as_ptr());
                            } else {
                                save_shortcut(sc);
                            }
                            tray(MAIN_HWND.load(Ordering::Relaxed), NIM_MODIFY);
                            close_shortcut_dialog(dlg);
                        }
                    }
                }
                IDCANCEL => {
                    // Put the previous shortcut back.
                    let _ = register_hotkeys(Shortcut::current());
                    close_shortcut_dialog(dlg);
                }
                _ => return 0,
            }
            1
        }
        _ => 0,
    }
}

unsafe fn dlg_message(owner: HWND, text: &str) {
    MessageBoxW(owner, wide(text).as_ptr(), wide(APP_NAME).as_ptr(), MB_OK | MB_ICONWARNING);
}

// ---------------------------------------------------------------------------
// Window switching
// ---------------------------------------------------------------------------

/// Cycle the foreground app's windows, the way Cmd+` does on macOS.
///
/// EnumWindows returns windows in z-order (front to back). With the app's
/// windows [A (front), B, C]:
///   forward  → bring the back-most (C) to front: [C, A, B] → [B, C, A] → ...
///   backward → bring B to front and send A to the back: [B, C, A] → ...
/// so repeated presses visit every window, and the two directions undo each other.
unsafe fn cycle(reverse: bool) {
    let fg = GetForegroundWindow();
    if fg.is_null() {
        return;
    }
    // If a dialog has focus, treat its main window as the current one.
    let fg = match GetAncestor(fg, GA_ROOTOWNER) {
        h if h.is_null() => fg,
        h => h,
    };

    let mut fg_pid = 0u32;
    GetWindowThreadProcessId(fg, &mut fg_pid);
    let Some(fg_exe) = process_path(fg_pid) else { return };

    let mut ctx = Enum { exe: fg_exe, fg_pid, cache: Vec::new(), windows: Vec::new() };
    EnumWindows(Some(enum_proc), &mut ctx as *mut Enum as LPARAM);
    let wins = ctx.windows;

    let cur = wins.iter().position(|&h| h == fg).unwrap_or(0);
    if wins.len() < 2 {
        return;
    }

    // Stop Alt's key-up from opening the menu bar in the window we activate
    // (VS Code, Explorer, etc. do that on a lone Alt press/release).
    mask_alt();

    if reverse {
        let target = wins[(cur + 1) % wins.len()];
        activate(target);
        SetWindowPos(fg, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    } else {
        let target = *wins.iter().rev().find(|&&h| h != fg).unwrap();
        activate(target);
    }
}

struct Enum {
    exe: String,
    fg_pid: u32,
    cache: Vec<(u32, bool)>, // pid -> "is the same exe"
    windows: Vec<HWND>,
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
        ctx.windows.push(hwnd);
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

unsafe fn activate(hwnd: HWND) {
    if IsIconic(hwnd) != 0 {
        ShowWindow(hwnd, SW_RESTORE);
    }
    if SetForegroundWindow(hwnd) != 0 {
        return;
    }
    // Fallback if Windows' focus-stealing rules refuse the request.
    let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), null_mut());
    let me = GetCurrentThreadId();
    if fg_thread != 0 && fg_thread != me {
        AttachThreadInput(me, fg_thread, 1);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        AttachThreadInput(me, fg_thread, 0);
    } else {
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
    }
}

/// Inject a press of an unassigned virtual key (0xE8) so the eventual Alt
/// (or Win) release isn't seen as a lone tap. Same trick AutoHotkey uses.
unsafe fn mask_alt() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: 0xE8, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    SendInput(inputs.len() as u32, inputs.as_ptr(), mem::size_of::<INPUT>() as i32);
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
        LoadIconMetric(GetModuleHandleW(null()), res(IDI_APP), LIM_SMALL, &mut nid.hIcon);
        if nid.hIcon.is_null() {
            nid.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
        }
        let tip = format!("{APP_NAME}  ({})", Shortcut::current().name());
        for (dst, src) in nid.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
            *dst = src;
        }
    }
    Shell_NotifyIconW(op, &nid);
}

unsafe fn show_menu(hwnd: HWND) {
    let sc = Shortcut::current();
    let header = if !HOTKEY_ACTIVE.load(Ordering::Relaxed) && DIALOG.load(Ordering::Relaxed).is_null() {
        format!("{} is in use by another program - choose another", sc.name())
    } else {
        match sc.reverse() {
            Some(r) => format!("Next window: {}    Previous: {}", sc.name(), r.name()),
            None => format!("Next window: {}", sc.name()),
        }
    };

    let menu = CreatePopupMenu();
    AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, wide(&header).as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_SHORTCUT, wide("&Change shortcut...").as_ptr());
    let check = if autostart_enabled() { MF_CHECKED } else { MF_UNCHECKED };
    AppendMenuW(menu, MF_STRING | check, ID_AUTOSTART, wide("&Start with Windows").as_ptr());
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
        let Ok(exe) = std::env::current_exe() else { return };
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
        RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), wide(RUN_VALUE).as_ptr());
    }
}

unsafe fn message_box(text: &str) {
    MessageBoxW(null_mut(), wide(text).as_ptr(), wide(APP_NAME).as_ptr(), MB_OK | MB_ICONWARNING);
}
