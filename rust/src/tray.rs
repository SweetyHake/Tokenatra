//! System tray (Windows, Linux, macOS: tray-icon + muda).
//! The icon lives for the whole app lifetime; hide means real exit.
//! muda objects are not Sync: the Tray lives on the main event loop
//! thread only; other threads talk through GuiCommand.

use std::sync::Arc;

use crate::state::AppState;

pub struct Tray {
    pub icon: tray_icon::TrayIcon,
    pub model_item: muda::MenuItem,
}

impl Tray {
    pub fn set_model_loaded(&self, loaded: bool) {
        let label = if loaded {
            "Выгрузить модель"
        } else {
            "Загрузить модель"
        };
        self.model_item.set_text(label);
    }
}

fn load_tray_icon() -> Option<tray_icon::Icon> {
    let logo = crate::paths::resource_dir().join("logo.png");
    let bytes = std::fs::read(logo).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let resized = image::imageops::resize(&img, 64, 64, image::imageops::FilterType::Lanczos3);
    tray_icon::Icon::from_rgba(resized.into_raw(), 64, 64).ok()
}

/// Builds the tray. Must run on the main event loop thread.
pub fn create(state: &Arc<AppState>) -> Option<Tray> {
    let icon = load_tray_icon()?;

    let initial_model_label = if state.is_model_loaded() {
        "Выгрузить модель"
    } else {
        "Загрузить модель"
    };
    let open_item = muda::MenuItem::with_id("open", "Открыть Tokenatra", true, None);
    let model_item = muda::MenuItem::with_id("model", initial_model_label, true, None);
    let quit_item = muda::MenuItem::with_id("quit", "Закрыть", true, None);

    let menu = muda::Menu::new();
    if menu
        .append_items(&[&open_item, &model_item, &quit_item])
        .is_err()
    {
        return None;
    }

    let icon = tray_icon::TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Tokenatra")
        .with_menu(Box::new(menu))
        // left click opens the window (TrayIconEvent below), menu on right click
        .with_menu_on_left_click(false)
        .build()
        .ok()?;

    // thread handling menu/icon clicks (clicks are cheap, model in background)
    let st = state.clone();
    std::thread::spawn(move || loop {
        match muda::MenuEvent::receiver().recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(ev) => match ev.id().as_ref() {
                "open" => {
                    let _ = crate::gui_control::send(crate::gui_control::GuiCommand::Show);
                }
                "model" => {
                    if !MODEL_BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        let st2 = st.clone();
                        std::thread::spawn(move || {
                            if crate::infer::model_loaded(&st2) {
                                crate::infer::unload_model(&st2);
                            } else {
                                let _ = crate::infer::load_session(&st2);
                            }
                            MODEL_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
                        });
                    }
                }
                "quit" => {
                    let _ = crate::gui_control::send(crate::gui_control::GuiCommand::Quit);
                }
                _ => {}
            },
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => return,
        }
        if let Ok(ev) = tray_icon::TrayIconEvent::receiver()
            .recv_timeout(std::time::Duration::from_millis(0))
        {
            // left click or double click opens the window; the menu opens on
            // right click (menu_on_left_click = false)
            let open = matches!(
                ev,
                tray_icon::TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    ..
                } | tray_icon::TrayIconEvent::DoubleClick { .. }
            );
            if open {
                let _ = crate::gui_control::send(crate::gui_control::GuiCommand::Show);
            }
        }
    });

    Some(Tray { icon, model_item })
}

static MODEL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn hide(tray: &Tray) {
    let _ = tray.icon.set_visible(false);
}

/// Initializes the standard macOS application menu bar (App, Edit, Window)
/// so that WKWebView receives standard keyboard shortcuts (Cmd+C/V/X/A/Z/Q).
#[cfg(target_os = "macos")]
pub fn init_macos_app_menu() -> Option<muda::Menu> {
    use muda::{AboutMetadata, Menu, PredefinedMenuItem, Submenu};
    let menu = Menu::new();
    let app_menu = Submenu::new(crate::consts::APP_NAME, true);
    let _ = app_menu.append_items(&[
        &PredefinedMenuItem::about(
            None,
            Some(AboutMetadata {
                name: Some(crate::consts::APP_NAME.into()),
                version: Some(crate::consts::VERSION.into()),
                ..Default::default()
            }),
        ),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::services(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(None),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::quit(None),
    ]);
    let edit_menu = Submenu::new("Edit", true);
    let _ = edit_menu.append_items(&[
        &PredefinedMenuItem::undo(None),
        &PredefinedMenuItem::redo(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::cut(None),
        &PredefinedMenuItem::copy(None),
        &PredefinedMenuItem::paste(None),
        &PredefinedMenuItem::select_all(None),
    ]);
    let window_menu = Submenu::new("Window", true);
    let _ = window_menu.append_items(&[
        &PredefinedMenuItem::minimize(None),
        &PredefinedMenuItem::maximize(None),
        &PredefinedMenuItem::close_window(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::fullscreen(None),
    ]);
    let _ = menu.append_items(&[&app_menu, &edit_menu, &window_menu]);
    menu.init_for_nsapp();
    Some(menu)
}
