use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::{err_status, ok_json};
use crate::consts::*;
use crate::paths;
use crate::state::AppState;

pub async fn version_info() -> Response {
    ok_json(json!({
        "version": VERSION,
        "name": APP_NAME,
        "platform": platform(),
    }))
}

pub async fn model_status() -> Response {
    let p = paths::get_selected_model_path();
    let exists = p.exists();
    let size = if exists { std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) } else { 0 };
    ok_json(json!({
        "exists": exists,
        "size": size,
        "path": p.to_string_lossy(),
        "models_dir": paths::models_dir().to_string_lossy(),
    }))
}

pub async fn device(State(state): State<std::sync::Arc<AppState>>) -> Response {
    ok_json(json!({
        "device": state.device_name(),
        "model_exists": paths::get_selected_model_path().exists(),
    }))
}

pub async fn health(State(state): State<std::sync::Arc<AppState>>) -> Response {
    let p = paths::get_selected_model_path();
    let exists = p.exists();
    ok_json(json!({
        "app": APP_NAME,
        "version": VERSION,
        "connectorApi": CONNECTOR_API_VERSION,
        "model_exists": exists,
        "model_loaded": crate::infer::model_loaded(&state),
        "model_name": exists.then(|| p.file_name().map(|s| s.to_string_lossy().to_string())).flatten(),
        "device": state.device_name(),
        "tray": state.tray_mode(),
    }))
}

pub async fn network_info() -> Response {
    let mut cands: Vec<(u32, u32, String)> = Vec::new();

    let ip_tier = |ip: &str| -> u32 {
        if ip.starts_with("192.168.") {
            0
        } else if ip.starts_with("10.") {
            1
        } else if let Some(rest) = ip.strip_prefix("172.") {
            let second = rest.split('.').next().and_then(|s| s.parse::<u32>().ok());
            match second {
                Some(s) if (16..=31).contains(&s) => 2,
                _ => 3,
            }
        } else {
            3
        }
    };

    let mut add = |ip: &str, iface_rank: u32| {
        let ip = ip.trim();
        if ip.is_empty() {
            return;
        }
        let Ok(addr) = ip.parse::<std::net::IpAddr>() else { return };
        if addr.is_loopback() {
            return;
        }
        if let std::net::IpAddr::V4(v4) = addr {
            if v4.is_link_local() || v4.is_broadcast() {
                // if_addrs has no broadcast, but the guard does not hurt
            }
        } else {
            return;
        }
        // 198.18.0.0/15 is VPN/proxy fake-IP
        if ip.starts_with("198.18.") || ip.starts_with("198.19.") {
            return;
        }
        cands.push((iface_rank, ip_tier(ip), ip.to_string()));
    };

    // default adapter
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0")
        && sock.connect("8.8.8.8:80").is_ok()
            && let Ok(a) = sock.local_addr() {
                add(&a.ip().to_string(), 2);
            }
    // walk all adapters
    let skip_kw = [
        "loopback", "vpn", "tun", "tap", "clash", "wintun", "wireguard", "tailscale",
        "zerotier", "ppp", "virtual", "mihomo", "sing-box", "vmware", "hyper-v", "vethernet",
    ];
    let prefer_kw = ["ethernet", "wi-fi", "wifi", "беспровод", "локал", "local area"];
    for iface in if_addrs::get_if_addrs().unwrap_or_default() {
        let lname = iface.name.to_lowercase();
        if skip_kw.iter().any(|k| lname.contains(k)) {
            continue;
        }
        let irank = if prefer_kw.iter().any(|k| lname.contains(k)) { 0 } else { 1 };
        if iface.ip().is_ipv4() {
            add(&iface.ip().to_string(), irank);
        }
    }
    // hostbyname extras
    
    if let Ok(name) = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME"))
        && let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(
            &format!("{name}:0"),
        ) {
            for a in addrs {
                add(&a.ip().to_string(), 2);
            }
        }

    cands.sort();
    let mut ips: Vec<String> = Vec::new();
    for (_, _, ip) in cands {
        if !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    ok_json(json!({
        "ips": ips,
        "port": PORT,
        "listen_host": crate::config::listen_host(),
    }))
}

pub async fn update_status() -> Response {
    ok_json(crate::updater::snapshot())
}

pub async fn start_update_download() -> Response {
    std::thread::spawn(crate::updater::download_update);
    ok_json(json!({ "ok": true }))
}

pub async fn check_update() -> Response {
    crate::updater::reset_check_state();
    std::thread::spawn(|| crate::updater::check_for_updates(true));
    ok_json(json!({ "ok": true }))
}

pub async fn shutdown() -> Response {
    // legacy no-op 'ok' route
    "ok".into_response()
}

pub async fn apply_update() -> Response {
    apply_update_inner().await
}

fn status_str(key: &str) -> String {
    crate::updater::snapshot()
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn apply_update_inner() -> std::future::Ready<Response> {
    std::future::ready(apply_update_sync())
}

fn apply_update_sync() -> Response {
    let src = status_str("download_path");
    if src.is_empty() || !std::path::Path::new(&src).exists() {
        return err_status(StatusCode::BAD_REQUEST, "No update file");
    }

    #[cfg(target_os = "macos")]
    {
        let kind = status_str("download_kind");
        if kind != "dmg" {
            return err_status(StatusCode::BAD_REQUEST, "Ожидался DMG-файл обновления");
        }
        return apply_update_macos(&src);
    }
    #[cfg(target_os = "linux")]
    {
        let kind = status_str("download_kind");
        if kind != "appimage" {
            return err_status(StatusCode::BAD_REQUEST, "Ожидался AppImage-файл обновления");
        }
        return apply_update_linux(&src);
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        apply_update_windows(&src)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn apply_update_windows(src: &str) -> Response {
    use std::process::Command;
    let kind = status_str("download_kind");

    let exe = std::env::current_exe().unwrap_or_default();
    let (dst, is_frozen) = if exe.file_name().map(|n| n == "tokenatra.exe" || n == "Tokenatra.exe").unwrap_or(false) {
        (exe.to_string_lossy().to_string(), true)
    } else {
        (paths::base_dir().join("Tokenatra.exe").to_string_lossy().to_string(), false)
    };
    let _ = is_frozen;
    let exe_name = std::path::Path::new(&dst)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dst_dir = std::path::Path::new(&dst)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let upd_dir = std::env::var("LOCALAPPDATA")
        .map(|l| std::path::PathBuf::from(l).join("Tokenatra").join("update"))
        .unwrap_or_else(|_| std::env::temp_dir().join("Tokenatra").join("update"));
    if std::fs::create_dir_all(&upd_dir).is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось создать каталог обновления");
    }
    let bat = upd_dir.join("_update.bat");

    let lines: Vec<String> = if kind == "installer" {
        vec![
            "@echo off".into(),
            "setlocal".into(),
            "set /a tries=0".into(),
            ":waitloop".into(),
            "tasklist /FI \"IMAGENAME eq %EXE%\" 2>nul | find /I \"%EXE%\" >nul".into(),
            "if errorlevel 1 goto settle".into(),
            "set /a tries+=1".into(),
            "if %tries% geq 30 (".into(),
            "    taskkill /F /IM \"%EXE%\" >nul 2>&1".into(),
            "    goto settle".into(),
            ")".into(),
            "ping 127.0.0.1 -n 2 > nul".into(),
            "goto waitloop".into(),
            ":settle".into(),
            "ping 127.0.0.1 -n 3 > nul".into(),
            "start \"\" /wait \"%SRC%\" /SILENT /SP- /SUPPRESSMSGBOXES /NORESTART".into(),
            "if exist \"%SRC%\" del \"%SRC%\" >nul 2>&1".into(),
            "if exist \"%DST%\" start \"\" /D \"%DSTDIR%\" \"%DST%\" >nul 2>&1".into(),
            "del \"%~f0\" >nul 2>&1".into(),
            "exit /b 0".into(),
        ]
    } else {
        vec![
            "@echo off".into(),
            "setlocal".into(),
            "set /a tries=0".into(),
            ":waitloop".into(),
            "tasklist /FI \"IMAGENAME eq %EXE%\" 2>nul | find /I \"%EXE%\" >nul".into(),
            "if errorlevel 1 goto copy".into(),
            "set /a tries+=1".into(),
            "if %tries% geq 30 (".into(),
            "    taskkill /F /IM \"%EXE%\" >nul 2>&1".into(),
            "    goto copy".into(),
            ")".into(),
            "ping 127.0.0.1 -n 2 > nul".into(),
            "goto waitloop".into(),
            ":copy".into(),
            "set /a ctries=0".into(),
            ":copyloop".into(),
            "copy /Y \"%SRC%\" \"%DST%.new\" >nul 2>&1".into(),
            "if exist \"%DST%.new\" goto moveit".into(),
            "set /a ctries+=1".into(),
            "if %ctries% geq 10 goto fail".into(),
            "ping 127.0.0.1 -n 2 > nul".into(),
            "goto copyloop".into(),
            ":moveit".into(),
            "set /a mtries=0".into(),
            ":moveloop".into(),
            "move /Y \"%DST%.new\" \"%DST%\" >nul 2>&1".into(),
            "if not exist \"%DST%.new\" goto done".into(),
            "set /a mtries+=1".into(),
            "if %mtries% geq 10 goto fail".into(),
            "ping 127.0.0.1 -n 2 > nul".into(),
            "goto moveloop".into(),
            ":done".into(),
            "if exist \"%SRC%\" del \"%SRC%\" >nul 2>&1".into(),
            "start \"\" /D \"%DSTDIR%\" \"%DST%\" >nul 2>&1".into(),
            "del \"%~f0\" >nul 2>&1".into(),
            "exit /b 0".into(),
            ":fail".into(),
            "if exist \"%DST%.new\" del \"%DST%.new\" >nul 2>&1".into(),
            "if exist \"%SRC%\" del \"%SRC%\" >nul 2>&1".into(),
            "del \"%~f0\" >nul 2>&1".into(),
            "exit /b 1".into(),
        ]
    };

    let content = format!("{}\r\n", lines.join("\r\n"));
    if std::fs::write(&bat, content).is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось записать bat-файл");
    }

    let mut cmd = Command::new("cmd");
    cmd.args(["/c", &bat.to_string_lossy()]);
    cmd.env("SRC", src)
        .env("DST", &dst)
        .env("DSTDIR", &dst_dir)
        .env("EXE", &exe_name);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    if cmd.spawn().is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось запустить обновление");
    }
    ok_json(json!({ "ok": true }))
}

#[cfg(target_os = "macos")]
fn apply_update_macos(src: &str) -> Response {
    use std::process::Command;
    // the .app bundle the app is running from
    let exe = std::env::current_exe().unwrap_or_default();
    let Some(exe_dir) = exe.parent() else {
        return err_status(StatusCode::BAD_REQUEST, "Обновление доступно только для установленного приложения (.app)");
    };
    if exe_dir.file_name().map(|n| n != "MacOS").unwrap_or(true) {
        return err_status(StatusCode::BAD_REQUEST, "Обновление доступно только для установленного приложения (.app)");
    }
    let bundle = exe_dir.parent().and_then(|p| p.parent()).unwrap_or(exe_dir);
    let target = bundle.to_string_lossy().to_string();
    if !target.ends_with(".app") || !bundle.is_dir() {
        return err_status(StatusCode::BAD_REQUEST, "Обновление доступно только для установленного приложения (.app)");
    }
    if target.starts_with("/Volumes/") {
        return err_status(
            StatusCode::BAD_REQUEST,
            "Приложение запущено из образа. Скопируйте Tokenatra в «Программы» и запустите оттуда",
        );
    }

    let upd_dir = crate::updater::update_dir();
    let script = upd_dir.join("_update.sh");
    let body = r#"#!/bin/sh
tries=0
while kill -0 "$PID" 2>/dev/null; do
  tries=$((tries+1))
  [ "$tries" -ge 120 ] && break
  sleep 0.5
done
sleep 1
MNT="$(mktemp -d)"
if ! hdiutil attach -nobrowse -quiet -mountpoint "$MNT" "$SRC"; then
  rmdir "$MNT" 2>/dev/null
  rm -f "$0"
  exit 1
fi
APP_SRC="$(find "$MNT" -maxdepth 2 -name "*.app" -type d 2>/dev/null | head -n 1)"
if [ -z "$APP_SRC" ]; then
  hdiutil detach "$MNT" -quiet 2>/dev/null
  rmdir "$MNT" 2>/dev/null
  rm -f "$0"
  exit 1
fi
mv "$TARGET" "$TARGET.old"
if cp -R "$APP_SRC" "$TARGET"; then
  rm -rf "$TARGET.old"
  hdiutil detach "$MNT" -quiet 2>/dev/null
  rm -f "$SRC"
  open "$TARGET"
else
  mv "$TARGET.old" "$TARGET"
  hdiutil detach "$MNT" -quiet 2>/dev/null
fi
rmdir "$MNT" 2>/dev/null
rm -f "$0"
"#;
    if std::fs::write(&script, body).is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось записать скрипт обновления");
    }
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script)
        .map(|m| m.permissions())
        .unwrap_or_else(|_| std::fs::Permissions::from_mode(0o644));
    perms.set_mode(0o755);
    let _ = std::fs::set_permissions(&script, perms);

    let mut cmd = Command::new("/bin/sh");
    cmd.arg(&script)
        .env("SRC", src)
        .env("TARGET", &target)
        .env("PID", std::process::id().to_string());
    let _ = cmd.spawn();
    ok_json(json!({ "ok": true }))
}

#[cfg(target_os = "linux")]
fn apply_update_linux(src: &str) -> Response {
    use std::process::Command;
    let Some(appimage) = std::env::var("APPIMAGE").ok().filter(|p| std::path::Path::new(p).is_file()) else {
        return err_status(
            StatusCode::BAD_REQUEST,
            "Обновление доступно только для AppImage-версии. Скачайте новый файл вручную.",
        );
    };
    let appimage = std::path::PathBuf::from(appimage);
    let tmp = appimage.with_extension(format!("{}.new", appimage.extension().and_then(|e| e.to_str()).unwrap_or("AppImage")));
    let tmp = appimage.parent().map(|p| p.join(format!("{}.new", appimage.file_name().and_then(|n| n.to_str()).unwrap_or_default()))).unwrap_or(tmp);
    if std::fs::copy(src, &tmp).is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось скопировать обновление");
    }
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(&tmp) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(&tmp, perms);
    }
    if std::fs::rename(&tmp, &appimage).is_err() {
        return err_status(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось заменить AppImage");
    }
    let _ = std::fs::remove_file(src);
    let _ = Command::new(appimage.as_os_str()).spawn();
    ok_json(json!({ "ok": true }))
}
