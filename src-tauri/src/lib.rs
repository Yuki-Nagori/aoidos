mod commands;
pub mod events;
pub mod game_commands;
pub mod ipc;
pub mod llm_commands;
pub mod store_commands;
pub mod theme_commands;
pub mod turn_commands;

use tauri::{AppHandle, Emitter, Manager};

use ipc::CmdError;

/// 桌面壳只提供 UI 主线程调度，存储 / 状态规则由 llm_commands 维护。
#[tauri::command]
async fn llm_set_key(
    app: AppHandle,
    provider_id: String,
    action: String,
) -> Result<aoidos_llm::credentials::KeyStatus, CmdError> {
    llm_commands::set_key_with_dispatch(
        provider_id,
        action,
        aoidos_llm::platform::platform_prompt(),
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
    ) -> Result<aoidos_engine::turn::TurnSnapshot, CmdError> {
        state.coordinator.snapshot(&turn_id).map_err(CmdError::from)
    }

    /// # Errors
    /// 未知 turnId 为 app.not-found；响应等待提交一致边界。
    #[tauri::command]
    pub async fn llm_cancel(
        state: tauri::State<'_, turn_commands::TurnService>,
        turn_id: String,
    ) -> Result<aoidos_engine::turn::CancelledTurn, CmdError> {
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
    ) -> Result<aoidos_engine::migration::Snapshot, CmdError> {
        store_commands::store_get_migration(state)
    }
    #[tauri::command]
    pub async fn store_get_ui_preferences(
        state: Service<'_>,
    ) -> Result<aoidos_engine::preferences::UiPreferences, CmdError> {
        store_commands::store_get_ui_preferences(state).await
    }
    #[tauri::command]
    pub async fn store_set_ui_preferences(
        state: Service<'_>,
        request: Request<'_>,
    ) -> Result<aoidos_engine::preferences::UiPreferences, CmdError> {
        store_commands::store_set_ui_preferences(state, request).await
    }
    #[tauri::command]
    pub async fn engine_get_record_page(
        state: Service<'_>,
        request: Request<'_>,
    ) -> Result<aoidos_engine::record::view::Page, CmdError> {
        store_commands::engine_get_record_page(state, request).await
    }
    #[tauri::command]
    pub async fn engine_get_record_view(
        state: Service<'_>,
        request: Request<'_>,
    ) -> Result<aoidos_engine::record::view::View, CmdError> {
        store_commands::engine_get_record_view(state, request).await
    }
    #[tauri::command]
    pub async fn engine_get_record_body(
        state: Service<'_>,
        request: Request<'_>,
    ) -> Result<aoidos_engine::record::view::BodyPage, CmdError> {
        store_commands::engine_get_record_body(state, request).await
    }
}

/// 产品阶段命令宏装配；原生夹具注册同一组入口。
pub mod game_ipc {
    use super::{CmdError, game_commands};
    use aoidos_engine::game::{runtime::Service, state::*};
    #[tauri::command]
    pub fn engine_list_scripts() -> Result<Vec<aoidos_engine::game::assets::ScriptInfo>, CmdError> {
        game_commands::list_scripts()
    }
    #[tauri::command]
    pub async fn engine_open_session(
        product: tauri::State<'_, aoidos_engine::game::product::Product>,
        state: tauri::State<'_, Service>,
        request: tauri::ipc::Request<'_>,
    ) -> Result<aoidos_engine::game::product::OpenedSession, CmdError> {
        game_commands::open_session(&product, &state, request).await
    }
    #[tauri::command]
    pub async fn engine_submit_input(
        state: tauri::State<'_, Service>,
        request: tauri::ipc::Request<'_>,
    ) -> Result<AcceptedRound, CmdError> {
        game_commands::submit_input(&state, request).await
    }
    #[tauri::command]
    pub async fn engine_interrupt(
        state: tauri::State<'_, Service>,
        request: tauri::ipc::Request<'_>,
    ) -> Result<AcceptedOperation, CmdError> {
        game_commands::interrupt(&state, request).await
    }
    #[tauri::command]
    pub async fn engine_cancel_round(
        state: tauri::State<'_, Service>,
        request: tauri::ipc::Request<'_>,
    ) -> Result<CancelledRound, CmdError> {
        game_commands::cancel_round(&state, request).await
    }
    #[tauri::command]
    pub async fn engine_resume(
        state: tauri::State<'_, Service>,
        request: tauri::ipc::Request<'_>,
    ) -> Result<AcceptedRound, CmdError> {
        game_commands::resume(&state, request).await
    }
    #[tauri::command]
    pub async fn engine_regenerate(
        state: tauri::State<'_, Service>,
        request: tauri::ipc::Request<'_>,
    ) -> Result<AcceptedRound, CmdError> {
        game_commands::regenerate(&state, request).await
    }
    #[tauri::command]
    pub async fn engine_rewind(
        state: tauri::State<'_, Service>,
        request: tauri::ipc::Request<'_>,
    ) -> Result<AcceptedOperation, CmdError> {
        game_commands::rewind(&state, request).await
    }
    #[tauri::command]
    pub async fn engine_submit_check(
        state: tauri::State<'_, Service>,
        request: tauri::ipc::Request<'_>,
    ) -> Result<AcceptedCheck, CmdError> {
        game_commands::submit_check(&state, request).await
    }
    #[tauri::command]
    pub fn engine_get_phase(
        state: tauri::State<'_, Service>,
        request: tauri::ipc::Request<'_>,
    ) -> Result<PhaseSnapshot, CmdError> {
        game_commands::get_phase(&state, request)
    }
}

macro_rules! command_handler {
    ($($extra:path),* $(,)?) => { tauri::generate_handler![
        game_ipc::engine_list_scripts, game_ipc::engine_open_session, game_ipc::engine_submit_input, game_ipc::engine_interrupt, game_ipc::engine_cancel_round, game_ipc::engine_resume, game_ipc::engine_regenerate, game_ipc::engine_rewind, game_ipc::engine_submit_check, game_ipc::engine_get_phase,
        commands::store_list_backups, store_ipc::store_get_migration,
        store_ipc::store_get_ui_preferences, store_ipc::store_set_ui_preferences, store_ipc::engine_get_record_page,store_ipc::engine_get_record_view,store_ipc::engine_get_record_body,
        llm_commands::llm_list_profiles, llm_commands::llm_save_profile, llm_commands::llm_delete_profile,
        llm_commands::llm_get_key_status, llm_set_key, turn_ipc::llm_get_turn, turn_ipc::llm_cancel,
        theme_commands::theme_list, theme_commands::theme_get_preference, theme_commands::theme_set_preference, theme_commands::theme_skin_load, $($extra),*
    ] };
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().setup(|app| {
        let dir = app.path().app_data_dir()?;
        // 配置 / 凭据也是数据根写者；持锁到应用退出，后续 store 服务复用。
        app.manage(aoidos_store::lock::acquire(&dir)?);
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
        let turns = turn_commands::TurnService::new(
            events.clone(),
            aoidos_llm::proxy::SystemProxySnapshot::capture(),
        )?;
        let factory = aoidos_engine::game::product::ProfileFactory::new(
            std::sync::Arc::new(game_commands::SavedProfiles),
            storage.storage.clone(),
            turns.proxy_snapshot.clone(),
            turns.diagnostics.clone(),
        );
        app.manage(aoidos_engine::game::product::Product::new(
            &dir,
            storage.storage.clone(),
            turns.coordinator.clone(),
            events,
            factory.clone(),
        ));
        app.manage(storage);
        app.manage(aoidos_engine::game::runtime::Service::new(
            turns.coordinator.clone(),
            factory.clone(),
        ));
        app.manage(factory);
        app.manage(turns);
        let theme = storage_theme_bootstrap(
            app.state::<store_commands::StorageService>()
                .storage
                .as_ref(),
        );
        let bootstrap = serde_json::to_string(&theme).expect("theme bootstrap serializes");
        app.manage(theme_commands::ThemeService::new(theme_resource_root(app)?));
        tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
            .title("Aoidos")
            .inner_size(960.0, 640.0)
            .background_color(tauri::webview::Color(8, 16, 24, 255))
            .initialization_script(format!("window.__AOIDOS_THEME_BOOTSTRAP__={bootstrap};"))
            .build()?;
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
                    handle
                        .state::<aoidos_engine::game::runtime::Service>()
                        .shutdown()
                        .await;
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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ThemeBootstrap {
    version: u8,
    theme: String,
    color_scheme: String,
    fallback_reason: Option<&'static str>,
}

fn storage_theme_bootstrap(storage: &aoidos_engine::storage::Storage) -> ThemeBootstrap {
    match storage
        .with_database(|connection| aoidos_theme::preference::get(connection).map_err(Into::into))
    {
        Ok(preference) => theme_bootstrap_for(preference),
        Err(error) => ThemeBootstrap {
            version: 1,
            theme: "dark".into(),
            color_scheme: "dark".into(),
            fallback_reason: Some(if error.code == "store.corrupt" {
                "invalidPreference"
            } else {
                "storageUnavailable"
            }),
        },
    }
}

fn theme_bootstrap_for(preference: aoidos_theme::preference::ThemePreference) -> ThemeBootstrap {
    let selected = aoidos_theme::catalog::themes().and_then(|themes| {
        themes
            .into_iter()
            .find(|theme| theme.id == preference.theme)
    });
    match selected {
        Some(theme) => ThemeBootstrap {
            version: 1,
            theme: theme.id,
            color_scheme: theme.color_scheme,
            fallback_reason: None,
        },
        None => ThemeBootstrap {
            version: 1,
            theme: "dark".into(),
            color_scheme: "dark".into(),
            fallback_reason: Some("themeUnavailable"),
        },
    }
}

fn theme_resource_root(
    _app: &tauri::App,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    #[cfg(debug_assertions)]
    {
        Ok(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../resources/scripts"))
    }
    #[cfg(not(debug_assertions))]
    {
        Ok(_app.path().resource_dir()?.join("resources/scripts"))
    }
}
fn window_delivery_error(_: tauri::Error) -> aoidos_engine::fault::Fault {
    aoidos_engine::fault::Fault::new("app.event-failed", "主窗口事件无法投递")
}

#[cfg(test)]
mod theme_bootstrap_tests {
    use super::*;

    #[test]
    fn registered_theme_is_used_and_retired_theme_falls_back_without_rewriting() {
        let available = theme_bootstrap_for(aoidos_theme::preference::ThemePreference {
            version: 1,
            theme: "light".into(),
        });
        assert_eq!(available.theme, "light");
        assert_eq!(available.color_scheme, "light");
        assert_eq!(available.fallback_reason, None);

        let unavailable = theme_bootstrap_for(aoidos_theme::preference::ThemePreference {
            version: 1,
            theme: "retired-theme".into(),
        });
        assert_eq!(unavailable.theme, "dark");
        assert_eq!(unavailable.color_scheme, "dark");
        assert_eq!(unavailable.fallback_reason, Some("themeUnavailable"));
    }
}
