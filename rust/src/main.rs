// Release builds use the Windows GUI subsystem (no console window).
// Debug builds keep the console for println and --server output.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod cli;
mod config;
mod consts;
mod ffmpeg;
mod gui_control;
mod http;
mod imaging;
mod infer;
mod mcp;
mod paths;
mod shell;
mod state;
mod tray;
mod updater;
#[cfg(windows)]
mod win;

use std::sync::Arc;

use crate::state::AppState;

fn print_banner(device: &str) {
    println!();
    println!("{}", "=".repeat(50));
    println!("Background Remover & Tokenatra");
    println!("{}", "=".repeat(50));
    println!("Device: {device}");
    println!("http://localhost:{}", consts::PORT);
    println!("{}", "=".repeat(50));
    println!();
}

fn spawn_server(
    state: Arc<AppState>,
) -> (std::thread::JoinHandle<()>, std::sync::mpsc::Receiver<bool>) {
    let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
    let handle = std::thread::Builder::new()
        .name("http-server".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let app = http::router(state.clone());
                let host = config::listen_host();
                let addr = format!("{host}:{}", consts::PORT);
                match tokio::net::TcpListener::bind(&addr).await {
                    Ok(listener) => {
                        let _ = tx.send(true);
                        println!("Serving on http://{addr}");
                        if let Err(e) = axum::serve(listener, app).await {
                            eprintln!("server error: {e}");
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(false);
                        eprintln!("Не удалось занять порт {}: {e}", consts::PORT);
                    }
                }
            });
        })
        .expect("spawn server thread");
    (handle, rx)
}

fn wait_for_server(bound_rx: std::sync::mpsc::Receiver<bool>, deadline_secs: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(deadline_secs);
    match bound_rx.recv_timeout(std::time::Duration::from_secs(deadline_secs)) {
        Ok(true) => {}
        Ok(false) | Err(_) => return false,
    }
    let url = format!("http://localhost:{}/health", consts::PORT);
    let client = reqwest::blocking::Client::new();
    while std::time::Instant::now() < deadline {
        if let Ok(resp) = client
            .get(&url)
            .timeout(std::time::Duration::from_secs(1))
            .send()
            && let Ok(v) = resp.json::<serde_json::Value>()
            && v.get("app").and_then(|a| a.as_str()) == Some(consts::APP_NAME)
        {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

/// Idle monitor: cleans embed exports and unloads the ONNX model
/// when idle for the configured timeout. Starts once per process.
fn ensure_tray_monitor(state: &Arc<AppState>) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let st = state.clone();
    std::thread::Builder::new()
        .name("tray-idle-monitor".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            {
                let mut map = st.embed_exports.lock().unwrap();
                AppState::embed_export_cleanup(&mut map);
            }
            let (_, minutes) = config::tray_settings();
            if minutes <= 0.0 {
                continue;
            }
            if infer::model_loaded(&st) && st.seconds_idle() as f64 > minutes * 60.0 {
                infer::unload_model(&st);
            }
        })
        .expect("spawn tray monitor");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Explorer context menu verbs
    let mut i = 1;
    while i < args.len() {
        let code = match args[i].as_str() {
            "--remove-bg" => args.get(i + 1).map(|f| cli::remove_bg(f)),
            "--to-webp" => args.get(i + 1).map(|f| cli::to_webp(f)),
            "--to-ogg" => args.get(i + 1).map(|f| cli::to_ogg(f)),
            "--folder-to-webp" => args.get(i + 1).map(|f| cli::folder_to_webp(f)),
            _ => None,
        };
        if let Some(code) = code {
            std::process::exit(code);
        }
        i += 1;
    }

    if args.iter().any(|a| a == "--mcp") {
        std::process::exit(mcp::run_stdio());
    }

    if args.iter().any(|a| a == "--install-mcp") {
        let _ = mcp::register_mcp_clients(true);
        return;
    }

    if args.iter().any(|a| a == "--tune-gpu") {
        paths::migrate_legacy_model();
        infer::cli_tune_gpu();
        return;
    }

    let server_only = args.iter().any(|a| a == "--server" || a == "--headless");

    // Linux (GTK3 / WebKitGTK): prefer X11/XWayland for frameless drag/resize
    // (fallback to Wayland), dark GTK theme, and disable DMABUF renderer
    // (avoids blank WebKitGTK window on WSLg/NVIDIA/VMs) unless overridden by user.
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("GDK_BACKEND").is_none() {
            unsafe { std::env::set_var("GDK_BACKEND", "x11,wayland") };
        }
        if std::env::var_os("GTK_THEME").is_none() {
            unsafe { std::env::set_var("GTK_THEME", "Adwaita:dark") };
        }
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
        }
    }

    paths::migrate_legacy_model();
    let state = AppState::new();
    state.purge_embed_export_dir();

    // device detection (PowerShell/DXGI) and provider cache
    infer::detect_device(&state);

    if !server_only {
        #[cfg(windows)]
        win::set_taskbar_identity();
        #[cfg(windows)]
        win::setup_webview2_gpu();
        #[cfg(windows)]
        win::enable_windows_dark_dialogs();

        // previous instance PID + free the port
        let pid_file = paths::pid_file();
        if let Ok(old) = std::fs::read_to_string(&pid_file)
            && let Ok(old_pid) = old.trim().parse::<u32>()
                && old_pid != std::process::id() {
                    #[cfg(windows)]
                    win::terminate_process(old_pid);
                    #[cfg(not(windows))]
                    kill_previous_pid_unix(old_pid);
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
        #[cfg(windows)]
        win::kill_process_on_port(consts::PORT);
        #[cfg(not(windows))]
        kill_process_on_port_unix(consts::PORT);

        let _ = std::fs::write(&pid_file, std::process::id().to_string());

        #[cfg(windows)]
        win::register_context_menu();
        #[cfg(target_os = "linux")]
        register_context_menu_linux();

        let _ = mcp::register_mcp_clients(false);
    }

    // background update check (once per session)
    updater::start_background_tasks(true, 15);
    ensure_tray_monitor(&state);

    let (server, bound_rx) = spawn_server(state.clone());
    if server_only {
        print_banner(&state.device_name());
        let _ = server.join();
        return;
    }

    if !wait_for_server(bound_rx, 15) {
        eprintln!(
            "Не удалось запустить локальный сервер Tokenatra на порту {}",
            consts::PORT
        );
    }

    let code = shell::run_gui(state.clone());
    std::process::exit(code);
}

#[cfg(not(windows))]
fn libc_kill(pid: i32) {
    let _ = std::process::Command::new("kill").arg(pid.to_string()).output();
}

#[cfg(not(windows))]
fn kill_previous_pid_unix(pid: u32) {
    if pid == 0 || pid == std::process::id() {
        return;
    }
    // Guard against PID reuse: only signal if the process command matches tokenatra.
    if let Ok(out) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
    {
        let comm = String::from_utf8_lossy(&out.stdout).to_lowercase();
        if comm.contains("tokenatra") {
            libc_kill(pid as i32);
        }
    }
}

#[cfg(not(windows))]
fn kill_process_on_port_unix(port: u16) {
    let self_pid = std::process::id();
    // Strictly filter to LISTEN sockets and verify via kill_previous_pid_unix
    // so neither client browsers nor foreign processes are ever killed.
    if let Ok(o) = std::process::Command::new("lsof")
        .args(["-t", "-i", &format!("tcp:{port}"), "-sTCP:LISTEN"])
        .output()
    {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines() {
            if let Ok(pid) = line.trim().parse::<u32>()
                && pid != self_pid
            {
                kill_previous_pid_unix(pid);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn register_context_menu_linux() {
    let exe = std::env::var("APPIMAGE")
        .ok()
        .filter(|p| std::path::Path::new(p).is_file())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        });
    let Some(exe) = exe else { return };
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));

    // KDE Dolphin ServiceMenu
    let kio_dir = data_home.join("kio/servicemenus");
    if std::fs::create_dir_all(&kio_dir).is_ok() {
        let desktop_file = kio_dir.join("tokenatra.desktop");
        let content = format!(
            "[Desktop Entry]\n\
             Type=Service\n\
             X-KDE-ServiceTypes=KonqPopupMenu/Plugin\n\
             MimeType=image/png;image/jpeg;image/webp;image/bmp;image/gif;image/tiff;audio/*;video/*;inode/directory;\n\
             Actions=RemoveBg;ToWebp;ToOgg;FolderToWebp;\n\
             \n\
             [Desktop Action RemoveBg]\n\
             Name=Удалить фон (Tokenatra)\n\
             Icon=tokenatra\n\
             Exec=\"{exe}\" --remove-bg %f\n\
             \n\
             [Desktop Action ToWebp]\n\
             Name=Конвертировать в WebP (Tokenatra)\n\
             Icon=tokenatra\n\
             Exec=\"{exe}\" --to-webp %f\n\
             \n\
             [Desktop Action ToOgg]\n\
             Name=Конвертировать в OGG (Tokenatra)\n\
             Icon=tokenatra\n\
             Exec=\"{exe}\" --to-ogg %f\n\
             \n\
             [Desktop Action FolderToWebp]\n\
             Name=Все изображения папки в WebP (Tokenatra)\n\
             Icon=tokenatra\n\
             Exec=\"{exe}\" --folder-to-webp %f\n"
        );
        let _ = std::fs::write(&desktop_file, content);
    }
}

#[cfg(target_os = "linux")]
pub fn unregister_context_menu_linux() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let desktop_file = data_home.join("kio/servicemenus/tokenatra.desktop");
    let _ = std::fs::remove_file(desktop_file);
}
