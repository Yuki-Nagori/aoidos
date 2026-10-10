//! 计费命令只解码类型化 IPC，并转交给引擎账本。

use crate::{ipc::CmdError, store_commands::StorageService};
use aoidos_billing::money::Currency;
use aoidos_billing::{money::NanoMoney, price::PriceVersion};
use aoidos_engine::{
    billing::{
        BillingError, BillingLedger, BudgetSettings, BudgetWarning, ModelUsageSummary, RequestPage,
        SuggestedLimits,
    },
    fault::Fault,
};
use serde::Deserialize;

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunArgs {
    run_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetSettingsArgs {
    run_id: String,
    cny_limit: String,
    usd_limit: String,
    expected_revision: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PriceArgs {
    price: PriceVersion,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceLookup {
    pub price: Option<PriceVersion>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPeriod {
    pub run_id: String,
    pub items: Vec<ModelUsageSummary>,
    pub warnings: Vec<BudgetWarning>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MonthArgs {
    month: String,
    detected_time_zone: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ListRequestsArgs {
    run_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RouteArgs {
    provider_id: String,
    model_id: String,
    route_policy: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProfileRouteArgs {
    profile_id: String,
    provider_id: String,
    model_id: String,
    route_policy: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SelectPriceArgs {
    profile_id: String,
    provider_id: String,
    model_id: String,
    route_policy: String,
    currency: Currency,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PriceLookupArgs {
    provider_id: String,
    model_id: String,
    route_policy: String,
    currency: Currency,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthlyPeriod {
    pub month: String,
    pub time_zone: String,
    pub items: Vec<ModelUsageSummary>,
}

/// 使用首次持久化的系统 IANA 时区读取自然月报表。
#[tauri::command]
pub(crate) async fn budget_get_monthly(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<MonthArgs>,
) -> Result<MonthlyPeriod, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        let time_zone = storage.with_database(|connection| {
            BillingLedger::report_time_zone(connection, &args.detected_time_zone)
                .map_err(billing_fault)
        })?;
        let items = storage.with_database(|connection| {
            BillingLedger::month_summary(connection, &args.month, &time_zone).map_err(billing_fault)
        })?;
        Ok(MonthlyPeriod {
            month: args.month,
            time_zone,
            items,
        })
    })
    .await
}

/// 返回有界单局报表；费用按服务商与原币分组。
#[tauri::command]
pub(crate) async fn budget_get_period(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<RunArgs>,
) -> Result<RunPeriod, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        let items = storage.with_database(|connection| {
            BillingLedger::run_summary(connection, &args.run_id).map_err(billing_fault)
        })?;
        let warnings = storage.with_database(|connection| {
            BillingLedger::warnings(connection, &args.run_id).map_err(billing_fault)
        })?;
        Ok(RunPeriod {
            run_id: args.run_id,
            items,
            warnings,
        })
    })
    .await
}

/// 返回有界请求页；账本变更后该游标失效。
#[tauri::command]
pub(crate) async fn budget_list_requests(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<ListRequestsArgs>,
) -> Result<RequestPage, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        storage.with_database(|connection| {
            BillingLedger::list_requests(
                connection,
                &args.run_id,
                args.cursor.as_deref(),
                args.limit,
            )
            .map_err(|error| {
                if error == BillingError::StaleRevision {
                    Fault::new("app.bad-request", "费用账本已更新，请重新读取")
                        .with_detail(serde_json::json!({ "reason": "staleRevision" }))
                } else {
                    billing_fault(error)
                }
            })
        })
    })
    .await
}

/// 根据已登记价格分别计算 CNY 与 USD 建议额度。
#[tauri::command]
pub(crate) async fn budget_get_suggested_limits(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<RouteArgs>,
) -> Result<SuggestedLimits, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        storage.with_database(|connection| {
            BillingLedger::suggested_limits(
                connection,
                &args.provider_id,
                &args.model_id,
                &args.route_policy,
                &now(),
            )
            .map_err(billing_fault)
        })
    })
    .await
}

/// 读取当前服务商 profile 明确选择的计价币种。
#[tauri::command]
pub(crate) async fn budget_get_selected_price(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<ProfileRouteArgs>,
) -> Result<PriceLookup, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        let dispatch_at = now();
        let price = storage.with_database(|connection| {
            BillingLedger::profile_price(
                connection,
                &args.profile_id,
                &args.provider_id,
                &args.model_id,
                &args.route_policy,
                &dispatch_at,
            )
            .map_err(billing_fault)
        })?;
        Ok(PriceLookup { price })
    })
    .await
}

/// 为 profile 和路由选择已登记且匹配账户币种的价格。
#[tauri::command]
pub(crate) async fn budget_select_price(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<SelectPriceArgs>,
) -> Result<PriceVersion, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        storage.with_database(|connection| {
            BillingLedger::select_profile_price(
                connection,
                &args.profile_id,
                &args.provider_id,
                &args.model_id,
                &args.route_policy,
                args.currency,
                &now(),
            )
            .map_err(billing_fault)
        })
    })
    .await
}

/// 新周目没有设置时返回空值，不隐式创建预算。
#[tauri::command]
pub(crate) async fn budget_get_settings(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<RunArgs>,
) -> Result<Option<BudgetSettings>, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        match storage.with_database(|connection| {
            BillingLedger::get_settings(connection, &args.run_id).map_err(billing_fault)
        }) {
            Ok(settings) => Ok(Some(settings)),
            Err(error) if error.code == "budget.run-limit-missing" => Ok(None),
            Err(error) => Err(error),
        }
    })
    .await
}

/// 通过乐观修订校验同时设置两种原币上限。
#[tauri::command]
pub(crate) async fn budget_set_settings(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<SetSettingsArgs>,
) -> Result<BudgetSettings, CmdError> {
    let cny = NanoMoney::parse(&args.cny_limit).map_err(|_| bad_amount())?;
    let usd = NanoMoney::parse(&args.usd_limit).map_err(|_| bad_amount())?;
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        storage.with_database(|connection| {
            BillingLedger::set_settings(
                connection,
                &args.run_id,
                cny,
                usd,
                args.expected_revision,
                &now(),
            )
            .map_err(billing_fault)
        })
    })
    .await
}

/// 查询指定服务商路由当前生效的价格版本。
#[tauri::command]
pub(crate) async fn budget_get_price(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<PriceLookupArgs>,
) -> Result<PriceLookup, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        let dispatch_at = now();
        let price = storage.with_database(|connection| {
            BillingLedger::active_price_for_currency(
                connection,
                &args.provider_id,
                &args.model_id,
                &args.route_policy,
                &dispatch_at,
                args.currency,
            )
            .map_err(billing_fault)
        })?;
        Ok(PriceLookup { price })
    })
    .await
}

/// 登记调用方确认的不可变价格；修改价格必须使用新版本 ID。
#[tauri::command]
pub(crate) async fn budget_register_price(
    state: tauri::State<'_, StorageService>,
    BillingArgs(args): BillingArgs<PriceArgs>,
) -> Result<PriceVersion, CmdError> {
    args.price.validate().map_err(|_| bad_amount())?;
    state.ready()?;
    let storage = state.storage.clone();
    storage_job(move || {
        storage.with_database(|connection| {
            BillingLedger::register_price(connection, &args.price, &now()).map_err(billing_fault)
        })?;
        Ok(args.price)
    })
    .await
}

/// 解码 IPC 的完整对象，使 Tauri 参数拒绝走可达的统一错误路径。
pub(crate) struct BillingArgs<T>(T);

impl<'de, R: tauri::Runtime, T: serde::de::DeserializeOwned> tauri::ipc::CommandArg<'de, R>
    for BillingArgs<T>
{
    fn from_command(
        command: tauri::ipc::CommandItem<'de, R>,
    ) -> Result<Self, tauri::ipc::InvokeError> {
        crate::ipc::decode_body(command.message.payload(), "费用参数不合法")
            .map(Self)
            .map_err(Into::into)
    }
}

async fn storage_job<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, Fault> + Send + 'static,
) -> Result<T, CmdError> {
    aoidos_engine::blocking::run(job)
        .await
        .map_err(CmdError::from)
}

fn billing_fault(error: BillingError) -> Fault {
    match error {
        BillingError::Invalid => Fault::new("app.bad-request", "费用参数不合法"),
        BillingError::StaleRevision => Fault::new("app.bad-request", "费用账本已更新，请重新读取")
            .with_detail(serde_json::json!({ "reason": "staleRevision" })),
        BillingError::LimitMissing => Fault::new("budget.run-limit-missing", "本局尚未设置预算"),
        BillingError::Exceeded => Fault::new("budget.exceeded", "本局对应币种额度不足"),
        BillingError::PriceMissing => Fault::new("budget.price-missing", "模型价格尚未登记"),
        BillingError::PriceHistoryLimit => {
            Fault::new("budget.price-history-limit", "该模型价格版本已达到安全上限")
        }
        BillingError::NotFound => Fault::new("app.not-found", "费用信息不存在"),
        BillingError::Conflict => Fault::new("budget.conflict", "费用记录发生冲突"),
        BillingError::Storage => Fault::new("store.database", "费用存储不可用"),
    }
}

fn bad_amount() -> CmdError {
    CmdError::new("app.bad-request", "金额或价格参数不合法", None)
}

fn now() -> String {
    format_timestamp(time::OffsetDateTime::now_utc())
}

fn format_timestamp(timestamp: time::OffsetDateTime) -> String {
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}
