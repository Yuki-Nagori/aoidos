//! 共用 Tauri 构建适配；Windows 测试入口也必须携带 Common Controls v6 manifest。

/// 从发布翻译表提取原生子集，并在编译期检查两种语言的键和插值参数。
pub fn embed_native_locales() {
    use std::{fs, path::PathBuf};

    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies the crate directory"),
    );
    let root = manifest.join("../locales");
    let chinese_path = root.join("zh-Hans.json");
    let english_path = root.join("en.json");
    println!("cargo:rerun-if-changed={}", chinese_path.display());
    println!("cargo:rerun-if-changed={}", english_path.display());
    let chinese: serde_json::Value =
        serde_json::from_slice(&fs::read(&chinese_path).expect("read zh-Hans locale resource"))
            .expect("parse zh-Hans locale resource");
    let english: serde_json::Value =
        serde_json::from_slice(&fs::read(&english_path).expect("read en locale resource"))
            .expect("parse en locale resource");
    let chinese_native = chinese.get("native").expect("zh-Hans native messages");
    let english_native = english.get("native").expect("en native messages");
    assert_eq!(native_shape(chinese_native), native_shape(english_native));
    let resources = serde_json::json!({ "zh-Hans": chinese_native, "en": english_native });
    let json = serde_json::to_string(&resources).expect("serialize native locale resource");
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"))
        .join("native_locales.rs");
    fs::write(
        output,
        format!("pub const NATIVE_LOCALES_JSON: &str = {json:?};\n"),
    )
    .expect("write embedded native locale resource");
}

fn native_shape(value: &serde_json::Value) -> Vec<(String, Vec<String>)> {
    fn collect(prefix: &str, value: &serde_json::Value, output: &mut Vec<(String, Vec<String>)>) {
        match value {
            serde_json::Value::Object(entries) => {
                for (key, child) in entries {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    collect(&path, child, output);
                }
            }
            serde_json::Value::String(message) => {
                let mut names = message
                    .split('{')
                    .skip(1)
                    .filter_map(|part| part.split_once('}').map(|(name, _)| name.to_owned()))
                    .collect::<Vec<_>>();
                names.sort();
                output.push((prefix.to_owned(), names));
            }
            _ => panic!("native message {prefix} must be a string or object"),
        }
    }

    let mut shape = Vec::new();
    collect("", value, &mut shape);
    shape.sort();
    shape
}

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
