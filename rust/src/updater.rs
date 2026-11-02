use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::consts::{APP_NAME, GITHUB_REPO, VERSION};

const CHECK_INTERVAL_HOURS: u64 = 6;

#[derive(Clone, Debug)]
pub struct UpdateState {
    pub checked: bool,
    pub check_error: bool,
    pub available: bool,
    pub url: Option<String>,
    pub tag: Option<String>,
    pub download_progress: i64,
    pub download_active: bool,
    pub download_done: bool,
    pub download_error: Option<String>,
    pub download_path: Option<String>,
    pub download_kind: Option<String>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            checked: false,
            check_error: false,
            available: false,
            url: None,
            tag: None,
            download_progress: -1,
            download_active: false,
            download_done: false,
            download_error: None,
            download_path: None,
            download_kind: None,
        }
    }
}

fn state() -> &'static Mutex<UpdateState> {
    static S: OnceLock<Mutex<UpdateState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(UpdateState::default()))
}

pub fn snapshot() -> Value {
    let s = state().lock().unwrap();
    json!({
        "update_checked": s.checked,
        "check_error": s.check_error,
        "update_available": s.available,
        "update_url": s.url,
        "update_tag": s.tag,
        "current_version": VERSION,
        "download_progress": s.download_progress,
        "download_active": s.download_active,
        "download_done": s.download_done,
        "download_error": s.download_error,
        "download_path": s.download_path,
        "download_kind": s.download_kind,
    })
}

pub fn reset_check_state() {
    let mut s = state().lock().unwrap();
    *s = UpdateState::default();
}

fn parse_version(v: &str) -> (u64, u64, u64) {
    let parts: Vec<u64> = v
        .split('.')
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    )
}

pub fn update_dir() -> PathBuf {
    let d = crate::paths::user_data_dir().join("update");
    if std::fs::create_dir_all(&d).is_err() {
        let d = std::env::temp_dir().join("Tokenatra_update");
        let _ = std::fs::create_dir_all(&d);
        return d;
    }
    d
}

fn throttled() -> bool {
    let path = update_dir().join(".last_update_check");
    match std::fs::metadata(&path) {
        Ok(m) => match m.modified() {
            Ok(t) => SystemTime::now()
                .duration_since(t)
                .map(|d| d < Duration::from_secs(CHECK_INTERVAL_HOURS * 3600))
                .unwrap_or(false),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

fn touch_throttle() {
    let _ = std::fs::File::create(update_dir().join(".last_update_check"));
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(format!("{APP_NAME}/{VERSION}"))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("http client")
}

fn find_exe_asset(assets: &[Value]) -> Option<String> {
    let mut bare = None;
    for a in assets {
        let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let lower = name.to_lowercase();
        if !lower.ends_with(".exe") || !lower.contains(&APP_NAME.to_lowercase()) {
            continue;
        }
        if lower.contains("setup") || lower.contains("installer") {
            return a.get("browser_download_url").and_then(|v| v.as_str()).map(String::from);
        }
        if bare.is_none() {
            bare = a.get("browser_download_url").and_then(|v| v.as_str()).map(String::from);
        }
    }
    bare
}

fn find_unix_asset(assets: &[Value]) -> Option<String> {
    let ext = if cfg!(target_os = "macos") { ".dmg" } else { ".appimage" };
    let machine = std::env::consts::ARCH;
    let arches: &[&str] = if machine == "arm64" || machine == "aarch64" {
        &["arm64", "aarch64"]
    } else {
        &["x86_64"]
    };
    let mut exact = None;
    let mut fallback = None;
    for a in assets {
        let lower = a
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        if !lower.ends_with(ext) || !lower.contains(&APP_NAME.to_lowercase()) {
            continue;
        }
        if arches.iter().any(|a| lower.contains(&format!("_{a}."))) {
            exact = a.get("browser_download_url").and_then(|v| v.as_str()).map(String::from);
            break;
        }
        if fallback.is_none() {
            fallback = a.get("browser_download_url").and_then(|v| v.as_str()).map(String::from);
        }
    }
    exact.or(fallback)
}

fn expected_update_extension() -> Option<&'static str> {
    if cfg!(windows) {
        Some(".exe")
    } else if cfg!(target_os = "macos") {
        Some(".dmg")
    } else if cfg!(target_os = "linux") {
        Some(".appimage")
    } else {
        None
    }
}

pub fn check_for_updates(force: bool) {
    let mut s = state().lock().unwrap();
    if GITHUB_REPO.is_empty() {
        s.checked = true;
        s.available = false;
        return;
    }
    if !force && throttled() {
        return;
    }
    drop(s);

    let result = (|| -> anyhow::Result<Value> {
        let resp = client()
            .get(format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest"))
            .header("Accept", "application/vnd.github.v3+json")
            .send()?
            .error_for_status()?;
        let status = resp.status();
        let text = resp.text()?;
        println!("  [update] API status: {status}, body: {} bytes", text.len());
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            println!(
                "  [update] tag: {:?}, message: {:?}",
                v.get("tag_name").and_then(|t| t.as_str()),
                v.get("message").and_then(|t| t.as_str())
            );
        }
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("json: {e}"))
    })();

    let mut s = state().lock().unwrap();
    match result {
        Err(e) => {
            println!("Update check failed: {e}");
            s.checked = true;
            s.available = false;
            s.check_error = true;
        }
        Ok(data) => {
            let tag = data
                .get("tag_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim_start_matches('v')
                .to_string();
            let assets = data
                .get("assets")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if parse_version(&tag) <= parse_version(VERSION) {
                touch_throttle();
                s.checked = true;
                s.available = false;
                s.check_error = false;
                return;
            }
            let url = if cfg!(windows) {
                find_exe_asset(&assets)
            } else if cfg!(any(target_os = "macos", target_os = "linux")) {
                find_unix_asset(&assets)
            } else {
                None
            };
            let url = url.or_else(|| {
                data.get("html_url")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
            touch_throttle();
            s.checked = true;
            s.check_error = false;
            s.available = true;
            s.url = url;
            s.tag = Some(tag);
        }
    }
}

pub fn download_update() {
    {
        let mut s = state().lock().unwrap();
        if s.download_active || s.download_done {
            return;
        }
        s.download_active = true;
        let url = s.url.clone();
        drop(s);

        let Some(url) = url else {
            {
                let mut s2 = state().lock().unwrap();
                s2.download_active = false;
                s2.download_error = Some("No download URL".into());
            }
            return;
        };

        let clean = url.split('?').next().unwrap_or(&url).to_string();
        let lower = clean.to_lowercase();
        let expected = expected_update_extension();
        let ok = match expected {
            Some(".exe") => lower.ends_with(".exe"),
            Some(e) => lower.ends_with(e),
            None => false,
        };
        if !ok {
            let mut s = state().lock().unwrap();
            s.download_active = false;
            s.download_error = Some(format!(
                "В релизе нет подходящего файла ({}) для этой платформы. Скачайте вручную с GitHub Releases",
                expected.unwrap_or("?")
            ));
            return;
        }

        let kind = if cfg!(windows) {
            let name = clean.rsplit(['/', '\\']).next().unwrap_or_default().to_lowercase();
            if name.contains("setup") || name.contains("installer") {
                "installer"
            } else {
                "bare"
            }
        } else if cfg!(target_os = "macos") {
            "dmg"
        } else {
            "appimage"
        };
        {
            let mut s3 = state().lock().unwrap();
            s3.download_kind = Some(kind.into());
        }
    }

    let result = (|| -> anyhow::Result<PathBuf> {
        let url = state().lock().unwrap().url.clone().unwrap_or_default();
        let _clean = url.split('?').next().unwrap_or_default().to_string();
        let dest = update_dir().join(format!(
            "Tokenatra_update{}",
            expected_update_extension().unwrap_or("")
        ));
        // clean up old update files
        if let Ok(rd) = std::fs::read_dir(update_dir()) {
            for e in rd.filter_map(|e| e.ok()) {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("Tokenatra_update")
                    && e.path() != dest
                {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }

        let dl = reqwest::blocking::Client::builder()
            .user_agent(format!("{APP_NAME}/{VERSION}"))
            .timeout(Duration::from_secs(300))
            .build()?;
        let mut resp = dl.get(&url).send()?.error_for_status()?;
        let total = resp.content_length().unwrap_or(0);
        let mut file = std::io::BufWriter::new(std::fs::File::create(&dest)?);
        let mut downloaded: u64 = 0;
        let mut buf = [0u8; 65536];
        loop {
            let n = resp.read(&mut buf)?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut file, &buf[..n])?;
            downloaded += n as u64;
            if total > 0 {
                let pct = (downloaded * 100 / total) as i64;
                state().lock().unwrap().download_progress = pct.min(100);
            }
        }
        drop(file);
        let size = std::fs::metadata(&dest)?.len();
        if size < 1024 * 1024 {
            let _ = std::fs::remove_file(&dest);
            anyhow::bail!("Скачанный файл повреждён (слишком маленький)");
        }
        Ok(dest)
    })();

    let mut s = state().lock().unwrap();
    match result {
        Ok(path) => {
            s.download_active = false;
            s.download_done = true;
            s.download_error = None;
            s.download_progress = 100;
            s.download_path = Some(path.to_string_lossy().to_string());
            println!("Update downloaded to {}", path.display());
        }
        Err(e) => {
            s.download_active = false;
            s.download_done = false;
            s.download_error = Some(e.to_string());
        }
    }
}

pub fn start_background_tasks(force: bool, delay_secs: u64) {
    std::thread::spawn(move || {
        if delay_secs > 0 {
            std::thread::sleep(Duration::from_secs(delay_secs));
        }
        check_for_updates(force);
    });
}

// reqwest::blocking::Response read
trait ReadExt {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

impl ReadExt for reqwest::blocking::Response {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        
        std::io::Read::read(self, buf)
    }
}
