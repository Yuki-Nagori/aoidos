//! 配置所有者提供冻结副本；会话登记及资源选择由 engine 执行。

use crate::ipc::CmdError;
use aoidos_engine::game::{
    assets::ScriptInfo,
    product::{OpenedSession, Product},
    runtime::Service,
};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenArgs {
    profile_id: String,
    script_id: String,
    start_new: bool,
}

/// # Errors
/// 内嵌资源不合法时拒绝，不触发网络或导入文件。
pub fn list_scripts() -> Result<Vec<ScriptInfo>, CmdError> {
    aoidos_engine::game::assets::list().map_err(CmdError::from)
}
/// # Errors
/// 完整参数、存储、配置或共享门禁错误按稳定码返回。
pub async fn open_session(
    product: &Product,
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<OpenedSession, CmdError> {
    let args: OpenArgs = crate::ipc::decode_request(&request, "会话选择参数不合法")?;
    product
        .open(service, args.profile_id, args.script_id, args.start_new)
        .await
        .map_err(CmdError::from)
}

#[cfg(test)]
mod tests;
