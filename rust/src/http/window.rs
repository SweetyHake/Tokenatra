use axum::extract::{Path, State};
use axum::response::Response;
use serde_json::json;

use super::ok_json;
use crate::gui_control::GuiCommand;
use crate::state::AppState;

pub async fn window_action(
    State(state): State<std::sync::Arc<AppState>>,
    Path(action): Path<String>,
) -> Response {
    if !crate::shell::has_window() {
        return ok_json(json!({ "ok": false, "error": "no window" }));
    }
    match action.as_str() {
        "minimize" => {
            crate::gui_control::send(GuiCommand::Minimize);
        }
        "maximize" => {
            crate::gui_control::send(GuiCommand::Maximize);
        }
        "restore" => {
            crate::gui_control::send(GuiCommand::Restore);
        }
        "close" | "destroy" => {
            crate::gui_control::send(GuiCommand::Destroy);
        }
        "toggle_fullscreen" => {
            crate::gui_control::send(GuiCommand::ToggleFullscreen);
        }
        "hide_to_tray" => {
            return hide_to_tray(state).await;
        }
        "quit" => {
            crate::gui_control::send(GuiCommand::Quit);
        }
        "move" => {
            return window_move();
        }
        _ => return ok_json(json!({ "ok": false, "error": "unknown action" })),
    }
    ok_json(json!({ "ok": true }))
}

/// /api/window/hide_to_tray: close to tray. {hidden:true} if hidden.
async fn hide_to_tray(_state: std::sync::Arc<AppState>) -> Response {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    if !crate::gui_control::send(GuiCommand::HideToTray(tx.clone())) {
        return ok_json(json!({ "hidden": false }));
    }
    let hidden = tokio::task::spawn_blocking(move || {
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    ok_json(json!({ "hidden": hidden }))
}

fn window_move() -> Response {
    #[cfg(windows)]
    {
        // Windows: Win32 hook with SetCapture (the tao drag_window modal
        // loop stuck on WebView2, see the old app.py history)
        match crate::win::hwnd() {
            Some(hwnd) => {
                crate::win::post_drag_message(hwnd);
                ok_json(json!({ "ok": true }))
            }
            None => ok_json(json!({ "ok": false, "error": "no window" })),
        }
    }
    #[cfg(not(windows))]
    {
        crate::gui_control::send(crate::gui_control::GuiCommand::DragWindow);
        ok_json(json!({ "ok": true }))
    }
}

pub async fn resize_begin() -> Response {
    // Linux frameless path unused (native decorations)
    ok_json(json!({ "ok": false, "error": "native resize is unavailable on this platform" }))
}

pub async fn resize_move() -> Response {
    ok_json(json!({ "ok": true }))
}

pub async fn resize_end() -> Response {
    ok_json(json!({ "ok": true }))
}

/// Edge resize: Windows uses a modal loop per button; Linux uses GTK
/// begin_resize_drag; macOS tao returns unsupported (no system API).
pub async fn resize_start(body: axum::body::Bytes) -> Response {
    #[cfg(windows)]
    {
        let data: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
        let edges = (
            data.get("left").and_then(|v| v.as_bool()).unwrap_or(false),
            data.get("right").and_then(|v| v.as_bool()).unwrap_or(false),
            data.get("top").and_then(|v| v.as_bool()).unwrap_or(false),
            data.get("bottom").and_then(|v| v.as_bool()).unwrap_or(false),
        );
        if !edges.0 && !edges.1 && !edges.2 && !edges.3 {
            return ok_json(json!({ "ok": false, "error": "no edges" }));
        }
        let Some(hwnd) = crate::win::hwnd() else {
            return ok_json(json!({ "ok": false, "error": "no window" }));
        };
        // modal resize loop runs in a blocking thread
        let send_hwnd = crate::win::SendHwnd(hwnd);
        let _ = tokio::task::spawn_blocking(move || crate::win::resize_loop(send_hwnd, edges)).await;
        ok_json(json!({ "ok": true }))
    }
    #[cfg(not(windows))]
    {
        use tao::window::ResizeDirection;
        let data: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
        let (l, r, t, b) = (
            data.get("left").and_then(|v| v.as_bool()).unwrap_or(false),
            data.get("right").and_then(|v| v.as_bool()).unwrap_or(false),
            data.get("top").and_then(|v| v.as_bool()).unwrap_or(false),
            data.get("bottom").and_then(|v| v.as_bool()).unwrap_or(false),
        );
        let direction = match (l, r, t, b) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, _, _, _) => ResizeDirection::West,
            (_, true, _, _) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return ok_json(json!({ "ok": false, "error": "no edges" })),
        };
        crate::gui_control::send(crate::gui_control::GuiCommand::DragResize(direction));
        ok_json(json!({ "ok": true }))
    }
}
