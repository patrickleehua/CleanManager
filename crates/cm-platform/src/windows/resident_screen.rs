//! Clean-screen mode on Windows (add-mole-parity-suite 15.2, RES-02).
//!
//! A black, top-most, borderless window covers the whole virtual screen (every monitor) and hides the pointer.
//! While it is shown, low-level keyboard and mouse hooks (`WH_KEYBOARD_LL`, `WH_MOUSE_LL`) swallow key presses,
//! clicks and wheel turns so wiping the keyboard or screen does nothing; pointer movement still passes.
//!
//! Exits (always restoring input, because the hooks are removed when the mode ends):
//! - hold Esc for 2 seconds ([`cm_core::resident::ESC_HOLD`]);
//! - the timeout (5–300 s) is reached;
//! - Ctrl+Alt+Del (cannot be intercepted by any program) and then any action that closes the window.
//!
//! A dim line at the bottom of the main display says how to leave and counts down. Only one clean-screen session
//! can run at a time. Key releases are swallowed only for keys whose press was swallowed, so a key that was
//! already held when the mode started does not stay stuck.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use cm_core::resident::{CleanScreenExit, EscHold};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject, BLACK_BRUSH, HBRUSH, InvalidateRect,
    PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics, HC_ACTION, KBDLLHOOKSTRUCT, KillTimer, MSG,
    PostQuitMessage, RegisterClassExW, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetCursor,
    SetForegroundWindow, SetTimer, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND,
    WM_KEYDOWN, WM_KEYUP, WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    WS_VISIBLE,
};

const VK_ESCAPE: u32 = 0x1B;
const TIMER_ID: usize = 1;

static RUNNING: AtomicBool = AtomicBool::new(false);
/// Esc pressed since (ms since the session started + 1), 0 when not held.
static ESC_SINCE: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicU64 = AtomicU64::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);
/// 0 = running, 1 = Esc held, 2 = timeout, 3 = closed.
static EXIT: AtomicUsize = AtomicUsize::new(0);
static SHOWN_SECS: AtomicU64 = AtomicU64::new(u64::MAX);
/// Virtual keys whose press was swallowed (their release is swallowed too).
static SWALLOWED: [AtomicU64; 4] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

fn now_ms() -> u64 {
    // SAFETY: no arguments.
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn mark(vk: u32, on: bool) -> bool {
    let (i, bit) = ((vk as usize / 64) & 3, 1u64 << (vk % 64));
    let before = if on { SWALLOWED[i].fetch_or(bit, Ordering::SeqCst) } else { SWALLOWED[i].fetch_and(!bit, Ordering::SeqCst) };
    before & bit != 0
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points to a KBDLLHOOKSTRUCT owned by the system for this call.
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let msg = wparam as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if kb.vkCode == VK_ESCAPE {
            if down {
                let since = now_ms().saturating_sub(STARTED.load(Ordering::SeqCst)) + 1;
                let _ = ESC_SINCE.compare_exchange(0, since, Ordering::SeqCst, Ordering::SeqCst);
            } else if up {
                ESC_SINCE.store(0, Ordering::SeqCst);
            }
        }
        if down {
            mark(kb.vkCode, true);
            return 1;
        }
        if up && mark(kb.vkCode, false) {
            return 1;
        }
    }
    // SAFETY: pass everything else along the hook chain.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && wparam as u32 != WM_MOUSEMOVE {
        // Clicks, wheel and extra buttons are swallowed; moving the pointer is harmless.
        return 1;
    }
    // SAFETY: pass along the hook chain.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

fn esc_hold() -> EscHold {
    let mut h = EscHold::default();
    let since = ESC_SINCE.load(Ordering::SeqCst);
    if since != 0 {
        h.key_down(since - 1);
    }
    h
}

fn remaining_secs() -> u64 {
    DEADLINE.load(Ordering::SeqCst).saturating_sub(now_ms()).div_ceil(1000)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            let elapsed = now_ms().saturating_sub(STARTED.load(Ordering::SeqCst));
            let reason = if esc_hold().held(elapsed) {
                1
            } else if now_ms() >= DEADLINE.load(Ordering::SeqCst) {
                2
            } else {
                0
            };
            if reason != 0 {
                EXIT.store(reason, Ordering::SeqCst);
                // SAFETY: our own window, destroyed on its thread.
                unsafe { DestroyWindow(hwnd) };
            } else if SHOWN_SECS.load(Ordering::SeqCst) != remaining_secs() {
                // SAFETY: repaint the countdown line.
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
            0
        }
        WM_SETCURSOR => {
            // SAFETY: hide the pointer over the window.
            unsafe { SetCursor(std::ptr::null_mut()) };
            1
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            // SAFETY: standard paint cycle on our window with stock/created GDI objects released below.
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                FillRect(dc, &ps.rcPaint, GetStockObject(BLACK_BRUSH) as HBRUSH);
                let secs = remaining_secs();
                SHOWN_SECS.store(secs, Ordering::SeqCst);
                // The main display starts at screen (0, 0); the window starts at the virtual-screen origin.
                let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
                let (pw, ph) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
                let mut r = RECT { left: -vx, top: -vy + ph - 96, right: -vx + pw, bottom: -vy + ph - 48 };
                let face = wide("Microsoft YaHei UI");
                let font = CreateFontW(-17, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr());
                let old = SelectObject(dc, font);
                SetBkMode(dc, TRANSPARENT as i32);
                SetTextColor(dc, 0x0060_6060);
                let text: Vec<u16> = format!("清洁屏幕：按键与点击已暂停。按住 Esc 2 秒退出，{secs} 秒后自动退出。").encode_utf16().collect();
                DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(dc, old);
                DeleteObject(font);
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_CLOSE => {
            let _ = EXIT.compare_exchange(0, 3, Ordering::SeqCst, Ordering::SeqCst);
            // SAFETY: our own window.
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            // SAFETY: our timer and message loop.
            unsafe {
                KillTimer(hwnd, TIMER_ID);
                PostQuitMessage(0);
            }
            0
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

struct Hook(windows_sys::Win32::UI::WindowsAndMessaging::HHOOK);

impl Drop for Hook {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a hook installed by this thread, removed once.
            unsafe { UnhookWindowsHookEx(self.0) };
        }
    }
}

/// Show clean-screen mode on the calling thread until Esc is held for 2 s, the timeout passes, or the window is
/// closed. Blocks; input is restored before it returns. Refuses to start a second session.
pub fn run(timeout: Duration) -> Result<CleanScreenExit, String> {
    if RUNNING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Err("clean-screen mode is already running".into());
    }
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }
    let _running = Running;
    let start = now_ms();
    STARTED.store(start, Ordering::SeqCst);
    DEADLINE.store(start + timeout.as_millis() as u64, Ordering::SeqCst);
    ESC_SINCE.store(0, Ordering::SeqCst);
    EXIT.store(0, Ordering::SeqCst);
    SHOWN_SECS.store(u64::MAX, Ordering::SeqCst);
    for s in &SWALLOWED {
        s.store(0, Ordering::SeqCst);
    }
    let class = wide("CleanManager.CleanScreen");
    let title = wide("CleanManager 清洁屏幕");
    // SAFETY: registers our window class (a second registration fails harmlessly), creates the window and the
    // hooks on this thread, and runs this thread's message loop until the window is destroyed.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: GetStockObject(BLACK_BRUSH) as HBRUSH,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        RegisterClassExW(&wc);
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, class.as_ptr(), title.as_ptr(), WS_POPUP | WS_VISIBLE, x, y, w, h, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null());
        if hwnd.is_null() {
            return Err("the clean-screen window could not be created".into());
        }
        SetForegroundWindow(hwnd);
        let keyboard = Hook(SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), instance, 0));
        let mouse = Hook(SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), instance, 0));
        if keyboard.0.is_null() {
            DestroyWindow(hwnd);
            return Err("keyboard input could not be paused".into());
        }
        SetTimer(hwnd, TIMER_ID, 100, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        drop(keyboard);
        drop(mouse);
    }
    Ok(match EXIT.load(Ordering::SeqCst) {
        1 => CleanScreenExit::EscHeld,
        2 => CleanScreenExit::Timeout,
        _ => CleanScreenExit::Closed,
    })
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-device check (blanks the screen for a few seconds and pauses input):
    /// `cargo test -p cm-platform clean_screen_exits -- --ignored --nocapture`
    #[test]
    #[ignore = "real device: blanks every display and pauses keyboard/mouse for up to ~8 seconds"]
    fn clean_screen_exits_on_timeout_and_on_a_long_esc_press() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_ESCAPE as VK_ESC};
        // Timeout; meanwhile check the window covers the virtual screen and is top-most and visible.
        let probe = std::thread::spawn(|| {
            use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GWL_EXSTYLE, GetWindowLongW, GetWindowRect, IsWindowVisible};
            std::thread::sleep(Duration::from_millis(800));
            let class = wide("CleanManager.CleanScreen");
            // SAFETY: read-only queries on a window found by class name.
            unsafe {
                let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
                assert!(!hwnd.is_null(), "clean-screen window not found");
                let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                GetWindowRect(hwnd, &mut r);
                let virt = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
                let topmost = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0;
                let visible = IsWindowVisible(hwnd) != 0;
                println!("window rect {:?} virtual screen {virt:?} topmost={topmost} visible={visible}", (r.left, r.top, r.right - r.left, r.bottom - r.top));
                ((r.left, r.top, r.right - r.left, r.bottom - r.top) == virt, topmost, visible)
            }
        });
        let t = std::time::Instant::now();
        assert_eq!(run(Duration::from_secs(3)).unwrap(), CleanScreenExit::Timeout);
        let took = t.elapsed();
        println!("timeout exit after {took:?}");
        assert_eq!(probe.join().unwrap(), (true, true, true));
        assert!(took >= Duration::from_millis(2900) && took < Duration::from_secs(5));
        assert!(!is_running());
        // A long Esc press (injected) ends it well before the 30 s timeout; a short one does not.
        let key = |up: bool| {
            let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_ESC, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 } } };
            // SAFETY: one keyboard input of the right size.
            unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
        };
        let presser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            key(false);
            std::thread::sleep(Duration::from_millis(500));
            key(true); // too short: ignored
            std::thread::sleep(Duration::from_millis(300));
            key(false);
            std::thread::sleep(Duration::from_millis(2600));
            key(true);
        });
        let t = std::time::Instant::now();
        let exit = run(Duration::from_secs(30)).unwrap();
        let took = t.elapsed();
        presser.join().unwrap();
        println!("esc exit after {took:?}: {exit:?}");
        assert_eq!(exit, CleanScreenExit::EscHeld);
        assert!(took >= Duration::from_millis(3300) && took < Duration::from_secs(6), "{took:?}");
    }

    #[test]
    fn only_one_session_at_a_time() {
        // The guard is process-wide; simulate a running session without opening a window.
        if RUNNING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            assert!(run(Duration::from_secs(5)).is_err());
            RUNNING.store(false, Ordering::SeqCst);
        }
    }
}
