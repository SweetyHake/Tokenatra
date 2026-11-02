// Agent API (/agent/v1/*).
// Invariant: ops never execute code, read config or secrets, write to
// disk or touch the network; inputs are base64/data_url only.

use std::sync::Mutex;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use image::RgbaImage;
use imageproc::drawing::draw_filled_circle_mut;

use crate::paths;

const LOGICAL: f32 = 1024.0;
const OPS_LIMIT: u32 = 200;
const SESSION_TTL_MIN: u64 = 30;
const MAX_SESSIONS: usize = 4;
const MAX_BASE_DIM: u32 = 2048;

// ---------- sessions ----------

pub struct AgentSession {
    pub id: String,
    pub base: RgbaImage,
    pub mask: image::GrayImage,
    pub scale: f32,
    pub rotation: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub ring: Option<String>,
    pub shadow_enabled: bool,
    pub shadow_angle: f32,
    pub shadow_distance: f32,
    pub shadow_blur: f32,
    pub shadow_opacity: f32,
    pub color_enabled: bool,
    pub color_sat: f32,
    pub color_light: f32,
    pub ops_log: Vec<Value>,
    pub ops_used: u32,
    pub created: Instant,
    pub last_used: Instant,
}

impl AgentSession {
    fn touch(&mut self) {
        self.last_used = Instant::now();
    }
    fn log_op(&mut self, op: &str, params: Value) {
        self.ops_log.push(json!({"op": op, "params": params}));
        self.ops_used += 1;
    }
}

struct Store;
static STORE: Mutex<Option<Vec<AgentSession>>> = Mutex::new(None);

fn with_store<T>(f: impl FnOnce(&mut Vec<AgentSession>) -> T) -> T {
    let mut guard = STORE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(Vec::new());
    }
    let v = guard.as_mut().unwrap();
    // TTL eviction
    let now = Instant::now();
    v.retain(|s| now.duration_since(s.last_used).as_secs() < SESSION_TTL_MIN * 60);
    f(v)
}

fn store_create(base: RgbaImage) -> String {
    with_store(|v| {
        // LRU: keep at most 4, drop oldest by last_used
        while v.len() >= MAX_SESSIONS {
            let oldest = v
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.last_used)
                .map(|(i, _)| i)
                .unwrap();
            v.remove(oldest);
        }
        let id: String = (0..16)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();
        let mask = image::GrayImage::from_pixel(base.width(), base.height(), image::Luma([255u8]));
        v.push(AgentSession {
            id: id.clone(),
            base,
            mask,
            scale: 1.0,
            rotation: 0.0,
            offset_x: 0.0,
            offset_y: 0.0,
            ring: None,
            shadow_enabled: false,
            shadow_angle: 135.0,
            shadow_distance: 7.0,
            shadow_blur: 3.0,
            shadow_opacity: 0.4,
            color_enabled: false,
            color_sat: 0.0,
            color_light: 0.0,
            ops_log: vec![],
            ops_used: 0,
            created: Instant::now(),
            last_used: Instant::now(),
        });
        id
    })
}

fn store_get(sid: &str) -> Option<AgentSession> {
    with_store(|v| {
        let s = v.iter_mut().find(|s| s.id == sid)?;
        s.touch();
        // copy the fields we need
        Some(AgentSession {
            id: s.id.clone(),
            base: s.base.clone(),
            mask: s.mask.clone(),
            scale: s.scale,
            rotation: s.rotation,
            offset_x: s.offset_x,
            offset_y: s.offset_y,
            ring: s.ring.clone(),
            shadow_enabled: s.shadow_enabled,
            shadow_angle: s.shadow_angle,
            shadow_distance: s.shadow_distance,
            shadow_blur: s.shadow_blur,
            shadow_opacity: s.shadow_opacity,
            color_enabled: s.color_enabled,
            color_sat: s.color_sat,
            color_light: s.color_light,
            ops_log: s.ops_log.clone(),
            ops_used: s.ops_used,
            created: s.created,
            last_used: s.last_used,
        })
    })
}

fn store_commit(updated: AgentSession) {
    with_store(|v| {
        if let Some(s) = v.iter_mut().find(|s| s.id == updated.id) {
            s.base = updated.base;
            s.mask = updated.mask;
            s.scale = updated.scale;
            s.rotation = updated.rotation;
            s.offset_x = updated.offset_x;
            s.offset_y = updated.offset_y;
            s.ring = updated.ring;
            s.shadow_enabled = updated.shadow_enabled;
            s.shadow_angle = updated.shadow_angle;
            s.shadow_distance = updated.shadow_distance;
            s.shadow_blur = updated.shadow_blur;
            s.shadow_opacity = updated.shadow_opacity;
            s.color_enabled = updated.color_enabled;
            s.color_sat = updated.color_sat;
            s.color_light = updated.color_light;
            s.ops_log = updated.ops_log;
            s.ops_used = updated.ops_used;
            s.last_used = Instant::now();
        }
    });
}

fn store_remove(sid: &str) {
    with_store(|v| v.retain(|s| s.id != sid));
}

// ---------- errors / responses ----------

fn agent_err(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({"error": code, "code": code, "message": message})),
    )
        .into_response()
}

fn bad(code: &str, message: &str) -> Response {
    agent_err(StatusCode::BAD_REQUEST, code, message)
}

fn ok_json(v: Value) -> Response {
    axum::Json(v).into_response()
}

fn file_response(bytes: Vec<u8>, mime: &str) -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, mime.to_string())],
        bytes,
    )
        .into_response()
}

// ---------- assets ----------

fn find_ring_file(name: &str) -> Option<std::path::PathBuf> {
    let stem = name.to_lowercase();
    let mut dirs = vec![paths::builtin_ring_dir(), paths::ring_dir()];
    for d in dirs.drain(..) {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                let fstem = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_lowercase();
                if fstem == stem {
                    return Some(p);
                }
            }
        }
    }
    // direct suffix lookup: name -> name.webp/png
    for ext in ["webp", "png", "jpg", "jpeg"] {
        for d in [paths::builtin_ring_dir(), paths::ring_dir()] {
            let p = d.join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let _ = stem;
    None
}

fn find_preset_file(name: &str) -> Option<std::path::PathBuf> {
    for d in [paths::user_preset_dir(), paths::builtin_preset_dir()] {
        for ext in ["png"] {
            let p = d.join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn list_dir_images(dir: &std::path::Path, builtin: bool) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut items: Vec<_> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && matches!(
                        super::ext_of(&p.to_string_lossy()).as_str(),
                        "webp" | "png" | "jpg" | "jpeg"
                    )
            })
            .collect();
        items.sort();
        for p in items {
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if stem.to_lowercase().ends_with(".mask") {
                continue;
            }
            out.push(json!({"name": stem, "file": p.file_name().unwrap_or_default().to_string_lossy(), "builtin": builtin}));
        }
    }
    out
}

// ---------- source decoding ----------

fn decode_source(data: &str) -> Result<RgbaImage, Response> {
    let b64 = match data.find("base64,") {
        Some(i) => &data[i + 7..],
        None => data,
    };
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.trim())
        .map_err(|_| bad("bad_request", "image must be valid base64"))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|_| bad("bad_request", "image decode failed"))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let img = if w.max(h) > MAX_BASE_DIM {
        let k = MAX_BASE_DIM as f32 / w.max(h) as f32;
        let nw = (w as f32 * k).round() as u32;
        let nh = (h as f32 * k).round() as u32;
        image::imageops::resize(&img, nw.max(1), nh.max(1), image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    Ok(img)
}

// ---------- masks / geometry ----------

fn mask_apply_shapes(mask: &mut image::GrayImage, shapes: &Value, value: u8) {
    if let Some(arr) = shapes.as_array() {
        for sh in arr {
            if let Some(c) = sh.get("circle") {
                let cx = c["cx"].as_f64().unwrap_or(0.0) as i64;
                let cy = c["cy"].as_f64().unwrap_or(0.0) as i64;
                let r = c["r"].as_f64().unwrap_or(0.0) as i64;
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dy * dy <= r * r {
                            let x = cx + dx;
                            let y = cy + dy;
                            if x >= 0 && y >= 0 {
                                let _ = mask.put_pixel(x as u32, y as u32, image::Luma([value]));
                            }
                        }
                    }
                }
            } else if let Some(rc) = sh.get("rect") {
                let x0 = rc["x0"].as_f64().unwrap_or(0.0).round() as i64;
                let y0 = rc["y0"].as_f64().unwrap_or(0.0).round() as i64;
                let x1 = rc["x1"].as_f64().unwrap_or(0.0).round() as i64;
                let y1 = rc["y1"].as_f64().unwrap_or(0.0).round() as i64;
                for y in y0.min(y1)..y1.max(y0) {
                    for x in x0.min(x1)..x1.max(x0) {
                        if x >= 0 && y >= 0 && (x as u32) < mask.width() && (y as u32) < mask.height() {
                            mask.put_pixel(x as u32, y as u32, image::Luma([value]));
                        }
                    }
                }
            } else if let Some(st) = sh.get("stroke") {
                let width = st["width"].as_f64().unwrap_or(30.0) as i64;
                if let Some(points) = st["points"].as_array() {
                    let pts: Vec<(f64, f64)> = points
                        .iter()
                        .filter_map(|p| {
                            Some((
                                p[0].as_f64().unwrap_or(0.0),
                                p[1].as_f64().unwrap_or(0.0),
                            ))
                        })
                        .collect();
                    for w in pts.windows(2) {
                        let (ax, ay) = w[0];
                        let (bx, by) = w[1];
                        let steps = ((bx - ax).hypot(by - ay) as i64).max(1);
                        for t in 0..=steps {
                            let tt = t as f64 / steps as f64;
                            let cxp = ax + (bx - ax) * tt;
                            let cyp = ay + (by - ay) * tt;
                            let half = width / 2;
                            for dy in -half..=half {
                                for dx in -half..=half {
                                    let x = (cxp as i64) + dx;
                                    let y = (cyp as i64) + dy;
                                    if x >= 0 && y >= 0 && (x as u32) < mask.width() && (y as u32) < mask.height() {
                                        mask.put_pixel(x as u32, y as u32, image::Luma([value]));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------- compose ----------

fn compose(session: &AgentSession, size: u32, scale_mode: &str, with_ring: bool) -> Result<RgbaImage, String> {
    let s3 = size * 3;
    let mut canvas = image::RgbaImage::new(s3, s3);

    if with_ring {
        if let Some(rname) = &session.ring {
            if let Some(rp) = find_ring_file(rname) {
                if let Ok(ring) = image::open(&rp) {
                    let ring = image::imageops::resize(
                        &ring.to_rgba8(),
                        size,
                        size,
                        image::imageops::FilterType::Lanczos3,
                    );
                    let off = ((s3 - size) / 2) as i64;
                    image::imageops::overlay(&mut canvas, &ring, off, off);
                }
            }
        }
    }

    // subject: base -> 1024*s, center + offset, rotation
    let s_f = session.scale;
    let big = (LOGICAL * s_f) as u32;
    let subj = image::imageops::resize(
        &session.base,
        big.max(1),
        big.max(1),
        image::imageops::FilterType::Lanczos3,
    );
    let mut subj = if session.rotation.abs() % 360.0 > f32::EPSILON {
        let theta = session.rotation.to_radians();
        imageproc::geometric_transformations::rotate_about_center_no_crop(
            &subj,
            theta,
            imageproc::geometric_transformations::Interpolation::Bilinear,
            imageproc::geometric_transformations::Border::Constant(image::Rgba([0u8, 0, 0, 0])),
        )
    } else {
        subj
    };
    // subject mask
    let mask_full = resize_mask(&session.mask, subj.width(), subj.height());
    for (p, m) in subj.pixels_mut().zip(mask_full.pixels()) {
        p.0[3] = ((p.0[3] as u32) * (m.0[0] as u32) / 255) as u8;
    }

    // logical position: subject center = 512 + offset; 1 subject px = s3/1024 px
    let scale3 = s3 as f32 / LOGICAL;
    let cx = (512.0 + session.offset_x) * scale3;
    let cy = (512.0 + session.offset_y) * scale3;
    let px0 = (cx - subj.width() as f32 / 2.0).round() as i64;
    let py0 = (cy - subj.height() as f32 / 2.0).round() as i64;

    // color correction
    if session.color_enabled {
        let sat = 1.0 + session.color_sat / 100.0;
        let light = 1.0 + session.color_light / 100.0;
        for p in subj.pixels_mut() {
            let [r, g, b, _] = p.0;
            let l = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
            let mut nr = l + (r as f32 - l) * sat;
            let mut ng = l + (g as f32 - l) * sat;
            let mut nb = l + (b as f32 - l) * sat;
            nr *= light;
            ng *= light;
            nb *= light;
            p.0[0] = nr.clamp(0.0, 255.0) as u8;
            p.0[1] = ng.clamp(0.0, 255.0) as u8;
            p.0[2] = nb.clamp(0.0, 255.0) as u8;
        }
    }

    // shadow
    if session.shadow_enabled {
        let sh = session;
        let rad = sh.shadow_angle.to_radians();
        let dx = (rad.cos() * sh.shadow_distance as f32 * scale3).round() as i64;
        let dy = (-rad.sin() * sh.shadow_distance as f32 * scale3).round() as i64;
        let mut sil = image::GrayImage::new(s3, s3);
        for (dp, sp) in sil.pixels_mut().zip(subj.pixels()) {
            *dp = image::Luma([sp.0[3]]);
        }
        let mut sil_shift = image::GrayImage::new(s3, s3);
        image::imageops::overlay(&mut sil_shift, &sil, dx, dy);
        let sil = image::imageops::blur(&sil_shift, (sh.shadow_blur as f32 * scale3 / 2.0).max(0.1));
        // black layer with shadow alpha, composited before the subject
        let mut shadow_layer = image::RgbaImage::new(s3, s3);
        for (p, sp) in shadow_layer.pixels_mut().zip(sil.pixels()) {
            *p = image::Rgba([0, 0, 0, ((sp.0[0] as f32) * sh.shadow_opacity) as u8]);
        }
        image::imageops::overlay(&mut canvas, &shadow_layer, 0, 0);
    }

    // subject on top
    image::imageops::overlay(&mut canvas, &subj, px0, py0);

    // scale mode
    let used = match scale_mode {
        "1" => 1,
        "2" => 2,
        "3" => 3,
        _ => {
            // auto: subject alpha bbox on a 512 scan, threshold 15, dilate 1
            let scan = 512u32;
            let small = image::imageops::resize(
                &subj,
                scan,
                scan,
                image::imageops::FilterType::Triangle,
            );
            let mut min_x = u32::MAX;
            let mut min_y = u32::MAX;
            let mut max_x = 0u32;
            let mut max_y = 0u32;
            for (x, y, p) in small.enumerate_pixels() {
                if p.0[3] > 15 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
            if max_x == 0 {
                1
            } else {
                let f = s3 as f32 / scan as f32;
                let (mn_x, mn_y) = (
                    (min_x as f32 * f - f as f32).max(0.0),
                    (min_y as f32 * f - f as f32).max(0.0),
                );
                let (mx_x, mx_y) = (
                    ((max_x + 1) as f32 * f + f as f32).min(s3 as f32),
                    ((max_y + 1) as f32 * f + f as f32).min(s3 as f32),
                );
                let mut chosen = 3;
                for cand in 1..=3u32 {
                    let border = (s3 * cand as u32 / 3) as f32;
                    let off = (s3 as f32 - border) / 2.0;
                    if mn_x >= off && mn_y >= off && mx_x <= off + border && mx_y <= off + border {
                        chosen = cand;
                        break;
                    }
                }
                chosen
            }
        }
    };
    let final_size = size * used;
    let crop = ((s3 - final_size) / 2) as u32;
    let out = image::imageops::crop_imm(&canvas, crop, crop, final_size, final_size).to_image();
    Ok(out)
}

fn resize_mask(mask: &image::GrayImage, w: u32, h: u32) -> image::GrayImage {
    image::imageops::resize(mask, w, h, image::imageops::FilterType::Triangle)
}

// ---------- inspect ----------

fn inspect_stats(mask: &image::GrayImage) -> Value {
    let (w, h) = mask.dimensions();
    let mut hist = [0u64; 5];
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut edge_px = 0u64;
    let mut covered = 0u64;
    let total = (w * h) as f32;
    let mut grid = [[0f64; 3]; 3];
    let mut grid_tot = [[0u64; 3]; 3];
    for (x, y, p) in mask.enumerate_pixels() {
        let a = p.0[0] as u64;
        hist[((a / 64) as usize).min(4)] += 1;
        if a > 100 {
            covered += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        if a > 15 && a < 240 {
            edge_px += 1;
        }
        let gx = (x * 3 / w).min(2) as usize;
        let gy = (y * 3 / h).min(2) as usize;
        grid_tot[gy][gx] += 1;
        if a > 100 {
            grid[gy][gx] += 1.0;
        }
    }
    for gy in 0..3 {
        for gx in 0..3 {
            if grid_tot[gy][gx] > 0 {
                grid[gy][gx] = (grid[gy][gx] / grid_tot[gy][gx] as f64 * 1000.0).round() / 10.0;
            }
        }
    }
    let bbox_json = if covered > 0 {
        json!([min_x, min_y, max_x, max_y])
    } else {
        Value::Null
    };
    let mut warnings = Vec::new();
    if min_x == 0 || min_y == 0 || max_x == w - 1 || max_y == h - 1 {
        warnings.push("subject touches canvas border");
    }
    json!({
        "bbox": bbox_json,
        "coverage_pct": round1(covered as f32 / total * 100.0),
        "alpha_hist": hist,
        "edge_px": edge_px,
        "grid_3x3_pct": grid,
        "warnings": warnings,
    })
}

fn round1(v: f32) -> f64 {
    ((v * 10.0).round() / 10.0) as f64
}

// ---------- handlers ----------

pub async fn ping() -> Response {
    ok_json(json!({"pong": true, "agent": true}))
}

pub async fn manifest() -> Response {
    ok_json(json!({
        "api_version": 1,
        "name": "tokenatra",
        "logical_size": 1024,
        "ops": ["create", "remove_bg", "mask", "transform", "ring", "effects", "inspect", "preview", "export", "close"],
        "mask_ops": ["erase", "restore", "apply_preset", "invert", "feather", "grow"],
        "export_formats": ["webp", "png", "jpg"],
        "limits": {
            "export_size_range": [256, 2048],
            "max_base_dim": MAX_BASE_DIM,
            "max_body_bytes": 15_728_640,
            "max_ops_per_session": OPS_LIMIT,
            "max_sessions": MAX_SESSIONS,
            "session_ttl_min": SESSION_TTL_MIN,
        },
        "defaults": {
            "shadow": {"enabled": false, "angle": 135.0, "distance": 7.0, "blur": 3.0, "opacity": 0.4},
            "color": {"enabled": false, "lightness": 0, "saturation": 0}
        },
        "required_header": {"X-Tokenatra-Agent": "1"}
    }))
}

pub async fn assets() -> Response {
    let mut rings = list_dir_images(&paths::builtin_ring_dir(), true);
    rings.extend(list_dir_images(&paths::ring_dir(), false));
    let mut presets = list_dir_images(&paths::builtin_preset_dir(), true);
    presets.extend(list_dir_images(&paths::user_preset_dir(), false));
    ok_json(json!({"rings": rings, "presets": presets, "logical_size": 1024}))
}

pub async fn tools_schema() -> Response {
    ok_json(json!({
        "tools": [
            {"type": "function", "function": {"name": "tokenatra_create_session", "description": "Create an editing session from an image", "parameters": {"type": "object", "properties": {"image_b64": {"type": "string", "description": "base64 or data:image/...;base64"}}, "required": ["image_b64"]}}},
            {"type": "function", "function": {"name": "tokenatra_remove_background", "description": "Remove background (ONNX)", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "edge_blur": {"type": "number"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_edit_mask", "description": "Edit visibility mask (1024x1024 logical). ops: erase/restore/apply_preset/invert/feather/grow", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "op": {"type": "string", "enum": ["erase", "restore", "apply_preset", "invert", "feather", "grow"]}, "shapes_json": {"type": "string"}, "preset": {"type": "string"}, "px": {"type": "integer"}, "protect": {"type": "boolean"}}, "required": ["session_id", "op"]}}},
            {"type": "function", "function": {"name": "tokenatra_set_transform", "description": "Set subject transform: scale relative to fit-in-canvas (1.0 = fitted), rotation deg (cw), offset in logical px", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "scale": {"type": "number"}, "rotation": {"type": "number"}, "offset_x": {"type": "number"}, "offset_y": {"type": "number"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_set_ring", "description": "Set token ring by name or remove (null)", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "name": {"type": ["string", "null"]}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_set_effects", "description": "Shadow and color correction", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "shadow_json": {"type": "string"}, "color_json": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_inspect", "description": "Session state stats", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "include_image": {"type": "boolean"}, "preview_size": {"type": "integer"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_preview", "description": "Full-canvas preview (webp)", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "size": {"type": "integer"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_export", "description": "Export final image", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "format": {"type": "string", "enum": ["webp", "png", "jpg"]}, "quality": {"type": "integer"}, "size": {"type": "integer"}, "scale_mode": {"type": "string", "enum": ["auto", "1", "2", "3"]}, "with_ring": {"type": "boolean"}, "save_to": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_close_session", "description": "Close session and free memory", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_list_assets", "description": "List rings and presets", "parameters": {"type": "object", "properties": {}}}}
        ]
    }))
}

pub async fn create_session(
    State(_state): State<std::sync::Arc<crate::state::AppState>>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let Some(img) = v["image"].as_str() else {
        return bad("bad_request", "image must be a base64 or data URL string");
    };
    if img.len() > 15_728_640 * 4 / 3 {
        return agent_err(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", "image body exceeds 15 MB");
    }
    match decode_source(img) {
        Ok(base) => {
            let id = store_create(base);
            with_store(|v| {
                if let Some(s) = v.iter_mut().find(|s| s.id == id) {
                    s.log_op("create", json!({}));
                }
            });
            ok_json(json!({"session_id": id}))
        }
        Err(r) => r,
    }
}

fn session_op_guard(sid: &str) -> Result<AgentSession, Response> {
    let s = store_get(sid).ok_or_else(|| {
        agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session")
    })?;
    if s.ops_used >= OPS_LIMIT {
        return Err(agent_err(
            StatusCode::TOO_MANY_REQUESTS,
            "ops_limit",
            "operation limit reached for this session",
        ));
    }
    Ok(s)
}

pub async fn get_session(
    State(_): State<std::sync::Arc<crate::state::AppState>>,
    Path(sid): Path<String>,
) -> Response {
    match store_get(&sid) {
        Some(s) => ok_json(json!({
            "session_id": s.id,
            "image": {"width": s.base.width(), "height": s.base.height()},
            "transform": {"scale": s.scale, "rotation": s.rotation, "offset_x": s.offset_x, "offset_y": s.offset_y},
            "ring": s.ring,
            "effects": {
                "shadow": {"enabled": s.shadow_enabled, "angle": s.shadow_angle, "distance": s.shadow_distance, "blur": s.shadow_blur, "opacity": s.shadow_opacity},
                "color": {"enabled": s.color_enabled, "saturation": s.color_sat, "lightness": s.color_light}
            },
            "ops_used": s.ops_used,
            "ops_limit": OPS_LIMIT,
            "recipe": s.ops_log,
        })),
        None => agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session"),
    }
}

pub async fn close_session(Path(sid): Path<String>) -> Response {
    store_remove(&sid);
    ok_json(json!({"closed": true}))
}

pub async fn op_remove_bg(
    State(state): State<std::sync::Arc<crate::state::AppState>>,
    Path(sid): Path<String>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let edge_blur = body
        .and_then(|axum::Json(v)| v["edge_blur"].as_f64())
        .unwrap_or(1.0);
    let mut s = match session_op_guard(&sid) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let dyn_img = image::DynamicImage::ImageRgba8(s.base.clone());
    match crate::infer::remove_background(&state, dyn_img, edge_blur) {
        Ok(rgba) => {
            // cutout alpha -> mask (subject = model alpha)
            let mask = image::imageops::resize(
                &image::imageops::grayscale(&rgba),
                s.base.width(),
                s.base.height(),
                image::imageops::FilterType::Lanczos3,
            );
            s.mask = mask;
            s.log_op("remove_bg", json!({"edge_blur": edge_blur}));
            let stats = inspect_stats(&s.mask);
            store_commit(s);
            ok_json(json!({"ok": true, "bg_removed": true, "mask": stats}))
        }
        Err(e) => agent_err(StatusCode::INTERNAL_SERVER_ERROR, "infer_error", &e.to_string()),
    }
}

pub async fn op_mask(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let mut s = match session_op_guard(&sid) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let op = v["op"].as_str().unwrap_or_default().to_string();
    let px = v["px"].as_u64().unwrap_or(0) as u32;
    let params = v.clone();
    match op.as_str() {
        "erase" | "restore" => {
            let value = if op == "erase" { 0u8 } else { 255u8 };
            match v["shapes"].as_array() {
                Some(_) => mask_apply_shapes(&mut s.mask, &v["shapes"], value),
                None => return bad("bad_request", "shapes required for erase/restore"),
            }
        }
        "invert" => {
            for p in s.mask.pixels_mut() {
                p.0[0] = 255 - p.0[0];
            }
        }
        "feather" => {
            if px == 0 {
                return bad("bad_request", "px required for feather");
            }
            let blurred = image::imageops::blur(&s.mask, px as f32);
            s.mask = blurred;
        }
        "grow" => {
            if px == 0 {
                return bad("bad_request", "px required for grow");
            }
            use imageproc::distance_transform::Norm;
            s.mask = imageproc::morphology::dilate(
                &s.mask,
                imageproc::distance_transform::Norm::LInf,
                px.min(255) as u8,
            );
        }
        "apply_preset" => {
            let pname = v["preset"].as_str().unwrap_or_default();
            let Some(pp) = find_preset_file(pname) else {
                return agent_err(StatusCode::NOT_FOUND, "unknown_preset", "preset not found");
            };
            let Ok(preset) = image::open(&pp) else {
                return bad("bad_request", "preset decode failed");
            };
            let preset = preset.to_rgba8();
            // preset alpha stretches over the canvas and replaces the mask
            let a = resize_mask(&image::imageops::grayscale(&preset), s.mask.width(), s.mask.height());
            s.mask = a;
        }
        other => return bad("bad_mask_op", &format!("unknown mask op {other:?}")),
    }
    s.log_op(&format!("mask.{op}"), params);
    let stats = inspect_stats(&s.mask);
    store_commit(s);
    ok_json(json!({"ok": true, "mask": stats}))
}

pub async fn op_transform(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let mut s = match session_op_guard(&sid) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let params = v.clone();
    if let Some(x) = v["scale"].as_f64() {
        s.scale = x as f32;
    }
    if let Some(x) = v["rotation"].as_f64() {
        s.rotation = x as f32;
    }
    if let Some(x) = v["offset_x"].as_f64() {
        s.offset_x = x as f32;
    }
    if let Some(x) = v["offset_y"].as_f64() {
        s.offset_y = x as f32;
    }
    if s.scale <= 0.0 || s.scale > 8.0 {
        return bad("bad_request", "scale out of range");
    }
    s.log_op("transform", params);
    store_commit(s);
    ok_json(json!({"ok": true}))
}

pub async fn op_ring(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let mut s = match session_op_guard(&sid) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let name = v["name"].as_str().map(|x| x.to_string());
    if let Some(n) = &name {
        if find_ring_file(n).is_none() {
            return agent_err(StatusCode::NOT_FOUND, "unknown_asset", "ring not found");
        }
    }
    s.ring = name.clone();
    s.log_op("ring", json!({"name": name}));
    store_commit(s);
    ok_json(json!({"ok": true}))
}

pub async fn op_effects(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let mut s = match session_op_guard(&sid) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let params = v.clone();
    if let Some(sh) = v["shadow"].as_object() {
        s.shadow_enabled = sh["enabled"].as_bool().unwrap_or(false);
        s.shadow_angle = sh["angle"].as_f64().unwrap_or(135.0) as f32;
        s.shadow_distance = sh["distance"].as_f64().unwrap_or(7.0) as f32;
        s.shadow_blur = sh["blur"].as_f64().unwrap_or(3.0) as f32;
        s.shadow_opacity = sh["opacity"].as_f64().unwrap_or(0.4) as f32;
    }
    if let Some(cl) = v["color"].as_object() {
        s.color_enabled = cl["enabled"].as_bool().unwrap_or(false);
        s.color_sat = cl["saturation"].as_f64().unwrap_or(0.0) as f32;
        s.color_light = cl["lightness"].as_f64().unwrap_or(0.0) as f32;
    }
    s.log_op("effects", params);
    store_commit(s);
    ok_json(json!({"ok": true}))
}

pub async fn op_preview(
    Path(sid): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(s) = store_get(&sid) else {
        return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
    };
    let size: u32 = q
        .get("size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(384)
        .clamp(64, 768);
    match compose(&s, size, "auto", true) {
        Ok(img) => match crate::imaging::save_image(
            &image::DynamicImage::ImageRgba8(img),
            "webp",
            90,
        ) {
            Ok((bytes, _)) => file_response(bytes, "image/webp"),
            Err(e) => agent_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "encode_error",
                &e.to_string(),
            ),
        },
        Err(e) => agent_err(StatusCode::INTERNAL_SERVER_ERROR, "compose_error", &e),
    }
}

pub async fn op_inspect(
    Path(sid): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(s) = store_get(&sid) else {
        return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
    };
    let mut v = json!({
        "session_id": s.id,
        "image": {"width": s.base.width(), "height": s.base.height()},
        "transform": {"scale": s.scale, "rotation": s.rotation, "offset_x": s.offset_x, "offset_y": s.offset_y},
        "ring": s.ring,
        "effects": {
            "shadow": {"enabled": s.shadow_enabled, "angle": s.shadow_angle, "distance": s.shadow_distance, "blur": s.shadow_blur, "opacity": s.shadow_opacity},
            "color": {"enabled": s.color_enabled, "saturation": s.color_sat, "lightness": s.color_light}
        },
        "ops_used": s.ops_used,
        "ops_limit": OPS_LIMIT,
        "recipe": s.ops_log,
    });
    let stats = inspect_stats(&s.mask);
    v["mask"] = stats;
    if q.get("include_image").map(|v| v == "1" || v == "true").unwrap_or(false) {
        let size: u32 = q
            .get("preview_size")
            .and_then(|v| v.parse().ok())
            .unwrap_or(384)
            .clamp(64, 768);
        if let Ok(img) = compose(&s, size, "auto", true) {
            if let Ok((bytes, _)) = crate::imaging::save_image(
                &image::DynamicImage::ImageRgba8(img),
                "webp",
                90,
            ) {
                v["image"] = json!(format!(
                    "data:image/webp;base64,{}",
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
                ));
            }
        }
    }
    ok_json(v)
}

pub async fn op_export(
    State(_state): State<std::sync::Arc<crate::state::AppState>>,
    Path(sid): Path<String>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let Some(s) = store_get(&sid) else {
        return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
    };
    let format = v["format"].as_str().unwrap_or("png").to_string();
    if !["webp", "png", "jpg"].contains(&format.as_str()) {
        return bad("bad_request", "format must be webp|png|jpg");
    }
    let quality = v["quality"].as_u64().unwrap_or(95).clamp(10, 100) as u8;
    let size = v["size"].as_u64().unwrap_or(1024).clamp(256, 2048) as u32;
    let scale_mode = v["scale_mode"].as_str().unwrap_or("auto").to_string();
    let with_ring = v["with_ring"].as_bool().unwrap_or(true);

    match compose(&s, size, &scale_mode, with_ring) {
        Ok(img) => match crate::imaging::save_image(
            &image::DynamicImage::ImageRgba8(img),
            &format,
            quality,
        ) {
            Ok((bytes, mime)) => file_response(bytes, mime),
            Err(e) => agent_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "encode_error",
                &e.to_string(),
            ),
        },
        Err(e) => agent_err(StatusCode::INTERNAL_SERVER_ERROR, "compose_error", &e),
    }
}
