use std::io::Cursor;

use image::imageops::FilterType;
use image::ImageEncoder as _;
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};

use crate::consts::{MAX_IMAGE_DIMENSION, MAX_MEGAPIXELS, MAX_PROCESS_DIM};

#[derive(Debug)]
pub enum ImgError {
    TooLarge,
    Decode(String),
    Encode(String),
    #[allow(dead_code)]
    AvifUnavailable,
}

impl std::fmt::Display for ImgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImgError::TooLarge => write!(f, "Изображение слишком большое (более 100 МП)"),
            ImgError::Decode(e) => write!(f, "Не удалось декодировать изображение: {e}"),
            ImgError::Encode(e) => write!(f, "Не удалось кодировать изображение: {e}"),
            ImgError::AvifUnavailable => write!(
                f,
                "Формат AVIF недоступен: установите pillow-avif-plugin"
            ),
        }
    }
}

/// Decodes images, checking dimensions before full decompression
/// (decompression bomb guard). AVIF falls back to system ffmpeg.
pub fn open_image(bytes: &[u8]) -> Result<DynamicImage, ImgError> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ImgError::Decode(e.to_string()))?;
    if let Ok((w, h)) = reader.into_dimensions()
        && (w as u64) * (h as u64) > MAX_MEGAPIXELS {
            return Err(ImgError::TooLarge);
        }
    match image::load_from_memory(bytes) {
        Ok(img) => Ok(img),
        Err(e) => {
            let sniff = looks_like_avif(bytes);
            if sniff
                && let Some(png) = crate::ffmpeg::decode_to_png(bytes)
                    && let Ok(img) = image::load_from_memory(&png) {
                        return Ok(img);
                    }
            Err(ImgError::Decode(e.to_string()))
        }
    }
}

fn looks_like_avif(bytes: &[u8]) -> bool {
    // ISO-BMFF: ftyp box with an avif/avis brand
    bytes.len() > 12
        && &bytes[4..8] == b"ftyp"
        && (bytes[8..16].starts_with(b"avif") || bytes[8..16].starts_with(b"avis"))
}

pub fn allowed_image_ext(filename: &str) -> bool {
    let ext = filename
        .rsplit('.')
        .next()
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    crate::consts::IMAGE_EXTENSIONS.contains(&ext.as_str())
}

pub fn is_media_ext(filename: &str, mime: Option<&str>) -> bool {
    let ext = filename
        .rsplit('.')
        .next()
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if crate::consts::MEDIA_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    mime.map(|m| m.starts_with("audio/") || m.starts_with("video/"))
        .unwrap_or(false)
}

/// Downscales to MAX_IMAGE_DIMENSION (Lanczos).
pub fn validate_image(img: DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w > MAX_IMAGE_DIMENSION || h > MAX_IMAGE_DIMENSION {
        let ratio = MAX_IMAGE_DIMENSION as f64 / w.max(h) as f64;
        let nw = ((w as f64) * ratio) as u32;
        let nh = ((h as f64) * ratio) as u32;
        return img.resize_exact(nw, nh, FilterType::Lanczos3);
    }
    img
}

/// Downscales the working image to MAX_PROCESS_DIM (bilinear).
pub fn cap_process_size(img: DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w.max(h) > MAX_PROCESS_DIM {
        let ratio = MAX_PROCESS_DIM as f64 / w.max(h) as f64;
        let nw = (((w as f64) * ratio) as u32).max(1);
        let nh = (((h as f64) * ratio) as u32).max(1);
        return img.resize_exact(nw, nh, FilterType::Triangle);
    }
    img
}

/// Alpha cannot be dropped: transparent pixels are near-black and
/// would leave a dark halo. Composite over white.
pub fn flatten_to_rgb(img: &DynamicImage) -> image::RgbImage {
    if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let mut out = image::RgbImage::new(w, h);
        for (x, y, px) in rgba.enumerate_pixels() {
            let a = px[3] as u32;
            out.put_pixel(
                x,
                y,
                image::Rgb([
                    ((255 * (255 - a) + px[0] as u32 * a) / 255) as u8,
                    ((255 * (255 - a) + px[1] as u32 * a) / 255) as u8,
                    ((255 * (255 - a) + px[2] as u32 * a) / 255) as u8,
                ]),
            );
        }
        out
    } else {
        img.to_rgb8()
    }
}

/// 3x3 min filter (PIL MinFilter(3) equivalent, edge-replicated).
pub fn min_filter3(gray: &image::GrayImage) -> image::GrayImage {
    let (w, h) = gray.dimensions();
    let mut out = image::GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut m = 255u8;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let nx = (x as i64 + dx).clamp(0, w as i64 - 1) as u32;
                    let ny = (y as i64 + dy).clamp(0, h as i64 - 1) as u32;
                    let v = gray.get_pixel(nx, ny)[0];
                    if v < m {
                        m = v;
                    }
                }
            }
            out.put_pixel(x, y, image::Luma([m]));
        }
    }
    out
}

/// Refines the mask on a downscaled copy, upscales to target size.
pub fn refine_mask(
    mask: &image::GrayImage,
    edge_blur: f64,
    threshold_low: f64,
    threshold_high: f64,
    work_size: u32,
    target_size: Option<(u32, u32)>,
    gentle: bool,
) -> image::GrayImage {
    let orig_size = target_size.unwrap_or_else(|| mask.dimensions());
    let (w, h) = orig_size;
    let work = if work_size > 0 && w.max(h) > work_size {
        let ratio = work_size as f64 / w.max(h) as f64;
        let nw = (((w as f64) * ratio) as u32).max(1);
        let nh = (((h as f64) * ratio) as u32).max(1);
        image::imageops::resize(mask, nw, nh, FilterType::Triangle)
    } else {
        mask.clone()
    };

    let low = threshold_low / 255.0;
    let high = threshold_high / 255.0;
    let (tw, th) = work.dimensions();
    let mut stage = image::GrayImage::new(tw, th);
    for y in 0..th {
        for x in 0..tw {
            let v = work.get_pixel(x, y)[0] as f64 / 255.0;
            let mut v = ((v - low) / (high - low + 1e-8)).clamp(0.0, 1.0);
            if !gentle {
                v = v.powf(1.2);
            }
            stage.put_pixel(x, y, image::Luma([(v * 255.0) as u8]));
        }
    }

    let stage = if !gentle { min_filter3(&stage) } else { stage };

    let (sw, sh) = stage.dimensions();
    let mut stage2 = image::GrayImage::new(sw, sh);
    let cut = if gentle { 0.05 } else { 0.15 };
    for y in 0..sh {
        for x in 0..sw {
            let v = stage.get_pixel(x, y)[0] as f64 / 255.0;
            let v = if v < cut {
                0.0
            } else if v > 0.92 {
                1.0
            } else {
                v
            };
            stage2.put_pixel(x, y, image::Luma([(v * 255.0) as u8]));
        }
    }

    let stage2 = if edge_blur > 0.0 {
        imageproc::filter::gaussian_blur_f32(&stage2, (edge_blur * 0.35) as f32)
    } else {
        stage2
    };

    if stage2.dimensions() != orig_size {
        image::imageops::resize(&stage2, w, h, FilterType::Lanczos3)
    } else {
        stage2
    }
}

/// Builds the NCHW f32 model input (1024x1024) from RGB.
/// BEN2 expects plain 0..1, others use ImageNet normalization.
pub fn prepare_model_input(rgb: &image::RgbImage, imagenet_norm: bool) -> Vec<f32> {
    let square = image::imageops::resize(rgb, 1024, 1024, FilterType::Lanczos3);
    let mut out = vec![0f32; 3 * 1024 * 1024];
    let (mean, std) = if imagenet_norm {
        (
            [0.485f32, 0.456, 0.406],
            [0.229f32, 0.224, 0.225],
        )
    } else {
        ([0.0f32; 3], [1.0f32; 3])
    };
    // NCHW: channel, row, column
    for (x, y, px) in square.enumerate_pixels() {
        let idx = (y as usize * 1024) + x as usize;
        for c in 0..3 {
            let v = px[c] as f32 / 255.0;
            out[c * 1024 * 1024 + idx] = (v - mean[c]) / std[c];
        }
    }
    out
}

/// Raw mask postprocessing: sigmoid or clip, stretched to 0-255.
/// None => no object found (near-uniform mask), empty mask.
pub fn mask_from_output(raw: &[f32]) -> Option<image::GrayImage> {
    if raw.len() < 1024 * 1024 {
        return None;
    }
    let plane = &raw[..1024 * 1024];
    let raw_min = plane.iter().copied().fold(f32::INFINITY, f32::min);
    let raw_max = plane.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let in_01 = raw_min >= 0.0 && raw_max <= 1.0;

    let mut mapped = vec![0.0f32; 1024 * 1024];
    let mut mn = f32::INFINITY;
    let mut mx = f32::NEG_INFINITY;
    for (dst, &v) in mapped.iter_mut().zip(plane.iter()) {
        let m = if in_01 {
            v.clamp(0.0, 1.0)
        } else {
            1.0 / (1.0 + (-v).exp())
        };
        *dst = m;
        if m < mn {
            mn = m;
        }
        if m > mx {
            mx = m;
        }
    }
    if (mx - mn) < 1e-4 {
        return Some(image::GrayImage::new(1024, 1024));
    }
    let mut img = image::GrayImage::new(1024, 1024);
    let span = mx - mn + 1e-8;
    for (i, &v) in mapped.iter().enumerate() {
        let stretched = ((v - mn) / span * 255.0).round().clamp(0.0, 255.0) as u8;
        img.get_pixel_mut((i % 1024) as u32, (i / 1024) as u32)[0] = stretched;
    }
    Some(img)
}

/// White penalty: bright near-background pixels (wp>220, alpha<180)
/// are suppressed via an alpha correction. Returns the correction
/// mask at image size.
pub fn white_penalty_correction(
    rgb: &image::RgbImage,
    alpha: &image::GrayImage,
) -> image::GrayImage {
    let small = image::imageops::resize(rgb, 1024, 1024, FilterType::Triangle);
    let alpha_small = image::imageops::resize(alpha, 1024, 1024, FilterType::Triangle);
    let mut corr = image::GrayImage::new(1024, 1024);
    for y in 0..1024 {
        for x in 0..1024 {
            let p = small.get_pixel(x, y);
            let wp = p[0] as f32 * 0.299 + p[1] as f32 * 0.587 + p[2] as f32 * 0.114;
            let a = alpha_small.get_pixel(x, y)[0] as f32;
            let v = if wp > 220.0 && a < 180.0 {
                ((wp - 220.0) * 3.5).min(255.0)
            } else {
                0.0
            };
            corr.put_pixel(x, y, image::Luma([v as u8]));
        }
    }
    let (w, h) = rgb.dimensions();
    image::imageops::resize(&corr, w, h, FilterType::Triangle)
}

/// Final composite: RGBA with alpha = mask - correction.
pub fn composite_result(
    rgb: &image::RgbImage,
    alpha: &image::GrayImage,
    correction: &image::GrayImage,
) -> RgbaImage {
    let (w, h) = rgb.dimensions();
    let mut out = RgbaImage::new(w, h);
    for (x, y, px) in rgb.enumerate_pixels() {
        let a = alpha.get_pixel(x, y)[0] as i16 - correction.get_pixel(x, y)[0] as i16;
        out.put_pixel(x, y, Rgba([px[0], px[1], px[2], a.clamp(0, 255) as u8]));
    }
    out
}

/// Encodes to (bytes, mime).
pub fn save_image(
    img: &DynamicImage,
    format_type: &str,
    quality: u8,
) -> Result<(Vec<u8>, &'static str), ImgError> {
    let mut buf = Vec::new();
    match format_type {
        "webp" => {
            encode_webp(img, quality, &mut buf)?;
            Ok((buf, "image/webp"))
        }
        "avif" => {
            // image::AvifEncoder (ravif), 8-bit AVIF
            let mut c = Cursor::new(&mut buf);
            let enc = image::codecs::avif::AvifEncoder::new_with_speed_quality(&mut c, 10, quality);
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            enc.write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
                .map_err(|e| ImgError::Encode(e.to_string()))?;
            Ok((buf, "image/avif"))
        }
        "jpg" | "jpeg" => {
            let flat = flatten_to_rgb(img);
            let mut c = Cursor::new(&mut buf);
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut c, quality);
            let (w, h) = flat.dimensions();
            enc.write_image(
                flat.as_raw(),
                w,
                h,
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| ImgError::Encode(e.to_string()))?;
            Ok((buf, "image/jpeg"))
        }
        "bmp" => {
            let rgba = img.to_rgba8();
            let mut c = Cursor::new(&mut buf);
            image::codecs::bmp::BmpEncoder::new(&mut c)
                .write_image(
                    rgba.as_raw(),
                    rgba.width(),
                    rgba.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(|e| ImgError::Encode(e.to_string()))?;
            Ok((buf, "image/bmp"))
        }
        "gif" => {
            let flat = flatten_to_rgb(img);
            let (w, h) = flat.dimensions();
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for px in flat.pixels() {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            let frame_img = image::RgbaImage::from_raw(w, h, rgba).expect("gif buffer");
            let frame = image::Frame::new(frame_img);
            let mut c = Cursor::new(&mut buf);
            {
                let mut enc = image::codecs::gif::GifEncoder::new(&mut c);
                enc.encode_frame(frame)
                    .map_err(|e| ImgError::Encode(e.to_string()))?;
            }
            Ok((buf, "image/gif"))
        }
        "tiff" => {
            let flat = flatten_to_rgb(img);
            let (w, h) = flat.dimensions();
            let mut c = Cursor::new(&mut buf);
            image::codecs::tiff::TiffEncoder::new(&mut c)
                .write_image(
                    flat.as_raw(),
                    w,
                    h,
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| ImgError::Encode(e.to_string()))?;
            Ok((buf, "image/tiff"))
        }
        _ => {
            // PNG (default)
            let png = img.to_rgba8();
            let mut c = Cursor::new(&mut buf);
            image::codecs::png::PngEncoder::new(&mut c)
                .write_image(
                    png.as_raw(),
                    png.width(),
                    png.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(|e| ImgError::Encode(e.to_string()))?;
            Ok((buf, "image/png"))
        }
    }
}

/// Lossy WebP via libwebp (PIL-compatible quality).
pub fn encode_webp(img: &DynamicImage, quality: u8, out: &mut Vec<u8>) -> Result<(), ImgError> {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mem = webp::Encoder::from_rgba(rgba.as_raw(), w, h).encode(f32::from(quality));
    out.extend_from_slice(&mem);
    Ok(())
}

/// Fallback ring (grey ellipse) when the file is missing.
pub fn create_default_ring(size: u32) -> RgbaImage {
    let scale = size as f32 / 1024.0;
    let w = (40.0 * scale) as i32;
    let mut img = RgbaImage::new(size, size);
    let color = image::Rgba([100, 100, 100, 255]);
    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    let r_out = size as f32 / 2.0 - (w as f32 / 2.0).max(1.0);
    let r_in = r_out - w as f32;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= r_out && d >= r_in {
                img.put_pixel(x, y, color);
            }
        }
    }
    img
}

