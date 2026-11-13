use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Multipart, Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::json;

use super::{err_status, ok_json};
use crate::consts::builtin_ring_name;
use crate::foundry_rings::{
    find_companion_file, import_foundry_from_disk, import_foundry_from_memory,
    preview_foundry_json, read_ring_meta, sanitize_ring_stem, scan_foundry_folder,
    write_ring_meta, RingMeta,
};
use crate::paths::{self, norm_folder};
use crate::state::AppState;

const EXTENSIONS: &[&str] = &["webp", "png", "jpg", "jpeg"];

pub async fn rings_list() -> Response {
    paths::migrate_legacy_rings();
    let mut ring_files: Vec<(bool, bool, std::time::SystemTime, String, PathBuf)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut collect = |source_dir: &Path, builtin: bool| {
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
            if stem.ends_with(".mask") || stem.ends_with(".top") {
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
            let is_default = key == "rune-steel.webp";
            ring_files.push((builtin, is_default, added, key, f));
        }
    };
    collect(&paths::builtin_ring_dir(), true);
    collect(&paths::ring_dir(), false);

    ring_files.sort_by(|a, b| {
        // default (rune-steel) first, then builtin alphabetically, then user rings oldest-first
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| {
                if a.0 {
                    a.3.cmp(&b.3)
                } else {
                    a.2.cmp(&b.2).then_with(|| a.3.cmp(&b.3))
                }
            })
    });

    let mut rings = Vec::new();
    for (is_builtin, _is_default, added, _, f) in ring_files {
        let name_str = f
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let lower = name_str.to_lowercase();
        let stem = f
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let orig_ext = super::ext_of(&name_str);
        let meta = read_ring_meta(&f);

        let mask_file = find_companion_file(&f, "mask")
            .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(String::from));

        let top_file = meta
            .as_ref()
            .and_then(|m| m.top_file.clone())
            .filter(|tf| f.with_file_name(tf).is_file())
            .or_else(|| {
                find_companion_file(&f, "top")
                    .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
            });

        let scale = meta
            .as_ref()
            .and_then(|m| m.scale)
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0);

        let display_name = builtin_ring_name(&lower)
            .map(String::from)
            .or_else(|| meta.as_ref().and_then(|m| m.name.clone()))
            .unwrap_or_else(|| stem.clone());

        let name_en = meta.as_ref().and_then(|m| m.name_en.clone());

        rings.push(json!({
            "name": display_name,
            "name_en": name_en,
            "file": name_str,
            "mime": match orig_ext.as_str() {
                "webp" => "image/webp",
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                _ => "image/png",
            },
            "builtin": is_builtin || builtin_ring_name(&lower).is_some(),
            "mask_file": mask_file,
            "top_file": top_file,
            "scale": scale,
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
    let filename = data
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') {
        return err_status(StatusCode::BAD_REQUEST, "Некорректное имя файла");
    }
    let safe = Path::new(filename)
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
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        for suffix in ["mask", "top"] {
            for ext in [&orig_ext[..], "png", "webp", "jpg", "jpeg"] {
                let cp = path.with_file_name(format!("{stem}.{suffix}.{ext}"));
                if cp.is_file() {
                    let _ = std::fs::remove_file(&cp);
                }
            }
        }
        let mp = path.with_file_name(format!("{stem}.meta.json"));
        if mp.is_file() {
            let _ = std::fs::remove_file(&mp);
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
    let mut top: Option<(String, Vec<u8>)> = None;
    let mut name = String::new();
    let mut scale_val: Option<f32> = None;

    while let Ok(Some(field)) = mp.next_field().await {
        let fname = field.name().unwrap_or_default().to_string();
        let filename = field.file_name().map(|s| s.to_string()).unwrap_or_default();
        let Ok(data) = field.bytes().await else { continue };
        match fname.as_str() {
            "ring" if !filename.is_empty() => ring = Some((filename, data.to_vec())),
            "mask" if !filename.is_empty() => mask = Some((filename, data.to_vec())),
            "top" if !filename.is_empty() => top = Some((filename, data.to_vec())),
            "name" => name = String::from_utf8_lossy(&data).trim().to_string(),
            "scale" => {
                if let Ok(parsed) = String::from_utf8_lossy(&data).trim().parse::<f32>()
                    && parsed.is_finite()
                    && (0.2..=5.0).contains(&parsed)
                {
                    scale_val = Some(parsed);
                }
            }
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
    let mut top_ext: Option<String> = None;
    if let Some((tfilename, tdata)) = &top {
        let ext = super::ext_of(tfilename);
        if !EXTENSIONS.contains(&ext.as_str()) {
            return err_status(
                StatusCode::BAD_REQUEST,
                "Второй слой должен быть в формате WebP, PNG или JPG",
            );
        }
        if image::load_from_memory(tdata).is_err() {
            return err_status(
                StatusCode::BAD_REQUEST,
                "Файл второго слоя не является корректным изображением",
            );
        }
        top_ext = Some(ext);
    }

    let stem_src = if name.is_empty() {
        ring_filename.clone()
    } else {
        name.clone()
    };
    let stem = Path::new(&stem_src)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let safe_stem = sanitize_ring_stem(stem);

    let dir = paths::ring_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut target = dir.join(format!("{safe_stem}.{extension}"));
    let mut suffix = 2;
    while target.exists() {
        target = dir.join(format!("{safe_stem}_{suffix}.{extension}"));
        suffix += 1;
    }

    let target_stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    if let Some((_, mdata)) = &mask {
        let mname = format!("{target_stem}.mask.{}", mask_ext.as_deref().unwrap_or("png"));
        if std::fs::write(target.with_file_name(mname), mdata).is_err() {
            return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось сохранить кольцо");
        }
    }
    let mut saved_top_file: Option<String> = None;
    if let Some((_, tdata)) = &top {
        let tname = format!("{target_stem}.top.{}", top_ext.as_deref().unwrap_or("webp"));
        if std::fs::write(target.with_file_name(&tname), tdata).is_err() {
            return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось сохранить второй слой");
        }
        saved_top_file = Some(tname);
    }
    if std::fs::write(&target, &ring_data).is_err() {
        let _ = std::fs::remove_file(&target);
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось сохранить кольцо");
    }

    let needs_meta = !name.is_empty()
        || saved_top_file.is_some()
        || scale_val.map(|s| (s - 1.0).abs() > 1e-4).unwrap_or(false);
    if needs_meta {
        let meta = RingMeta {
            name: (!name.is_empty()).then_some(name.clone()),
            name_en: None,
            scale: scale_val,
            top_file: saved_top_file.clone(),
            source: Some("manual".to_string()),
        };
        let _ = write_ring_meta(&target, &meta);
    }

    ok_json(json!({
        "ok": true,
        "name": if name.is_empty() { target_stem.clone() } else { name },
        "file": target.file_name().and_then(|s| s.to_str()).unwrap_or_default(),
        "top_file": saved_top_file,
        "scale": scale_val.unwrap_or(1.0),
    }))
}

pub async fn pick_foundry_rings(State(state): State<Arc<AppState>>) -> Response {
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Выберите JSON спрайт-листы колец Foundry")
            .add_filter("Foundry Ring JSON", &["json"])
            .pick_files()
    })
    .await
    .unwrap_or(None);

    let Some(paths) = picked else {
        return ok_json(json!({ "cancelled": true }));
    };

    let mut items = Vec::new();
    let mut errors = Vec::new();
    for p in paths {
        if let Some(parent) = p.parent() {
            state.remember_picked_folder(parent.to_path_buf());
        }
        match preview_foundry_json(&p) {
            Ok(prev) => items.push(prev),
            Err(err) => {
                let fname = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file.json");
                errors.push(format!("{fname}: {err}"));
            }
        }
    }
    ok_json(json!({ "items": items, "errors": errors }))
}

pub async fn pick_foundry_folder(State(state): State<Arc<AppState>>) -> Response {
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Выберите папку модуля или паков колец Foundry")
            .pick_folder()
    })
    .await
    .unwrap_or(None);

    let Some(folder) = picked else {
        return ok_json(json!({ "cancelled": true }));
    };

    state.remember_picked_folder(folder.clone());
    let items = tokio::task::spawn_blocking(move || scan_foundry_folder(&folder))
        .await
        .unwrap_or_default();
    ok_json(json!({ "items": items, "errors": [] }))
}

fn path_is_within_picked(state: &AppState, file_path: &Path) -> bool {
    let picked = state.picked_folders.lock().unwrap();
    let folder = norm_folder(file_path.parent().unwrap_or(file_path));
    picked.iter().any(|allowed| {
        let a = norm_folder(allowed);
        folder == a || folder.starts_with(&a)
    })
}

pub async fn import_foundry_from_paths(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Response {
    let data: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let Some(items) = data.get("items").and_then(|v| v.as_array()) else {
        return err_status(StatusCode::BAD_REQUEST, "Передайте список колец items");
    };
    if items.is_empty() {
        return err_status(StatusCode::BAD_REQUEST, "Список колец пуст");
    }

    let mut requests: Vec<(PathBuf, Option<String>)> = Vec::new();
    for item in items {
        let Some(pstr) = item.get("json_path").and_then(|v| v.as_str()) else {
            continue;
        };
        let p = PathBuf::from(pstr);
        if !p.is_file() {
            return err_status(StatusCode::NOT_FOUND, &format!("Файл не найден: {pstr}"));
        }
        if !path_is_within_picked(&state, &p) {
            return err_status(
                StatusCode::FORBIDDEN,
                "Путь не был выбран через диалог",
            );
        }
        let custom_name = item
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from);
        requests.push((p, custom_name));
    }

    let target_dir = paths::ring_dir();
    let result = tokio::task::spawn_blocking(move || {
        let mut imported = Vec::new();
        let mut errors = Vec::new();
        for (p, cname) in requests {
            match import_foundry_from_disk(&p, cname.as_deref(), &target_dir) {
                Ok((name, file, has_top, scale)) => {
                    imported.push(json!({
                        "name": name,
                        "file": file,
                        "has_top": has_top,
                        "scale": scale,
                    }));
                }
                Err(e) => {
                    let fname = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("ring.json");
                    errors.push(format!("{fname}: {e}"));
                }
            }
        }
        (imported, errors)
    })
    .await
    .unwrap_or_else(|e| (Vec::new(), vec![e.to_string()]));

    if result.0.is_empty() {
        let msg = result
            .1
            .into_iter()
            .next()
            .unwrap_or_else(|| "Не удалось импортировать кольца".to_string());
        return err_status(StatusCode::BAD_REQUEST, &msg);
    }

    ok_json(json!({
        "ok": true,
        "imported": result.0,
        "errors": result.1,
    }))
}

pub async fn import_foundry_upload(mut mp: Multipart) -> Response {
    let mut jsons: Vec<(String, Vec<u8>)> = Vec::new();
    let mut files_by_name: HashMap<String, Vec<u8>> = HashMap::new();
    let mut custom_names: HashMap<String, String> = HashMap::new();

    while let Ok(Some(field)) = mp.next_field().await {
        let field_name = field.name().unwrap_or_default().to_string();
        let filename = field.file_name().map(|s| s.to_string()).unwrap_or_default();
        let Ok(data) = field.bytes().await else { continue };

        if field_name == "names" {
            if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(&data) {
                custom_names = map;
            }
            continue;
        }
        if field_name == "name" && !data.is_empty() {
            let n = String::from_utf8_lossy(&data).trim().to_string();
            if !n.is_empty() {
                custom_names.insert("*".to_string(), n);
            }
            continue;
        }
        if filename.is_empty() {
            continue;
        }
        let norm = filename.trim().replace('\\', "/").to_lowercase();
        let base = Path::new(&norm)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&norm)
            .to_string();
        if base.ends_with(".json") {
            jsons.push((filename, data.to_vec()));
        } else {
            let raw = data.to_vec();
            files_by_name.insert(norm, raw.clone());
            files_by_name.insert(base, raw);
        }
    }

    if jsons.is_empty() {
        return err_status(StatusCode::BAD_REQUEST, "Выберите хотя бы один .json файл кольца");
    }

    let target_dir = paths::ring_dir();
    let result = tokio::task::spawn_blocking(move || {
        let mut imported = Vec::new();
        let mut errors = Vec::new();
        let multi = jsons.len() > 1;
        for (jname, jbytes) in jsons {
            let cname = custom_names
                .get(&jname)
                .or_else(|| (!multi).then(|| custom_names.get("*")).flatten())
                .map(String::as_str);
            match import_foundry_from_memory(&jname, &jbytes, &files_by_name, cname, &target_dir) {
                Ok((name, file, has_top, scale)) => {
                    imported.push(json!({
                        "name": name,
                        "file": file,
                        "has_top": has_top,
                        "scale": scale,
                    }));
                }
                Err(e) => {
                    errors.push(format!("{jname}: {e}"));
                }
            }
        }
        (imported, errors)
    })
    .await
    .unwrap_or_else(|e| (Vec::new(), vec![e.to_string()]));

    if result.0.is_empty() {
        let msg = result
            .1
            .into_iter()
            .next()
            .unwrap_or_else(|| "Не удалось импортировать кольца".to_string());
        return err_status(StatusCode::BAD_REQUEST, &msg);
    }

    ok_json(json!({
        "ok": true,
        "imported": result.0,
        "errors": result.1,
    }))
}

pub async fn ring_file(AxumPath(filename): AxumPath<String>) -> Response {
    let safe = Path::new(&filename)
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
        "json" => "application/json",
        _ => "image/octet-stream",
    };
    super::send_file_from_disk(&path, mime, Some("public, max-age=86400"))
}
