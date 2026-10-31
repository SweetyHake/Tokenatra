#![cfg(windows)]
//! Системный трей (Windows, tray-icon + muda) — порт tray_manager.py.
//! Иконка живёт весь цикл приложения; скрытие — только реальный выход.
//! muda-объекты не Sync: Tray живёт ТОЛЬКО в главном потоке event loop,
//! действия из других потоков приходят через каналы.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use crate::state::AppState;

pub struct Tray {
    pub icon: tray_icon::TrayIcon,
    pub model_item: muda::MenuItem,
}

/// Канал обновления подписи «Загрузить/Выгрузить модель» (в главный поток).
struct ModelLabelChannel {
    tx: Sender<bool>,
    rx: Mutex<Option<std::sync::mpsc::Receiver<bool>>>,
}

static MODEL_LABEL: OnceLock<ModelLabelChannel> = OnceLock::new();

fn model_label() -> &'static ModelLabelChannel {
    MODEL_LABEL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        ModelLabelChannel {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    })
}

fn load_tray_icon() -> Option<tray_icon::Icon> {
    let logo = crate::paths::resource_dir().join("logo.png");
    let bytes = std::fs::read(logo).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let resized = image::imageops::resize(&img, 64, 64, image::imageops::FilterType::Lanczos3);
    tray_icon::Icon::from_rgba(resized.into_raw(), 64, 64).ok()
}

/// Создать трей. Вызывать ТОЛЬКО в главном потоке (event loop).
pub fn create(state: &Arc<AppState>) -> Option<Tray> {
    let Some(icon) = load_tray_icon() else {
        crate::tray_control::set_created(false);
        return None;
    };

    let open_item = muda::MenuItem::with_id("open", "Открыть Tokenatra", true, None);
    let model_item = muda::MenuItem::with_id("model", "Загрузить модель", true, None);
    let quit_item = muda::MenuItem::with_id("quit", "Закрыть", true, None);

    let menu = muda::Menu::new();
    if menu
        .append_items(&[&open_item, &model_item, &quit_item])
        .is_err()
    {
        crate::tray_control::set_created(false);
        return None;
    }

    let icon = tray_icon::TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Tokenatra")
        .with_menu(Box::new(menu))
        // ЛКМ — открыть окно (обработчик TrayIconEvent ниже), меню — по ПКМ
        .with_menu_on_left_click(false)
        .build()
        .ok()?;

    // Обновление подписи пункта меню из любого потока — через канал
    {
        let ch = model_label();
        crate::tray_control::set_model_state_handler(Box::new(move |loaded| {
            let _ = ch.tx.send(loaded);
            crate::shell::wake();
        }));
    }

    // Поток обработки кликов меню/иконки (клики — дёшево, модель — в фоне)
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
                            let loaded = crate::infer::model_loaded(&st2);
                            if loaded {
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
            // ЛКМ или двойной клик — открыть окно; меню открывается по ПКМ
            // силами tray-icon (menu_on_left_click = false)
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

    crate::tray_control::set_created(true);
    Some(Tray { icon, model_item })
}

static MODEL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Применить отложенные смены подписи модели (вызывать в event loop).
pub fn drain_model_labels(tray: &Tray) {
    let guard = model_label().rx.lock().unwrap();
    if let Some(rx) = guard.as_ref() {
        while let Ok(loaded) = rx.try_recv() {
            let label = if loaded { "Выгрузить модель" } else { "Загрузить модель" };
            let _ = tray.model_item.set_text(label);
        }
    }
}

pub fn hide(tray: &Tray) {
    let _ = tray.icon.set_visible(false);
}
