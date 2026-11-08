use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::json;

use axum::http::StatusCode;
use super::{err_status, err_json, ok_json};
use crate::state::AppState;

pub async fn get_config(headers: HeaderMap) -> Response {
    let cfg = crate::config::load().unwrap_or_else(|_| json!({}));
    let cfg = strip_secret_for_origin(cfg, &headers);
    ok_json(cfg)
}

/// Browser read from an unknown Origin: strip network secret and local folder paths.
fn strip_secret_for_origin(mut cfg: serde_json::Value, headers: &HeaderMap) -> serde_json::Value {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_lowercase();
    if !origin.is_empty()
        && !crate::config::allowed_origins().contains(&origin)
        && !crate::http::guard_connector_origin(&origin)
        && let Some(obj) = cfg.as_object_mut()
    {
        obj.remove("shared_secret");
        obj.remove("lastFolders");
    }
    cfg
}

pub async fn save_config(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    if !ct.starts_with("application/json") {
        return err_status(StatusCode::UNSUPPORTED_MEDIA_TYPE, "JSON Content-Type required");
    }
    let data: Option<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_slice(&body).ok();
    let Some(data) = data else {
        return err_status(StatusCode::BAD_REQUEST, "Invalid body");
    };
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_lowercase();
    let origin_is_foreign = !origin.is_empty()
        && !crate::config::allowed_origins().contains(&origin)
        && !crate::http::guard_connector_origin(&origin);

    let picked = state.picked_folders.lock().unwrap().clone();
    let merged = crate::config::merge_client_config(&data, origin_is_foreign, &picked);
    if crate::config::save(&merged).is_err() {
        return err_json("Не удалось записать config.json");
    }
    ok_json(json!({ "ok": true }))
}
