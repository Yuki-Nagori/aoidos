//! 真实主 Webview 的命令 / 事件往返；本地夹具不发送收费 HTTP，独立进程主线程运行。

use aoidos_engine::fault::Fault;
use aoidos_lib::store_commands::{StorageEvents, StorageService};
use aoidos_lib::turn_commands::{TurnService, WindowEvents};
use aoidos_llm::{
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
#[path = "game-fixture.rs"]
mod game_fixture;

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
    app.state::<aoidos_engine::game::runtime::Service>()
        .shutdown()
        .await;
    service.coordinator.shutdown().await;
    let closed = app.state::<StorageService>().storage.close().is_ok();
    app.state::<StorageService>().events.shutdown().await;
    app.state::<Arc<WindowEvents>>().close();
    let cleaned = std::fs::remove_dir_all(&report.dir).is_ok();
    if passed && cleaned && closed {
        println!(
            "main Webview turn / record / phase / check / fork / restart / migration / preferences / recovery / listener release: PASS"
        );
    }
    app.exit(if passed && cleaned && closed { 0 } else { 1 });
    Ok(())
}

// 记录链路的合成请求只在此集成 target 注册，不是产品提交入口。
#[tauri::command]
async fn record_smoke(
    report: tauri::State<'_, Report>,
    service: tauri::State<'_, TurnService>,
    storage: tauri::State<'_, StorageService>,
) -> Result<serde_json::Value, String> {
    use aoidos_engine::{
        record::{
            format::{Header, hash, now},
            grammar,
            session::{PersistentWriter, Target},
        },
        request::PreparedGeneration,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let prefix = "[AOIDOS:STATIC]\n只推进已知场景。\n[/AOIDOS:STATIC]\n".to_owned();
    let header = Header {
        kind: "header".into(),
        format_version: 1,
        grammar_version: 1,
        projection_version: 1,
        script_id: "demo".into(),
        session_id: id.clone(),
        created_at: now(),
        static_prefix_hash: hash(prefix.as_bytes()),
        static_prefix: prefix.clone(),
        script_revision: hash(b"fixture"),
    };
    let session = storage
        .storage
        .records
        .create(header)
        .map_err(smoke_snapshot_error)?;
    let profiles = ProfileStore::new(&report.dir)
        .load()
        .map_err(smoke_store_error)?;
    let frozen = aoidos_llm::config::freeze(&profiles[0], &CredentialFile::new(&report.dir))
        .map_err(smoke_store_error)?;
    let request = PreparedGeneration::local_fixture_with_guard(
        frozen,
        &service.proxy_snapshot,
        None,
        aoidos_llm::provider::ProviderInput::Completion(aoidos_llm::provider::CompletionInput {
            prompt: format!("{prefix}{}", grammar::open(&Target::Narration)),
        }),
        grammar::guard(&Target::Narration),
    )
    .map_err(smoke_snapshot_error)?;
    let turn = service
        .coordinator
        .submit(
            request,
            Arc::new(PersistentWriter {
                session,
                target: Target::Narration,
                high: true,
            }),
        )
        .map_err(smoke_snapshot_error)?;
    Ok(serde_json::json!({"sessionId":id,"turnId":turn}))
}
fn smoke_store_error(error: aoidos_store::error::StoreError) -> String {
    format!("store.{}", error.code())
}

// 测试专用同步屏障不取消回合、不轮询；用于验证最后事件全丢后的主动恢复。
#[tauri::command]
async fn wait_smoke(service: tauri::State<'_, TurnService>, turn_id: String) -> Result<(), String> {
    service
        .coordinator
        .wait(&turn_id)
        .await
        .map_err(smoke_snapshot_error)?;
    Ok(())
}

// 卸载后以真实窗口事件确认 JS 监听器已释放；此标记不写入业务快照。
#[tauri::command]
fn emit_smoke(
    app: AppHandle,
    service: tauri::State<'_, TurnService>,
    turn_id: String,
    session_id: String,
    game: tauri::State<'_, aoidos_engine::game::runtime::Service>,
) -> Result<(), String> {
    let phase = game.get_phase(&session_id).map_err(smoke_snapshot_error)?;
    app.emit_to(
        "main",
        "engine:phase:changed",
        serde_json::json!({"seq": phase.seq.phase_changed + 1, "data": phase.state}),
    )
    .map_err(smoke_emit_error)?;
    let snapshot = service
        .coordinator
        .snapshot(&turn_id)
        .map_err(smoke_snapshot_error)?;
    app.emit_to(
        "main",
        "llm:turn:chunk",
        serde_json::json!({
            "seq": snapshot.seq.chunk + 1,
            "data": { "turnId": turn_id, "delta": "unmounted" }
        }),
    )
    .map_err(smoke_emit_error)
}
fn smoke_snapshot_error(error: Fault) -> String {
    error.code
}
fn smoke_emit_error(_: tauri::Error) -> String {
    "fixture-emit-failed".into()
}

fn delivery_failed(_: tauri::Error) -> Fault {
    Fault::new("app.event-failed", "主窗口事件无法投递")
}

// 可信场景只在本集成目标登记，产品参数无法指定模型、规则或文件路径。
#[tauri::command]
async fn game_smoke(
    scenario: Option<String>,
    game: tauri::State<'_, aoidos_engine::game::runtime::Service>,
    storage: tauri::State<'_, StorageService>,
    turns: tauri::State<'_, TurnService>,
    events: tauri::State<'_, Arc<WindowEvents>>,
) -> Result<String, String> {
    let header = game_fixture::header();
    let id = header.session_id.clone();
    let session = storage
        .storage
        .records
        .create(header)
        .map_err(smoke_snapshot_error)?;
    let context = game_fixture::context(
        storage.storage.clone(),
        session,
        turns.coordinator.clone(),
        events.inner().clone(),
        scenario,
    )
    .await
    .map_err(smoke_snapshot_error)?;
    game.register(context).map_err(smoke_snapshot_error)?;
    Ok(id)
}
#[tauri::command]
async fn game_restart_smoke(
    game: tauri::State<'_, aoidos_engine::game::runtime::Service>,
    storage: tauri::State<'_, StorageService>,
    turns: tauri::State<'_, TurnService>,
    events: tauri::State<'_, Arc<WindowEvents>>,
    session_id: String,
) -> Result<(), String> {
    game.close_session(&session_id)
        .await
        .map_err(smoke_snapshot_error)?;
    storage
        .storage
        .records
        .close(&session_id)
        .map_err(smoke_snapshot_error)?;
    let session = storage
        .storage
        .records
        .open("demo", &session_id)
        .map_err(smoke_snapshot_error)?;
    let context = game_fixture::context(
        storage.storage.clone(),
        session,
        turns.coordinator.clone(),
        events.inner().clone(),
        None,
    )
    .await
    .map_err(smoke_snapshot_error)?;
    game.register(context).map_err(smoke_snapshot_error)
}

// 仅原生测试等待确认快照；产品消费者没有定时轮询。
#[tauri::command]
async fn game_wait_smoke(
    game: tauri::State<'_, aoidos_engine::game::runtime::Service>,
    session_id: String,
    operation_id: String,
) -> Result<(), String> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let snapshot = game.get_phase(&session_id).map_err(smoke_snapshot_error)?;
            if snapshot.state.last_operation.is_some_and(|operation| {
                operation.operation_id == operation_id
                    && operation.outcome == aoidos_engine::game::state::OperationOutcome::Completed
            }) {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(smoke_timeout_error)?
}
fn smoke_timeout_error(_: tokio::time::error::Elapsed) -> String {
    "fixture-game-timeout".into()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("aoidos-ipc-{}", std::process::id()));
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
        secrecy::SecretString::from("aoidos-local-fixture".to_owned()),
    )?;
    aoidos_lib::llm_commands::init_llm_dir(dir.clone());
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
            let storage = StorageService::new(&fixture_dir, StorageEvents::new(events.clone()));
            let turns = TurnService::new(events, SystemProxySnapshot::default())?;
            app.manage(aoidos_engine::game::runtime::Service::new(
                turns.coordinator.clone(),
                Arc::new(game_fixture::Factory {
                    dir: fixture_dir.clone(),
                    storage: storage.storage.clone(),
                }),
            ));
            app.manage(storage);
            app.manage(turns);
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Aoidos IPC fixture")
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
            aoidos_lib::turn_ipc::llm_submit,
            aoidos_lib::turn_ipc::llm_get_turn,
            aoidos_lib::turn_ipc::llm_cancel,
            aoidos_lib::store_ipc::store_get_migration,
            aoidos_lib::store_ipc::store_get_ui_preferences,
            aoidos_lib::store_ipc::store_set_ui_preferences,
            aoidos_lib::store_ipc::engine_get_record_page,
            aoidos_lib::store_ipc::engine_get_record_view,
            aoidos_lib::store_ipc::engine_get_record_body,
            aoidos_lib::game_ipc::engine_submit_input,
            aoidos_lib::game_ipc::engine_interrupt,
            aoidos_lib::game_ipc::engine_cancel_round,
            aoidos_lib::game_ipc::engine_resume,
            aoidos_lib::game_ipc::engine_regenerate,
            aoidos_lib::game_ipc::engine_rewind,
            aoidos_lib::game_ipc::engine_submit_check,
            aoidos_lib::game_ipc::engine_get_phase,
            game_smoke,
            game_restart_smoke,
            game_wait_smoke,
            record_smoke,
            report_smoke,
            wait_smoke,
            emit_smoke
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
    println!(
        "main Webview turn / record / phase / check / fork / restart / migration / preferences / recovery / listener release: PASS"
    );
    Ok(())
}
