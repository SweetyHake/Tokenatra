use axum::extract::{Multipart, State};
use axum::response::Response;

use axum::http::StatusCode;
use super::{err_status, file_response, ok_json};
use crate::state::AppState;

/// POST /process: background removal.
pub async fn process(
    State(state): State<std::sync::Arc<AppState>>,
    mut mp: Multipart,
) -> Response {
    state.mark_activity();

    let mut image: Option<(String, Vec<u8>)> = None;
    let mut format_type = "webp".to_string();
    let mut quality: u8 = 90;
    let mut edge_blur: f64 = 1.0;

    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "image" => {
                let filename = field.file_name().map(|s| s.to_string()).unwrap_or_default();
                if filename.is_empty() {
                    return err_status(StatusCode::BAD_REQUEST, "Файл не выбран");
                }
                let Ok(b) = field.bytes().await else {
                    return err_status(StatusCode::BAD_REQUEST, "Нет изображения");
                };
                image = Some((filename, b.to_vec()));
            }
            "format" => {
                if let Ok(b) = field.bytes().await {
                    let v = String::from_utf8_lossy(&b).trim().to_string();
                    if ["webp", "png", "jpg", "avif"].contains(&v.as_str()) {
                        format_type = v;
                    }
                }
            }
            "quality" => {
                if let Ok(b) = field.bytes().await {
                    if let Ok(v) = String::from_utf8_lossy(&b).trim().parse::<i64>() {
                        quality = v.clamp(10, 100) as u8;
                    }
                }
            }
            "edge_blur" => {
                if let Ok(b) = field.bytes().await {
                    if let Ok(v) = String::from_utf8_lossy(&b).trim().parse::<f64>() {
                        edge_blur = v.clamp(0.0, 10.0);
                    }
                }
            }
            _ => {}
        }
    }

    let Some((filename, data)) = image else {
        return err_status(StatusCode::BAD_REQUEST, "Нет изображения");
    };
    if !crate::imaging::allowed_image_ext(&filename) {
        return err_status(StatusCode::BAD_REQUEST, "Недопустимый формат файла");
    }

    let st = state.clone();
    let fmt = format_type.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, &'static str), String> {
        let img = crate::imaging::open_image(&data).map_err(|e| e.to_string())?;
        let img = crate::imaging::validate_image(img);
        let rgba = crate::infer::remove_background(&st, img, edge_blur)
            .map_err(|e| {
                eprintln!("process error: {e:#}");
                match e.downcast_ref::<std::io::Error>() {
                    Some(io_err) if io_err.kind() == std::io::ErrorKind::NotFound => {
                        "Модель не найдена. Загрузите файл .onnx в разделе «Настройки»".to_string()
                    }
                    _ => "Ошибка обработки изображения".to_string(),
                }
            })?;
        let dyn_img = image::DynamicImage::ImageRgba8(rgba);
        crate::imaging::save_image(&dyn_img, &fmt, quality)
            .map_err(|_| "Ошибка обработки изображения".to_string())
    })
    .await;

    state.mark_activity();

    match result {
        Ok(Ok((bytes, mime))) => file_response(bytes, mime, None),
        Ok(Err(msg)) => {
            // the "model not found" message goes out with 500 (as before)
            err_status(StatusCode::INTERNAL_SERVER_ERROR, &msg)
        }
        Err(e) => {
            eprintln!("process join error: {e}");
            err_status(StatusCode::INTERNAL_SERVER_ERROR, "Ошибка обработки изображения")
        }
    }
}

/// POST /convert: image and media transcoding.
pub async fn convert_file(mut mp: Multipart) -> Response {
    let mut file: Option<(String, String, Option<String>, Vec<u8>)> = None; // name, filename, content_type, data
    let mut format_type = "webp".to_string();
    let mut quality: u8 = 90;

    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "image" => {
                let filename = field.file_name().map(|s| s.to_string()).unwrap_or_default();
                let ct = field.content_type().map(|s| s.to_string());
                let Ok(b) = field.bytes().await else { continue };
                file = Some((name, filename, ct, b.to_vec()));
            }
            "format" => {
                if let Ok(b) = field.bytes().await {
                    let v = String::from_utf8_lossy(&b).trim().to_string();
                    if ["webp", "png", "jpg", "avif", "bmp", "gif", "tiff", "ogg"]
                        .contains(&v.as_str())
                    {
                        format_type = v;
                    }
                }
            }
            "quality" => {
                if let Ok(b) = field.bytes().await {
                    if let Ok(v) = String::from_utf8_lossy(&b).trim().parse::<i64>() {
                        quality = v.clamp(10, 100) as u8;
                    }
                }
            }
            _ => {}
        }
    }

    let Some((_, filename, content_type, data)) = file else {
        return err_status(StatusCode::BAD_REQUEST, "Файл не выбран");
    };

    let media = crate::imaging::is_media_ext(&filename, content_type.as_deref());
    if media && format_type != "ogg" {
        return err_status(StatusCode::BAD_REQUEST, "Аудио и видео можно конвертировать только в OGG");
    }
    if format_type == "ogg" && !media {
        return err_status(StatusCode::BAD_REQUEST, "Формат OGG доступен только для аудио и видео");
    }

    let fmt = format_type.clone();
    let fname = filename.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, &'static str), String> {
        if fmt == "ogg" {
            let ext = std::path::Path::new(&fname)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{e}"))
                .unwrap_or_else(|| ".media".into());
            let out = crate::ffmpeg::convert_media_to_ogg(&data, &ext, quality)?;
            return Ok((out, "audio/ogg"));
        }
        let img = crate::imaging::open_image(&data).map_err(|e| e.to_string())?;
        crate::imaging::save_image(&img, &fmt, quality)
            .map_err(|_| "Ошибка конвертации файла".to_string())
    })
    .await;

    match result {
        Ok(Ok((bytes, mime))) => file_response(bytes, mime, None),
        Ok(Err(msg)) => err_status(StatusCode::INTERNAL_SERVER_ERROR, &msg),
        Err(e) => {
            eprintln!("convert join error: {e}");
            err_status(StatusCode::INTERNAL_SERVER_ERROR, "Ошибка конвертации файла")
        }
    }
}

pub async fn _unused(_s: State<std::sync::Arc<AppState>>) -> Response {
    ok_json(serde_json::json!({}))
}
