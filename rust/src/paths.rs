use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::consts;

/// Resource dir candidates: env -> exe dir (and parents for
/// target/debug runs) -> compile-time repo root.
fn resource_dir_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(cfg) = std::env::var("TOKENATRA_RESOURCE_DIR")
        && !cfg.trim().is_empty() {
            out.push(PathBuf::from(cfg));
        }
    if let Ok(exe) = std::env::current_exe() {
        let exe_dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        // macOS .app bundle: Tokenatra.app/Contents/MacOS/tokenatra -> Contents/Resources
        if exe_dir.file_name().and_then(|n| n.to_str()) == Some("MacOS")
            && let Some(contents) = exe_dir.parent()
        {
            out.push(contents.join("Resources"));
        }
        out.push(exe_dir.clone());
        // Linux FHS / AppImage: <prefix>/bin/tokenatra -> <prefix>/share/tokenatra
        if let Some(prefix) = exe_dir.parent() {
            out.push(prefix.join("share").join("tokenatra"));
        }
        // cargo run: target/debug/tokenatra.exe -> three levels up to repo root
        for anc in exe_dir.ancestors().skip(1).take(4) {
            out.push(anc.to_path_buf());
        }
    }
    // compile-time: rust/ -> repo root
    out.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
    out
}

fn looks_like_resource_dir(p: &Path) -> bool {
    p.join("templates").is_dir() && p.join("static").is_dir()
}

pub fn resource_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let candidates = resource_dir_candidates();
        for c in &candidates {
            if looks_like_resource_dir(c) {
                return c.clone();
            }
        }
        candidates
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."))
    })
}

/// App dir: next to the exe (installed/portable) or repo root (dev).
pub fn base_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Ok(v) = std::env::var("TOKENMAKER_DIR")
            && !v.trim().is_empty() {
                return PathBuf::from(v);
            }
        if let Ok(exe) = std::env::current_exe()
            && let Some(d) = exe.parent() {
                return d.to_path_buf();
            }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    })
}

fn probe_write(dir: &Path) -> bool {
    let probe = dir.join(format!(".tokenatra_write_probe_{}", unique_tmp_tag()));
    std::fs::write(&probe, b"x").is_ok() && std::fs::remove_file(&probe).is_ok()
}

/// LOCALAPPDATA / ~/Library/Application Support / XDG_DATA_HOME candidate.
fn platform_data_base() -> Option<PathBuf> {
    if cfg!(windows) {
        let root = std::env::var("LOCALAPPDATA")
            .ok()
            .or_else(|| std::env::var("APPDATA").ok());
        if let Some(r) = root {
            return Some(PathBuf::from(r));
        }
        return std::env::var("USERPROFILE")
            .ok()
            .map(|h| PathBuf::from(h).join("AppData").join("Local"));
    }
    if cfg!(target_os = "macos") {
        return std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"));
    }
    std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })
}

pub fn user_data_dir_raw() -> PathBuf {
    platform_data_base()
        .map(|b| b.join(consts::APP_NAME))
        .filter(|d| {
            let _ = std::fs::create_dir_all(d);
            probe_write(d)
        })
        .unwrap_or_else(|| {
            let d = std::env::temp_dir().join(consts::APP_NAME);
            let _ = std::fs::create_dir_all(&d);
            d
        })
}

/// User data dir (config.json, models/, user rings/presets).
/// Installed builds live in Program Files (read-only), fall back to LOCALAPPDATA.
pub fn user_data_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = user_data_dir_raw();
        if probe_write(&base) {
            return base;
        }
        if probe_write(base_dir()) {
            return base_dir().to_path_buf();
        }
        base
    })
}

pub fn config_file() -> PathBuf {
    user_data_dir().join("config.json")
}
pub fn models_dir() -> PathBuf {
    user_data_dir().join("models")
}
pub fn builtin_ring_dir() -> PathBuf {
    resource_dir().join("token_rings")
}
pub fn ring_dir() -> PathBuf {
    user_data_dir().join("token_rings")
}
pub fn builtin_preset_dir() -> PathBuf {
    resource_dir().join("presets")
}
pub fn user_preset_dir() -> PathBuf {
    user_data_dir().join("presets")
}
pub fn mask_path() -> PathBuf {
    resource_dir().join("mask.png")
}
pub fn embed_export_dir() -> PathBuf {
    user_data_dir().join("embed_export")
}
pub fn pid_file() -> PathBuf {
    user_data_dir().join("app.pid")
}

/// One-time migration of model.onnx into models/.
pub fn migrate_legacy_model() {
    let legacy = base_dir().join("model.onnx");
    if !legacy.exists() {
        return;
    }
    let models = models_dir();
    let _ = std::fs::create_dir_all(&models);
    let dest = models.join("model.onnx");
    match std::fs::rename(&legacy, &dest) {
        Ok(()) => println!(" Модель перемещена: model.onnx -> models/"),
        Err(_) => {
            // cross-volume rename fails, fall back to copy+remove
            if !dest.exists()
                && std::fs::copy(&legacy, &dest).is_ok()
                && std::fs::remove_file(&legacy).is_ok()
            {
                println!(" Модель перемещена: model.onnx -> models/");
            } else {
                let _ = std::fs::remove_file(&legacy);
            }
        }
    }
}

/// Selected model path: config selected_model -> first .onnx in models/.
pub fn get_selected_model_path() -> PathBuf {
    let candidates = [
        models_dir(),
        base_dir().join("models"),
        resource_dir().join("models"),
    ];
    if let Ok(cfg) = crate::config::load()
        && let Some(sel) = cfg.get("selected_model").and_then(|v| v.as_str())
        && let Some(fname) = Path::new(sel).file_name()
    {
        for dir in &candidates {
            let p = dir.join(fname);
            if p.exists() {
                return p;
            }
        }
    }
    let mut visited: Vec<PathBuf> = Vec::new();
    for dir in &candidates {
        if visited.iter().any(|v| same_path(v, dir)) {
            continue;
        }
        visited.push(dir.clone());
        if let Some(p) = first_onnx_in(dir) {
            return p;
        }
    }
    models_dir().join("model.onnx")
}

pub fn first_onnx_in(dir: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("onnx"))
                    .unwrap_or(false)
        })
        .collect();
    files.sort();
    files.into_iter().next()
}

#[cfg(windows)]
pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| -> String {
        let s = p.to_string_lossy().to_lowercase();
        let c = s.replace('/', "\\");
        let c = c.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(c);
        c.trim_end_matches('\\').to_string()
    };
    std::fs::canonicalize(a).map(|p| norm(&p)).unwrap_or_else(|_| norm(a))
        == std::fs::canonicalize(b).map(|p| norm(&p)).unwrap_or_else(|_| norm(b))
}

#[cfg(not(windows))]
pub fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// str(Path) without the canonicalize verbatim prefix.
pub fn clean_path(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    // Network files canonicalize to the verbatim UNC form
    // \\?\UNC\server\share\...; strip it back to \\server\share\... (the
    // leftover "UNC\..." form is not a valid path and 404s on re-fetch).
    const VERBATIM_UNC: &str = "\\\\?\\UNC\\";
    if let Some(rest) = s.strip_prefix(VERBATIM_UNC) {
        return format!("\\\\{rest}");
    }
    const VERBATIM: &str = "\\\\?\\";
    s.strip_prefix(VERBATIM).unwrap_or(&s).to_string()
}

#[cfg(windows)]
pub fn norm_folder(p: &Path) -> PathBuf {
    let s = clean_path(p).to_lowercase().replace('/', "\\");
    let s = s.trim_end_matches('\\').to_string();
    PathBuf::from(s)
}

#[cfg(not(windows))]
pub fn norm_folder(p: &Path) -> PathBuf {
    let mut s = p.to_string_lossy().to_string();
    if let Ok(c) = std::fs::canonicalize(p) {
        s = c.to_string_lossy().to_string();
    }
    if s.len() > 1 {
        s = s.trim_end_matches('/').to_string();
    }
    PathBuf::from(s)
}

pub fn unique_tmp_tag() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let rnd: u32 = rand::random();
    format!("{}_{seq}_{rnd:08x}", std::process::id())
}

/// Atomic file write (temp + sync + rename).
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", unique_tmp_tag()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        if let Err(e) = f.write_all(data).and_then(|_| f.sync_all()) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Migrates user-added rings out of the read-only builtin resource dir into user_data_dir.
pub fn migrate_legacy_rings() {
    let builtin = builtin_ring_dir();
    let rings = ring_dir();
    if same_path(&builtin, &rings) || !builtin.is_dir() {
        return;
    }
    let _ = std::fs::create_dir_all(&rings);
    let entries = match std::fs::read_dir(&builtin) {
        Ok(e) => e,
        Err(_) => return,
    };
    let exts = ["webp", "png", "jpg", "jpeg"];
    for entry in entries.filter_map(|e| e.ok()) {
        let f = entry.path();
        if !f.metadata().map(|m| m.is_file()).unwrap_or(false) {
            continue;
        }
        let ext = f
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if !exts.contains(&ext.as_str()) {
            continue;
        }
        let stem = f
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if stem.ends_with(".mask") {
            continue;
        }
        if consts::builtin_ring_name(&format!("{}.{}", stem, ext)).is_some() {
            continue;
        }
        let dest = rings.join(f.file_name().unwrap_or_default());
        if dest.exists() {
            continue;
        }
        if std::fs::rename(&f, &dest).is_ok() {
            for me in [&ext[..], "png", "webp", "jpg", "jpeg"] {
                let legacy_mask = f.with_file_name(format!(
                    "{}.mask.{}",
                    f.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
                    me
                ));
                let migrated = dest.with_file_name(legacy_mask.file_name().unwrap_or_default());
                if legacy_mask.is_file() && !migrated.exists() {
                    let _ = std::fs::rename(&legacy_mask, &migrated);
                }
            }
        }
    }
}
