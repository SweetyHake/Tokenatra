use std::path::PathBuf;

use axum::extract::{Multipart, Path};
use axum::response::Response;
use serde_json::json;

use axum::http::StatusCode;
use super::{err_status, ok_json};
use crate::paths;

const EXTENSIONS: &[&str] = &["png", "webp", "jpg", "jpeg"];

pub async fn presets_list() -> Response {
    let mut presets = Vec::new();

    // Встроенные пресеты (read-only в установленной сборке)
    let builtin = paths::builtin_preset_dir();
    if builtin.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&builtin)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        files.sort();
        for f in files {
            let ext = super::ext_of(&f.to_string_lossy());
            if f.is_file() && EXTENSIONS.contains(&ext.as_str()) {
                presets.push(json!({
                    "name": f.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
                    "file": f.file_name().and_then(|s| s.to_str()).unwrap_or_default(),
                    "builtin": true,
                }));
            }
        }
    }

    // Пользовательские — сверху, новые первыми (по ctime)
    let user = paths::user_preset_dir();
    if user.is_dir() {
        let mut items: Vec<(std::time::SystemTime, String, PathBuf)> = Vec::new();
        for f in std::fs::read_dir(&user)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect::<Vec<_>>())
            .unwrap_or_default()
        {
            let ext = super::ext_of(&f.to_string_lossy());
            if f.is_file() && EXTENSIONS.contains(&ext.as_str()) {
                let added = f
                    .metadata()
                    .and_then(|m| m.created())
                    .unwrap_or(std::time::UNIX_EPOCH);
                items.push((
                    added,
                    f.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string(),
                    f,
                ));
            }
        }
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, _, f) in items {
            presets.push(json!({
                "name": f.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
                "file": f.file_name().and_then(|s| s.to_str()).unwrap_or_default(),
                "builtin": false,
            }));
        }
    }
    ok_json(json!(presets))
}

pub async fn preset_file(Path(filename): Path<String>) -> Response {
    let safe = sanitize_name(&filename);
    for base in [paths::user_preset_dir(), paths::builtin_preset_dir()] {
        let path = base.join(&safe);
        if path.is_file() {
            let ext = super::ext_of(&safe);
            let mime = match ext.as_str() {
                "png" => "image/png",
                "webp" => "image/webp",
                "jpg" | "jpeg" => "image/jpeg",
                "avif" => "image/avif",
                _ => "image/octet-stream",
            };
            return super::send_file_from_disk(&path, mime, Some("public, max-age=86400"));
        }
    }
    err_status(StatusCode::NOT_FOUND, "Not found")
}

fn sanitize_name(name: &str) -> String {
    std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

pub async fn add_preset(mut mp: Multipart) -> Response {
    let mut image: Option<(String, Vec<u8>)> = None;
    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        if name != "image" {
            continue;
        }
        let filename = field.file_name().map(|s| s.to_string()).unwrap_or_default();
        let Ok(data) = field.bytes().await else { continue };
        if filename.is_empty() {
            return err_status(StatusCode::BAD_REQUEST, "Нет данных маски");
        }
        image = Some((filename, data.to_vec()));
    }
    let Some((filename, data)) = image else {
        return err_status(StatusCode::BAD_REQUEST, "Нет данных маски");
    };
    // Проверка, что это корректное изображение
    if image::load_from_memory(&data).is_err() {
        return err_status(StatusCode::BAD_REQUEST, "Файл не является корректным изображением");
    }
    let dir = paths::user_preset_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось создать папку пресетов");
    }
    // Имя «Пресет N» по первому свободному номеру
    let mut num = 1;
    while dir.join(format!("Пресет {num}.png")).exists() {
        num += 1;
    }
    let target = dir.join(format!("Пресет {num}.png"));
    // Сохраняем как PNG независимо от входа? Python сохраняет оригинальные байты.
    // У нас имя .png, содержимое должно быть PNG-совместимым.
    let save_data = if super::ext_of(&filename) == "png" {
        data
    } else {
        // перекодируем в PNG ради соответствия расширения
        match image::load_from_memory(&data) {
            Ok(img) => {
                let mut buf = Vec::new();
                if image::DynamicImage::write_to(&img, std::io::Cursor::new(&mut buf), image::ImageFormat::Png).is_err() {
                    return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось сохранить пресет");
                }
                buf
            }
            Err(_) => return err_status(StatusCode::BAD_REQUEST, "Файл не является корректным изображением"),
        }
    };
    if std::fs::write(&target, save_data).is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось сохранить пресет");
    }
    ok_json(json!({
        "ok": true,
        "name": target.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
        "file": target.file_name().and_then(|s| s.to_str()).unwrap_or_default(),
    }))
}

pub async fn delete_preset(body: axum::body::Bytes) -> Response {
    let data: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let filename = data.get("file").and_then(|v| v.as_str()).unwrap_or_default();
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') {
        return err_status(StatusCode::BAD_REQUEST, "Некорректное имя файла");
    }
    let path = paths::user_preset_dir().join(filename);
    if !path.is_file() {
        return err_status(StatusCode::NOT_FOUND, "Пресет не найден");
    }
    // Короткий retry: файл может быть на мгновение занят (антивирус)
    let mut last_err = None;
    for attempt in 0..3 {
        match std::fs::remove_file(&path) {
            Ok(()) => return ok_json(json!({ "ok": true })),
            Err(e) if attempt < 2 => {
                std::thread::sleep(std::time::Duration::from_millis(150));
                last_err = Some(e);
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }
    let _ = last_err;
    err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось удалить пресет")
}
