#![cfg(windows)]
//! Windows specifics: custom titlebar (DWM + WndProc), DXGI, Explorer
//! context menu, WebView2 GPU settings, dark dialogs, port release.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIFactory6, DXGI_ADAPTER_FLAG_SOFTWARE,
    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::*;

static HWND_STORED: AtomicIsize = AtomicIsize::new(0);

pub fn store_hwnd(hwnd: isize) {
    HWND_STORED.store(hwnd, Ordering::SeqCst);
}

/// HWND wrapper for moving into blocking threads (resize_loop).
pub struct SendHwnd(pub HWND);
unsafe impl Send for SendHwnd {}

pub fn hwnd() -> Option<HWND> {
    let h = HWND_STORED.load(Ordering::SeqCst);
    if h != 0 {
        Some(HWND(h as *mut _))
    } else {
        None
    }
}

// ===== WndProc: NCCALCSIZE/NCHITTEST + WebView drag =====

static ORIG_WNDPROC: AtomicIsize = AtomicIsize::new(0);

// Webview for resize from the hook. The hook always runs on the event
// loop thread where the Arc<WebView> was created, so thread_local is
// safe and avoids requiring Sync from the wry COM wrapper.
thread_local! {
    static RESIZE_TARGET: std::cell::RefCell<Option<std::sync::Arc<wry::WebView>>> =
        const { std::cell::RefCell::new(None) };
}

/// Registers the webview that hook_proc stretches on WM_SIZE.
pub fn set_resize_target(webview: std::sync::Arc<wry::WebView>) {
    RESIZE_TARGET.with(|c| *c.borrow_mut() = Some(webview));
}

/// Stretches the webview over the parent client area (called from the
/// hook on WM_SIZE and from shell on window show). False if no target.
pub fn resize_webview_to_client() -> bool {
    let Some(hwnd) = hwnd() else { return false };
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return false;
    }
    let w = (rect.right - rect.left).max(0) as u32;
    let h = (rect.bottom - rect.top).max(0) as u32;
    RESIZE_TARGET.with(|c| {
        if let Some(wv) = c.borrow().as_ref() {
            let _ = wv.set_bounds(wry::Rect {
                position: tao::dpi::PhysicalPosition::new(0, 0).into(),
                size: tao::dpi::PhysicalSize::new(w, h).into(),
            });
            return true;
        }
        false
    })
}

pub fn post_drag_message(hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_APP + 1, WPARAM(0), LPARAM(0));
    }
}

/// Restores a hidden/minimized window and brings it to the foreground reliably on Windows.
pub fn bring_to_front(hwnd: HWND) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let fg = GetForegroundWindow();
        if fg != hwnd {
            let fg_tid = if !fg.0.is_null() {
                GetWindowThreadProcessId(fg, None)
            } else {
                0
            };
            let cur_tid = GetCurrentThreadId();
            let attached = fg_tid != 0
                && fg_tid != cur_tid
                && AttachThreadInput(cur_tid, fg_tid, true).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(cur_tid, fg_tid, false);
            }
        }
    }
}

// ===== F11 fullscreen =====
// tao Borderless keeps WS_CAPTION and does not raise the window above
// the taskbar: Windows does not treat it as fullscreen. Real Win32
// transition here: drop WS_CAPTION|WS_THICKFRAME, stretch over the
// monitor; on exit restore styles, borders and maximized state.
// State lives only on the event loop thread.

struct FullscreenState {
    rect: RECT,
    style: isize,
    zoomed: bool,
}

static FULLSCREEN: Mutex<Option<FullscreenState>> = Mutex::new(None);

pub fn is_fullscreen() -> bool {
    FULLSCREEN.lock().map(|s| s.is_some()).unwrap_or(false)
}

pub fn toggle_fullscreen(hwnd: HWND) {
    unsafe {
        if let Some(prev) = FULLSCREEN.lock().unwrap().take() {
            // exit: restore styles, borders and (if it was) maximized state
            let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, prev.style);
            let _ = SetWindowPos(
                hwnd,
                None,
                prev.rect.left,
                prev.rect.top,
                prev.rect.right - prev.rect.left,
                prev.rect.bottom - prev.rect.top,
                SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if prev.zoomed {
                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            }
        } else {
            // If maximized: restore first so tao/Windows agree on the base
            // placement; otherwise stripping WS_MAXIMIZE behind tao's back
            // desyncs its maximized flag and the next maximize/restore goes
            // crooked. The pre-restore zoomed flag is kept so exit can
            // re-apply maximized.
            let zoomed = IsZoomed(hwnd).as_bool();
            if zoomed {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return;
            }
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            *FULLSCREEN.lock().unwrap() = Some(FullscreenState { rect, style, zoomed });
            let _ = SetWindowLongPtrW(
                hwnd,
                GWL_STYLE,
                style
                    & !(WS_CAPTION.0 as isize
                        | WS_THICKFRAME.0 as isize
                        // otherwise IsZoomed() and the maximized NCCALCSIZE inset would eat
                        // 10px around the fullscreen frame
                        | WS_MAXIMIZE.0 as isize),
            );
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
                let rc = mi.rcMonitor;
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_FRAMECHANGED,
                );
            }
        }
        let _ = resize_webview_to_client();
    }
}

unsafe extern "system" fn hook_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT { unsafe {
    let is_zoomed = IsZoomed(hwnd).as_bool();
    if msg == WM_NCHITTEST && !is_zoomed && !is_fullscreen() {
        let x = (lp.0 & 0xFFFF) as i16 as i32;
        let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let edge = 6;
            let top = y < rect.top + edge;
            let bottom = y >= rect.bottom - edge;
            let left = x < rect.left + edge;
            let right = x >= rect.right - edge;
            let ht = if top && left {
                HTTOPLEFT
            } else if top && right {
                HTTOPRIGHT
            } else if bottom && left {
                HTBOTTOMLEFT
            } else if bottom && right {
                HTBOTTOMRIGHT
            } else if left {
                HTLEFT
            } else if right {
                HTRIGHT
            } else if top {
                HTTOP
            } else if bottom {
                HTBOTTOM
            } else {
                0
            };
            if ht != 0 {
                return LRESULT(ht as isize);
            }
        }
    }
    if msg == WM_NCCALCSIZE && wp.0 != 0 {
        // Full client area: WebView2 covers the whole window. On maximize
        // Windows inflates the window by the invisible frame width;
        // compensate or the titlebar top moves off-screen.
        if IsZoomed(hwnd).as_bool() {
            let frame =
                GetSystemMetrics(SM_CXSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
            if let Some(r) = (lp.0 as *mut RECT).as_mut() {
                r.left += frame;
                r.top += frame;
                r.right -= frame;
                r.bottom -= frame;
            }
        }
        return LRESULT(0);
    }
    if msg == WM_SIZE && wp.0 != SIZE_MINIMIZED as usize {
        // the wry subclass does not get WM_SIZE through our hook chain;
        // stretch the webview ourselves (our hook is first in the chain)
        resize_webview_to_client();
    }
    if msg == WM_DPICHANGED {
        // move to the monitor-suggested rect so the client rect and WebView2
        // DPI stay in sync; WM_SIZE fires afterwards and stretches the webview
        if let Some(r) = (lp.0 as *const RECT).as_ref() {
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    if msg == WM_DISPLAYCHANGE && is_fullscreen() {
        // resolution changed while in custom fullscreen: re-fit to the
        // current monitor rect (the stored rect is restored on exit as-is)
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
            let rc = mi.rcMonitor;
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                rc.left,
                rc.top,
                rc.right - rc.left,
                rc.bottom - rc.top,
                SWP_FRAMECHANGED,
            );
        }
        let _ = resize_webview_to_client();
    }
    if msg == WM_APP + 1 {
        // Native modal move loop instead of a manual SetCapture loop: the
        // system moves the window, so Aero Snap (edge snap to half screen,
        // up to maximize, Win11 Snap Layouts flyout) and dragging a maximized
        // window work. ReleaseCapture is required: without it WebView2 keeps
        // mouse capture and the loop sticks without WM_LBUTTONUP.
        use windows::Win32::UI::WindowsAndMessaging::{HTCAPTION, SC_MOVE};
        let swap = GetSystemMetrics(SM_SWAPBUTTON);
        let vk_primary = if swap != 0 { VK_RBUTTON } else { VK_LBUTTON };
        if (GetAsyncKeyState(vk_primary.0 as i32) as u32 & 0x8000) != 0 {
            let _ = ReleaseCapture();
            SendMessageW(
                hwnd,
                WM_SYSCOMMAND,
                Some(WPARAM((SC_MOVE | HTCAPTION) as usize)),
                Some(LPARAM(0)),
            );
        }
        return LRESULT(0);
    }
    let orig = ORIG_WNDPROC.load(Ordering::SeqCst);
    if orig != 0 {
        CallWindowProcW(
            std::mem::transmute::<isize, WNDPROC>(orig),
            hwnd,
            msg,
            wp,
            lp,
        )
    } else {
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}}

/// Applies the custom titlebar: dark frame, hides NC rendering, hook.
pub fn customize_window(hwnd_val: isize) {
    let hwnd = HWND(hwnd_val as *mut _);
    unsafe {
        // Dark title bar (DWMWA_USE_IMMERSIVE_DARK_MODE)
        let dark: i32 = 1;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(20),
            &dark as *const i32 as *const _,
            4,
        );
        // Hide DWM NC rendering so our content shows through
        let disable: i32 = 1;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(2),
            &disable as *const i32 as *const _,
            4,
        );
        // Subclass
        let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, hook_proc as *const () as isize);
        if prev != 0 {
            ORIG_WNDPROC.store(prev, Ordering::SeqCst);
        }
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Window icon from icon.ico (taskbar 16px, Alt+Tab 32px).
pub fn set_window_icon(hwnd_val: isize, icon_path: &std::path::Path) {
    let hwnd = HWND(hwnd_val as *mut _);
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_LOADFROMFILE, SM_CXICON,
        SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
    };
    unsafe {
        let pathw =
            windows::core::HSTRING::from(icon_path.to_string_lossy().as_ref());
        let null_handle = windows::Win32::Foundation::HANDLE(std::ptr::null_mut());
        let cx = GetSystemMetrics(SM_CXICON);
        let cy = GetSystemMetrics(SM_CYICON);
        let big = LoadImageW(
            None,
            PCWSTR(pathw.as_ptr()),
            IMAGE_ICON,
            cx,
            cy,
            LR_LOADFROMFILE,
        )
        .unwrap_or(null_handle);
        let cx = GetSystemMetrics(SM_CXSMICON);
        let cy = GetSystemMetrics(SM_CYSMICON);
        let small = LoadImageW(
            None,
            PCWSTR(pathw.as_ptr()),
            IMAGE_ICON,
            cx,
            cy,
            LR_LOADFROMFILE,
        )
        .unwrap_or(null_handle);
        let big = if big.is_invalid() { small } else { big };
        if !big.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(big.0 as isize)),
            );
        }
        if !small.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(small.0 as isize)),
            );
        }
    }
}

/// Modal resize loop (/api/window/resize_start).
pub fn resize_loop(h: SendHwnd, edges: (bool, bool, bool, bool)) {
    let hwnd = h.0;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowRect, IsWindowVisible, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
    };
    let (eleft, eright, etop, ebottom) = edges;
    unsafe {
        let mut start_rect = RECT::default();
        if GetWindowRect(hwnd, &mut start_rect).is_err() {
            return;
        }
        let mut start_cursor = POINT::default();
        if GetCursorPos(&mut start_cursor).is_err() {
            return;
        }
        let (min_width, min_height) = (800i32, 600i32);

        while (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u32 & 0x8000) != 0 {
            let mut cursor = POINT::default();
            if GetCursorPos(&mut cursor).is_err() {
                break;
            }
            let dx = cursor.x - start_cursor.x;
            let dy = cursor.y - start_cursor.y;

            let mut left = start_rect.left + if eleft { dx } else { 0 };
            let mut right = start_rect.right + if eright { dx } else { 0 };
            let mut top = start_rect.top + if etop { dy } else { 0 };
            let mut bottom = start_rect.bottom + if ebottom { dy } else { 0 };

            if right - left < min_width {
                if eleft {
                    left = right - min_width;
                } else {
                    right = left + min_width;
                }
            }
            if bottom - top < min_height {
                if etop {
                    top = bottom - min_height;
                } else {
                    bottom = top + min_height;
                }
            }
            let _ = SetWindowPos(
                hwnd,
                None,
                left,
                top,
                right - left,
                bottom - top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        let _ = SIZE::default();
        let _ = IsWindowVisible(hwnd);
    }
}

// ===== DXGI =====

#[derive(Clone, Debug)]
pub struct DxgiAdapterInfo {
    pub index: i32,
    pub name: String,
    pub vendor_id: u32,
    pub dedicated_vram: u64,
    pub is_software: bool,
    pub is_high_perf_first: bool,
}

fn is_virtual_adapter_name(name: &str) -> bool {
    let low = name.to_lowercase();
    const VIRTUAL_KEYWORDS: &[&str] = &[
        "microsoft basic",
        "microsoft remote",
        "hyper-v",
        "warp",
        "parsec",
        "virtual",
        "vmware",
        "vbox",
        "qxl",
        "citrix",
        "spacedesk",
        "indirect",
        "display only",
        "rdp",
        "teamviewer",
        "anydesk",
        "software renderer",
        "basic render",
        "basic display",
    ];
    VIRTUAL_KEYWORDS.iter().any(|kw| low.contains(kw))
}

pub fn enumerate_dxgi_adapters_detailed() -> Option<Vec<DxgiAdapterInfo>> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;

        // Ask Windows DXGI 1.6 which hardware GPU is preferred for HighPerformance
        let mut high_perf_luid: Option<(i32, u32)> = None;
        if let Ok(factory6) = factory.cast::<IDXGIFactory6>() {
            for pref_idx in 0..8 {
                let Ok(adapter) = factory6.EnumAdapterByGpuPreference::<IDXGIAdapter1>(
                    pref_idx,
                    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
                ) else {
                    break;
                };
                if let Ok(desc) = adapter.GetDesc1() {
                    let sw_flag = (desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32)) != 0
                        || desc.VendorId == 0x1414;
                    let name_len = desc
                        .Description
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(desc.Description.len());
                    let name = String::from_utf16_lossy(&desc.Description[..name_len]);
                    if !sw_flag && !is_virtual_adapter_name(name.trim()) {
                        high_perf_luid =
                            Some((desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart));
                        break;
                    }
                }
            }
        }

        let mut out = Vec::new();
        for i in 0..32 {
            let adapter = match factory.EnumAdapters1(i) {
                Ok(a) => a,
                Err(_) => break,
            };
            if let Ok(desc) = adapter.GetDesc1() {
                let name_len = desc
                    .Description
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(desc.Description.len());
                let name = String::from_utf16_lossy(&desc.Description[..name_len])
                    .trim()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let sw_flag = (desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32)) != 0
                    || desc.VendorId == 0x1414;
                let is_software = sw_flag || is_virtual_adapter_name(&name);
                let luid = (desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart);
                out.push(DxgiAdapterInfo {
                    index: i as i32,
                    name,
                    vendor_id: desc.VendorId,
                    dedicated_vram: desc.DedicatedVideoMemory as u64,
                    is_software,
                    is_high_perf_first: !is_software && high_perf_luid == Some(luid),
                });
            }
        }
        Some(out)
    }
}

pub fn cpu_name_from_registry() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .ok()?;
    let name: String = key.get_value("ProcessorNameString").ok()?;
    let trimmed = name.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

// ===== AppUserModelID / dark dialogs / WebView2 GPU =====

pub fn set_taskbar_identity() {
    unsafe {
        let id = windows::core::w!("Tokenatra.App");
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(id);
    }
}

type PamFn = unsafe extern "system" fn(i32) -> i32;
type FlushFn = unsafe extern "system" fn() -> i32;

pub fn enable_windows_dark_dialogs() {
    unsafe {
        let ux = windows::Win32::System::LibraryLoader::GetModuleHandleW(windows::core::w!("uxtheme.dll"));
        let Ok(ux) = ux else { return };
        let make = |ord: u16| -> Option<*mut core::ffi::c_void> {
            let name = windows::core::PCSTR(ord as usize as *const u8);
            windows::Win32::System::LibraryLoader::GetProcAddress(ux, name).map(|p| p as *mut _)
        };
        // functions exported by ordinals 135/136 on current builds
        let pam = make(135)
            .or_else(|| windows::Win32::System::LibraryLoader::GetProcAddress(ux, windows::core::s!("SetPreferredAppMode")).map(|p| p as *mut _));
        let flush = make(136)
            .or_else(|| windows::Win32::System::LibraryLoader::GetProcAddress(ux, windows::core::s!("FlushMenuThemes")).map(|p| p as *mut _));
        if let Some(p) = pam {
            let f: PamFn = std::mem::transmute(p);
            f(1); // PAM_ALLOW
        }
        if let Some(p) = flush {
            let f: FlushFn = std::mem::transmute(p);
            f();
        }
    }
}

pub fn setup_webview2_gpu() {
    // WebView2 hardware acceleration (ANGLE d3d11) via official args
    let args = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
    let mut args = args;
    for sw in ["--use-angle=d3d11", "--enable-gpu-rasterization"] {
        if !args.contains(sw) {
            args = format!("{args} {sw}").trim().to_string();
        }
    }
    unsafe { std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", args) };

    // GpuPreference=2 by FULL process path (app + msedgewebview2)
    use winreg::enums::*;
    use winreg::RegKey;
    let set_pref = |exe: &str| -> anyhow::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu.create_subkey(r"Software\Microsoft\DirectX\UserGpuPreferences")?.0;
        key.set_value(exe, &"GpuPreference=2;")?;
        Ok(())
    };
    if let Ok(exe) = std::env::current_exe() {
        let _ = set_pref(&exe.to_string_lossy());
    }
    let clients_keys = [
        r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-27E8-4D08-8A21-E6FBEFF3DFB0}",
        r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-27E8-4D08-8A21-E6FBEFF3DFB0}",
    ];
    let mut versions: Vec<String> = Vec::new();
    for hive in [RegKey::predef(HKEY_LOCAL_MACHINE), RegKey::predef(HKEY_CURRENT_USER)] {
        for sub in clients_keys {
            if let Ok(k) = hive.open_subkey(sub)
                && let Ok(ver) = k.get_value::<String, _>("pv")
                    && !ver.is_empty() && !versions.contains(&ver) {
                        versions.push(ver);
                    }
        }
    }
    let webview_root = r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application";
    let mut found = std::collections::BTreeSet::new();
    for v in &versions {
        let exe = format!(r"{webview_root}\{v}\msedgewebview2.exe");
        if std::path::Path::new(&exe).is_file() {
            found.insert(exe);
        }
    }
    if let Ok(rd) = std::fs::read_dir(webview_root) {
        for e in rd.filter_map(|e| e.ok()) {
            let exe = e.path().join("msedgewebview2.exe");
            if exe.is_file() {
                found.insert(exe.to_string_lossy().to_string());
            }
        }
    }
    for exe in found {
        let _ = set_pref(&exe);
    }
}

// ===== Explorer context menu =====

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "tif"];
const MEDIA_EXTS: &[&str] = &[
    "mp3", "wav", "m4a", "flac", "aac", "wma", "opus", "ogg", "mp4", "mkv", "mov", "avi",
    "webm", "mpeg", "mpg", "m4v",
];

fn file_reg_entries() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("Tokenatra_RemoveBg", "Удалить фон", "--remove-bg"),
        ("Tokenatra_ToWebp", "Конвертировать в WebP", "--to-webp"),
    ]
}

fn media_reg_entries() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![("Tokenatra_ToOgg", "Конвертировать в OGG", "--to-ogg")]
}

fn dir_reg_entries() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            r"Software\Classes\Directory\shell\Tokenatra_FolderToWebp",
            "Конвертировать все изображения в WebP",
            "--folder-to-webp",
        ),
        (
            r"Software\Classes\Directory\Background\shell\Tokenatra_FolderToWebp",
            "Конвертировать все изображения в WebP",
            "--folder-to-webp",
        ),
    ]
}

const SFA_IMAGE: &str = r"Software\Classes\SystemFileAssociations\image\shell";
const LEGACY_REG_ENTRIES: &[&str] = &[
    r"Software\Classes\*\shell\Tokenatra_RemoveBg",
    r"Software\Classes\*\shell\Tokenatra_ToWebp",
    r"Software\Classes\CLSID\{B2ED14AF-9138-42A4-AA80-386EEAD2F219}",
    r"Software\Classes\CLSID\{D10C6CD7-4123-4D99-A508-F6FCB5565B47}",
];

fn applies_to(exts: &[&str]) -> String {
    let mut sorted: Vec<&str> = exts.to_vec();
    sorted.sort();
    sorted
        .iter()
        .map(|e| format!("System.FileExtension:\".{e}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn delete_verb(root: &winreg::RegKey, reg_path: &str) {
    let _ = root.delete_subkey(format!("{reg_path}\\command").as_str());
    let _ = root.delete_subkey(reg_path);
}

fn set_verb(
    root: &winreg::RegKey,
    reg_path: &str,
    label: &str,
    icon_path: &str,
    cmd: &str,
    extra: &[(&str, &str)],
) -> anyhow::Result<()> {
    
    let (key, _) = root.create_subkey(reg_path)?;
    key.set_value("", &label)?;
    key.set_value("Icon", &icon_path)?;
    for (n, v) in extra {
        key.set_value(*n, v)?;
    }
    let (cmd_key, _) = root.create_subkey(format!("{reg_path}\\command"))?;
    cmd_key.set_value("", &cmd)?;
    Ok(())
}

pub fn register_context_menu() {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    let (cmd_prefix, icon_path) = match std::env::current_exe() {
        Ok(exe) => {
            let s = exe.to_string_lossy().to_string();
            (format!("\"{s}\""), s)
        }
        Err(_) => return,
    };

    // clean up keys from older versions
    for reg_path in LEGACY_REG_ENTRIES {
        for root in [&hkcu, &hklm] {
            if reg_path.contains("CLSID") {
                let _ = root.delete_subkey_all(reg_path);
            } else {
                delete_verb(root, reg_path);
            }
        }
    }
    for ext in IMAGE_EXTS {
        for (name, _, _) in file_reg_entries() {
            delete_verb(&hkcu, &format!(r"Software\Classes\.{ext}\shell\{name}"));
        }
    }
    for root in [&hkcu, &hklm] {
        for (name, _, _) in file_reg_entries().iter().chain(media_reg_entries().iter()) {
            delete_verb(root, &format!(r"Software\Classes\*\shell\{name}"));
            delete_verb(root, &format!(r"{SFA_IMAGE}\{name}"));
        }
    }

    // verbs on *\shell + AppliesTo (Windows 11 25H2)
    for (entries, applies) in [
        (file_reg_entries(), applies_to(IMAGE_EXTS)),
        (media_reg_entries(), applies_to(MEDIA_EXTS)),
    ] {
        for (name, label, flag) in entries {
            let reg_path = format!(r"Software\Classes\*\shell\{name}");
            let _ = set_verb(
                &hkcu,
                &reg_path,
                label,
                &icon_path,
                &format!("{cmd_prefix} {flag} \"%1\""),
                &[("AppliesTo", applies.as_str()), ("MultiSelectModel", "Player")],
            );
        }
    }
    for (reg_path, label, flag) in dir_reg_entries() {
        let arg = if reg_path.contains("Background") {
            "\"%V\""
        } else {
            "\"%1\""
        };
        let _ = set_verb(
            &hkcu,
            reg_path,
            label,
            &icon_path,
            &format!("{cmd_prefix} {flag} {arg}"),
            &[],
        );
    }
}

pub fn unregister_context_menu() {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for root in [&hkcu, &hklm] {
        for reg_path in LEGACY_REG_ENTRIES {
            if reg_path.contains("CLSID") {
                let _ = root.delete_subkey_all(reg_path);
            } else {
                delete_verb(root, reg_path);
            }
        }
        for (name, _, _) in file_reg_entries().iter().chain(media_reg_entries().iter()) {
            delete_verb(root, &format!(r"Software\Classes\*\shell\{name}"));
            delete_verb(root, &format!(r"{SFA_IMAGE}\{name}"));
        }
        for (reg_path, _, _) in dir_reg_entries() {
            delete_verb(root, reg_path);
        }
        for ext in IMAGE_EXTS {
            for (name, _, _) in file_reg_entries() {
                delete_verb(root, &format!(r"Software\Classes\.{ext}\shell\{name}"));
            }
        }
    }
}

/// Frees the port from a previous Tokenatra instance (netstat + terminate_process).
pub fn kill_process_on_port(port: u16) {
    use std::process::{Command, Stdio};
    let target_loopback = format!("127.0.0.1:{port}");
    let target_any = format!("0.0.0.0:{port}");
    let out = (|| -> Option<String> {
        let mut c = Command::new("netstat");
        c.args(["-ano"]).stdout(Stdio::piped()).stderr(Stdio::null());
        c.creation_flags(0x0800_0000);
        Some(String::from_utf8_lossy(&c.output().ok()?.stdout).to_string())
    })()
    .unwrap_or_default();
    let my_pid = std::process::id();
    for line in out.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Proto Local_Address Foreign_Address State PID
        if cols.len() >= 5
            && cols[3] == "LISTENING"
            && (cols[1] == target_loopback || cols[1] == target_any)
            && let Ok(pid) = cols[4].parse::<u32>()
            && pid != 0
            && pid != my_pid
        {
            terminate_process(pid);
        }
    }
}

/// Kills a Tokenatra process by PID (verifies image name via tasklist first).
pub fn terminate_process(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    use std::process::{Command, Stdio};
    let is_tokenatra = (|| -> bool {
        let mut chk = Command::new("tasklist");
        chk.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        chk.creation_flags(0x0800_0000);
        let Ok(out) = chk.output() else { return false };
        String::from_utf8_lossy(&out.stdout)
            .to_lowercase()
            .contains("tokenatra")
    })();
    if !is_tokenatra {
        return false;
    }
    let mut c = Command::new("taskkill");
    c.args(["/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c.creation_flags(0x0800_0000);
    c.output().is_ok()
}

trait CommandExt {
    fn creation_flags(&mut self, flags: u32) -> &mut Self;
}

impl CommandExt for std::process::Command {
    fn creation_flags(&mut self, flags: u32) -> &mut Self {
        std::os::windows::process::CommandExt::creation_flags(self, flags);
        self
    }
}
