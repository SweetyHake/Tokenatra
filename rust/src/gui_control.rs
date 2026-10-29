use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};

/// Команды GUI из HTTP-обработчиков (порт app.window_ref + js_api).
#[derive(Debug)]
pub enum GuiCommand {
    Minimize,
    Maximize,
    Restore,
    Destroy,
    ToggleFullscreen,
    HideToTray(Sender<bool>),
    Quit,
    Show,
    #[allow(dead_code)]
    EvalJs(String),
}

struct Channel {
    tx: Sender<GuiCommand>,
    rx: Mutex<Option<Receiver<GuiCommand>>>,
}

static CHANNEL: OnceLock<Channel> = OnceLock::new();

fn channel() -> &'static Channel {
    CHANNEL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        Channel {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    })
}

pub fn send(cmd: GuiCommand) -> bool {
    let ok = channel().tx.send(cmd).is_ok();
    if ok {
        crate::shell::wake();
    }
    ok
}

/// Вызывается ТОЛЬКО в потоке event loop.
pub fn try_recv() -> Result<GuiCommand, std::sync::mpsc::TryRecvError> {
    // Receiver не Sync, но живёт в Mutex — безопасно при использовании
    // из одного потока event loop.
    let guard = channel().rx.lock().unwrap();
    if let Some(rx) = guard.as_ref() {
        return rx.try_recv();
    }
    Err(std::sync::mpsc::TryRecvError::Disconnected)
}

/// Уведомление о смене состояния модели (иконка/меню трея).
pub mod model_state {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    static FLAG: OnceLock<AtomicBool> = OnceLock::new();

    fn flag() -> &'static AtomicBool {
        FLAG.get_or_init(|| AtomicBool::new(false))
    }

    pub fn set(loaded: bool) {
        flag().store(loaded, Ordering::SeqCst);
        crate::tray_control::tray_model_state_changed(loaded);
    }

    #[allow(dead_code)]
    pub fn get() -> bool {
        flag().load(Ordering::SeqCst)
    }
}
