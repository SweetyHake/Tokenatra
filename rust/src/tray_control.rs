use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Доступность трея (как tray_manager.available(): только Windows).
#[allow(dead_code)]
pub fn tray_available() -> bool {
    cfg!(windows)
}

static CREATED: AtomicBool = AtomicBool::new(false);

pub fn set_created(v: bool) {
    CREATED.store(v, Ordering::SeqCst);
}

#[allow(dead_code)]
pub fn is_created() -> bool {
    CREATED.load(Ordering::SeqCst)
}

static HANDLER: OnceLock<Box<dyn Fn(bool) + Send + Sync>> = OnceLock::new();

/// Регистрирует обработчик смены состояния модели (обновление пункта меню).
pub fn set_model_state_handler(f: Box<dyn Fn(bool) + Send + Sync>) {
    let _ = HANDLER.set(f);
}

pub fn notify_model_state(loaded: bool) {
    crate::gui_control::model_state::set(loaded);
}

pub(crate) fn tray_model_state_changed(loaded: bool) {
    if let Some(h) = HANDLER.get() {
        h(loaded);
    }
}

/// Настройки трея из config.json — делегируем config.
#[allow(dead_code)]
pub fn tray_settings() -> (bool, f64) {
    crate::config::tray_settings()
}
