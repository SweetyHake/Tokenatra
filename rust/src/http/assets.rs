use std::io::Cursor;

use axum::extract::Path;
use axum::response::Response;

use axum::http::StatusCode;
use super::{err_status, file_response, mime_by_ext, query_pairs};
use crate::consts::{GITHUB_REPO, VERSION};
use crate::paths;

/// Renders templates/index.html, substituting the Jinja placeholders.
pub fn render_index() -> String {
    let tpl = std::fs::read_to_string(paths::resource_dir().join("templates").join("index.html"))
        .unwrap_or_else(|_| "<html><body>index.html missing</body></html>".into());
    let allowed = crate::config::allowed_origins();
    let embed_json = serde_json::to_string(&allowed).unwrap_or_else(|_| "[]".into());
    let mut out = tpl.replace("{{ github_repo }}", GITHUB_REPO);
    out = out.replace("{{ __version__ }}", VERSION);
    out = out.replace("{{ embed_allowed|tojson }}", &embed_json);
    // {{ url_for('static', filename='X', v=__version__) }} -> /static/X?v=VERSION
    // (url_for has a ", v=__version__" tail, part of the substitution)
    let marker = "{{ url_for('static', filename='";
    while let Some(pos) = out.find(marker) {
        let after = &out[pos + marker.len()..];
        // filename ends with ') or ', take everything up to the closing "}}}"
        let Some(end_rel) = after.find('\'') else { break };
        let filename = &after[..end_rel];
        let rest = &after[end_rel..];
        // skip remaining url_for args up to "}}"
        let Some(close) = rest.find("}}") else { break };
        let replacement = format!("/static/{filename}?v={VERSION}");
        let end_full = pos + marker.len() + end_rel + close + 2;
        out.replace_range(pos..end_full, &replacement);
    }
    if out.contains("{{") {
        // guard against template drift: raw Jinja must never reach the UI
        println!(" WARNING: render_index оставил необработанные подстановки в index.html");
    }
    out
}

pub async fn index(uri: axum::http::Uri) -> Response {
    let query = query_pairs(&uri);
    let html = render_index();
    let mut resp = file_response(html.into_bytes(), "text/html; charset=utf-8", None);
    // network mode: query key becomes a cookie
    if let Some(key) = query.get("key").cloned() {
        let mut valid = false;
        {
            let secret = crate::config::shared_secret();
            if !secret.is_empty() && !key.is_empty() && key.len() == secret.len()
            {
                valid = key.bytes().zip(secret.bytes()).all(|(a, b)| a == b);
            }
        }
        if valid {
            let cookie = format!(
                "tokenatra_key={key}; Max-Age=43200; HttpOnly; SameSite=Lax; Path=/"
            );
            if let Ok(v) = axum::http::HeaderValue::from_str(&cookie) {
                resp.headers_mut().insert(
                    axum::http::header::SET_COOKIE,
                    v,
                );
            }
        }
    }
    resp
}

pub async fn splash() -> Response {
    let tpl = std::fs::read_to_string(paths::resource_dir().join("templates").join("splash.html"))
        .unwrap_or_else(|_| "<html><body>splash</body></html>".into());
    file_response(tpl.into_bytes(), "text/html; charset=utf-8", None)
}

pub async fn app_icon() -> Response {
    super::send_file_from_disk(&paths::resource_dir().join("icon.ico"), "image/x-icon", None)
}

pub async fn app_logo() -> Response {
    let logo = paths::resource_dir().join("logo.png");
    if logo.exists() {
        super::send_file_from_disk(&logo, "image/png", None)
    } else {
        super::send_file_from_disk(&paths::resource_dir().join("icon.ico"), "image/x-icon", None)
    }
}

pub async fn static_file(Path(path): Path<String>) -> Response {
    // sanitize: no .. or absolute paths
    if path.contains("..") || path.starts_with('/') || path.contains('\\') {
        return super::err_status(StatusCode::NOT_FOUND, "Not found");
    }
    let full = paths::resource_dir().join("static").join(&path);
    match std::fs::read(&full) {
        Ok(bytes) => {
            let ext = super::ext_of(&path);
            // no caching for static: WebView2 must not keep stale JS
            file_response(bytes, mime_by_ext(&ext), Some("no-cache"))
        }
        Err(_) => err_status(StatusCode::NOT_FOUND, "Not found"),
    }
}

pub async fn ring(uri: axum::http::Uri) -> Response {
    let size = query_pairs(&uri)
        .get("size")
        .cloned()
        .unwrap_or_else(|| "1024".into());
    let size = if ["512", "1024", "2048"].contains(&size.as_str()) {
        size
    } else {
        "1024".to_string()
    };
    let ring_dir = paths::builtin_ring_dir();
    let file = ring_dir.join(format!("token{size}.webp"));
    if file.exists() {
        return super::send_file_from_disk(&file, "image/webp", Some("public, max-age=86400"));
    }
    let size_int: u32 = size.parse().unwrap_or(1024);
    let img = crate::imaging::create_default_ring(size_int);
    let mut buf = Vec::new();
    let dyn_img = image::DynamicImage::ImageRgba8(img);
    if image::DynamicImage::write_to(&dyn_img, Cursor::new(&mut buf), image::ImageFormat::Png)
        .is_ok()
    {
        return file_response(buf, "image/png", Some("public, max-age=86400"));
    }
    err_status(StatusCode::INTERNAL_SERVER_ERROR, "ring error")
}

pub async fn mask() -> Response {
    let mask = paths::mask_path();
    if mask.exists()
        && let Ok(bytes) = std::fs::read(&mask)
            && let Ok(img) = image::load_from_memory(&bytes) {
                let rgba = img.to_rgba8();
                let mut buf = Vec::new();
                if image::DynamicImage::write_to(
                    &image::DynamicImage::ImageRgba8(rgba),
                    Cursor::new(&mut buf),
                    image::ImageFormat::Png,
                )
                .is_ok()
                {
                    return file_response(buf, "image/png", Some("public, max-age=86400"));
                }
            }
    let img = image::RgbaImage::new(1024, 1024);
    let mut buf = Vec::new();
    let _ = image::DynamicImage::write_to(
        &image::DynamicImage::ImageRgba8(img),
        Cursor::new(&mut buf),
        image::ImageFormat::Png,
    );
    file_response(buf, "image/png", Some("public, max-age=86400"))
}

pub async fn preset(uri: axum::http::Uri) -> Response {
    let name = query_pairs(&uri)
        .get("name")
        .cloned()
        .unwrap_or_else(|| "preset1".into());
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return err_status(StatusCode::BAD_REQUEST, "Invalid preset name");
    }
    let preset_dir = paths::resource_dir().join("presets");
    for ext in ["png", "webp", "jpg"] {
        let p = preset_dir.join(format!("{name}.{ext}"));
        if p.exists() {
            let mime = match ext {
                "png" => "image/png",
                "webp" => "image/webp",
                _ => "image/jpeg",
            };
            return super::send_file_from_disk(&p, mime, Some("public, max-age=86400"));
        }
    }
    err_status(StatusCode::NOT_FOUND, "Preset not found")
}

pub async fn example() -> Response {
    let base = paths::resource_dir();
    let p = base.join("example.png");
    if p.exists() {
        return super::send_file_from_disk(&p, "image/png", Some("public, max-age=86400"));
    }
    for ext in ["webp", "jpg"] {
        let alt = base.join(format!("example.{ext}"));
        if alt.exists() {
            let mime = if ext == "webp" { "image/webp" } else { "image/jpeg" };
            return super::send_file_from_disk(&alt, mime, Some("public, max-age=86400"));
        }
    }
    err_status(StatusCode::NOT_FOUND, "Example not found")
}

