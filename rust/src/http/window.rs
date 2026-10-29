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

/// /api/window/hide_to_tray: ✕ в трей. {hidden:true} — окно спрятано.
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
        ok_json(json!({ "ok": false, "error": "native move is unavailable on this platform" }))
    }
}

pub async fn resize_begin() -> Response {
    // Linux frameless-путь не используется (нативные декорации)
    ok_json(json!({ "ok": false, "error": "native resize is unavailable on this platform" }))
}

pub async fn resize_move() -> Response {
    ok_json(json!({ "ok": true }))
}

pub async fn resize_end() -> Response {
    ok_json(json!({ "ok": true }))
}

/// Windows: WebView2 держит hit-test краёв — тянем родное окно сами.
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
        // Модальный цикл ресайза — в blocking-потоке
        let send_hwnd = crate::win::SendHwnd(hwnd);
        let _ = tokio::task::spawn_blocking(move || crate::win::resize_loop(send_hwnd, edges)).await;
        ok_json(json!({ "ok": true }))
    }
    #[cfg(not(windows))]
    {
        let _ = body;
        ok_json(json!({ "ok": false, "error": "native resize is unavailable on this platform" }))
    }
}
