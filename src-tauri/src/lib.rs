mod commands;
pub mod events;
pub mod ipc;
pub mod llm_commands;
pub mod store_commands;
pub mod turn_commands;

use tauri::{AppHandle, Emitter, Manager};

use ipc::CmdError;

/// 桌面壳只提供 UI 主线程调度，存储 / 状态规则由 llm_commands 维护。
#[tauri::command]
async fn llm_set_key(
    app: AppHandle,
    provider_id: String,
    action: String,
) -> Result<mythos_llm::credentials::KeyStatus, CmdError> {
    llm_commands::set_key_with_dispatch(
        provider_id,
        action,
        mythos_llm::platform::platform_prompt(),
        &move |job| app.run_on_main_thread(job),
    )
    .await
}

/// 回合 IPC 薄适配；公开模块供真实窗口集成注册同一组命令。
pub mod turn_ipc {
    use super::{CmdError, turn_commands};
    /// 开发构建只提交已登记的本地夹具；生产不编译 / 注册此命令。
    #[cfg(debug_assertions)]
    #[tauri::command]
    pub async fn llm_submit(
        state: tauri::State<'_, turn_commands::TurnService>,
        profile_id: String,
        input: serde_json::Value,
        guard_spec_id: String,
    ) -> Result<turn_commands::AcceptedTurn, CmdError> {
        turn_commands::submit(
            &state,
            profile_id,
            turn_commands::decode_input(input)?,
            guard_spec_id,
        )
    }

    /// # Errors
    /// 未知或已清退 turnId 为 app.not-found。
    #[tauri::command]
    pub fn llm_get_turn(
        state: tauri::State<'_, turn_commands::TurnService>,
        turn_id: String,
    ) -> Result<mythos_engine::turn::TurnSnapshot, CmdError> {
        state.coordinator.snapshot(&turn_id).map_err(CmdError::from)
    }

    /// # Errors
    /// 未知 turnId 为 app.not-found；响应等待提交一致边界。
    #[tauri::command]
    pub async fn llm_cancel(
        state: tauri::State<'_, turn_commands::TurnService>,
        turn_id: String,
    ) -> Result<mythos_engine::turn::CancelledTurn, CmdError> {
        state
            .coordinator
            .cancel(&turn_id)
            .await
            .map_err(CmdError::from)
    }
}

/// 存储 IPC 宏装配与业务模块分离，原生夹具注册同一组入口。
pub mod store_ipc {
    use super::{CmdError, store_commands};
    use tauri::{State, ipc::Request};
    type Service<'a> = State<'a, store_commands::StorageService>;
    #[tauri::command]
    pub fn store_get_migration(
        state: Service<'_>,
    ) -> Result<mythos_engine::migration::Snapshot, CmdError> {
        store_commands::store_get_migration(state)
    }
    #[tauri::command]
    pub async fn store_get_ui_preferences(
        state: Service<'_>,
    ) -> Result<mythos_engine::preferences::UiPreferences, CmdError> {
        store_commands::store_get_ui_preferences(state).await
    }
    #[tauri::command]
    pub async fn store_set_ui_preferences(
        state: Service<'_>,
        request: Request<'_>,
    ) -> Result<mythos_engine::preferences::UiPreferences, CmdError> {
        store_commands::store_set_ui_preferences(state, request).await
    }
    #[tauri::command]
    pub async fn engine_get_record_page(
        state: Service<'_>,
        request: Request<'_>,
    ) -> Result<mythos_engine::record::view::Page, CmdError> {
        store_commands::engine_get_record_page(state, request).await
    }
    #[tauri::command]
    pub async fn engine_get_record_view(
        state: Service<'_>,
        request: Request<'_>,
    ) -> Result<mythos_engine::record::view::View, CmdError> {
        store_commands::engine_get_record_view(state, request).await
    }
    #[tauri::command]
    pub async fn engine_get_record_body(
        state: Service<'_>,
        request: Request<'_>,
    ) -> Result<mythos_engine::record::view::BodyPage, CmdError> {
        store_commands::engine_get_record_body(state, request).await
    }
}

macro_rules! command_handler {
    ($($extra:path),* $(,)?) => { tauri::generate_handler![
        commands::greet, commands::store_list_backups, store_ipc::store_get_migration,
        store_ipc::store_get_ui_preferences, store_ipc::store_set_ui_preferences, store_ipc::engine_get_record_page,store_ipc::engine_get_record_view,store_ipc::engine_get_record_body,
        llm_commands::llm_list_profiles, llm_commands::llm_save_profile, llm_commands::llm_delete_profile,
        llm_commands::llm_get_key_status, llm_set_key, turn_ipc::llm_get_turn, turn_ipc::llm_cancel, $($extra),*
    ] };
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().setup(|app| {
        let dir = app.path().app_data_dir()?;
        // 配置 / 凭据也是数据根写者；持锁到应用退出，后续 store 服务复用。
        app.manage(mythos_store::lock::acquire(&dir)?);
        commands::init_db_path(dir.join("storage.sqlite"));
        llm_commands::init_llm_dir(dir.join("llm"));
        let handle = app.handle().clone();
        let events =
            std::sync::Arc::new(turn_commands::WindowEvents::new(move |event, payload| {
                handle
                    .emit_to("main", event, payload)
                    .map_err(window_delivery_error)
            }));
        app.manage(events.clone());
        let storage_events = store_commands::StorageEvents::new(events.clone());
        let storage = store_commands::StorageService::new(&dir, storage_events);
        app.manage(storage);
        app.manage(turn_commands::TurnService::new(
            events,
            mythos_llm::proxy::SystemProxySnapshot::capture(),
        )?);
        Ok(())
    });
    #[cfg(debug_assertions)]
    let builder = builder.invoke_handler(command_handler!(turn_ipc::llm_submit));
    #[cfg(not(debug_assertions))]
    let builder = builder.invoke_handler(command_handler!());
    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    let exiting = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.run(move |handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event
            && !finished.load(std::sync::atomic::Ordering::Acquire)
        {
            api.prevent_exit();
            if !exiting.swap(true, std::sync::atomic::Ordering::AcqRel) {
                let coordinator = handle
                    .state::<turn_commands::TurnService>()
                    .coordinator
                    .clone();
                let handle = handle.clone();
                let finished = finished.clone();
                tauri::async_runtime::spawn(async move {
                    coordinator.shutdown().await;
                    let _ = handle
                        .state::<store_commands::StorageService>()
                        .storage
                        .close();
                    handle
                        .state::<store_commands::StorageService>()
                        .events
                        .shutdown()
                        .await;
                    handle
                        .state::<std::sync::Arc<turn_commands::WindowEvents>>()
                        .close();
                    finished.store(true, std::sync::atomic::Ordering::Release);
                    handle.exit(0);
                });
            }
        }
    });
}
fn window_delivery_error(_: tauri::Error) -> mythos_engine::fault::Fault {
    mythos_engine::fault::Fault::new("app.event-failed", "主窗口事件无法投递")
}
