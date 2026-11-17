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
const MAX_BASE_DIM: u32 = 2048;
const MAX_HISTORY: usize = 30;
const DEFAULT_PREVIEW_SIZE: u32 = 768;
const PREVIEW_WEBP_QUALITY: u8 = 92;

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
    /// Monotonic revision counters for Live GUI Sync.
    pub rev: u64,
    pub img_rev: u64,
    pub canvas_mask_rev: u64,
    pub image_mask_rev: u64,
    pub live_sync: bool,
    pub source_label: Option<String>,
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
    if find_ring_file("rune-steel").is_some() {
        return Some("rune-steel".to_string());
    }
    let rings = list_dir_images(&paths::builtin_ring_dir(), true);
    rings
        .first()
        .and_then(|r| r["name"].as_str())
        .map(|s| s.to_string())
}

fn store_create(
    base: RgbaImage,
    ring_override: Option<Option<String>>,
    live_sync: bool,
    source_label: Option<String>,
) -> Arc<Mutex<AgentSession>> {
    let max_sessions = crate::config::max_agent_sessions();
    with_store(|v| {
        while v.len() >= max_sessions {
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
            rev: 1,
            img_rev: 1,
            canvas_mask_rev: 1,
            image_mask_rev: 1,
            live_sync,
            source_label,
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

fn notify_gui_sync(sid: &str, rev: u64, force: bool) {
    if crate::shell::has_window()
        && let Ok(sid_json) = serde_json::to_string(sid)
    {
        let script = format!(
            "if(window.TokenAgentSync)window.TokenAgentSync.onServerPush({sid_json},{rev},{force});"
        );
        crate::gui_control::send(crate::gui_control::GuiCommand::EvalJs(script));
    }
}

fn notify_gui_closed(sid: &str) {
    if crate::shell::has_window()
        && let Ok(sid_json) = serde_json::to_string(sid)
    {
        let script = format!(
            "if(window.TokenAgentSync)window.TokenAgentSync.onSessionClosed({sid_json});"
        );
        crate::gui_control::send(crate::gui_control::GuiCommand::EvalJs(script));
    }
}

fn notify_gui_detached() {
    if crate::shell::has_window() {
        crate::gui_control::send(crate::gui_control::GuiCommand::EvalJs(
            "if(window.TokenAgentSync)window.TokenAgentSync.onSessionDetached();".to_string(),
        ));
    }
}

fn source_label_from_json(v: &Value) -> Option<String> {
    v.get("image_path")
        .or_else(|| v.get("path"))
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|p| {
            std::path::Path::new(p)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
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
                let ext = super::ext_of(&p.to_string_lossy());
                if !matches!(ext.as_str(), "webp" | "png" | "jpg" | "jpeg") {
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
                if fstem.ends_with(".mask") || fstem.ends_with(".top") {
                    continue;
                }
                let localized = builtin_ring_name(&fname).map(|s| s.to_lowercase());
                let meta = crate::foundry_rings::read_ring_meta(&p);
                let meta_name = meta.as_ref().and_then(|m| m.name.as_deref()).map(|s| s.to_lowercase());
                let meta_name_en = meta.as_ref().and_then(|m| m.name_en.as_deref()).map(|s| s.to_lowercase());
                if fstem == stem
                    || fname == stem
                    || localized.as_deref() == Some(stem.as_str())
                    || meta_name.as_deref() == Some(stem.as_str())
                    || meta_name_en.as_deref() == Some(stem.as_str())
                {
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

fn load_ring_assets(ring_name: &str) -> (Option<RgbaImage>, Option<RgbaImage>, f32) {
    let Some(rp) = find_ring_file(ring_name) else {
        return (None, None, 1.0);
    };
    let ring_img = image::open(&rp).ok().map(|i| i.to_rgba8());
    let meta = crate::foundry_rings::read_ring_meta(&rp);
    let scale = meta
        .as_ref()
        .and_then(|m| m.scale)
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0);
    let top_path = meta
        .as_ref()
        .and_then(|m| m.top_file.as_ref())
        .map(|tf| rp.with_file_name(tf))
        .filter(|p| p.is_file())
        .or_else(|| crate::foundry_rings::find_companion_file(&rp, "top"));
    let top_img = top_path.and_then(|tp| image::open(&tp).ok().map(|i| i.to_rgba8()));
    (ring_img, top_img, scale)
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

/// Builds a 1024x1024 anti-aliased protection mask in 1x ring space where 255 = fully protected,
/// 1..254 = smooth inner-rim edge transition, and 0 = unprotected outside the ring.
fn load_protection_mask_1024(ring_name: Option<&str>) -> Option<GrayImage> {
    let path = find_protection_mask_file(ring_name)?;
    let bytes = std::fs::read(&path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let resized = image::imageops::resize(
        &img,
        LOGICAL_U32,
        LOGICAL_U32,
        image::imageops::FilterType::Triangle,
    );
    let mut prot = GrayImage::new(LOGICAL_U32, LOGICAL_U32);
    for (x, y, p) in resized.enumerate_pixels() {
        prot.put_pixel(x, y, Luma([p.0[3]]));
    }
    Some(image::imageops::blur(&prot, 0.55))
}

fn sample_gray_bilinear(img: &GrayImage, x: f32, y: f32) -> u8 {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
        return 0;
    }
    let x0 = (x.floor() as u32).min(w - 1);
    let y0 = (y.floor() as u32).min(h - 1);
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = (x - x0 as f32).clamp(0.0, 1.0);
    let fy = (y - y0 as f32).clamp(0.0, 1.0);
    let p00 = img.get_pixel(x0, y0).0[0] as f32;
    let p10 = img.get_pixel(x1, y0).0[0] as f32;
    let p01 = img.get_pixel(x0, y1).0[0] as f32;
    let p11 = img.get_pixel(x1, y1).0[0] as f32;
    let top = p00 + (p10 - p00) * fx;
    let bot = p01 + (p11 - p01) * fx;
    (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8
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

/// Processes a preset image with smooth anti-aliased edges (`TokenPresets.processMaskImage`):
/// Dark/purple opaque pixels mark the CUTOUT region (removed -> mask = 0), while transparent
/// or pure-white pixels mark the KEPT region (mask = 255). Preserves continuous edge alpha
/// instead of hard-binarizing, and applies a subpixel blur so binary 6144x6144 presets have
/// clean anti-aliased rims without stair-stepping.
fn load_preset_mask(path: &std::path::Path, target_w: u32, target_h: u32) -> Result<GrayImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let resized = image::imageops::resize(
        &img,
        target_w,
        target_h,
        image::imageops::FilterType::Triangle,
    );
    let mut out = GrayImage::new(target_w, target_h);
    for (x, y, p) in resized.enumerate_pixels() {
        let [r, g, b, a] = p.0;
        let alpha_w = a as f32 / 255.0;
        let brightness = (r as f32 + g as f32 + b as f32) / 3.0;
        let dark_w = if brightness >= 245.0 {
            0.0
        } else if brightness <= 195.0 {
            1.0
        } else {
            (245.0 - brightness) / 50.0
        };
        let cut = (alpha_w * dark_w).clamp(0.0, 1.0);
        let mask_val = ((1.0 - cut) * 255.0).round() as u8;
        out.put_pixel(x, y, Luma([mask_val]));
    }
    Ok(image::imageops::blur(&out, 0.55))
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
            if stem.to_lowercase().ends_with(".mask") || stem.to_lowercase().ends_with(".top") {
                continue;
            }
            let fname = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let meta = crate::foundry_rings::read_ring_meta(&p);
            let display_name = builtin_ring_name(&fname.to_lowercase())
                .map(String::from)
                .or_else(|| meta.as_ref().and_then(|m| m.name.clone()))
                .unwrap_or_else(|| stem.clone());
            let top_file = meta
                .as_ref()
                .and_then(|m| m.top_file.clone())
                .filter(|tf| p.with_file_name(tf).is_file())
                .or_else(|| {
                    crate::foundry_rings::find_companion_file(&p, "top")
                        .and_then(|tp| tp.file_name().and_then(|n| n.to_str()).map(String::from))
                });
            let scale = meta
                .as_ref()
                .and_then(|m| m.scale)
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(1.0);
            out.push(json!({
                "name": stem,
                "display_name": display_name,
                "file": fname,
                "builtin": builtin,
                "top_file": top_file,
                "scale": scale
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

fn parse_point_pair(p: &Value, sx: f64, sy: f64, ox: f64, oy: f64) -> Option<(f32, f32)> {
    if let Some(arr) = p.as_array() {
        if arr.len() >= 2 {
            let x = arr[0].as_f64()?;
            let y = arr[1].as_f64()?;
            if x.is_finite() && y.is_finite() {
                return Some(((x * sx + ox) as f32, (y * sy + oy) as f32));
            }
        }
        return None;
    }
    if let Some(obj) = p.as_object() {
        let x = obj.get("x").and_then(|v| v.as_f64())?;
        let y = obj.get("y").and_then(|v| v.as_f64())?;
        if x.is_finite() && y.is_finite() {
            return Some(((x * sx + ox) as f32, (y * sy + oy) as f32));
        }
    }
    None
}

/// Fills a polygon with 4x vertical subpixel scanline anti-aliasing, plus optional soft `feather` border.
fn fill_polygon_aa(mask: &mut GrayImage, pts: &[(f32, f32)], erase: bool, feather: f32) {
    if pts.len() < 3 {
        return;
    }
    let (mw, mh) = mask.dimensions();
    if mw == 0 || mh == 0 {
        return;
    }
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for &(_, py) in pts {
        min_y = min_y.min(py);
        max_y = max_y.max(py);
    }
    let y_start = min_y.floor().max(0.0) as i64;
    let y_end = max_y.ceil().min((mh - 1) as f32) as i64;
    if y_start <= y_end {
        let mut nodes: Vec<f32> = Vec::with_capacity(pts.len());
        let mut row_cov = vec![0u8; mw as usize];
        for y in y_start..=y_end {
            let mut min_xi = mw as usize;
            let mut max_xi = 0usize;
            for sub in 0..4 {
                let yf = y as f32 + (sub as f32 + 0.5) * 0.25;
                nodes.clear();
                let mut j = pts.len() - 1;
                for i in 0..pts.len() {
                    let (xi, yi) = pts[i];
                    let (xj, yj) = pts[j];
                    if (yi <= yf && yj > yf) || (yj <= yf && yi > yf) {
                        let t = (yf - yi) / (yj - yi);
                        nodes.push(xi + t * (xj - xi));
                    }
                    j = i;
                }
                nodes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let mut idx = 0;
                while idx + 1 < nodes.len() {
                    let x0 = nodes[idx];
                    let x1 = nodes[idx + 1];
                    idx += 2;
                    let xa = x0.floor().max(0.0) as usize;
                    let xb = x1.ceil().min(mw as f32).min(row_cov.len() as f32) as usize;
                    if xa < xb {
                        min_xi = min_xi.min(xa);
                        max_xi = max_xi.max(xb);
                        for (x, cov) in row_cov[xa..xb].iter_mut().enumerate() {
                            let xc = (xa + x) as f32 + 0.5;
                            if xc >= x0 && xc <= x1 {
                                *cov += 1;
                            }
                        }
                    }
                }
            }
            let end_xi = max_xi.min(mw as usize).min(row_cov.len());
            if min_xi < end_xi {
                for (dx, cov) in row_cov[min_xi..end_xi].iter_mut().enumerate() {
                    let c = *cov;
                    if c > 0 {
                        *cov = 0;
                        let x = (min_xi + dx) as u32;
                        let strength = c as f32 * 0.25;
                        let cur = mask.get_pixel(x, y as u32).0[0] as f32;
                        let next = if erase {
                            cur * (1.0 - strength)
                        } else {
                            cur + (255.0 - cur) * strength
                        };
                        mask.put_pixel(x, y as u32, Luma([next.round().clamp(0.0, 255.0) as u8]));
                    }
                }
            }
        }
    }

    if feather > 0.5 {
        let step_len = (feather * 0.35).max(1.0);
        for i in 0..pts.len() {
            let (ax, ay) = pts[i];
            let (bx, by) = pts[(i + 1) % pts.len()];
            let dist = (bx - ax).hypot(by - ay);
            let steps = (dist / step_len).ceil().max(1.0) as i64;
            for t in 0..=steps {
                let tt = t as f32 / steps as f32;
                stamp_radial_brush(
                    mask,
                    ax + (bx - ax) * tt,
                    ay + (by - ay) * tt,
                    feather,
                    erase,
                    0.2,
                );
            }
        }
    }
}

/// Applies shapes (`circle`, `rect`, `stroke`, `polygon`) to `mask` with strict validation.
/// Accepts BOTH externally tagged (`{"rect": {"x0":..}}`) and internally tagged (`{"type": "rect", "x0":..}`) objects.
/// If `full_s3` is true, `mask` is `3*LOGICAL x 3*LOGICAL` (3072x3072) and 1x logical
/// coordinates `(0..1024)` map to `(1024..2048)` so that shapes can reach `(-1024..2048)`.
/// If `full_s3` is false, `mask` is `image_mask` (`mw x mh`) and `(0..1024)` maps across `0..mw, 0..mh`.
fn mask_apply_shapes(
    mask: &mut GrayImage,
    shapes: &Value,
    erase: bool,
    full_s3: bool,
) -> Result<usize, String> {
    let Some(arr) = shapes.as_array() else {
        return Err("'shapes' must be a JSON array".into());
    };
    if arr.is_empty() {
        return Err("'shapes' array must not be empty".into());
    }

    let (mw, mh) = mask.dimensions();
    let (sx, sy, ox, oy) = if full_s3 {
        let s = (mw as f64) / (LOGICAL as f64 * 3.0);
        (s, s, LOGICAL as f64 * s, LOGICAL as f64 * s)
    } else {
        (mw as f64 / LOGICAL as f64, mh as f64 / LOGICAL as f64, 0.0, 0.0)
    };
    let sr = (sx + sy) * 0.5;
    let value = if erase { 0u8 } else { 255u8 };

    // Validate all shapes up front so a malformed item never leaves a half-mutated mask.
    enum ParsedShape {
        Circle { cx: f32, cy: f32, r: f32, feather: f32 },
        Rect { x0: i64, y0: i64, x1: i64, y1: i64, feather: f32 },
        Stroke { pts: Vec<(f32, f32)>, radius: f32, hardness: f32 },
        Polygon { pts: Vec<(f32, f32)>, feather: f32 },
    }

    let mut parsed: Vec<ParsedShape> = Vec::with_capacity(arr.len());

    for (idx, sh) in arr.iter().enumerate() {
        let Some(obj) = sh.as_object() else {
            return Err(format!("shapes[{idx}] must be a JSON object"));
        };

        let (kind, payload): (&str, &Value) = if let Some(v) = obj.get("circle") {
            ("circle", v)
        } else if let Some(v) = obj.get("rect") {
            ("rect", v)
        } else if let Some(v) = obj.get("stroke") {
            ("stroke", v)
        } else if let Some(v) = obj.get("polygon").or_else(|| obj.get("poly")) {
            ("polygon", v)
        } else if let Some(t) = obj
            .get("type")
            .or_else(|| obj.get("shape"))
            .or_else(|| obj.get("kind"))
            .and_then(|v| v.as_str())
        {
            (t, sh)
        } else {
            return Err(format!(
                "shapes[{idx}] unrecognized shape format: expected key 'circle'|'rect'|'stroke'|'polygon' or '\"type\": \"circle\"|\"rect\"|\"stroke\"|\"polygon\"'"
            ));
        };

        match kind.trim().to_lowercase().as_str() {
            "circle" => {
                let cx_raw = payload
                    .get("cx")
                    .or_else(|| payload.get("x"))
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| format!("shapes[{idx}] (circle) requires numeric 'cx'"))?;
                let cy_raw = payload
                    .get("cy")
                    .or_else(|| payload.get("y"))
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| format!("shapes[{idx}] (circle) requires numeric 'cy'"))?;
                let r_raw = payload
                    .get("r")
                    .or_else(|| payload.get("radius"))
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| format!("shapes[{idx}] (circle) requires numeric 'r'"))?;
                if !cx_raw.is_finite() || !cy_raw.is_finite() || !r_raw.is_finite() || r_raw <= 0.0 {
                    return Err(format!("shapes[{idx}] (circle) requires finite cx, cy and r > 0"));
                }
                let cx = (cx_raw * sx + ox) as f32;
                let cy = (cy_raw * sy + oy) as f32;
                let r = (r_raw * sr) as f32;
                let feather = (payload.get("feather").and_then(|v| v.as_f64()).unwrap_or(0.0) * sr)
                    .max(0.0) as f32;
                parsed.push(ParsedShape::Circle { cx, cy, r, feather });
            }
            "rect" | "rectangle" | "box" => {
                let (rx0, ry0, rx1, ry1) = if let (Some(x0), Some(y0), Some(x1), Some(y1)) = (
                    payload.get("x0").and_then(|v| v.as_f64()),
                    payload.get("y0").and_then(|v| v.as_f64()),
                    payload.get("x1").and_then(|v| v.as_f64()),
                    payload.get("y1").and_then(|v| v.as_f64()),
                ) {
                    (x0, y0, x1, y1)
                } else if let (Some(x), Some(y), Some(w), Some(h)) = (
                    payload.get("x").and_then(|v| v.as_f64()),
                    payload.get("y").and_then(|v| v.as_f64()),
                    payload
                        .get("width")
                        .or_else(|| payload.get("w"))
                        .and_then(|v| v.as_f64()),
                    payload
                        .get("height")
                        .or_else(|| payload.get("h"))
                        .and_then(|v| v.as_f64()),
                ) {
                    (x, y, x + w, y + h)
                } else {
                    return Err(format!(
                        "shapes[{idx}] (rect) requires numeric 'x0','y0','x1','y1' (or 'x','y','width','height')"
                    ));
                };
                if !rx0.is_finite() || !ry0.is_finite() || !rx1.is_finite() || !ry1.is_finite() {
                    return Err(format!("shapes[{idx}] (rect) coordinates must be finite"));
                }
                let x0 = (rx0 * sx + ox).round() as i64;
                let y0 = (ry0 * sy + oy).round() as i64;
                let x1 = (rx1 * sx + ox).round() as i64;
                let y1 = (ry1 * sy + oy).round() as i64;
                if x0 == x1 || y0 == y1 {
                    return Err(format!("shapes[{idx}] (rect) has zero width or height"));
                }
                let feather = (payload.get("feather").and_then(|v| v.as_f64()).unwrap_or(0.0) * sr)
                    .max(0.0) as f32;
                parsed.push(ParsedShape::Rect {
                    x0: x0.min(x1),
                    y0: y0.min(y1),
                    x1: x0.max(x1),
                    y1: y0.max(y1),
                    feather,
                });
            }
            "stroke" | "line" => {
                let pts_arr = payload
                    .get("points")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| format!("shapes[{idx}] (stroke) requires 'points' array"))?;
                if pts_arr.is_empty() {
                    return Err(format!("shapes[{idx}] (stroke) 'points' must not be empty"));
                }
                let mut pts = Vec::with_capacity(pts_arr.len());
                for (pi, p) in pts_arr.iter().enumerate() {
                    let pt = parse_point_pair(p, sx, sy, ox, oy).ok_or_else(|| {
                        format!("shapes[{idx}] (stroke) invalid point at points[{pi}], expected [x, y]")
                    })?;
                    pts.push(pt);
                }
                let width_raw = payload.get("width").and_then(|v| v.as_f64()).unwrap_or(30.0);
                if !width_raw.is_finite() || width_raw <= 0.0 {
                    return Err(format!("shapes[{idx}] (stroke) 'width' must be > 0"));
                }
                let radius = ((width_raw * sr).max(1.0) * 0.5) as f32;
                let soft = payload.get("soft").and_then(|v| v.as_bool()).unwrap_or(true);
                let hardness = if soft { 0.7 } else { 0.99 };
                parsed.push(ParsedShape::Stroke {
                    pts,
                    radius,
                    hardness,
                });
            }
            "polygon" | "poly" => {
                let pts_arr = payload
                    .get("points")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| format!("shapes[{idx}] (polygon) requires 'points' array"))?;
                if pts_arr.len() < 3 {
                    return Err(format!(
                        "shapes[{idx}] (polygon) requires at least 3 points, got {}",
                        pts_arr.len()
                    ));
                }
                let mut pts = Vec::with_capacity(pts_arr.len());
                for (pi, p) in pts_arr.iter().enumerate() {
                    let pt = parse_point_pair(p, sx, sy, ox, oy).ok_or_else(|| {
                        format!("shapes[{idx}] (polygon) invalid point at points[{pi}], expected [x, y]")
                    })?;
                    pts.push(pt);
                }
                let feather = (payload.get("feather").and_then(|v| v.as_f64()).unwrap_or(0.0) * sr)
                    .max(0.0) as f32;
                parsed.push(ParsedShape::Polygon { pts, feather });
            }
            other => {
                return Err(format!(
                    "shapes[{idx}] unknown shape type {other:?}; supported: circle, rect, stroke, polygon"
                ));
            }
        }
    }

    let count = parsed.len();
    for item in parsed {
        match item {
            ParsedShape::Circle { cx, cy, r, feather } => {
                let eff_f = feather.max(1.0);
                let hard = ((r - eff_f).max(0.0) / r.max(1.0)).clamp(0.0, 0.99);
                stamp_radial_brush(mask, cx, cy, r, erase, hard);
            }
            ParsedShape::Rect {
                x0,
                y0,
                x1,
                y1,
                feather,
            } => {
                if feather > 0.5 {
                    let pts = vec![
                        (x0 as f32, y0 as f32),
                        (x1 as f32, y0 as f32),
                        (x1 as f32, y1 as f32),
                        (x0 as f32, y1 as f32),
                    ];
                    fill_polygon_aa(mask, &pts, erase, feather);
                } else {
                    let xa = x0.max(0) as u32;
                    let ya = y0.max(0) as u32;
                    let xb = (x1.max(0) as u32).min(mw);
                    let yb = (y1.max(0) as u32).min(mh);
                    for y in ya..yb {
                        for x in xa..xb {
                            mask.put_pixel(x, y, Luma([value]));
                        }
                    }
                }
            }
            ParsedShape::Stroke {
                pts,
                radius,
                hardness,
            } => {
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
            ParsedShape::Polygon { pts, feather } => {
                fill_polygon_aa(mask, &pts, erase, feather);
            }
        }
    }

    Ok(count)
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
fn render_unmasked_subject_on_s3(session: &AgentSession, size: u32) -> RgbaImage {
    let s3 = size * 3;
    let base = build_composited_subject(session);
    let (bw, bh) = base.dimensions();
    let max_dim = bw.max(bh).max(1) as f32;

    // Fit into 1x frame (`size` px) when scale == 1.0, preserving aspect ratio.
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

    let px_per_logical = size as f32 / LOGICAL;
    let cx = (s3 as f32) * 0.5 + session.offset_x * px_per_logical;
    let cy = (s3 as f32) * 0.5 + session.offset_y * px_per_logical;
    let px0 = (cx - subj_rotated.width() as f32 * 0.5).round() as i64;
    let py0 = (cy - subj_rotated.height() as f32 * 0.5).round() as i64;

    let mut layer = RgbaImage::new(s3, s3);
    image::imageops::overlay(&mut layer, &subj_rotated, px0, py0);
    layer
}

fn apply_canvas_mask_on_s3(session: &AgentSession, unmasked: &RgbaImage, size: u32) -> RgbaImage {
    let s3 = size * 3;
    let mut layer = unmasked.clone();
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

fn render_subject_on_s3(session: &AgentSession, size: u32) -> RgbaImage {
    let unmasked = render_unmasked_subject_on_s3(session, size);
    apply_canvas_mask_on_s3(session, &unmasked, size)
}

fn compose_foreground_s3(
    session: &AgentSession,
    size: u32,
    with_ring: bool,
    xray: bool,
) -> RgbaImage {
    let s3 = size * 3;
    let mut canvas = RgbaImage::new(s3, s3);

    let (ring_opt, top_opt, ring_scale) = if with_ring
        && let Some(rname) = &session.ring
    {
        load_ring_assets(rname)
    } else {
        (None, None, 1.0)
    };
    let ring_draw_size = ((size as f32 * ring_scale).round().max(1.0)) as u32;
    let ring_off = ((s3 as i64) - (ring_draw_size as i64)) / 2;

    if let Some(ring) = ring_opt {
        let resized = image::imageops::resize(
            &ring,
            ring_draw_size,
            ring_draw_size,
            image::imageops::FilterType::Lanczos3,
        );
        image::imageops::overlay(&mut canvas, &resized, ring_off, ring_off);
    }

    if xray {
        let unmasked = render_unmasked_subject_on_s3(session, size);
        let masked = apply_canvas_mask_on_s3(session, &unmasked, size);

        // Hot-pink ghost layer wherever `canvas_mask` attenuated or removed subject pixels
        let mut ghost = RgbaImage::new(s3, s3);
        for (gp, (up, mp)) in ghost.pixels_mut().zip(unmasked.pixels().zip(masked.pixels())) {
            let ua = up.0[3];
            let ma = mp.0[3];
            if ua > ma {
                let cut_a = ua - ma;
                let ga = ((cut_a as f32) * 0.62).round().clamp(0.0, 255.0) as u8;
                let gr = (up.0[0] as f32 * 0.28 + 255.0 * 0.72).round().clamp(0.0, 255.0) as u8;
                let gg = (up.0[1] as f32 * 0.28 + 65.0 * 0.72).round().clamp(0.0, 255.0) as u8;
                let gb = (up.0[2] as f32 * 0.28 + 145.0 * 0.72).round().clamp(0.0, 255.0) as u8;
                *gp = Rgba([gr, gg, gb, ga]);
            }
        }
        image::imageops::overlay(&mut canvas, &ghost, 0, 0);
        image::imageops::overlay(&mut canvas, &masked, 0, 0);
    } else {
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
    }

    if let Some(top_img) = top_opt {
        let resized_top = image::imageops::resize(
            &top_img,
            ring_draw_size,
            ring_draw_size,
            image::imageops::FilterType::Lanczos3,
        );
        image::imageops::overlay(&mut canvas, &resized_top, ring_off, ring_off);
    }

    canvas
}

fn compose(
    session: &AgentSession,
    size: u32,
    scale_mode: &str,
    with_ring: bool,
) -> Result<(RgbaImage, u32), String> {
    let s3 = size * 3;
    let canvas = compose_foreground_s3(session, size, with_ring, false);

    let used = match scale_mode {
        "1" => 1,
        "2" => 2,
        "3" => 3,
        _ => {
            let scan = 512u32;
            let small = image::imageops::resize(
                &canvas,
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
    let crop = (s3 - final_size) / 2;
    let out = image::imageops::crop_imm(&canvas, crop, crop, final_size, final_size).to_image();
    Ok((out, used))
}

fn fill_teal_checkerboard(w: u32, h: u32, tile: u32) -> RgbaImage {
    let tile = tile.max(8);
    let mut out = RgbaImage::new(w, h);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let dark = (((x / tile) + (y / tile)) & 1) == 0;
        *p = if dark {
            Rgba([17, 45, 43, 255])
        } else {
            Rgba([25, 63, 60, 255])
        };
    }
    out
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

fn view_bounds_for_scale(used_scale: u32) -> (f32, f32, f32, f32) {
    let used = used_scale.clamp(1, 3) as f32;
    let l_min = -((used - 1.0) * 512.0);
    let l_max = 1024.0 + (used - 1.0) * 512.0;
    (l_min, l_min, l_max, l_max)
}

fn draw_example_overlay_in_view(
    img: &mut RgbaImage,
    lx0: f32,
    ly0: f32,
    lx1: f32,
    ly1: f32,
    example_scale: u32,
    opacity: f32,
) {
    let Some(ex) = load_example_image() else {
        return;
    };
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let ex_m = example_scale.clamp(1, 3) as f32;
    let span_x = (lx1 - lx0).max(1.0);
    let span_y = (ly1 - ly0).max(1.0);
    let ex_log_min = 512.0 - 512.0 * ex_m;
    let ex_log_span = 1024.0 * ex_m;

    let ox = ((ex_log_min - lx0) / span_x * w as f32).round() as i64;
    let oy = ((ex_log_min - ly0) / span_y * h as f32).round() as i64;
    let ex_w = ((ex_log_span / span_x) * w as f32).round().max(1.0) as u32;
    let ex_h = ((ex_log_span / span_y) * h as f32).round().max(1.0) as u32;
    if ex_w > 6144 || ex_h > 6144 {
        return;
    }
    let ex_resized = image::imageops::resize(&ex, ex_w, ex_h, image::imageops::FilterType::Lanczos3);
    let alpha_mul = opacity.clamp(0.0, 1.0);

    let x_start = (-ox).max(0) as u32;
    let y_start = (-oy).max(0) as u32;
    let x_end = ((w as i64 - ox).min(ex_w as i64)).max(0) as u32;
    let y_end = ((h as i64 - oy).min(ex_h as i64)).max(0) as u32;

    for ex_y in y_start..y_end {
        let dy = (oy + ex_y as i64) as u32;
        for ex_x in x_start..x_end {
            let src_px = ex_resized.get_pixel(ex_x, ex_y);
            if src_px.0[3] == 0 {
                continue;
            }
            let dx = (ox + ex_x as i64) as u32;
            let dst = img.get_pixel_mut(dx, dy);
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

fn draw_example_overlay(img: &mut RgbaImage, used_scale: u32, example_scale: u32, opacity: f32) {
    let (lx0, ly0, lx1, ly1) = view_bounds_for_scale(used_scale);
    draw_example_overlay_in_view(img, lx0, ly0, lx1, ly1, example_scale, opacity);
}

fn blend_rgba_px(img: &mut RgbaImage, x: i64, y: i64, color: [u8; 4]) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        let dst = img.get_pixel_mut(x as u32, y as u32);
        let sa = color[3] as f32 / 255.0;
        let da = dst.0[3] as f32 / 255.0;
        let out_a = sa + da * (1.0 - sa);
        if out_a > 0.0 {
            dst.0[0] = ((color[0] as f32 * sa + dst.0[0] as f32 * da * (1.0 - sa)) / out_a)
                .round()
                .clamp(0.0, 255.0) as u8;
            dst.0[1] = ((color[1] as f32 * sa + dst.0[1] as f32 * da * (1.0 - sa)) / out_a)
                .round()
                .clamp(0.0, 255.0) as u8;
            dst.0[2] = ((color[2] as f32 * sa + dst.0[2] as f32 * da * (1.0 - sa)) / out_a)
                .round()
                .clamp(0.0, 255.0) as u8;
            dst.0[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn blend_rgba_thick(img: &mut RgbaImage, x: i64, y: i64, thickness: i64, color: [u8; 4]) {
    let t = thickness.max(1);
    for dy in 0..t {
        for dx in 0..t {
            blend_rgba_px(img, x + dx, y + dy, color);
        }
    }
}

/// Draws visual alignment guides on a preview view `[lx0, ly0, lx1, ly1]` in 1x ring coords.
fn draw_preview_guides_in_view(img: &mut RgbaImage, lx0: f32, ly0: f32, lx1: f32, ly1: f32) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let span_x = (lx1 - lx0).max(1.0);
    let span_y = (ly1 - ly0).max(1.0);
    let map_x = |lx: f32| -> i64 { ((lx - lx0) / span_x * w as f32).round() as i64 };
    let map_y = |ly: f32| -> i64 { ((ly - ly0) / span_y * h as f32).round() as i64 };
    let thick = if w.min(h) >= 700 { 2 } else { 1 };

    let cx = map_x(512.0);
    for y in 0..h as i64 {
        if (y / 8) % 2 == 0 {
            blend_rgba_thick(img, cx, y, thick, [0, 220, 255, 195]);
        }
    }

    let crown_y = map_y(-76.0);
    for x in 0..w as i64 {
        if (x / 8) % 2 == 0 {
            blend_rgba_thick(img, x, crown_y, thick, [200, 0, 255, 220]);
        }
    }

    let jaw_y = map_y(199.0);
    for x in 0..w as i64 {
        if (x / 8) % 2 == 0 {
            blend_rgba_thick(img, x, jaw_y, thick, [200, 0, 255, 220]);
        }
    }

    let steps = 360;
    for i in 0..steps {
        if (i / 3) % 2 == 0 {
            let ang = (i as f32 / steps as f32) * std::f32::consts::TAU;
            let ox = map_x(535.0 + 181.0 * ang.cos());
            let oy = map_y(62.0 + 137.5 * ang.sin());
            blend_rgba_thick(img, ox, oy, thick, [200, 0, 255, 210]);
        }
    }

    let belt_y = map_y(716.0);
    for x in 0..w as i64 {
        if (x / 8) % 2 == 0 {
            blend_rgba_thick(img, x, belt_y, thick, [255, 140, 0, 220]);
        }
    }
}

fn draw_preview_guides(img: &mut RgbaImage, used_scale: u32) {
    let (lx0, ly0, lx1, ly1) = view_bounds_for_scale(used_scale);
    draw_preview_guides_in_view(img, lx0, ly0, lx1, ly1);
}

/// Composes a mask X-Ray diagnostic view:
/// - Dark teal checkerboard background (`#112d2b` / `#193f3c`) so both dark cloaks and bright glows stand out
/// - 1x token ring (`with_ring`)
/// - Semi-transparent hot-pink ghost (`[255, 65, 145]`) showing everything `canvas_mask` cut away from the transformed subject
/// - Final masked subject rendered at 100% opacity on top
/// - Reference geometry markers: cyan inner protection circle (`r_in = 343`), orange outer ring rim (`r_out = 468`),
///   and yellow `preset1` horizontal cut line (`y = 564` outside `r_in`).
fn compose_xray(
    session: &AgentSession,
    size: u32,
    scale_mode: &str,
    with_ring: bool,
) -> Result<(RgbaImage, u32), String> {
    let s3 = size * 3;
    let canvas = compose_foreground_s3(session, size, with_ring, true);

    let used = match scale_mode {
        "1" => 1,
        "2" => 2,
        "3" => 3,
        _ => 2,
    };
    let final_size = size * used;
    let crop = (s3 - final_size) / 2;
    let fg = image::imageops::crop_imm(&canvas, crop, crop, final_size, final_size).to_image();

    let mut out = fill_teal_checkerboard(final_size, final_size, final_size / 32);
    image::imageops::overlay(&mut out, &fg, 0, 0);
    draw_xray_markers(&mut out, used);
    Ok((out, used))
}

fn draw_xray_markers_in_view(img: &mut RgbaImage, lx0: f32, ly0: f32, lx1: f32, ly1: f32) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let span_x = (lx1 - lx0).max(1.0);
    let span_y = (ly1 - ly0).max(1.0);
    let map_x = |lx: f32| -> i64 { ((lx - lx0) / span_x * w as f32).round() as i64 };
    let map_y = |ly: f32| -> i64 { ((ly - ly0) / span_y * h as f32).round() as i64 };
    let thick = if w.min(h) >= 700 { 2 } else { 1 };

    // Inner protection circle r_in = 343 (cyan dashed) & outer ring rim r_out = 468 (coral dashed)
    let steps = 720;
    for i in 0..steps {
        if (i / 4) % 2 == 0 {
            let ang = (i as f32 / steps as f32) * std::f32::consts::TAU;
            let ix = map_x(512.0 + 343.0 * ang.cos());
            let iy = map_y(512.0 + 343.0 * ang.sin());
            blend_rgba_thick(img, ix, iy, thick, [0, 235, 255, 225]);

            let ox = map_x(512.0 + 468.0 * ang.cos());
            let oy = map_y(512.0 + 468.0 * ang.sin());
            blend_rgba_thick(img, ox, oy, thick, [255, 125, 60, 195]);
        }
    }

    // Horizontal preset1 cut threshold y = 564 outside the inner protection circle (yellow dashed)
    let y564 = map_y(564.0);
    let x_left_end = map_x(512.0 - 338.0);
    let x_right_start = map_x(512.0 + 338.0);
    for x in 0..w as i64 {
        if (x <= x_left_end || x >= x_right_start) && (x / 7) % 2 == 0 {
            blend_rgba_thick(img, x, y564, thick, [255, 225, 55, 225]);
        }
    }
}

fn draw_xray_markers(img: &mut RgbaImage, used_scale: u32) {
    let (lx0, ly0, lx1, ly1) = view_bounds_for_scale(used_scale);
    draw_xray_markers_in_view(img, lx0, ly0, lx1, ly1);
}

fn glyph_5x7(ch: char) -> [u8; 7] {
    match ch {
        ' ' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '#' => [0x0a, 0x0a, 0x1f, 0x0a, 0x1f, 0x0a, 0x0a],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '*' => [0x00, 0x0a, 0x04, 0x1f, 0x04, 0x0a, 0x00],
        '+' => [0x00, 0x04, 0x04, 0x1f, 0x04, 0x04, 0x00],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x08],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x06],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        ':' => [0x00, 0x06, 0x06, 0x00, 0x06, 0x06, 0x00],
        '=' => [0x00, 0x00, 0x1f, 0x00, 0x1f, 0x00, 0x00],
        '?' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1c, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1c],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '[' => [0x0e, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0e],
        ']' => [0x0e, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0e],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        'a' => [0x00, 0x00, 0x0e, 0x01, 0x0f, 0x11, 0x0f],
        'b' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1e],
        'c' => [0x00, 0x00, 0x0e, 0x10, 0x10, 0x11, 0x0e],
        'd' => [0x01, 0x01, 0x0d, 0x13, 0x11, 0x11, 0x0f],
        'e' => [0x00, 0x00, 0x0e, 0x11, 0x1f, 0x10, 0x0e],
        'f' => [0x06, 0x09, 0x08, 0x1c, 0x08, 0x08, 0x08],
        'g' => [0x00, 0x00, 0x0f, 0x11, 0x0f, 0x01, 0x0e],
        'h' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11],
        'i' => [0x04, 0x00, 0x0c, 0x04, 0x04, 0x04, 0x0e],
        'j' => [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0c],
        'k' => [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12],
        'l' => [0x0c, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'm' => [0x00, 0x00, 0x1a, 0x15, 0x15, 0x11, 0x11],
        'n' => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0e, 0x11, 0x11, 0x11, 0x0e],
        'p' => [0x00, 0x00, 0x1e, 0x11, 0x1e, 0x10, 0x10],
        'q' => [0x00, 0x00, 0x0d, 0x13, 0x0f, 0x01, 0x01],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        's' => [0x00, 0x00, 0x0e, 0x10, 0x0e, 0x01, 0x1e],
        't' => [0x08, 0x08, 0x1c, 0x08, 0x08, 0x09, 0x06],
        'u' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0d],
        'v' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'w' => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0a],
        'x' => [0x00, 0x00, 0x11, 0x0a, 0x04, 0x0a, 0x11],
        'y' => [0x00, 0x00, 0x11, 0x11, 0x0f, 0x01, 0x0e],
        'z' => [0x00, 0x00, 0x1f, 0x02, 0x04, 0x08, 0x1f],
        '|' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        _ => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
    }
}

fn label_ascii_translit(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
            continue;
        }
        let rep = match ch {
            'а' | 'А' => "a",
            'б' | 'Б' => "b",
            'в' | 'В' => "v",
            'г' | 'Г' => "g",
            'д' | 'Д' => "d",
            'е' | 'Е' => "e",
            'ё' | 'Ё' => "yo",
            'ж' | 'Ж' => "zh",
            'з' | 'З' => "z",
            'и' | 'И' => "i",
            'й' | 'Й' => "y",
            'к' | 'К' => "k",
            'л' | 'Л' => "l",
            'м' | 'М' => "m",
            'н' | 'Н' => "n",
            'о' | 'О' => "o",
            'п' | 'П' => "p",
            'р' | 'Р' => "r",
            'с' | 'С' => "s",
            'т' | 'Т' => "t",
            'у' | 'У' => "u",
            'ф' | 'Ф' => "f",
            'х' | 'Х' => "kh",
            'ц' | 'Ц' => "ts",
            'ч' | 'Ч' => "ch",
            'ш' | 'Ш' => "sh",
            'щ' | 'Щ' => "shch",
            'ъ' | 'Ъ' | 'ь' | 'Ь' => "",
            'ы' | 'Ы' => "y",
            'э' | 'Э' => "e",
            'ю' | 'Ю' => "yu",
            'я' | 'Я' => "ya",
            '—' | '–' => "-",
            ' ' => " ",
            _ => "?",
        };
        if ch.is_uppercase() {
            out.push_str(&rep.to_uppercase());
        } else {
            out.push_str(rep);
        }
    }
    out
}

fn draw_text_5x7(
    img: &mut RgbaImage,
    text: &str,
    x0: i64,
    y0: i64,
    scale: i64,
    fg: [u8; 4],
    bg: Option<[u8; 4]>,
) {
    let ascii = label_ascii_translit(text);
    let chars: Vec<char> = ascii.chars().collect();
    if chars.is_empty() {
        return;
    }
    let s = scale.max(1);
    let text_w = (chars.len() as i64) * 6 * s - s;
    let text_h = 7 * s;
    if let Some(bg_col) = bg {
        let pad_x = (3 * s).min(6);
        let pad_y = (2 * s).min(4);
        for py in (y0 - pad_y)..(y0 + text_h + pad_y) {
            for px in (x0 - pad_x)..(x0 + text_w + pad_x) {
                blend_rgba_px(img, px, py, bg_col);
            }
        }
    }
    for (ci, ch) in chars.iter().enumerate() {
        let gx0 = x0 + (ci as i64) * 6 * s;
        let glyph = glyph_5x7(*ch);
        for (row, &bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if (bits >> (4 - col)) & 1 == 1 {
                    for dy in 0..s {
                        for dx in 0..s {
                            blend_rgba_px(img, gx0 + col * s + dx, y0 + (row as i64) * s + dy, fg);
                        }
                    }
                }
            }
        }
    }
}

/// Draws a `target: "canvas"` logical coordinate grid over any logical viewport `[lx0..lx1, ly0..ly1]`
/// with adaptive minor spacing (`10`, `20`, or `50`, or `step_override`), major 100-unit lines,
/// highlighted `0` / `512` / `1024` axes, and crisp `5x7` coordinate labels on all four edges.
fn draw_canvas_grid_in_view(
    img: &mut RgbaImage,
    lx0: f32,
    ly0: f32,
    lx1: f32,
    ly1: f32,
    step_override: Option<i64>,
) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let span_x = (lx1 - lx0).max(1.0);
    let span_y = (ly1 - ly0).max(1.0);
    let max_span = span_x.max(span_y);

    let map_x = |lx: i64| -> i64 { (((lx as f32 - lx0) / span_x) * w as f32).round() as i64 };
    let map_y = |ly: i64| -> i64 { (((ly as f32 - ly0) / span_y) * h as f32).round() as i64 };

    let step_minor = if let Some(ov) = step_override {
        ov.clamp(5, 200)
    } else if max_span <= 600.0 {
        10
    } else if max_span <= 1600.0 || (20.0 / max_span) * (w.min(h) as f32) >= 4.0 {
        20
    } else {
        50
    };

    let thick_axis = if w.min(h) >= 700 { 2 } else { 1 };

    let min_x = lx0.floor() as i64;
    let max_x = lx1.ceil() as i64;
    let mut lv = (min_x.div_euclid(step_minor)) * step_minor;
    while lv <= max_x {
        let is_major = lv % 100 == 0 || lv % 50 == 0 && step_minor <= 10;
        let (col, th) = if lv == 512 {
            ([0, 235, 255, 185], thick_axis)
        } else if lv == 0 || lv == 1024 {
            ([110, 210, 255, 150], thick_axis)
        } else if lv % 100 == 0 {
            ([255, 255, 255, 100], 1)
        } else if is_major {
            ([255, 255, 255, 68], 1)
        } else {
            ([255, 255, 255, 38], 1)
        };
        let px = map_x(lv);
        for y in 0..h as i64 {
            for d in 0..th {
                blend_rgba_px(img, px + d, y, col);
            }
        }
        lv += step_minor;
    }

    let min_y = ly0.floor() as i64;
    let max_y = ly1.ceil() as i64;
    let mut lv = (min_y.div_euclid(step_minor)) * step_minor;
    while lv <= max_y {
        let is_major = lv % 100 == 0 || lv % 50 == 0 && step_minor <= 10;
        let (col, th) = if lv == 512 {
            ([0, 235, 255, 185], thick_axis)
        } else if lv == 0 || lv == 1024 {
            ([110, 210, 255, 150], thick_axis)
        } else if lv % 100 == 0 {
            ([255, 255, 255, 100], 1)
        } else if is_major {
            ([255, 255, 255, 68], 1)
        } else {
            ([255, 255, 255, 38], 1)
        };
        let py = map_y(lv);
        for x in 0..w as i64 {
            for d in 0..th {
                blend_rgba_px(img, x, py + d, col);
            }
        }
        lv += step_minor;
    }

    // Center axes x=512, y=512 (512 is not divisible by 10/20/50)
    if (512.0 >= lx0 && 512.0 <= lx1) && 512 % step_minor != 0 {
        let cx = map_x(512);
        for y in 0..h as i64 {
            for d in 0..thick_axis {
                blend_rgba_px(img, cx + d, y, [0, 235, 255, 185]);
            }
        }
    }
    if (512.0 >= ly0 && 512.0 <= ly1) && 512 % step_minor != 0 {
        let cy = map_y(512);
        for x in 0..w as i64 {
            for d in 0..thick_axis {
                blend_rgba_px(img, x, cy + d, [0, 235, 255, 185]);
            }
        }
    }
    for bound in [0i64, 1024i64] {
        if bound % step_minor != 0 {
            if (bound as f32) >= lx0 && (bound as f32) <= lx1 {
                let bx = map_x(bound);
                for y in 0..h as i64 {
                    for d in 0..thick_axis {
                        blend_rgba_px(img, bx + d, y, [110, 210, 255, 150]);
                    }
                }
            }
            if (bound as f32) >= ly0 && (bound as f32) <= ly1 {
                let by = map_y(bound);
                for x in 0..w as i64 {
                    for d in 0..thick_axis {
                        blend_rgba_px(img, x, by + d, [110, 210, 255, 150]);
                    }
                }
            }
        }
    }

    let font_scale = if w.max(h) >= 1200 {
        3
    } else if w.max(h) >= 480 {
        2
    } else {
        1
    };
    let pick_label_step = |span: f32, dim: u32| -> i64 {
        let min_px = (24 * font_scale + 18) as f32;
        for cand in [10i64, 20, 50, 100, 200, 500] {
            if cand >= step_minor && (cand as f32 / span) * (dim as f32) >= min_px {
                return cand;
            }
        }
        500
    };
    let label_step_x = pick_label_step(span_x, w);
    let label_step_y = pick_label_step(span_y, h);

    let bg_pill = Some([8, 12, 16, 205]);
    let pad_x = 30 * font_scale;
    let pad_y = 12 * font_scale;

    let mut vx = (min_x.div_euclid(label_step_x)) * label_step_x;
    while vx <= max_x {
        let px = map_x(vx);
        if px >= pad_x && px <= w as i64 - pad_x {
            let txt = vx.to_string();
            let text_w = (txt.len() as i64) * 6 * font_scale - font_scale;
            let fg = if vx == 0 || vx == 1024 {
                [140, 230, 255, 255]
            } else {
                [245, 248, 252, 250]
            };
            let tx = (px - text_w / 2).clamp(4, (w as i64 - text_w - 4).max(4));
            draw_text_5x7(img, &txt, tx, 5, font_scale, fg, bg_pill);
            if h as i64 >= 26 * font_scale {
                draw_text_5x7(
                    img,
                    &txt,
                    tx,
                    h as i64 - 7 * font_scale - 5,
                    font_scale,
                    fg,
                    bg_pill,
                );
            }
        }
        vx += label_step_x;
    }

    let mut vy = (min_y.div_euclid(label_step_y)) * label_step_y;
    while vy <= max_y {
        let py = map_y(vy);
        if py >= pad_y && py <= h as i64 - pad_y {
            let txt = vy.to_string();
            let text_w = (txt.len() as i64) * 6 * font_scale - font_scale;
            let fg = if vy == 0 || vy == 1024 {
                [140, 230, 255, 255]
            } else {
                [245, 248, 252, 250]
            };
            let ty = (py - (7 * font_scale) / 2).clamp(4, (h as i64 - 7 * font_scale - 4).max(4));
            draw_text_5x7(img, &txt, 5, ty, font_scale, fg, bg_pill);
            if w as i64 >= 64 * font_scale {
                draw_text_5x7(
                    img,
                    &txt,
                    (w as i64 - text_w - 5).max(5),
                    ty,
                    font_scale,
                    fg,
                    bg_pill,
                );
            }
        }
        vy += label_step_y;
    }
}

fn parse_crop_box(val: &Value) -> Result<Option<[f32; 4]>, String> {
    if val.is_null() {
        return Ok(None);
    }
    let coords: [f32; 4] = if let Some(arr) = val.as_array() {
        if arr.len() != 4 {
            return Err("crop array must have exactly 4 numbers [x0, y0, x1, y1]".into());
        }
        let mut out = [0.0f32; 4];
        for (i, item) in arr.iter().enumerate() {
            let f = item
                .as_f64()
                .ok_or_else(|| format!("crop[{i}] must be a number"))?;
            if !f.is_finite() {
                return Err("crop coordinates must be finite".into());
            }
            out[i] = f as f32;
        }
        out
    } else if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let parts: Vec<&str> = trimmed.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err("crop string must be 'x0,y0,x1,y1'".into());
        }
        let mut out = [0.0f32; 4];
        for (i, p) in parts.iter().enumerate() {
            let f = p
                .parse::<f32>()
                .map_err(|_| format!("invalid number in crop: {p:?}"))?;
            if !f.is_finite() {
                return Err("crop coordinates must be finite".into());
            }
            out[i] = f;
        }
        out
    } else {
        return Err("crop must be [x0, y0, x1, y1] or 'x0,y0,x1,y1'".into());
    };

    let lx0 = coords[0].min(coords[2]).clamp(-1024.0, 2048.0);
    let lx1 = coords[0].max(coords[2]).clamp(-1024.0, 2048.0);
    let ly0 = coords[1].min(coords[3]).clamp(-1024.0, 2048.0);
    let ly1 = coords[1].max(coords[3]).clamp(-1024.0, 2048.0);
    if lx1 - lx0 < 32.0 || ly1 - ly0 < 32.0 {
        return Err("crop region must be at least 32x32 logical units".into());
    }
    Ok(Some([lx0, ly0, lx1, ly1]))
}

#[allow(clippy::too_many_arguments)]
fn render_cropped_preview(
    session: &AgentSession,
    crop_box: [f32; 4],
    out_max: u32,
    with_ring: bool,
    xray: bool,
    grid: bool,
    grid_step: Option<i64>,
    show_example: bool,
    example_scale: u32,
    example_opacity: f32,
    guides: bool,
) -> RgbaImage {
    let [lx0, ly0, lx1, ly1] = crop_box;
    let cw = (lx1 - lx0).max(1.0);
    let ch = (ly1 - ly0).max(1.0);
    let max_span = cw.max(ch);
    let out_max = out_max.clamp(256, 1536);
    let out_w = ((cw / max_span) * out_max as f32).round().max(1.0) as u32;
    let out_h = ((ch / max_span) * out_max as f32).round().max(1.0) as u32;

    let needed_1x = ((out_max as f32 * LOGICAL / max_span).round() as u32).clamp(512, 1536);
    let s3 = needed_1x * 3;
    let fg_s3 = compose_foreground_s3(session, needed_1x, with_ring, xray);

    let px0 = (((lx0 + 1024.0) / 3072.0) * s3 as f32).floor().clamp(0.0, (s3 - 1) as f32) as u32;
    let py0 = (((ly0 + 1024.0) / 3072.0) * s3 as f32).floor().clamp(0.0, (s3 - 1) as f32) as u32;
    let px1 = (((lx1 + 1024.0) / 3072.0) * s3 as f32).ceil().clamp((px0 + 1) as f32, s3 as f32) as u32;
    let py1 = (((ly1 + 1024.0) / 3072.0) * s3 as f32).ceil().clamp((py0 + 1) as f32, s3 as f32) as u32;

    let sub = image::imageops::crop_imm(&fg_s3, px0, py0, px1 - px0, py1 - py0).to_image();
    let fg = image::imageops::resize(&sub, out_w, out_h, image::imageops::FilterType::Lanczos3);

    let mut out = if xray {
        let mut bg = fill_teal_checkerboard(out_w, out_h, out_w.max(out_h) / 28);
        image::imageops::overlay(&mut bg, &fg, 0, 0);
        bg
    } else {
        fg
    };

    if show_example {
        draw_example_overlay_in_view(&mut out, lx0, ly0, lx1, ly1, example_scale, example_opacity);
    }
    if xray {
        draw_xray_markers_in_view(&mut out, lx0, ly0, lx1, ly1);
    }
    if guides {
        draw_preview_guides_in_view(&mut out, lx0, ly0, lx1, ly1);
    }
    if grid {
        draw_canvas_grid_in_view(&mut out, lx0, ly0, lx1, ly1, grid_step);
    }
    out
}

/// Extracts a 1:1 full-resolution crop centered on the character's head/eyes for gaze inspection.
fn render_eye_crop(orig: &RgbaImage, cutout: Option<&RgbaImage>, min_width: Option<u32>) -> RgbaImage {
    let (w, h) = orig.dimensions();
    if w == 0 || h == 0 {
        return orig.clone();
    }
    let alpha_src = cutout.filter(|c| c.dimensions() == (w, h)).unwrap_or(orig);
    let mut min_alpha = 255u8;
    let mut top_y_opt: Option<u32> = None;
    for (y, row) in alpha_src.rows().enumerate() {
        for p in row {
            let a = p.0[3];
            if a < min_alpha {
                min_alpha = a;
            }
            if a > 60 && top_y_opt.is_none() {
                top_y_opt = Some(y as u32);
            }
        }
        if min_alpha < 200 && top_y_opt.is_some() {
            break;
        }
    }

    let (top_y, cx) = if min_alpha < 200 {
        let ty = top_y_opt.unwrap_or((h as f32 * 0.08) as u32);
        let y_lim = (ty + h / 4).min(h);
        let mut xs: Vec<u32> = Vec::new();
        for y in (ty..y_lim).step_by(2) {
            for x in (0..w).step_by(2) {
                if alpha_src.get_pixel(x, y).0[3] > 60 {
                    xs.push(x);
                }
            }
        }
        let mid_x = if xs.is_empty() {
            w / 2
        } else {
            let mid = xs.len() / 2;
            let (_, nth, _) = xs.select_nth_unstable(mid);
            *nth
        };
        (ty, mid_x)
    } else {
        ((h as f32 * 0.08) as u32, w / 2)
    };

    let crop_w = ((w as f32 * 0.34).round() as u32).max(320).min(w);
    let crop_h = ((crop_w as f32 * 0.60).round() as u32).max(192).min(h);
    let cy = top_y + (h as f32 * 0.13).round() as u32;
    let lx = cx.saturating_sub(crop_w / 2).min(w.saturating_sub(crop_w));
    let ly = cy.saturating_sub(crop_h / 2).min(h.saturating_sub(crop_h));
    let cropped = image::imageops::crop_imm(orig, lx, ly, crop_w, crop_h).to_image();
    if let Some(mw) = min_width
        && crop_w < mw
    {
        let nh = ((crop_h as f32 * (mw as f32 / crop_w as f32)).round() as u32).max(1);
        return image::imageops::resize(&cropped, mw, nh, image::imageops::FilterType::Lanczos3);
    }
    cropped
}

fn resize_mask(mask: &GrayImage, w: u32, h: u32) -> GrayImage {
    image::imageops::resize(mask, w, h, image::imageops::FilterType::Triangle)
}

// ---------- mask QA linter & inspect ----------

/// Analyses a 3x canvas subject image (`1536x1536`, 2 logical units per pixel) for:
/// 1. Detached islands (disconnected pieces outside the main figure, >= 48 logical px^2)
/// 2. Lower ring rim gap (insufficient coverage inside the bottom inner rim `r = 316..340`)
/// 3. Horizontal `preset1` cut steps along `y = 564` outside `r_in = 343`
fn compute_mask_qa_from_s3(s3_img: &RgbaImage, has_ring: bool) -> (Value, Vec<String>) {
    let (w, h) = s3_img.dimensions();
    if w == 0 || h == 0 {
        return (
            json!({
                "clean": false,
                "detached_islands": [],
                "bottom_ring_gap": false,
                "bottom_rim_fill_pct": 0.0,
                "step_564": []
            }),
            vec![],
        );
    }
    let scale_px = 3072.0 / w as f32;
    let px_area_log = (scale_px * scale_px).round() as u64;
    let mut visited = vec![0u8; (w * h) as usize];
    let raw = s3_img.as_raw();
    let wi = w as usize;
    let hi = h as usize;

    struct Comp {
        count: u64,
        min_x: u32,
        min_y: u32,
        max_x: u32,
        max_y: u32,
        sum_x: u64,
        sum_y: u64,
    }

    let mut comps: Vec<Comp> = Vec::new();
    let mut queue: Vec<u32> = Vec::new();

    for sy in 0..h {
        let row_off = sy as usize * wi;
        for sx in 0..w {
            let idx = row_off + sx as usize;
            if visited[idx] != 0 || raw[idx * 4 + 3] <= 24 {
                continue;
            }
            visited[idx] = 1;
            queue.clear();
            queue.push(sy * w + sx);
            let mut head = 0usize;
            let mut c = Comp {
                count: 0,
                min_x: sx,
                min_y: sy,
                max_x: sx,
                max_y: sy,
                sum_x: 0,
                sum_y: 0,
            };
            while head < queue.len() {
                let cur = queue[head];
                head += 1;
                let cx = cur % w;
                let cy = cur / w;
                c.count += 1;
                c.min_x = c.min_x.min(cx);
                c.min_y = c.min_y.min(cy);
                c.max_x = c.max_x.max(cx);
                c.max_y = c.max_y.max(cy);
                c.sum_x += cx as u64;
                c.sum_y += cy as u64;

                let x0 = cx.saturating_sub(1);
                let y0 = cy.saturating_sub(1);
                let x1 = (cx + 1).min((wi - 1) as u32);
                let y1 = (cy + 1).min((hi - 1) as u32);
                for ny in y0..=y1 {
                    let n_row = ny as usize * wi;
                    for nx in x0..=x1 {
                        let nidx = n_row + nx as usize;
                        if visited[nidx] == 0 && raw[nidx * 4 + 3] > 24 {
                            visited[nidx] = 1;
                            queue.push(ny * w + nx);
                        }
                    }
                }
            }
            comps.push(c);
        }
    }

    let to_lx = |px: f32| -> i64 { (px * scale_px - 1024.0).round() as i64 };
    let mut qa_warnings: Vec<String> = Vec::new();

    let mut islands: Vec<(u64, Value)> = Vec::new();
    let has_subject = !comps.is_empty();
    if comps.len() > 1 {
        let main_idx = comps
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| c.count)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let min_count = ((48.0 / (scale_px * scale_px)).ceil() as u64).max(4);
        for (i, c) in comps.iter().enumerate() {
            if i == main_idx || c.count < min_count {
                continue;
            }
            let lx0 = to_lx(c.min_x as f32);
            let ly0 = to_lx(c.min_y as f32);
            let lx1 = to_lx((c.max_x + 1) as f32);
            let ly1 = to_lx((c.max_y + 1) as f32);
            let cx = to_lx(c.sum_x as f32 / c.count as f32);
            let cy = to_lx(c.sum_y as f32 / c.count as f32);
            let area = c.count * px_area_log;
            islands.push((
                area,
                json!({
                    "bbox": [lx0, ly0, lx1, ly1],
                    "center": [cx, cy],
                    "area_logical_px": area,
                }),
            ));
        }
        islands.sort_by(|a, b| b.0.cmp(&a.0));
        for (area, isl) in &islands {
            let b = &isl["bbox"];
            qa_warnings.push(format!(
                "detached island at [{}, {}, {}, {}] ({area} logical px^2) — severed hand/weapon or leftover scrap",
                b[0], b[1], b[2], b[3]
            ));
        }
    }
    let detached_islands: Vec<Value> = islands.into_iter().map(|(_, v)| v).collect();

    let mut bottom_rim_fill_pct = 0.0f64;
    let mut bottom_ring_gap = false;
    let mut step_564: Vec<Value> = Vec::new();

    if has_ring && has_subject {
        let px_from_l = |l: f32| -> i64 { ((l + 1024.0) / scale_px).round() as i64 };
        let px_a = px_from_l(412.0).max(0) as u32;
        let px_b = (px_from_l(612.0) as u32).min(w.saturating_sub(1));
        let py_a = px_from_l(800.0).max(0) as u32;
        let py_b = (px_from_l(854.0) as u32).min(h.saturating_sub(1));
        let mut arc_total = 0u64;
        let mut arc_filled = 0u64;
        for py in py_a..=py_b {
            let ly = py as f32 * scale_px - 1024.0;
            let row_off = py as usize * wi;
            for px in px_a..=px_b {
                let lx = px as f32 * scale_px - 1024.0;
                let r = (lx - 512.0).hypot(ly - 512.0);
                if (316.0..=340.0).contains(&r) {
                    arc_total += 1;
                    if raw[(row_off + px as usize) * 4 + 3] > 180 {
                        arc_filled += 1;
                    }
                }
            }
        }
        if arc_total > 0 {
            bottom_rim_fill_pct = ((arc_filled as f64 / arc_total as f64) * 1000.0).round() / 10.0;
            if bottom_rim_fill_pct < 85.0 {
                bottom_ring_gap = true;
                qa_warnings.push(format!(
                    "gap inside lower ring rim (fill {bottom_rim_fill_pct:.1}% < 85% near y=830..855)"
                ));
            }
        }

        // Detect horizontal preset1 cut steps along y = 564 outside r_in = 343
        let above_y0 = px_from_l(548.0).max(0) as u32;
        let above_y1 = (px_from_l(560.0) as u32).min(h.saturating_sub(1));
        let below_y0 = px_from_l(568.0).max(0) as u32;
        let below_y1 = (px_from_l(580.0) as u32).min(h.saturating_sub(1));
        let min_step_cols = ((18.0 / scale_px).ceil() as u32).max(2);

        for (lx_min, lx_max) in [(-512.0f32, 166.0f32), (858.0f32, 1536.0f32)] {
            let sx0 = px_from_l(lx_min).max(0) as u32;
            let sx1 = (px_from_l(lx_max) as u32).min(w.saturating_sub(1));
            let mut run_start: Option<u32> = None;
            let mut run_end: u32 = 0;
            let mut gap_cols: u32 = 0;

            let flush_run = |rs: Option<u32>, re: u32, out: &mut Vec<Value>, warns: &mut Vec<String>| {
                if let Some(s) = rs
                    && re >= s
                    && (re - s + 1) >= min_step_cols
                {
                    let bx0 = to_lx(s as f32);
                    let bx1 = to_lx((re + 1) as f32);
                    warns.push(format!(
                        "horizontal preset1 step near y=564 at x=[{bx0}, {bx1}]"
                    ));
                    out.push(json!([bx0, 544, bx1, 580]));
                }
            };

            for px in sx0..=sx1 {
                let mut above_max = 0u8;
                for py in above_y0..=above_y1 {
                    let a = raw[(py as usize * wi + px as usize) * 4 + 3];
                    if a > above_max {
                        above_max = a;
                    }
                }
                let mut below_max = 0u8;
                for py in below_y0..=below_y1 {
                    let a = raw[(py as usize * wi + px as usize) * 4 + 3];
                    if a > below_max {
                        below_max = a;
                    }
                }
                let is_step = above_max > 120 && below_max <= 15;
                if is_step {
                    if run_start.is_none() {
                        run_start = Some(px);
                    }
                    run_end = px;
                    gap_cols = 0;
                } else if run_start.is_some() {
                    gap_cols += 1;
                    if gap_cols > 1 {
                        flush_run(run_start, run_end, &mut step_564, &mut qa_warnings);
                        run_start = None;
                    }
                }
            }
            flush_run(run_start, run_end, &mut step_564, &mut qa_warnings);
        }
    }

    let clean = detached_islands.is_empty() && !bottom_ring_gap && step_564.is_empty() && has_subject;
    (
        json!({
            "clean": clean,
            "detached_islands": detached_islands,
            "bottom_ring_gap": bottom_ring_gap,
            "bottom_rim_fill_pct": bottom_rim_fill_pct,
            "step_564": step_564,
        }),
        qa_warnings,
    )
}

fn compute_mask_qa(session: &AgentSession) -> (Value, Vec<String>) {
    let s3_512 = render_subject_on_s3(session, 512);
    compute_mask_qa_from_s3(&s3_512, session.ring.is_some())
}

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

    let mut warnings: Vec<String> = Vec::new();
    if covered > 0 {
        if min_x == 0 || min_y == 0 || max_x >= LOGICAL_U32 - 1 || max_y >= LOGICAL_U32 - 1 {
            warnings.push("subject touches 1x canvas border".into());
        }
        if max_y > 920 && session.ring.is_some() {
            warnings.push("subject extends below ring lower rim (y > 920); consider erasing bottom arc or raising offset_y".into());
        }
    } else {
        warnings.push("subject is completely empty/invisible in 1x frame".into());
    }
    if !already_transparent && !session.bg_removed {
        warnings.push("source has opaque background; call remove_background first".into());
    }

    let (qa_json, qa_warnings) = compute_mask_qa(session);
    warnings.extend(qa_warnings);

    json!({
        "bbox": bbox_json,
        "coverage_pct": cov_pct,
        "alpha_hist": hist,
        "edge_px": edge_px,
        "grid_3x3_pct": grid,
        "warnings": warnings,
        "qa": qa_json,
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

struct PreviewOpts {
    size: Option<u32>,
    scale_mode: Option<String>,
    with_ring: bool,
    xray: bool,
    grid: bool,
    grid_step: Option<i64>,
    example: bool,
    example_scale: u32,
    example_opacity: f32,
    guides: bool,
    crop: Option<[f32; 4]>,
    eyes: bool,
}

fn render_preview_image(session: &AgentSession, opts: &PreviewOpts) -> Result<RgbaImage, String> {
    if opts.eyes {
        return Ok(render_eye_crop(
            &session.original,
            session.cutout.as_ref(),
            Some(640),
        ));
    }
    if let Some(crop_box) = opts.crop {
        let out_max = opts.size.unwrap_or(1024).clamp(256, 1536);
        return Ok(render_cropped_preview(
            session,
            crop_box,
            out_max,
            opts.with_ring,
            opts.xray,
            opts.grid,
            opts.grid_step,
            opts.example,
            opts.example_scale,
            opts.example_opacity,
            opts.guides,
        ));
    }
    let size = opts.size.unwrap_or(DEFAULT_PREVIEW_SIZE).clamp(64, 1024);
    let def_sm = if opts.example || opts.xray || opts.grid {
        "2"
    } else {
        "1"
    };
    let sm = opts.scale_mode.as_deref().unwrap_or(def_sm);
    let (mut img, used) = if opts.xray {
        compose_xray(session, size, sm, opts.with_ring)?
    } else {
        compose(session, size, sm, opts.with_ring)?
    };
    if opts.example {
        draw_example_overlay(&mut img, used, opts.example_scale, opts.example_opacity);
    }
    if opts.guides {
        draw_preview_guides(&mut img, used);
    }
    if opts.grid {
        let (lx0, ly0, lx1, ly1) = view_bounds_for_scale(used);
        draw_canvas_grid_in_view(&mut img, lx0, ly0, lx1, ly1, opts.grid_step);
    }
    Ok(img)
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
            "preview", "export", "pipeline", "qa_sheet", "gui_sync", "close"
        ],
        "mask_ops": ["erase", "restore", "apply_preset", "invert", "feather", "grow", "reset"],
        "mask_targets": ["canvas", "image", "both"],
        "export_formats": ["webp", "png", "jpg"],
        "limits": {
            "export_size_range": [256, 2048],
            "max_base_dim": MAX_BASE_DIM,
            "max_body_bytes": 15_728_640,
            "max_ops_per_session": OPS_LIMIT,
            "max_sessions": crate::config::max_agent_sessions(),
            "session_ttl_min": SESSION_TTL_MIN,
        },
        "defaults": {
            "ring": default_ring_name(),
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
            {"type": "function", "function": {"name": "tokenatra_create_session", "description": "Create an editing session from an image_path or image_b64", "parameters": {"type": "object", "properties": {"image_path": {"type": "string"}, "image_b64": {"type": "string"}, "ring": {"type": ["string", "null"]}, "live_sync": {"type": "boolean"}}}}},
            {"type": "function", "function": {"name": "tokenatra_remove_background", "description": "Remove background (ONNX) or toggle showing_original", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "edge_blur": {"type": "number"}, "showing_original": {"type": "boolean"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_edit_mask", "description": "Edit visibility mask (1024x1024 logical). target: canvas (pink mask, fixed to ring) | image (blue eraser, moves with subject). ops: erase/restore/apply_preset/invert/feather/grow/reset. Supports circle, rect, stroke, polygon.", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "target": {"type": "string", "enum": ["canvas", "image", "both"]}, "op": {"type": "string", "enum": ["erase", "restore", "apply_preset", "invert", "feather", "grow", "reset"]}, "shapes": {"type": "array", "items": {"type": "object"}}, "shapes_json": {"type": "string"}, "preset": {"type": "string"}, "px": {"type": "integer"}, "protect": {"type": "boolean"}}, "required": ["session_id", "op"]}}},
            {"type": "function", "function": {"name": "tokenatra_set_transform", "description": "Set subject transform in 1024x1024 1x export frame: L(f) = 512 + (f - 512)*scale + offset", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "scale": {"type": "number"}, "rotation": {"type": "number"}, "offset_x": {"type": "number"}, "offset_y": {"type": "number"}, "fit_zone": {"type": "integer", "enum": [1, 2, 3]}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_set_ring", "description": "Set token ring by name or remove (null)", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "name": {"type": ["string", "null"]}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_set_effects", "description": "Shadow and color correction", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "shadow_json": {"type": "string"}, "color_json": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_history", "description": "Undo or redo last editing action", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "action": {"type": "string", "enum": ["undo", "redo"]}}, "required": ["session_id", "action"]}}},
            {"type": "function", "function": {"name": "tokenatra_portrait", "description": "Configure and export a square portrait (PortraitGenerator)", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "variant": {"type": "string", "enum": ["original", "cutout"]}, "scale": {"type": "number"}, "rotation": {"type": "number"}, "offset_x": {"type": "number"}, "offset_y": {"type": "number"}, "size": {"type": "integer"}, "format": {"type": "string", "enum": ["webp", "png", "jpg"]}, "quality": {"type": "integer"}, "output_path": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_inspect", "description": "Session state, mask QA linter (detached_islands, bottom_ring_gap, step_564) & alpha bbox stats", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "include_image": {"type": "boolean"}, "preview_size": {"type": "integer"}, "xray": {"type": "boolean"}, "grid": {"type": "boolean"}, "grid_step": {"type": "integer"}, "crop": {"type": ["array", "string"]}, "eyes": {"type": "boolean"}, "example": {"type": "boolean"}, "guides": {"type": "boolean"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_preview", "description": "Preview image (webp, default size=768 -> 1536x1536 at 2x) with optional xray, grid, grid_step, crop, eyes, example, or alignment guides", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "size": {"type": "integer"}, "scale_mode": {"type": "string", "enum": ["auto", "1", "2", "3"]}, "with_ring": {"type": "boolean"}, "xray": {"type": "boolean"}, "grid": {"type": "boolean"}, "grid_step": {"type": "integer"}, "crop": {"type": ["array", "string"]}, "eyes": {"type": "boolean"}, "example": {"type": "boolean"}, "guides": {"type": "boolean"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_export", "description": "Export final token image", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "format": {"type": "string", "enum": ["webp", "png", "jpg"]}, "quality": {"type": "integer"}, "size": {"type": "integer"}, "scale_mode": {"type": "string", "enum": ["auto", "1", "2", "3"]}, "with_ring": {"type": "boolean"}, "output_path": {"type": "string"}, "save_to": {"type": "string"}}, "required": ["session_id"]}}},
            {"type": "function", "function": {"name": "tokenatra_pipeline", "description": "Execute a full multi-step token & portrait workflow in a single call", "parameters": {"type": "object", "properties": {"session_id": {"type": "string"}, "image_path": {"type": "string"}, "ring": {"type": ["string", "null"]}, "live_sync": {"type": "boolean"}, "remove_bg": {"type": ["boolean", "string"]}, "portrait": {"type": "object"}, "transform": {"type": "object"}, "preset": {"type": "string"}, "mask_ops": {"type": "array", "items": {"type": "object"}}, "effects": {"type": "object"}, "export": {"type": "object"}, "exports": {"type": "array", "items": {"type": "object"}}, "preview": {"type": ["boolean", "object"]}, "close_session": {"type": "boolean"}}}}},
            {"type": "function", "function": {"name": "tokenatra_qa_sheet", "description": "Render paginated 2x2 xray, 4x4 eyes, or 4x4 showcase contact sheet from sessions or folders", "parameters": {"type": "object", "properties": {"mode": {"type": "string", "enum": ["xray", "eyes", "showcase"]}, "session_ids": {"type": "array", "items": {"type": "string"}}, "items": {"type": "array", "items": {"type": "object"}}, "subjects_dir": {"type": "string"}, "portraits_dir": {"type": "string"}, "input_dir": {"type": "string"}, "transforms_json": {"type": "string"}, "prefix": {"type": "string"}, "ring": {"type": "string"}, "page": {"type": "integer"}, "per_page": {"type": "integer"}, "max_width": {"type": "integer"}, "save_to": {"type": "string"}}}}},
            {"type": "function", "function": {"name": "tokenatra_gui_sync", "description": "Sync an agent session live with the open Tokenatra GUI window (push, detach, or status)", "parameters": {"type": "object", "properties": {"action": {"type": "string", "enum": ["push", "detach", "status"]}, "session_id": {"type": "string"}, "show_window": {"type": "boolean"}}, "required": ["action"]}}},
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

        let live_sync = v.get("live_sync").and_then(|x| x.as_bool()).unwrap_or(true);
        let label = source_label_from_json(&v);

        match decode_source(&v) {
            Ok(base) => {
                let arc = store_create(base, ring_override, live_sync, label);
                let (resp, sid_out, rev_out) = {
                    let sess = arc.lock().unwrap();
                    let stats = inspect_session(&sess);
                    let sid_out = sess.id.clone();
                    let rev_out = sess.rev;
                    (
                        json!({
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
                        }),
                        sid_out,
                        rev_out,
                    )
                };
                if live_sync {
                    notify_gui_sync(&sid_out, rev_out, false);
                }
                ok_json(resp)
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
    notify_gui_closed(&sid);
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
            s.rev += 1;
            s.log_op("toggle_original", json!({"showing_original": show_orig}));
            s.push_history();
            let stats = inspect_session(&s);
            let do_sync = s.live_sync;
            let rev = s.rev;
            drop(s);
            if do_sync {
                notify_gui_sync(&sid, rev, false);
            }
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
            let (stats, do_sync, rev) = {
                let mut s = arc.lock().unwrap();
                s.cutout = Some(rgba);
                s.bg_removed = true;
                s.showing_original = false;
                s.portrait_variant = "cutout".to_string();
                // Just like TokenEditor.handleRemoveBackground in UI: reset imageMask to white
                s.image_mask = GrayImage::from_pixel(s.original.width(), s.original.height(), Luma([255u8]));
                s.img_rev += 1;
                s.image_mask_rev += 1;
                s.rev += 1;
                s.log_op("remove_bg", json!({"edge_blur": edge_blur}));
                s.push_history();
                (inspect_session(&s), s.live_sync, s.rev)
            };
            if do_sync {
                notify_gui_sync(&sid, rev, false);
            }
            ok_json(json!({"ok": true, "bg_removed": true, "showing_original": false, "mask": stats}))
        }
        Ok(Err(e)) => agent_err(StatusCode::INTERNAL_SERVER_ERROR, "infer_error", &e.to_string()),
        Err(e) => agent_err(StatusCode::INTERNAL_SERVER_ERROR, "join_error", &e.to_string()),
    }
}

fn apply_mask_step(s: &mut AgentSession, v: &Value) -> Result<(), (StatusCode, &'static str, String)> {
    let op = v["op"].as_str().unwrap_or_default().to_string();
    let target = v["target"].as_str().unwrap_or("canvas").to_lowercase();
    let px = v["px"].as_f64().map(|f| f.round().max(0.0) as u32).unwrap_or(0);
    if let Some(pe) = v["protection_enabled"].as_bool() {
        s.protection_enabled = pe;
    }
    let protect = v["protect"].as_bool().unwrap_or(s.protection_enabled);
    let params = v.clone();

    // Accept either `shapes` array OR `shapes_json` string
    let parsed_shapes_opt: Option<Value> = if let Some(arr) = v.get("shapes")
        && !arr.is_null()
    {
        Some(arr.clone())
    } else if let Some(raw_str) = v.get("shapes_json").and_then(|x| x.as_str()) {
        if raw_str.trim().is_empty() {
            None
        } else {
            let pv: Value = serde_json::from_str(raw_str).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    format!("shapes_json is not valid JSON: {e}"),
                )
            })?;
            Some(pv)
        }
    } else {
        None
    };

    match op.as_str() {
        "erase" | "restore" => {
            let erase = op == "erase";
            let Some(ref shapes_val) = parsed_shapes_opt else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "shapes array required for erase/restore".into(),
                ));
            };
            if target == "image" || target == "blue" {
                let prev = s.image_mask.clone();
                mask_apply_shapes(&mut s.image_mask, shapes_val, erase, false)
                    .map_err(|msg| (StatusCode::BAD_REQUEST, "bad_request", msg))?;
                if erase
                    && protect
                    && let Some(prot_1024) = load_protection_mask_1024(s.ring.as_deref())
                {
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
                        let prot_a = sample_gray_bilinear(&prot_1024, rx, ry);
                        if prot_a > 0 {
                            let floor = ((old_v as u32 * prot_a as u32 + 127) / 255) as u8;
                            if p.0[0] < floor {
                                p.0[0] = floor;
                            }
                        }
                    }
                }
            } else {
                let prev = s.canvas_mask.clone();
                mask_apply_shapes(&mut s.canvas_mask, shapes_val, erase, true)
                    .map_err(|msg| (StatusCode::BAD_REQUEST, "bad_request", msg))?;
                if erase
                    && protect
                    && let Some(prot_1024) = load_protection_mask_1024(s.ring.as_deref())
                {
                    let off = LOGICAL_U32;
                    for y in 0..LOGICAL_U32 {
                        for x in 0..LOGICAL_U32 {
                            let prot_a = prot_1024.get_pixel(x, y).0[0];
                            if prot_a > 0 {
                                let old_v = prev.get_pixel(off + x, off + y).0[0];
                                let cur_v = s.canvas_mask.get_pixel(off + x, off + y).0[0];
                                let floor = ((old_v as u32 * prot_a as u32 + 127) / 255) as u8;
                                if cur_v < floor {
                                    s.canvas_mask.put_pixel(off + x, off + y, Luma([floor]));
                                }
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
                s.image_mask =
                    GrayImage::from_pixel(s.original.width(), s.original.height(), Luma([255u8]));
            } else if target == "both" {
                s.canvas_mask =
                    GrayImage::from_pixel(LOGICAL_U32 * 3, LOGICAL_U32 * 3, Luma([255u8]));
                s.image_mask =
                    GrayImage::from_pixel(s.original.width(), s.original.height(), Luma([255u8]));
            } else {
                s.canvas_mask =
                    GrayImage::from_pixel(LOGICAL_U32 * 3, LOGICAL_U32 * 3, Luma([255u8]));
            }
        }
        "feather" => {
            if px == 0 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "px required for feather".into(),
                ));
            }
            if target == "image" || target == "blue" {
                s.image_mask = image::imageops::blur(&s.image_mask, px as f32 * 0.5);
            } else {
                s.canvas_mask = image::imageops::blur(&s.canvas_mask, px as f32 * 0.5);
            }
        }
        "grow" => {
            if px == 0 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "px required for grow".into(),
                ));
            }
            use imageproc::distance_transform::Norm;
            if target == "image" || target == "blue" {
                s.image_mask =
                    imageproc::morphology::dilate(&s.image_mask, Norm::LInf, px.min(255) as u8);
            } else {
                s.canvas_mask =
                    imageproc::morphology::dilate(&s.canvas_mask, Norm::LInf, px.min(255) as u8);
            }
        }
        "apply_preset" => {
            let pname = v["preset"].as_str().unwrap_or_default();
            let Some(pp) = find_preset_file(pname) else {
                return Err((
                    StatusCode::NOT_FOUND,
                    "unknown_preset",
                    format!("preset not found: {pname}"),
                ));
            };
            match load_preset_mask(&pp, LOGICAL_U32 * 3, LOGICAL_U32 * 3) {
                Ok(m) => s.canvas_mask = m,
                Err(e) => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "bad_request",
                        format!("preset decode failed: {e}"),
                    ));
                }
            }
        }
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                "bad_mask_op",
                format!("unknown mask op {other:?}"),
            ));
        }
    }

    if target == "image" || target == "blue" {
        s.image_mask_rev += 1;
    } else if target == "both" {
        s.canvas_mask_rev += 1;
        s.image_mask_rev += 1;
    } else {
        s.canvas_mask_rev += 1;
    }
    s.rev += 1;
    s.log_op(&format!("mask.{op}"), params);
    s.push_history();
    Ok(())
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
        let (stats, do_sync, rev) = {
            let mut s = arc.lock().unwrap();
            if let Err((status, code, msg)) = apply_mask_step(&mut s, &v) {
                return agent_err(status, code, &msg);
            }
            (inspect_session(&s), s.live_sync, s.rev)
        };
        if do_sync {
            notify_gui_sync(&sid, rev, false);
        }
        ok_json(json!({"ok": true, "mask": stats}))
    })
    .await
}

fn apply_transform_step(s: &mut AgentSession, v: &Value) -> Result<(), (StatusCode, &'static str, String)> {
    if let Some(sc) = v["scale"].as_f64()
        && (!sc.is_finite() || sc <= 0.0 || sc > 8.0)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "bad_request",
            "scale out of range (0.01..8.0)".into(),
        ));
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
    if let Some(x) = v["rotation"].as_f64()
        && x.is_finite()
    {
        s.rotation = x as f32;
    }
    if let Some(x) = v["offset_x"].as_f64()
        && x.is_finite()
    {
        s.offset_x = x as f32;
    }
    if let Some(x) = v["offset_y"].as_f64()
        && x.is_finite()
    {
        s.offset_y = x as f32;
    }
    s.rev += 1;
    s.log_op("transform", v.clone());
    s.push_history();
    Ok(())
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
        let (resp, do_sync, rev) = {
            let mut s = arc.lock().unwrap();
            if let Err((status, code, msg)) = apply_transform_step(&mut s, &v) {
                return agent_err(status, code, &msg);
            }
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
            (resp, s.live_sync, s.rev)
        };
        if do_sync {
            notify_gui_sync(&sid, rev, false);
        }
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
    let (do_sync, rev) = {
        let mut s = arc.lock().unwrap();
        s.ring = name.clone();
        s.rev += 1;
        s.log_op("ring", json!({"name": name}));
        (s.live_sync, s.rev)
    };
    if do_sync {
        notify_gui_sync(&sid, rev, false);
    }
    ok_json(json!({"ok": true, "ring": name}))
}

fn apply_effects_step(s: &mut AgentSession, v: &Value) {
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
    s.rev += 1;
    s.log_op("effects", v.clone());
}

pub async fn op_effects(Path(sid): Path<String>, body: Option<axum::Json<Value>>) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let arc = match session_op_guard(&sid) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (resp, do_sync, rev) = {
        let mut s = arc.lock().unwrap();
        apply_effects_step(&mut s, &v);
        let resp = json!({
            "ok": true,
            "effects": {
                "shadow": {"enabled": s.shadow_enabled, "angle": s.shadow_angle, "distance": s.shadow_distance, "blur": s.shadow_blur, "opacity": s.shadow_opacity},
                "color": {"enabled": s.color_enabled, "saturation": s.color_sat, "lightness": s.color_light, "contrast": s.color_contrast},
            }
        });
        (resp, s.live_sync, s.rev)
    };
    if do_sync {
        notify_gui_sync(&sid, rev, false);
    }
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
        let (resp, do_sync, rev) = {
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
            s.canvas_mask_rev += 1;
            s.image_mask_rev += 1;
            s.rev += 1;
            let idx = s.history_index;
            let total = s.history.len();
            s.log_op(&format!("history.{action}"), json!({"index": idx}));
            let stats = inspect_session(&s);
            let resp = json!({
                "ok": true,
                "history_index": idx,
                "history_count": total,
                "mask": stats,
            });
            (resp, s.live_sync, s.rev)
        };
        if do_sync {
            notify_gui_sync(&sid, rev, false);
        }
        ok_json(resp)
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

fn parse_bool_str(v: &str) -> bool {
    v == "1" || v.eq_ignore_ascii_case("true")
}

fn preview_opts_from_query(q: &HashMap<String, String>, size_key: &str, default_with_ring: bool) -> Result<PreviewOpts, String> {
    let size = q.get(size_key).and_then(|v| v.parse::<u32>().ok());
    let crop = match q.get("crop") {
        Some(c) if !c.trim().is_empty() => parse_crop_box(&Value::String(c.clone()))?,
        _ => None,
    };
    let grid_step = q.get("grid_step").and_then(|v| v.parse::<i64>().ok());
    Ok(PreviewOpts {
        size,
        scale_mode: q.get("scale_mode").cloned(),
        with_ring: q
            .get("with_ring")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(default_with_ring),
        xray: q.get("xray").map(|v| parse_bool_str(v)).unwrap_or(false),
        grid: q.get("grid").map(|v| parse_bool_str(v)).unwrap_or(false),
        grid_step,
        example: q.get("example").map(|v| parse_bool_str(v)).unwrap_or(false),
        example_scale: q
            .get("example_scale")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2)
            .clamp(1, 3),
        example_opacity: q
            .get("example_opacity")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.65_f32)
            .clamp(0.1, 1.0),
        guides: q.get("guides").map(|v| parse_bool_str(v)).unwrap_or(false),
        crop,
        eyes: q.get("eyes").map(|v| parse_bool_str(v)).unwrap_or(false),
    })
}

fn preview_opts_from_json(ps: &Value) -> Result<PreviewOpts, String> {
    let crop = match ps.get("crop") {
        Some(c) => parse_crop_box(c)?,
        None => None,
    };
    Ok(PreviewOpts {
        size: ps.get("size").and_then(|x| x.as_u64()).map(|n| n as u32),
        scale_mode: ps.get("scale_mode").and_then(|x| x.as_str()).map(String::from),
        with_ring: ps.get("with_ring").and_then(|x| x.as_bool()).unwrap_or(true),
        xray: ps.get("xray").and_then(|x| x.as_bool()).unwrap_or(false),
        grid: ps.get("grid").and_then(|x| x.as_bool()).unwrap_or(false),
        grid_step: ps.get("grid_step").and_then(|x| x.as_i64()),
        example: ps.get("example").and_then(|x| x.as_bool()).unwrap_or(false),
        example_scale: ps.get("example_scale").and_then(|x| x.as_u64()).unwrap_or(2).clamp(1, 3) as u32,
        example_opacity: ps.get("example_opacity").and_then(|x| x.as_f64()).unwrap_or(0.65).clamp(0.1, 1.0) as f32,
        guides: ps.get("guides").and_then(|x| x.as_bool()).unwrap_or(false),
        crop,
        eyes: ps.get("eyes").and_then(|x| x.as_bool()).unwrap_or(false),
    })
}

pub async fn op_preview(
    Path(sid): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    run_blocking(move || {
        let Some(arc) = store_get(&sid) else {
            return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
        };
        let opts = match preview_opts_from_query(&q, "size", true) {
            Ok(o) => o,
            Err(msg) => return bad("bad_request", &msg),
        };
        let q_webp = if opts.eyes { 95 } else { PREVIEW_WEBP_QUALITY };
        let rendered = {
            let s = arc.lock().unwrap();
            render_preview_image(&s, &opts)
        };
        match rendered {
            Ok(img) => match crate::imaging::save_image(
                &image::DynamicImage::ImageRgba8(img),
                "webp",
                q_webp,
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
        let include_img = q.get("include_image").map(|v| parse_bool_str(v)).unwrap_or(false);
        let preview_opts = if include_img {
            match preview_opts_from_query(&q, "preview_size", true) {
                Ok(o) => Some(o),
                Err(msg) => return bad("bad_request", &msg),
            }
        } else {
            None
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
        if let Some(opts) = preview_opts {
            let rendered = render_preview_image(&s, &opts);
            drop(s);
            let q_webp = if opts.eyes { 95 } else { PREVIEW_WEBP_QUALITY };
            if let Ok(img) = rendered
                && let Ok((bytes, _)) = crate::imaging::save_image(
                    &image::DynamicImage::ImageRgba8(img),
                    "webp",
                    q_webp,
                )
            {
                let data_url = format!(
                    "data:image/webp;base64,{}",
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
                );
                v["image_data_url"] = json!(data_url);
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

fn infer_format_from_spec(spec: &Value, save_to: Option<&str>, default_fmt: &str) -> Result<String, String> {
    if let Some(f) = spec.get("format").and_then(|v| v.as_str())
        && !f.trim().is_empty()
    {
        let fl = f.trim().to_lowercase();
        let norm = if fl == "jpeg" { "jpg".to_string() } else { fl };
        if !["webp", "png", "jpg"].contains(&norm.as_str()) {
            return Err(format!("unsupported format {f:?}; expected webp, png, or jpg"));
        }
        return Ok(norm);
    }
    if let Some(st) = save_to {
        let ext = std::path::Path::new(st)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if matches!(ext.as_str(), "webp" | "png" | "jpg" | "jpeg") {
            return Ok(if ext == "jpeg" { "jpg".to_string() } else { ext });
        }
    }
    Ok(default_fmt.to_string())
}

fn save_bytes_to_target(payload: &[u8], fmt: &str, save_to: &str, prefix: &str) -> Result<String, String> {
    let s = save_to.trim();
    if s.is_empty() {
        return Err("save_to path is empty".into());
    }
    let target = std::path::Path::new(s);
    let ext = target
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if matches!(ext.as_str(), "png" | "webp" | "jpg" | "jpeg") && !target.is_dir() {
        if let Some(parent) = target.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| format!("failed to create parent dir: {e}"))?;
        }
        paths::atomic_write(target, payload).map_err(|e| format!("failed to write {}: {e}", target.display()))?;
        return Ok(target.to_string_lossy().to_string());
    }
    if !target.exists() {
        std::fs::create_dir_all(target)
            .map_err(|e| format!("failed to create directory {}: {e}", target.display()))?;
    }
    if !target.is_dir() {
        return Err(format!(
            "save_to is neither an image file path nor a directory: {s}"
        ));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let file_path = target.join(format!("{prefix}-{stamp}.{fmt}"));
    paths::atomic_write(&file_path, payload)
        .map_err(|e| format!("failed to write {}: {e}", file_path.display()))?;
    Ok(file_path.to_string_lossy().to_string())
}

fn source_already_transparent(img: &RgbaImage) -> bool {
    let (w, h) = img.dimensions();
    let total = (w * h).max(1) as f64;
    let mut opaque = 0u64;
    for p in img.pixels() {
        if p.0[3] >= 250 {
            opaque += 1;
        }
    }
    let opaque_pct = (opaque as f64 / total) * 100.0;
    opaque_pct < 92.0
}

/// Composite single-call token & portrait pipeline (`POST /agent/v1/pipeline`).
/// Runs session open (or reuses `session_id`), smart `remove_bg` (`"auto"` | `"always"` | `"never"`),
/// optional `portrait` export, `transform`, `preset`, `mask_ops`, `effects`, one or more `export`s,
/// optional `preview` (including `xray` and `grid`), and optional `close_session`.
pub async fn op_pipeline(
    State(state): State<std::sync::Arc<AppState>>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let st = state.clone();
    run_blocking(move || {
        // 1. Resolve existing session or create a new one from image_path / image
        let (arc, sid_str, created_inline) = if let Some(sid) = v
            .get("session_id")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match session_op_guard(sid) {
                Ok(a) => (a, sid.to_string(), false),
                Err(r) => return r,
            }
        } else {
            let ring_override = if v.get("ring").is_some() {
                if v["ring"].is_null() {
                    Some(None)
                } else if let Some(rname) = v["ring"].as_str() {
                    let trimmed = rname.trim();
                    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") || trimmed.eq_ignore_ascii_case("null") {
                        Some(None)
                    } else {
                        if find_ring_file(trimmed).is_none() {
                            return agent_err(StatusCode::NOT_FOUND, "unknown_asset", "ring not found");
                        }
                        Some(Some(trimmed.to_string()))
                    }
                } else {
                    None
                }
            } else {
                None
            };
            let base = match decode_source(&v) {
                Ok(img) => img,
                Err(r) => return r,
            };
            let close_default = v.get("close_session").and_then(|x| x.as_bool()).unwrap_or(true);
            let live_sync = v
                .get("live_sync")
                .and_then(|x| x.as_bool())
                .unwrap_or(!close_default);
            let label = source_label_from_json(&v);
            let a = store_create(base, ring_override, live_sync, label);
            let id = a.lock().unwrap().id.clone();
            (a, id, true)
        };

        let cleanup_on_err = |created: bool, id: &str| {
            if created {
                store_remove(id);
            }
        };

        // Optional ring update when reusing an existing session
        if !created_inline && v.get("ring").is_some() {
            let rname_opt = if v["ring"].is_null() {
                None
            } else {
                v["ring"]
                    .as_str()
                    .map(str::trim)
                    .filter(|x| !x.is_empty() && !x.eq_ignore_ascii_case("none") && !x.eq_ignore_ascii_case("null"))
                    .map(String::from)
            };
            if let Some(ref rn) = rname_opt
                && find_ring_file(rn).is_none()
            {
                return agent_err(StatusCode::NOT_FOUND, "unknown_asset", "ring not found");
            }
            let mut s = arc.lock().unwrap();
            s.ring = rname_opt.clone();
            s.rev += 1;
            s.log_op("ring", json!({"name": rname_opt}));
        }

        // 2. Background removal ("auto" | "always" | "never" | bool)
        let rm_mode = match v.get("remove_bg") {
            Some(Value::Bool(true)) => "always",
            Some(Value::Bool(false)) => "never",
            Some(Value::String(s)) => match s.trim().to_lowercase().as_str() {
                "always" | "true" | "1" | "force" => "always",
                "never" | "false" | "0" | "skip" => "never",
                _ => "auto",
            },
            _ => {
                if created_inline {
                    "auto"
                } else {
                    "never"
                }
            }
        };
        let edge_blur = v.get("edge_blur").and_then(|x| x.as_f64()).unwrap_or(1.0);

        let should_run_rm = {
            let s = arc.lock().unwrap();
            match rm_mode {
                "always" => true,
                "auto" => !s.bg_removed && !source_already_transparent(&s.original),
                _ => false,
            }
        };

        let mut bg_remove_ran = false;
        if should_run_rm {
            let orig = arc.lock().unwrap().original.clone();
            st.mark_activity();
            let infer_res = crate::infer::remove_background(&st, image::DynamicImage::ImageRgba8(orig), edge_blur);
            st.mark_activity();
            match infer_res {
                Ok(rgba) => {
                    let mut s = arc.lock().unwrap();
                    s.cutout = Some(rgba);
                    s.bg_removed = true;
                    s.showing_original = false;
                    s.portrait_variant = "cutout".to_string();
                    s.image_mask = GrayImage::from_pixel(s.original.width(), s.original.height(), Luma([255u8]));
                    s.img_rev += 1;
                    s.image_mask_rev += 1;
                    s.rev += 1;
                    s.log_op("remove_bg", json!({"edge_blur": edge_blur}));
                    s.push_history();
                    bg_remove_ran = true;
                }
                Err(e) => {
                    cleanup_on_err(created_inline, &sid_str);
                    return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "infer_error", &e.to_string());
                }
            }
        }

        // 3. Optional portrait export
        let mut portrait_out: Option<Value> = None;
        if let Some(p_spec) = v.get("portrait").and_then(|x| x.as_object()) {
            let p_val = Value::Object(p_spec.clone());
            let save_to = p_val
                .get("save_to")
                .or_else(|| p_val.get("output_path"))
                .and_then(|x| x.as_str());
            let fmt = match infer_format_from_spec(&p_val, save_to, "webp") {
                Ok(f) => f,
                Err(msg) => {
                    cleanup_on_err(created_inline, &sid_str);
                    return bad("bad_request", &format!("portrait: {msg}"));
                }
            };
            let size = p_val.get("size").and_then(|x| x.as_u64()).unwrap_or(1024).clamp(256, 4096) as u32;
            let quality = p_val.get("quality").and_then(|x| x.as_u64()).unwrap_or(95).clamp(10, 100) as u8;
            let img = {
                let mut s = arc.lock().unwrap();
                if let Some(var) = p_val.get("variant").and_then(|x| x.as_str()) {
                    s.portrait_variant = if var == "cutout" { "cutout".into() } else { "original".into() };
                } else if s.cutout.is_some() {
                    s.portrait_variant = "cutout".into();
                }
                if let Some(sc) = p_val.get("scale").and_then(|x| x.as_f64()) {
                    s.portrait_scale = sc.clamp(0.05, 8.0) as f32;
                }
                if let Some(rot) = p_val.get("rotation").and_then(|x| x.as_f64()) {
                    s.portrait_rotation = rot as f32;
                }
                if let Some(ox) = p_val.get("offset_x").and_then(|x| x.as_f64()) {
                    s.portrait_x = ox as f32;
                }
                if let Some(oy) = p_val.get("offset_y").and_then(|x| x.as_f64()) {
                    s.portrait_y = oy as f32;
                }
                s.log_op("portrait", p_val.clone());
                render_portrait(&s, size)
            };
            let (bytes, mime) = match crate::imaging::save_image(&image::DynamicImage::ImageRgba8(img), &fmt, quality) {
                Ok(bm) => bm,
                Err(e) => {
                    cleanup_on_err(created_inline, &sid_str);
                    return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "encode_error", &e.to_string());
                }
            };
            if let Some(st_path) = save_to {
                match save_bytes_to_target(&bytes, &fmt, st_path, "tokenatra-portrait") {
                    Ok(saved) => {
                        portrait_out = Some(json!({
                            "path": saved,
                            "format": fmt,
                            "size": size,
                        }));
                    }
                    Err(msg) => {
                        cleanup_on_err(created_inline, &sid_str);
                        return bad("io_error", &format!("portrait: {msg}"));
                    }
                }
            } else {
                let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
                portrait_out = Some(json!({
                    "format": fmt,
                    "size": size,
                    "data_url": format!("data:{mime};base64,{b64}"),
                }));
            }
        }

        // 4. Transform, Preset, Mask Ops, Effects
        {
            let mut s = arc.lock().unwrap();
            if let Some(tr) = v.get("transform")
                && !tr.is_null()
                && let Err((status, code, msg)) = apply_transform_step(&mut s, tr)
            {
                drop(s);
                cleanup_on_err(created_inline, &sid_str);
                return agent_err(status, code, &format!("transform: {msg}"));
            }

            if let Some(pname) = v.get("preset").and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty())
                && let Err((status, code, msg)) = apply_mask_step(&mut s, &json!({"op": "apply_preset", "preset": pname}))
            {
                drop(s);
                cleanup_on_err(created_inline, &sid_str);
                return agent_err(status, code, &msg);
            }

            if let Some(ops_arr) = v.get("mask_ops").and_then(|x| x.as_array()) {
                for (mi, mop) in ops_arr.iter().enumerate() {
                    if let Err((status, code, msg)) = apply_mask_step(&mut s, mop) {
                        drop(s);
                        cleanup_on_err(created_inline, &sid_str);
                        return agent_err(status, code, &format!("mask_ops[{mi}]: {msg}"));
                    }
                }
            }

            if let Some(eff) = v.get("effects")
                && eff.is_object()
            {
                apply_effects_step(&mut s, eff);
            }
        }

        // 5. Token / Subject Exports (`export` object and/or `exports` array)
        let mut export_specs: Vec<Value> = Vec::new();
        if let Some(ex_one) = v.get("export")
            && ex_one.is_object()
        {
            export_specs.push(ex_one.clone());
        }
        if let Some(ex_many) = v.get("exports").and_then(|x| x.as_array()) {
            for item in ex_many {
                if item.is_object() {
                    export_specs.push(item.clone());
                }
            }
        }

        let mut exports_out: Vec<Value> = Vec::with_capacity(export_specs.len());
        for (ei, ex_spec) in export_specs.iter().enumerate() {
            let save_to = ex_spec
                .get("save_to")
                .or_else(|| ex_spec.get("output_path"))
                .and_then(|x| x.as_str());
            let fmt = match infer_format_from_spec(ex_spec, save_to, "webp") {
                Ok(f) => f,
                Err(msg) => {
                    cleanup_on_err(created_inline, &sid_str);
                    return bad("bad_request", &format!("exports[{ei}]: {msg}"));
                }
            };
            let quality = ex_spec.get("quality").and_then(|x| x.as_u64()).unwrap_or(95).clamp(10, 100) as u8;
            let size = ex_spec.get("size").and_then(|x| x.as_u64()).unwrap_or(1024).clamp(256, 2048) as u32;
            let scale_mode = ex_spec.get("scale_mode").and_then(|x| x.as_str()).unwrap_or("1");
            let with_ring = ex_spec.get("with_ring").and_then(|x| x.as_bool()).unwrap_or(true);

            let composed = {
                let s = arc.lock().unwrap();
                compose(&s, size, scale_mode, with_ring)
            };
            let (img, used) = match composed {
                Ok(iu) => iu,
                Err(msg) => {
                    cleanup_on_err(created_inline, &sid_str);
                    return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "compose_error", &msg);
                }
            };
            let out_dim = img.width();
            let (bytes, mime) = match crate::imaging::save_image(&image::DynamicImage::ImageRgba8(img), &fmt, quality) {
                Ok(bm) => bm,
                Err(e) => {
                    cleanup_on_err(created_inline, &sid_str);
                    return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "encode_error", &e.to_string());
                }
            };
            if let Some(st_path) = save_to {
                match save_bytes_to_target(&bytes, &fmt, st_path, "tokenatra-export") {
                    Ok(saved) => {
                        exports_out.push(json!({
                            "path": saved,
                            "format": fmt,
                            "width": out_dim,
                            "height": out_dim,
                            "used_scale": used,
                            "with_ring": with_ring,
                        }));
                    }
                    Err(msg) => {
                        cleanup_on_err(created_inline, &sid_str);
                        return bad("io_error", &format!("exports[{ei}]: {msg}"));
                    }
                }
            } else {
                let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
                exports_out.push(json!({
                    "format": fmt,
                    "width": out_dim,
                    "height": out_dim,
                    "used_scale": used,
                    "with_ring": with_ring,
                    "data_url": format!("data:{mime};base64,{b64}"),
                }));
            }
        }

        // 6. Optional visual preview (regular, example, xray, grid, crop, eyes)
        let mut preview_data_url: Option<String> = None;
        let prev_spec_opt: Option<Value> = match v.get("preview") {
            Some(Value::Bool(true)) => Some(json!({})),
            Some(Value::Object(m)) => Some(Value::Object(m.clone())),
            _ => None,
        };
        if let Some(ps) = prev_spec_opt {
            let opts = match preview_opts_from_json(&ps) {
                Ok(o) => o,
                Err(msg) => {
                    cleanup_on_err(created_inline, &sid_str);
                    return bad("bad_request", &format!("preview: {msg}"));
                }
            };
            let q_webp = if opts.eyes { 95 } else { PREVIEW_WEBP_QUALITY };
            let rendered = {
                let s = arc.lock().unwrap();
                render_preview_image(&s, &opts)
            };
            if let Ok(pimg) = rendered
                && let Ok((bytes, _)) =
                    crate::imaging::save_image(&image::DynamicImage::ImageRgba8(pimg), "webp", q_webp)
            {
                let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
                preview_data_url = Some(format!("data:image/webp;base64,{b64}"));
            }
        }

        let (stats, tr_json, ring_val, bg_rem, do_sync, cur_rev) = {
            let mut s = arc.lock().unwrap();
            if let Some(ls) = v.get("live_sync").and_then(|x| x.as_bool()) {
                s.live_sync = ls;
            }
            (
                inspect_session(&s),
                json!({
                    "scale": s.scale,
                    "rotation": s.rotation,
                    "offset_x": s.offset_x,
                    "offset_y": s.offset_y,
                }),
                s.ring.clone(),
                s.bg_removed,
                s.live_sync,
                s.rev,
            )
        };

        let close_after = v
            .get("close_session")
            .and_then(|x| x.as_bool())
            .unwrap_or(created_inline);
        if close_after {
            store_remove(&sid_str);
            if !created_inline {
                notify_gui_closed(&sid_str);
            }
        } else if do_sync {
            notify_gui_sync(&sid_str, cur_rev, false);
        }

        let mut out = json!({
            "ok": true,
            "session_id": sid_str,
            "session_closed": close_after,
            "bg_removed": bg_rem,
            "bg_remove_ran": bg_remove_ran,
            "ring": ring_val,
            "transform": tr_json,
            "exports": exports_out,
            "mask": stats,
        });
        if let Some(po) = portrait_out {
            out["portrait"] = po;
        }
        if let Some(pdu) = preview_data_url {
            out["preview_data_url"] = Value::String(pdu);
        }
        ok_json(out)
    })
    .await
}

// ---------- Live GUI Sync handlers ----------

fn resolve_ring_sync_info(ring_name: Option<&str>) -> Value {
    let Some(rname) = ring_name.map(str::trim).filter(|s| !s.is_empty()) else {
        return json!({
            "ring": Value::Null,
            "ring_file": Value::Null,
            "mask_file": Value::Null,
            "top_file": Value::Null,
            "ring_scale": 1.0,
        });
    };
    let Some(rp) = find_ring_file(rname) else {
        return json!({
            "ring": rname,
            "ring_file": Value::Null,
            "mask_file": Value::Null,
            "top_file": Value::Null,
            "ring_scale": 1.0,
        });
    };
    let ring_file = rp.file_name().and_then(|s| s.to_str()).map(String::from);
    let mask_file = find_protection_mask_file(Some(rname))
        .and_then(|p| p.file_name().and_then(|s| s.to_str()).map(String::from));
    let meta = crate::foundry_rings::read_ring_meta(&rp);
    let top_file = meta
        .as_ref()
        .and_then(|m| m.top_file.clone())
        .filter(|tf| rp.with_file_name(tf).is_file())
        .or_else(|| {
            crate::foundry_rings::find_companion_file(&rp, "top")
                .and_then(|p| p.file_name().and_then(|s| s.to_str()).map(String::from))
        });
    let ring_scale = meta
        .as_ref()
        .and_then(|m| m.scale)
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0);
    json!({
        "ring": rname,
        "ring_file": ring_file,
        "mask_file": mask_file,
        "top_file": top_file,
        "ring_scale": ring_scale,
    })
}

pub async fn op_gui_sync(
    State(state): State<std::sync::Arc<AppState>>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let v = body.map(|axum::Json(v)| v).unwrap_or_else(|| json!({}));
    let action = v["action"].as_str().unwrap_or("status").to_lowercase();
    let has_window = crate::shell::has_window();
    let in_tray = state.tray_mode();

    match action.as_str() {
        "status" => {
            let sessions: Vec<Value> = with_store(|slots| {
                let mut list: Vec<(Instant, Value)> = slots
                    .iter()
                    .filter_map(|slot| {
                        let s = slot.session.lock().ok()?;
                        Some((
                            s.last_used,
                            json!({
                                "session_id": s.id,
                                "source_label": s.source_label,
                                "width": s.original.width(),
                                "height": s.original.height(),
                                "rev": s.rev,
                                "ring": s.ring,
                                "live_sync": s.live_sync,
                            }),
                        ))
                    })
                    .collect();
                list.sort_by(|a, b| b.0.cmp(&a.0));
                list.into_iter().map(|(_, item)| item).collect()
            });
            ok_json(json!({
                "ok": true,
                "has_window": has_window,
                "in_tray": in_tray,
                "active_sessions": sessions,
            }))
        }
        "detach" => {
            let target_sid = v["session_id"].as_str().map(str::trim).filter(|s| !s.is_empty());
            with_store(|slots| {
                for slot in slots.iter() {
                    if (target_sid.is_none() || target_sid == Some(slot.id.as_str()))
                        && let Ok(mut s) = slot.session.lock()
                    {
                        s.live_sync = false;
                    }
                }
            });
            notify_gui_detached();
            ok_json(json!({
                "ok": true,
                "detached": true,
                "has_window": has_window,
            }))
        }
        "push" => {
            let show_window = v["show_window"].as_bool().unwrap_or(false);
            let target_arc = if let Some(sid) = v["session_id"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                store_get(sid)
            } else {
                with_store(|slots| {
                    slots
                        .iter()
                        .max_by_key(|slot| {
                            slot.session
                                .lock()
                                .map(|s| s.last_used)
                                .unwrap_or_else(|_| Instant::now())
                        })
                        .map(|slot| slot.session.clone())
                })
            };
            let Some(arc) = target_arc else {
                return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no matching session to push");
            };
            let (sid_out, rev_out) = {
                let mut s = arc.lock().unwrap();
                s.live_sync = true;
                (s.id.clone(), s.rev)
            };
            if show_window && has_window {
                crate::gui_control::send(crate::gui_control::GuiCommand::Show);
            }
            notify_gui_sync(&sid_out, rev_out, true);
            ok_json(json!({
                "ok": true,
                "pushed": true,
                "session_id": sid_out,
                "rev": rev_out,
                "has_window": has_window,
                "in_tray": in_tray,
            }))
        }
        other => bad("bad_request", &format!("unknown gui_sync action {other:?}; expected push|detach|status")),
    }
}

pub async fn op_sync_state(Path(sid): Path<String>) -> Response {
    let Some(arc) = store_get(&sid) else {
        return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
    };
    let s = arc.lock().unwrap();
    let ring_info = resolve_ring_sync_info(s.ring.as_deref());
    ok_json(json!({
        "ok": true,
        "session_id": s.id,
        "rev": s.rev,
        "img_rev": s.img_rev,
        "canvas_mask_rev": s.canvas_mask_rev,
        "image_mask_rev": s.image_mask_rev,
        "live_sync": s.live_sync,
        "source_label": s.source_label,
        "image": { "width": s.original.width(), "height": s.original.height() },
        "bg_removed": s.bg_removed,
        "has_cutout": s.cutout.is_some(),
        "showing_original": s.showing_original,
        "protection_enabled": s.protection_enabled,
        "transform": {
            "scale": s.scale,
            "rotation": s.rotation,
            "offset_x": s.offset_x,
            "offset_y": s.offset_y,
        },
        "ring": ring_info["ring"],
        "ring_file": ring_info["ring_file"],
        "mask_file": ring_info["mask_file"],
        "top_file": ring_info["top_file"],
        "ring_scale": ring_info["ring_scale"],
        "effects": {
            "shadow": {
                "enabled": s.shadow_enabled,
                "angle": s.shadow_angle,
                "distance": s.shadow_distance,
                "blur": s.shadow_blur,
                "opacity": s.shadow_opacity,
            },
            "color": {
                "enabled": s.color_enabled,
                "saturation": s.color_sat,
                "lightness": s.color_light,
                "contrast": s.color_contrast,
            },
        },
    }))
}

pub async fn op_sync_layer(Path((sid, layer)): Path<(String, String)>) -> Response {
    run_blocking(move || {
        use image::ImageEncoder as _;
        let Some(arc) = store_get(&sid) else {
            return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
        };
        let mut buf: Vec<u8> = Vec::new();
        match layer.as_str() {
            "original" => {
                let img = { arc.lock().unwrap().original.clone() };
                let enc = image::codecs::png::PngEncoder::new(&mut buf);
                if let Err(e) = enc.write_image(
                    img.as_raw(),
                    img.width(),
                    img.height(),
                    image::ExtendedColorType::Rgba8,
                ) {
                    return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "encode_error", &e.to_string());
                }
            }
            "cutout" => {
                let img_opt = { arc.lock().unwrap().cutout.clone() };
                let Some(img) = img_opt else {
                    return agent_err(StatusCode::NOT_FOUND, "no_cutout", "cutout not available");
                };
                let enc = image::codecs::png::PngEncoder::new(&mut buf);
                if let Err(e) = enc.write_image(
                    img.as_raw(),
                    img.width(),
                    img.height(),
                    image::ExtendedColorType::Rgba8,
                ) {
                    return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "encode_error", &e.to_string());
                }
            }
            "canvas_mask" => {
                let mask = { arc.lock().unwrap().canvas_mask.clone() };
                let enc = image::codecs::png::PngEncoder::new(&mut buf);
                if let Err(e) = enc.write_image(
                    mask.as_raw(),
                    mask.width(),
                    mask.height(),
                    image::ExtendedColorType::L8,
                ) {
                    return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "encode_error", &e.to_string());
                }
            }
            "image_mask" => {
                let mask = { arc.lock().unwrap().image_mask.clone() };
                let enc = image::codecs::png::PngEncoder::new(&mut buf);
                if let Err(e) = enc.write_image(
                    mask.as_raw(),
                    mask.width(),
                    mask.height(),
                    image::ExtendedColorType::L8,
                ) {
                    return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "encode_error", &e.to_string());
                }
            }
            other => return bad("bad_request", &format!("unknown layer {other:?}")),
        }
        file_response(buf, "image/png")
    })
    .await
}

pub async fn op_gui_update(
    Path(sid): Path<String>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let Some(axum::Json(v)) = body else {
        return bad("bad_request", "JSON body required");
    };
    let Some(arc) = store_get(&sid) else {
        return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
    };
    let mut s = arc.lock().unwrap();
    let mut push_hist = false;

    if let Some(sc) = v["scale"].as_f64()
        && sc.is_finite()
        && sc > 0.0
        && sc <= 8.0
        && (s.scale - sc as f32).abs() > 1e-4
    {
        s.scale = sc as f32;
        push_hist = true;
    }
    if let Some(rot) = v["rotation"].as_f64()
        && rot.is_finite()
        && (s.rotation - rot as f32).abs() > 1e-3
    {
        s.rotation = rot as f32;
        push_hist = true;
    }
    if let Some(ox) = v["offset_x"].as_f64()
        && ox.is_finite()
        && (s.offset_x - ox as f32).abs() > 1e-3
    {
        s.offset_x = ox as f32;
        push_hist = true;
    }
    if let Some(oy) = v["offset_y"].as_f64()
        && oy.is_finite()
        && (s.offset_y - oy as f32).abs() > 1e-3
    {
        s.offset_y = oy as f32;
        push_hist = true;
    }
    if let Some(so) = v["showing_original"].as_bool()
        && s.showing_original != so
    {
        s.showing_original = so;
        push_hist = true;
    }
    if let Some(pe) = v["protection_enabled"].as_bool() {
        s.protection_enabled = pe;
    }
    if v.get("ring").is_some() {
        if v["ring"].is_null() {
            s.ring = None;
        } else if let Some(rn) = v["ring"].as_str().map(str::trim).filter(|x| !x.is_empty())
            && find_ring_file(rn).is_some()
        {
            s.ring = Some(rn.to_string());
        }
    }
    if let Some(eff) = v.get("effects").and_then(|e| e.as_object()) {
        if let Some(sh) = eff.get("shadow").and_then(|x| x.as_object()) {
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
        if let Some(cl) = eff.get("color").and_then(|x| x.as_object()) {
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
    }
    if push_hist {
        s.push_history();
    }
    s.rev += 1;
    ok_json(json!({ "ok": true, "rev": s.rev }))
}

pub async fn op_gui_mask(
    Path(sid): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    body: axum::body::Bytes,
) -> Response {
    run_blocking(move || {
        let Some(arc) = store_get(&sid) else {
            return agent_err(StatusCode::NOT_FOUND, "session_not_found", "no such session");
        };
        let target = q.get("target").map(|s| s.to_lowercase()).unwrap_or_else(|| "canvas".into());
        let dyn_img = match image::load_from_memory(&body) {
            Ok(img) => img,
            Err(e) => return bad("bad_image", &format!("mask decode failed: {e}")),
        };
        let gray = if dyn_img.color().has_alpha() {
            let rgba = dyn_img.to_rgba8();
            let (w, h) = rgba.dimensions();
            GrayImage::from_fn(w, h, |x, y| Luma([rgba.get_pixel(x, y).0[3]]))
        } else {
            dyn_img.to_luma8()
        };

        let mut s = arc.lock().unwrap();
        if target == "image" || target == "blue" {
            let (bw, bh) = s.original.dimensions();
            s.image_mask = if gray.dimensions() == (bw, bh) {
                gray
            } else {
                resize_mask(&gray, bw, bh)
            };
            s.image_mask_rev += 1;
        } else {
            let tsize = LOGICAL_U32 * 3;
            s.canvas_mask = if gray.dimensions() == (tsize, tsize) {
                gray
            } else {
                resize_mask(&gray, tsize, tsize)
            };
            s.canvas_mask_rev += 1;
        }
        s.rev += 1;
        s.push_history();
        ok_json(json!({
            "ok": true,
            "rev": s.rev,
            "canvas_mask_rev": s.canvas_mask_rev,
            "image_mask_rev": s.image_mask_rev,
        }))
    })
    .await
}

// ---------- Batch QA Contact Sheets (`POST /agent/v1/qa_sheet`) ----------

fn fit_to_1024(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w == LOGICAL_U32 && h == LOGICAL_U32 {
        return img.clone();
    }
    let scale = LOGICAL / w.max(h).max(1) as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    let resized = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Lanczos3);
    let mut canvas = RgbaImage::new(LOGICAL_U32, LOGICAL_U32);
    let ox = ((LOGICAL_U32 as i64) - (nw as i64)) / 2;
    let oy = ((LOGICAL_U32 as i64) - (nh as i64)) / 2;
    image::imageops::overlay(&mut canvas, &resized, ox, oy);
    canvas
}

fn align_portrait_to_subject_2x(port_rgba: &RgbaImage, subj_rgba: &RgbaImage) -> (RgbaImage, f32, f32, f32, f32) {
    let port_fit = fit_to_1024(port_rgba);
    let subj_512 = image::imageops::resize(subj_rgba, 512, 512, image::imageops::FilterType::Lanczos3);
    let port_512 = image::imageops::resize(&port_fit, 512, 512, image::imageops::FilterType::Lanczos3);

    let candidates = [
        (232u32, 120u32, 280u32, 180u32),
        (226, 135, 286, 205),
        (220, 160, 292, 250),
    ];
    let mut chosen = candidates[0];
    for &cand in &candidates {
        let mut cnt = 0u32;
        for y in cand.1..cand.3 {
            for x in cand.0..cand.2 {
                if subj_512.get_pixel(x, y).0[3] > 240 {
                    cnt += 1;
                }
            }
        }
        if cnt >= 125 {
            chosen = cand;
            break;
        }
    }
    let (tx0, ty0, tx1, ty1) = chosen;
    let mut patch_pts: Vec<(u32, u32, [u8; 3])> = Vec::new();
    for y in ty0..ty1 {
        for x in tx0..tx1 {
            let p = subj_512.get_pixel(x, y);
            if p.0[3] > 240 {
                patch_pts.push((x, y, [p.0[0], p.0[1], p.0[2]]));
            }
        }
    }
    if patch_pts.len() < 16 {
        let small = image::imageops::resize(&port_fit, 512, 512, image::imageops::FilterType::Lanczos3);
        let mut aligned = RgbaImage::new(LOGICAL_U32, LOGICAL_U32);
        image::imageops::overlay(&mut aligned, &small, 256, 256);
        return (aligned, 1.0, 0.0, 0.0, 999.0);
    }

    let mut best_err = f32::MAX;
    let mut best_params = (1.22f32, 0.0f32, -40.0f32);

    // Coarse search on 512x512 2x canvas
    for si in 0..=35 {
        let sc = 0.90 + (si as f32) * 0.02;
        let new_sz = ((512.0 * sc * 0.5).round() as i32).max(32) as u32;
        let pr = image::imageops::resize(&port_512, new_sz, new_sz, image::imageops::FilterType::Triangle);
        for dx_i in (-120..=120).step_by(8) {
            let dx = dx_i as f32;
            let x_tl = (256.0 - new_sz as f32 * 0.5 + dx * 0.25).round() as i32;
            if (tx0 as i32) < x_tl || (tx1 as i32) > x_tl + new_sz as i32 {
                continue;
            }
            for dy_i in (-260..=60).step_by(8) {
                let dy = dy_i as f32;
                let y_tl = (256.0 - new_sz as f32 * 0.5 + dy * 0.25).round() as i32;
                if (ty0 as i32) < y_tl || (ty1 as i32) > y_tl + new_sz as i32 {
                    continue;
                }
                let mut sum_err = 0.0f32;
                for &(sx, sy, rgb) in &patch_pts {
                    let px = (sx as i32 - x_tl) as u32;
                    let py = (sy as i32 - y_tl) as u32;
                    let q = pr.get_pixel(px, py).0;
                    sum_err += (q[0] as f32 - rgb[0] as f32).abs()
                        + (q[1] as f32 - rgb[1] as f32).abs()
                        + (q[2] as f32 - rgb[2] as f32).abs();
                }
                let mae = sum_err / (patch_pts.len() as f32 * 3.0);
                if mae < best_err {
                    best_err = mae;
                    best_params = (sc, dx, dy);
                }
            }
        }
    }

    // Fine refinement on 512x512 2x canvas
    let (sc0, dx0, dy0) = best_params;
    for si in -5..=5 {
        let sc = (sc0 + si as f32 * 0.005).clamp(0.8, 2.0);
        let new_sz = ((512.0 * sc * 0.5).round() as i32).max(32) as u32;
        let pr = image::imageops::resize(&port_512, new_sz, new_sz, image::imageops::FilterType::Triangle);
        for dxi in -8..=8 {
            let dx = dx0 + dxi as f32 * 1.5;
            let x_tl = (256.0 - new_sz as f32 * 0.5 + dx * 0.25).round() as i32;
            if (tx0 as i32) < x_tl || (tx1 as i32) > x_tl + new_sz as i32 {
                continue;
            }
            for dyi in -8..=8 {
                let dy = dy0 + dyi as f32 * 1.5;
                let y_tl = (256.0 - new_sz as f32 * 0.5 + dy * 0.25).round() as i32;
                if (ty0 as i32) < y_tl || (ty1 as i32) > y_tl + new_sz as i32 {
                    continue;
                }
                let mut sum_err = 0.0f32;
                for &(sx, sy, rgb) in &patch_pts {
                    let px = (sx as i32 - x_tl) as u32;
                    let py = (sy as i32 - y_tl) as u32;
                    let q = pr.get_pixel(px, py).0;
                    sum_err += (q[0] as f32 - rgb[0] as f32).abs()
                        + (q[1] as f32 - rgb[1] as f32).abs()
                        + (q[2] as f32 - rgb[2] as f32).abs();
                }
                let mae = sum_err / (patch_pts.len() as f32 * 3.0);
                if mae < best_err {
                    best_err = mae;
                    best_params = (sc, dx, dy);
                }
            }
        }
    }

    let (sc, dx, dy) = best_params;
    let new_sz = ((1024.0 * sc * 0.5).round() as i64).max(16) as u32;
    let p_res = image::imageops::resize(&port_fit, new_sz, new_sz, image::imageops::FilterType::Lanczos3);
    let x_tl = (512.0 - new_sz as f32 * 0.5 + dx * 0.5).round() as i64;
    let y_tl = (512.0 - new_sz as f32 * 0.5 + dy * 0.5).round() as i64;
    let mut aligned = RgbaImage::new(LOGICAL_U32, LOGICAL_U32);
    image::imageops::overlay(&mut aligned, &p_res, x_tl, y_tl);
    (aligned, sc, dx, dy, best_err)
}

fn fill_solid_rect(img: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32, col: Rgba<u8>) {
    let (w, h) = img.dimensions();
    let xa = x0.min(w);
    let xb = x1.min(w);
    let ya = y0.min(h);
    let yb = y1.min(h);
    for y in ya..yb {
        for x in xa..xb {
            img.put_pixel(x, y, col);
        }
    }
}

fn make_qa_clean_base(ring_512: Option<&RgbaImage>) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(LOGICAL_U32, LOGICAL_U32, Rgba([30, 26, 40, 255]));
    for (x, y, p) in img.enumerate_pixels_mut() {
        let d = (x as f32 - 512.0).hypot(y as f32 - 512.0);
        if d <= 185.0 {
            *p = Rgba([16, 15, 22, 255]);
        }
    }
    if let Some(ring) = ring_512 {
        image::imageops::overlay(&mut img, ring, 256, 256);
    }
    img
}

fn make_qa_xray_base(ring_512_rim: Option<&RgbaImage>) -> RgbaImage {
    let mut img = fill_teal_checkerboard(LOGICAL_U32, LOGICAL_U32, 16);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let d = (x as f32 - 512.0).hypot(y as f32 - 512.0);
        if d <= 171.5 {
            let dark = (((x / 16) + (y / 16)) & 1) == 0;
            *p = if dark {
                Rgba([22, 30, 48, 255])
            } else {
                Rgba([16, 22, 38, 255])
            };
        }
    }
    if let Some(rim) = ring_512_rim {
        let mut semi = rim.clone();
        for p in semi.pixels_mut() {
            p.0[3] = ((p.0[3] as f32) * 0.45).round() as u8;
        }
        image::imageops::overlay(&mut img, &semi, 256, 256);
    }
    img
}

fn load_image_from_path(path: &std::path::Path) -> Result<RgbaImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dyn_img = crate::imaging::open_image(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(dyn_img.to_rgba8())
}

fn qa_status_tag(qa: &Value) -> (&'static str, [u8; 4]) {
    let clean = qa["clean"].as_bool().unwrap_or(false);
    if clean {
        return ("OK", [120, 245, 145, 255]);
    }
    let mut tags: Vec<&str> = Vec::new();
    if qa["detached_islands"].as_array().is_some_and(|a| !a.is_empty()) {
        tags.push("ISLAND");
    }
    if qa["bottom_ring_gap"].as_bool().unwrap_or(false) {
        tags.push("GAP");
    }
    if qa["step_564"].as_array().is_some_and(|a| !a.is_empty()) {
        tags.push("STEP");
    }
    if tags.is_empty() {
        ("WARN", [255, 210, 90, 255])
    } else if tags.len() == 1 {
        (tags[0], [255, 110, 110, 255])
    } else {
        ("ISSUES", [255, 110, 110, 255])
    }
}

pub async fn op_qa_sheet(
    State(_state): State<std::sync::Arc<AppState>>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let v = body.map(|axum::Json(v)| v).unwrap_or_else(|| json!({}));
    run_blocking(move || {
        let mode = v["mode"].as_str().unwrap_or("xray").to_lowercase();
        if !matches!(mode.as_str(), "xray" | "eyes" | "showcase") {
            return bad("bad_request", "mode must be xray|eyes|showcase");
        }
        let default_per_page = if mode == "xray" { 4 } else { 16 };
        let per_page = (v["per_page"].as_u64().unwrap_or(default_per_page) as usize).clamp(1, 32);
        let page = (v["page"].as_u64().unwrap_or(1) as usize).max(1);
        let max_width = (v["max_width"].as_u64().unwrap_or(1536) as u32).clamp(512, 3072);
        let ring_name = v["ring"].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("rune-steel");
        let prefix = v["prefix"].as_str().unwrap_or("");

        // Optional transforms map from transforms_json path
        let mut transforms_map: serde_json::Map<String, Value> = serde_json::Map::new();
        if let Some(tpath) = v["transforms_json"].as_str().map(str::trim).filter(|s| !s.is_empty())
            && let Ok(raw) = std::fs::read_to_string(tpath)
            && let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&raw)
        {
            transforms_map = m;
        }

        enum EntrySource {
            Session(Arc<Mutex<AgentSession>>),
            Files {
                name: String,
                subject_path: Option<std::path::PathBuf>,
                portrait_path: Option<std::path::PathBuf>,
                image_path: Option<std::path::PathBuf>,
                tf: Option<(f32, f32, f32)>,
            },
        }

        let parse_tf = |obj: &Value| -> Option<(f32, f32, f32)> {
            let sc = obj.get("scale").and_then(|x| x.as_f64())? as f32;
            let dx = obj.get("offset_x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            let dy = obj.get("offset_y").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            Some((sc, dx, dy))
        };

        let mut entries: Vec<EntrySource> = Vec::new();
        if let Some(sids) = v["session_ids"].as_array() {
            for s in sids {
                if let Some(sid) = s.as_str()
                    && let Some(arc) = store_get(sid)
                {
                    entries.push(EntrySource::Session(arc));
                }
            }
        } else if let Some(items) = v["items"].as_array() {
            for (idx, it) in items.iter().enumerate() {
                let subj_p = it.get("subject_path").and_then(|x| x.as_str()).map(std::path::PathBuf::from);
                let port_p = it.get("portrait_path").and_then(|x| x.as_str()).map(std::path::PathBuf::from);
                let img_p = it.get("image_path").and_then(|x| x.as_str()).map(std::path::PathBuf::from);
                let name = it
                    .get("name")
                    .and_then(|x| x.as_str())
                    .map(String::from)
                    .or_else(|| {
                        subj_p
                            .as_ref()
                            .or(img_p.as_ref())
                            .or(port_p.as_ref())
                            .and_then(|p| p.file_name().and_then(|s| s.to_str()).map(String::from))
                    })
                    .unwrap_or_else(|| format!("item_{}", idx + 1));
                let tf = it.get("transform").and_then(parse_tf);
                entries.push(EntrySource::Files {
                    name,
                    subject_path: subj_p,
                    portrait_path: port_p,
                    image_path: img_p,
                    tf,
                });
            }
        } else if let Some(paths_arr) = v["image_paths"].as_array().or_else(|| v["subject_paths"].as_array()) {
            for p_val in paths_arr {
                if let Some(ps) = p_val.as_str() {
                    let p = std::path::PathBuf::from(ps);
                    let name = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("image")
                        .to_string();
                    entries.push(EntrySource::Files {
                        name,
                        subject_path: (mode != "eyes").then(|| p.clone()),
                        portrait_path: None,
                        image_path: (mode == "eyes").then_some(p),
                        tf: None,
                    });
                }
            }
        } else if mode == "eyes"
            && let Some(idir) = v["input_dir"].as_str().map(str::trim).filter(|s| !s.is_empty())
        {
            let Ok(rd) = std::fs::read_dir(idir) else {
                return bad("file_not_found", &format!("cannot read input_dir: {idir}"));
            };
            let mut files: Vec<_> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.is_file()
                        && matches!(
                            super::ext_of(&p.to_string_lossy()).as_str(),
                            "webp" | "png" | "jpg" | "jpeg"
                        )
                        && p.file_name()
                            .and_then(|s| s.to_str())
                            .is_some_and(|fnm| prefix.is_empty() || fnm.starts_with(prefix))
                })
                .collect();
            files.sort();
            for p in files {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("img").to_string();
                entries.push(EntrySource::Files {
                    name,
                    subject_path: None,
                    portrait_path: None,
                    image_path: Some(p),
                    tf: None,
                });
            }
        } else if let Some(sdir) = v["subjects_dir"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            let Ok(rd) = std::fs::read_dir(sdir) else {
                return bad("file_not_found", &format!("cannot read subjects_dir: {sdir}"));
            };
            let pdir = v["portraits_dir"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(std::path::PathBuf::from);
            let mut files: Vec<_> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.is_file()
                        && matches!(super::ext_of(&p.to_string_lossy()).as_str(), "webp" | "png")
                        && p.file_name()
                            .and_then(|s| s.to_str())
                            .is_some_and(|fnm| prefix.is_empty() || fnm.starts_with(prefix))
                })
                .collect();
            files.sort();
            for p in files {
                let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("token").to_string();
                let port_p = pdir.as_ref().and_then(|pd| {
                    let cand = pd.join(&fname);
                    if cand.is_file() {
                        Some(cand)
                    } else {
                        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                        ["webp", "png", "jpg", "jpeg"]
                            .iter()
                            .map(|ext| pd.join(format!("{stem}.{ext}")))
                            .find(|cp| cp.is_file())
                    }
                });
                let tf = transforms_map.get(&fname).and_then(parse_tf);
                entries.push(EntrySource::Files {
                    name: fname,
                    subject_path: Some(p),
                    portrait_path: port_p,
                    image_path: None,
                    tf,
                });
            }
        }

        if entries.is_empty() {
            return bad(
                "bad_request",
                "no matching sessions or images provided (pass session_ids, items, image_paths, subjects_dir, or input_dir)",
            );
        }

        let total_items = entries.len();
        let total_pages = total_items.div_ceil(per_page).max(1);
        let page_clamped = page.min(total_pages);
        let start = (page_clamped - 1) * per_page;
        let end = (start + per_page).min(total_items);
        let chunk = &entries[start..end];

        let (ring_full_opt, ring_top_opt, _ring_scale) = load_ring_assets(ring_name);
        let ring_512_full = ring_full_opt
            .as_ref()
            .map(|r| image::imageops::resize(r, 512, 512, image::imageops::FilterType::Lanczos3));
        let ring_512_top = ring_top_opt
            .as_ref()
            .map(|r| image::imageops::resize(r, 512, 512, image::imageops::FilterType::Lanczos3));
        let ring_512_rim = ring_512_full.as_ref().map(|r| {
            let mut rim = r.clone();
            for (x, y, p) in rim.enumerate_pixels_mut() {
                if (x as f32 - 256.0).hypot(y as f32 - 256.0) < 171.5 {
                    p.0[3] = 0;
                }
            }
            rim
        });
        let clean_base = make_qa_clean_base(ring_512_full.as_ref());
        let xray_base = make_qa_xray_base(ring_512_rim.as_ref());

        let mut items_meta: Vec<Value> = Vec::with_capacity(chunk.len());
        let mut sheet: RgbaImage;

        match mode.as_str() {
            "xray" => {
                let cols = 2u32;
                let cell_w = 1100u32;
                let cell_h = 580u32;
                let rows = (chunk.len() as u32).div_ceil(cols).max(1);
                sheet = RgbaImage::from_pixel(cols * cell_w, rows * cell_h, Rgba([18, 16, 24, 255]));

                for (i, item) in chunk.iter().enumerate() {
                    let g_idx = start + i + 1;
                    let r = (i as u32) / cols;
                    let c = (i as u32) % cols;
                    let x0 = c * cell_w;
                    let y0 = r * cell_h;

                    let (label, c_img, x_img, tf_info, qa_val, sid_opt) = match item {
                        EntrySource::Session(arc) => {
                            let s = arc.lock().unwrap();
                            let name = s.source_label.clone().unwrap_or_else(|| s.id[..8.min(s.id.len())].to_string());
                            let subj_s3 = render_subject_on_s3(&s, 512);
                            let subj_2x = image::imageops::crop_imm(&subj_s3, 256, 256, 1024, 1024).to_image();
                            let unmasked_s3 = render_unmasked_subject_on_s3(&s, 512);
                            let unmasked_2x = image::imageops::crop_imm(&unmasked_s3, 256, 256, 1024, 1024).to_image();

                            let mut c_img = clean_base.clone();
                            image::imageops::overlay(&mut c_img, &subj_2x, 0, 0);
                            if let Some(ref top) = ring_512_top {
                                image::imageops::overlay(&mut c_img, top, 256, 256);
                            }

                            let mut x_img = xray_base.clone();
                            let mut ghost = RgbaImage::new(LOGICAL_U32, LOGICAL_U32);
                            for (gp, (up, mp)) in ghost.pixels_mut().zip(unmasked_2x.pixels().zip(subj_2x.pixels())) {
                                if up.0[3] > 30 && mp.0[3] < 80 {
                                    let gr = (up.0[0] as f32 * 0.45 + 255.0 * 0.55).round().clamp(0.0, 255.0) as u8;
                                    let gg = (up.0[1] as f32 * 0.45 + 50.0 * 0.55).round().clamp(0.0, 255.0) as u8;
                                    let gb = (up.0[2] as f32 * 0.45 + 120.0 * 0.55).round().clamp(0.0, 255.0) as u8;
                                    let ga = (up.0[3] as f32 * 0.72).round().clamp(0.0, 210.0) as u8;
                                    *gp = Rgba([gr, gg, gb, ga]);
                                }
                            }
                            image::imageops::overlay(&mut x_img, &ghost, 0, 0);
                            image::imageops::overlay(&mut x_img, &subj_2x, 0, 0);
                            draw_xray_markers_in_view(&mut x_img, -512.0, -512.0, 1536.0, 1536.0);

                            let (qa_json, _) = compute_mask_qa(&s);
                            let tf_txt = format!("s={:.2} dx={:.0} dy={:.0}", s.scale, s.offset_x, s.offset_y);
                            (name, c_img, x_img, tf_txt, qa_json, Some(s.id.clone()))
                        }
                        EntrySource::Files { name, subject_path, portrait_path, tf, .. } => {
                            let Some(sp) = subject_path else {
                                return bad("bad_request", &format!("missing subject_path for {name}"));
                            };
                            let subj_raw = match load_image_from_path(sp) {
                                Ok(im) => im,
                                Err(e) => return bad("file_not_found", &e),
                            };
                            let subj_2x = image::imageops::resize(&subj_raw, 1024, 1024, image::imageops::FilterType::Lanczos3);

                            let mut qa_s3 = RgbaImage::new(1536, 1536);
                            image::imageops::overlay(&mut qa_s3, &subj_2x, 256, 256);
                            let (qa_json, _) = compute_mask_qa_from_s3(&qa_s3, true);

                            let mut c_img = clean_base.clone();
                            image::imageops::overlay(&mut c_img, &subj_2x, 0, 0);
                            if let Some(ref top) = ring_512_top {
                                image::imageops::overlay(&mut c_img, top, 256, 256);
                            }

                            let mut x_img = xray_base.clone();
                            let mut tf_txt = String::new();
                            if let Some(pp) = portrait_path
                                && let Ok(port_raw) = load_image_from_path(pp)
                            {
                                let (aligned_port, sc, dx, dy, err) = if let Some((sc, dx, dy)) = *tf {
                                    let p_fit = fit_to_1024(&port_raw);
                                    let nsz = ((1024.0 * sc * 0.5).round() as i64).max(16) as u32;
                                    let p_res = image::imageops::resize(&p_fit, nsz, nsz, image::imageops::FilterType::Lanczos3);
                                    let mut al = RgbaImage::new(LOGICAL_U32, LOGICAL_U32);
                                    image::imageops::overlay(
                                        &mut al,
                                        &p_res,
                                        (512.0 - nsz as f32 * 0.5 + dx * 0.5).round() as i64,
                                        (512.0 - nsz as f32 * 0.5 + dy * 0.5).round() as i64,
                                    );
                                    (al, sc, dx, dy, 0.0f32)
                                } else {
                                    align_portrait_to_subject_2x(&port_raw, &subj_2x)
                                };
                                let mut ghost = RgbaImage::new(LOGICAL_U32, LOGICAL_U32);
                                for (gp, (ap, spx)) in ghost.pixels_mut().zip(aligned_port.pixels().zip(subj_2x.pixels())) {
                                    if ap.0[3] > 30 && spx.0[3] < 80 {
                                        let gr = (ap.0[0] as f32 * 0.45 + 255.0 * 0.55).round().clamp(0.0, 255.0) as u8;
                                        let gg = (ap.0[1] as f32 * 0.45 + 50.0 * 0.55).round().clamp(0.0, 255.0) as u8;
                                        let gb = (ap.0[2] as f32 * 0.45 + 120.0 * 0.55).round().clamp(0.0, 255.0) as u8;
                                        let ga = (ap.0[3] as f32 * 0.72).round().clamp(0.0, 210.0) as u8;
                                        *gp = Rgba([gr, gg, gb, ga]);
                                    }
                                }
                                image::imageops::overlay(&mut x_img, &ghost, 0, 0);
                                tf_txt = format!("s={sc:.2} dx={dx:.0} dy={dy:.0} e={err:.1}");
                            }
                            image::imageops::overlay(&mut x_img, &subj_2x, 0, 0);
                            draw_xray_markers_in_view(&mut x_img, -512.0, -512.0, 1536.0, 1536.0);
                            (name.clone(), c_img, x_img, tf_txt, qa_json, None)
                        }
                    };

                    let c_crop = image::imageops::crop_imm(&c_img, 96, 64, 832, 832).to_image();
                    let x_crop = image::imageops::crop_imm(&x_img, 96, 64, 832, 832).to_image();
                    let c_fin = image::imageops::resize(&c_crop, 536, 536, image::imageops::FilterType::Lanczos3);
                    let x_fin = image::imageops::resize(&x_crop, 536, 536, image::imageops::FilterType::Lanczos3);

                    image::imageops::overlay(&mut sheet, &c_fin, (x0 + 10) as i64, (y0 + 8) as i64);
                    image::imageops::overlay(&mut sheet, &x_fin, (x0 + 554) as i64, (y0 + 8) as i64);

                    fill_solid_rect(&mut sheet, x0 + 10, y0 + 546, x0 + 1090, y0 + 574, Rgba([10, 10, 14, 255]));
                    let (tag, tag_col) = qa_status_tag(&qa_val);
                    let lbl = if tf_info.is_empty() {
                        format!("[#{g_idx}] {label}")
                    } else {
                        format!("[#{g_idx}] {label} ({tf_info})")
                    };
                    let lbl_trunc: String = lbl.chars().take(68).collect();
                    draw_text_5x7(&mut sheet, &lbl_trunc, (x0 + 18) as i64, (y0 + 553) as i64, 2, [240, 240, 245, 255], None);
                    let tag_w = (tag.len() as i64) * 12;
                    draw_text_5x7(&mut sheet, tag, (x0 + 1082) as i64 - tag_w, (y0 + 553) as i64, 2, tag_col, None);

                    items_meta.push(json!({
                        "index": g_idx,
                        "name": label,
                        "session_id": sid_opt,
                        "qa": qa_val,
                    }));
                }
            }
            "eyes" => {
                let cols = 4u32;
                let cell_w = 440u32;
                let cell_h = 300u32;
                let rows = (chunk.len() as u32).div_ceil(cols).max(1);
                sheet = RgbaImage::from_pixel(cols * cell_w, rows * cell_h, Rgba([20, 18, 26, 255]));

                for (i, item) in chunk.iter().enumerate() {
                    let g_idx = start + i + 1;
                    let r = (i as u32) / cols;
                    let c = (i as u32) % cols;
                    let x0 = c * cell_w;
                    let y0 = r * cell_h;

                    let (label, eye_crop, sid_opt) = match item {
                        EntrySource::Session(arc) => {
                            let s = arc.lock().unwrap();
                            let name = s.source_label.clone().unwrap_or_else(|| s.id[..8.min(s.id.len())].to_string());
                            let ec = render_eye_crop(&s.original, s.cutout.as_ref(), None);
                            (name, ec, Some(s.id.clone()))
                        }
                        EntrySource::Files { name, image_path, portrait_path, subject_path, .. } => {
                            let Some(p) = image_path.as_ref().or(portrait_path.as_ref()).or(subject_path.as_ref()) else {
                                return bad("bad_request", &format!("missing image_path for {name}"));
                            };
                            let im = match load_image_from_path(p) {
                                Ok(im) => im,
                                Err(e) => return bad("file_not_found", &e),
                            };
                            (name.clone(), render_eye_crop(&im, None, None), None)
                        }
                    };
                    let resized = image::imageops::resize(&eye_crop, 420, 252, image::imageops::FilterType::Lanczos3);
                    image::imageops::overlay(&mut sheet, &resized, (x0 + 10) as i64, (y0 + 8) as i64);
                    fill_solid_rect(&mut sheet, x0 + 10, y0 + 264, x0 + 430, y0 + 292, Rgba([10, 10, 15, 255]));
                    let lbl: String = format!("[#{g_idx}] {label}").chars().take(32).collect();
                    draw_text_5x7(&mut sheet, &lbl, (x0 + 16) as i64, (y0 + 271) as i64, 2, [240, 240, 245, 255], None);
                    items_meta.push(json!({ "index": g_idx, "name": label, "session_id": sid_opt }));
                }
            }
            "showcase" | _ => {
                let cols = 4u32;
                let cell_w = 460u32;
                let cell_h = 488u32;
                let rows = (chunk.len() as u32).div_ceil(cols).max(1);
                sheet = RgbaImage::from_pixel(cols * cell_w, rows * cell_h, Rgba([20, 18, 28, 255]));

                for (i, item) in chunk.iter().enumerate() {
                    let g_idx = start + i + 1;
                    let r = (i as u32) / cols;
                    let c = (i as u32) % cols;
                    let x0 = c * cell_w + 10;
                    let y0 = r * cell_h + 10;

                    let (label, comp, sid_opt) = match item {
                        EntrySource::Session(arc) => {
                            let s = arc.lock().unwrap();
                            let name = s.source_label.clone().unwrap_or_else(|| s.id[..8.min(s.id.len())].to_string());
                            let subj_s3 = render_subject_on_s3(&s, 512);
                            let subj_2x = image::imageops::crop_imm(&subj_s3, 256, 256, 1024, 1024).to_image();
                            let mut comp = clean_base.clone();
                            image::imageops::overlay(&mut comp, &subj_2x, 0, 0);
                            if let Some(ref top) = ring_512_top {
                                image::imageops::overlay(&mut comp, top, 256, 256);
                            }
                            (name, comp, Some(s.id.clone()))
                        }
                        EntrySource::Files { name, subject_path, image_path, .. } => {
                            let Some(p) = subject_path.as_ref().or(image_path.as_ref()) else {
                                return bad("bad_request", &format!("missing subject_path for {name}"));
                            };
                            let subj_raw = match load_image_from_path(p) {
                                Ok(im) => im,
                                Err(e) => return bad("file_not_found", &e),
                            };
                            let subj_2x = image::imageops::resize(&subj_raw, 1024, 1024, image::imageops::FilterType::Lanczos3);
                            let mut comp = clean_base.clone();
                            image::imageops::overlay(&mut comp, &subj_2x, 0, 0);
                            if let Some(ref top) = ring_512_top {
                                image::imageops::overlay(&mut comp, top, 256, 256);
                            }
                            (name.clone(), comp, None)
                        }
                    };
                    let cropped = image::imageops::crop_imm(&comp, 148, 120, 728, 728).to_image();
                    let tile = image::imageops::resize(&cropped, 440, 440, image::imageops::FilterType::Lanczos3);
                    image::imageops::overlay(&mut sheet, &tile, x0 as i64, y0 as i64);
                    fill_solid_rect(&mut sheet, x0, y0 + 442, x0 + 440, y0 + 474, Rgba([10, 10, 15, 255]));
                    let lbl: String = format!("[#{g_idx}] {label}").chars().take(34).collect();
                    draw_text_5x7(&mut sheet, &lbl, (x0 + 8) as i64, (y0 + 452) as i64, 2, [240, 240, 245, 255], None);
                    items_meta.push(json!({ "index": g_idx, "name": label, "session_id": sid_opt }));
                }
            }
        }

        if sheet.width() > max_width {
            let nh = ((sheet.height() as f32 * (max_width as f32 / sheet.width() as f32)).round() as u32).max(1);
            sheet = image::imageops::resize(&sheet, max_width, nh, image::imageops::FilterType::Lanczos3);
        }

        let save_to = v
            .get("save_to")
            .or_else(|| v.get("output_path"))
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let fmt = match infer_format_from_spec(&v, save_to, "webp") {
            Ok(f) => f,
            Err(msg) => return bad("bad_request", &msg),
        };
        let (bytes, mime) = match crate::imaging::save_image(
            &image::DynamicImage::ImageRgba8(sheet),
            &fmt,
            PREVIEW_WEBP_QUALITY,
        ) {
            Ok(bm) => bm,
            Err(e) => return agent_err(StatusCode::INTERNAL_SERVER_ERROR, "encode_error", &e.to_string()),
        };

        let mut saved_path: Option<String> = None;
        if let Some(st) = save_to {
            match save_bytes_to_target(&bytes, &fmt, st, &format!("qa-{mode}-p{page_clamped}")) {
                Ok(p) => saved_path = Some(p),
                Err(msg) => return bad("io_error", &msg),
            }
        }

        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
        ok_json(json!({
            "ok": true,
            "mode": mode,
            "page": page_clamped,
            "per_page": per_page,
            "total_pages": total_pages,
            "total_items": total_items,
            "count": chunk.len(),
            "saved_to": saved_path,
            "items": items_meta,
            "image_data_url": format!("data:{mime};base64,{b64}"),
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_s3_canvas_with_circle(cx: i32, cy: i32, r: i32) -> RgbaImage {
        let mut img = RgbaImage::new(1536, 1536);
        let r2 = r * r;
        for py in (cy - r).max(0)..=(cy + r).min(1535) {
            for px in (cx - r).max(0)..=(cx + r).min(1535) {
                let dx = px - cx;
                let dy = py - cy;
                if dx * dx + dy * dy <= r2 {
                    img.put_pixel(px as u32, py as u32, Rgba([200, 180, 160, 255]));
                }
            }
        }
        img
    }

    #[test]
    fn qa_linter_detects_clean_and_defects() {
        // 1. Clean token filling lower inner rim (center (512,512) -> px=(768,768), r=180 -> lr=360)
        let clean_img = test_s3_canvas_with_circle(768, 768, 180);
        let (qa_clean, warns_clean) = compute_mask_qa_from_s3(&clean_img, true);
        assert_eq!(qa_clean["clean"], Value::Bool(true), "warns: {warns_clean:?}");
        assert_eq!(qa_clean["bottom_ring_gap"], Value::Bool(false));

        // 2. Detached island outside main body
        let mut island_img = clean_img.clone();
        for py in 760..780 {
            for px in 1050..1070 {
                island_img.put_pixel(px, py, Rgba([220, 120, 90, 255]));
            }
        }
        let (qa_isl, _) = compute_mask_qa_from_s3(&island_img, true);
        assert_eq!(qa_isl["clean"], Value::Bool(false));
        assert_eq!(qa_isl["detached_islands"].as_array().unwrap().len(), 1);

        // 3. Bottom ring gap (short figure stopping above y=780 -> py=902)
        let short_img = test_s3_canvas_with_circle(768, 680, 150);
        let (qa_gap, _) = compute_mask_qa_from_s3(&short_img, true);
        assert_eq!(qa_gap["clean"], Value::Bool(false));
        assert_eq!(qa_gap["bottom_ring_gap"], Value::Bool(true));
    }

    #[test]
    fn crop_box_validation() {
        let ok = parse_crop_box(&json!([100, 400, 500, 800])).unwrap().unwrap();
        assert_eq!(ok, [100.0, 400.0, 500.0, 800.0]);
        let ok_str = parse_crop_box(&json!("-100, 200, 400, 700")).unwrap().unwrap();
        assert_eq!(ok_str, [-100.0, 200.0, 400.0, 700.0]);
        assert!(parse_crop_box(&json!([100, 100, 110, 110])).is_err());
    }
}
