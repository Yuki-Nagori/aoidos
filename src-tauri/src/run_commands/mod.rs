//! 周目目录 IPC；命令只解码参数并调用共享应用服务。

use crate::ipc::CmdError;
use aoidos_app::runs::RunLifecycleService;
use aoidos_billing::money::NanoMoney;
use aoidos_engine::game::{product::Product, runtime::Service};
use aoidos_engine::run_catalog::{RunCursor, RunPage};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenSelectedArgs {
    session_id: String,
    expected_revision: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteRunArgs {
    operation_id: String,
    session_id: String,
    expected_revision: u64,
}

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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelectedLimits {
    cny_limit: NanoMoney,
    usd_limit: NanoMoney,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReconcileBillingArgs {
    session_id: String,
    expected_revision: u64,
    #[serde(default)]
    selected_limits: Option<SelectedLimits>,
}

fn default_page_size() -> usize {
    aoidos_engine::run_catalog::DEFAULT_PAGE_SIZE
}

/// 显式恢复所选周目，并返回其冻结的公开 profile；不会自动发送行动。
/// # Errors
/// revision 过期、身份不一致、配置损坏或 owner 恢复失败时返回稳定错误。
pub async fn open_selected(
    lifecycle: &RunLifecycleService,
    product: &Product,
    runtime: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<aoidos_app::runs::OpenedRun, CmdError> {
    let args: OpenSelectedArgs = crate::ipc::decode_request(&request, "周目恢复参数不合法")?;
    lifecycle
        .open_selected(product, runtime, args.session_id, args.expected_revision)
        .await
        .map_err(CmdError::from)
}

/// 玩家确认后通过共享生命周期服务推进删除；同一 operation ID 可用于安全重试。
/// # Errors
/// 计费门禁、revision、身份或 journal 阶段不匹配时返回稳定错误。
pub async fn delete_run(
    lifecycle: &RunLifecycleService,
    product: &Product,
    runtime: &Service,
    request: tauri::ipc::Request<'_>,
) -> Result<aoidos_engine::run_catalog::DeletionOperation, CmdError> {
    let args: DeleteRunArgs = crate::ipc::decode_request(&request, "周目删除参数不合法")?;
    lifecycle
        .delete_run(
            product,
            runtime,
            args.operation_id,
            args.session_id,
            args.expected_revision,
        )
        .await
        .map_err(CmdError::from)
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

/// 玩家主动推进一个受 I/O 预算限制的旧周目发现批次。
/// # Errors
/// 存储不可用或扫描批次无法登记时返回稳定错误。
pub async fn scan_legacy(
    service: RunLifecycleService,
) -> Result<aoidos_engine::run_catalog::LegacyScanResult, CmdError> {
    aoidos_engine::blocking::run(move || service.scan_legacy())
        .await
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

/// 显式迁移历史账务 scope；预算冲突时由玩家提交最终双币种额度。
/// # Errors
/// revision、身份映射或账务冲突不满足迁移条件时返回稳定错误。
pub fn reconcile_billing(
    service: &RunLifecycleService,
    request: tauri::ipc::Request<'_>,
) -> Result<aoidos_engine::run_catalog::RunBillingReconciliation, CmdError> {
    let args: ReconcileBillingArgs =
        crate::ipc::decode_request(&request, "历史费用迁移参数不合法")?;
    let selected = args
        .selected_limits
        .map(|limits| (limits.cny_limit, limits.usd_limit));
    service
        .reconcile_billing(&args.session_id, args.expected_revision, selected)
        .map_err(CmdError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_limits_use_decimal_strings_and_named_currencies() {
        let args: ReconcileBillingArgs = serde_json::from_value(serde_json::json!({
            "sessionId": "00000000-0000-4000-8000-000000000001",
            "expectedRevision": 4,
            "selectedLimits": { "cnyLimit": "12.5", "usdLimit": "3" }
        }))
        .unwrap();

        assert_eq!(args.session_id, "00000000-0000-4000-8000-000000000001");
        let limits = args.selected_limits.unwrap();
        assert_eq!(limits.cny_limit.to_decimal_string(), "12.5");
        assert_eq!(limits.usd_limit.to_decimal_string(), "3");
        assert!(
            serde_json::from_value::<ReconcileBillingArgs>(serde_json::json!({
                "sessionId": "00000000-0000-4000-8000-000000000001",
                "expectedRevision": 4,
                "selectedLimits": { "cnyLimit": 12.5, "usdLimit": "3" }
            }))
            .is_err()
        );
    }
}
