use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::consts;

/// Кандидаты каталога ресурсов: env -> exe-каталог (и его родители для
/// target/debug-запусков) -> compile-time корень репозитория.
fn resource_dir_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(cfg) = std::env::var("TOKENATRA_RESOURCE_DIR") {
        if !cfg.trim().is_empty() {
            out.push(PathBuf::from(cfg));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let exe_dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        out.push(exe_dir.clone());
        // cargo run: target/debug/tokenatra.exe -> три уровня вверх к корню
        for anc in exe_dir.ancestors().skip(1).take(4) {
            out.push(anc.to_path_buf());
        }
    }
    // Compile-time: rust/ -> корень репозитория
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

/// Каталог приложения: рядом с exe (installed/portable) или корень репозитория (dev).
pub fn base_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Ok(v) = std::env::var("TOKENMAKER_DIR") {
            if !v.trim().is_empty() {
                return PathBuf::from(v);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                return d.to_path_buf();
            }
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    })
}

fn probe_write(dir: &Path) -> bool {
    let probe = dir.join(".tokenatra_write_probe");
    std::fs::write(&probe, b"x").is_ok() && std::fs::remove_file(&probe).is_ok()
}

/// Кандидат LOCALAPPDATA/~/Library/Application Support/XDG_DATA_HOME.
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

/// Каталог пользовательских данных (config.json, models/, пользовательские
/// кольца/пресеты). Установленная сборка лежит в Program Files и недоступна
/// для записи — тогда уходим в LOCALAPPDATA (как в Python).
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

/// Разовая миграция model.onnx из каталога приложения в models/.
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
            // rename через тома падает — пробуем copy+remove
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

/// Путь к выбранной модели: config selected_model -> первая .onnx в models/.
pub fn get_selected_model_path() -> PathBuf {
    if let Ok(cfg) = crate::config::load() {
        if let Some(sel) = cfg.get("selected_model").and_then(|v| v.as_str()) {
            let p = models_dir().join(Path::new(sel).file_name().unwrap_or_default());
            if p.exists() {
                return p;
            }
        }
    }
    if let Some(p) = first_onnx_in(&models_dir()) {
        return p;
    }
    // Portable-сборки: модели рядом с exe
    let legacy = base_dir().join("models");
    if !same_path(&legacy, &models_dir()) {
        if let Some(p) = first_onnx_in(&legacy) {
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
        let c = if c.starts_with("\\\\?\\") { c[4..].to_string() } else { c };
        c.trim_end_matches('\\').to_string()
    };
    std::fs::canonicalize(a).map(|p| norm(&p)).unwrap_or_else(|_| norm(a))
        == std::fs::canonicalize(b).map(|p| norm(&p)).unwrap_or_else(|_| norm(b))
}

#[cfg(not(windows))]
pub fn same_path(a: &Path, b: &Path) -> bool {
    std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok()
}

/// Пользовательские кольца из старого каталога (BASE_DIR/token_rings)
/// переносим в RING_DIR — в установленной сборке BASE_DIR недоступен для записи.
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
        let meta = match f.metadata() {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        let _ = meta;
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
        let dest = rings.join(&f);
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
