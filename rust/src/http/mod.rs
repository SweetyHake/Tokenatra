pub mod agent;
pub mod assets;
pub mod config_routes;
pub mod embed;
pub mod files;
pub mod models;
pub mod presets;
pub mod process;
pub mod rings;
pub mod system;
pub mod window;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::consts::*;

#[allow(dead_code)]
pub type Json = serde_json::Value;

pub fn err_json(msg: &str) -> Response {
    let v = json!({ "error": msg });
    axum::Json(v).into_response()
}

/// Error with an HTTP status.
pub fn err_status(code: StatusCode, msg: &str) -> Response {
    (
        code,
        axum::Json(json!({ "error": msg })),
    )
        .into_response()
}

pub fn ok_json(v: serde_json::Value) -> Response {
    axum::Json(v).into_response()
}

pub fn file_response(bytes: Vec<u8>, mime: &str, cache: Option<&str>) -> Response {
    let mut resp = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .body(Body::from(bytes))
        .unwrap();
    if let Some(c) = cache {
        if let Ok(v) = HeaderValue::from_str(c) {
            resp.headers_mut().insert(header::CACHE_CONTROL, v);
        }
    }
    resp
}

pub fn send_file_from_disk(path: &std::path::Path, mime: &str, cache: Option<&str>) -> Response {
    match std::fs::read(path) {
        Ok(b) => file_response(b, mime, cache),
        Err(_) => err_status(StatusCode::NOT_FOUND, "Not found"),
    }
}

pub fn mime_by_ext(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "css" => "text/css",
        "js" => "text/javascript; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

pub fn ext_of(name: &str) -> String {
    name.rsplit('.')
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

pub fn query_pairs(uri: &Uri) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(q) = uri.query() {
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            out.insert(k.to_string(), v.to_string());
        }
    }
    out
}

// ===== Guard / CORS / CSP =====

pub fn localhost_host_name(host: &str) -> String {
    let host = host.trim().to_lowercase();
    if let Some(rest) = host.strip_prefix('[') {
        return match rest.find(']') {
            Some(i) => rest[..i].to_string(),
            None => host.trim_start_matches('[').to_string(),
        };
    }
    match host.rfind(':') {
        Some(i) => host[..i].to_string(),
        None => host,
    }
}

fn is_local_host_name(h: &str) -> bool {
    h == "127.0.0.1" || h == "localhost" || h == "::1"
}

fn is_connector_route(path: &str) -> bool {
    CONNECTOR_ROUTES.contains(&path)
        || CONNECTOR_ROUTE_PREFIXES.iter().any(|p| path.starts_with(p))
}

fn is_public_proxy_route(path: &str) -> bool {
    PUBLIC_PROXY_ROUTES.contains(&path)
        || PUBLIC_PROXY_ROUTE_PREFIXES.iter().any(|p| path.starts_with(p))
}

fn is_ui_asset_path(path: &str) -> bool {
    path == "/"
        || PUBLIC_OPEN_EXACT.contains(&path)
        || PUBLIC_OPEN_PREFIXES.iter().any(|p| path.starts_with(p))
}

fn origin_host(origin: &str) -> Option<String> {
    // scheme://host[:port]
    let rest = origin
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(origin);
    if rest.is_empty() {
        return None;
    }
    Some(localhost_host_name(rest.split('/').next().unwrap_or(rest)))
}

/// Origin in the localhost family or explicitly allowed in allowed_origins.
pub fn guard_connector_origin(origin: &str) -> bool {
    connector_origin(origin).is_some()
}

fn connector_origin(origin: &str) -> Option<String> {
    if origin.is_empty() {
        return None;
    }
    if let Some(h) = origin_host(origin) {
        if is_local_host_name(&h) {
            return Some(origin.to_string());
        }
    }
    let normalized = origin.trim().trim_end_matches('/').to_lowercase();
    if crate::config::allowed_origins().iter().any(|o| o == &normalized) {
        return Some(origin.to_string());
    }
    None
}

fn supplied_secret(req_headers: &axum::http::HeaderMap, uri: &Uri) -> String {
    let mut key = req_headers
        .get("X-Tokenatra-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if key.is_empty() {
        if let Some(k) = query_pairs(uri).get("key") {
            key = k.clone();
        }
    }
    if key.is_empty() {
        if let Some(c) = req_headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
        {
            for part in c.split(';') {
                let part = part.trim();
                if let Some(v) = part.strip_prefix("tokenatra_key=") {
                    key = v.to_string();
                    break;
                }
            }
        }
    }
    key.trim().to_string()
}

fn request_key_valid(req_headers: &axum::http::HeaderMap, uri: &Uri) -> bool {
    let secret = crate::config::shared_secret();
    let supplied = supplied_secret(req_headers, uri);
    if secret.is_empty() || supplied.is_empty() {
        return false;
    }
    constant_time_eq(supplied.as_bytes(), secret.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn forbidden() -> Response {
    err_status(StatusCode::FORBIDDEN, "Forbidden")
}

/// The guard middleware.
pub async fn guard_mw(req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let host_hdr = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let origin_hdr = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let sec_fetch_site = req
        .headers()
        .get("Sec-Fetch-Site")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    let host_name = localhost_host_name(&host_hdr);

    if path.starts_with("/agent/v1") {
        // Agent API: localhost only + X-Tokenatra-Agent: 1
        let agent_ok = req
            .headers()
            .get("X-Tokenatra-Agent")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        if is_local_host_name(&host_name) && agent_ok {
            return next.run(req).await;
        }
        return forbidden();
    }

    if !is_local_host_name(&host_name) {
        // network mode (listen_host=0.0.0.0): only the studio is exposed
        if !crate::config::network_mode() {
            return forbidden();
        }
        let allowed_hosts = crate::config::allowed_hosts();
        if !allowed_hosts.is_empty() && !allowed_hosts.contains(&host_name) {
            return forbidden();
        }
        if method == Method::OPTIONS {
            return next.run(req).await;
        }
        let key_required = !crate::config::shared_secret().is_empty();
        if !key_required || request_key_valid(req.headers(), req.uri()) {
            if is_public_proxy_route(&path) || PUBLIC_OPEN_EXACT.contains(&path.as_str()) {
                return next.run(req).await;
            }
            return forbidden();
        }
        // keyed hardening: without the key only UI static is served
        if (method == Method::GET || method == Method::HEAD)
            && (PUBLIC_OPEN_EXACT.contains(&path.as_str())
                || PUBLIC_OPEN_PREFIXES.iter().any(|p| path.starts_with(p)))
        {
            return next.run(req).await;
        }
        return forbidden();
    }

    // Foundry module: localhost:30000 -> 127.0.0.1:7878 (same machine)
    if is_connector_route(&path) && connector_origin(&origin_hdr).is_some() {
        return next.run(req).await;
    }

    let origin_normalized = origin_hdr.trim().trim_end_matches('/').to_lowercase();
    let allowed_origins = crate::config::allowed_origins();
    let origin_allowed =
        !origin_normalized.is_empty() && allowed_origins.contains(&origin_normalized);

    if origin_allowed && (method == Method::GET || method == Method::HEAD) && is_ui_asset_path(&path)
    {
        return next.run(req).await;
    }
    if origin_allowed && path == "/config" && (method == Method::GET || method == Method::POST || method == Method::OPTIONS) {
        return next.run(req).await;
    }

    // "allow browser connections" (on by default)
    if crate::config::allow_browser()
        && (is_connector_route(&path) || is_ui_asset_path(&path) || path == "/config")
    {
        return next.run(req).await;
    }

    let state_changing = matches!(
        method,
        Method::POST | Method::PUT | Method::DELETE | Method::PATCH
    );
    if state_changing
        && !sec_fetch_site.is_empty()
        && !["same-origin", "same-site", "none"].contains(&sec_fetch_site.as_str())
    {
        return forbidden();
    }
    if !origin_hdr.is_empty() {
        let expected = format!("http://{host_hdr}").to_lowercase();
        if origin_hdr.to_lowercase() != expected {
            return forbidden();
        }
    } else if state_changing {
        return forbidden();
    }

    next.run(req).await
}

/// OPTIONS response (preflight).
pub async fn options_mw(req: Request<Body>, next: Next) -> Response {
    if req.method() == Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::ALLOW, "GET, POST, OPTIONS")
            .body(Body::empty())
            .unwrap();
    }
    next.run(req).await
}

/// Connector headers (after_request): CORS + CSP frame-ancestors.
pub async fn headers_mw(req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let origin_hdr = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let query = query_pairs(req.uri());
    let key_valid = request_key_valid(req.headers(), req.uri());
    let has_private_network_req = req
        .headers()
        .contains_key("Access-Control-Request-Private-Network");

    let mut resp = next.run(req).await;

    let origin_normalized = origin_hdr.trim().trim_end_matches('/').to_lowercase();
    let allowed = crate::config::allowed_origins();
    let origin_in_allowed = !origin_normalized.is_empty() && allowed.contains(&origin_normalized);
    let config_read = path == "/config" && origin_in_allowed;
    let ui_read = (method == Method::GET || method == Method::HEAD)
        && origin_in_allowed
        && is_ui_asset_path(&path);

    if is_connector_route(&path) || is_public_proxy_route(&path) || config_read || ui_read {
        let preflight = method == Method::OPTIONS;
        let cors_allowed = connector_origin(&origin_hdr).is_some()
            || crate::config::network_mode()
            || key_valid
            || ui_read
            || (preflight && !crate::config::shared_secret().is_empty())
            || crate::config::allow_browser();
        if !origin_hdr.is_empty() && cors_allowed {
            let h = resp.headers_mut();
            if let Ok(v) = HeaderValue::from_str(&origin_hdr) {
                h.insert("Access-Control-Allow-Origin", v);
            }
            h.insert(header::VARY, HeaderValue::from_static("Origin"));
            h.insert("Access-Control-Allow-Methods", HeaderValue::from_static("GET, POST, OPTIONS"));
            h.insert(
                "Access-Control-Allow-Headers",
                HeaderValue::from_static("Content-Type, Authorization, X-Tokenatra-Key"),
            );
            h.insert("Access-Control-Max-Age", HeaderValue::from_static("600"));
            if preflight && has_private_network_req {
                h.insert(
                    "Access-Control-Allow-Private-Network",
                    HeaderValue::from_static("true"),
                );
            }
        }
    }

    // anti-clickjacking
    let is_html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);
    if is_html {
        let csp = if query.get("embed").map(|v| v.as_str()) == Some("1") {
            let mut ancestors: Vec<String> = vec![
                "http://localhost:*".into(),
                "https://localhost:*".into(),
                "http://127.0.0.1:*".into(),
                "https://127.0.0.1:*".into(),
                "http://[::1]:*".into(),
                "https://[::1]:*".into(),
            ];
            ancestors.extend(allowed.iter().cloned());
            if crate::config::network_mode() || key_valid {
                if let Some(o) = query.get("origin") {
                    let o = o.trim().to_lowercase();
                    if o.starts_with("http://") || o.starts_with("https://") {
                        ancestors.push(o.trim_end_matches('/').to_string());
                    }
                }
            }
            format!("frame-ancestors {}", ancestors.join(" "))
        } else {
            "frame-ancestors 'none'".to_string()
        };
        if let Ok(v) = HeaderValue::from_str(&csp) {
            resp.headers_mut().insert("Content-Security-Policy", v);
        }
    }

    resp
}

pub fn router(state: std::sync::Arc<crate::state::AppState>) -> axum::Router {
    use axum::routing::{get, post};
    let agent_routes = axum::Router::new()
        .route("/ping", get(agent::ping))
        .route("/manifest", get(agent::manifest))
        .route("/assets", get(agent::assets))
        .route("/tools_schema", get(agent::tools_schema))
        .route("/sessions", post(agent::create_session))
        .route(
            "/sessions/{sid}",
            get(agent::get_session).delete(agent::close_session),
        )
        .route("/sessions/{sid}/remove_bg", post(agent::op_remove_bg))
        .route("/sessions/{sid}/mask", post(agent::op_mask))
        .route("/sessions/{sid}/transform", post(agent::op_transform))
        .route("/sessions/{sid}/ring", post(agent::op_ring))
        .route("/sessions/{sid}/effects", post(agent::op_effects))
        .route("/sessions/{sid}/preview", get(agent::op_preview))
        .route("/sessions/{sid}/inspect", get(agent::op_inspect))
        .route("/sessions/{sid}/export", post(agent::op_export))
        .with_state(state.clone());

    let mut r = axum::Router::new()
        .route("/", get(assets::index))
        .route("/splash", get(assets::splash))
        .route("/version", get(system::version_info))
        .route("/model_status", get(system::model_status))
        .route("/models_list", get(models::models_list))
        .route("/open_model_link", post(models::open_model_link))
        .route("/select_model", post(models::select_model))
        .route("/download_model_from_server", post(models::download_model_from_server))
        .route("/load_model_from_disk", post(models::load_model_from_disk))
        .route("/icon", get(assets::app_icon))
        .route("/logo", get(assets::app_logo))
        .route("/api/window/{action}", post(window::window_action))
        .route("/api/window/resize_begin", post(window::resize_begin))
        .route("/api/window/resize_move", post(window::resize_move))
        .route("/api/window/resize_end", post(window::resize_end))
        .route("/api/window/resize_start", post(window::resize_start))
        .route("/presets_list", get(presets::presets_list))
        .route("/preset_file/{filename}", get(presets::preset_file))
        .route("/add_preset", post(presets::add_preset))
        .route("/delete_preset", post(presets::delete_preset))
        .route("/device", get(system::device))
        .route("/network_info", get(system::network_info))
        .route("/health", get(system::health))
        .route("/update_status", get(system::update_status))
        .route("/start_update_download", post(system::start_update_download))
        .route("/check_update", post(system::check_update))
        .route("/apply_update", post(system::apply_update))
        .route("/ring", get(assets::ring))
        .route("/mask", get(assets::mask))
        .route("/preset", get(assets::preset))
        .route("/example", get(assets::example))
        .route("/process", post(process::process))
        .route("/convert", post(process::convert_file))
        .route("/rings_list", get(rings::rings_list))
        .route("/delete_ring", post(rings::delete_ring))
        .route("/add_ring", post(rings::add_ring))
        .route("/ring_file/{filename}", get(rings::ring_file))
        .route("/save_file", post(files::save_file))
        .route("/pick_folder", get(files::pick_folder))
        .route("/save_to_folder", post(files::save_to_folder))
        .route("/pick_image_to_open", get(files::pick_image_to_open))
        .route("/list_images", post(files::list_images))
        .route("/get_image_by_path", get(files::get_image_by_path))
        .route("/config", get(config_routes::get_config).post(config_routes::save_config))
        .route("/shutdown", post(system::shutdown))
        .route("/embed_export", post(embed::embed_export_save))
        .route("/embed_export/{file_name}", get(embed::embed_export_get))
        .route("/static/{*path}", get(assets::static_file))
        .nest("/agent/v1", agent_routes);

    r = r
        .layer(axum::middleware::from_fn(guard_mw))
        .layer(axum::middleware::from_fn(options_mw))
        .layer(axum::middleware::from_fn(headers_mw))
        .layer(axum::extract::DefaultBodyLimit::max(50 * 1024 * 1024));
    r.with_state(state)
}
