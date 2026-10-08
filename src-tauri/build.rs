#[path = "../scripts/rust/tauri-build.rs"]
mod tauri_build_support;

fn main() {
    tauri_build_support::build("windows-app-manifest.xml");
}
