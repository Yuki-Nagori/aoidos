mod commands;
pub mod events;
pub mod ipc;
pub mod llm_commands;

use tauri::{AppHandle, Emitter, Manager, Runtime};

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

/// 事件发送装配胶水：契约信封（events 模块）+ `emit_to("main")`。
/// 本文件在覆盖率口径外（装配代码需要活的 `AppHandle`，`tauri::test` 的 mock
/// 在 Windows 触发 `STATUS_ENTRYPOINT_NOT_FOUND`）——事件名与载荷形状的
/// 纯逻辑在 events / ipc 模块内有直测，这里只适配平台投递。
///
/// # Errors
///
/// 序列化、序号分配或平台投递失败时返回 `app.event-failed`。
/// 同一流由调用方串行发送；失败不代表前端已收到，序号也可能已被保留。
// TODO(task 020): 020 / 022 首个真实发送方接入后移除 dead_code 允许，并验证 main 窗口投递。
#[allow(dead_code)]
pub(crate) fn emit_event<R: Runtime, T: serde::Serialize + ?Sized>(
    app: &AppHandle<R>,
    event: &str,
    stream_id: &str,
    data: &T,
) -> Result<(), CmdError> {
    let payload = events::envelope(event, stream_id, data)?;
    match app.emit_to("main", event, payload) {
        Ok(()) => Ok(()),
        Err(source) => Err(ipc::event_delivery_error(event, source.to_string())),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            // 配置 / 凭据也是数据根写者；持锁到应用退出，后续 store 服务复用。
            app.manage(mythos_store::lock::acquire(&dir)?);
            commands::init_db_path(dir.join("storage.sqlite"));
            llm_commands::init_llm_dir(dir.join("llm"));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::store_list_backups,
            commands::store_get_migration,
            llm_commands::llm_list_profiles,
            llm_commands::llm_save_profile,
            llm_commands::llm_delete_profile,
            llm_commands::llm_get_key_status,
            llm_set_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
