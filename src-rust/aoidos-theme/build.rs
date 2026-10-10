use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let themes = manifest.join("assets/themes");
    println!("cargo:rerun-if-changed={}", themes.display());
    let mut files = fs::read_dir(&themes)
        .expect("theme assets directory")
        .map(|entry| entry.expect("theme asset entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "css"))
        .collect::<Vec<_>>();
    files.sort();
    assert!(!files.is_empty(), "at least one theme CSS file is required");
    let mut generated = String::from("pub const THEME_CSS_FILES: &[(&str, &str)] = &[\n");
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let id = path.file_stem().expect("theme file name").to_string_lossy();
        let source = fs::read_to_string(&path).expect("theme CSS must be UTF-8");
        generated.push_str(&format!("    ({id:?}, {source:?}),\n"));
    }
    generated.push_str("];\n");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("output directory")).join("theme_css_files.rs");
    fs::write(output, generated).expect("write generated theme registry");
}
