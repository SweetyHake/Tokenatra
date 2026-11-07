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
}
