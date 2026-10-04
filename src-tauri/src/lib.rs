mod commands;
pub mod events;
pub mod ipc;

use tauri::{AppHandle, Emitter, Manager, Runtime};

use ipc::CmdError;

/// 事件发送装配胶水：契约信封（events 模块）+ `emit_to("main")`。
/// 本文件在覆盖率口径外（装配代码需要活的 AppHandle，tauri::test 的 mock
/// 在 Windows 触发 STATUS_ENTRYPOINT_NOT_FOUND）——事件名与载荷形状的
/// 纯逻辑在 events 模块内有直测，这里只做一行的平台投递。
// TODO(task 017): 首个调用方在 017 落地；在此之前 dead_code 为接口先行的预期状态。
#[allow(dead_code)]
pub(crate) fn emit_event<R: Runtime, T: serde::Serialize + ?Sized>(
    app: &AppHandle<R>,
    event: &str,
    stream_id: &str,
    data: &T,
) -> Result<(), CmdError> {
    let payload = events::envelope(event, stream_id, data);
    app.emit_to("main", event, payload).map_err(|source| {
        CmdError::new(
            "app.event-failed",
            format!("事件 {event} 发送失败"),
            Some(serde_json::json!({ "event": event, "source": source.to_string() })),
        )
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("app data dir");
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
