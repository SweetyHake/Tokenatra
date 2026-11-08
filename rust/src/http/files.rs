use std::path::{Path, PathBuf};

use axum::extract::{Multipart, State};
use axum::response::Response;
use serde_json::json;

use axum::http::StatusCode;
use super::{err_status, ok_json};
use crate::paths::{clean_path, norm_folder};
use crate::state::AppState;

pub async fn save_file(mut mp: Multipart) -> Response {
    let mut filename = "file.webp".to_string();
    let mut data: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or_default() {
            "filename" => {
                if let Ok(b) = field.bytes().await {
                    filename = String::from_utf8_lossy(&b).trim().to_string();
                    if filename.is_empty() {
                        filename = "file.webp".into();
                    }
                }
            }
            "file" => {
                if let Ok(b) = field.bytes().await {
                    data = Some(b.to_vec());
                }
            }
            _ => {}
        }
    }
    let Some(data) = data else {
        return err_status(StatusCode::BAD_REQUEST, "No file");
    };

    let ext = super::ext_of(&filename);

    let suggested = filename.clone();
    let picked = tokio::task::spawn_blocking(move || {
        // file type filter: all export formats, the selected file format first
        // (the dialog opens with it), then the rest
        let mut types: Vec<(&str, Vec<&str>)> = vec![
            ("WebP Image", vec!["webp"]),
            ("PNG Image", vec!["png"]),
            ("JPEG Image", vec!["jpg", "jpeg"]),
            ("AVIF Image", vec!["avif"]),
        ];
        types.sort_by_key(|(_, exts)| {
            if exts.contains(&ext.as_str()) { 0 } else { 1 }
        });
        let mut dlg = rfd::FileDialog::new().set_file_name(&suggested);
        for (name, exts) in &types {
            dlg = dlg.add_filter(*name, exts.as_slice());
        }
        dlg.add_filter("All files", &["*"]).save_file()
    })
    .await
    .unwrap_or(None);

    let Some(path) = picked else {
        return ok_json(json!({ "cancelled": true }));
    };
    match std::fs::write(&path, data) {
        Ok(()) => ok_json(json!({ "saved": true, "path": path.to_string_lossy() })),
        Err(e) => err_status(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

pub async fn pick_folder(State(state): State<std::sync::Arc<AppState>>) -> Response {
    let picked = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_folder())
        .await
        .unwrap_or(None);
    let Some(path) = picked else {
        return ok_json(json!({ "cancelled": true }));
    };
    state.remember_picked_folder(path.clone());
    ok_json(json!({ "path": path.to_string_lossy() }))
}

pub async fn save_to_folder(
    State(state): State<std::sync::Arc<AppState>>,
    mut mp: Multipart,
) -> Response {
    let mut folder = String::new();
    let mut filename = "file.webp".to_string();
    let mut data: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or_default() {
            "folder" => {
                if let Ok(b) = field.bytes().await {
                    folder = String::from_utf8_lossy(&b).trim().to_string();
                }
            }
            "filename" => {
                if let Ok(b) = field.bytes().await {
                    filename = String::from_utf8_lossy(&b).trim().to_string();
                }
            }
            "file" => {
                if let Ok(b) = field.bytes().await {
                    data = Some(b.to_vec());
                }
            }
            _ => {}
        }
    }
    if folder.is_empty() {
        return err_status(StatusCode::BAD_REQUEST, "No folder");
    }
    let Some(data) = data else {
        return err_status(StatusCode::BAD_REQUEST, "No file");
    };
    let folder_path = PathBuf::from(&folder);
    if !folder_path.is_dir() {
        return err_status(StatusCode::BAD_REQUEST, "Invalid folder");
    }

    // only folders from this session's dialog or lastFolders from config
    let mut allowed: Vec<PathBuf> = state
        .picked_folders
        .lock()
        .unwrap()
        .iter()
        .map(|p| norm_folder(p))
        .collect();
    if let Ok(cfg) = crate::config::load()
        && let Some(folders) = cfg.get("lastFolders").and_then(|v| v.as_object()) {
            for (_, v) in folders {
                if let Some(s) = v.as_str()
                    && !s.is_empty() {
                        allowed.push(norm_folder(Path::new(s)));
                    }
            }
        }
    if !allowed.contains(&norm_folder(&folder_path)) {
        return err_status(StatusCode::FORBIDDEN, "Folder was not picked via dialog");
    }

    let out = folder_path.join(
        Path::new(&filename)
            .file_name()
            .unwrap_or_default(),
    );
    match std::fs::write(&out, data) {
        Ok(()) => ok_json(json!({ "saved": true, "path": out.to_string_lossy() })),
        Err(e) => err_status(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

pub async fn pick_image_to_open() -> Response {
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Выберите изображение")
            .add_filter("Images", &["webp", "png", "jpg", "jpeg"])
            .add_filter("All files", &["*"])
            .pick_file()
    })
    .await
    .unwrap_or(None);
    let Some(path) = picked else {
        return ok_json(json!({ "cancelled": true }));
    };
    let p = PathBuf::from(&path);
    let ext = super::ext_of(&p.to_string_lossy());
    let mime = match ext.as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "avif" => "image/avif",
        _ => "image/octet-stream",
    };
    let resolved = std::fs::canonicalize(&p).unwrap_or(p);
    ok_json(json!({ "path": clean_path(&resolved), "mime": mime }))
}

pub async fn list_images(body: axum::body::Bytes) -> Response {
    let data: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let path_str = data.get("path").and_then(|v| v.as_str()).unwrap_or_default();
    if path_str.is_empty() {
        return err_status(StatusCode::BAD_REQUEST, "No path provided");
    }
    let path = PathBuf::from(path_str);
    let parent = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    if !parent.is_dir() {
        return err_status(StatusCode::NOT_FOUND, "Directory not found");
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&parent)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.is_file()
                        && ["webp", "png", "jpg", "jpeg"]
                            .contains(&super::ext_of(&p.to_string_lossy()).as_str())
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();

    let current_name = path
        .canonicalize()
        .unwrap_or(path.clone())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let mut current_index = -1i64;
    for (i, fp) in files.iter().enumerate() {
        if fp.file_name().and_then(|n| n.to_str()) == Some(current_name.as_str()) {
            current_index = i as i64;
            break;
        }
    }
    let resolved: Vec<String> = files
        .iter()
        .map(|f| clean_path(&std::fs::canonicalize(f).unwrap_or_else(|_| f.clone())))
        .collect();
    ok_json(json!({
        "folder": clean_path(&std::fs::canonicalize(&parent).unwrap_or(parent)),
        "files": resolved,
        "currentIndex": current_index,
        "total": files.len(),
    }))
}

pub async fn get_image_by_path(uri: axum::http::Uri) -> Response {
    let path_str = super::query_pairs(&uri)
        .get("path")
        .cloned()
        .unwrap_or_default();
    if path_str.is_empty() {
        return err_status(StatusCode::BAD_REQUEST, "No path");
    }
    let path = PathBuf::from(path_str);
    if !path.is_file() {
        return err_status(StatusCode::NOT_FOUND, "File not found");
    }
    let ext = super::ext_of(&path.to_string_lossy());
    let mime = match ext.as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "avif" => "image/avif",
        _ => "image/octet-stream",
    };
    super::send_file_from_disk(&path, mime, None)
}
