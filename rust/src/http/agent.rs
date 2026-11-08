// Agent API (/agent/v1/*).
// Provides headless token & portrait studio parity with the human GUI
// (TokenCanvas, TokenEditor, TokenPresets, PortraitGenerator, Remover, Converter).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use image::{GrayImage, Luma, Rgba, RgbaImage};

use crate::consts::*;
use crate::paths;
use crate::state::AppState;

const LOGICAL: f32 = 1024.0;
const LOGICAL_U32: u32 = 1024;
const OPS_LIMIT: u32 = 200;
const SESSION_TTL_MIN: u64 = 30;
const MAX_SESSIONS: usize = 4;
const MAX_BASE_DIM: u32 = 2048;
const MAX_HISTORY: usize = 30;

// ---------- sessions ----------

#[derive(Clone)]
pub struct HistoryEntry {
    pub canvas_mask: GrayImage,
    pub image_mask: GrayImage,
    pub scale: f32,
    pub rotation: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub showing_original: bool,
}

pub struct AgentSession {
    pub id: String,
    pub original: RgbaImage,
    pub cutout: Option<RgbaImage>,
    pub bg_removed: bool,
    pub showing_original: bool,
    /// Pink mask (logical 1024x1024 canvas space, fixed on the ring/canvas).
    pub canvas_mask: GrayImage,
    /// Blue eraser mask (source image space, moves/scales/rotates with subject).
    pub image_mask: GrayImage,
    pub protection_enabled: bool,
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
    pub color_contrast: f32,
    /// Portrait generator state (2048x2048 logical frame, just like PortraitGenerator in UI).
    pub portrait_scale: f32,
    pub portrait_rotation: f32,
    pub portrait_x: f32,
    pub portrait_y: f32,
    pub portrait_variant: String, // "original" | "cutout"
    pub history: Vec<HistoryEntry>,
    pub history_index: usize,
    pub last_inspect_coverage: Option<f64>,
    pub ops_log: Vec<Value>,
    pub ops_used: u32,
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

    fn active_base(&self) -> &RgbaImage {
        if !self.showing_original
            && let Some(ref c) = self.cutout {
                return c;
            }
        &self.original
    }

    fn snapshot(&self) -> HistoryEntry {
        HistoryEntry {
            canvas_mask: self.canvas_mask.clone(),
            image_mask: self.image_mask.clone(),
            scale: self.scale,
            rotation: self.rotation,
            offset_x: self.offset_x,
            offset_y: self.offset_y,
            showing_original: self.showing_original,
        }
    }

    fn push_history(&mut self) {
        if self.history_index + 1 < self.history.len() {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(self.snapshot());
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
        self.history_index = self.history.len().saturating_sub(1);
    }

    fn apply_history(&mut self, entry: HistoryEntry) {
        self.canvas_mask = entry.canvas_mask;
        self.image_mask = entry.image_mask;
        self.scale = entry.scale;
        self.rotation = entry.rotation;
        self.offset_x = entry.offset_x;
        self.offset_y = entry.offset_y;
        self.showing_original = entry.showing_original;
    }
}

struct SessionSlot {
    id: String,
    session: Arc<Mutex<AgentSession>>,
}

static STORE: Mutex<Option<Vec<SessionSlot>>> = Mutex::new(None);

fn with_store<T>(f: impl FnOnce(&mut Vec<SessionSlot>) -> T) -> T {
    let mut guard = STORE.lock().unwrap();
    let v = guard.get_or_insert_with(Vec::new);
    let now = Instant::now();
    v.retain(|slot| {
        slot.session
            .lock()
            .map(|s| now.duration_since(s.last_used).as_secs() < SESSION_TTL_MIN * 60)
            .unwrap_or(false)
    });
    f(v)
}

fn default_ring_name() -> Option<String> {
    if find_ring_file("steel").is_some() {
        return Some("steel".to_string());
    }
    let rings = list_dir_images(&paths::builtin_ring_dir(), true);
    rings
        .first()
        .and_then(|r| r["name"].as_str())
        .map(|s| s.to_string())
}

fn store_create(base: RgbaImage, ring_override: Option<Option<String>>) -> Arc<Mutex<AgentSession>> {
    with_store(|v| {
        while v.len() >= MAX_SESSIONS {
            let oldest = v
                .iter()
                .enumerate()
                .min_by_key(|(_, slot)| {
                    slot.session
                        .lock()
                        .map(|s| s.last_used)
                        .unwrap_or_else(|_| Instant::now())
                })
                .map(|(i, _)| i)
                .unwrap();
            v.remove(oldest);
        }
        let id: String = (0..16)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();
        let canvas_mask = GrayImage::from_pixel(LOGICAL_U32 * 3, LOGICAL_U32 * 3, Luma([255u8]));
        let image_mask = GrayImage::from_pixel(base.width(), base.height(), Luma([255u8]));
        let ring = match ring_override {
            Some(r) => r,
            None => default_ring_name(),
        };
        let mut sess = AgentSession {
            id: id.clone(),
            original: base,
            cutout: None,
            bg_removed: false,
            showing_original: false,
            canvas_mask,
            image_mask,
            protection_enabled: true,
            scale: 1.0,
            rotation: 0.0,
            offset_x: 0.0,
            offset_y: 0.0,
            ring,
            shadow_enabled: false,
            shadow_angle: 135.0,
            shadow_distance: 7.0,
            shadow_blur: 3.0,
            shadow_opacity: 0.4,
            color_enabled: false,
            color_sat: 0.0,
            color_light: 0.0,
            color_contrast: 0.0,
            portrait_scale: 1.0,
            portrait_rotation: 0.0,
            portrait_x: 0.0,
            portrait_y: 0.0,
            portrait_variant: "original".to_string(),
            history: Vec::new(),
            history_index: 0,
            last_inspect_coverage: None,
            ops_log: vec![],
            ops_used: 0,
            last_used: Instant::now(),
        };
        sess.history.push(sess.snapshot());
        sess.log_op(
            "create",
            json!({"width": sess.original.width(), "height": sess.original.height()}),
        );
        let arc = Arc::new(Mutex::new(sess));
        v.push(SessionSlot {
            id,
            session: arc.clone(),
        });
        arc
    })
}

fn store_get(sid: &str) -> Option<Arc<Mutex<AgentSession>>> {
    with_store(|v| {
        let slot = v.iter().find(|s| s.id == sid)?;
        if let Ok(mut s) = slot.session.lock() {
            s.touch();
        }
        Some(slot.session.clone())
    })
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
    let stem = name.trim().to_lowercase();
    if stem.is_empty() {
        return None;
    }
    for d in [paths::ring_dir(), paths::builtin_ring_dir()] {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                let fname = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_lowercase();
                let fstem = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_lowercase();
                if fstem.ends_with(".mask") {
                    continue;
                }
                let localized = builtin_ring_name(&fname).map(|s| s.to_lowercase());
                if fstem == stem || fname == stem || localized.as_deref() == Some(stem.as_str()) {
                    return Some(p);
                }
            }
        }
    }
    for ext in ["webp", "png", "jpg", "jpeg"] {
        for d in [paths::ring_dir(), paths::builtin_ring_dir()] {
            let p = d.join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn find_protection_mask_file(ring_name: Option<&str>) -> Option<std::path::PathBuf> {
    if let Some(rname) = ring_name
        && let Some(rp) = find_ring_file(rname) {
            let stem = rp
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let orig_ext = super::ext_of(&rp.to_string_lossy());
            for ext in [&orig_ext[..], "png", "webp", "jpg", "jpeg"] {
                let mp = rp.with_file_name(format!("{stem}.mask.{ext}"));
                if mp.is_file() {
                    return Some(mp);
                }
            }
        }
    let default_mask = paths::mask_path();
    if default_mask.is_file() {
        Some(default_mask)
    } else {
        None
    }
}

/// Builds a 1024x1024 protection mask in 1x ring space where 255 = protected (cannot be erased).
fn load_protection_mask_1024(ring_name: Option<&str>) -> Option<GrayImage> {
    let path = find_protection_mask_file(ring_name)?;
    let bytes = std::fs::read(&path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let resized = image::imageops::resize(
        &img,
        LOGICAL_U32,
        LOGICAL_U32,
        image::imageops::FilterType::Lanczos3,
    );
    let mut prot = GrayImage::new(LOGICAL_U32, LOGICAL_U32);
    for (x, y, p) in resized.enumerate_pixels() {
        let protected = if p.0[3] > 0 { 255u8 } else { 0u8 };
        prot.put_pixel(x, y, Luma([protected]));
    }
    Some(prot)
}

fn find_preset_file(name: &str) -> Option<std::path::PathBuf> {
    let target = name.trim().to_lowercase();
    for d in [paths::user_preset_dir(), paths::builtin_preset_dir()] {
        for ext in ["png", "webp", "jpg", "jpeg"] {
            let p = d.join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                let stem = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_lowercase();
                let fname = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_lowercase();
                if stem == target || fname == target {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Processes a preset image with the exact UI semantics (`TokenPresets.processMaskImage`):
/// In preset PNGs, dark opaque pixels (`a > 16 && brightness < 220`) mark the CUTOUT region
/// (removed -> mask = 0), and transparent/white pixels mark the KEPT region (mask = 255).
/// Builtin presets (`preset1.png` etc.) are 6144x6144 (3x canvas) and stretch 1:1 onto the
/// 1024x1024 3x logical canvas mask, just like in the UI!
fn load_preset_mask(path: &std::path::Path, target_w: u32, target_h: u32) -> Result<GrayImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let resized = image::imageops::resize(
        &img,
        target_w,
        target_h,
        image::imageops::FilterType::Lanczos3,
    );
    let mut out = GrayImage::new(target_w, target_h);
    for (x, y, p) in resized.enumerate_pixels() {
        let [r, g, b, a] = p.0;
        let brightness = (r as u32 + g as u32 + b as u32) / 3;
        let is_cutout = a > 16 && brightness < 220;
        out.put_pixel(x, y, Luma([if is_cutout { 0u8 } else { 255u8 }]));
    }
    Ok(out)
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
            let fname = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let display_name = builtin_ring_name(&fname.to_lowercase()).unwrap_or(&stem);
            out.push(json!({
                "name": stem,
                "display_name": display_name,
                "file": fname,
                "builtin": builtin
            }));
        }
    }
    out
}

// ---------- source decoding ----------

// Err is a full axum Response (heavy) on purpose: handlers reply with JSON
// error bodies. These are one-shot request paths, not hot loops, so the
// "large Err variant" lint is noise here.
#[allow(clippy::result_large_err)]
fn decode_bytes(bytes: &[u8]) -> Result<RgbaImage, Response> {
    let img = crate::imaging::open_image(bytes)
        .map_err(|e| bad("bad_image", &e.to_string()))?
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

#[allow(clippy::result_large_err)]
fn decode_source(v: &Value) -> Result<RgbaImage, Response> {
    if let Some(p) = v["image_path"].as_str().or_else(|| v["path"].as_str()) {
        let p = p.trim();
        if !p.is_empty() {
            let bytes = std::fs::read(p)
                .map_err(|e| bad("file_not_found", &format!("cannot read {p}: {e}")))?;
            return decode_bytes(&bytes);
        }
    }
    let Some(data) = v["image"].as_str().or_else(|| v["image_b64"].as_str()) else {
        return Err(bad(
            "bad_request",
            "provide 'image_path' (local file path) or 'image' (base64 / data URL)",
        ));
    };
    if data.len() > 15_728_640 * 4 / 3 {
        return Err(agent_err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "image body exceeds 15 MB",
        ));
    }
    let b64 = match data.find("base64,") {
        Some(i) => &data[i + 7..],
        None => data,
    };
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.trim())
        .map_err(|_| bad("bad_request", "image must be valid base64"))?;
    decode_bytes(&bytes)
}

// ---------- masks / geometry ----------

/// Stamp a soft radial brush (identical to TokenCanvas.updateEraserBrush:
/// 1.0 up to 0.7*r, 0.5 at 0.85*r, 0.0 at 1.0*r) onto `mask` at `(cx, cy)`.
fn stamp_radial_brush(
    mask: &mut GrayImage,
    cx: f32,
    cy: f32,
    radius: f32,
    erase: bool,
    hardness: f32,
) {
    if radius <= 0.0 {
        return;
    }
    let (mw, mh) = mask.dimensions();
    let r_ceil = radius.ceil() as i64 + 1;
    let icx = cx.round() as i64;
    let icy = cy.round() as i64;
    let inner = hardness.clamp(0.0, 0.99);
    for dy in -r_ceil..=r_ceil {
        let y = icy + dy;
        if y < 0 || y >= mh as i64 {
            continue;
        }
        for dx in -r_ceil..=r_ceil {
            let x = icx + dx;
            if x < 0 || x >= mw as i64 {
                continue;
            }
            let dist = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if dist > radius {
                continue;
            }
            let t = dist / radius;
            let strength = if t <= inner {
                1.0
            } else {
                ((1.0 - t) / (1.0 - inner)).clamp(0.0, 1.0)
            };
            let cur = mask.get_pixel(x as u32, y as u32).0[0] as f32;
            let next = if erase {
                cur * (1.0 - strength)
            } else {
                cur + (255.0 - cur) * strength
            };
            mask.put_pixel(x as u32, y as u32, Luma([next.round().clamp(0.0, 255.0) as u8]));
        }
    }
}

/// Applies shapes (`circle`, `rect`, `stroke`) to `mask`.
/// If `full_s3` is true, `mask` is `3*LOGICAL x 3*LOGICAL` (3072x3072) and 1x logical
/// coordinates `(0..1024)` map to `(1024..2048)` so that shapes can also reach `(-1024..2048)`!
/// If `full_s3` is false, `mask` is `image_mask` (`mw x mh`) and `(0..1024)` maps across `0..mw, 0..mh`.
fn mask_apply_shapes(mask: &mut GrayImage, shapes: &Value, erase: bool, full_s3: bool) {
    let (mw, mh) = mask.dimensions();
    let (sx, sy, ox, oy) = if full_s3 {
        let s = (mw as f64) / (LOGICAL as f64 * 3.0);
        (s, s, LOGICAL as f64 * s, LOGICAL as f64 * s)
    } else {
        (mw as f64 / LOGICAL as f64, mh as f64 / LOGICAL as f64, 0.0, 0.0)
    };
    let sr = (sx + sy) * 0.5;
    let value = if erase { 0u8 } else { 255u8 };

    if let Some(arr) = shapes.as_array() {
        for sh in arr {
            if let Some(c) = sh.get("circle") {
                let cx = (c["cx"].as_f64().unwrap_or(0.0) * sx + ox).round() as i64;
                let cy = (c["cy"].as_f64().unwrap_or(0.0) * sy + oy).round() as i64;
                let r = (c["r"].as_f64().unwrap_or(0.0) * sr).round() as i64;
                let feather = c["feather"].as_f64().unwrap_or(0.0) * sr;
                if feather > 0.5 {
                    let hard = ((r as f64 - feather).max(0.0) / (r as f64).max(1.0)) as f32;
                    stamp_radial_brush(mask, cx as f32, cy as f32, r as f32, erase, hard);
                } else {
                    for dy in -r..=r {
                        for dx in -r..=r {
                            if dx * dx + dy * dy <= r * r {
                                let x = cx + dx;
                                let y = cy + dy;
                                if x >= 0 && y >= 0 && (x as u32) < mw && (y as u32) < mh {
                                    mask.put_pixel(x as u32, y as u32, Luma([value]));
                                }
                            }
                        }
                    }
                }
            } else if let Some(rc) = sh.get("rect") {
                let x0 = (rc["x0"].as_f64().unwrap_or(0.0) * sx + ox).round() as i64;
                let y0 = (rc["y0"].as_f64().unwrap_or(0.0) * sy + oy).round() as i64;
                let x1 = (rc["x1"].as_f64().unwrap_or(0.0) * sx + ox).round() as i64;
                let y1 = (rc["y1"].as_f64().unwrap_or(0.0) * sy + oy).round() as i64;
                for y in y0.min(y1)..y1.max(y0) {
                    for x in x0.min(x1)..x1.max(x0) {
                        if x >= 0 && y >= 0 && (x as u32) < mw && (y as u32) < mh {
                            mask.put_pixel(x as u32, y as u32, Luma([value]));
                        }
                    }
                }
            } else if let Some(st) = sh.get("stroke") {
                let width = (st["width"].as_f64().unwrap_or(30.0) * sr).max(1.0) as f32;
                let radius = width * 0.5;
                let soft = st["soft"].as_bool().unwrap_or(true);
                let hardness = if soft { 0.7 } else { 0.99 };
                if let Some(points) = st["points"].as_array() {
                    let pts: Vec<(f32, f32)> = points
                        .iter()
                        .filter_map(|p| {
                            Some((
                                (p[0].as_f64()? * sx + ox) as f32,
                                (p[1].as_f64()? * sy + oy) as f32,
                            ))
                        })
                        .collect();
                    if pts.len() == 1 {
                        stamp_radial_brush(mask, pts[0].0, pts[0].1, radius, erase, hardness);
                    }
                    let step_len = (radius * 0.35).max(1.0);
                    for w in pts.windows(2) {
                        let (ax, ay) = w[0];
                        let (bx, by) = w[1];
                        let dist = (bx - ax).hypot(by - ay);
                        let steps = (dist / step_len).ceil().max(1.0) as i64;
                        for t in 0..=steps {
                            let tt = t as f32 / steps as f32;
                            let cxp = ax + (bx - ax) * tt;
                            let cyp = ay + (by - ay) * tt;
                            stamp_radial_brush(mask, cxp, cyp, radius, erase, hardness);
                        }
                    }
                }
            }
        }
    }
}

// ---------- compose ----------

/// Color-corrects the active base image and applies the blue `image_mask` (in source coords).
fn build_composited_subject(session: &AgentSession) -> RgbaImage {
    let mut img = session.active_base().clone();
    if session.color_enabled {
        let sat = 1.0 + session.color_sat / 100.0;
        let light = 1.0 + session.color_light / 100.0;
        let con = 1.0 + session.color_contrast / 100.0;
        for p in img.pixels_mut() {
            let [r, g, b, _] = p.0;
            let l = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
            let mut nr = l + (r as f32 - l) * sat;
            let mut ng = l + (g as f32 - l) * sat;
            let mut nb = l + (b as f32 - l) * sat;
            nr *= light;
            ng *= light;
            nb *= light;
            nr = (nr - 128.0) * con + 128.0;
            ng = (ng - 128.0) * con + 128.0;
            nb = (nb - 128.0) * con + 128.0;
            p.0[0] = nr.clamp(0.0, 255.0) as u8;
            p.0[1] = ng.clamp(0.0, 255.0) as u8;
            p.0[2] = nb.clamp(0.0, 255.0) as u8;
        }
    }
    let (iw, ih) = img.dimensions();
    let img_mask = if session.image_mask.dimensions() == (iw, ih) {
        session.image_mask.clone()
    } else {
        resize_mask(&session.image_mask, iw, ih)
    };
    for (p, m) in img.pixels_mut().zip(img_mask.pixels()) {
        p.0[3] = ((p.0[3] as u32) * (m.0[0] as u32) / 255) as u8;
    }
    img
}

/// Renders the subject layer onto the `s3 x s3` canvas (where `s3 = size * 3` and the 1x ring
/// occupies the central `[size .. 2*size]` box).
///
/// Crucially:
/// - Aspect ratio of non-square images is preserved (`k_fit = 1024 / max(w, h)`).
/// - `scale` is relative to fitting inside the 1x ring frame (`size`), or in legacy 3x coordinates.
///   Wait: let's make sure BOTH `offset` in 1024 export frame AND legacy `offset` work seamlessly!
///   Wait: how did `compose` work before?
///   In `agent.rs` previously:
///   `let big = (LOGICAL * s_f) as u32;` (so at `size = 1024`, `s3 = 3072`, `subj` had size `1024 * S`,
///   which is `size * S`, i.e., `S = 1.0` fits the 1x ring frame of 1024x1024!).
///   And for position:
///   `let scale3 = s3 as f32 / LOGICAL;` (`scale3 = 3.0` at `size = 1024`)
///   `let cx = (512.0 + session.offset_x) * scale3;`
///   Wait: why did `agent.rs` have `scale3 = s3 / LOGICAL` for `cx` while `subj` was `size * S`?
///   Because in `agent_pipeline.py` (the original Python version):
///   `k = k_fit * t['scale'] * (s3 / LOGICAL)` AND `cx = s3 / 2 + t['offset_x'] * (s3 / LOGICAL)`!
///   In Python, BOTH `subj` and `offset` were multiplied by `s3 / LOGICAL` (so `S=1.0` fit the 3x canvas),
///   whereas in Rust `agent.rs`, the author changed `subj` to `size * S` (so `S=1.0` fits the 1x ring frame!),
///   and left `cx = (512 + offset_x) * scale3`!
///   AND notice: in `agent.rs`, when `size = 384` (for preview), `big` was still `1024 * S` instead of `size * S`!
///   That was a bug in `agent.rs` line 398 (`let big = (LOGICAL * s_f) as u32;` instead of `(size as f32 * s_f)`),
///   which made `preview(384)` draw the subject 2.66x too huge relative to the 384 ring!
fn render_subject_on_s3(session: &AgentSession, size: u32) -> RgbaImage {
    let s3 = size * 3;
    let base = build_composited_subject(session);
    let (bw, bh) = base.dimensions();
    let max_dim = bw.max(bh).max(1) as f32;

    // Fit into 1x frame (`size` px) when scale == 1.0, preserving aspect ratio!
    let fit_scale = (size as f32 / max_dim) * session.scale;
    let target_w = (bw as f32 * fit_scale).round().max(1.0) as u32;
    let target_h = (bh as f32 * fit_scale).round().max(1.0) as u32;

    let subj_resized = image::imageops::resize(
        &base,
        target_w,
        target_h,
        image::imageops::FilterType::Lanczos3,
    );

    let subj_rotated = if session.rotation.abs() % 360.0 > f32::EPSILON {
        let theta = session.rotation.to_radians();
        imageproc::geometric_transformations::rotate_about_center_no_crop(
            &subj_resized,
            theta,
            imageproc::geometric_transformations::Interpolation::Bilinear,
            imageproc::geometric_transformations::Border::Constant(Rgba([0u8, 0, 0, 0])),
        )
    } else {
        subj_resized
    };

    // Place on s3 x s3 canvas.
    // Center of s3 is 1.5 * size.
    // How should offset_x / offset_y be scaled?
    // Wait: in `tokenatra_recipe` and `TOKEN_STYLE.md` and `SUBJECT_ANALYSIS.md`:
    // Let's check what offset units are expected!
    // In `TOKEN_STYLE.md`:
    //   `L(fx) = 512 + (fx - 512) * S + OX`
    //   `OX = 512 - (ex - 512) * S`
    //   `OY = 315 - (ey - 512) * S`
    //   Look at `TOKEN_STYLE.md` lines 52-63 and 123-129:
    //   Lotta: S = 1.045, OX = -22, OY = +16.
    //   Elias: S = 1.75, OX = -48, OY = +85.
    //   Taumiel: S = 1.71, OX = +96, OY = +202.
    //   In ALL of `TOKEN_STYLE.md` and `AGENTS.md`:
    //   `L(f) = 512 + (f - 512)*scale + offset`!
    //   1 unit of `offset_x` / `offset_y` is 1 logical pixel of the 1024x1024 1x export frame!
    //   Wait why did `tokenatra_recipe` in `tokenatra_mcp.py` say `O is multiplied by 3 in the export`?
    //   Because in `agent.rs` line 423, `scale3 = s3 / 1024 = 3.0` multiplied `offset` by 3 while `subj` was 1x!
    //   Wait: if we check `AGENTS.md`:
    //   "**Семантика transform**: логический канвас 1024, `L(f) = 512 + (f−512)·scale + offset` (f — fit-координата источника)."
    //   And in `TOKEN_STYLE.md`:
    //   `L(fx) = 512 + (fx - 512)*S + OX`
    //   `S = (870 - 315) / (wy - ey)`
    //   `OX = 512 - (ex - 512)*S`
    //   `OY = 315 - (ey - 512)*S`
    //    When `1 offset unit = 1 pixel in 1024 1x export frame`, the formula `L(f) = 512 + (f - 512)*S + O`
    //   has NO factor of 3! It's 1:1 with the 1024 export frame AND 1:1 with `state.imageX` in the frontend!
    //   Let's verify that!
    //   In the frontend (`tokenCanvas.js` line 1256):
    //   `var imgRenderScale = internalSize / 1024;`
    //   `var coordScale = scale3Size / internalSize;` -> `imgRenderScale * coordScale = scale3Size / 1024 = 3 * qualityBase / 1024`
    //   In the frontend, `1024` is the full `3x` canvas (`internalSize`), so the 1x ring is `1024 / 3 = 341.33` units wide.
    //   Whereas in the Agent API (`AGENTS.md` and `TOKEN_STYLE.md`), the 1x ring frame is `1024 x 1024`,
    //   so `S = 1.0` means the subject is `1024 px` wide (filling the 1x ring frame), and `offset_x, offset_y`
    //   are in pixels of that same `1024 x 1024` 1x ring frame!
    let px_per_logical = size as f32 / LOGICAL;
    let cx = (s3 as f32) * 0.5 + session.offset_x * px_per_logical;
    let cy = (s3 as f32) * 0.5 + session.offset_y * px_per_logical;
    let px0 = (cx - subj_rotated.width() as f32 * 0.5).round() as i64;
    let py0 = (cy - subj_rotated.height() as f32 * 0.5).round() as i64;

    let mut layer = RgbaImage::new(s3, s3);
    image::imageops::overlay(&mut layer, &subj_rotated, px0, py0);

    // Apply pink canvas_mask in 1x export frame coordinates (the central `size x size` region of `s3`,
    // and extended outside if needed: wait — what if the head sticks out above the 1x frame into 2x/3x?).
    // Let's check! In the 1x export frame (`[size .. 2*size]`), `canvas_mask` (1024x1024) maps 1:1 to
    // the `[size .. 2*size]` square! What about pixels outside `[size .. 2*size]` (if someone exports 2x/3x)?
    // For any `(x, y)` in `s3`, its clamped coordinate in `[size .. 2*size]` (or mapped across 1x)
    // Wait: in `TOKEN_STYLE.md`:
    //   `erase rect (0, 270, 1024, 1024)`
    //   `restore circle (512, 270, r=480)`
    // Notice those coordinates `(512, 270, r=480)` are in `[0..1024]` of the 1x export frame!
    // If we map `canvas_mask` (1024x1024) to the 1x export frame `[off .. off + size]` (where `off = size`),
    // and for pixels outside `[off .. off + size]` we clamp to the nearest edge of `canvas_mask`
    // (so `y < off` uses `mask_y = 0`, which is 255 = visible, and `y >= off + size` uses `mask_y = size - 1`,
    // which is 0 = erased at the bottom!), then:
    // - Inside the 1x export frame, `(0..1024)` is 100% exact pixel-for-pixel!
    // - Above the 1x frame (if exporting 2x/3x with a tall hat), the top is not clipped because `mask_y = 0` is white (255)!
    // - Below the 1x frame (where the skirt/legs were erased at the bottom), `mask_y = size - 1` is black (0), so legs stay erased even in 2x/3x!
    // Apply pink canvas_mask across the entire s3 x s3 canvas (1:1 at size=1024, or resized for preview)
    let mask_s3 = if session.canvas_mask.dimensions() == (s3, s3) {
        session.canvas_mask.clone()
    } else {
        resize_mask(&session.canvas_mask, s3, s3)
    };
    for (p, m) in layer.pixels_mut().zip(mask_s3.pixels()) {
        if p.0[3] > 0 {
            p.0[3] = ((p.0[3] as u32) * (m.0[0] as u32) / 255) as u8;
        }
    }

    layer
}

fn compose(
    session: &AgentSession,
    size: u32,
    scale_mode: &str,
    with_ring: bool,
) -> Result<(RgbaImage, u32), String> {
    let s3 = size * 3;
    let mut canvas = RgbaImage::new(s3, s3);

    if with_ring
        && let Some(rname) = &session.ring
            && let Some(rp) = find_ring_file(rname)
                && let Ok(ring) = image::open(&rp) {
                    let ring = image::imageops::resize(
                        &ring.to_rgba8(),
                        size,
                        size,
                        image::imageops::FilterType::Lanczos3,
                    );
                    let off = ((s3 - size) / 2) as i64;
                    image::imageops::overlay(&mut canvas, &ring, off, off);
                }

    let subj = render_subject_on_s3(session, size);
    let px_per_logical = size as f32 / LOGICAL;

    // Drop shadow from the masked subject (identical to TokenCanvas.renderForSave)
    if session.shadow_enabled {
        let rad = session.shadow_angle.to_radians();
        let dx = (rad.cos() * session.shadow_distance * px_per_logical * 3.0).round() as i64;
        let dy = (-rad.sin() * session.shadow_distance * px_per_logical * 3.0).round() as i64;
        let mut sil = GrayImage::new(s3, s3);
        for (dp, sp) in sil.pixels_mut().zip(subj.pixels()) {
            *dp = Luma([sp.0[3]]);
        }
        let mut sil_shift = GrayImage::new(s3, s3);
        image::imageops::overlay(&mut sil_shift, &sil, dx, dy);
        let blur_sigma = (session.shadow_blur * px_per_logical * 1.5).max(0.1);
        let sil_blur = image::imageops::blur(&sil_shift, blur_sigma);
        let mut shadow_layer = RgbaImage::new(s3, s3);
        for (p, sp) in shadow_layer.pixels_mut().zip(sil_blur.pixels()) {
            *p = Rgba([0, 0, 0, ((sp.0[0] as f32) * session.shadow_opacity) as u8]);
        }
        image::imageops::overlay(&mut canvas, &shadow_layer, 0, 0);
    }

    image::imageops::overlay(&mut canvas, &subj, 0, 0);

    let used = match scale_mode {
        "1" => 1,
        "2" => 2,
        "3" => 3,
        _ => {
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
            if min_x == u32::MAX {
                1
            } else {
                let f = s3 as f32 / scan as f32;
                let mn_x = (min_x as f32 * f - f).max(0.0);
                let mn_y = (min_y as f32 * f - f).max(0.0);
                let mx_x = ((max_x + 1) as f32 * f + f).min(s3 as f32);
                let mx_y = ((max_y + 1) as f32 * f + f).min(s3 as f32);
                let mut chosen = 3;
                for cand in 1..=3u32 {
                    let border = (s3 * cand / 3) as f32;
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
    Ok((out, used))
}

/// Loads `example.png` (from resource_dir or base_dir) for the Example overlay.
fn load_example_image() -> Option<RgbaImage> {
    for base in [paths::resource_dir(), paths::base_dir()] {
        for ext in ["png", "webp", "jpg"] {
            let p = base.join(format!("example.{ext}"));
            if p.is_file()
                && let Ok(bytes) = std::fs::read(&p)
                    && let Ok(img) = image::load_from_memory(&bytes) {
                        return Some(img.to_rgba8());
                    }
        }
    }
    None
}

/// Overlays `example.png` onto the preview image (mirrors `TokenCanvas._drawExampleOverlay` in UI).
/// By default (`example_scale = 2`), `example.png` covers the 2x export area (so on a 2x preview it
/// maps 1:1 to the entire preview canvas; on a 1x preview its central 1/2 is cropped to match the 1x ring).
fn draw_example_overlay(img: &mut RgbaImage, used_scale: u32, example_scale: u32, opacity: f32) {
    let Some(ex) = load_example_image() else {
        return;
    };
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let used = used_scale.clamp(1, 3) as f32;
    let ex_m = example_scale.clamp(1, 3) as f32;
    // Size of the example overlay in pixels of `img` (which represents `used` x)
    let ex_w = ((w as f32) * ex_m / used).round().max(1.0) as u32;
    let ex_h = ((h as f32) * ex_m / used).round().max(1.0) as u32;
    let ex_resized = image::imageops::resize(&ex, ex_w, ex_h, image::imageops::FilterType::Lanczos3);
    let ox = ((w as i64) - (ex_w as i64)) / 2;
    let oy = ((h as i64) - (ex_h as i64)) / 2;
    let alpha_mul = opacity.clamp(0.0, 1.0);

    for (ex_x, ex_y, src_px) in ex_resized.enumerate_pixels() {
        if src_px.0[3] == 0 {
            continue;
        }
        let dx = ox + ex_x as i64;
        let dy = oy + ex_y as i64;
        if dx >= 0 && dy >= 0 && (dx as u32) < w && (dy as u32) < h {
            let dst = img.get_pixel_mut(dx as u32, dy as u32);
            let sa = (src_px.0[3] as f32 / 255.0) * alpha_mul;
            let da = dst.0[3] as f32 / 255.0;
            let out_a = sa + da * (1.0 - sa);
            if out_a > 0.0 {
                dst.0[0] = ((src_px.0[0] as f32 * sa + dst.0[0] as f32 * da * (1.0 - sa)) / out_a)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                dst.0[1] = ((src_px.0[1] as f32 * sa + dst.0[1] as f32 * da * (1.0 - sa)) / out_a)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                dst.0[2] = ((src_px.0[2] as f32 * sa + dst.0[2] as f32 * da * (1.0 - sa)) / out_a)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                dst.0[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Draws visual alignment guides on a preview image (1x ring coordinate space):
/// Matches `example.png` (scaled at 2x over the 1x ring):
/// - Vertical center line `x = 512` (cyan)
/// - Head oval (`cx = 524`, `cy = 134`, `rx = 362`, `ry = 274`) & Crown line `y = -140` / Jaw line `y = 408` (magenta)
/// - Belt line `y = 716` (amber/orange)
fn draw_preview_guides(img: &mut RgbaImage, used_scale: u32) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let map_coord = |logical_val: f32, dim: u32| -> i64 {
        let norm_1x = logical_val / LOGICAL;
        let frac = (used_scale as f32 - 1.0) * 0.5 / (used_scale as f32) + norm_1x / (used_scale as f32);
        (frac * dim as f32).round() as i64
    };

    let blend_px = |img: &mut RgbaImage, x: i64, y: i64, color: [u8; 4]| {
        if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
            let dst = img.get_pixel_mut(x as u32, y as u32);
            let sa = color[3] as f32 / 255.0;
            let inv = 1.0 - sa;
            dst.0[0] = (color[0] as f32 * sa + dst.0[0] as f32 * inv) as u8;
            dst.0[1] = (color[1] as f32 * sa + dst.0[1] as f32 * inv) as u8;
            dst.0[2] = (color[2] as f32 * sa + dst.0[2] as f32 * inv) as u8;
            dst.0[3] = dst.0[3].max(color[3]);
        }
    };

    // Center X = 512 (dashed cyan)
    let cx = map_coord(512.0, w);
    for y in 0..h as i64 {
        if (y / 6) % 2 == 0 {
            blend_px(img, cx, y, [0, 220, 255, 180]);
        }
    }

    // Crown line ("Макушка") y = -140 in 1x coords (visible in 2x/3x preview)
    let crown_y = map_coord(-140.0, h);
    for x in 0..w as i64 {
        if (x / 6) % 2 == 0 {
            blend_px(img, x, crown_y, [200, 0, 255, 210]);
        }
    }

    // Jaw line ("Челюсть") y = 408 in 1x coords
    let jaw_y = map_coord(408.0, h);
    for x in 0..w as i64 {
        if (x / 6) % 2 == 0 {
            blend_px(img, x, jaw_y, [200, 0, 255, 210]);
        }
    }

    // Belt line ("Пояс") y = 716 in 1x coords
    let belt_y = map_coord(716.0, h);
    for x in 0..w as i64 {
        if (x / 6) % 2 == 0 {
            blend_px(img, x, belt_y, [255, 140, 0, 210]);
        }
    }
}

fn resize_mask(mask: &GrayImage, w: u32, h: u32) -> GrayImage {
    image::imageops::resize(mask, w, h, image::imageops::FilterType::Triangle)
}

// ---------- inspect ----------

/// Computes comprehensive statistics on BOTH the source image and the composed 1x token frame.
fn inspect_session(session: &AgentSession) -> Value {
    // 1. Source image alpha analysis (to know if background is already transparent and where the figure is)
    let base = session.active_base();
    let (bw, bh) = base.dimensions();
    let btotal = (bw * bh).max(1) as f64;
    let mut b_opaque = 0u64;
    let mut b_visible = 0u64;
    let mut b_min_x = u32::MAX;
    let mut b_min_y = u32::MAX;
    let mut b_max_x = 0u32;
    let mut b_max_y = 0u32;

    for (x, y, p) in base.enumerate_pixels() {
        let a = p.0[3];
        if a >= 250 {
            b_opaque += 1;
        }
        if a > 15 {
            b_visible += 1;
            b_min_x = b_min_x.min(x);
            b_min_y = b_min_y.min(y);
            b_max_x = b_max_x.max(x);
            b_max_y = b_max_y.max(y);
        }
    }
    let opaque_pct = ((b_opaque as f64 / btotal) * 1000.0).round() / 10.0;
    let visible_pct = ((b_visible as f64 / btotal) * 1000.0).round() / 10.0;
    let already_transparent = opaque_pct < 92.0;

    // Fit-1024 coordinates of the source figure bbox
    let max_dim = bw.max(bh).max(1) as f32;
    let fit_k = LOGICAL / max_dim;
    let pad_x = (LOGICAL - bw as f32 * fit_k) * 0.5;
    let pad_y = (LOGICAL - bh as f32 * fit_k) * 0.5;
    let source_fit_bbox = if b_visible > 0 {
        json!([
            (b_min_x as f32 * fit_k + pad_x).round() as i64,
            (b_min_y as f32 * fit_k + pad_y).round() as i64,
            (b_max_x as f32 * fit_k + pad_x).round() as i64,
            (b_max_y as f32 * fit_k + pad_y).round() as i64,
        ])
    } else {
        Value::Null
    };

    // 2. Composed subject in the 1024x1024 1x export frame
    let s3_subj = render_subject_on_s3(session, LOGICAL_U32);
    let subj_1x = image::imageops::crop_imm(&s3_subj, LOGICAL_U32, LOGICAL_U32, LOGICAL_U32, LOGICAL_U32).to_image();

    let mut hist = [0u64; 5];
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut edge_px = 0u64;
    let mut covered = 0u64;
    let total = (LOGICAL_U32 * LOGICAL_U32) as f32;
    let mut grid = [[0f64; 3]; 3];
    let mut grid_tot = [[0u64; 3]; 3];

    for (x, y, p) in subj_1x.enumerate_pixels() {
        let a = p.0[3] as u64;
        hist[((a / 52) as usize).min(4)] += 1;
        if a > 15 {
            covered += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        if a > 15 && a < 240 {
            edge_px += 1;
        }
        let gx = (x * 3 / LOGICAL_U32).min(2) as usize;
        let gy = (y * 3 / LOGICAL_U32).min(2) as usize;
        grid_tot[gy][gx] += 1;
        if a > 15 {
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
    let cov_pct = round1(covered as f32 / total * 100.0);

    let mut warnings = Vec::new();
    if covered > 0 {
        if min_x == 0 || min_y == 0 || max_x >= LOGICAL_U32 - 1 || max_y >= LOGICAL_U32 - 1 {
            warnings.push("subject touches 1x canvas border");
        }
        if max_y > 920 && session.ring.is_some() {
            warnings.push("subject extends below ring lower rim (y > 920); consider erasing bottom arc or raising offset_y");
        }
    } else {
        warnings.push("subject is completely empty/invisible in 1x frame");
    }
    if !already_transparent && !session.bg_removed {
        warnings.push("source has opaque background; call remove_background first");
    }

    json!({
        "bbox": bbox_json,
        "coverage_pct": cov_pct,
        "alpha_hist": hist,
        "edge_px": edge_px,
        "grid_3x3_pct": grid,
        "warnings": warnings,
        "source_info": {
            "width": bw,
            "height": bh,
            "opaque_pct": opaque_pct,
            "visible_pct": visible_pct,
            "already_transparent": already_transparent,
            "fit1024_bbox": source_fit_bbox,
        }
    })
}

fn round1(v: f32) -> f64 {
    ((v * 10.0).round() / 10.0) as f64
}

// ---------- portrait generator ----------

fn render_portrait(session: &AgentSession, size: u32) -> RgbaImage {
    let src = if session.portrait_variant == "cutout" {
        session.cutout.as_ref().unwrap_or(&session.original)
    } else {
        &session.original
    };
    let (sw, sh) = src.dimensions();
    let max_dim = sw.max(sh).max(1) as f32;
    // In PortraitGenerator, `resetTransform` sets `imageScale = 2048 / maxDim`,
    // so relative scale 1.0 fits the image into `size`.
    let fit_scale = (size as f32 / max_dim) * session.portrait_scale;
    let tw = (sw as f32 * fit_scale).round().max(1.0) as u32;
    let th = (sh as f32 * fit_scale).round().max(1.0) as u32;
    let resized = image::imageops::resize(src, tw, th, image::imageops::FilterType::Lanczos3);
    let rotated = if session.portrait_rotation.abs() % 360.0 > f32::EPSILON {
        let theta = session.portrait_rotation.to_radians();
        imageproc::geometric_transformations::rotate_about_center_no_crop(
            &resized,
            theta,
            imageproc::geometric_transformations::Interpolation::Bilinear,
            imageproc::geometric_transformations::Border::Constant(Rgba([0u8, 0, 0, 0])),
        )
    } else {
        resized
    };
    let px_per_logical = size as f32 / LOGICAL;
    let cx = size as f32 * 0.5 + session.portrait_x * px_per_logical;
    let cy = size as f32 * 0.5 + session.portrait_y * px_per_logical;
    let px0 = (cx - rotated.width() as f32 * 0.5).round() as i64;
    let py0 = (cy - rotated.height() as f32 * 0.5).round() as i64;

    let mut out = RgbaImage::new(size, size);
    image::imageops::overlay(&mut out, &rotated, px0, py0);
    out
}

// ---------- handlers ----------

pub async fn ping() -> Response {
    ok_json(json!({"pong": true, "agent": true}))
}

pub async fn manifest(State(state): State<std::sync::Arc<AppState>>) -> Response {
    let model_path = paths::get_selected_model_path();
    let model_exists = model_path.exists();
    ok_json(json!({
        "api_version": 2,
        "name": "tokenatra",
        "version": VERSION,
        "device": state.device_name(),
        "model_exists": model_exists,
        "model_loaded": crate::infer::model_loaded(&state),
        "model_name": model_exists.then(|| model_path.file_name().map(|s| s.to_string_lossy().to_string())).flatten(),
        "logical_size": 1024,
        "ops": [
            "create", "remove_bg", "mask", "transform", "fit", "ring",
            "effects", "history", "portrait", "save_preset", "inspect",
            "preview", "export", "close"
        ],
        "mask_ops": ["erase", "restore", "apply_preset", "invert", "feather", "grow", "reset"],
        "mask_targets": ["canvas", "image", "both"],
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
            "ring": "steel",
            "protection_enabled": true,
            "shadow": {"enabled": false, "angle": 135.0, "distance": 7.0, "blur": 3.0, "opacity": 0.4},
            "color": {"enabled": false, "lightness": 0, "saturation": 0, "contrast": 0}
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
            {"type": "function", "function": {"name": "tokenatra_create_session", "description": "Create an editing session from an image_path or image_b64", "parameters": {"type": "object", "properties": {"image_path": {"type": "string"}, "image_b64": {"type": "string"}, "ring": {"type": ["string", "null"]}}}}},
            {"type": "function", "function": {"name": "tokenatra_remove_background", "description": "Remove background (ONNX) or toggle showing_original", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "edge_blur": {"type": "number"}, "showing_original": {"type": "boolean"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_edit_mask", "description": "Edit visibility mask (1024x1024 logical). target: canvas (pink mask, fixed to ring) | image (blue eraser, moves with subject). ops: erase/restore/apply_preset/invert/feather/grow/reset", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "target": {"type": "string", "enum": ["canvas", "image", "both"]}, "op": {"type": "string", "enum": ["erase", "restore", "apply_preset", "invert", "feather", "grow", "reset"]}, "shapes_json": {"type": "string"}, "preset": {"type": "string"}, "px": {"type": "integer"}, "protect": {"type": "boolean"}}, "required": ["session_id", "op"]}}},
            {"type": "function", "function": {"name": "tokenatra_set_transform", "description": "Set subject transform in 1024x1024 1x export frame: L(f) = 512 + (f - 512)*scale + offset", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "scale": {"type": "number"}, "rotation": {"type": "number"}, "offset_x": {"type": "number"}, "offset_y": {"type": "number"}, "fit_zone": {"type": "integer", "enum": [1, 2, 3]}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_set_ring", "description": "Set token ring by name or remove (null)", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "name": {"type": ["string", "null"]}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_set_effects", "description": "Shadow and color correction", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "shadow_json": {"type": "string"}, "color_json": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_history", "description": "Undo or redo last editing action", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "action": {"type": "string", "enum": ["undo", "redo"]}}, "required": ["session_id", "action"]}}},
            {"type": "function", "function": {"name": "tokenatra_portrait", "description": "Configure and export a square portrait (PortraitGenerator)", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "variant": {"type": "string", "enum": ["original", "cutout"]}, "scale": {"type": "number"}, "rotation": {"type": "number"}, "offset_x": {"type": "number"}, "offset_y": {"type": "number"}, "size": {"type": "integer"}, "format": {"type": "string", "enum": ["webp", "png", "jpg"]}, "quality": {"type": "integer"}, "output_path": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_inspect", "description": "Session state & alpha bbox stats", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "include_image": {"type": "boolean"}, "preview_size": {"type": "integer"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_preview", "description": "Preview image (webp) with optional alignment guides", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "size": {"type": "integer"}, "scale_mode": {"type": "string", "enum": ["auto", "1", "2", "3"]}, "with_ring": {"type": "boolean"}, "guides": {"type": "boolean"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_export", "description": "Export final token image", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "format": {"type": "string", "enum": ["webp", "png", "jpg"]}, "quality": {"type": "integer"}, "size": {"type": "integer"}, "scale_mode": {"type": "string", "enum": ["auto", "1", "2", "3"]}, "with_ring": {"type": "boolean"}, "output_path": {"type": "string"}, "save_to": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_close_session", "description": "Close session and free memory", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_list_assets", "description": "List rings and presets", "parameters": {"type": "object", "properties": {}}}}
        ]
    }))
}

async fn run_blocking<F>(f: F) -> Response
where
    F: FnOnce() -> Response + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(resp) => resp,
        Err(e) => agent_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "join_error",
            &e.to_string(),
        ),
    }
}

pub async fn create_session(
    State(_state): State<std::sync::Arc<AppState>>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    run_blocking(move || {
        let ring_override = if v.get("ring").is_some() {
            if v["ring"].is_null() {
                Some(None)
            } else if let Some(rname) = v["ring"].as_str() {
                if find_ring_file(rname).is_none() {
                    return agent_err(StatusCode::NOT_FOUND, "unknown_asset", "ring not found");
                }
                Some(Some(rname.to_string()))
            } else {
                None
            }
        } else {
            None
        };

        match decode_source(&v) {
            Ok(base) => {
                let arc = store_create(base, ring_override);
                let sess = arc.lock().unwrap();
                let stats = inspect_session(&sess);
                ok_json(json!({
                    "session_id": sess.id,
                    "image": {"width": sess.original.width(), "height": sess.original.height()},
                    "ring": sess.ring,
                    "transform": {
                        "scale": sess.scale,
                        "rotation": sess.rotation,
                        "offset_x": sess.offset_x,
                        "offset_y": sess.offset_y,
                    },
                    "mask": stats,
                }))
            }
            Err(r) => r,
        }
    })
    .await
}

#[allow(clippy::result_large_err)]
fn session_op_guard(sid: &str) -> Result<Arc<Mutex<AgentSession>>, Response> {
    let arc = store_get(sid).ok_or_else(|| {
        agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session")
    })?;
    {
        let s = arc.lock().unwrap();
        if s.ops_used >= OPS_LIMIT {
            return Err(agent_err(
                StatusCode::TOO_MANY_REQUESTS,
                "ops_limit",
                "operation limit reached for this session",
            ));
        }
    }
    Ok(arc)
}

pub async fn get_session(
    State(_): State<std::sync::Arc<AppState>>,
    Path(sid): Path<String>,
) -> Response {
    match store_get(&sid) {
        Some(arc) => {
            let s = arc.lock().unwrap();
            ok_json(json!({
                "session_id": s.id,
                "image": {"width": s.original.width(), "height": s.original.height()},
                "bg_removed": s.bg_removed,
                "showing_original": s.showing_original,
                "protection_enabled": s.protection_enabled,
                "transform": {"scale": s.scale, "rotation": s.rotation, "offset_x": s.offset_x, "offset_y": s.offset_y},
                "ring": s.ring,
                "effects": {
                    "shadow": {"enabled": s.shadow_enabled, "angle": s.shadow_angle, "distance": s.shadow_distance, "blur": s.shadow_blur, "opacity": s.shadow_opacity},
                    "color": {"enabled": s.color_enabled, "saturation": s.color_sat, "lightness": s.color_light, "contrast": s.color_contrast}
                },
                "portrait": {
                    "variant": s.portrait_variant,
                    "scale": s.portrait_scale,
                    "rotation": s.portrait_rotation,
                    "offset_x": s.portrait_x,
                    "offset_y": s.portrait_y,
                },
                "history": {
                    "index": s.history_index,
                    "count": s.history.len(),
                    "can_undo": s.history_index > 0,
                    "can_redo": s.history_index + 1 < s.history.len(),
                },
                "ops_used": s.ops_used,
                "ops_limit": OPS_LIMIT,
                "recipe": s.ops_log,
            }))
        }
        None => agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session"),
    }
}

pub async fn close_session(Path(sid): Path<String>) -> Response {
    store_remove(&sid);
    ok_json(json!({"closed": true}))
}

pub async fn op_remove_bg(
    State(state): State<std::sync::Arc<AppState>>,
    Path(sid): Path<String>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let v = body.map(|axum::Json(v)| v).unwrap_or_else(|| json!({}));
    let edge_blur = v["edge_blur"].as_f64().unwrap_or(1.0);
    let arc = match session_op_guard(&sid) {
        Ok(s) => s,
        Err(r) => return r,
    };

    let orig = {
        let mut s = arc.lock().unwrap();
        // If `showing_original` is explicitly passed and we already have a cutout, just toggle it (like clicking Remove BG again in UI)
        if let Some(show_orig) = v["showing_original"].as_bool()
            && s.cutout.is_some()
        {
            s.showing_original = show_orig;
            s.log_op("toggle_original", json!({"showing_original": show_orig}));
            s.push_history();
            let stats = inspect_session(&s);
            return ok_json(json!({"ok": true, "bg_removed": true, "showing_original": show_orig, "mask": stats}));
        }
        s.original.clone()
    };

    state.mark_activity();
    let st = state.clone();
    let infer_res = tokio::task::spawn_blocking(move || {
        let dyn_img = image::DynamicImage::ImageRgba8(orig);
        crate::infer::remove_background(&st, dyn_img, edge_blur)
    })
    .await;

    state.mark_activity();

    match infer_res {
        Ok(Ok(rgba)) => {
            let mut s = arc.lock().unwrap();
            s.cutout = Some(rgba);
            s.bg_removed = true;
            s.showing_original = false;
            s.portrait_variant = "cutout".to_string();
            // Just like TokenEditor.handleRemoveBackground in UI: reset imageMask to white
            s.image_mask = GrayImage::from_pixel(s.original.width(), s.original.height(), Luma([255u8]));
            s.log_op("remove_bg", json!({"edge_blur": edge_blur}));
            s.push_history();
            let stats = inspect_session(&s);
            ok_json(json!({"ok": true, "bg_removed": true, "showing_original": false, "mask": stats}))
        }
        Ok(Err(e)) => agent_err(StatusCode::INTERNAL_SERVER_ERROR, "infer_error", &e.to_string()),
        Err(e) => agent_err(StatusCode::INTERNAL_SERVER_ERROR, "join_error", &e.to_string()),
    }
}

pub async fn op_mask(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    run_blocking(move || {
        let arc = match session_op_guard(&sid) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let mut s = arc.lock().unwrap();
        let op = v["op"].as_str().unwrap_or_default().to_string();
        let target = v["target"].as_str().unwrap_or("canvas").to_lowercase();
        let px = v["px"].as_u64().unwrap_or(0) as u32;
        if let Some(pe) = v["protection_enabled"].as_bool() {
            s.protection_enabled = pe;
        }
        let protect = v["protect"].as_bool().unwrap_or(s.protection_enabled);
        let params = v.clone();

        match op.as_str() {
            "erase" | "restore" => {
                let erase = op == "erase";
                if v["shapes"].as_array().is_none() {
                    return bad("bad_request", "shapes array required for erase/restore");
                }
                if target == "image" || target == "blue" {
                    let prev = s.image_mask.clone();
                    mask_apply_shapes(&mut s.image_mask, &v["shapes"], erase, false);
                    if erase && protect
                        && let Some(prot_1024) = load_protection_mask_1024(s.ring.as_deref()) {
                            let (iw, ih) = s.image_mask.dimensions();
                            let max_dim = iw.max(ih).max(1) as f32;
                            let fit_scale = (LOGICAL / max_dim) * s.scale;
                            let theta = s.rotation.to_radians();
                            let cos_r = theta.cos();
                            let sin_r = theta.sin();
                            let cx = 512.0 + s.offset_x;
                            let cy = 512.0 + s.offset_y;
                            for (x, y, p) in s.image_mask.enumerate_pixels_mut() {
                                let old_v = prev.get_pixel(x, y).0[0];
                                if p.0[0] >= old_v {
                                    continue;
                                }
                                let lx = (x as f32 - iw as f32 * 0.5) * fit_scale;
                                let ly = (y as f32 - ih as f32 * 0.5) * fit_scale;
                                let rx = cos_r * lx - sin_r * ly + cx;
                                let ry = sin_r * lx + cos_r * ly + cy;
                                if rx >= 0.0 && ry >= 0.0 && rx < LOGICAL && ry < LOGICAL
                                    && prot_1024.get_pixel(rx as u32, ry as u32).0[0] > 0 {
                                        p.0[0] = old_v;
                                    }
                            }
                        }
                } else {
                    let prev = s.canvas_mask.clone();
                    mask_apply_shapes(&mut s.canvas_mask, &v["shapes"], erase, true);
                    if erase && protect
                        && let Some(prot_1024) = load_protection_mask_1024(s.ring.as_deref()) {
                            let off = LOGICAL_U32;
                            for y in 0..LOGICAL_U32 {
                                for x in 0..LOGICAL_U32 {
                                    if prot_1024.get_pixel(x, y).0[0] > 0 {
                                        let old_v = prev.get_pixel(off + x, off + y).0[0];
                                        s.canvas_mask.put_pixel(off + x, off + y, Luma([old_v]));
                                    }
                                }
                            }
                        }
                }
            }
            "invert" => {
                let m = if target == "image" || target == "blue" {
                    &mut s.image_mask
                } else {
                    &mut s.canvas_mask
                };
                for p in m.pixels_mut() {
                    p.0[0] = 255 - p.0[0];
                }
            }
            "reset" => {
                if target == "image" || target == "blue" {
                    s.image_mask = GrayImage::from_pixel(s.original.width(), s.original.height(), Luma([255u8]));
                } else if target == "both" {
                    s.canvas_mask = GrayImage::from_pixel(LOGICAL_U32 * 3, LOGICAL_U32 * 3, Luma([255u8]));
                    s.image_mask = GrayImage::from_pixel(s.original.width(), s.original.height(), Luma([255u8]));
                } else {
                    s.canvas_mask = GrayImage::from_pixel(LOGICAL_U32 * 3, LOGICAL_U32 * 3, Luma([255u8]));
                }
            }
            "feather" => {
                if px == 0 {
                    return bad("bad_request", "px required for feather");
                }
                if target == "image" || target == "blue" {
                    s.image_mask = image::imageops::blur(&s.image_mask, px as f32 * 0.5);
                } else {
                    s.canvas_mask = image::imageops::blur(&s.canvas_mask, px as f32 * 0.5);
                }
            }
            "grow" => {
                if px == 0 {
                    return bad("bad_request", "px required for grow");
                }
                use imageproc::distance_transform::Norm;
                if target == "image" || target == "blue" {
                    s.image_mask = imageproc::morphology::dilate(&s.image_mask, Norm::LInf, px.min(255) as u8);
                } else {
                    s.canvas_mask = imageproc::morphology::dilate(&s.canvas_mask, Norm::LInf, px.min(255) as u8);
                }
            }
            "apply_preset" => {
                let pname = v["preset"].as_str().unwrap_or_default();
                let Some(pp) = find_preset_file(pname) else {
                    return agent_err(StatusCode::NOT_FOUND, "unknown_preset", "preset not found");
                };
                match load_preset_mask(&pp, LOGICAL_U32 * 3, LOGICAL_U32 * 3) {
                    Ok(m) => s.canvas_mask = m,
                    Err(e) => return bad("bad_request", &format!("preset decode failed: {e}")),
                }
            }
            other => return bad("bad_mask_op", &format!("unknown mask op {other:?}")),
        }

        s.log_op(&format!("mask.{op}"), params);
        s.push_history();
        let stats = inspect_session(&s);
        ok_json(json!({"ok": true, "mask": stats}))
    })
    .await
}

pub async fn op_transform(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    run_blocking(move || {
        let arc = match session_op_guard(&sid) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let mut s = arc.lock().unwrap();
        let params = v.clone();

        if let Some(sc) = v["scale"].as_f64()
            && (sc <= 0.0 || sc > 8.0)
        {
            return bad("bad_request", "scale out of range (0.01..8.0)");
        }
        if let Some(zone) = v["fit_zone"].as_u64() {
            let z = zone.clamp(1, 3) as f32;
            s.scale = z;
            s.offset_x = 0.0;
            s.offset_y = 0.0;
            s.rotation = 0.0;
        }
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
        s.log_op("transform", params);
        s.push_history();
        let stats = inspect_session(&s);
        let resp = json!({
            "ok": true,
            "transform": {
                "scale": s.scale,
                "rotation": s.rotation,
                "offset_x": s.offset_x,
                "offset_y": s.offset_y,
            },
            "mask": stats,
        });
        ok_json(resp)
    })
    .await
}

pub async fn op_ring(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let arc = match session_op_guard(&sid) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let name = if v["name"].is_null() {
        None
    } else {
        v["name"].as_str().map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
    };
    if let Some(n) = &name
        && find_ring_file(n).is_none() {
            return agent_err(StatusCode::NOT_FOUND, "unknown_asset", "ring not found");
        }
    let mut s = arc.lock().unwrap();
    s.ring = name.clone();
    s.log_op("ring", json!({"name": name}));
    ok_json(json!({"ok": true, "ring": name}))
}

pub async fn op_effects(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let arc = match session_op_guard(&sid) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = arc.lock().unwrap();
    let params = v.clone();
    if let Some(sh) = v["shadow"].as_object() {
        if let Some(en) = sh.get("enabled").and_then(|x| x.as_bool()) {
            s.shadow_enabled = en;
        }
        if let Some(a) = sh.get("angle").and_then(|x| x.as_f64()) {
            s.shadow_angle = a as f32;
        }
        if let Some(d) = sh.get("distance").and_then(|x| x.as_f64()) {
            s.shadow_distance = d as f32;
        }
        if let Some(b) = sh.get("blur").and_then(|x| x.as_f64()) {
            s.shadow_blur = b as f32;
        }
        if let Some(o) = sh.get("opacity").and_then(|x| x.as_f64()) {
            s.shadow_opacity = o.clamp(0.0, 1.0) as f32;
        }
    }
    if let Some(cl) = v["color"].as_object() {
        if let Some(en) = cl.get("enabled").and_then(|x| x.as_bool()) {
            s.color_enabled = en;
        }
        if let Some(sat) = cl.get("saturation").and_then(|x| x.as_f64()) {
            s.color_sat = sat as f32;
        }
        if let Some(l) = cl.get("lightness").and_then(|x| x.as_f64()) {
            s.color_light = l as f32;
        }
        if let Some(c) = cl.get("contrast").and_then(|x| x.as_f64()) {
            s.color_contrast = c as f32;
        }
    }
    s.log_op("effects", params);
    let resp = json!({
        "ok": true,
        "effects": {
            "shadow": {"enabled": s.shadow_enabled, "angle": s.shadow_angle, "distance": s.shadow_distance, "blur": s.shadow_blur, "opacity": s.shadow_opacity},
            "color": {"enabled": s.color_enabled, "saturation": s.color_sat, "lightness": s.color_light, "contrast": s.color_contrast},
        }
    });
    ok_json(resp)
}

pub async fn op_history(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    run_blocking(move || {
        let arc = match session_op_guard(&sid) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let mut s = arc.lock().unwrap();
        let action = v["action"].as_str().unwrap_or("undo");
        match action {
            "undo" => {
                if s.history_index == 0 {
                    return bad("nothing_to_undo", "already at oldest state");
                }
                s.history_index -= 1;
                let snap = s.history[s.history_index].clone();
                s.apply_history(snap);
            }
            "redo" => {
                if s.history_index + 1 >= s.history.len() {
                    return bad("nothing_to_redo", "already at newest state");
                }
                s.history_index += 1;
                let snap = s.history[s.history_index].clone();
                s.apply_history(snap);
            }
            other => return bad("bad_request", &format!("unknown history action {other:?}")),
        }
        let idx = s.history_index;
        let total = s.history.len();
        s.log_op(&format!("history.{action}"), json!({"index": idx}));
        let stats = inspect_session(&s);
        ok_json(json!({
            "ok": true,
            "history_index": idx,
            "history_count": total,
            "mask": stats,
        }))
    })
    .await
}

pub async fn op_portrait(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    run_blocking(move || {
        let arc = match session_op_guard(&sid) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let size = v["size"].as_u64().unwrap_or(1024).clamp(256, 4096) as u32;
        let format = v["format"].as_str().unwrap_or("webp").to_lowercase();
        let quality = v["quality"].as_u64().unwrap_or(95).clamp(10, 100) as u8;
        let img = {
            let mut s = arc.lock().unwrap();
            if let Some(var) = v["variant"].as_str() {
                s.portrait_variant = if var == "cutout" { "cutout".into() } else { "original".into() };
            }
            if let Some(sc) = v["scale"].as_f64() {
                s.portrait_scale = sc.clamp(0.05, 8.0) as f32;
            }
            if let Some(rot) = v["rotation"].as_f64() {
                s.portrait_rotation = rot as f32;
            }
            if let Some(ox) = v["offset_x"].as_f64() {
                s.portrait_x = ox as f32;
            }
            if let Some(oy) = v["offset_y"].as_f64() {
                s.portrait_y = oy as f32;
            }
            s.log_op("portrait", v.clone());
            render_portrait(&s, size)
        };

        match crate::imaging::save_image(&image::DynamicImage::ImageRgba8(img), &format, quality) {
            Ok((bytes, mime)) => file_response(bytes, mime),
            Err(e) => agent_err(StatusCode::INTERNAL_SERVER_ERROR, "encode_error", &e.to_string()),
        }
    })
    .await
}

pub async fn op_save_preset(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let v = body.map(|axum::Json(v)| v).unwrap_or_else(|| json!({}));
    run_blocking(move || {
        let arc = match session_op_guard(&sid) {
            Ok(s) => s,
            Err(r) => return r,
        };
        // Save current `canvas_mask` in UI preset semantics:
        // opaque purple (#9833ff) = cutout (where mask == 0), transparent = kept (where mask == 255).
        let mask_1024 = {
            let s = arc.lock().unwrap();
            resize_mask(&s.canvas_mask, LOGICAL_U32, LOGICAL_U32)
        };
        let mut snap = RgbaImage::new(LOGICAL_U32, LOGICAL_U32);
        for (x, y, p) in mask_1024.enumerate_pixels() {
            let cut_alpha = 255u8.saturating_sub(p.0[0]);
            snap.put_pixel(x, y, Rgba([152, 51, 255, cut_alpha]));
        }
        let dir = paths::user_preset_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "io_error", &e.to_string());
        }
        let custom_name = v["name"]
            .as_str()
            .map(|n| {
                n.chars()
                    .filter(|c| c.is_alphanumeric() || [' ', '_', '-'].contains(c))
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .filter(|n| !n.is_empty());
        let target = if let Some(cname) = custom_name {
            dir.join(format!("{cname}.png"))
        } else {
            let mut num = 1;
            while dir.join(format!("Пресет {num}.png")).exists() {
                num += 1;
            }
            dir.join(format!("Пресет {num}.png"))
        };
        let mut buf = Vec::new();
        if image::DynamicImage::write_to(
            &image::DynamicImage::ImageRgba8(snap),
            std::io::Cursor::new(&mut buf),
            image::ImageFormat::Png,
        )
        .is_err()
        {
            return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "encode_error", "failed to encode preset PNG");
        }
        if let Err(e) = paths::atomic_write(&target, &buf) {
            return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "io_error", &e.to_string());
        }
        ok_json(json!({
            "ok": true,
            "name": target.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
            "file": target.file_name().and_then(|s| s.to_str()).unwrap_or_default(),
        }))
    })
    .await
}

pub async fn op_preview(
    Path(sid): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    run_blocking(move || {
        let Some(arc) = store_get(&sid) else {
            return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
        };
        let size: u32 = q
            .get("size")
            .and_then(|v| v.parse().ok())
            .unwrap_or(512)
            .clamp(64, 1024);
        let show_example = q
            .get("example")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);
        let example_scale: u32 = q
            .get("example_scale")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2)
            .clamp(1, 3);
        let example_opacity: f32 = q
            .get("example_opacity")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.65_f32)
            .clamp(0.1, 1.0);
        // When `example=1` is requested and `scale_mode` is not explicitly given, default to "2"
        // so the entire 2x `example.png` template ("Макушка", "Челюсть", "Пояс") is visible!
        let default_sm = if show_example { "2" } else { "1" };
        let scale_mode = q.get("scale_mode").map(|s| s.as_str()).unwrap_or(default_sm);
        let with_ring = q
            .get("with_ring")
            .map(|v| v != "0" && v != "false")
            .unwrap_or(true);
        let guides = q
            .get("guides")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);

        let composed = {
            let s = arc.lock().unwrap();
            compose(&s, size, scale_mode, with_ring)
        };
        match composed {
            Ok((mut img, used)) => {
                if show_example {
                    draw_example_overlay(&mut img, used, example_scale, example_opacity);
                }
                if guides {
                    draw_preview_guides(&mut img, used);
                }
                match crate::imaging::save_image(
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
                }
            }
            Err(e) => agent_err(StatusCode::INTERNAL_SERVER_ERROR, "compose_error", &e),
        }
    })
    .await
}

pub async fn op_inspect(
    Path(sid): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    run_blocking(move || {
        let Some(arc) = store_get(&sid) else {
            return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
        };
        let mut s = arc.lock().unwrap();
        let mut stats = inspect_session(&s);
        let cur_cov = stats["coverage_pct"].as_f64().unwrap_or(0.0);
        if let Some(prev_cov) = s.last_inspect_coverage {
            stats["delta"] = json!({
                "coverage_dpct": ((cur_cov - prev_cov) * 10.0).round() / 10.0
            });
        }
        s.last_inspect_coverage = Some(cur_cov);

        let mut v = json!({
            "session_id": s.id,
            "image": {"width": s.original.width(), "height": s.original.height()},
            "bg_removed": s.bg_removed,
            "showing_original": s.showing_original,
            "protection_enabled": s.protection_enabled,
            "transform": {"scale": s.scale, "rotation": s.rotation, "offset_x": s.offset_x, "offset_y": s.offset_y},
            "ring": s.ring,
            "effects": {
                "shadow": {"enabled": s.shadow_enabled, "angle": s.shadow_angle, "distance": s.shadow_distance, "blur": s.shadow_blur, "opacity": s.shadow_opacity},
                "color": {"enabled": s.color_enabled, "saturation": s.color_sat, "lightness": s.color_light}
            },
            "history": {
                "index": s.history_index,
                "count": s.history.len(),
                "can_undo": s.history_index > 0,
                "can_redo": s.history_index + 1 < s.history.len(),
            },
            "ops_used": s.ops_used,
            "ops_limit": OPS_LIMIT,
            "recipe": s.ops_log,
            "mask": stats,
        });
        if q.get("include_image").map(|v| v == "1" || v == "true").unwrap_or(false) {
            let size: u32 = q
                .get("preview_size")
                .and_then(|v| v.parse().ok())
                .unwrap_or(512)
                .clamp(64, 1024);
            let show_example = q
                .get("example")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false);
            let example_scale: u32 = q
                .get("example_scale")
                .and_then(|v| v.parse().ok())
                .unwrap_or(2)
                .clamp(1, 3);
            let example_opacity: f32 = q
                .get("example_opacity")
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.65_f32)
                .clamp(0.1, 1.0);
            let default_sm = if show_example { "2" } else { "1" };
            let scale_mode = q.get("scale_mode").map(|s| s.as_str()).unwrap_or(default_sm);
            let guides = q
                .get("guides")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false);
            if let Ok((mut img, used)) = compose(&s, size, scale_mode, true) {
                drop(s);
                if show_example {
                    draw_example_overlay(&mut img, used, example_scale, example_opacity);
                }
                if guides {
                    draw_preview_guides(&mut img, used);
                }
                if let Ok((bytes, _)) = crate::imaging::save_image(
                    &image::DynamicImage::ImageRgba8(img),
                    "webp",
                    90,
                ) {
                    let data_url = format!(
                        "data:image/webp;base64,{}",
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
                    );
                    v["image_data_url"] = json!(data_url);
                }
            }
        }
        ok_json(v)
    })
    .await
}

pub async fn op_export(
    State(_state): State<std::sync::Arc<AppState>>,
    Path(sid): Path<String>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    run_blocking(move || {
        let Some(arc) = store_get(&sid) else {
            return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
        };
        let format = v["format"].as_str().unwrap_or("png").to_lowercase();
        if !["webp", "png", "jpg"].contains(&format.as_str()) {
            return bad("bad_request", "format must be webp|png|jpg");
        }
        let quality = v["quality"].as_u64().unwrap_or(95).clamp(10, 100) as u8;
        let size = v["size"].as_u64().unwrap_or(1024).clamp(256, 2048) as u32;
        let scale_mode = v["scale_mode"].as_str().unwrap_or("1").to_string();
        let with_ring = v["with_ring"].as_bool().unwrap_or(true);

        let composed = {
            let s = arc.lock().unwrap();
            compose(&s, size, &scale_mode, with_ring)
        };
        match composed {
            Ok((img, _used)) => match crate::imaging::save_image(
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
    })
    .await
}
