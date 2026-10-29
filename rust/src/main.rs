mod cli;
mod config;
mod consts;
mod ffmpeg;
mod gui_control;
mod http;
mod imaging;
mod infer;
mod paths;
mod shell;
mod state;
mod tray;
mod tray_control;
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

fn spawn_server(state: Arc<AppState>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
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
                        println!("Serving on http://{addr}");
                        if let Err(e) = axum::serve(listener, app).await {
                            eprintln!("server error: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("Не удалось занять порт {}: {e}", consts::PORT);
                    }
                }
            });
        })
        .expect("spawn server thread")
}

fn wait_for_server(deadline_secs: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(deadline_secs);
    let url = format!("http://localhost:{}/splash", consts::PORT);
    while std::time::Instant::now() < deadline {
        if reqwest::blocking::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(1))
            .send()
            .is_ok()
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Монитор простоя трея (порт _tray_monitor_loop): чистит embed-экспорт
/// и выгружает модель в простое. Стартует ЛЕНИВО (как set_tray_mode(True)).
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
            if !st.tray_mode() {
                continue;
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

    // CLI-глаголы контекстного меню (как context_menu_helper.py)
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

    if args.iter().any(|a| a == "--tune-gpu") {
        paths::migrate_legacy_model();
        infer::cli_tune_gpu();
        return;
    }

    let server_only = args.iter().any(|a| a == "--server" || a == "--headless");

    // Linux (WSLg/виртуалки): принудительный X11 и мягкий рендеринг WebKit
    if !cfg!(windows) {
        unsafe { std::env::set_var("GDK_BACKEND", "x11") };
        unsafe { std::env::set_var("GTK_THEME", "Adwaita-dark") };
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    paths::migrate_legacy_model();
    let state = AppState::new();
    state.purge_embed_export_dir();

    // Определение устройства (PowerShell/DXGI) и кеш провайдеров
    infer::detect_device(&state);

    if !server_only {
        #[cfg(windows)]
        win::set_taskbar_identity();
        #[cfg(windows)]
        win::setup_webview2_gpu();
        #[cfg(windows)]
        win::enable_windows_dark_dialogs();

        // PID прошлого экземпляра + освобождение порта
        let pid_file = paths::pid_file();
        if let Ok(old) = std::fs::read_to_string(&pid_file) {
            if let Ok(old_pid) = old.trim().parse::<u32>() {
                if old_pid != std::process::id() {
                    #[cfg(windows)]
                    win::terminate_process(old_pid);
                    #[cfg(not(windows))]
                    unsafe {
                        libc_kill(old_pid as i32);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
            }
        }
        #[cfg(windows)]
        win::kill_process_on_port(consts::PORT);
        #[cfg(not(windows))]
        kill_process_on_port_unix(consts::PORT);

        let _ = std::fs::write(&pid_file, std::process::id().to_string());

        #[cfg(windows)]
        win::register_context_menu();
    }

    // Фоновая проверка обновлений (раз за сессию — реальная)
    updater::start_background_tasks(true, 15);

    let server = spawn_server(state.clone());
    if server_only {
        print_banner(&state.device_name());
        let _ = server.join();
        return;
    }

    wait_for_server(15);
    ensure_tray_monitor(&state);

    let code = shell::run_gui(state.clone());
    std::process::exit(code);
}

#[cfg(not(windows))]
unsafe fn libc_kill(pid: i32) {
    let _ = std::process::Command::new("kill").arg(pid.to_string()).output();
}

#[cfg(not(windows))]
fn kill_process_on_port_unix(port: u16) {
    let out = std::process::Command::new("lsof")
        .args(["-t", "-i", &format!("tcp:{port}")])
        .output();
    if let Ok(o) = out {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                unsafe {
                    libc_kill(pid as i32);
                }
            }
        }
    }
}
