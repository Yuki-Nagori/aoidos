#[path = "../../../scripts/rust/tauri-build.rs"]
// 此 build script 共用 Windows manifest 适配，但仅产品 crate 生成原生语言表。
#[allow(dead_code)]
mod tauri_build_support;

fn main() {
    tauri_build_support::build("../../../src-tauri/windows-app-manifest.xml");
}
