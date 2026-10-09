//! 产品参数只携带 session / round / plan 身份；业务转换与存储属于 engine。

mod events;
use crate::ipc::CmdError;
use aoidos_engine::game::{runtime::Service, state::*};

mod product;
pub use product::SavedProfiles;
pub use product::{list_scripts, open_session};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionArgs {
    session_id: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InputArgs {
    session_id: String,
    text: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoundArgs {
    session_id: String,
    round_id: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InterruptArgs {
    session_id: String,
    round_id: String,
    text: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckArgs {
    session_id: String,
    round_id: String,
    plan_id: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RewindArgs {
    session_id: String,
    target_seq: u64,
}

fn decode<T: serde::de::DeserializeOwned>(
    request: &tauri::ipc::Request<'_>,
) -> Result<T, CmdError> {
    crate::ipc::decode_request(request, "阶段命令参数不合法")
}
/// # Errors
/// 完整 DTO 形状、原文、身份、共享门禁或接纳失败按稳定错误透传。
pub async fn submit_input(
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<AcceptedRound, CmdError> {
    let args: InputArgs = decode(&request)?;
    service
        .submit_input(&args.session_id, args.text)
        .await
        .map_err(CmdError::from)
}
/// # Errors
/// 身份、忙碌交接或非法阶段拒绝，响应等待旧调用的一致边界。
pub async fn interrupt(
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<AcceptedOperation, CmdError> {
    let args: InterruptArgs = decode(&request)?;
    service
        .interrupt(&args.session_id, &args.round_id, args.text)
        .await
        .map_err(CmdError::from)
}
/// # Errors
/// 非法 / 未登记身份拒绝；缓存的终态不产生第二次封口。
pub async fn cancel_round(
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<CancelledRound, CmdError> {
    let args: RoundArgs = decode(&request)?;
    service
        .cancel_round(&args.session_id, &args.round_id)
        .await
        .map_err(CmdError::from)
}
/// # Errors
/// 无确认检查点、pending 或共享门禁错误透传，不隐式续费。
pub async fn resume(
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<AcceptedRound, CmdError> {
    let args: SessionArgs = decode(&request)?;
    service
        .resume(&args.session_id)
        .await
        .map_err(CmdError::from)
}
/// # Errors
/// 非最近终态 / 不合法边界或世界重建失败拒绝，不接受骰值覆盖。
pub async fn regenerate(
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<AcceptedRound, CmdError> {
    let args: RoundArgs = decode(&request)?;
    service
        .regenerate(&args.session_id, &args.round_id)
        .await
        .map_err(CmdError::from)
}
/// # Errors
/// 任意正文切片、pending 或世界解释器失败拒绝。
pub async fn rewind(
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<AcceptedOperation, CmdError> {
    let args: RewindArgs = decode(&request)?;
    service
        .rewind(&args.session_id, args.target_seq)
        .await
        .map_err(CmdError::from)
}
/// # Errors
/// 不合法 / 过期计划拒绝，确认过的计划幂等返回。
pub async fn submit_check(
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<AcceptedCheck, CmdError> {
    let args: CheckArgs = decode(&request)?;
    service
        .submit_check(&args.session_id, &args.round_id, &args.plan_id)
        .await
        .map_err(CmdError::from)
}
/// # Errors
/// 身份形状及未登记会话拒绝；读取不发 LLM 请求或恢复磁盘。
pub fn get_phase(
    service: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<PhaseSnapshot, CmdError> {
    let args: SessionArgs = decode(&request)?;
    service.get_phase(&args.session_id).map_err(CmdError::from)
}

#[cfg(test)]
mod tests;
