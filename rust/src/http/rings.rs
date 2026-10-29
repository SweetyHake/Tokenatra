use std::path::PathBuf;

use axum::extract::{Multipart, Path};
use axum::response::Response;
use serde_json::json;

use axum::http::StatusCode;
use super::{err_status, ok_json};
use crate::consts::builtin_ring_name;
use crate::paths;

const EXTENSIONS: &[&str] = &["webp", "png", "jpg", "jpeg"];

pub async fn rings_list() -> Response {
    paths::migrate_legacy_rings();
    let mut ring_files: Vec<(bool, std::time::SystemTime, String, PathBuf)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut collect = |source_dir: &std::path::Path, builtin: bool| {
        if !source_dir.is_dir() {
            return;
        }
        let entries = std::fs::read_dir(source_dir)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect::<Vec<_>>())
            .unwrap_or_default();
        for f in entries {
            let ext = super::ext_of(&f.to_string_lossy());
            if !f.is_file() || !EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }
            let stem = f
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if stem.ends_with(".mask") {
                continue;
            }
            let key = f
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key.clone());
            let added = f
                .metadata()
                .and_then(|m| m.created())
                .unwrap_or(std::time::UNIX_EPOCH);
            ring_files.push((builtin, added, key, f));
        }
    };
    collect(&paths::builtin_ring_dir(), true);
    collect(&paths::ring_dir(), false);

    ring_files.sort_by(|a, b| {
        // builtin первыми, затем старые первыми
        b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2))
    });

    let mut rings = Vec::new();
    for (_is_builtin, added, _, f) in ring_files {
        let name_str = f.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let lower = name_str.to_lowercase();
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        // Поиск связанной маски <stem>.mask<ext>
        let mut mask_file: Option<String> = None;
        let orig_ext = super::ext_of(&name_str);
        for ext in [&orig_ext[..], "png", "webp", "jpg", "jpeg"] {
            let mp = f.with_file_name(format!(
                "{}.mask.{}",
                f.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
                ext
            ));
            if mp.is_file() {
                mask_file = mp.file_name().and_then(|n| n.to_str()).map(String::from);
                break;
            }
        }
        rings.push(json!({
            "name": builtin_ring_name(&lower).unwrap_or(&stem),
            "file": name_str,
            "mime": match orig_ext.as_str() {
                "webp" => "image/webp",
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                _ => "image/png",
            },
            "builtin": builtin_ring_name(&lower).is_some(),
            "mask_file": mask_file,
            "added_at": system_time_ns(added),
        }));
    }
    ok_json(json!(rings))
}

fn system_time_ns(t: std::time::SystemTime) -> u128 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

pub async fn delete_ring(body: axum::body::Bytes) -> Response {
    let data: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let filename = data.get("file").and_then(|v| v.as_str()).unwrap_or_default();
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') {
        return err_status(StatusCode::BAD_REQUEST, "Некорректное имя файла");
    }
    let safe = std::path::Path::new(filename)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    if builtin_ring_name(&safe.to_lowercase()).is_some() {
        return err_status(StatusCode::FORBIDDEN, "Встроенное кольцо нельзя удалить");
    }
    let path = paths::ring_dir().join(&safe);
    if !path.is_file() {
        return err_status(StatusCode::NOT_FOUND, "Кольцо не найдено");
    }
    let res = (|| -> std::io::Result<()> {
        std::fs::remove_file(&path)?;
        let orig_ext = super::ext_of(&safe);
        for ext in [&orig_ext[..], "png", "webp", "jpg", "jpeg"] {
            let mp = path.with_file_name(format!(
                "{}.mask.{}",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
                ext
            ));
            if mp.is_file() {
                let _ = std::fs::remove_file(&mp);
            }
        }
        Ok(())
    })();
    match res {
        Ok(()) => ok_json(json!({ "ok": true })),
        Err(_) => err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось удалить кольцо"),
    }
}

pub async fn add_ring(mut mp: Multipart) -> Response {
    let mut ring: Option<(String, Vec<u8>)> = None;
    let mut mask: Option<(String, Vec<u8>)> = None;
    let mut name = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        let fname = field.name().unwrap_or_default().to_string();
        let filename = field.file_name().map(|s| s.to_string()).unwrap_or_default();
        let Ok(data) = field.bytes().await else { continue };
        match fname.as_str() {
            "ring" if !filename.is_empty() => ring = Some((filename, data.to_vec())),
            "mask" if !filename.is_empty() => mask = Some((filename, data.to_vec())),
            "name" => name = String::from_utf8_lossy(&data).trim().to_string(),
            _ => {}
        }
    }
    let Some((ring_filename, ring_data)) = ring else {
        return err_status(StatusCode::BAD_REQUEST, "Выберите изображение кольца");
    };
    let extension = super::ext_of(&ring_filename);
    if !EXTENSIONS.contains(&extension.as_str()) {
        return err_status(StatusCode::BAD_REQUEST, "Поддерживаются только WebP, PNG и JPG");
    }
    if image::load_from_memory(&ring_data).is_err() {
        return err_status(StatusCode::BAD_REQUEST, "Файл не является корректным изображением");
    }
    let mut mask_ext: Option<String> = None;
    if let Some((mfilename, mdata)) = &mask {
        let ext = super::ext_of(mfilename);
        if !EXTENSIONS.contains(&ext.as_str()) {
            return err_status(StatusCode::BAD_REQUEST, "Маска должна быть в формате WebP, PNG или JPG");
        }
        if image::load_from_memory(mdata).is_err() {
            return err_status(StatusCode::BAD_REQUEST, "Файл маски не является корректным изображением");
        }
        mask_ext = Some(ext);
    }

    let stem_src = if name.is_empty() { ring_filename.clone() } else { name.clone() };
    let stem = std::path::Path::new(&stem_src)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let mut safe_stem: String = stem
        .chars()
        .filter(|c| c.is_alphanumeric() || [' ', '_', '-'].contains(c))
        .collect::<String>()
        .trim_matches([' ', '.', '_', '-'])
        .to_string();
    if safe_stem.is_empty() {
        safe_stem = "Кольцо".into();
    }

    let dir = paths::ring_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut target = dir.join(format!("{safe_stem}.{extension}"));
    let mut suffix = 2;
    while target.exists() {
        target = dir.join(format!("{safe_stem}_{suffix}.{extension}"));
        suffix += 1;
    }

    if let Some((_, mdata)) = &mask {
        let mname = format!(
            "{}.mask.{}",
            target.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
            mask_ext.as_deref().unwrap_or("png")
        );
        if std::fs::write(target.with_file_name(mname), mdata).is_err() {
            return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось сохранить кольцо");
        }
    }
    if std::fs::write(&target, &ring_data).is_err() {
        let _ = std::fs::remove_file(&target);
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось сохранить кольцо");
    }
    ok_json(json!({
        "ok": true,
        "name": target.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
        "file": target.file_name().and_then(|s| s.to_str()).unwrap_or_default(),
    }))
}

pub async fn ring_file(Path(filename): Path<String>) -> Response {
    let safe = std::path::Path::new(&filename)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let mut path = paths::ring_dir().join(safe);
    if !path.is_file() {
        path = paths::builtin_ring_dir().join(safe);
    }
    if !path.is_file() {
        return err_status(StatusCode::NOT_FOUND, "Not found");
    }
    let ext = super::ext_of(safe);
    let mime = match ext.as_str() {
        "webp" => "image/webp",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "image/octet-stream",
    };
    super::send_file_from_disk(&path, mime, Some("public, max-age=86400"))
}
