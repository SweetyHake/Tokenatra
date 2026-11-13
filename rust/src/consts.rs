// App constants.
pub const VERSION: &str = "26.5.2";
pub const APP_NAME: &str = "Tokenatra";
pub const GITHUB_REPO: &str = "SweetyHake/Tokenatra";
pub const PORT: u16 = 7878;

#[allow(dead_code)]
pub const MODEL_FILE: &str = "model.onnx";

pub fn model_download_url() -> String {
    std::env::var("TOKENATRA_MODEL_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL_DOWNLOAD_URL.to_string())
}

const DEFAULT_MODEL_DOWNLOAD_URL: &str = "https://huggingface.co/PramaLLC/BEN2/resolve/main/BEN2_Base.onnx";

/// Built-in rings: filename -> display name (i18n at runtime).
pub fn builtin_ring_name(file_name_lower: &str) -> Option<&'static str> {
    Some(match file_name_lower {
        "ag-obsidian-red.webp" => "Кровавый обсидиан",
        "ag-wood-runic.webp" => "Рунное дерево",
        "archer.webp" => "Стрелок",
        "armor_2x.webp" => "Доспех",
        "chronopunk.webp" => "Хронопанк",
        "circus.webp" => "Цирк",
        "cosmic.webp" => "Космос",
        "cristmas_2x.webp" => "Рождество",
        "druid.webp" => "Друид",
        "eldritch.webp" => "Бездна",
        "feywild.webp" => "Фейвайльд",
        "gold-white-rune.webp" => "Золотые руны",
        "high-noon.webp" => "Ковбой",
        "kraken.webp" => "Морская пучина",
        "lava.webp" => "Лава",
        "plague.webp" => "Чума",
        "rogue.webp" => "Теневое ремесло",
        "rune-gold.webp" => "Руны: золото",
        "rune-green.webp" => "Руны: зелёные",
        "rune-ice.webp" => "Руны: лёд",
        "rune-purple.webp" => "Руны: фиолетовые",
        "rune-red.webp" => "Руны: красные",
        "rune-steel.webp" => "Руны: сталь",
        "stained-glass.webp" => "Витраж",
        "tentacle.webp" => "Щупальца",
        "theathre.webp" => "Театр",
        _ => return None,
    })
}

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif"];
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "m4a", "flac", "aac", "wma", "opus", "ogg", "mp4", "mkv", "mov", "avi",
    "webm", "mpeg", "mpg", "m4v",
];

pub const MAX_IMAGE_DIMENSION: u32 = 8192;
/// Processing resolution cap.
pub const MAX_PROCESS_DIM: u32 = 4096;
pub const MAX_MEGAPIXELS: u64 = 100_000_000;

/// Foundry connector protocol version.
pub const CONNECTOR_API_VERSION: i64 = 1;

// ===== FoundryVTT integration =====
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
