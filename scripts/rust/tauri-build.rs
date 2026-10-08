//! 共用 Tauri 构建适配；Windows 测试入口也必须携带 Common Controls v6 manifest。

/// 入口传入相对于本 crate 的 manifest 路径，其余平台使用 Tauri 默认构建。
/// # Panics
/// Cargo 输入缺失、Windows 目标非 MSVC 或 Tauri 配置无效时终止构建。
pub fn build(manifest_path: &str) {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo supplies the target OS");
    if target_os == "windows" {
        assert_eq!(
            std::env::var("CARGO_CFG_TARGET_ENV").as_deref(),
            Ok("msvc"),
            "Tauri Windows builds require the MSVC target"
        );
        // tauri-build 默认仅给 bins 链接资源，lib 单测 / 原生集成测试会缺失 v6
        // 导出并在启动时报 0xc0000139。上游修复全部测试目标后可撤除此适配：
        // https://github.com/tauri-apps/tauri/issues/13419
        let manifest = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies the crate directory"),
        )
        .join(manifest_path);
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        let attributes = tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        tauri_build::try_build(attributes).expect("Tauri build configuration is valid");
    } else {
        tauri_build::build();
    }
}
