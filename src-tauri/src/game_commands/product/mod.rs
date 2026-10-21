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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenSelectedRunArgs {
    session_id: String,
    expected_revision: u64,
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

/// 玩家从目录选择周目后显式恢复，不自动提交行动或发送模型请求。
/// # Errors
/// revision 过期、身份不一致、剧本不兼容或恢复失败时保持旧活动选择。
pub async fn open_selected_run(
    product: &Product,
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<OpenedSession, CmdError> {
    let args: OpenSelectedRunArgs = crate::ipc::decode_request(&request, "周目恢复参数不合法")?;
    product
        .open_selected(service, args.session_id, args.expected_revision)
        .await
        .map_err(CmdError::from)
}

#[cfg(test)]
mod tests;
