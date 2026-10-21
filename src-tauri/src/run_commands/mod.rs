//! 周目目录 IPC；命令只解码参数并调用共享应用服务。

use crate::ipc::CmdError;
use aoidos_app::runs::RunLifecycleService;
use aoidos_engine::run_catalog::{RunCursor, RunPage};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageArgs {
    #[serde(default)]
    cursor: Option<RunCursor>,
    #[serde(default = "default_page_size")]
    page_size: usize,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindProfileArgs {
    session_id: String,
    expected_revision: u64,
    profile_id: String,
}

fn default_page_size() -> usize {
    aoidos_engine::run_catalog::DEFAULT_PAGE_SIZE
}

/// 按固定游标读取本地周目摘要，不访问 transcript 或恢复游戏状态。
/// # Errors
/// 非法游标、目录修订已变化或存储故障时返回稳定错误。
pub fn list_page(
    service: &RunLifecycleService,
    request: tauri::ipc::Request<'_>,
) -> Result<RunPage, CmdError> {
    let args: PageArgs = crate::ipc::decode_request(&request, "周目目录参数不合法")?;
    service
        .page(args.cursor.as_ref(), args.page_size)
        .map_err(CmdError::from)
}

/// 以 CAS 方式绑定旧周目的公开 profile 快照，不读取密钥。
/// # Errors
/// 参数无效、周目 revision 已变化或配置不可用时返回稳定错误。
pub fn bind_profile(
    service: &RunLifecycleService,
    request: tauri::ipc::Request<'_>,
) -> Result<u64, CmdError> {
    let args: BindProfileArgs = crate::ipc::decode_request(&request, "周目配置绑定参数不合法")?;
    service
        .bind_profile(&args.session_id, args.expected_revision, &args.profile_id)
        .map_err(CmdError::from)
}
