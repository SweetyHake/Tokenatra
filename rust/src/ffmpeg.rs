use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Ищет FFmpeg рядом с приложением, в tools/ или в PATH.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let resource = crate::paths::resource_dir();
    let base = crate::paths::base_dir();
    let mut candidates: Vec<PathBuf> = vec![
        resource.join("ffmpeg.exe"),
        resource.join("tools").join("ffmpeg.exe"),
        resource.join("ffmpeg"),
        resource.join("tools").join("ffmpeg"),
        base.join("ffmpeg.exe"),
        base.join("tools").join("ffmpeg.exe"),
        base.join("ffmpeg"),
        base.join("tools").join("ffmpeg"),
    ];
    if cfg!(windows) {
        candidates.push(PathBuf::from("ffmpeg.exe"));
    }
    for c in &candidates {
        if c.is_file() {
            return Some(c.clone());
        }
    }
    which::which("ffmpeg").ok()
}

fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Конвертирует первый аудиопоток в OGG Vorbis (порт convert_media_to_ogg).
pub fn convert_media_to_ogg(data: &[u8], src_ext: &str, quality: u8) -> Result<Vec<u8>, String> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        "FFmpeg не найден. Установите FFmpeg или добавьте его в PATH".to_string()
    })?;

    let tmp = std::env::temp_dir();
    let uniq = std::process::id();
    let input_path = tmp.join(format!("tokenatra_{uniq}_in{}", src_ext));
    let output_path = tmp.join(format!("tokenatra_{uniq}_out.ogg"));
    let _ = std::fs::remove_file(&input_path);
    let _ = std::fs::remove_file(&output_path);
    std::fs::write(&input_path, data).map_err(|e| e.to_string())?;

    let result = (|| -> Result<Vec<u8>, String> {
        let status = no_window(Command::new(&ffmpeg)
            .args([
                "-y", "-v", "error", "-i", input_path.to_string_lossy().as_ref(),
                "-map", "0:a:0", "-vn", "-c:a", "libvorbis", "-q:a",
                &vorbis_quality(quality).to_string(),
                output_path.to_string_lossy().as_ref(),
            ]))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| e.to_string())?;
        if !status.status.success() {
            let err = String::from_utf8_lossy(&status.stderr);
            let detail = err.lines().last().unwrap_or("неизвестная ошибка FFmpeg");
            return Err(format!("FFmpeg: {detail}"));
        }
        std::fs::read(&output_path).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&input_path);
    let _ = std::fs::remove_file(&output_path);
    result
}

/// Vorbis-качество из UI-качества 10..100 -> -1..10 (как в Python).
pub fn vorbis_quality(quality: u8) -> i32 {
    (((quality as f32 - 10.0) * 10.0 / 90.0) as f64).round() as i32
}

/// Декодирование AVIF в PNG через системный ffmpeg (когда недоступен Rust-декодер).
pub fn decode_to_png(data: &[u8]) -> Option<Vec<u8>> {
    let ffmpeg = find_ffmpeg()?;
    let tmp = std::env::temp_dir();
    let uniq = std::process::id();
    let in_path = tmp.join(format!("tokenatra_{uniq}.avif"));
    std::fs::write(&in_path, data).ok()?;
    let out = (|| -> Option<Vec<u8>> {
        let status = no_window(Command::new(&ffmpeg)
            .args([
                "-y", "-v", "error", "-i", in_path.to_string_lossy().as_ref(),
                "-c:v", "png", "-f", "image2pipe", "-",
            ]))
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if status.status.success() && !status.stdout.is_empty() {
            Some(status.stdout)
        } else {
            None
        }
    })();
    let _ = std::fs::remove_file(&in_path);
    out
}

pub fn notify(message: &str) {
    let msg = message.to_string();
    std::thread::spawn(move || {
        if cfg!(target_os = "macos") {
            let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
            let _ = Command::new("osascript")
                .args(["-e", &format!("display notification \"{escaped}\" with title \"Tokenatra\"")])
                .spawn();
        } else if cfg!(target_os = "linux") {
            let _ = Command::new("notify-send").args(["-a", "Tokenatra", &msg]).spawn();
        } else {
            let ps = format!(
                "[System.Reflection.Assembly]::LoadWithPartialName(\"System.Windows.Forms\") | Out-Null; \
                 $n = New-Object System.Windows.Forms.NotifyIcon; \
                 $n.Icon = [System.Drawing.SystemIcons]::Information; \
                 $n.Visible = $true; \
                 $n.ShowBalloonTip(4000, \"Tokenatra\", \"{msg}\", [System.Windows.Forms.ToolTipIcon]::Info); \
                 Start-Sleep -Milliseconds 4500; \
                 $n.Dispose()"
            );
            let _ = no_window(Command::new("powershell")
                .args(["-WindowStyle", "Hidden", "-Command", &ps]))
                .spawn();
        }
    });
}

/// Записать файл атомарно (temp + rename), как в context_menu_helper.
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        std::process::id()
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

pub const _UNUSED: Duration = Duration::from_secs(0);
