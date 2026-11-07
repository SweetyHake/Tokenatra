fn main() {
    // ort-static требует DXCore.lib; она появляется в Windows SDK только с
    // 10.0.19041. Для старых SDK генерируем import-lib сами (vendor/):
    // единственный именованный экспорт dxcore.dll — DXCoreCreateAdapterFactory,
    // остальное — forwards/[NONAME], которые линковщик по имени не требует.
    if cfg!(windows) {
        println!(
            "cargo:rustc-link-search=native={}",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("vendor")
                .display()
        );
    }

    // Embed icon.ico into the exe (Explorer, pinned shortcuts, uninstall
    // entry, registry verbs). Keyed on the TARGET, not the host, so
    // cross-compiles stay correct.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("../icon.ico");
        let mut res = winresource::WindowsResource::new();
        if let Some(icon) = icon.to_str() {
            res.set_icon(icon);
            res.compile().expect("embed icon.ico into the exe");
        }
    }

    // macOS: ort-sys static archive links libonnxruntime_providers_coreml.a,
    // whose Objective-C++ objects (@available) reference ___isPlatformVersionAtLeast
    // from Clang's compiler-rt (libclang_rt.osx.a). Because rustc invokes the
    // linker driver with -nodefaultlibs, add Clang's darwin runtime directory explicitly.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let mut found_dir = None;
        if let Ok(out) = std::process::Command::new("clang")
            .arg("--print-runtime-dir")
            .output()
            && out.status.success()
        {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !dir.is_empty() && std::path::Path::new(&dir).exists() {
                found_dir = Some(std::path::PathBuf::from(dir));
            }
        }
        if found_dir.is_none()
            && let Ok(out) = std::process::Command::new("clang")
                .arg("--print-resource-dir")
                .output()
            && out.status.success()
        {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let darwin_dir = std::path::Path::new(&dir).join("lib").join("darwin");
            if darwin_dir.exists() {
                found_dir = Some(darwin_dir);
            }
        }
        if let Some(dir) = found_dir {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
    }
}
