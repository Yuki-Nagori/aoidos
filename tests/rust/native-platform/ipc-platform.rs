//! 真实主 Webview 的命令 / 事件往返；本地夹具不发送收费 HTTP，独立进程主线程运行。

use mythos_engine::fault::Fault;
use mythos_lib::turn_commands::{TurnService, WindowEvents};
use mythos_llm::{
    config::{LlmProfile, ProfileMode, ProfileStore, ProxyConfig},
    credentials::{CredentialFile, CredentialStore},
    proxy::SystemProxySnapshot,
    sampling::Sampling,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Emitter, Manager};

struct Report {
    passed: Arc<AtomicBool>,
    dir: std::path::PathBuf,
}

#[tauri::command]
async fn report_smoke(
    app: AppHandle,
    report: tauri::State<'_, Report>,
    service: tauri::State<'_, TurnService>,
    passed: bool,
    diagnostic: String,
) -> Result<(), String> {
    report.passed.store(passed, Ordering::Release);
    if !passed {
        eprintln!("IPC fixture failed: {diagnostic}");
    }
    service.coordinator.shutdown().await;
    app.state::<Arc<WindowEvents>>().close();
    let cleaned = std::fs::remove_dir_all(&report.dir).is_ok();
    if passed && cleaned {
        println!("main Webview submit / chunk / done / snapshot / cancel: PASS");
    }
    app.exit(if passed && cleaned { 0 } else { 1 });
    Ok(())
}

fn delivery_failed(_: tauri::Error) -> Fault {
    Fault::new("app.event-failed", "主窗口事件无法投递")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("mythos-ipc-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    ProfileStore::new(&dir).save(&[LlmProfile {
        profile_id: "ipc-fixture".into(),
        provider_id: "deepseek".into(),
        model: "deepseek-v4-pro".into(),
        mode: ProfileMode::Completion,
        thinking: false,
        sampling: Sampling {
            temperature: 1.0,
            max_tokens: 64,
        },
        proxy: ProxyConfig::None,
    }])?;
    CredentialFile::new(&dir).set_key(
        "deepseek",
        secrecy::SecretString::from("mythos-local-fixture".to_owned()),
    )?;
    mythos_lib::llm_commands::init_llm_dir(dir.clone());
    let passed = Arc::new(AtomicBool::new(false));
    let observed = passed.clone();
    let fixture_dir = dir.clone();
    let app = tauri::Builder::default()
        .setup(move |app| {
            app.manage(Report {
                passed: observed,
                dir: fixture_dir.clone(),
            });
            let handle = app.handle().clone();
            let events = Arc::new(WindowEvents::new(move |event, envelope| {
                handle
                    .emit_to("main", event, envelope)
                    .map_err(delivery_failed)
            }));
            app.manage(events.clone());
            app.manage(TurnService::new(events, SystemProxySnapshot::default())?);
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Mythos IPC fixture")
            .visible(false)
            .build()?;
            // 超时只用于集成测试兜底，不是产品正文轮询或恢复策略。
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(45));
                handle.exit(1);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            mythos_lib::turn_ipc::llm_submit,
            mythos_lib::turn_ipc::llm_get_turn,
            mythos_lib::turn_ipc::llm_cancel,
            report_smoke
        ])
        .build(tauri::generate_context!("tauri.conf.json", test = true))?;
    app.run(|_, _| {});
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    assert!(
        passed.load(Ordering::Acquire),
        "real main Webview IPC did not pass"
    );
    println!("main Webview submit / chunk / done / snapshot / cancel: PASS");
    Ok(())
}
