use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Map, Value};

struct CachedConfig {
    value: Value,
    mtime: Option<SystemTime>,
    checked_at: Instant,
}

static CACHE: Mutex<Option<CachedConfig>> = Mutex::new(None);
static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Reads config.json (cached in memory; empty object if missing or corrupt).
pub fn load() -> anyhow::Result<Value> {
    let path = crate::paths::config_file();
    {
        let mut guard = CACHE.lock().unwrap();
        if let Some(c) = guard.as_mut() {
            if c.checked_at.elapsed() < Duration::from_secs(2) {
                return Ok(c.value.clone());
            }
            let cur_mtime = file_mtime(&path);
            if cur_mtime == c.mtime {
                c.checked_at = Instant::now();
                return Ok(c.value.clone());
            }
        }
    }
    let val = load_from(&path)?;
    let mtime = file_mtime(&path);
    let mut guard = CACHE.lock().unwrap();
    *guard = Some(CachedConfig {
        value: val.clone(),
        mtime,
        checked_at: Instant::now(),
    });
    Ok(val)
}

pub fn load_from(path: &Path) -> anyhow::Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_else(|_| json!({}))),
        Err(_) => Ok(json!({})),
    }
}

pub fn to_string(cfg: &Value) -> String {
    // pretty-print, 2-space indent
    serde_json::to_string_pretty(cfg).unwrap_or_else(|_| "{}".into())
}

fn write_and_cache(cfg: &Value) -> anyhow::Result<()> {
    let path = crate::paths::config_file();
    crate::paths::atomic_write(&path, to_string(cfg).as_bytes())?;
    let mtime = file_mtime(&path);
    let mut guard = CACHE.lock().unwrap();
    *guard = Some(CachedConfig {
        value: cfg.clone(),
        mtime,
        checked_at: Instant::now(),
    });
    Ok(())
}

pub fn save(cfg: &Value) -> anyhow::Result<()> {
    let _g = WRITE_LOCK.lock().unwrap();
    write_and_cache(cfg)
}

/// Atomic read-modify-write; the mutator returns Some(new) to save.
pub fn update<R>(
    mutator: impl FnOnce(&mut Value) -> Option<R>,
) -> anyhow::Result<(Value, Option<R>)> {
    let _g = WRITE_LOCK.lock().unwrap();
    let path = crate::paths::config_file();
    let mut cfg = load_from(&path)?;
    let r = mutator(&mut cfg);
    if r.is_some() {
        write_and_cache(&cfg)?;
    }
    Ok((cfg, r))
}

fn cfg_str(cfg: &Value, key: &str) -> Option<String> {
    cfg.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

pub fn shared_secret() -> String {
    cfg_str(&load().unwrap_or_else(|_| json!({})), "shared_secret")
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn allowed_hosts() -> Vec<String> {
    let cfg = load().unwrap_or_else(|_| json!({}));
    match cfg.get("allowed_hosts").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_lowercase().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => Vec::new(),
    }
}

pub fn allowed_origins() -> Vec<String> {
    let cfg = load().unwrap_or_else(|_| json!({}));
    match cfg.get("allowed_origins").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| {
                s.trim()
                    .trim_end_matches('/')
                    .to_lowercase()
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .collect(),
        None => Vec::new(),
    }
}

pub fn listen_host() -> String {
    let cfg = load().unwrap_or_else(|_| json!({}));
    match cfg_str(&cfg, "listen_host").as_deref() {
        Some("0.0.0.0") => "0.0.0.0".into(),
        _ => "127.0.0.1".into(),
    }
}

pub fn network_mode() -> bool {
    listen_host() == "0.0.0.0"
}

pub fn allow_browser() -> bool {
    let cfg = load().unwrap_or_else(|_| json!({}));
    cfg.get("allow_browser").and_then(|v| v.as_bool()) != Some(false)
}

/// Tray settings (server-side keys).
pub fn tray_settings() -> (bool, f64) {
    let cfg = load().unwrap_or_else(|_| json!({}));
    let close = cfg.get("close_to_tray").and_then(|v| v.as_bool()) != Some(false);
    let minutes = cfg
        .get("tray_idle_unload_min")
        .and_then(|v| v.as_f64())
        .unwrap_or(5.0);
    (close, minutes.max(0.0))
}

pub fn gpu_device_id() -> Option<i64> {
    load()
        .unwrap_or_else(|_| json!({}))
        .get("gpu_device_id")
        .and_then(|v| v.as_i64())
        .filter(|v| *v >= 0)
}

/// Server keys the client snapshot must not overwrite.
pub fn merge_client_config(
    data: &Map<String, Value>,
    origin_is_foreign: bool,
    picked_folders: &[PathBuf],
) -> Value {
    let existing = load().unwrap_or_else(|_| json!({}));
    let mut merged = Value::Object(data.clone());

    if let Value::Object(m) = &mut merged {
        // selected_model / gpu_device_id / device_cache change only via dedicated backend paths
        for key in ["selected_model", "gpu_device_id", "device_cache"] {
            if let Some(v) = existing.get(key) {
                m.insert(key.into(), v.clone());
            } else {
                m.remove(key);
            }
        }
        // server keys missing from the body keep their values
        for key in [
            "close_to_tray",
            "tray_idle_unload_min",
            "allowed_origins",
            "shared_secret",
            "allowed_hosts",
            "listen_host",
            "allow_browser",
        ] {
            let absent = data
                .get(key)
                .map(|v| v.is_null())
                .unwrap_or(true);
            if (absent || origin_is_foreign)
                && let Some(v) = existing.get(key)
            {
                m.insert(key.into(), v.clone());
            } else if origin_is_foreign {
                m.remove(key);
            }
        }
        // foreign origins cannot overwrite local folder paths; local origins can only
        // save folders that were already present or picked via the native dialog
        if origin_is_foreign {
            if let Some(v) = existing.get("lastFolders") {
                m.insert("lastFolders".into(), v.clone());
            } else {
                m.remove("lastFolders");
            }
        } else if let Some(lf) = m.get_mut("lastFolders").and_then(|v| v.as_object_mut()) {
            let mut allowed: Vec<PathBuf> = picked_folders
                .iter()
                .map(|p| crate::paths::norm_folder(p))
                .collect();
            if let Some(existing_lf) = existing.get("lastFolders").and_then(|v| v.as_object()) {
                for v in existing_lf.values() {
                    if let Some(s) = v.as_str()
                        && !s.is_empty()
                    {
                        allowed.push(crate::paths::norm_folder(Path::new(s)));
                    }
                }
            }
            for (k, v) in lf.iter_mut() {
                if let Some(s) = v.as_str()
                    && !s.is_empty()
                    && !allowed.contains(&crate::paths::norm_folder(Path::new(s)))
                {
                    *v = existing
                        .get("lastFolders")
                        .and_then(|o| o.get(k))
                        .cloned()
                        .unwrap_or(Value::Null);
                }
            }
        }
    }
    merged
}

/// Writes config.json (top-level merge).
pub fn set_key(key: &str, value: Value) -> anyhow::Result<()> {
    update(|cfg| {
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert(key.to_string(), value.clone());
        }
        Some(())
    })?;
    Ok(())
}
