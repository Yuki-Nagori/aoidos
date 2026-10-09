//! 正式会话命令与真实 DeepSeek HTTP/SSE adapter；所有请求仅到回环服务。

use aoidos_engine::game::product::{Product, ProfileFactory};
use aoidos_lib::{
    game_commands::SavedProfiles,
    store_commands::{StorageEvents, StorageService},
    turn_commands::{TurnService, WindowEvents},
};
use aoidos_llm::{
    config::{LlmProfile, ProfileMode, ProfileStore, ProxyConfig},
    credentials::{CredentialFile, CredentialStore},
    proxy::SystemProxySnapshot,
    sampling::Sampling,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tauri::{AppHandle, Emitter, Manager};
#[path = "product-server.rs"]
mod product_server;

#[derive(Default)]
struct CommitGate {
    armed: AtomicBool,
    entered: AtomicBool,
    released: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
}
impl CommitGate {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
struct SlowRecords {
    inner: Arc<StorageEvents>,
    gate: Arc<CommitGate>,
}
impl aoidos_engine::record::session::RecordEvents for SlowRecords {
    fn prepare(
        &self,
        event: &aoidos_engine::record::session::Appended,
    ) -> Result<u64, aoidos_engine::fault::Fault> {
        if event.kind == "narration" && self.gate.armed.swap(false, Ordering::AcqRel) {
            self.gate.entered.store(true, Ordering::Release);
            let released = self.gate.released.lock().unwrap();
            let (released, _) = self
                .gate
                .changed
                .wait_timeout_while(released, std::time::Duration::from_secs(5), |value| !*value)
                .unwrap();
            if !*released {
                return Err(aoidos_engine::fault::Fault::new(
                    "store.io",
                    "合成提交门限超时",
                ));
            }
        }
        self.inner.prepare(event)
    }
    fn deliver(
        &self,
        seq: u64,
        event: aoidos_engine::record::session::Appended,
    ) -> Result<(), aoidos_engine::fault::Fault> {
        self.inner.deliver(seq, event)
    }
    fn retire(&self, id: &str) {
        self.inner.retire(id);
    }
}
#[tauri::command]
fn product_commit_gate(state: tauri::State<'_, Arc<CommitGate>>, action: String) -> bool {
    match action.as_str() {
        "arm" => {
            state.entered.store(false, Ordering::Release);
            *state.released.lock().unwrap() = false;
            state.armed.store(true, Ordering::Release);
        }
        "release" => state.release(),
        _ => {}
    }
    state.entered.load(Ordering::Acquire)
}
struct Fixture {
    restarting: bool,
    dir: PathBuf,
    count: Arc<AtomicUsize>,
    mode: Arc<AtomicUsize>,
    passed: Arc<AtomicBool>,
}
#[tauri::command]
fn product_restarting(state: tauri::State<'_, Fixture>) -> bool {
    state.restarting
}
#[tauri::command]
fn product_http(state: tauri::State<'_, Fixture>, mode: Option<usize>) -> usize {
    if let Some(mode) = mode {
        state.mode.store(mode, Ordering::SeqCst);
    }
    state.count.load(Ordering::SeqCst)
}
#[tauri::command]
async fn product_wait(
    app: AppHandle,
    session_id: String,
    round_id: String,
) -> Result<aoidos_engine::game::state::PhaseSnapshot, String> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let snapshot = app
            .state::<aoidos_engine::game::runtime::Service>()
            .get_phase(&session_id)
            .map_err(stable_error)?;
        if snapshot
            .state
            .last_operation
            .as_ref()
            .is_some_and(|operation| {
                operation.round_id.as_deref() == Some(&round_id)
                    && operation.outcome != aoidos_engine::game::state::OperationOutcome::Accepted
            })
        {
            return Ok(snapshot);
        }
        if tokio::time::Instant::now() > deadline {
            return Err("fixture.timeout".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
#[tauri::command]
async fn product_restart(app: AppHandle, session_id: String) -> Result<(), String> {
    app.state::<aoidos_engine::game::runtime::Service>()
        .close_session(&session_id)
        .await
        .map_err(stable_error)?;
    app.state::<StorageService>()
        .storage
        .records
        .close(&session_id)
        .map_err(stable_error)
}
#[tauri::command]
async fn product_disk(app: AppHandle, session_id: String) -> Result<serde_json::Value, String> {
    let root = app.state::<Fixture>().dir.clone();
    aoidos_engine::blocking::run(move || {
        let path = root
            .join("workspaces/mistbell/transcript")
            .join(format!("{session_id}.jsonl"));
        let text = std::fs::read_to_string(path)
            .map_err(aoidos_store::error::StoreError::from_io)
            .map_err(aoidos_engine::fault::Fault::from)?;
        let values = text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<Vec<serde_json::Value>, _>>()
            .map_err(invalid_record)?;
        Ok(serde_json::Value::Array(values))
    })
    .await
    .map_err(stable_error)
}
fn invalid_record(_: serde_json::Error) -> aoidos_engine::fault::Fault {
    aoidos_engine::fault::Fault::new("store.corrupt", "夹具记录不可解析")
}
fn stable_error(error: aoidos_engine::fault::Fault) -> String {
    error.code
}
#[tauri::command]
async fn product_report(app: AppHandle, passed: bool, diagnostic: String) {
    app.state::<Arc<CommitGate>>().release();
    app.state::<Fixture>()
        .passed
        .store(passed, Ordering::Release);
    if !passed {
        eprintln!("product integration failed: {diagnostic}");
    }
    app.state::<aoidos_engine::game::runtime::Service>()
        .shutdown()
        .await;
    app.state::<TurnService>().coordinator.shutdown().await;
    let closed = app.state::<StorageService>().storage.close().is_ok();
    app.state::<StorageService>().events.shutdown().await;
    app.state::<Arc<WindowEvents>>().close();
    // 父进程在两次真实子进程退出后统一清理；第二次使用同一磁盘事实和空内存 ring。
    if passed && closed {
        println!(
            "production profile / raw scenario / real HTTP SSE / manual check / cancel / failure / recovery / disk reopen: PASS"
        );
    }
    app.exit(if passed && closed { 0 } else { 1 });
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os().collect::<Vec<_>>();
    if args.get(1).is_some_and(|value| value == "--fixture-child") {
        return run(
            PathBuf::from(args.get(3).ok_or("fixture root missing")?),
            args.get(2).is_some_and(|value| value == "restart"),
        );
    }
    let dir = std::env::temp_dir().join(format!("aoidos-product-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let executable = std::env::current_exe()?;
    for phase in ["initial", "restart"] {
        let status = std::process::Command::new(&executable)
            .arg("--fixture-child")
            .arg(phase)
            .arg(&dir)
            .status()?;
        if !status.success() {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("product fixture {phase} failed").into());
        }
    }
    std::fs::remove_dir_all(dir)?;
    println!("product HTTP/SSE integration and fresh-process restart: PASS");
    Ok(())
}
fn run(dir: PathBuf, restarting: bool) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(&dir)?;
    let llm = dir.join("llm");
    let profile = LlmProfile {
        profile_id: "native-product".into(),
        provider_id: "deepseek".into(),
        model: "deepseek-v4-pro".into(),
        mode: ProfileMode::Completion,
        thinking: false,
        sampling: Sampling {
            temperature: 1.0,
            max_tokens: 2048,
        },
        proxy: ProxyConfig::None,
    };
    let mut chat = profile.clone();
    chat.profile_id = "native-chat".into();
    chat.mode = ProfileMode::Chat;
    chat.model = "deepseek-flash".into();
    ProfileStore::new(&llm).save(&[profile, chat])?;
    // File 标记阻止夹具读取用户真实 OS 条目；合成 key 不写入用户目录。
    CredentialFile::new(&llm).set_key(
        "deepseek",
        secrecy::SecretString::from("native-synthetic-key".to_owned()),
    )?;
    aoidos_lib::llm_commands::init_llm_dir(llm);
    let count = Arc::new(AtomicUsize::new(0));
    let mode = Arc::new(AtomicUsize::new(0));
    let passed = Arc::new(AtomicBool::new(false));
    let server = product_server::Server::new(count.clone(), mode.clone())?;
    let endpoint = server.endpoint.clone();
    let fixture_dir = dir.clone();
    let observed = passed.clone();
    let app = tauri::Builder::default()
        .setup(move |app| {
            app.manage(Fixture {
                restarting,
                dir: fixture_dir.clone(),
                count: count.clone(),
                mode: mode.clone(),
                passed: observed.clone(),
            });
            app.manage(aoidos_store::lock::acquire(&fixture_dir)?);
            let handle = app.handle().clone();
            let events = Arc::new(WindowEvents::new(move |name, payload| {
                handle
                    .emit_to("main", name, payload)
                    .map_err(delivery_error)
            }));
            app.manage(events.clone());
            let storage_events = StorageEvents::new(events.clone());
            let gate = Arc::new(CommitGate::default());
            app.manage(gate.clone());
            let storage = StorageService {
                storage: Arc::new(aoidos_engine::storage::Storage::new(
                    &fixture_dir,
                    storage_events.clone(),
                    Arc::new(SlowRecords {
                        inner: storage_events.clone(),
                        gate,
                    }),
                )),
                events: storage_events,
            };
            let turns = TurnService::new(events.clone(), SystemProxySnapshot::default())?;
            let factory = ProfileFactory::with_builder(
                Arc::new(SavedProfiles),
                storage.storage.clone(),
                turns.proxy_snapshot.clone(),
                turns.diagnostics.clone(),
                Arc::new(move |frozen, snapshot, auth| {
                    let key = frozen.credential.ok_or_else(missing_key)?;
                    let client =
                        aoidos_llm::proxy::build_client(&frozen.profile.proxy, snapshot, auth)
                            .map_err(transport_error)?;
                    Ok(Arc::new(aoidos_llm::providers::DeepSeek::with_client(
                        endpoint.clone(),
                        key,
                        client,
                    )))
                }),
                Arc::new(aoidos_llm::schedule::AllowAll),
            );
            app.manage(Product::new(
                &fixture_dir,
                storage.storage.clone(),
                turns.coordinator.clone(),
                events,
                factory.clone(),
            ));
            app.manage(aoidos_engine::game::runtime::Service::new(
                turns.coordinator.clone(),
                factory,
            ));
            app.manage(storage);
            app.manage(turns);
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("product.html".into()),
            )
            .title("Aoidos product integration")
            .visible(false)
            .build()?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(60));
                handle.exit(1);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            aoidos_lib::game_ipc::engine_list_scripts,
            aoidos_lib::game_ipc::engine_open_session,
            aoidos_lib::game_ipc::engine_submit_input,
            aoidos_lib::game_ipc::engine_get_phase,
            aoidos_lib::game_ipc::engine_cancel_round,
            aoidos_lib::game_ipc::engine_resume,
            aoidos_lib::game_ipc::engine_submit_check,
            aoidos_lib::turn_ipc::llm_get_turn,
            aoidos_lib::turn_ipc::llm_cancel,
            aoidos_lib::store_ipc::engine_get_record_view,
            aoidos_lib::store_ipc::engine_get_record_page,
            aoidos_lib::store_ipc::engine_get_record_body,
            aoidos_lib::store_ipc::store_set_ui_preferences,
            product_commit_gate,
            product_restarting,
            product_http,
            product_wait,
            product_restart,
            product_disk,
            product_report
        ])
        .build(tauri::generate_context!("tauri.conf.json", test = true))?;
    app.run(|_, _| {});
    drop(server);
    assert!(
        passed.load(Ordering::Acquire),
        "production HTTP/SSE main Webview verification failed"
    );
    println!(
        "production profile / raw scenario / HTTP SSE / cancel / recovery / reopen / disk: PASS"
    );
    Ok(())
}
fn delivery_error(_: tauri::Error) -> aoidos_engine::fault::Fault {
    aoidos_engine::fault::Fault::new("app.event-failed", "夹具窗口投递失败")
}
fn missing_key() -> aoidos_engine::fault::Fault {
    aoidos_engine::fault::Fault::new("llm.missing-key", "合成凭据缺失")
}
fn transport_error(_: reqwest::Error) -> aoidos_engine::fault::Fault {
    aoidos_engine::fault::Fault::new("llm.network", "夹具传输构造失败")
}
