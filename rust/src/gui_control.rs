use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};

/// GUI commands from HTTP handlers.
#[derive(Debug)]
pub enum GuiCommand {
    Minimize,
    Maximize,
    Restore,
    Destroy,
    ToggleFullscreen,
    HideToTray(Sender<bool>),
    Quit,
    #[allow(dead_code)] // constructed on windows only
    Show,
    #[allow(dead_code)]
    EvalJs(String),
    /// Window drag by header (Linux/macOS; Windows uses a Win32 hook).
    #[allow(dead_code)]
    DragWindow,
    /// Edge resize (Linux/macOS; macOS returns unsupported).
    #[allow(dead_code)]
    DragResize(tao::window::ResizeDirection),
    /// Frameless edge resize start/move/end for platforms without native drag_resize_window (macOS).
    #[allow(dead_code)]
    ResizeBegin {
        left: bool,
        right: bool,
        top: bool,
        bottom: bool,
    },
    #[allow(dead_code)]
    ResizeMove { dx: f64, dy: f64 },
    #[allow(dead_code)]
    ResizeEnd,
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

/// Must run on the event loop thread.
pub fn try_recv() -> Result<GuiCommand, std::sync::mpsc::TryRecvError> {
    // Receiver is not Sync; kept in a Mutex and used on the
    let guard = channel().rx.lock().unwrap();
    if let Some(rx) = guard.as_ref() {
        return rx.try_recv();
    }
    Err(std::sync::mpsc::TryRecvError::Disconnected)
}

/// Notifies the tray about model state changes.
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
