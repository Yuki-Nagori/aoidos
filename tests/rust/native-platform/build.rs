#[path = "../../../scripts/rust/tauri-build.rs"]
mod tauri_build_support;

fn main() {
    tauri_build_support::build("../../../src-tauri/windows-app-manifest.xml");
}
