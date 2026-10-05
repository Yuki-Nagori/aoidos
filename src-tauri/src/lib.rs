mod commands;
pub mod events;
pub mod ipc;

use tauri::{AppHandle, Emitter, Manager, Runtime};

use ipc::CmdError;

/// 事件发送装配胶水：契约信封（events 模块）+ `emit_to("main")`。
/// 本文件在覆盖率口径外（装配代码需要活的 AppHandle，tauri::test 的 mock
/// 在 Windows 触发 STATUS_ENTRYPOINT_NOT_FOUND）——事件名与载荷形状的
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
            commands::init_db_path(dir.join("storage.sqlite"));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::store_list_backups,
            commands::store_get_migration
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
