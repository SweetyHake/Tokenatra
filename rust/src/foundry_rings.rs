use std::collections::HashMap;
use std::path::{Path, PathBuf};

use image::{DynamicImage, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct RingMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Clone, Debug)]
struct FrameRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Clone, Debug)]
struct ColorBand {
    start: f32,
    end: f32,
}

#[derive(Clone, Debug)]
pub struct VariantSpec {
    #[allow(dead_code)]
    pub name: String,
    pub w: u32,
    pub h: u32,
    ring_frame: FrameRect,
    bkg_frame: Option<FrameRect>,
    msk_frame: Option<FrameRect>,
    top_frame: Option<(Option<String>, FrameRect)>,
    ring_color: [f32; 3],
    bg_color: [f32; 3],
    color_band: ColorBand,
}

#[derive(Clone, Debug)]
pub struct ParsedFoundrySheet {
    pub meta_image: String,
    pub scale: f32,
    pub best_variant: VariantSpec,
}

#[derive(Clone, Debug, Serialize)]
pub struct FoundryRingPreview {
    pub json_path: String,
    pub stem: String,
    pub suggested_name: String,
    pub suggested_name_en: Option<String>,
    pub scale: f32,
    pub has_top: bool,
    pub size: u32,
    pub atlas_image: String,
}

pub fn read_ring_meta(ring_path: &Path) -> Option<RingMeta> {
    let stem = ring_path.file_stem()?.to_str()?;
    let meta_path = ring_path.with_file_name(format!("{stem}.meta.json"));
    let bytes = std::fs::read(meta_path).ok()?;
    serde_json::from_slice::<RingMeta>(&bytes).ok()
}

pub fn write_ring_meta(ring_path: &Path, meta: &RingMeta) -> std::io::Result<()> {
    let stem = ring_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ring");
    let meta_path = ring_path.with_file_name(format!("{stem}.meta.json"));
    let json = serde_json::to_vec_pretty(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(meta_path, json)
}

pub fn find_companion_file(ring_path: &Path, suffix: &str) -> Option<PathBuf> {
    let stem = ring_path.file_stem()?.to_str()?;
    let orig_ext = ring_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("webp")
        .to_lowercase();
    for ext in [orig_ext.as_str(), "webp", "png", "jpg", "jpeg"] {
        let p = ring_path.with_file_name(format!("{stem}.{suffix}.{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

pub fn parse_hex_rgb01(val: Option<&Value>, fallback: [f32; 3]) -> [f32; 3] {
    let Some(v) = val else {
        return fallback;
    };
    if let Some(n) = v.as_u64() {
        let n = n as u32;
        return [
            ((n >> 16) & 255) as f32 / 255.0,
            ((n >> 8) & 255) as f32 / 255.0,
            (n & 255) as f32 / 255.0,
        ];
    }
    if let Some(f) = v.as_f64()
        && f.is_finite()
        && f >= 0.0
    {
        let n = f as u32;
        return [
            ((n >> 16) & 255) as f32 / 255.0,
            ((n >> 8) & 255) as f32 / 255.0,
            (n & 255) as f32 / 255.0,
        ];
    }
    let Some(raw) = v.as_str() else {
        return fallback;
    };
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        s = rest;
    } else if let Some(rest) = s.strip_prefix('#') {
        s = rest;
    } else {
        return fallback;
    }
    if s.len() == 3 {
        let b: Vec<u8> = s.bytes().collect();
        let expanded = format!(
            "{}{}{}{}{}{}",
            b[0] as char,
            b[0] as char,
            b[1] as char,
            b[1] as char,
            b[2] as char,
            b[2] as char
        );
        if let Ok(parsed) = u32::from_str_radix(&expanded, 16) {
            return [
                ((parsed >> 16) & 255) as f32 / 255.0,
                ((parsed >> 8) & 255) as f32 / 255.0,
                (parsed & 255) as f32 / 255.0,
            ];
        }
        return fallback;
    }
    if s.len() >= 6
        && let Ok(parsed) = u32::from_str_radix(&s[..6], 16)
    {
        return [
            ((parsed >> 16) & 255) as f32 / 255.0,
            ((parsed >> 8) & 255) as f32 / 255.0,
            (parsed & 255) as f32 / 255.0,
        ];
    }
    fallback
}

fn parse_rect(v: &Value) -> Option<FrameRect> {
    let f = v.get("frame")?;
    let x = f.get("x")?.as_f64()?;
    let y = f.get("y")?.as_f64()?;
    let w = f.get("w")?.as_f64()?;
    let h = f.get("h")?.as_f64()?;
    if !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(FrameRect {
        x: x.max(0.0).round() as u32,
        y: y.max(0.0).round() as u32,
        w: w.round() as u32,
        h: h.round() as u32,
    })
}

fn parse_color_band(v: Option<&Value>, fallback: ColorBand) -> ColorBand {
    let Some(obj) = v.and_then(|x| x.as_object()) else {
        return fallback;
    };
    let s = obj
        .get("startRadius")
        .and_then(|x| x.as_f64())
        .unwrap_or(fallback.start as f64) as f32;
    let e = obj
        .get("endRadius")
        .and_then(|x| x.as_f64())
        .unwrap_or(fallback.end as f64) as f32;
    ColorBand { start: s, end: e }
}

fn extract_frames_map(json: &Value) -> Option<HashMap<String, Value>> {
    let raw = json
        .get("frames")
        .or_else(|| json.get("config").and_then(|c| c.get("frames")))?;
    if let Some(obj) = raw.as_object() {
        let mut map = HashMap::new();
        for (k, v) in obj {
            map.insert(k.clone(), v.clone());
        }
        return Some(map);
    }
    if let Some(arr) = raw.as_array() {
        let mut map = HashMap::new();
        for item in arr {
            let name = item
                .get("filename")
                .or_else(|| item.get("name"))
                .and_then(|n| n.as_str());
            if let Some(n) = name {
                map.insert(n.to_string(), item.clone());
            }
        }
        return Some(map);
    }
    None
}

pub fn parse_foundry_sheet(json: &Value) -> Result<ParsedFoundrySheet, String> {
    let frames = extract_frames_map(json)
        .ok_or_else(|| "JSON не содержит секцию frames (это не спрайт-лист Foundry)".to_string())?;
    let meta_image = json
        .get("meta")
        .and_then(|m| m.get("image"))
        .and_then(|i| i.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "JSON не содержит meta.image".to_string())?;

    let cfg = json.get("config").and_then(|c| c.as_object());
    let overlays = cfg.and_then(|c| c.get("overlays")).and_then(|o| o.as_object());
    let default_band = parse_color_band(
        cfg.and_then(|c| c.get("defaultColorBand")),
        ColorBand {
            start: 0.59,
            end: 0.7225,
        },
    );
    let default_ring_color = parse_hex_rgb01(
        cfg.and_then(|c| c.get("defaultRingColor")),
        [1.0, 1.0, 1.0],
    );
    let default_bg_color = parse_hex_rgb01(
        cfg.and_then(|c| c.get("defaultBackgroundColor")),
        [1.0, 1.0, 1.0],
    );

    let raw_scale = cfg
        .and_then(|c| c.get("scale"))
        .and_then(|s| s.as_f64())
        .unwrap_or(1.0) as f32;
    let scale = if raw_scale.is_finite() && (0.1..=10.0).contains(&raw_scale) {
        raw_scale
    } else {
        1.0
    };

    let mut variants = Vec::new();
    for (name, entry) in &frames {
        if name.ends_with("-bkg") || name.ends_with("-msk") || name.ends_with("-top") {
            continue;
        }
        let Some(ring_frame) = parse_rect(entry) else {
            continue;
        };
        let bkg_frame = frames
            .get(&format!("{name}-bkg"))
            .and_then(parse_rect);
        let msk_frame = frames
            .get(&format!("{name}-msk"))
            .and_then(parse_rect);

        let mut top_frame: Option<(Option<String>, FrameRect)> = None;
        if let Some(ov) = overlays.and_then(|o| o.get(name))
            && let Some(rect) = parse_rect(ov)
        {
            let ov_img = ov
                .get("image")
                .and_then(|s| s.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            top_frame = Some((ov_img, rect));
        }
        if top_frame.is_none()
            && let Some(inline_top) = frames.get(&format!("{name}-top"))
            && let Some(rect) = parse_rect(inline_top)
        {
            top_frame = Some((None, rect));
        }

        let ring_color = parse_hex_rgb01(entry.get("ringColor"), default_ring_color);
        let bg_color = parse_hex_rgb01(entry.get("backgroundColor"), default_bg_color);
        let color_band = parse_color_band(entry.get("colorBand"), default_band.clone());

        variants.push(VariantSpec {
            name: name.clone(),
            w: ring_frame.w,
            h: ring_frame.h,
            ring_frame,
            bkg_frame,
            msk_frame,
            top_frame,
            ring_color,
            bg_color,
            color_band,
        });
    }

    if variants.is_empty() {
        return Err("В спрайт-листе не найдено кадров кольца".to_string());
    }

    variants.sort_by_key(|v| (v.w, v.h));
    let best_variant = variants
        .iter()
        .rfind(|v| v.w <= 2048 && v.h <= 2048)
        .cloned()
        .unwrap_or_else(|| variants[0].clone());

    Ok(ParsedFoundrySheet {
        meta_image,
        scale,
        best_variant,
    })
}

fn crop_and_resize(img: &DynamicImage, rect: &FrameRect, target_size: u32) -> Result<RgbaImage, String> {
    let (iw, ih) = (img.width(), img.height());
    if rect.x + rect.w > iw || rect.y + rect.h > ih {
        return Err(format!(
            "Кадр ({}, {}, {}x{}) выходит за пределы атласа ({}x{})",
            rect.x, rect.y, rect.w, rect.h, iw, ih
        ));
    }
    let cropped = img.crop_imm(rect.x, rect.y, rect.w, rect.h).to_rgba8();
    if cropped.width() == target_size && cropped.height() == target_size {
        Ok(cropped)
    } else {
        Ok(image::imageops::resize(
            &cropped,
            target_size,
            target_size,
            image::imageops::FilterType::Lanczos3,
        ))
    }
}

/// Composites `ring` + `bkg` + `msk` using the exact Foundry `TokenRingSamplerShader` math.
fn composite_foundry_ring(
    target_size: u32,
    variant: &VariantSpec,
    ring_img: &RgbaImage,
    bkg_img: Option<&RgbaImage>,
    msk_img: Option<&RgbaImage>,
) -> RgbaImage {
    let [rc_r, rc_g, rc_b] = variant.ring_color;
    let [bc_r, bc_g, bc_b] = variant.bg_color;
    let tint_bg = (bc_r - 1.0).abs() > 1e-3 || (bc_g - 1.0).abs() > 1e-3 || (bc_b - 1.0).abs() > 1e-3;
    let band_start = variant.color_band.start.max(0.0);
    let band_end = variant.color_band.end.max(0.0);
    let has_band = band_end > band_start;
    let inv_size2 = 2.0 / target_size as f32;

    let mut out = RgbaImage::new(target_size, target_size);
    for y in 0..target_size {
        let dy = (y as f32 + 0.5) * inv_size2 - 1.0;
        let dy2 = dy * dy;
        for x in 0..target_size {
            let rp = ring_img.get_pixel(x, y).0;
            let mut bg_pm_r = 0.0f32;
            let mut bg_pm_g = 0.0f32;
            let mut bg_pm_b = 0.0f32;
            let mut bg_a = 0.0f32;

            if let Some(bkg) = bkg_img {
                let bp = bkg.get_pixel(x, y).0;
                let ba = bp[3] as f32 / 255.0;
                if ba > 0.0 {
                    let mut br = bp[0] as f32 / 255.0;
                    let mut bg = bp[1] as f32 / 255.0;
                    let mut bb = bp[2] as f32 / 255.0;
                    if tint_bg {
                        br = if br < 0.5 { 2.0 * br * bc_r } else { 1.0 - 2.0 * (1.0 - br) * (1.0 - bc_r) };
                        bg = if bg < 0.5 { 2.0 * bg * bc_g } else { 1.0 - 2.0 * (1.0 - bg) * (1.0 - bc_g) };
                        bb = if bb < 0.5 { 2.0 * bb * bc_b } else { 1.0 - 2.0 * (1.0 - bb) * (1.0 - bc_b) };
                    }
                    bg_a = ba;
                    bg_pm_r = br * ba;
                    bg_pm_g = bg * ba;
                    bg_pm_b = bb * ba;
                }
            }

            let ra = rp[3] as f32 / 255.0;
            let mp = msk_img.map(|m| m.get_pixel(x, y).0);
            let ma = mp.map(|p| p[3] as f32 / 255.0).unwrap_or(0.0);

            let mut ring_pm_r = 0.0f32;
            let mut ring_pm_g = 0.0f32;
            let mut ring_pm_b = 0.0f32;
            let mut ring_a = 0.0f32;

            if ra > 0.0 || ma > 0.0 {
                let tr = rp[0] as f32 / 255.0;
                let tg = rp[1] as f32 / 255.0;
                let tb = rp[2] as f32 / 255.0;
                let red_ch = if ma > 0.0 {
                    mp.map(|p| p[0] as f32 / 255.0).unwrap_or(tr)
                } else {
                    tr
                };
                let grad_r = rc_r * red_ch;
                let grad_g = rc_g * red_ch;
                let grad_b = rc_b * red_ch;

                let mut mix_f = false;
                if has_band && ra > 0.0 {
                    let dx = (x as f32 + 0.5) * inv_size2 - 1.0;
                    let dist = (dx * dx + dy2).sqrt();
                    if dist >= band_start && dist < band_end {
                        mix_f = true;
                    }
                }

                let (cr, cg, cb) = if mix_f {
                    (grad_r, grad_g, grad_b)
                } else {
                    (tr, tg, tb)
                };
                ring_pm_r = cr * ra;
                ring_pm_g = cg * ra;
                ring_pm_b = cb * ra;
                ring_a = ra;

                if ma > 0.0 {
                    let inv_ma = 1.0 - ma;
                    ring_pm_r = grad_r * ma + ring_pm_r * inv_ma;
                    ring_pm_g = grad_g * ma + ring_pm_g * inv_ma;
                    ring_pm_b = grad_b * ma + ring_pm_b * inv_ma;
                    ring_a = ma + ring_a * inv_ma;
                }
            }

            let out_a = ring_a + bg_a * (1.0 - ring_a);
            if out_a > 1e-5 {
                let inv_ring_a = 1.0 - ring_a;
                let out_pm_r = ring_pm_r + bg_pm_r * inv_ring_a;
                let out_pm_g = ring_pm_g + bg_pm_g * inv_ring_a;
                let out_pm_b = ring_pm_b + bg_pm_b * inv_ring_a;
                let r = ((out_pm_r / out_a) * 255.0).round().clamp(0.0, 255.0) as u8;
                let g = ((out_pm_g / out_a) * 255.0).round().clamp(0.0, 255.0) as u8;
                let b = ((out_pm_b / out_a) * 255.0).round().clamp(0.0, 255.0) as u8;
                let a = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
                out.put_pixel(x, y, Rgba([r, g, b, a]));
            }
        }
    }
    out
}

fn encode_rgba_webp(img: &RgbaImage, quality: f32) -> Vec<u8> {
    let (w, h) = img.dimensions();
    webp::Encoder::from_rgba(img.as_raw(), w, h)
        .encode(quality)
        .to_vec()
}

pub fn sanitize_ring_stem(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .filter(|c| c.is_alphanumeric() || [' ', '_', '-'].contains(c))
        .collect::<String>()
        .trim_matches([' ', '.', '_', '-'])
        .to_string();
    if s.is_empty() {
        s = "Кольцо".to_string();
    }
    s
}

fn prettify_stem(stem: &str) -> String {
    let cleaned = stem
        .trim_end_matches("_2x")
        .trim_end_matches("-2x")
        .replace(['_', '-'], " ");
    let mut words = Vec::new();
    for w in cleaned.split_whitespace() {
        let mut chars = w.chars();
        if let Some(first) = chars.next() {
            let cap: String = first.to_uppercase().collect();
            words.push(format!("{cap}{}", chars.as_str()));
        }
    }
    if words.is_empty() {
        stem.to_string()
    } else {
        words.join(" ")
    }
}

/// Attempts to locate localized RU/EN ring names inside a Foundry module tree
/// (e.g. `cs-rings` layout: `scripts/*.mjs` + `languages/{ru,en}.json`).
fn lookup_module_ring_names(json_path: &Path) -> (Option<String>, Option<String>) {
    let stem = json_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let mut dir = json_path.parent();
    for _ in 0..5 {
        let Some(d) = dir else { break };
        if d.join("module.json").is_file() {
            let mut key_found: Option<String> = None;
            let scripts_dir = d.join("scripts");
            if scripts_dir.is_dir()
                && let Ok(rd) = std::fs::read_dir(&scripts_dir)
            {
                for entry in rd.filter_map(|e| e.ok()) {
                    let p = entry.path();
                    if !p.is_file() {
                        continue;
                    }
                    if let Ok(txt) = std::fs::read_to_string(&p) {
                        for line in txt.lines() {
                            if line.to_lowercase().contains(&stem)
                                && let Some(lbl_pos) = line.find("label")
                            {
                                let after = &line[lbl_pos..];
                                for quote in ['\'', '"'] {
                                    if let Some(q1) = after.find(quote) {
                                        let rest = &after[q1 + 1..];
                                        if let Some(q2) = rest.find(quote) {
                                            let cand = rest[..q2].trim();
                                            if !cand.is_empty() {
                                                key_found = Some(cand.to_string());
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            if key_found.is_some() {
                                break;
                            }
                        }
                    }
                    if key_found.is_some() {
                        break;
                    }
                }
            }
            if let Some(key) = key_found {
                let read_lang = |lang: &str| -> Option<String> {
                    for sub in ["languages", "lang"] {
                        let lp = d.join(sub).join(format!("{lang}.json"));
                        if lp.is_file()
                            && let Ok(bytes) = std::fs::read(&lp)
                            && let Ok(v) = serde_json::from_slice::<Value>(&bytes)
                            && let Some(s) = v.get(&key).and_then(|x| x.as_str())
                        {
                            return Some(s.to_string());
                        }
                    }
                    None
                };
                return (read_lang("ru"), read_lang("en"));
            }
            break;
        }
        dir = d.parent();
    }
    (None, None)
}

pub fn preview_foundry_json(json_path: &Path) -> Result<FoundryRingPreview, String> {
    let bytes = std::fs::read(json_path).map_err(|e| format!("Не удалось прочитать файл: {e}"))?;
    let json: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Некорректный JSON: {e}"))?;
    let parsed = parse_foundry_sheet(&json)?;

    let stem = json_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ring")
        .to_string();
    let base_dir = json_path.parent().unwrap_or_else(|| Path::new("."));
    let atlas_path = resolve_safe_relative(base_dir, &parsed.meta_image)?;
    if !atlas_path.is_file() {
        return Err(format!("Не найден атлас: {}", parsed.meta_image));
    }

    let (loc_ru, loc_en) = lookup_module_ring_names(json_path);
    let fallback = prettify_stem(&stem);
    let suggested_name = loc_ru.or_else(|| loc_en.clone()).unwrap_or(fallback);

    Ok(FoundryRingPreview {
        json_path: crate::paths::clean_path(json_path),
        stem,
        suggested_name,
        suggested_name_en: loc_en,
        scale: (parsed.scale * 10000.0).round() / 10000.0,
        has_top: parsed.best_variant.top_frame.is_some(),
        size: parsed.best_variant.w.min(2048),
        atlas_image: parsed.meta_image,
    })
}

pub fn scan_foundry_folder(dir: &Path) -> Vec<FoundryRingPreview> {
    let mut jsons = Vec::new();
    collect_jsons_recursive(dir, 0, 5, &mut jsons);
    jsons.sort();
    let mut out = Vec::new();
    for jp in jsons {
        if let Ok(prev) = preview_foundry_json(&jp) {
            out.push(prev);
        }
    }
    out
}

fn collect_jsons_recursive(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<PathBuf>) {
    if depth > max_depth || !dir.is_dir() {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir() {
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if matches!(name.as_str(), ".git" | "node_modules" | "target" | "__pycache__") {
                continue;
            }
            collect_jsons_recursive(&p, depth + 1, max_depth, out);
        } else if p.is_file() {
            let fname = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if !fname.ends_with(".json")
                || fname == "module.json"
                || fname == "manifest.json"
                || fname.ends_with(".meta.json")
            {
                continue;
            }
            out.push(p);
        }
    }
}

fn resolve_safe_relative(base_dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_clean = rel.trim().replace('\\', "/");
    if rel_clean.is_empty() || rel_clean.starts_with('/') || rel_clean.contains(':') {
        return Err(format!("Некорректный относительный путь: {rel}"));
    }
    let cand = base_dir.join(rel_clean);
    let base_can = std::fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.to_path_buf());
    let cand_can = std::fs::canonicalize(&cand).unwrap_or_else(|_| cand.clone());
    if !cand_can.starts_with(&base_can) {
        return Err(format!("Путь выходит за пределы папки пака: {rel}"));
    }
    Ok(cand_can)
}

pub fn import_foundry_from_disk(
    json_path: &Path,
    custom_name: Option<&str>,
    target_dir: &Path,
) -> Result<(String, String, bool, f32), String> {
    let bytes = std::fs::read(json_path).map_err(|e| format!("Не удалось прочитать JSON: {e}"))?;
    let json: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Некорректный JSON: {e}"))?;
    let parsed = parse_foundry_sheet(&json)?;
    let base_dir = json_path.parent().unwrap_or_else(|| Path::new("."));

    let atlas_path = resolve_safe_relative(base_dir, &parsed.meta_image)?;
    let atlas_bytes =
        std::fs::read(&atlas_path).map_err(|e| format!("Не удалось прочитать {}: {e}", parsed.meta_image))?;
    let atlas_img = crate::imaging::open_image(&atlas_bytes).map_err(|e| e.to_string())?;

    let top_img = if let Some((Some(ov_rel), _)) = &parsed.best_variant.top_frame {
        let ov_path = resolve_safe_relative(base_dir, ov_rel)?;
        let ov_bytes =
            std::fs::read(&ov_path).map_err(|e| format!("Не удалось прочитать {ov_rel}: {e}"))?;
        Some(crate::imaging::open_image(&ov_bytes).map_err(|e| e.to_string())?)
    } else {
        None
    };

    let (loc_ru, loc_en) = lookup_module_ring_names(json_path);
    let stem_raw = json_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("foundry-ring");
    let display_name = custom_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or(loc_ru)
        .or_else(|| loc_en.clone())
        .unwrap_or_else(|| prettify_stem(stem_raw));

    save_parsed_ring(
        &parsed,
        &atlas_img,
        top_img.as_ref(),
        stem_raw,
        &display_name,
        loc_en,
        target_dir,
    )
}

pub fn import_foundry_from_memory(
    json_filename: &str,
    json_bytes: &[u8],
    files_by_name: &HashMap<String, Vec<u8>>,
    custom_name: Option<&str>,
    target_dir: &Path,
) -> Result<(String, String, bool, f32), String> {
    let json: Value =
        serde_json::from_slice(json_bytes).map_err(|e| format!("Некорректный JSON: {e}"))?;
    let parsed = parse_foundry_sheet(&json)?;

    let find_bytes = |rel: &str| -> Result<&[u8], String> {
        let norm = rel.trim().replace('\\', "/").to_lowercase();
        let base = Path::new(&norm)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&norm)
            .to_string();
        files_by_name
            .get(&norm)
            .or_else(|| files_by_name.get(&base))
            .map(|v| v.as_slice())
            .ok_or_else(|| format!("Приложите файл «{base}», указанный в JSON"))
    };

    let atlas_bytes = find_bytes(&parsed.meta_image)?;
    let atlas_img = crate::imaging::open_image(atlas_bytes).map_err(|e| e.to_string())?;

    let top_img = if let Some((Some(ov_rel), _)) = &parsed.best_variant.top_frame {
        let ov_bytes = find_bytes(ov_rel)?;
        Some(crate::imaging::open_image(ov_bytes).map_err(|e| e.to_string())?)
    } else {
        None
    };

    let stem_raw = Path::new(json_filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("foundry-ring");
    let display_name = custom_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| prettify_stem(stem_raw));

    save_parsed_ring(
        &parsed,
        &atlas_img,
        top_img.as_ref(),
        stem_raw,
        &display_name,
        None,
        target_dir,
    )
}

fn save_parsed_ring(
    parsed: &ParsedFoundrySheet,
    atlas_img: &DynamicImage,
    top_override_img: Option<&DynamicImage>,
    stem_raw: &str,
    display_name: &str,
    name_en: Option<String>,
    target_dir: &Path,
) -> Result<(String, String, bool, f32), String> {
    let v = &parsed.best_variant;
    let out_size = v.w.clamp(128, 2048);

    let ring_im = crop_and_resize(atlas_img, &v.ring_frame, out_size)?;
    let bkg_im = match &v.bkg_frame {
        Some(fr) => Some(crop_and_resize(atlas_img, fr, out_size)?),
        None => None,
    };
    let msk_im = match &v.msk_frame {
        Some(fr) => Some(crop_and_resize(atlas_img, fr, out_size)?),
        None => None,
    };

    let composed = composite_foundry_ring(
        out_size,
        v,
        &ring_im,
        bkg_im.as_ref(),
        msk_im.as_ref(),
    );
    let ring_webp = encode_rgba_webp(&composed, 92.0);

    let top_webp = if let Some((_, top_rect)) = &v.top_frame {
        let src = top_override_img.unwrap_or(atlas_img);
        let top_im = crop_and_resize(src, top_rect, out_size)?;
        Some(encode_rgba_webp(&top_im, 92.0))
    } else {
        None
    };

    let _ = std::fs::create_dir_all(target_dir);
    let safe_stem = sanitize_ring_stem(stem_raw);
    let mut final_stem = safe_stem.clone();
    let mut target_ring = target_dir.join(format!("{final_stem}.webp"));
    let mut suffix = 2u32;
    while target_ring.exists() {
        final_stem = format!("{safe_stem}_{suffix}");
        target_ring = target_dir.join(format!("{final_stem}.webp"));
        suffix += 1;
    }

    std::fs::write(&target_ring, &ring_webp)
        .map_err(|e| format!("Не удалось сохранить кольцо: {e}"))?;

    let mut top_file = None;
    if let Some(tw) = &top_webp {
        let tf_name = format!("{final_stem}.top.webp");
        let tf_path = target_dir.join(&tf_name);
        if let Err(e) = std::fs::write(&tf_path, tw) {
            let _ = std::fs::remove_file(&target_ring);
            return Err(format!("Не удалось сохранить второй слой: {e}"));
        }
        top_file = Some(tf_name);
    }

    let scale = (parsed.scale * 10000.0).round() / 10000.0;
    let meta = RingMeta {
        name: Some(display_name.to_string()),
        name_en,
        scale: Some(scale),
        top_file: top_file.clone(),
        source: Some("foundry".to_string()),
    };
    if let Err(e) = write_ring_meta(&target_ring, &meta) {
        let _ = std::fs::remove_file(&target_ring);
        if let Some(tf) = &top_file {
            let _ = std::fs::remove_file(target_dir.join(tf));
        }
        return Err(format!("Не удалось сохранить метаданные: {e}"));
    }

    let file_name = format!("{final_stem}.webp");
    Ok((display_name.to_string(), file_name, top_file.is_some(), scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_hash_and_array_sheets() {
        let hash_json = json!({
            "frames": {
                "token-ring-med-bkg": { "frame": { "x": 0, "y": 0, "w": 512, "h": 512 } },
                "token-ring-med": { "frame": { "x": 512, "y": 0, "w": 512, "h": 512 }, "gridTarget": 1 },
                "token-ring-med-msk": { "frame": { "x": 1024, "y": 0, "w": 512, "h": 512 } }
            },
            "meta": { "image": "test.webp" },
            "config": {
                "scale": 2.2171,
                "defaultRingColor": "#675e68",
                "overlays": {
                    "token-ring-med": { "image": "test-top.webp", "frame": { "x": 0, "y": 0, "w": 512, "h": 512 } }
                }
            }
        });
        let parsed = parse_foundry_sheet(&hash_json).unwrap();
        assert!((parsed.scale - 2.2171).abs() < 1e-4);
        assert_eq!(parsed.best_variant.name, "token-ring-med");
        assert!(parsed.best_variant.bkg_frame.is_some());
        assert!(parsed.best_variant.msk_frame.is_some());
        let (ov_img, _) = parsed.best_variant.top_frame.unwrap();
        assert_eq!(ov_img.as_deref(), Some("test-top.webp"));

        let arr_json = json!({
            "frames": [
                { "filename": "ring-med", "frame": { "x": 0, "y": 0, "w": 512, "h": 512 } },
                { "filename": "ring-med-top", "frame": { "x": 512, "y": 0, "w": 512, "h": 512 } }
            ],
            "meta": { "image": "sheet.png" }
        });
        let parsed_arr = parse_foundry_sheet(&arr_json).unwrap();
        assert_eq!(parsed_arr.best_variant.name, "ring-med");
        let (inline_ov, _) = parsed_arr.best_variant.top_frame.unwrap();
        assert!(inline_ov.is_none());
    }

    #[test]
    fn composite_preserves_ring_over_bkg() {
        let mut ring = RgbaImage::new(4, 4);
        let mut bkg = RgbaImage::new(4, 4);
        for p in bkg.pixels_mut() {
            *p = Rgba([20, 40, 60, 255]);
        }
        ring.put_pixel(1, 1, Rgba([200, 100, 50, 255]));
        let v = VariantSpec {
            name: "test".into(),
            w: 4,
            h: 4,
            ring_frame: FrameRect { x: 0, y: 0, w: 4, h: 4 },
            bkg_frame: Some(FrameRect { x: 0, y: 0, w: 4, h: 4 }),
            msk_frame: None,
            top_frame: None,
            ring_color: [1.0, 1.0, 1.0],
            bg_color: [1.0, 1.0, 1.0],
            color_band: ColorBand { start: 0.0, end: 0.0 },
        };
        let out = composite_foundry_ring(4, &v, &ring, Some(&bkg), None);
        assert_eq!(out.get_pixel(0, 0).0, [20, 40, 60, 255]);
        assert_eq!(out.get_pixel(1, 1).0, [200, 100, 50, 255]);
    }

    #[test]
    fn import_from_memory_writes_ring_top_and_meta() {
        let mut atlas = RgbaImage::new(256, 128);
        for y in 0..128 {
            for x in 0..128 {
                atlas.put_pixel(x, y, Rgba([20, 20, 30, 255]));
                atlas.put_pixel(x + 128, y, Rgba([180, 190, 200, 255]));
            }
        }
        let mut atlas_png = Vec::new();
        DynamicImage::ImageRgba8(atlas)
            .write_to(&mut std::io::Cursor::new(&mut atlas_png), image::ImageFormat::Png)
            .unwrap();

        let mut top_img = RgbaImage::new(128, 128);
        top_img.put_pixel(64, 100, Rgba([255, 80, 40, 255]));
        let mut top_png = Vec::new();
        DynamicImage::ImageRgba8(top_img)
            .write_to(&mut std::io::Cursor::new(&mut top_png), image::ImageFormat::Png)
            .unwrap();

        let sheet_json = serde_json::to_vec(&json!({
            "frames": {
                "token-ring-med-bkg": { "frame": { "x": 0, "y": 0, "w": 128, "h": 128 } },
                "token-ring-med": { "frame": { "x": 128, "y": 0, "w": 128, "h": 128 } }
            },
            "meta": { "image": "test-atlas.png" },
            "config": {
                "scale": 2.2,
                "overlays": {
                    "token-ring-med": { "image": "test-top.png", "frame": { "x": 0, "y": 0, "w": 128, "h": 128 } }
                }
            }
        }))
        .unwrap();

        let mut files = HashMap::new();
        files.insert("test-atlas.png".to_string(), atlas_png);
        files.insert("test-top.png".to_string(), top_png);

        let tmp = std::env::temp_dir().join(format!("tokenatra_ring_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (name, file, has_top, scale) =
            import_foundry_from_memory("custom_pack.json", &sheet_json, &files, Some("Test Armor"), &tmp)
                .unwrap();

        assert_eq!(name, "Test Armor");
        assert_eq!(file, "custom_pack.webp");
        assert!(has_top);
        assert!((scale - 2.2).abs() < 1e-3);
        assert!(tmp.join("custom_pack.webp").is_file());
        assert!(tmp.join("custom_pack.top.webp").is_file());
        assert!(tmp.join("custom_pack.meta.json").is_file());
        let meta = read_ring_meta(&tmp.join("custom_pack.webp")).unwrap();
        assert_eq!(meta.top_file.as_deref(), Some("custom_pack.top.webp"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
