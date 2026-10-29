use std::time::Instant;

use axum::extract::{Multipart, Path, State};
use axum::response::Response;
use serde_json::json;

use axum::http::StatusCode;
use super::{err_status, file_response, ok_json};
use crate::state::{AppState, EmbedExport, EMBED_EXPORT_MAX_BYTES, EMBED_EXPORT_MAX_ITEMS, EMBED_EXPORT_TTL};

/// Порт /embed_export POST: принять готовое изображение из embed-редактора.
pub async fn embed_export_save(
    State(state): State<std::sync::Arc<AppState>>,
    mut mp: Multipart,
) -> Response {
    let mut file: Option<(String, Vec<u8>)> = None;
    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        if name == "image" || name == "file" {
            let filename = field.file_name().map(|s| s.to_string()).unwrap_or_default();
            if filename.is_empty() {
                continue;
            }
            if let Ok(b) = field.bytes().await {
                file = Some((filename, b.to_vec()));
            }
        }
    }
    let Some((filename, data)) = file else {
        return err_status(StatusCode::BAD_REQUEST, "Нет файла");
    };
    let mut ext = super::ext_of(&filename);
    if !["webp", "png", "jpg", "jpeg"].contains(&ext.as_str()) {
        ext = "webp".into();
    }
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "image/webp",
    };

    let dir = crate::paths::embed_export_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось создать каталог экспорта");
    }
    let file_id = AppState::new_embed_id();
    let file_name = format!("{file_id}.{ext}");
    let target = dir.join(&file_name);
    if std::fs::write(&target, &data).is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось сохранить");
    }
    if target.metadata().map(|m| m.len()).unwrap_or(0) > EMBED_EXPORT_MAX_BYTES {
        let _ = std::fs::remove_file(&target);
        return err_status(StatusCode::PAYLOAD_TOO_LARGE, "Файл больше 40 МБ");
    }

    {
        let mut map = state.embed_exports.lock().unwrap();
        AppState::embed_export_cleanup(&mut map);
        // LRU-вытеснение при переполнении
        while map.len() >= EMBED_EXPORT_MAX_ITEMS {
            let oldest = map
                .iter()
                .min_by_key(|(_, e)| e.expires)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    if let Some(item) = map.remove(&k) {
                        let _ = std::fs::remove_file(&item.path);
                    }
                }
                None => break,
            }
        }
        map.insert(
            file_name.clone(),
            EmbedExport {
                path: target,
                mime,
                expires: Instant::now() + EMBED_EXPORT_TTL,
            },
        );
    }
    ok_json(json!({ "id": file_id, "url": format!("/embed_export/{file_name}") }))
}

/// Порт /embed_export/<file_name> GET: модуль забирает блоб fetch'ем с CORS.
pub async fn embed_export_get(
    State(state): State<std::sync::Arc<AppState>>,
    Path(file_name): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    let safe = std::path::Path::new(&file_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let stem = safe.split('.').next().unwrap_or_default().to_string();

    let entry = {
        let map = state.embed_exports.lock().unwrap();
        match map.get(&safe) {
            Some(e) => Some((safe.clone(), e.clone())),
            None => map
                .iter()
                .find(|(n, _)| n.split('.').next() == Some(stem.as_str()))
                .map(|(n, e)| (n.clone(), e.clone())),
        }
    };

    let Some(entry) = entry else {
        return err_status(StatusCode::NOT_FOUND, "Not found");
    };
    // Прямые GET с чужих страниц (<img> без Origin) — cross-site режем
    let connector_ok = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|o| super::guard_connector_origin(o))
        .unwrap_or(false);
    if !connector_ok {
        let sfs = headers
            .get("Sec-Fetch-Site")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_lowercase();
        if sfs == "cross-site" {
            return err_status(StatusCode::FORBIDDEN, "Forbidden");
        }
    }
    let entry = entry.1;
    match std::fs::read(&entry.path) {
        Ok(bytes) => {
            let mut resp = file_response(bytes, entry.mime, None);
            resp.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            resp
        }
        Err(_) => err_status(StatusCode::NOT_FOUND, "Not found"),
    }
}
