//! Verifies Vite CSS HMR inside a real Tauri WebView under the development CSP.

use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{WebviewUrl, WebviewWindowBuilder};

struct ViteServer(Child);

impl Drop for ViteServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let fixture = root.join("tests/rust/native-platform/gen/hmr");
    let fixture_source = root.join("tests/rust/native-platform/webview/hmr/style.css");
    let css = fixture.join("style.css");
    let report = fixture.join("hmr-report.json");
    let vite = root.join("node_modules/vite/bin/vite.js");
    std::fs::copy(fixture_source, &css)?;
    let _server = ViteServer(start_vite(&fixture, &vite)?);
    wait_for_server()?;
    let _ = std::fs::remove_file(&report);

    let passed = Arc::new(AtomicBool::new(false));
    let app_passed = passed.clone();
    let context = tauri::generate_context!("tauri.conf.json", test = true);
    let app = tauri::Builder::default()
        .setup(move |app| {
            let handle = app.handle().clone();
            let passed = app_passed.clone();
            let css = css.clone();
            let report = report.clone();
            thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(20);
                while Instant::now() < deadline {
                    let first = read_report(&report);
                    if let Some((document_id, marker)) = first {
                        if marker != "before" {
                            eprintln!("unexpected HMR initial value: {marker}");
                            handle.exit(1);
                            return;
                        }
                        if let Err(error) = std::fs::write(&css, ":root { --hmr-marker: after; }\n")
                        {
                            eprintln!("failed to update HMR fixture: {error}");
                            handle.exit(1);
                            return;
                        }
                        while Instant::now() < deadline {
                            let updated = read_report(&report).is_some_and(
                                |(updated_document, updated_marker)| {
                                    updated_document == document_id && updated_marker == "after"
                                },
                            );
                            if updated {
                                passed.store(true, Ordering::Release);
                                println!(
                                    "development CSP and Vite CSS HMR in native WebView: PASS"
                                );
                                handle.exit(0);
                                return;
                            }
                            thread::sleep(Duration::from_millis(50));
                        }
                        eprintln!("Vite CSS update did not apply without replacing the document");
                        handle.exit(1);
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                eprintln!("native HMR fixture did not report its initial style");
                handle.exit(1);
            });
            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External("http://localhost:1420".parse()?),
            )
            .title("Aoidos HMR fixture")
            .visible(false)
            .build()?;
            Ok(())
        })
        .build(context)?;
    app.run(|_, _| {});
    assert!(
        passed.load(Ordering::Acquire),
        "native Vite HMR did not pass"
    );
    Ok(())
}

fn read_report(path: &std::path::Path) -> Option<(String, String)> {
    let content = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&content).ok()?;
    Some((
        value.get("documentId")?.as_str()?.to_owned(),
        value.get("marker")?.as_str()?.to_owned(),
    ))
}

fn start_vite(
    fixture: &std::path::Path,
    vite: &std::path::Path,
) -> Result<Child, Box<dyn std::error::Error>> {
    let server = Command::new("bun")
        .arg(vite)
        .args([
            "--config",
            "vite.config.ts",
            "--host",
            "localhost",
            "--port",
            "1420",
            "--strictPort",
        ])
        .current_dir(fixture)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    Ok(server)
}

fn wait_for_server() -> Result<(), Box<dyn std::error::Error>> {
    let addresses = ("localhost", 1420).to_socket_addrs()?.collect::<Vec<_>>();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        for address in &addresses {
            if let Ok(mut stream) = TcpStream::connect_timeout(address, Duration::from_millis(100))
            {
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                stream.write_all(
                    b"GET / HTTP/1.1\r\nHost: localhost:1420\r\nConnection: close\r\n\r\n",
                )?;
                let mut response = String::new();
                stream.read_to_string(&mut response)?;
                if (response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.1 304"))
                    && response.contains("Aoidos HMR fixture")
                {
                    return Ok(());
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("Vite HMR server did not become ready".into())
}
