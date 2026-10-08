//! 共用 Tauri 构建适配；Windows 测试入口也必须携带 Common Controls v6 manifest。

/// 入口传入相对于本 crate 的 manifest 路径，其余平台使用 Tauri 默认构建。
pub fn build(manifest_path: &str) {
    #[cfg(windows)]
    {
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
    }
    #[cfg(not(windows))]
    {
        let _ = manifest_path;
        tauri_build::build();
    }
}
