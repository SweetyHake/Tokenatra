use std::path::Path;
use std::sync::Mutex;

use serde_json::{json, Map, Value};

/// Чтение config.json (пустой объект при отсутствии/повреждении).
pub fn load() -> anyhow::Result<Value> {
    load_from(&crate::paths::config_file())
}

pub fn load_from(path: &Path) -> anyhow::Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_else(|_| json!({}))),
        Err(_) => Ok(json!({})),
    }
}

static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn to_string(cfg: &Value) -> String {
    // ensure_ascii=False + indent 2, как в Python
    serde_json::to_string_pretty(cfg).unwrap_or_else(|_| "{}".into())
}

pub fn save(cfg: &Value) -> anyhow::Result<()> {
    let _g = WRITE_LOCK.lock().unwrap();
    std::fs::write(crate::paths::config_file(), to_string(cfg))?;
    Ok(())
}

/// Атомарный read-modify-write. mutator возвращает Some(new) для записи.
pub fn update<R>(
    mutator: impl FnOnce(&mut Value) -> Option<R>,
) -> anyhow::Result<(Value, Option<R>)> {
    let _g = WRITE_LOCK.lock().unwrap();
    let mut cfg = load()?;
    let r = mutator(&mut cfg);
    if r.is_some() {
        std::fs::write(crate::paths::config_file(), to_string(&cfg))?;
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

/// Настройки трея (серверные ключи, клиент их не хранит).
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

/// Серверные ключи, которые клиентский снапшот /config не может затереть.
pub fn merge_client_config(data: &Map<String, Value>, origin_is_foreign: bool) -> Value {
    let existing = load().unwrap_or_else(|_| json!({}));
    let mut merged = Value::Object(data.clone());

    if let Value::Object(m) = &mut merged {
        // selected_model / gpu_device_id меняются только /select_model и tune-gpu
        for key in ["selected_model", "gpu_device_id"] {
            if let Some(v) = existing.get(key) {
                m.insert(key.into(), v.clone());
            }
        }
        // Серверные ключи, которых нет в теле — берём существующие
        for key in [
            "close_to_tray",
            "tray_idle_unload_min",
            "allowed_origins",
            "shared_secret",
            "allowed_hosts",
            "listen_host",
        ] {
            let absent = data
                .get(key)
                .map(|v| v.is_null())
                .unwrap_or(true);
            if absent {
                if let Some(v) = existing.get(key) {
                    m.insert(key.into(), v.clone());
                }
            }
        }
        // Чужой origin не может менять сетевые ключи безопасности
        if origin_is_foreign {
            for key in [
                "shared_secret",
                "listen_host",
                "allowed_origins",
                "allowed_hosts",
                "allow_browser",
            ] {
                m.remove(key);
            }
        }
    }
    merged
}

/// Записать значение в config.json (merge на верхнем уровне).
pub fn set_key(key: &str, value: Value) -> anyhow::Result<()> {
    update(|cfg| {
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert(key.to_string(), value.clone());
        }
        Some(())
    })?;
    Ok(())
}
