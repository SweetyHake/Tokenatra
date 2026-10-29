//! GUI-оболочка: wry/tao окно + мост window.pywebview поверх HTTP-роутов.
//! Фронтенд не меняется ни на байт: js_api pywebview эмулируется init-скриптом.

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

pub fn wake() {
    if let Some(m) = PROXY.get() {
        if let Ok(guard) = m.lock() {
            if let Some(p) = guard.as_ref() {
                let _ = p.send_event(UserEvent::Wake);
            }
        }
    }
}

fn enter_idle_js() -> &'static str {
    "window.TokenTray && window.TokenTray.enterIdle && window.TokenTray.enterIdle(true)"
}

fn exit_idle_js() -> &'static str {
    "window.TokenTray && window.TokenTray.exitIdle && window.TokenTray.exitIdle()"
}

pub fn run_gui(state: Arc<AppState>) -> i32 {
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;

    let port = crate::consts::PORT;
    let splash_url = format!("http://localhost:{port}/splash");

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy_slot = PROXY.get_or_init(|| Mutex::new(None));
    *proxy_slot.lock().unwrap() = Some(event_loop.create_proxy());

    let window = WindowBuilder::new()
        .with_title(crate::consts::APP_NAME)
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(800.0, 600.0))
        .with_resizable(true)
        // Единый кастомный тайтлбар на всех ОС: нативные декорации выключены,
        // кнопки/перетаскивание/ресайз — наши (Windows: Win32-хук; Linux/macOS:
        // tao drag_window/drag_resize_window + краевой ресайз из фронтенда)
        .with_decorations(false)
        // Windows: прячем до настройки тайтлбара/иконки (как pywebview);
        // Linux/macOS — окно видно сразу
        .with_visible(!cfg!(windows))
        .build(&event_loop)
        .expect("create window");

    HAS_WINDOW.store(true, Ordering::SeqCst);

    let webview = wry::WebViewBuilder::new()
        .with_url(&splash_url)
        .with_initialization_script(PYWEBVIEW_SHIM)
        .build_as_child(&window)
        .expect("create webview");

    // Windows: кастомный тайтлбар (тёмная рамка, DWM NC off, WndProc-хук)
    #[cfg(windows)]
    let (window, webview) = {
        use tao::platform::windows::WindowExtWindows as _;
        let hwnd = window.hwnd();
        crate::win::store_hwnd(hwnd);
        crate::win::customize_window(hwnd);
        crate::win::set_window_icon(hwnd, &crate::paths::resource_dir().join("icon.ico"));
        window.set_visible(true);
        // Регистрируем вебвью для ресайза из хука (WM_SIZE) и сразу
        // растягиваем: wry создал вебвью при СКРЫТОМ окне и снял неверный
        // клиентский размер — без этого child остаётся ~280px квадратом.
        let webview = Arc::new(webview);
        crate::win::set_resize_target(webview.clone());
        crate::win::resize_webview_to_client();
        (window, webview)
    };
    #[cfg(not(windows))]
    let (window, webview) = (window, Arc::new(webview));

    // ===== Трей: иконка живёт весь цикл приложения (как у Discord) =====
    #[cfg(windows)]
    let tray = crate::tray::create(&state);
    #[cfg(not(windows))]
    let tray: Option<()> = None;

    let window = Arc::new(window);
    let st = state.clone();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(UserEvent::Wake) => {
                #[cfg(windows)]
                if let Some(t) = tray.as_ref() {
                    crate::tray::drain_model_labels(t);
                }
                while let Ok(cmd) = crate::gui_control::try_recv() {
                    handle_gui_command(cmd, &window, &webview, &st, &tray);
                }
                if EXIT_REQUESTED.load(Ordering::SeqCst) {
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                #[cfg(windows)]
                {
                    let (close_to_tray, _) = crate::config::tray_settings();
                    if close_to_tray && tray.is_some() {
                        window.set_visible(false);
                        st.set_tray_mode(true);
                        let _ = webview.evaluate_script(enter_idle_js());
                        return;
                    }
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
                #[cfg(windows)]
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
    #[cfg(windows)] tray: &Option<crate::tray::Tray>,
    #[cfg(not(windows))] tray: &Option<()>,
) {
    match cmd {
        GuiCommand::Minimize => window.set_minimized(true),
        GuiCommand::Maximize => window.set_maximized(true),
        GuiCommand::Restore => {
            window.set_maximized(false);
        }
        GuiCommand::Destroy | GuiCommand::Quit => {
            FORCE_QUIT.store(true, Ordering::SeqCst);
            EXIT_REQUESTED.store(true, Ordering::SeqCst);
        }
        GuiCommand::Show => {
            window.set_visible(true);
            state.set_tray_mode(false);
            let _ = webview.evaluate_script(exit_idle_js());
        }
        GuiCommand::ToggleFullscreen => {
            use tao::window::Fullscreen;
            if window.fullscreen().is_some() {
                window.set_fullscreen(None);
            } else {
                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
            }
        }
        GuiCommand::EvalJs(code) => {
            let _ = webview.evaluate_script(&code);
        }
        GuiCommand::DragWindow => {
            let _ = window.drag_window();
        }
        GuiCommand::DragResize(direction) => {
            let _ = window.drag_resize_window(direction);
        }
        GuiCommand::HideToTray(tx) => {
            #[cfg(windows)]
            {
                let (close_to_tray, _) = crate::config::tray_settings();
                let ok = close_to_tray && tray.is_some();
                if ok {
                    window.set_visible(false);
                    state.set_tray_mode(true);
                    let _ = webview.evaluate_script(enter_idle_js());
                }
                let _ = tx.send(ok);
            }
            #[cfg(not(windows))]
            {
                let _ = (tx, tray, webview);
            }
        }
    }
}

fn final_exit() {
    // Как _final_exit/_nuclear_exit в app.py
    #[cfg(windows)]
    {
        crate::win::unregister_context_menu();
    }
    let _ = std::fs::remove_file(crate::paths::pid_file());
}
