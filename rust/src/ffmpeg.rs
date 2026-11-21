use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Locates FFmpeg next to the app, in tools/ or on PATH.
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

/// Converts the first audio stream to OGG Vorbis.
pub fn convert_media_to_ogg(data: &[u8], src_ext: &str, quality: u8) -> Result<Vec<u8>, String> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        "FFmpeg не найден. Установите FFmpeg или добавьте его в PATH".to_string()
    })?;

    let tmp = std::env::temp_dir();
    let uniq = crate::paths::unique_tmp_tag();
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

/// Maps UI quality 10..100 to Vorbis -1..10.
pub fn vorbis_quality(quality: u8) -> i32 {
    (((quality as f32 - 10.0) * 10.0 / 90.0) as f64).round() as i32
}

/// Maps UI quality 10..100 to VP9 CRF 45..15.
pub fn vp9_crf(quality: u8) -> u32 {
    let q = quality.clamp(10, 100) as f32;
    (45.0 - (q - 10.0) * 30.0 / 90.0).round().clamp(15.0, 45.0) as u32
}

/// Converts video or animated image (GIF/WebP/APNG/MP4/MOV/MKV/AVI/WebM) to WebM (VP9).
/// Preserves alpha channel (yuva420p) for formats that may carry transparency.
pub fn convert_media_to_webm(data: &[u8], src_ext: &str, quality: u8) -> Result<Vec<u8>, String> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        "FFmpeg не найден. Установите FFmpeg или добавьте его в PATH".to_string()
    })?;

    let tmp = std::env::temp_dir();
    let uniq = crate::paths::unique_tmp_tag();
    let ext_clean = if src_ext.starts_with('.') {
        src_ext.to_lowercase()
    } else {
        format!(".{}", src_ext.to_lowercase())
    };
    let input_path = tmp.join(format!("tokenatra_{uniq}_in{ext_clean}"));
    let output_path = tmp.join(format!("tokenatra_{uniq}_out.webm"));
    let _ = std::fs::remove_file(&input_path);
    let _ = std::fs::remove_file(&output_path);
    std::fs::write(&input_path, data).map_err(|e| e.to_string())?;

    let may_have_alpha = matches!(
        ext_clean.as_str(),
        ".gif" | ".webm" | ".mov" | ".webp" | ".png" | ".apng"
    );
    let pix_fmt = if may_have_alpha { "yuva420p" } else { "yuv420p" };
    let crf = vp9_crf(quality).to_string();
    let no_audio = matches!(ext_clean.as_str(), ".gif" | ".webp" | ".png" | ".apng");

    let result = (|| -> Result<Vec<u8>, String> {
        let mut args: Vec<String> = vec![
            "-y".into(),
            "-v".into(),
            "error".into(),
        ];
        if ext_clean == ".webm" {
            // libvpx-vp9 decoder preserves alpha when decoding transparent webm
            args.extend(["-c:v".into(), "libvpx-vp9".into()]);
        }
        args.extend([
            "-i".into(),
            input_path.to_string_lossy().into_owned(),
            "-c:v".into(),
            "libvpx-vp9".into(),
            "-b:v".into(),
            "0".into(),
            "-crf".into(),
            crf.clone(),
            "-pix_fmt".into(),
            pix_fmt.into(),
            "-auto-alt-ref".into(),
            "0".into(),
            "-deadline".into(),
            "good".into(),
            "-cpu-used".into(),
            "4".into(),
            "-row-mt".into(),
            "1".into(),
        ]);
        if no_audio {
            args.push("-an".into());
        } else {
            args.extend(["-c:a".into(), "libopus".into(), "-b:a".into(), "128k".into()]);
        }
        args.push(output_path.to_string_lossy().into_owned());

        let mut status = no_window(Command::new(&ffmpeg).args(&args))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| e.to_string())?;

        // Retry without forced libvpx-vp9 input decoder (e.g. VP8/AV1 webm)
        if !status.status.success() && ext_clean == ".webm" {
            let retry_args: Vec<String> = vec![
                "-y".into(),
                "-v".into(),
                "error".into(),
                "-i".into(),
                input_path.to_string_lossy().into_owned(),
                "-c:v".into(),
                "libvpx-vp9".into(),
                "-b:v".into(),
                "0".into(),
                "-crf".into(),
                crf,
                "-pix_fmt".into(),
                pix_fmt.into(),
                "-auto-alt-ref".into(),
                "0".into(),
                "-deadline".into(),
                "good".into(),
                "-cpu-used".into(),
                "4".into(),
                "-row-mt".into(),
                "1".into(),
                "-an".into(),
                output_path.to_string_lossy().into_owned(),
            ];
            status = no_window(Command::new(&ffmpeg).args(&retry_args))
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .map_err(|e| e.to_string())?;
        }

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

/// Decodes AVIF to PNG via system ffmpeg.
pub fn decode_to_png(data: &[u8]) -> Option<Vec<u8>> {
    let ffmpeg = find_ffmpeg()?;
    let tmp = std::env::temp_dir();
    let uniq = crate::paths::unique_tmp_tag();
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
    if cfg!(target_os = "macos") {
        let _ = Command::new("osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 1 of argv) with title \"Tokenatra\"",
                "-e",
                "end run",
                "--",
                message,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    } else if cfg!(target_os = "linux") {
        let _ = Command::new("notify-send")
            .args(["-a", "Tokenatra", "--", message])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    } else {
        let ps = "[System.Reflection.Assembly]::LoadWithPartialName(\"System.Windows.Forms\") | Out-Null; \
                  $n = New-Object System.Windows.Forms.NotifyIcon; \
                  $n.Icon = [System.Drawing.SystemIcons]::Information; \
                  $n.Visible = $true; \
                  $n.ShowBalloonTip(4000, \"Tokenatra\", $env:TOKENATRA_NOTIFY_MSG, [System.Windows.Forms.ToolTipIcon]::Info); \
                  Start-Sleep -Milliseconds 4500; \
                  $n.Dispose()";
        let _ = no_window(
            Command::new("powershell")
                .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", ps])
                .env("TOKENATRA_NOTIFY_MSG", message)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .spawn();
    }
}

/// Atomic file write (temp + rename).
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    crate::paths::atomic_write(path, data)
}
