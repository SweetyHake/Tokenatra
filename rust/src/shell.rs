//! GUI shell: wry/tao window with a pywebview bridge over the HTTP routes.
//! The frontend is unchanged; pywebview js_api is emulated by an init script.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::gui_control::GuiCommand;
use crate::state::AppState;

pub static FORCE_QUIT: AtomicBool = AtomicBool::new(false);
static EXIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static HAS_WINDOW: AtomicBool = AtomicBool::new(false);

pub fn has_window() -> bool {
    HAS_WINDOW.load(Ordering::SeqCst)
}

const PYWEBVIEW_SHIM: &str = r#"
(function () {
  if (window.pywebview) return;
  var post = function (a) { return fetch('/api/window/' + a, { method: 'POST' }).catch(function () {}); };
  window.pywebview = {
    api: {
      minimize: function () { return post('minimize'); },
      maximize: function () { return post('maximize'); },
      restore: function () { return post('restore'); },
      destroy: function () { return post('destroy'); },
      toggle_fullscreen: function () { return post('toggle_fullscreen'); },
      quit_app: function () { return post('quit'); },
      hide_to_tray: function () {
        return fetch('/api/window/hide_to_tray', { method: 'POST' })
          .then(function (r) { return r.json(); })
          .then(function (d) { return !!(d && d.hidden === true); })
          .catch(function () { return false; });
      }
    }
  };
})();
"#;

#[derive(Clone)]
pub enum UserEvent {
    Wake,
}

static PROXY: OnceLock<Mutex<Option<tao::event_loop::EventLoopProxy<UserEvent>>>> = OnceLock::new();

#[derive(Clone, Copy)]
struct FramelessResizeState {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
    orig_x: f64,
    orig_y: f64,
    orig_w: f64,
    orig_h: f64,
}

static RESIZE_STATE: OnceLock<Mutex<Option<FramelessResizeState>>> = OnceLock::new();

fn resize_state() -> &'static Mutex<Option<FramelessResizeState>> {
    RESIZE_STATE.get_or_init(|| Mutex::new(None))
}

pub fn wake() {
    if let Some(m) = PROXY.get()
        && let Ok(guard) = m.lock()
            && let Some(p) = guard.as_ref() {
                let _ = p.send_event(UserEvent::Wake);
            }
}

fn enter_idle_js() -> &'static str {
    "window.TokenTray && window.TokenTray.enterIdle && window.TokenTray.enterIdle(true)"
}

fn exit_idle_js() -> &'static str {
    "window.TokenTray && window.TokenTray.exitIdle && window.TokenTray.exitIdle()"
}

/// Taskbar/dock icon on Linux/macOS, loaded from logo.png like the tray one.
/// Windows skips this: the icon is set later via WM_SETICON from icon.ico
/// (win::set_window_icon), which also covers the titlebar and Alt+Tab.
#[cfg(not(windows))]
fn load_window_icon() -> Option<tao::window::Icon> {
    let logo = crate::paths::resource_dir().join("logo.png");
    let bytes = std::fs::read(logo).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let resized = image::imageops::resize(&img, 128, 128, image::imageops::FilterType::Lanczos3);
    tao::window::Icon::from_rgba(resized.into_raw(), 128, 128).ok()
}

pub fn run_gui(state: Arc<AppState>) -> i32 {
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;

    // WebView2 defaults its user data folder next to the exe; in Program
    // Files that is unwritable and environment creation fails with
    // E_ACCESSDENIED. Point the loader at a per-user folder instead (the
    // loader reads this variable when the options leave it unset).
    #[cfg(windows)]
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none_or(|v| v.is_empty())
        && let Some(base) = std::env::var_os("LOCALAPPDATA")
    {
        let dir = std::path::PathBuf::from(base).join("Tokenatra").join("WebView2");
        let _ = std::fs::create_dir_all(&dir);
        // Main thread, before the event loop spawns any other thread.
        unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &dir) };
    }

    let port = crate::consts::PORT;
    let splash_url = format!("http://localhost:{port}/splash");

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy_slot = PROXY.get_or_init(|| Mutex::new(None));
    *proxy_slot.lock().unwrap() = Some(event_loop.create_proxy());

    #[cfg(windows)]
    let window_icon = None;
    #[cfg(not(windows))]
    let window_icon = load_window_icon();

    let window = WindowBuilder::new()
        .with_title(crate::consts::APP_NAME)
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(800.0, 600.0))
        .with_resizable(true)
        // Unified custom titlebar: native decorations off, we handle buttons,
        // drag and resize ourselves (Windows: Win32 hook; Linux/macOS:
        // tao drag_window/drag_resize_window + edge resize from the frontend)
        .with_decorations(false)
        // Windows: hidden until titlebar and icon are set (like pywebview);
        // Linux/macOS show the window immediately
        .with_visible(!cfg!(windows))
        .with_window_icon(window_icon)
        .build(&event_loop)
        .expect("create window");

    HAS_WINDOW.store(true, Ordering::SeqCst);

    #[cfg(windows)]
    let webview = wry::WebViewBuilder::new()
        .with_url(&splash_url)
        .with_initialization_script(PYWEBVIEW_SHIM)
        .build_as_child(&window)
        .expect("create webview");

    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix as _;
        use wry::WebViewBuilderExtUnix as _;
        let vbox = window.default_vbox().expect("tao default_vbox");
        wry::WebViewBuilder::new()
            .with_url(&splash_url)
            .with_initialization_script(PYWEBVIEW_SHIM)
            .build_gtk(vbox)
            .expect("create webview")
    };

    #[cfg(not(any(windows, target_os = "linux")))]
    let webview = wry::WebViewBuilder::new()
        .with_url(&splash_url)
        .with_initialization_script(PYWEBVIEW_SHIM)
        .build(&window)
        .expect("create webview");

    // Windows: custom titlebar (dark frame, DWM NC off, WndProc hook)
    #[cfg(windows)]
    let (window, webview) = {
        use tao::platform::windows::WindowExtWindows as _;
        let hwnd = window.hwnd();
        crate::win::store_hwnd(hwnd);
        crate::win::customize_window(hwnd);
        crate::win::set_window_icon(hwnd, &crate::paths::resource_dir().join("icon.ico"));
        window.set_visible(true);
        // Register the webview for resize from the hook (WM_SIZE) and stretch
        // it right away: wry created the webview while the window was hidden
        // and captured a wrong client size; without this it stays a ~280px square.
        #[allow(clippy::arc_with_non_send_sync)] // wry::WebView is !Sync, hook runs on this thread
        let webview = Arc::new(webview);
        crate::win::set_resize_target(webview.clone());
        crate::win::resize_webview_to_client();
        (window, webview)
    };
    #[cfg(not(windows))]
    #[allow(clippy::arc_with_non_send_sync)]
    let (window, webview) = (window, Arc::new(webview));

    // ===== Tray: the icon lives for the whole app lifetime =====
    let mut tray: Option<crate::tray::Tray> = None;
    #[cfg(target_os = "macos")]
    let mut macos_menu: Option<muda::Menu> = None;

    let window = Arc::new(window);
    let st = state.clone();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(tao::event::StartCause::Init) => {
                #[cfg(target_os = "macos")]
                {
                    macos_menu = crate::tray::init_macos_app_menu();
                    let _ = &macos_menu;
                }
                tray = crate::tray::create(&st);
            }
            Event::UserEvent(UserEvent::Wake) => {
                while let Ok(cmd) = crate::gui_control::try_recv() {
                    handle_gui_command(cmd, &window, &webview, &st, &tray);
                }
                if EXIT_REQUESTED.load(Ordering::SeqCst) {
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                // Sync the DOM is-maximized class with the real window state:
                // maximize can come from outside the button (Aero Snap, Win+Up,
                // drag detach) and a stale class inverts the button's next
                // action (restore on a windowed window and vice versa).
                let _ = size;
                let maximized = window.is_maximized();
                let _ = webview.evaluate_script(&format!(
                    "document.documentElement.classList.toggle('is-maximized', {});",
                    maximized
                ));
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                let (close_to_tray, _) = crate::config::tray_settings();
                if close_to_tray && tray.is_some() {
                    window.set_visible(false);
                    st.set_tray_mode(true);
                    let _ = webview.evaluate_script(enter_idle_js());
                    return;
                }
                FORCE_QUIT.store(true, Ordering::SeqCst);
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::LoopDestroyed => {
                #[cfg(target_os = "macos")]
                let _ = &macos_menu;
                if let Some(t) = tray.as_ref() {
                    crate::tray::hide(t);
                }
                final_exit();
            }
            _ => {}
        }
    });
}

fn handle_gui_command(
    cmd: GuiCommand,
    window: &Arc<tao::window::Window>,
    webview: &Arc<wry::WebView>,
    state: &Arc<AppState>,
    tray: &Option<crate::tray::Tray>,
) {
    match cmd {
        GuiCommand::Minimize => window.set_minimized(true),
        GuiCommand::Maximize => {
            // The maximize/restore buttons must not fight the F11 fullscreen
            // state: cancel fullscreen first or it leaks (the next F11 would
            // "exit" a fullscreen the user never saw and teleport the window).
            #[cfg(windows)]
            if crate::win::is_fullscreen()
                && let Some(hwnd) = crate::win::hwnd() {
                    crate::win::toggle_fullscreen(hwnd);
                }
            window.set_maximized(true)
        }
        GuiCommand::Restore => {
            #[cfg(windows)]
            if crate::win::is_fullscreen()
                && let Some(hwnd) = crate::win::hwnd() {
                    crate::win::toggle_fullscreen(hwnd);
                }
            window.set_maximized(false);
        }
        GuiCommand::Destroy | GuiCommand::Quit => {
            FORCE_QUIT.store(true, Ordering::SeqCst);
            EXIT_REQUESTED.store(true, Ordering::SeqCst);
        }
        GuiCommand::Show => {
            // the window may be hidden or minimized from the tray; restore it
            if window.is_minimized() {
                window.set_minimized(false);
            }
            if !window.is_visible() {
                window.set_visible(true);
            }
            window.set_focus();
            #[cfg(windows)]
            if let Some(hwnd) = crate::win::hwnd() {
                crate::win::bring_to_front(hwnd);
            }
            state.set_tray_mode(false);
            // the DPI or resolution may have changed while hidden to the tray
            #[cfg(windows)]
            crate::win::resize_webview_to_client();
            let _ = webview.evaluate_script(exit_idle_js());
        }
        GuiCommand::ModelStateChanged(loaded) => {
            if let Some(t) = tray.as_ref() {
                t.set_model_loaded(loaded);
            }
        }
        GuiCommand::ToggleFullscreen => {
            // Linux/macOS: native tao borderless works fine.
            #[cfg(windows)]
            if let Some(hwnd) = crate::win::hwnd() {
                crate::win::toggle_fullscreen(hwnd);
            }
            #[cfg(not(windows))]
            {
                use tao::window::Fullscreen;
                if window.fullscreen().is_some() {
                    window.set_fullscreen(None);
                } else {
                    window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                }
            }
        }
        GuiCommand::DragWindow => {
            let _ = window.drag_window();
        }
        GuiCommand::DragResize(direction) => {
            let _ = window.drag_resize_window(direction);
        }
        GuiCommand::ResizeBegin {
            left,
            right,
            top,
            bottom,
        } => {
            let scale = window.scale_factor();
            let pos = window
                .outer_position()
                .map(|p| p.to_logical::<f64>(scale))
                .unwrap_or(tao::dpi::LogicalPosition::new(0.0, 0.0));
            let size = window.inner_size().to_logical::<f64>(scale);
            *resize_state().lock().unwrap() = Some(FramelessResizeState {
                left,
                right,
                top,
                bottom,
                orig_x: pos.x,
                orig_y: pos.y,
                orig_w: size.width,
                orig_h: size.height,
            });
        }
        GuiCommand::ResizeMove { dx, dy } => {
            if let Some(st) = *resize_state().lock().unwrap() {
                let min_w = 800.0_f64;
                let min_h = 600.0_f64;
                let mut new_x = st.orig_x;
                let mut new_y = st.orig_y;
                let mut new_w = st.orig_w;
                let mut new_h = st.orig_h;
                if st.right {
                    new_w = (st.orig_w + dx).max(min_w);
                } else if st.left {
                    new_w = (st.orig_w - dx).max(min_w);
                    new_x = st.orig_x + (st.orig_w - new_w);
                }
                if st.bottom {
                    new_h = (st.orig_h + dy).max(min_h);
                } else if st.top {
                    new_h = (st.orig_h - dy).max(min_h);
                    new_y = st.orig_y + (st.orig_h - new_h);
                }
                window.set_inner_size(tao::dpi::LogicalSize::new(new_w, new_h));
                if st.left || st.top {
                    window.set_outer_position(tao::dpi::LogicalPosition::new(new_x, new_y));
                }
            }
        }
        GuiCommand::ResizeEnd => {
            *resize_state().lock().unwrap() = None;
        }
        GuiCommand::HideToTray(tx) => {
            let (close_to_tray, _) = crate::config::tray_settings();
            let ok = close_to_tray && tray.is_some();
            if ok {
                window.set_visible(false);
                state.set_tray_mode(true);
                let _ = webview.evaluate_script(enter_idle_js());
            }
            let _ = tx.send(ok);
        }
    }
}

fn final_exit() {
    #[cfg(windows)]
    {
        crate::win::unregister_context_menu();
    }
    #[cfg(target_os = "linux")]
    {
        crate::unregister_context_menu_linux();
    }
    let _ = std::fs::remove_file(crate::paths::pid_file());
}
