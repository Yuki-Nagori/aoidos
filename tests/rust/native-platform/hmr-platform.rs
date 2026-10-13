//! Verifies Vite CSS HMR inside a real Tauri WebView under the development CSP.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{WebviewUrl, WebviewWindowBuilder, webview::PageLoadEvent};

// Windows CI 曾出现 localhost 探测成功但 WebView2 未加载的超时；显式回环地址让两者命中同一接口。
const HMR_HOST: &str = "127.0.0.1";
const HMR_PORT: u16 = 1420;
const WEBVIEW_CREATE_TIMEOUT: Duration = Duration::from_secs(60);
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(60);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(20);

struct ViteServer(Arc<Mutex<Child>>);

fn stop_server(child: &Mutex<Child>) {
    let mut child = child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = child.kill();
    let _ = child.wait();
}

impl Drop for ViteServer {
    fn drop(&mut self) {
        stop_server(&self.0);
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
    let server = ViteServer(Arc::new(Mutex::new(start_vite(&fixture, &vite)?)));
    wait_for_server()?;
    let _ = std::fs::remove_file(&report);
    let server_process = server.0.clone();

    let passed = Arc::new(AtomicBool::new(false));
    let app_passed = passed.clone();
    let page_started = Arc::new(AtomicBool::new(false));
    let app_page_started = page_started.clone();
    let page_finished = Arc::new(AtomicBool::new(false));
    let app_page_finished = page_finished.clone();
    let context = tauri::generate_context!("tauri.conf.json", test = true);
    let app = tauri::Builder::default()
        .setup(move |app| {
            let handle = app.handle().clone();
            let passed = app_passed.clone();
            let page_started = app_page_started.clone();
            let page_finished = app_page_finished.clone();
            let page_started_for_callback = page_started.clone();
            let page_finished_for_callback = page_finished.clone();
            let css = css.clone();
            let report = report.clone();
            let server_for_watchdog = server_process.clone();
            let webview_created = Arc::new(AtomicBool::new(false));
            let webview_created_for_watchdog = webview_created.clone();
            thread::spawn(move || {
                let deadline = Instant::now() + WEBVIEW_CREATE_TIMEOUT;
                while Instant::now() < deadline
                    && !webview_created_for_watchdog.load(Ordering::Acquire)
                {
                    thread::sleep(Duration::from_millis(50));
                }
                if !webview_created_for_watchdog.load(Ordering::Acquire) {
                    eprintln!(
                        "native HMR fixture WebView creation timed out after {:.0}s",
                        WEBVIEW_CREATE_TIMEOUT.as_secs_f64()
                    );
                    // 创建卡在原生消息泵时事件循环可能无法处理 exit 请求；先清理 Vite，再结束测试进程。
                    stop_server(&server_for_watchdog);
                    std::process::exit(1);
                }
            });
            let build_started = Instant::now();
            eprintln!("HMR fixture WebView creation started");
            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(format!("http://{HMR_HOST}:{HMR_PORT}").parse()?),
            )
            .title("Aoidos HMR fixture")
            .visible(false)
            .on_page_load(move |_, payload| match payload.event() {
                PageLoadEvent::Started => {
                    page_started_for_callback.store(true, Ordering::Release);
                    eprintln!("HMR fixture navigation started: {}", payload.url());
                }
                PageLoadEvent::Finished => {
                    page_finished_for_callback.store(true, Ordering::Release);
                    eprintln!("HMR fixture navigation finished: {}", payload.url());
                }
            })
            .build()?;
            webview_created.store(true, Ordering::Release);
            eprintln!(
                "HMR fixture WebView creation finished in {:.2}s",
                build_started.elapsed().as_secs_f64()
            );
            thread::spawn(move || {
                let navigation_deadline = Instant::now() + NAVIGATION_TIMEOUT;
                while Instant::now() < navigation_deadline {
                    let first = read_report(&report);
                    if let Some(initial) = first {
                        if initial.phase == "runtime-error" {
                            eprintln!("HMR fixture JavaScript failed: {}", initial.error);
                            handle.exit(1);
                            return;
                        }
                        if initial.phase == "ready" {
                            if initial.marker.as_deref() != Some("before") {
                                eprintln!("unexpected HMR initial value: {:?}", initial.marker);
                                handle.exit(1);
                                return;
                            }
                            if let Err(error) =
                                std::fs::write(&css, ":root { --hmr-marker: after; }\n")
                            {
                                eprintln!("failed to update HMR fixture: {error}");
                                handle.exit(1);
                                return;
                            }
                            let update_deadline = Instant::now() + UPDATE_TIMEOUT;
                            while Instant::now() < update_deadline {
                                if let Some(updated) = read_report(&report) {
                                    if updated.phase == "runtime-error" {
                                        eprintln!(
                                            "HMR fixture JavaScript failed during update: {}",
                                            updated.error
                                        );
                                        handle.exit(1);
                                        return;
                                    }
                                    if updated.document_id == initial.document_id
                                        && updated.phase == "after-update"
                                        && updated.marker.as_deref() == Some("after")
                                    {
                                        passed.store(true, Ordering::Release);
                                        println!(
                                            "development CSP and Vite CSS HMR in native WebView: PASS"
                                        );
                                        handle.exit(0);
                                        return;
                                    }
                                }
                                thread::sleep(Duration::from_millis(50));
                            }
                            eprintln!(
                                "Vite CSS update did not apply without replacing the document"
                            );
                            handle.exit(1);
                            return;
                        }
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                let stage = if !page_started.load(Ordering::Acquire) {
                    "WebView did not start navigation"
                } else if !page_finished.load(Ordering::Acquire) {
                    "WebView navigation started but did not finish"
                } else {
                    "WebView loaded the page but its startup module did not report"
                };
                eprintln!(
                    "native HMR fixture navigation timed out after {:.0}s: {stage}",
                    NAVIGATION_TIMEOUT.as_secs_f64()
                );
                handle.exit(1);
            });
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

struct HmrReport {
    document_id: String,
    phase: String,
    marker: Option<String>,
    error: String,
}

fn read_report(path: &std::path::Path) -> Option<HmrReport> {
    let content = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&content).ok()?;
    Some(HmrReport {
        document_id: value.get("documentId")?.as_str()?.to_owned(),
        phase: value.get("phase")?.as_str()?.to_owned(),
        marker: value
            .get("marker")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        error: value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

fn start_vite(
    fixture: &std::path::Path,
    vite: &std::path::Path,
) -> Result<Child, Box<dyn std::error::Error>> {
    let port = HMR_PORT.to_string();
    let server = Command::new("bun")
        .arg(vite)
        .args([
            "--config",
            "vite.config.ts",
            "--host",
            HMR_HOST,
            "--port",
            &port,
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
    let address = SocketAddr::from(([127, 0, 0, 1], HMR_PORT));
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.write_all(
                format!(
                    "GET / HTTP/1.1\r\nHost: {HMR_HOST}:{HMR_PORT}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )?;
            let mut response = String::new();
            stream.read_to_string(&mut response)?;
            if (response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.1 304"))
                && response.contains("Aoidos HMR fixture")
            {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("Vite HMR server did not become ready on loopback".into())
}
