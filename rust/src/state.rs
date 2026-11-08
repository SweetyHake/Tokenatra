use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// ONNX provider configuration (result of get_providers).
#[derive(Clone, Debug)]
pub struct ProviderConfig {
    /// Provider names in priority order.
    pub providers: Vec<String>,
    /// DML device_id (None = default adapter).
    pub dml_device_id: Option<i32>,
}

pub struct AppState {
    /// Device name for /device and /health.
    pub device_name: RwLock<String>,
    pub provider_cfg: RwLock<Option<ProviderConfig>>,
    /// Inference monopoly (session_lock equivalent): from session setup
    /// to run(); two live DML sessions break ExecuteKernel.
    pub session_lock: Mutex<Option<crate::infer::Session>>,
    /// Lock-free indicator of whether the ONNX session is currently loaded.
    pub model_loaded: AtomicBool,
    /// Tray mode (window hidden).
    pub tray_mode: AtomicBool,
    pub last_activity: AtomicU64, // unix seconds
    /// Folders picked via the dialog this session (save_to_folder allowlist).
    pub picked_folders: Mutex<Vec<PathBuf>>,
    /// Embed export registry: filename -> entry.
    pub embed_exports: Mutex<HashMap<String, EmbedExport>>,
}

#[derive(Clone)]
pub struct EmbedExport {
    pub path: PathBuf,
    pub mime: &'static str,
    pub expires: Instant,
}

pub const EMBED_EXPORT_TTL: Duration = Duration::from_secs(600);
pub const EMBED_EXPORT_MAX_ITEMS: usize = 32;
pub const EMBED_EXPORT_MAX_BYTES: u64 = 40 * 1024 * 1024;

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            device_name: RwLock::new("Определение…".into()),
            provider_cfg: RwLock::new(None),
            session_lock: Mutex::new(None),
            model_loaded: AtomicBool::new(false),
            tray_mode: AtomicBool::new(false),
            last_activity: AtomicU64::new(now_secs()),
            picked_folders: Mutex::new(Vec::new()),
            embed_exports: Mutex::new(HashMap::new()),
        })
    }

    pub fn set_model_loaded(&self, loaded: bool) {
        self.model_loaded.store(loaded, Ordering::SeqCst);
        let _ = crate::gui_control::send(crate::gui_control::GuiCommand::ModelStateChanged(loaded));
    }

    pub fn is_model_loaded(&self) -> bool {
        self.model_loaded.load(Ordering::SeqCst)
    }

    pub fn mark_activity(&self) {
        self.last_activity.store(now_secs(), Ordering::SeqCst);
    }

    pub fn seconds_idle(&self) -> u64 {
        now_secs().saturating_sub(self.last_activity.load(Ordering::SeqCst))
    }

    pub fn set_tray_mode(&self, on: bool) {
        self.tray_mode.store(on, Ordering::SeqCst);
        self.mark_activity();
    }

    pub fn tray_mode(&self) -> bool {
        self.tray_mode.load(Ordering::SeqCst)
    }

    pub fn device_name(&self) -> String {
        self.device_name.read().unwrap().clone()
    }

    /// Secret id for embed export.
    pub fn new_embed_id() -> String {
        use base64::Engine;
        let buf: [u8; 16] = rand::random();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
    }

    /// Drops expired entries (call under the embed_exports lock).
    pub fn embed_export_cleanup(map: &mut HashMap<String, EmbedExport>) {
        let now = Instant::now();
        let expired: Vec<String> = map
            .iter()
            .filter(|(_, e)| e.expires <= now)
            .map(|(k, _)| k.clone())
            .collect();
        for name in expired {
            if let Some(item) = map.remove(&name) {
                let _ = std::fs::remove_file(&item.path);
            }
        }
    }

    pub fn purge_embed_export_dir(&self) {
        self.embed_exports.lock().unwrap().clear();
        if let Ok(rd) = std::fs::read_dir(crate::paths::embed_export_dir()) {
            for e in rd.filter_map(|e| e.ok()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    /// Clears the export directory (app startup).
    pub fn remember_picked_folder(&self, p: PathBuf) {
        let norm = crate::paths::norm_folder(&p);
        let mut f = self.picked_folders.lock().unwrap();
        if !f.iter().any(|x| crate::paths::norm_folder(x) == norm) {
            f.push(p);
        }
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
