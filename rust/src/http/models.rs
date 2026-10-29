use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::response::Response;
use serde_json::json;

use axum::http::StatusCode;
use super::{err_status, ok_json};
use crate::consts::{model_download_url, model_download_url_lite, APP_NAME};
use crate::paths;
use crate::state::AppState;

pub async fn models_list() -> Response {
    let _ = std::fs::create_dir_all(paths::models_dir());
    let selected = paths::get_selected_model_path();
    let mut models = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut dirs = vec![paths::models_dir()];
    for extra in [paths::base_dir().join("models"), paths::resource_dir().join("models")] {
        if !dirs.iter().any(|d| paths::same_path(d, &extra)) {
            dirs.push(extra);
        }
    }
    for d in dirs {
        if !d.is_dir() {
            continue;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&d)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.eq_ignore_ascii_case("onnx"))
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for f in files {
            if !f.is_file() {
                continue;
            }
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if seen.contains(&name) {
                continue;
            }
            seen.insert(name.clone());
            let size = f.metadata().map(|m| m.len()).unwrap_or(0);
            models.push(json!({
                "name": name,
                "size": size,
                "selected": paths::same_path(&f, &selected),
            }));
        }
    }
    ok_json(json!({
        "models": models,
        "models_dir": paths::models_dir().to_string_lossy(),
        "selected": selected.exists().then(|| selected.file_name().and_then(|n| n.to_str()).map(String::from)).flatten(),
        "model_download_url": model_download_url(),
        "model_download_url_lite": model_download_url_lite(),
    }))
}

/// Сохранить модель выбранной и сбросить кеш сессии.
pub fn activate_model(path: &Path, state: &AppState) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let _ = crate::config::set_key("selected_model", json!(name));
    let mut guard = state.session_lock.lock().unwrap();
    *guard = None;
}

pub async fn select_model(
    State(state): State<std::sync::Arc<AppState>>,
    body: axum::body::Bytes,
) -> Response {
    let data: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let filename = data.get("name").and_then(|v| v.as_str()).unwrap_or_default();
    let base_name = Path::new(filename)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut p = paths::models_dir().join(&base_name);
    let ok_ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("onnx"))
        .unwrap_or(false);
    if !ok_ext || !p.is_file() {
        p = PathBuf::new();
        for legacy in [
            paths::base_dir().join("models").join(&base_name),
            paths::resource_dir().join("models").join(&base_name),
        ] {
            let ok = legacy
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("onnx"))
                .unwrap_or(false);
            if ok && legacy.is_file() {
                p = legacy;
                break;
            }
        }
        if p.as_os_str().is_empty() {
            return err_status(StatusCode::NOT_FOUND, "Модель не найдена");
        }
    }
    activate_model(&p, &state);
    ok_json(json!({
        "ok": true,
        "name": p.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
    }))
}

pub async fn open_model_link(body: axum::body::Bytes) -> Response {
    let data: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let kind = data.get("kind").and_then(|v| v.as_str());
    let url = if kind == Some("lite") {
        model_download_url_lite()
    } else {
        model_download_url()
    };
    if url.is_empty() {
        return err_status(StatusCode::SERVICE_UNAVAILABLE, "Ссылка на модель ещё не настроена");
    }
    match open::that_detached(&url) {
        Ok(()) => ok_json(json!({ "ok": true, "url": url })),
        Err(e) => err_status(StatusCode::INTERNAL_SERVER_ERROR, &format!("Не удалось открыть браузер: {e}")),
    }
}

/// Dropbox-ссылки превращаем в прямые (dl=1).
fn dropbox_direct_url(url: &str) -> String {
    if !url.to_lowercase().contains("dropbox.com") {
        return url.to_string();
    }
    if let Some(pos) = url.find('?') {
        let (base, query) = url.split_at(pos);
        let mut out = String::from(base);
        if query.contains("dl=") {
            // заменяем dl=0 на dl=1
            let q = query.replace("dl=0", "dl=1");
            out.push_str(&q);
        } else {
            out.push_str(query);
            out.push_str("&dl=1");
        }
        out
    } else {
        format!("{url}?dl=1")
    }
}

fn download_model_sync(url: &str, state: &AppState) -> Result<PathBuf, String> {
    use std::io::Read;
    let models = paths::models_dir();
    let _ = std::fs::create_dir_all(&models);
    let path_part = url.split('?').next().unwrap_or(url);
    let decoded = percent_encoding::percent_decode_str(path_part.split('/').last().unwrap_or(""))
        .decode_utf8_lossy()
        .to_string();
    let mut filename = decoded;
    if !filename.to_lowercase().ends_with(".onnx") {
        filename = "model.onnx".into();
    }
    let target = models.join(&filename);
    if target.exists() {
        return Err(format!(
            "Модель «{filename}» уже загружена — повторная загрузка не требуется"
        ));
    }
    let uniq = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let temp_path = models.join(format!("{filename}.{}.{}.download", std::process::id(), uniq));

    let result = (|| -> Result<(), String> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("{APP_NAME} model downloader"))
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?;
        let mut resp = client
            .get(dropbox_direct_url(url))
            .send()
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        let mut file = std::fs::File::create(&temp_path).map_err(|e| e.to_string())?;
        let mut buf = [0u8; 1024 * 1024];
        loop {
            let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| e.to_string())?;
        }
        drop(file);
        if std::fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0) < 1024 {
            return Err("Сервер вернул слишком маленький файл".into());
        }
        {
            let mut guard = state.session_lock.lock().unwrap();
            *guard = None;
        }
        std::fs::rename(&temp_path, &target).map_err(|e| e.to_string())?;
        Ok(())
    })();

    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e);
    }
    activate_model(&target, state);
    Ok(target)
}

pub async fn download_model_from_server(
    State(state): State<std::sync::Arc<AppState>>,
) -> Response {
    let url = model_download_url();
    if url.is_empty() {
        return err_status(StatusCode::SERVICE_UNAVAILABLE, "Ссылка на модель ещё не настроена");
    }
    let st = state.clone();
    let result = tokio::task::spawn_blocking(move || download_model_sync(&url, &st))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match result {
        Ok(path) => {
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            ok_json(json!({
                "ok": true,
                "name": path.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
                "size": size,
            }))
        }
        Err(e) => {
            println!("model download error: {e}");
            err_status(StatusCode::BAD_GATEWAY, &format!("Не удалось загрузить модель: {e}"))
        }
    }
}

pub async fn load_model_from_disk(
    State(state): State<std::sync::Arc<AppState>>,
) -> Response {
    // Нативный диалог выбора .onnx
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Выберите модель ONNX")
            .add_filter("Модель ONNX", &["onnx"])
            .pick_file()
    })
    .await
    .unwrap_or(None);

    let Some(path) = picked else {
        return ok_json(json!({ "cancelled": true }));
    };
    let source = PathBuf::from(&path);
    let ext_ok = source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("onnx"))
        .unwrap_or(false);
    if !ext_ok || !source.is_file() {
        return err_status(StatusCode::BAD_REQUEST, "Можно выбрать только файл .onnx");
    }

    let models = paths::models_dir();
    let _ = std::fs::create_dir_all(&models);
    let target = models.join(source.file_name().unwrap_or_default());
    let temp_path = models.join(format!(
        "{}.download",
        source.file_name().and_then(|n| n.to_str()).unwrap_or_default()
    ));
    let res = (|| -> Result<(), String> {
        if !paths::same_path(&source, &target) {
            std::fs::copy(&source, &temp_path).map_err(|e| e.to_string())?;
            {
                let mut guard = state.session_lock.lock().unwrap();
                *guard = None;
            }
            std::fs::rename(&temp_path, &target).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&temp_path);
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, &format!("Не удалось загрузить модель: {e}"));
    }
    activate_model(&target, &state);
    ok_json(json!({
        "ok": true,
        "name": target.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
        "size": target.metadata().map(|m| m.len()).unwrap_or(0),
    }))
}
