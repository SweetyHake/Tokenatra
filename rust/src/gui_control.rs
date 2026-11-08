use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};

/// GUI commands from HTTP and background threads.
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
    ModelStateChanged(bool),
    /// Window drag by header (Linux/macOS; Windows uses a Win32 hook).
    #[cfg_attr(windows, allow(dead_code))]
    DragWindow,
    /// Edge resize (Linux/macOS; macOS returns unsupported).
    #[cfg_attr(windows, allow(dead_code))]
    DragResize(tao::window::ResizeDirection),
    /// Frameless edge resize start/move/end for platforms without native drag_resize_window (macOS).
    ResizeBegin {
        left: bool,
        right: bool,
        top: bool,
        bottom: bool,
    },
    ResizeMove { dx: f64, dy: f64 },
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
    let guard = channel().rx.lock().unwrap();
    if let Some(rx) = guard.as_ref() {
        return rx.try_recv();
    }
    Err(std::sync::mpsc::TryRecvError::Disconnected)
}
