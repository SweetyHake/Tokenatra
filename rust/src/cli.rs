//! Context menu & CLI verbs.

use std::path::{Path, PathBuf};

use crate::consts::PORT;

fn base_url() -> String {
    format!("http://localhost:{PORT}")
}

fn server_running() -> bool {
    reqwest::blocking::Client::new()
        .get(format!("{}/device", base_url()))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .is_ok()
}

fn read(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

pub fn remove_bg(file: &str) -> i32 {
    let path = PathBuf::from(file);
    if !path.exists() {
        return 1;
    }

    // background removal settings mirror the app config
    let (mut fmt, mut quality, mut edge_blur) = ("webp".to_string(), "90".to_string(), "1".to_string());
    if let Ok(cfg) = crate::config::load() {
        if let Some(r) = cfg.get("remover").and_then(|v| v.as_object()) {
            if let Some(f) = r.get("format").and_then(|v| v.as_str()) {
                fmt = f.to_string();
            }
            if let Some(q) = r.get("quality") {
                if let Some(n) = q.as_i64() {
                    quality = n.to_string();
                } else if let Some(s) = q.as_str() {
                    quality = s.trim().to_string();
                }
            }
        }
        if let Some(eb) = cfg.get("edgeBlur") {
            if let Some(n) = eb.as_f64() {
                edge_blur = n.to_string();
            } else if let Some(s) = eb.as_str() {
                edge_blur = s.trim().to_string();
            }
        }
    }

    let Some(data) = read(&path) else { return 1 };

    let result = if server_running() {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image.png")
            .replace(['"', '\r', '\n', '\\'], "_");

        let boundary = "TokenatraBoundary";
        let mut body: Vec<u8> = Vec::new();
        let head = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{filename}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        );
        body.extend_from_slice(head.as_bytes());
        body.extend_from_slice(&data);
        let tail = format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"format\"\r\n\r\n{fmt}\
             \r\n--{boundary}\r\nContent-Disposition: form-data; name=\"quality\"\r\n\r\n{quality}\
             \r\n--{boundary}\r\nContent-Disposition: form-data; name=\"edge_blur\"\r\n\r\n{edge_blur}\
             \r\n--{boundary}--\r\n"
        );
        body.extend_from_slice(tail.as_bytes());

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap();
        let resp = match client
            .post(format!("{}/process", base_url()))
            .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
            .header("Origin", base_url())
            .body(body)
            .send()
        {
            Ok(r) => r,
            Err(_) => return 1,
        };
        if !resp.status().is_success() {
            let err = resp.text().unwrap_or_default();
            crate::ffmpeg::notify(&format!(
                "Ошибка сервера: {}",
                err.chars().take(200).collect::<String>()
            ));
            return 1;
        }
        resp.bytes().unwrap_or_default().to_vec()
    } else {
        // Fallback: run local ONNX inference directly without a running server
        crate::paths::migrate_legacy_model();
        let state = crate::state::AppState::new();
        crate::infer::detect_device(&state);
        let Ok(img) = crate::imaging::open_image(&data) else {
            return 1;
        };
        let img = crate::imaging::validate_image(img);
        let eb = edge_blur.parse::<f64>().unwrap_or(1.0).clamp(0.0, 10.0);
        let q = quality.parse::<i64>().unwrap_or(90).clamp(10, 100) as u8;
        let rgba = match crate::infer::remove_background(&state, img, eb) {
            Ok(rgba) => rgba,
            Err(e) => {
                crate::ffmpeg::notify(&format!(
                    "Ошибка удаления фона: {}",
                    e.to_string().chars().take(200).collect::<String>()
                ));
                return 1;
            }
        };
        let dyn_img = image::DynamicImage::ImageRgba8(rgba);
        match crate::imaging::save_image(&dyn_img, &fmt, q) {
            Ok((bytes, _)) => bytes,
            Err(_) => return 1,
        }
    };

    let ext_map = [("webp", "webp"), ("png", "png"), ("jpg", "jpg"), ("avif", "avif")];
    let out_ext = ext_map
        .iter()
        .find(|(k, _)| *k == fmt)
        .map(|(_, v)| *v)
        .unwrap_or("webp");
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let out_path = path.with_file_name(format!("{stem}.{out_ext}"));
    // atomic write
    if crate::ffmpeg::atomic_write(&out_path, &result).is_err() {
        return 1;
    }
    0
}

pub fn to_webp(file: &str) -> i32 {
    let path = PathBuf::from(file);
    if !path.exists() {
        return 1;
    }
    if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("webp")).unwrap_or(false) {
        println!("Уже WebP, пропущено: {}", path.display());
        return 0;
    }
    let Some(data) = read(&path) else { return 1 };
    let Ok(img) = crate::imaging::open_image(&data) else { return 1 };
    let out_path = path.with_extension("webp");
    match crate::imaging::save_image(&img, "webp", 90) {
        Ok((bytes, _)) => {
            if crate::ffmpeg::atomic_write(&out_path, &bytes).is_ok() {
                if !crate::paths::same_path(&path, &out_path) {
                    let _ = std::fs::remove_file(&path);
                }
                println!("Сохранено: {}", out_path.display());
                0
            } else {
                1
            }
        }
        Err(_) => 1,
    }
}

pub fn to_ogg(file: &str) -> i32 {
    let path = PathBuf::from(file);
    if !path.exists() {
        return 1;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let media = [
        "mp3", "wav", "m4a", "flac", "aac", "wma", "opus", "ogg", "mp4", "mkv", "mov", "avi",
        "webm", "mpeg", "mpg", "m4v",
    ]
    .contains(&ext.as_str());
    if !media {
        return 1;
    }
    if ext == "ogg" {
        return 0;
    }
    let Some(data) = read(&path) else { return 1 };
    match crate::ffmpeg::convert_media_to_ogg(&data, &format!(".{ext}"), 90) {
        Ok(out) => {
            let out_path = path.with_extension("ogg");
            if crate::ffmpeg::atomic_write(&out_path, &out).is_ok() {
                if !crate::paths::same_path(&path, &out_path) {
                    let _ = std::fs::remove_file(&path);
                }
                0
            } else {
                1
            }
        }
        Err(e) => {
            crate::ffmpeg::notify(&e);
            1
        }
    }
}

pub fn folder_to_webp(folder: &str) -> i32 {
    let dir = PathBuf::from(folder);
    if !dir.is_dir() {
        return 1;
    }
    let exts = ["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "tif"];
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.is_file()
                        && !p.extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.eq_ignore_ascii_case("webp"))
                            .unwrap_or(false)
                        && p.extension()
                            .and_then(|e| e.to_str())
                            .map(|e| exts.contains(&e.to_lowercase().as_str()))
                            .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    if files.is_empty() {
        crate::ffmpeg::notify("Нет изображений для конвертации.");
        return 0;
    }
    let mut converted = 0usize;
    let mut failed = 0usize;
    for f in files {
        let out = f.with_extension("webp");
        let ok = read(&f)
            .and_then(|d| crate::imaging::open_image(&d).ok())
            .and_then(|img| crate::imaging::save_image(&img, "webp", 90).ok())
            .map(|(bytes, _)| crate::ffmpeg::atomic_write(&out, &bytes).is_ok())
            .unwrap_or(false);
        if ok {
            if !crate::paths::same_path(&f, &out) {
                let _ = std::fs::remove_file(&f);
            }
            converted += 1;
        } else {
            failed += 1;
        }
    }
    if failed > 0 {
        crate::ffmpeg::notify(&format!("Конвертировано: {converted}, ошибок: {failed}"));
    } else {
        crate::ffmpeg::notify(&format!("Готово. Конвертировано файлов: {converted}."));
    }
    0
}
