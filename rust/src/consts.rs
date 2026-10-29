// Статические константы приложения (порт версии.py).
pub const VERSION: &str = "26.4.0";
pub const APP_NAME: &str = "Tokenatra";
pub const GITHUB_REPO: &str = "SweetyHake/Tokenatra";
pub const PORT: u16 = 7878;

#[allow(dead_code)] // паритет version.py
pub const MODEL_FILE: &str = "model.onnx";

pub fn model_download_url() -> String {
    std::env::var("TOKENATRA_MODEL_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL_DOWNLOAD_URL.to_string())
}

pub fn model_download_url_lite() -> String {
    std::env::var("TOKENATRA_MODEL_URL_LITE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL_DOWNLOAD_URL_LITE.to_string())
}

const DEFAULT_MODEL_DOWNLOAD_URL: &str = "https://www.dropbox.com/scl/fi/r8yxydp1phmief0fed5k6/RMBG_2.0_fp16.onnx?rlkey=otrroli64yc3ridsz80oj9eul&st=t78dfodp&dl=1";
const DEFAULT_MODEL_DOWNLOAD_URL_LITE: &str = "https://huggingface.co/PramaLLC/BEN2/resolve/main/BEN2_Base.onnx";

/// Встроенные кольца: имя файла (casefold) -> отображаемое имя (переводится через I18n).
pub fn builtin_ring_name(file_name_lower: &str) -> Option<&'static str> {
    Some(match file_name_lower {
        "steel.webp" => "Сталь",
        "ag-obsidian-red.webp" => "Красный обсидиан",
        "ag-stormsteel.webp" => "Штормовая сталь",
        "ag-wood-runic.png" => "Руническое дерево",
        "high-noon.webp" => "Ковбой (High Noon)",
        "plague.webp" => "Чумной венок",
        "purple-crystals.webp" => "Аметистовые кристаллы",
        "theathre.webp" => "Театр",
        "white-runic.webp" => "Белые руны",
        _ => return None,
    })
}

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif"];
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "m4a", "flac", "aac", "wma", "opus", "ogg", "mp4", "mkv", "mov", "avi",
    "webm", "mpeg", "mpg", "m4v",
];

pub const MAX_IMAGE_DIMENSION: u32 = 8192;
/// Рабочее разрешение фоновой обработки (как MAX_PROCESS_DIM в Python).
pub const MAX_PROCESS_DIM: u32 = 4096;
pub const MAX_MEGAPIXELS: u64 = 100_000_000;

/// Версия протокола модуля-коннектора Foundry.
pub const CONNECTOR_API_VERSION: i64 = 1;

// ===== Интеграция с FoundryVTT =====
pub const CONNECTOR_ROUTES: &[&str] = &[
    "/health", "/process", "/version", "/device", "/models_list", "/embed_export",
];
pub const CONNECTOR_ROUTE_PREFIXES: &[&str] = &["/embed_export/"];

pub const PUBLIC_PROXY_ROUTES: &[&str] = &[
    "/health", "/process", "/version", "/device", "/models_list", "/embed_export", "/icon",
    "/logo", "/mask", "/ring", "/example", "/presets_list", "/rings_list",
];
pub const PUBLIC_PROXY_ROUTE_PREFIXES: &[&str] =
    &["/embed_export/", "/static/", "/preset_file/", "/ring_file/"];

pub const PUBLIC_OPEN_EXACT: &[&str] = &[
    "/", "/splash", "/icon", "/logo", "/mask", "/ring", "/example", "/presets_list",
    "/rings_list",
];
pub const PUBLIC_OPEN_PREFIXES: &[&str] = &["/static/", "/preset_file/", "/ring_file/"];

#[allow(dead_code)]
pub fn is_windows() -> bool {
    cfg!(windows)
}

pub fn platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}
