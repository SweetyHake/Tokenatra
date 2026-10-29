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
}
