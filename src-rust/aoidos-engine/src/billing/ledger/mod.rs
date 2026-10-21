use aoidos_billing::{
    money::{Currency, NanoMoney, UnitPrice},
    price::PriceVersion,
    usage::NormalizedUsage,
};
use jiff::{ToSpan, civil::Date};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_PRICE_VERSIONS_PER_IDENTITY: i64 = 1_024;
const MAX_PENDING_SETTLEMENTS: i64 = 256;

struct StoredUsage {
    state: String,
    input_total: Option<i64>,
    input_cached: Option<i64>,
    input_cached_write: Option<i64>,
    output_total: Option<i64>,
    reasoning_included: Option<i64>,
}

/// 结算前持久化的供应商 Token 用量，不包含提示词、回复正文或凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSettlement {
    pub request_id: String,
    pub input_total: u64,
    pub input_cached: Option<u64>,
    pub input_cached_write: Option<u64>,
    pub output_total: u64,
    pub reasoning_included: Option<u64>,
    pub diagnostic_code: Option<String>,
}

/// 单个稳定周目的原币独立额度。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetSettings {
    pub run_id: String,
    pub cny_limit: NanoMoney,
    pub usd_limit: NanoMoney,
    pub revision: u64,
}

/// 单次网络尝试冻结的价格身份与保守预留金额。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub request_id: String,
    pub run_id: String,
    /// 经显式映射验证的旧账本范围；仍有数据时阻止新请求绕过迁移。
    pub legacy_scope_id: Option<String>,
    pub turn_id: String,
    pub profile_id: String,
    pub attempt: u32,
    pub provider_id: String,
    pub model_id: String,
    pub route_policy: String,
    pub price_version_id: String,
    pub currency: Currency,
    pub dispatch_at: String,
    pub amount: NanoMoney,
}

/// 已校验的完整用量和精确费用；缺失用量单独表示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settlement {
    pub input_total: u64,
    pub input_cached: u64,
    pub input_cached_write: u64,
    pub output_total: u64,
    pub reasoning_included: Option<u64>,
    pub total_tokens: u64,
    pub amount: NanoMoney,
    pub result_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementResult {
    Applied,
    AlreadyApplied,
    MarkedUnconfirmed,
}

/// 单局报表分组；币种保持分离，未知用量明确保留。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageSummary {
    pub provider_id: String,
    pub currency: Currency,
    pub model_id: String,
    pub settled_amount: Option<NanoMoney>,
    pub reserved_amount: Option<NanoMoney>,
    pub known_tokens: Option<u64>,
    pub incomplete_requests: u64,
    pub request_count: u64,
}

#[derive(Default)]
struct MonthlyAggregate {
    settled_nanos: i64,
    settled_count: u64,
    reserved_nanos: i64,
    reserved_count: u64,
    known_tokens: u64,
    known_count: u64,
    request_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetWarning {
    pub currency: Currency,
    pub threshold_percent: u8,
    pub reached_at: String,
    pub read: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestItem {
    pub request_id: String,
    pub profile_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub currency: Currency,
    pub price_version_id: String,
    pub dispatch_at: String,
    pub state: String,
    pub reserved_amount: NanoMoney,
    pub actual_amount: Option<NanoMoney>,
    pub input_total: Option<u64>,
    pub input_cached: Option<u64>,
    pub input_cached_write: Option<u64>,
    pub output_total: Option<u64>,
    pub reasoning_included: Option<u64>,
    pub total_tokens: Option<u64>,
    pub settlement_pending: bool,
    pub diagnostic_code: Option<String>,
    #[serde(skip)]
    row_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPage {
    pub revision: String,
    pub items: Vec<RequestItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// 可恢复旧版中已保存完整 Token 用量但尚未写入金额的单个历史请求。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillPreview {
    pub request_id: String,
    pub run_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub currency: Currency,
    pub price_version_id: String,
    pub estimated_amount: NanoMoney,
    pub ledger_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillResult {
    /// 用量已进入本地恢复队列，可继续执行结算恢复。
    Pending(PendingSettlement),
    /// 相同操作已完成结算，无需再次写入或恢复。
    AlreadySettled,
}

/// 根据所选模型的参考用量分别给出各币种的建议上限。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedLimits {
    pub cny: Option<NanoMoney>,
    pub usd: Option<NanoMoney>,
    pub reference_input_tokens: u64,
    pub reference_output_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingError {
    Invalid,
    LimitMissing,
    Exceeded,
    PriceMissing,
    PriceHistoryLimit,
    SettlementBacklog,
    NotFound,
    Conflict,
    ReconciliationRequired,
    StaleRevision,
    Storage,
}

impl fmt::Display for BillingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid billing input",
            Self::LimitMissing => "run currency limit is missing",
            Self::Exceeded => "run currency budget exceeded",
            Self::PriceMissing => "no price is registered for the active profile route",
            Self::PriceHistoryLimit => "price history reached its safety limit",
            Self::SettlementBacklog => "pending settlement recovery reached its safety limit",
            Self::NotFound => "billing request not found",
            Self::Conflict => "billing request conflicts with a previous operation",
            Self::ReconciliationRequired => "legacy run billing scope requires reconciliation",
            Self::StaleRevision => "billing settings revision is stale",
            Self::Storage => "billing storage operation failed",
        })
    }
}

impl std::error::Error for BillingError {}

#[cfg(test)]
mod tests;

/// 操作保持同步；共享数据库锁只在短事务期间持有。
pub struct BillingLedger;

#[derive(Debug, PartialEq, Eq)]
struct StoredPrice {
    provider_id: String,
    model_id: String,
    route_policy: String,
    currency: String,
    unit_tokens: i64,
    input_uncached: String,
    input_cached: String,
    input_cached_write: Option<String>,
    output: String,
    schedule_json: Option<String>,
    usage_mapping_version: String,
    source_kind: String,
    source_url: Option<String>,
    checked_at: String,
    effective_from: String,
    valid_until: Option<String>,
}

type ExistingReservation = (
    String,
    String,
    String,
    u32,
    String,
    String,
    String,
    String,
    String,
    i64,
);
type ExistingSettlement = (
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    String,
);

impl BillingLedger {
    /// 预览完整保存了用量且仍能验证原冻结价格的历史未决请求。
    /// 不会使用当前价格目录替换请求原先绑定的价格。
    ///
    /// # Errors
    ///
    /// 请求标识无效、冻结价格缺失、历史身份或用量不一致，以及数据库读取失败时返回错误。
    pub fn preview_backfill(
        connection: &Connection,
        request_id: &str,
    ) -> Result<Option<BackfillPreview>, BillingError> {
        validate_text(request_id, 128)?;
        let Some(row) = backfillable_usage(connection, request_id)? else {
            return Ok(None);
        };
        let price = Self::price_by_id(connection, &row.price_version_id)?
            .ok_or(BillingError::PriceMissing)?;
        validate_backfill_price(&row, &price)?;
        let amount = backfill_amount(&row, &price)?;
        let revision = read_ledger_revision(connection)?;
        Ok(Some(BackfillPreview {
            request_id: row.request_id,
            run_id: row.run_id,
            provider_id: row.provider_id,
            model_id: row.model_id,
            currency: row.currency,
            price_version_id: row.price_version_id,
            estimated_amount: amount,
            ledger_revision: revision,
        }))
    }

    /// 使用完整持久化用量和请求原先绑定的不可变价格，暂存带审计且幂等的历史补记。
    /// 调用方随后可复用本地结算恢复流程。
    ///
    /// # Errors
    ///
    /// 请求或操作标识无效、价格或历史数据不一致、账本版本过期、恢复队列已满，
    /// 以及数据库读写失败时返回错误。
    pub fn stage_backfill(
        connection: &mut Connection,
        request_id: &str,
        operation_id: &str,
        expected_revision: u64,
        updated_at: &str,
    ) -> Result<BackfillResult, BillingError> {
        validate_text(request_id, 128)?;
        validate_text(operation_id, 128)?;
        validate_text(updated_at, 64)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let Some(row) = backfillable_usage(&transaction, request_id)? else {
            // 同一操作可能已暂存恢复或完成结算，必须先检查幂等结果再判断队列容量。
            let existing: Option<(String, String)> = transaction
                .query_row(
                    "SELECT operation_kind,target_id FROM billing_audit WHERE operation_id=?1",
                    [operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            if existing
                .as_ref()
                .is_some_and(|(kind, target)| kind == "settlement-backfill" && target == request_id)
            {
                let pending = BillingLedger::pending_settlement(&transaction, request_id)?;
                if let Some(pending) = pending {
                    transaction.commit().map_err(storage)?;
                    return Ok(BackfillResult::Pending(pending));
                }
                let settled: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM billing_physical_requests WHERE request_id=?1 AND state='settled' AND actual_nanos IS NOT NULL)",
                        [request_id],
                        |row| row.get(0),
                    )
                    .map_err(storage)?;
                if settled {
                    transaction.commit().map_err(storage)?;
                    return Ok(BackfillResult::AlreadySettled);
                }
                return Err(BillingError::Conflict);
            }
            return Err(BillingError::Conflict);
        };
        let current_revision = read_ledger_revision(&transaction)?;
        if current_revision != expected_revision {
            return Err(BillingError::StaleRevision);
        }
        let operation: Option<(String, String)> = transaction
            .query_row(
                "SELECT operation_kind,target_id FROM billing_audit WHERE operation_id=?1",
                [operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage)?;
        if operation.is_some() {
            // 已暂存或结算的重放在前面的分支处理；此处已有审计却无恢复项说明状态冲突。
            return Err(BillingError::Conflict);
        }
        let price = Self::price_by_id(&transaction, &row.price_version_id)?
            .ok_or(BillingError::PriceMissing)?;
        validate_backfill_price(&row, &price)?;
        let amount = backfill_amount(&row, &price)?;
        let usage = row.usage.clone();
        let pending_count: i64 = transaction
            .query_row(
                "SELECT count(*) FROM billing_pending_settlements",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        // 历史补记只把已计入总上限的未决请求转入恢复队列，不会增加名额占用。
        if pending_count >= MAX_PENDING_SETTLEMENTS {
            return Err(BillingError::SettlementBacklog);
        }
        let digest = pending_usage_digest(&usage);
        let details = aoidos_json::to_string(&serde_json::json!({
            "currency": row.currency.code(),
            "priceVersionId": row.price_version_id,
            "usageDigest": digest,
            "estimatedAmount": amount.to_string(),
        }))
        .map_err(|_| BillingError::Invalid)?;
        transaction
            .execute(
                "INSERT INTO billing_pending_settlements(request_id,input_total,input_cached,input_cached_write,output_total,reasoning_included,result_digest,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",
                params![request_id,to_i64(usage.input_total)?,usage.input_cached.map(to_i64).transpose()?,usage.input_cached_write.map(to_i64).transpose()?,to_i64(usage.output_total)?,usage.reasoning_included.map(to_i64).transpose()?,digest,updated_at],
            )
            .map_err(storage)?;
        transaction
            .execute(
                "INSERT INTO billing_audit(operation_id,operation_kind,target_id,details_json,created_at) VALUES(?1,'settlement-backfill',?2,?3,?4)",
                params![operation_id,request_id,details,updated_at],
            )
            .map_err(storage)?;
        bump_revision(&transaction)?;
        transaction.commit().map_err(storage)?;
        Ok(BackfillResult::Pending(usage))
    }

    /// 查询请求所属的周目，用于 IPC 权限校验，不读取或返回请求正文。
    ///
    /// # Errors
    ///
    /// 请求标识无效或 SQLite 查询失败时返回错误；请求不存在时返回 `None`。
    pub fn request_run_id(
        connection: &Connection,
        request_id: &str,
    ) -> Result<Option<String>, BillingError> {
        validate_text(request_id, 128)?;
        connection
            .query_row(
                "SELECT run_id FROM billing_physical_requests WHERE request_id=?1",
                [request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)
    }

    /// 检查账本范围是否存在预算、物理请求或本局预算修改审计。
    ///
    /// # Errors
    /// 范围标识无效或 SQLite 查询失败时返回错误。
    pub fn has_run_scope_data(connection: &Connection, run_id: &str) -> Result<bool, BillingError> {
        validate_text(run_id, 128)?;
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM billing_run_budgets WHERE run_id=?1) OR EXISTS(SELECT 1 FROM billing_physical_requests WHERE run_id=?1) OR EXISTS(SELECT 1 FROM billing_audit WHERE target_id=?1 AND operation_kind='run-budget-update')",
                [run_id],
                |row| row.get(0),
            )
            .map_err(storage)
    }

    /// 在计价或修改账本前暂存完整的供应商 Token 数值。
    ///
    /// 暂存记录不含正文、凭据或供应商原始响应，且全局最多保留 256 条。
    ///
    /// # Errors
    ///
    /// 输入无效、请求不存在或状态冲突，以及 SQLite 写入 / 上限检查失败时返回错误。
    pub fn stage_settlement(
        connection: &mut Connection,
        pending: &PendingSettlement,
        updated_at: &str,
    ) -> Result<(), BillingError> {
        validate_text(&pending.request_id, 128)?;
        let input_total = to_i64(pending.input_total)?;
        let input_cached = pending.input_cached.map(to_i64).transpose()?;
        let input_cached_write = pending.input_cached_write.map(to_i64).transpose()?;
        let output_total = to_i64(pending.output_total)?;
        let reasoning = pending.reasoning_included.map(to_i64).transpose()?;
        let digest = pending_usage_digest(pending);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let current: Option<StoredUsage> = transaction
            .query_row(
                "SELECT state,input_total,input_cached,input_cached_write,output_total,reasoning_included FROM billing_physical_requests WHERE request_id=?1",
                [&pending.request_id],
                |row| Ok(StoredUsage {
                    state: row.get(0)?,
                    input_total: row.get(1)?,
                    input_cached: row.get(2)?,
                    input_cached_write: row.get(3)?,
                    output_total: row.get(4)?,
                    reasoning_included: row.get(5)?,
                }),
            )
            .optional()
            .map_err(storage)?;
        let Some(current) = current else {
            return Err(BillingError::NotFound);
        };
        if current.state == "settled" {
            let matches = current.input_total == Some(input_total)
                && current.input_cached == Some(input_cached.unwrap_or(0))
                && current.input_cached_write == Some(input_cached_write.unwrap_or(0))
                && current.output_total == Some(output_total)
                && current.reasoning_included == reasoning;
            if matches {
                transaction.commit().map_err(storage)?;
                return Ok(());
            }
            let changed = transaction
                .execute(
                    "UPDATE billing_physical_requests SET diagnostic_code='budget.conflict',updated_at=?2 WHERE request_id=?1 AND diagnostic_code IS NOT 'budget.conflict'",
                    params![pending.request_id, updated_at],
                )
                .map_err(storage)?;
            if changed == 1 {
                bump_revision(&transaction)?;
            }
            transaction.commit().map_err(storage)?;
            return Err(BillingError::Conflict);
        }
        if current.state != "reserved" && current.state != "unconfirmed" {
            return Err(BillingError::Conflict);
        }
        let existing: Option<String> = transaction
            .query_row(
                "SELECT result_digest FROM billing_pending_settlements WHERE request_id=?1",
                [&pending.request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if let Some(existing) = existing {
            if existing == digest {
                transaction.commit().map_err(storage)?;
                return Ok(());
            }
            transaction
                .execute(
                    "UPDATE billing_pending_settlements SET diagnostic_code='budget.conflict',updated_at=?2 WHERE request_id=?1",
                    params![pending.request_id, updated_at],
                )
                .map_err(storage)?;
            transaction
                .execute(
                    "UPDATE billing_physical_requests SET diagnostic_code='budget.conflict',updated_at=?2 WHERE request_id=?1 AND diagnostic_code IS NOT 'budget.conflict'",
                    params![pending.request_id, updated_at],
                )
                .map_err(storage)?;
            bump_revision(&transaction)?;
            transaction.commit().map_err(storage)?;
            return Err(BillingError::Conflict);
        }
        let pending_count: i64 = transaction
            .query_row(
                "SELECT count(*) FROM billing_pending_settlements",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if pending_count >= MAX_PENDING_SETTLEMENTS {
            return Err(BillingError::SettlementBacklog);
        }
        transaction
            .execute(
                "INSERT INTO billing_pending_settlements(request_id,input_total,input_cached,input_cached_write,output_total,reasoning_included,result_digest,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",
                params![pending.request_id,input_total,input_cached,input_cached_write,output_total,reasoning,digest,updated_at],
            )
            .map_err(storage)?;
        bump_revision(&transaction)?;
        transaction.commit().map_err(storage)
    }

    /// 读取一条暂存用量，供显式、幂等的本地恢复使用。
    ///
    /// # Errors
    ///
    /// 请求标识无效或 SQLite 读取失败时返回错误。
    pub fn pending_settlement(
        connection: &Connection,
        request_id: &str,
    ) -> Result<Option<PendingSettlement>, BillingError> {
        validate_text(request_id, 128)?;
        connection
            .query_row(
                "SELECT input_total,input_cached,input_cached_write,output_total,reasoning_included,diagnostic_code FROM billing_pending_settlements WHERE request_id=?1",
                [request_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(storage)?
            .map(|(input, cached, cached_write, output, reasoning, diagnostic_code)| {
                Ok(PendingSettlement {
                    request_id: request_id.to_owned(),
                    input_total: u64::try_from(input).map_err(|_| BillingError::Storage)?,
                    input_cached: cached
                        .map(u64::try_from)
                        .transpose()
                        .map_err(|_| BillingError::Storage)?,
                    input_cached_write: cached_write
                        .map(u64::try_from)
                        .transpose()
                        .map_err(|_| BillingError::Storage)?,
                    output_total: u64::try_from(output).map_err(|_| BillingError::Storage)?,
                    reasoning_included: reasoning
                        .map(u64::try_from)
                        .transpose()
                        .map_err(|_| BillingError::Storage)?,
                    diagnostic_code,
                })
            })
            .transpose()
    }

    /// 只保存稳定诊断码，不持久化 SQL、完整用量载荷或供应商错误文本。
    ///
    /// # Errors
    ///
    /// 请求标识或诊断码无效，或 SQLite 更新失败时返回错误。
    pub fn diagnose_pending_settlement(
        connection: &Connection,
        request_id: &str,
        code: &str,
        updated_at: &str,
    ) -> Result<(), BillingError> {
        validate_text(request_id, 128)?;
        if !matches!(
            code,
            "budget.invalid-usage" | "budget.price-missing" | "store.database" | "budget.conflict"
        ) {
            return Err(BillingError::Invalid);
        }
        let transaction = connection.unchecked_transaction().map_err(storage)?;
        let changed = transaction
            .execute(
                "UPDATE billing_pending_settlements SET diagnostic_code=?2,updated_at=?3 WHERE request_id=?1 AND diagnostic_code IS NOT ?2",
                params![request_id, code, updated_at],
            )
            .map_err(storage)?;
        if changed > 0 {
            bump_revision(&transaction)?;
        }
        transaction.commit().map_err(storage)
    }

    /// 删除已解决的 outbox 记录；可见状态变化时使请求页游标失效。
    ///
    /// # Errors
    ///
    /// SQLite 删除或提交失败时返回错误。
    pub fn clear_pending_settlement(
        connection: &Connection,
        request_id: &str,
    ) -> Result<(), BillingError> {
        let transaction = connection.unchecked_transaction().map_err(storage)?;
        let changed = transaction
            .execute(
                "DELETE FROM billing_pending_settlements WHERE request_id=?1",
                [request_id],
            )
            .map_err(storage)?;
        if changed > 0 {
            bump_revision(&transaction)?;
        }
        transaction.commit().map_err(storage)
    }

    /// Lists a frozen, bounded request snapshot; any ledger mutation invalidates its cursor.
    pub fn list_requests(
        connection: &Connection,
        run_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<RequestPage, BillingError> {
        validate_text(run_id, 128)?;
        let limit = limit.unwrap_or(50);
        if limit == 0 || limit > 200 {
            return Err(BillingError::Invalid);
        }
        let revision: i64 = connection
            .query_row("SELECT revision FROM billing_meta WHERE id=1", [], |row| {
                row.get(0)
            })
            .map_err(storage)?;
        let (max_row_id, after_row_id) = if let Some(cursor) = cursor {
            let parts = parse_cursor(cursor)?;
            if parts.0 != revision {
                return Err(BillingError::StaleRevision);
            }
            (parts.1, parts.2)
        } else {
            let max: Option<i64> = connection
                .query_row(
                    "SELECT MAX(rowid) FROM billing_physical_requests WHERE run_id=?1",
                    [run_id],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let Some(max) = max else {
                return Ok(RequestPage {
                    revision: revision.to_string(),
                    items: Vec::new(),
                    next_cursor: None,
                });
            };
            (max, 0)
        };
        let mut statement = connection
            .prepare("SELECT r.rowid,r.request_id,r.profile_id,r.provider_id,r.model_id,r.currency,r.price_version_id,r.dispatch_at,r.state,r.reserved_nanos,r.actual_nanos,r.input_total,r.input_cached,r.input_cached_write,r.output_total,r.reasoning_included,r.total_tokens,p.request_id IS NOT NULL,COALESCE(p.diagnostic_code,r.diagnostic_code) FROM billing_physical_requests r LEFT JOIN billing_pending_settlements p ON p.request_id=r.request_id WHERE r.run_id=?1 AND r.rowid>?2 AND r.rowid<=?3 ORDER BY r.rowid ASC LIMIT ?4")
            .map_err(storage)?;
        let rows = statement
            .query_map(
                params![run_id, after_row_id, max_row_id, i64::from(limit) + 1],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                        row.get::<_, Option<i64>>(11)?,
                        row.get::<_, Option<i64>>(12)?,
                        row.get::<_, Option<i64>>(13)?,
                        row.get::<_, Option<i64>>(14)?,
                        row.get::<_, Option<i64>>(15)?,
                        row.get::<_, Option<i64>>(16)?,
                        row.get::<_, bool>(17)?,
                        row.get::<_, Option<String>>(18)?,
                    ))
                },
            )
            .map_err(storage)?;
        let mut raw = Vec::new();
        for row in rows {
            raw.push(row.map_err(storage)?);
        }
        let has_more = raw.len() > limit as usize;
        raw.truncate(limit as usize);
        let mut items = Vec::with_capacity(raw.len());
        for row in raw {
            let (
                row_id,
                request_id,
                profile_id,
                provider_id,
                model_id,
                currency,
                price_version_id,
                dispatch_at,
                state,
                reserved,
                actual,
                input,
                cached,
                cached_write,
                output,
                reasoning,
                total,
                settlement_pending,
                diagnostic_code,
            ) = row;
            items.push(RequestItem {
                request_id,
                profile_id,
                provider_id,
                model_id,
                currency: currency.parse().map_err(|_| BillingError::Storage)?,
                price_version_id,
                dispatch_at,
                state,
                reserved_amount: NanoMoney::from_nanos(reserved)
                    .map_err(|_| BillingError::Storage)?,
                actual_amount: actual
                    .map(NanoMoney::from_nanos)
                    .transpose()
                    .map_err(|_| BillingError::Storage)?,
                input_total: input
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| BillingError::Storage)?,
                input_cached: cached
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| BillingError::Storage)?,
                input_cached_write: cached_write
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| BillingError::Storage)?,
                output_total: output
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| BillingError::Storage)?,
                reasoning_included: reasoning
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| BillingError::Storage)?,
                total_tokens: total
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| BillingError::Storage)?,
                settlement_pending,
                diagnostic_code,
                row_id,
            });
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|item| make_cursor(revision, max_row_id, item.row_id))
        } else {
            None
        };
        Ok(RequestPage {
            revision: revision.to_string(),
            items,
            next_cursor,
        })
    }

    /// Persists the first detected device time zone and keeps later reports stable.
    pub fn report_time_zone(
        connection: &mut Connection,
        detected_time_zone: &str,
    ) -> Result<String, BillingError> {
        let requested = if month_range("2026-01", detected_time_zone).is_ok() {
            detected_time_zone
        } else {
            "UTC"
        };
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let stored: Option<String> = transaction
            .query_row("SELECT time_zone FROM billing_meta WHERE id=1", [], |row| {
                row.get(0)
            })
            .map_err(storage)?;
        if let Some(stored) = stored {
            transaction.commit().map_err(storage)?;
            return Ok(stored);
        }
        transaction
            .execute(
                "UPDATE billing_meta SET time_zone=?1 WHERE id=1 AND time_zone IS NULL",
                [requested],
            )
            .map_err(storage)?;
        bump_revision(&transaction)?;
        transaction.commit().map_err(storage)?;
        Ok(requested.to_owned())
    }

    /// Monthly report grouped by provider, original currency and model in a frozen IANA zone.
    pub fn month_summary(
        connection: &Connection,
        month: &str,
        time_zone: &str,
    ) -> Result<Vec<ModelUsageSummary>, BillingError> {
        let (start, end) = month_range(month, time_zone)?;
        let start = OffsetDateTime::parse(&start, &Rfc3339).map_err(|_| BillingError::Invalid)?;
        let end = OffsetDateTime::parse(&end, &Rfc3339).map_err(|_| BillingError::Invalid)?;
        let candidate_start = start.format(&Rfc3339).map_err(|_| BillingError::Invalid)?;
        let candidate_end = end.format(&Rfc3339).map_err(|_| BillingError::Invalid)?;
        let mut statement = connection
            .prepare(
                "SELECT dispatch_at,provider_id,currency,model_id,state,actual_nanos,reserved_nanos,total_tokens FROM billing_physical_requests WHERE julianday(dispatch_at)>=julianday(?1)-2 AND julianday(dispatch_at)<julianday(?2)+2 ORDER BY provider_id,currency,model_id",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(params![candidate_start, candidate_end], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                ))
            })
            .map_err(storage)?;
        let mut groups = BTreeMap::<(String, String, String), MonthlyAggregate>::new();
        for row in rows {
            let (dispatch_at, provider_id, currency, model_id, state, settled, reserved, tokens) =
                row.map_err(storage)?;
            let dispatch_at =
                OffsetDateTime::parse(&dispatch_at, &Rfc3339).map_err(|_| BillingError::Storage)?;
            if dispatch_at < start || dispatch_at >= end {
                continue;
            }
            let key = (provider_id, currency, model_id);
            if !groups.contains_key(&key) && groups.len() == 200 {
                return Err(BillingError::Invalid);
            }
            let group = groups.entry(key).or_default();
            group.request_count = group
                .request_count
                .checked_add(1)
                .ok_or(BillingError::Storage)?;
            if let Some(tokens) = tokens {
                group.known_tokens = group
                    .known_tokens
                    .checked_add(u64::try_from(tokens).map_err(|_| BillingError::Storage)?)
                    .ok_or(BillingError::Storage)?;
                group.known_count = group
                    .known_count
                    .checked_add(1)
                    .ok_or(BillingError::Storage)?;
            }
            if state == "settled" {
                group.settled_nanos = group
                    .settled_nanos
                    .checked_add(settled.ok_or(BillingError::Storage)?)
                    .ok_or(BillingError::Storage)?;
                group.settled_count = group
                    .settled_count
                    .checked_add(1)
                    .ok_or(BillingError::Storage)?;
            } else {
                group.reserved_nanos = group
                    .reserved_nanos
                    .checked_add(reserved)
                    .ok_or(BillingError::Storage)?;
                group.reserved_count = group
                    .reserved_count
                    .checked_add(1)
                    .ok_or(BillingError::Storage)?;
            }
        }
        groups
            .into_iter()
            .map(|((provider_id, currency, model_id), group)| {
                Ok(ModelUsageSummary {
                    provider_id,
                    currency: currency.parse().map_err(|_| BillingError::Storage)?,
                    model_id,
                    settled_amount: if group.settled_count == 0 {
                        None
                    } else {
                        Some(
                            NanoMoney::from_nanos(group.settled_nanos)
                                .map_err(|_| BillingError::Storage)?,
                        )
                    },
                    reserved_amount: if group.reserved_count == 0 {
                        None
                    } else {
                        Some(
                            NanoMoney::from_nanos(group.reserved_nanos)
                                .map_err(|_| BillingError::Storage)?,
                        )
                    },
                    known_tokens: if group.known_count == 0 {
                        None
                    } else {
                        Some(group.known_tokens)
                    },
                    incomplete_requests: group
                        .request_count
                        .checked_sub(group.known_count)
                        .ok_or(BillingError::Storage)?,
                    request_count: group.request_count,
                })
            })
            .collect()
    }

    /// Reads this run's deduplicated per-currency budget warnings.
    pub fn warnings(
        connection: &Connection,
        run_id: &str,
    ) -> Result<Vec<BudgetWarning>, BillingError> {
        validate_text(run_id, 128)?;
        let mut statement = connection
            .prepare("SELECT currency,threshold_percent,reached_at,read_at FROM billing_budget_warnings WHERE run_id=?1 ORDER BY currency LIMIT 2")
            .map_err(storage)?;
        let rows = statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u8>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(storage)?;
        rows.map(|row| {
            let (currency, threshold_percent, reached_at, read_at) = row.map_err(storage)?;
            Ok(BudgetWarning {
                currency: currency.parse().map_err(|_| BillingError::Storage)?,
                threshold_percent,
                reached_at,
                read: read_at.is_some(),
            })
        })
        .collect()
    }

    /// Returns a bounded run report grouped by provider, original currency and model.
    pub fn run_summary(
        connection: &Connection,
        run_id: &str,
    ) -> Result<Vec<ModelUsageSummary>, BillingError> {
        validate_text(run_id, 128)?;
        let mut statement = connection
            .prepare(
                "SELECT provider_id,currency,model_id,SUM(CASE WHEN state='settled' THEN actual_nanos ELSE 0 END),SUM(CASE WHEN state='settled' THEN 1 ELSE 0 END),SUM(CASE WHEN state<>'settled' THEN reserved_nanos ELSE 0 END),SUM(CASE WHEN state<>'settled' THEN 1 ELSE 0 END),SUM(total_tokens),COUNT(total_tokens),COUNT(*) FROM billing_physical_requests WHERE run_id=?1 GROUP BY provider_id,currency,model_id ORDER BY provider_id,currency,model_id LIMIT 201",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })
            .map_err(storage)?;
        let mut result = Vec::new();
        for row in rows {
            let (
                provider_id,
                currency,
                model_id,
                settled,
                settled_count,
                reserved,
                reserved_count,
                tokens,
                known_count,
                request_count,
            ) = row.map_err(storage)?;
            if result.len() == 200 {
                return Err(BillingError::Invalid);
            }
            let currency = currency.parse().map_err(|_| BillingError::Storage)?;
            result.push(ModelUsageSummary {
                provider_id,
                currency,
                model_id,
                settled_amount: if settled_count == 0 {
                    None
                } else {
                    Some(
                        NanoMoney::from_nanos(settled.ok_or(BillingError::Storage)?)
                            .map_err(|_| BillingError::Storage)?,
                    )
                },
                reserved_amount: if reserved_count == 0 {
                    None
                } else {
                    Some(
                        NanoMoney::from_nanos(reserved.ok_or(BillingError::Storage)?)
                            .map_err(|_| BillingError::Storage)?,
                    )
                },
                known_tokens: if known_count == 0 {
                    None
                } else {
                    Some(
                        u64::try_from(tokens.ok_or(BillingError::Storage)?)
                            .map_err(|_| BillingError::Storage)?,
                    )
                },
                incomplete_requests: u64::try_from(request_count - known_count)
                    .map_err(|_| BillingError::Storage)?,
                request_count: u64::try_from(request_count).map_err(|_| BillingError::Storage)?,
            });
        }
        Ok(result)
    }

    /// Reads a run's two currency limits and revision without creating a budget implicitly.
    pub fn get_settings(
        connection: &Connection,
        run_id: &str,
    ) -> Result<BudgetSettings, BillingError> {
        validate_text(run_id, 128)?;
        read_settings(connection, run_id)
    }

    /// Resolves the version frozen by a reserved physical request.
    pub fn request_price(
        connection: &Connection,
        request_id: &str,
    ) -> Result<PriceVersion, BillingError> {
        let version_id: Option<String> = connection
            .query_row(
                "SELECT price_version_id FROM billing_physical_requests WHERE request_id=?1",
                [request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let version_id = version_id.ok_or(BillingError::NotFound)?;
        Self::price_by_id(connection, &version_id)?.ok_or(BillingError::NotFound)
    }

    /// Reads the immutable price and send time pinned to a physical request.
    pub fn request_price_context(
        connection: &Connection,
        request_id: &str,
    ) -> Result<(PriceVersion, String), BillingError> {
        validate_text(request_id, 128)?;
        let (version_id, dispatch_at) = connection
            .query_row(
                "SELECT price_version_id,dispatch_at FROM billing_physical_requests WHERE request_id=?1",
                [request_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(storage)?
            .ok_or(BillingError::NotFound)?;
        let price = Self::price_by_id(connection, &version_id)?.ok_or(BillingError::Storage)?;
        Ok((price, dispatch_at))
    }

    /// Reads a frozen immutable price by ID for settlement and audit.
    pub fn price_by_id(
        connection: &Connection,
        version_id: &str,
    ) -> Result<Option<PriceVersion>, BillingError> {
        let stored = connection
            .query_row(
                "SELECT provider_id,model_id,route_policy,currency,unit_tokens,input_uncached_price,input_cached_price,input_cached_write_price,output_price,schedule_json,usage_mapping_version,source_kind,source_url,checked_at,effective_from,valid_until FROM billing_price_versions WHERE version_id=?1",
                [version_id],
                |row| Ok((
                    row.get::<_, String>(0)?,row.get::<_, String>(1)?,row.get::<_, String>(2)?,row.get::<_, String>(3)?,row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,row.get::<_, String>(6)?,row.get::<_, Option<String>>(7)?,row.get::<_, String>(8)?,row.get::<_, Option<String>>(9)?,row.get::<_, String>(10)?,row.get::<_, String>(11)?,
                    row.get::<_, Option<String>>(12)?,row.get::<_, String>(13)?,row.get::<_, String>(14)?,row.get::<_, Option<String>>(15)?,
                )),
            )
            .optional()
            .map_err(storage)?;
        let Some((
            provider_id,
            model_id,
            route_policy,
            currency,
            unit_tokens,
            input_uncached,
            input_cached,
            input_cached_write,
            output,
            schedule_json,
            usage_mapping_version,
            source_kind,
            source_url,
            checked_at,
            effective_from,
            valid_until,
        )) = stored
        else {
            return Ok(None);
        };
        let value = PriceVersion {
            version_id: version_id.to_owned(),
            provider_id,
            model_id,
            route_policy,
            currency: currency.parse().map_err(|_| BillingError::Storage)?,
            unit_tokens: u64::try_from(unit_tokens).map_err(|_| BillingError::Storage)?,
            input_uncached: UnitPrice::parse(&input_uncached).map_err(|_| BillingError::Storage)?,
            input_cached: UnitPrice::parse(&input_cached).map_err(|_| BillingError::Storage)?,
            input_cached_write: input_cached_write
                .map(|value| UnitPrice::parse(&value))
                .transpose()
                .map_err(|_| BillingError::Storage)?,
            output: UnitPrice::parse(&output).map_err(|_| BillingError::Storage)?,
            schedule: schedule_json
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|_| BillingError::Storage)?,
            usage_mapping_version,
            source_kind,
            source_url,
            checked_at,
            effective_from,
            valid_until,
        };
        value.validate().map_err(|_| BillingError::Storage)?;
        Ok(Some(value))
    }

    /// Calculates independent CNY/USD suggestions from this route's active price versions.
    pub fn suggested_limits(
        connection: &Connection,
        provider_id: &str,
        model_id: &str,
        route_policy: &str,
        dispatch_at: &str,
    ) -> Result<SuggestedLimits, BillingError> {
        const REFERENCE_INPUT: u64 = 2_000_000;
        const REFERENCE_OUTPUT: u64 = 500_000;
        let mut result = SuggestedLimits {
            cny: None,
            usd: None,
            reference_input_tokens: REFERENCE_INPUT,
            reference_output_tokens: REFERENCE_OUTPUT,
        };
        for currency in [Currency::Cny, Currency::Usd] {
            if let Some(price) = Self::active_price_for_currency(
                connection,
                provider_id,
                model_id,
                route_policy,
                dispatch_at,
                currency,
            )? {
                let amount = price
                    .reference_budget(REFERENCE_INPUT, REFERENCE_OUTPUT)
                    .map_err(|_| BillingError::Invalid)?;
                match currency {
                    Currency::Cny => result.cny = Some(amount),
                    Currency::Usd => result.usd = Some(amount),
                }
            }
        }
        Ok(result)
    }

    /// Reads the newest active immutable price for one explicitly selected account currency.
    pub fn active_price_for_currency(
        connection: &Connection,
        provider_id: &str,
        model_id: &str,
        route_policy: &str,
        dispatch_at: &str,
        currency: Currency,
    ) -> Result<Option<PriceVersion>, BillingError> {
        // SQLite 日期函数会丢失亚毫秒精度，因此在 Rust 中按 RFC 3339 实际时刻比较价格版本。
        let mut statement = connection.prepare(
            "SELECT version_id,unit_tokens,input_uncached_price,input_cached_price,input_cached_write_price,output_price,schedule_json,usage_mapping_version,source_kind,source_url,checked_at,effective_from,valid_until FROM billing_price_versions WHERE provider_id=?1 AND model_id=?2 AND route_policy=?3 AND currency=?4 ORDER BY created_at DESC,version_id DESC LIMIT ?5",
        ).map_err(storage)?;
        let mut rows = statement
            .query(params![
                provider_id,
                model_id,
                route_policy,
                currency.code(),
                MAX_PRICE_VERSIONS_PER_IDENTITY + 1
            ])
            .map_err(storage)?;
        let mut newest: Option<(OffsetDateTime, PriceVersion)> = None;
        let mut examined = 0_i64;
        while let Some(row) = rows.next().map_err(storage)? {
            examined += 1;
            if examined > MAX_PRICE_VERSIONS_PER_IDENTITY {
                return Err(BillingError::PriceHistoryLimit);
            }
            let price = PriceVersion {
                version_id: row.get(0).map_err(storage)?,
                provider_id: provider_id.to_owned(),
                model_id: model_id.to_owned(),
                route_policy: route_policy.to_owned(),
                currency,
                unit_tokens: u64::try_from(row.get::<_, i64>(1).map_err(storage)?)
                    .map_err(|_| BillingError::Storage)?,
                input_uncached: UnitPrice::parse(&row.get::<_, String>(2).map_err(storage)?)
                    .map_err(|_| BillingError::Storage)?,
                input_cached: UnitPrice::parse(&row.get::<_, String>(3).map_err(storage)?)
                    .map_err(|_| BillingError::Storage)?,
                input_cached_write: row
                    .get::<_, Option<String>>(4)
                    .map_err(storage)?
                    .map(|value| UnitPrice::parse(&value))
                    .transpose()
                    .map_err(|_| BillingError::Storage)?,
                output: UnitPrice::parse(&row.get::<_, String>(5).map_err(storage)?)
                    .map_err(|_| BillingError::Storage)?,
                schedule: row
                    .get::<_, Option<String>>(6)
                    .map_err(storage)?
                    .map(|value| serde_json::from_str(&value))
                    .transpose()
                    .map_err(|_| BillingError::Storage)?,
                usage_mapping_version: row.get(7).map_err(storage)?,
                source_kind: row.get(8).map_err(storage)?,
                source_url: row.get(9).map_err(storage)?,
                checked_at: row.get(10).map_err(storage)?,
                effective_from: row.get(11).map_err(storage)?,
                valid_until: row.get(12).map_err(storage)?,
            };
            if !price
                .is_effective_at(dispatch_at)
                .map_err(|_| BillingError::Storage)?
            {
                continue;
            }
            let effective_from = OffsetDateTime::parse(&price.effective_from, &Rfc3339)
                .map_err(|_| BillingError::Storage)?;
            if newest
                .as_ref()
                .is_none_or(|(selected_at, _)| effective_from > *selected_at)
            {
                newest = Some((effective_from, price));
            }
        }
        Ok(newest.map(|(_, price)| price))
    }

    /// Pins the selected provider billing currency and price to the active profile.
    pub fn select_profile_price(
        connection: &mut Connection,
        profile_id: &str,
        provider_id: &str,
        model_id: &str,
        route_policy: &str,
        currency: Currency,
        selected_at: &str,
    ) -> Result<PriceVersion, BillingError> {
        validate_text(profile_id, 128)?;
        validate_text(selected_at, 64)?;
        let price = Self::active_price_for_currency(
            connection,
            provider_id,
            model_id,
            route_policy,
            selected_at,
            currency,
        )?
        .ok_or(BillingError::NotFound)?;
        Self::pin_profile_price(connection, profile_id, price, selected_at)
    }

    /// Pins a specific immutable price version to a profile without resolving by currency.
    pub fn select_profile_price_version(
        connection: &mut Connection,
        profile_id: &str,
        version_id: &str,
        selected_at: &str,
    ) -> Result<PriceVersion, BillingError> {
        validate_text(profile_id, 128)?;
        validate_text(version_id, 128)?;
        validate_text(selected_at, 64)?;
        let price = Self::price_by_id(connection, version_id)?.ok_or(BillingError::NotFound)?;
        if !price
            .is_effective_at(selected_at)
            .map_err(|_| BillingError::Invalid)?
        {
            return Err(BillingError::NotFound);
        }
        Self::pin_profile_price(connection, profile_id, price, selected_at)
    }

    fn pin_profile_price(
        connection: &mut Connection,
        profile_id: &str,
        price: PriceVersion,
        selected_at: &str,
    ) -> Result<PriceVersion, BillingError> {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT price_version_id FROM billing_active_profile_prices WHERE profile_id=?1",
                [profile_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if existing.as_deref() == Some(price.version_id.as_str()) {
            transaction.commit().map_err(storage)?;
            return Ok(price);
        }
        transaction
            .execute(
                "INSERT INTO billing_active_profile_prices(profile_id,provider_id,model_id,route_policy,currency,price_version_id,selected_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(profile_id) DO UPDATE SET provider_id=excluded.provider_id,model_id=excluded.model_id,route_policy=excluded.route_policy,currency=excluded.currency,price_version_id=excluded.price_version_id,selected_at=excluded.selected_at",
                params![profile_id,price.provider_id,price.model_id,price.route_policy,price.currency.code(),price.version_id,selected_at],
            )
            .map_err(storage)?;
        let operation_id = format!("profile-price:{}:{}", profile_id, price.version_id);
        let details = serde_json::json!({
            "profileId": profile_id,
            "priceVersionId": price.version_id,
            "currency": price.currency.code(),
        });
        transaction
            .execute(
                "INSERT OR IGNORE INTO billing_audit(operation_id,operation_kind,target_id,details_json,created_at) VALUES(?1,'profile-price-select',?2,?3,?4)",
                params![operation_id,profile_id,details.to_string(),selected_at],
            )
            .map_err(storage)?;
        bump_revision(&transaction)?;
        transaction.commit().map_err(storage)?;
        Ok(price)
    }

    /// Resolves the price explicitly selected for one provider profile.
    pub fn profile_price(
        connection: &Connection,
        profile_id: &str,
        provider_id: &str,
        model_id: &str,
        route_policy: &str,
        dispatch_at: &str,
    ) -> Result<Option<PriceVersion>, BillingError> {
        validate_text(profile_id, 128)?;
        let version_id: Option<String> = connection
            .query_row(
                "SELECT price_version_id FROM billing_active_profile_prices WHERE profile_id=?1 AND provider_id=?2 AND model_id=?3 AND route_policy=?4",
                params![profile_id,provider_id,model_id,route_policy],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let Some(version_id) = version_id else {
            return Ok(None);
        };
        let price = Self::price_by_id(connection, &version_id)?.ok_or(BillingError::Storage)?;
        if !price
            .is_effective_at(dispatch_at)
            .map_err(|_| BillingError::Storage)?
        {
            return Ok(None);
        }
        Ok(Some(price))
    }

    /// Registers an immutable price version. Reusing an ID with different values is a conflict.
    pub fn register_price(
        connection: &mut Connection,
        price: &PriceVersion,
        created_at: &str,
    ) -> Result<(), BillingError> {
        price.validate().map_err(|_| BillingError::Invalid)?;
        validate_text(created_at, 64)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let existing: Option<StoredPrice> = transaction.query_row(
            "SELECT provider_id,model_id,route_policy,currency,unit_tokens,input_uncached_price,input_cached_price,input_cached_write_price,output_price,schedule_json,usage_mapping_version,source_kind,source_url,checked_at,effective_from,valid_until FROM billing_price_versions WHERE version_id=?1",
            [&price.version_id],
            |row| Ok(StoredPrice {
                provider_id: row.get(0)?, model_id: row.get(1)?, route_policy: row.get(2)?,
                currency: row.get(3)?, unit_tokens: row.get(4)?, input_uncached: row.get(5)?,
                input_cached: row.get(6)?, input_cached_write: row.get(7)?, output: row.get(8)?, schedule_json: row.get(9)?, usage_mapping_version: row.get(10)?,
                source_kind: row.get(11)?, source_url: row.get(12)?, checked_at: row.get(13)?,
                effective_from: row.get(14)?, valid_until: row.get(15)?,
            }),
        ).optional().map_err(storage)?;
        let expected = StoredPrice {
            provider_id: price.provider_id.clone(),
            model_id: price.model_id.clone(),
            route_policy: price.route_policy.clone(),
            currency: price.currency.code().to_owned(),
            unit_tokens: i64::try_from(price.unit_tokens).map_err(|_| BillingError::Invalid)?,
            input_uncached: price.input_uncached.to_decimal_string(),
            input_cached: price.input_cached.to_decimal_string(),
            input_cached_write: price
                .input_cached_write
                .as_ref()
                .map(UnitPrice::to_decimal_string),
            output: price.output.to_decimal_string(),
            schedule_json: price
                .schedule
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| BillingError::Invalid)?,
            usage_mapping_version: price.usage_mapping_version.clone(),
            source_kind: price.source_kind.clone(),
            source_url: price.source_url.clone(),
            checked_at: price.checked_at.clone(),
            effective_from: price.effective_from.clone(),
            valid_until: price.valid_until.clone(),
        };
        if let Some(existing) = existing {
            return if existing == expected {
                Ok(())
            } else {
                Err(BillingError::Conflict)
            };
        }
        let version_count: i64 = transaction
            .query_row(
                "SELECT count(*) FROM billing_price_versions WHERE provider_id=?1 AND model_id=?2 AND route_policy=?3 AND currency=?4",
                params![price.provider_id, price.model_id, price.route_policy, price.currency.code()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if version_count >= MAX_PRICE_VERSIONS_PER_IDENTITY {
            return Err(BillingError::PriceHistoryLimit);
        }
        transaction.execute(
            "INSERT INTO billing_price_versions(version_id,provider_id,model_id,route_policy,currency,unit_tokens,input_uncached_price,input_cached_price,input_cached_write_price,output_price,schedule_json,usage_mapping_version,source_kind,source_url,checked_at,effective_from,valid_until,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            params![price.version_id,price.provider_id,price.model_id,price.route_policy,price.currency.code(),expected.unit_tokens,price.input_uncached.to_decimal_string(),price.input_cached.to_decimal_string(),price.input_cached_write.as_ref().map(UnitPrice::to_decimal_string),price.output.to_decimal_string(),expected.schedule_json,price.usage_mapping_version,price.source_kind,price.source_url,price.checked_at,price.effective_from,price.valid_until,created_at],
        ).map_err(storage)?;
        insert_audit(
            &transaction,
            &format!("price:{}", price.version_id),
            "price-register",
            &price.version_id,
            &serde_json::json!({
                "priceVersionId": &price.version_id,
                "providerId": &price.provider_id,
                "modelId": &price.model_id,
                "currency": price.currency.code(),
                "sourceKind": &price.source_kind,
            })
            .to_string(),
            created_at,
        )?;
        bump_revision(&transaction)?;
        transaction.commit().map_err(storage)
    }

    /// Creates or updates both currency limits using optimistic revision checking.
    pub fn set_settings(
        connection: &mut Connection,
        run_id: &str,
        cny_limit: NanoMoney,
        usd_limit: NanoMoney,
        expected_revision: Option<u64>,
        updated_at: &str,
    ) -> Result<BudgetSettings, BillingError> {
        validate_text(run_id, 128)?;
        validate_text(updated_at, 64)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let current: Option<(i64, i64, i64)> = transaction
            .query_row(
                "SELECT cny_limit_nanos, usd_limit_nanos, revision FROM billing_run_budgets WHERE run_id=?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(storage)?;
        let next_revision = match (current, expected_revision) {
            (None, None) => 1,
            (None, Some(_)) => return Err(BillingError::StaleRevision),
            (Some((_, _, revision)), Some(expected))
                if u64::try_from(revision).ok() == Some(expected) =>
            {
                expected.checked_add(1).ok_or(BillingError::Invalid)?
            }
            (Some(_), _) => return Err(BillingError::StaleRevision),
        };
        transaction
            .execute(
                "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(run_id) DO UPDATE SET cny_limit_nanos=excluded.cny_limit_nanos, usd_limit_nanos=excluded.usd_limit_nanos, revision=excluded.revision, updated_at=excluded.updated_at",
                params![run_id, cny_limit.nanos(), usd_limit.nanos(), i64::try_from(next_revision).map_err(|_| BillingError::Invalid)?, updated_at],
            )
            .map_err(storage)?;
        insert_audit(
            &transaction,
            &format!("settings:{run_id}:{next_revision}"),
            "run-budget-update",
            run_id,
            &serde_json::json!({
                "revision": next_revision,
                "cnyLimit": cny_limit.to_decimal_string(),
                "usdLimit": usd_limit.to_decimal_string(),
            })
            .to_string(),
            updated_at,
        )?;
        for (currency, cap) in [(Currency::Cny, cny_limit), (Currency::Usd, usd_limit)] {
            maybe_warn(&transaction, run_id, currency, cap, updated_at)?;
        }
        bump_revision(&transaction)?;
        transaction.commit().map_err(storage)?;
        Ok(BudgetSettings {
            run_id: run_id.to_owned(),
            cny_limit,
            usd_limit,
            revision: next_revision,
        })
    }

    /// Atomically verifies remaining same-currency headroom and persists a physical attempt.
    pub fn reserve(connection: &mut Connection, request: &Reservation) -> Result<(), BillingError> {
        validate_reservation(request)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let existing: Option<ExistingReservation> = transaction
            .query_row(
                "SELECT run_id,turn_id,profile_id,attempt,provider_id,model_id,route_policy,price_version_id,currency,reserved_nanos FROM billing_physical_requests WHERE request_id=?1",
                [&request.request_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?)),
            )
            .optional()
            .map_err(storage)?;
        if let Some((
            run_id,
            turn_id,
            profile_id,
            attempt,
            provider_id,
            model_id,
            route_policy,
            price_id,
            currency,
            nanos,
        )) = existing
        {
            if run_id == request.run_id
                && turn_id == request.turn_id
                && profile_id == request.profile_id
                && attempt == request.attempt
                && provider_id == request.provider_id
                && model_id == request.model_id
                && route_policy == request.route_policy
                && price_id == request.price_version_id
                && currency == request.currency.code()
                && nanos == request.amount.nanos()
            {
                return Ok(());
            }
            return Err(BillingError::Conflict);
        }

        if let Some(scope) = request
            .legacy_scope_id
            .as_deref()
            .filter(|scope| *scope != request.run_id)
            && Self::has_run_scope_data(&transaction, scope)?
        {
            return Err(BillingError::ReconciliationRequired);
        }

        let price = Self::price_by_id(&transaction, &request.price_version_id)?
            .ok_or(BillingError::NotFound)?;
        if price.provider_id != request.provider_id
            || price.model_id != request.model_id
            || price.route_policy != request.route_policy
            || price.currency != request.currency
            || !price
                .is_effective_at(&request.dispatch_at)
                .map_err(|_| BillingError::Storage)?
        {
            return Err(BillingError::Invalid);
        }
        let selected_price: Option<String> = transaction
            .query_row(
                "SELECT price_version_id FROM billing_active_profile_prices WHERE profile_id=?1 AND provider_id=?2 AND model_id=?3 AND route_policy=?4 AND currency=?5",
                params![request.profile_id,request.provider_id,request.model_id,request.route_policy,request.currency.code()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if selected_price.as_deref() != Some(request.price_version_id.as_str()) {
            return Err(BillingError::Invalid);
        }

        let settings = read_settings(&transaction, &request.run_id)?;
        let cap = match request.currency {
            Currency::Cny => settings.cny_limit,
            Currency::Usd => settings.usd_limit,
        };
        let reserved: i64 = transaction
            .query_row(
                "SELECT COALESCE(SUM(CASE WHEN state='settled' THEN actual_nanos ELSE reserved_nanos END),0) FROM billing_physical_requests WHERE run_id=?1 AND currency=?2",
                params![request.run_id, request.currency.code()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let total = reserved
            .checked_add(request.amount.nanos())
            .ok_or(BillingError::Invalid)?;
        if total > cap.nanos() {
            return Err(BillingError::Exceeded);
        }
        let pending: i64 = transaction
            .query_row(
                "SELECT count(*) FROM billing_pending_settlements",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let unresolved: i64 = transaction
            .query_row(
                "SELECT count(*) FROM billing_physical_requests r WHERE r.state IN ('reserved','unconfirmed') AND NOT EXISTS (SELECT 1 FROM billing_pending_settlements p WHERE p.request_id=r.request_id)",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if pending
            .checked_add(unresolved)
            .is_none_or(|count| count >= MAX_PENDING_SETTLEMENTS)
        {
            return Err(BillingError::SettlementBacklog);
        }
        transaction
            .execute(
                "INSERT INTO billing_physical_requests(request_id,run_id,turn_id,profile_id,attempt,provider_id,model_id,route_policy,price_version_id,currency,dispatch_at,state,reserved_nanos,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'reserved',?12,?11)",
                params![request.request_id, request.run_id, request.turn_id, request.profile_id, request.attempt, request.provider_id, request.model_id, request.route_policy, request.price_version_id, request.currency.code(), request.dispatch_at, request.amount.nanos()],
            )
            .map_err(storage)?;
        maybe_warn(
            &transaction,
            &request.run_id,
            request.currency,
            cap,
            &request.dispatch_at,
        )?;
        bump_revision(&transaction)?;
        transaction.commit().map_err(storage)
    }

    /// Settles by request identity. Unknown usage retains the conservative hold; duplicate matching
    /// settlement is idempotent and a conflicting callback never overwrites the first result.
    pub fn settle(
        connection: &mut Connection,
        request_id: &str,
        settlement: Option<&Settlement>,
        updated_at: &str,
    ) -> Result<SettlementResult, BillingError> {
        validate_text(request_id, 128)?;
        validate_text(updated_at, 64)?;
        if let Some(settlement) = settlement {
            validate_settlement(settlement)?;
        }
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let current: Option<ExistingSettlement> = transaction
            .query_row(
                "SELECT state,dispatch_at,result_digest,actual_nanos,input_total,input_cached,input_cached_write,output_total,reasoning_included,total_tokens,reserved_nanos,price_version_id FROM billing_physical_requests WHERE request_id=?1",
                [request_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?,row.get(11)?)),
            )
            .optional()
            .map_err(storage)?;
        let Some((
            state,
            dispatch_at,
            digest,
            actual_nanos,
            input_total,
            input_cached,
            input_cached_write,
            output_total,
            reasoning,
            total_tokens,
            _reserved,
            price_version_id,
        )) = current
        else {
            return Err(BillingError::NotFound);
        };
        let Some(value) = settlement else {
            if state == "reserved" {
                transaction
                    .execute(
                        "UPDATE billing_physical_requests SET state='unconfirmed',updated_at=?2 WHERE request_id=?1 AND state='reserved'",
                        params![request_id, updated_at],
                    )
                    .map_err(storage)?;
                bump_revision(&transaction)?;
                transaction.commit().map_err(storage)?;
                return Ok(SettlementResult::MarkedUnconfirmed);
            }
            return if state == "unconfirmed" {
                Ok(SettlementResult::MarkedUnconfirmed)
            } else {
                Err(BillingError::Conflict)
            };
        };
        let price =
            Self::price_by_id(&transaction, &price_version_id)?.ok_or(BillingError::Storage)?;
        let expected_amount = NormalizedUsage::from_provider_with_cache_write(
            value.input_total,
            Some(value.input_cached),
            Some(value.input_cached_write),
            value.output_total,
            value.reasoning_included,
        )
        .and_then(|usage| usage.exact_cost_at(&price, &dispatch_at))
        .and_then(|cost| cost.settle().map_err(Into::into))
        .map_err(|_| BillingError::Invalid)?;
        if expected_amount != value.amount {
            return Err(BillingError::Invalid);
        }
        if state == "settled" {
            return if digest.as_deref() == Some(&value.result_digest)
                && actual_nanos == Some(value.amount.nanos())
                && input_total == Some(to_i64(value.input_total)?)
                && input_cached == Some(to_i64(value.input_cached)?)
                && input_cached_write == Some(to_i64(value.input_cached_write)?)
                && output_total == Some(to_i64(value.output_total)?)
                && reasoning == value.reasoning_included.map(to_i64).transpose()?
                && total_tokens == Some(to_i64(value.total_tokens)?)
            {
                Ok(SettlementResult::AlreadyApplied)
            } else {
                Err(BillingError::Conflict)
            };
        }
        if state != "reserved" && state != "unconfirmed" {
            return Err(BillingError::Conflict);
        }
        let changed = transaction
            .execute(
                "UPDATE billing_physical_requests SET state='settled',input_total=?2,input_cached=?3,input_cached_write=?4,output_total=?5,reasoning_included=?6,total_tokens=?7,actual_nanos=?8,result_digest=?9,updated_at=?10 WHERE request_id=?1 AND state IN ('reserved','unconfirmed')",
                params![request_id, to_i64(value.input_total)?, to_i64(value.input_cached)?, to_i64(value.input_cached_write)?, to_i64(value.output_total)?, value.reasoning_included.map(to_i64).transpose()?, to_i64(value.total_tokens)?, value.amount.nanos(), value.result_digest, updated_at],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(BillingError::Conflict);
        }
        let (run_id, currency, cap) = {
            let (run_id, currency_code, _) = transaction
                .query_row(
                    "SELECT run_id,currency,reserved_nanos FROM billing_physical_requests WHERE request_id=?1",
                    [request_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
                )
                .map_err(storage)?;
            let currency = currency_code.parse().map_err(|_| BillingError::Storage)?;
            let settings = read_settings(&transaction, &run_id)?;
            let cap = match currency {
                Currency::Cny => settings.cny_limit,
                Currency::Usd => settings.usd_limit,
            };
            (run_id, currency, cap)
        };
        maybe_warn(&transaction, &run_id, currency, cap, updated_at)?;
        bump_revision(&transaction)?;
        transaction.commit().map_err(storage)?;
        Ok(SettlementResult::Applied)
    }
}

fn read_settings(connection: &Connection, run_id: &str) -> Result<BudgetSettings, BillingError> {
    connection
        .query_row(
            "SELECT cny_limit_nanos,usd_limit_nanos,revision FROM billing_run_budgets WHERE run_id=?1",
            [run_id],
            |row| {
                Ok(BudgetSettings {
                    run_id: run_id.to_owned(),
                    cny_limit: NanoMoney::from_nanos(row.get(0)?).map_err(to_sql_error)?,
                    usd_limit: NanoMoney::from_nanos(row.get(1)?).map_err(to_sql_error)?,
                    revision: u64::try_from(row.get::<_, i64>(2)?)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, -1))?,
                })
            },
        )
        .optional()
        .map_err(storage)?
        .ok_or(BillingError::LimitMissing)
}

fn maybe_warn(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    currency: Currency,
    cap: NanoMoney,
    reached_at: &str,
) -> Result<(), BillingError> {
    if cap.nanos() == 0 {
        return Ok(());
    }
    let committed: i64 = transaction
        .query_row(
            "SELECT COALESCE(SUM(CASE WHEN state='settled' THEN actual_nanos ELSE reserved_nanos END),0) FROM billing_physical_requests WHERE run_id=?1 AND currency=?2",
            params![run_id, currency.code()],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if i128::from(committed) * 100 >= i128::from(cap.nanos()) * 80 {
        transaction
            .execute(
                "INSERT OR IGNORE INTO billing_budget_warnings(run_id,currency,threshold_percent,reached_at) VALUES(?1,?2,80,?3)",
                params![run_id,currency.code(),reached_at],
            )
            .map_err(storage)?;
    }
    Ok(())
}

pub(super) fn month_range(month: &str, time_zone: &str) -> Result<(String, String), BillingError> {
    validate_text(time_zone, 128)?;
    if month.len() != 7
        || month.as_bytes().get(4) != Some(&b'-')
        || !month
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 4 || byte.is_ascii_digit())
    {
        return Err(BillingError::Invalid);
    }
    let year = month[..4]
        .parse::<i16>()
        .map_err(|_| BillingError::Invalid)?;
    let month_number = month[5..]
        .parse::<i8>()
        .map_err(|_| BillingError::Invalid)?;
    let first = Date::new(year, month_number, 1).map_err(|_| BillingError::Invalid)?;
    let after = first
        .checked_add(1.months())
        .map_err(|_| BillingError::Invalid)?;
    let start = first
        .in_tz(time_zone)
        .map_err(|_| BillingError::Invalid)?
        .timestamp()
        .to_string();
    let end = after
        .in_tz(time_zone)
        .map_err(|_| BillingError::Invalid)?
        .timestamp()
        .to_string();
    Ok((start, end))
}

fn bump_revision(transaction: &rusqlite::Transaction<'_>) -> Result<(), BillingError> {
    let changed = transaction
        .execute("UPDATE billing_meta SET revision=revision+1 WHERE id=1 AND revision<9223372036854775807", [])
        .map_err(storage)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(BillingError::Storage)
    }
}

fn pending_usage_digest(pending: &PendingSettlement) -> String {
    let mut hash = Sha256::new();
    hash.update(pending.input_total.to_be_bytes());
    hash_optional_count(&mut hash, pending.input_cached);
    hash_optional_count(&mut hash, pending.input_cached_write);
    hash.update(pending.output_total.to_be_bytes());
    hash_optional_count(&mut hash, pending.reasoning_included);
    format!("sha256:{:x}", hash.finalize())
}

struct BackfillableUsage {
    request_id: String,
    run_id: String,
    provider_id: String,
    model_id: String,
    route_policy: String,
    currency: Currency,
    price_version_id: String,
    dispatch_at: String,
    total_tokens: Option<u64>,
    usage: PendingSettlement,
}

impl BackfillableUsage {
    fn normalized(&self) -> Result<NormalizedUsage, BillingError> {
        NormalizedUsage::from_provider_with_cache_write(
            self.usage.input_total,
            self.usage.input_cached,
            self.usage.input_cached_write,
            self.usage.output_total,
            self.usage.reasoning_included,
        )
        .map_err(|_| BillingError::Invalid)
    }
}

fn backfillable_usage(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<BackfillableUsage>, BillingError> {
    let stored = connection
        .query_row(
            "SELECT run_id,provider_id,model_id,route_policy,currency,price_version_id,dispatch_at,input_total,input_cached,input_cached_write,output_total,reasoning_included,total_tokens FROM billing_physical_requests WHERE request_id=?1 AND state='unconfirmed' AND actual_nanos IS NULL AND NOT EXISTS (SELECT 1 FROM billing_pending_settlements WHERE request_id=?1)",
            [request_id],
            |row| Ok((
                row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?,
                row.get::<_, String>(6)?, row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?, row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?, row.get::<_, Option<i64>>(11)?,
                row.get::<_, Option<i64>>(12)?,
            )),
        )
        .optional()
        .map_err(storage)?;
    let Some((
        run_id,
        provider_id,
        model_id,
        route_policy,
        currency,
        price_version_id,
        dispatch_at,
        input_total,
        input_cached,
        input_cached_write,
        output_total,
        reasoning_included,
        total_tokens,
    )) = stored
    else {
        return Ok(None);
    };
    let Some((input_total, input_cached, input_cached_write, output_total)) = input_total
        .zip(input_cached)
        .zip(input_cached_write)
        .zip(output_total)
        .map(|(((input, cached), cache_write), output)| (input, cached, cache_write, output))
    else {
        return Ok(None);
    };
    let usage = PendingSettlement {
        request_id: request_id.to_owned(),
        input_total: u64::try_from(input_total).map_err(|_| BillingError::Storage)?,
        input_cached: Some(u64::try_from(input_cached).map_err(|_| BillingError::Storage)?),
        input_cached_write: Some(
            u64::try_from(input_cached_write).map_err(|_| BillingError::Storage)?,
        ),
        output_total: u64::try_from(output_total).map_err(|_| BillingError::Storage)?,
        reasoning_included: reasoning_included
            .map(u64::try_from)
            .transpose()
            .map_err(|_| BillingError::Storage)?,
        diagnostic_code: None,
    };
    Ok(Some(BackfillableUsage {
        request_id: request_id.to_owned(),
        run_id,
        provider_id,
        model_id,
        route_policy,
        currency: currency.parse().map_err(|_| BillingError::Storage)?,
        price_version_id,
        dispatch_at,
        total_tokens: total_tokens
            .map(u64::try_from)
            .transpose()
            .map_err(|_| BillingError::Storage)?,
        usage,
    }))
}

fn validate_backfill_price(
    row: &BackfillableUsage,
    price: &PriceVersion,
) -> Result<(), BillingError> {
    if row.provider_id != price.provider_id
        || row.model_id != price.model_id
        || row.route_policy != price.route_policy
        || row.currency != price.currency
    {
        return Err(BillingError::Conflict);
    }
    let normalized = row.normalized()?;
    if row
        .total_tokens
        .is_some_and(|tokens| tokens != normalized.total_tokens)
    {
        return Err(BillingError::Conflict);
    }
    Ok(())
}

fn backfill_amount(
    row: &BackfillableUsage,
    price: &PriceVersion,
) -> Result<NanoMoney, BillingError> {
    row.normalized()?
        .exact_cost_at(price, &row.dispatch_at)
        .map_err(|_| BillingError::Invalid)?
        .settle()
        .map_err(|_| BillingError::Invalid)
}

fn read_ledger_revision(connection: &Connection) -> Result<u64, BillingError> {
    connection
        .query_row("SELECT revision FROM billing_meta WHERE id=1", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(storage)
        .and_then(|revision| u64::try_from(revision).map_err(|_| BillingError::Storage))
}

fn hash_optional_count(hash: &mut Sha256, value: Option<u64>) {
    match value {
        Some(value) => {
            hash.update([1]);
            hash.update(value.to_be_bytes());
        }
        None => hash.update([0]),
    }
}

fn insert_audit(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: &str,
    operation_kind: &str,
    target_id: &str,
    details_json: &str,
    created_at: &str,
) -> Result<(), BillingError> {
    transaction
        .execute(
            "INSERT INTO billing_audit(operation_id,operation_kind,target_id,details_json,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![operation_id, operation_kind, target_id, details_json, created_at],
        )
        .map(|_| ())
        .map_err(storage)
}

fn make_cursor(revision: i64, max_row_id: i64, after_row_id: i64) -> String {
    format!("r{revision};m{max_row_id};a{after_row_id}")
}

fn parse_cursor(cursor: &str) -> Result<(i64, i64, i64), BillingError> {
    if cursor.len() > 96 {
        return Err(BillingError::Invalid);
    }
    let mut parts = cursor.split(';');
    let revision = parts
        .next()
        .and_then(|part| part.strip_prefix('r'))
        .and_then(|part| part.parse::<i64>().ok());
    let max_row_id = parts
        .next()
        .and_then(|part| part.strip_prefix('m'))
        .and_then(|part| part.parse::<i64>().ok());
    let after_row_id = parts
        .next()
        .and_then(|part| part.strip_prefix('a'))
        .and_then(|part| part.parse::<i64>().ok());
    match (revision, max_row_id, after_row_id, parts.next()) {
        (Some(revision), Some(max), Some(after), None)
            if revision > 0 && max > 0 && after >= 0 && after < max =>
        {
            Ok((revision, max, after))
        }
        _ => Err(BillingError::Invalid),
    }
}

fn validate_reservation(request: &Reservation) -> Result<(), BillingError> {
    for value in [
        &request.request_id,
        &request.run_id,
        &request.turn_id,
        &request.profile_id,
        &request.provider_id,
        &request.model_id,
        &request.route_policy,
        &request.price_version_id,
        &request.dispatch_at,
    ] {
        validate_text(value, 2048)?;
    }
    if request.attempt == 0 {
        return Err(BillingError::Invalid);
    }
    Ok(())
}

fn validate_settlement(value: &Settlement) -> Result<(), BillingError> {
    if value
        .input_cached
        .checked_add(value.input_cached_write)
        .is_none_or(|count| count > value.input_total)
        || value
            .reasoning_included
            .is_some_and(|count| count > value.output_total)
        || value.input_total.checked_add(value.output_total) != Some(value.total_tokens)
        || value.result_digest.is_empty()
        || value.result_digest.len() > 128
    {
        return Err(BillingError::Invalid);
    }
    for count in [
        value.input_total,
        value.input_cached,
        value.input_cached_write,
        value.output_total,
        value.total_tokens,
    ] {
        to_i64(count)?;
    }
    if let Some(reasoning) = value.reasoning_included {
        to_i64(reasoning)?;
    }
    Ok(())
}

fn validate_text(value: &str, max: usize) -> Result<(), BillingError> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(BillingError::Invalid)
    } else {
        Ok(())
    }
}

fn to_i64(value: u64) -> Result<i64, BillingError> {
    i64::try_from(value).map_err(|_| BillingError::Invalid)
}

fn to_sql_error(_: aoidos_billing::money::MoneyError) -> rusqlite::Error {
    rusqlite::Error::IntegralValueOutOfRange(0, i64::MAX)
}

fn storage(_: rusqlite::Error) -> BillingError {
    BillingError::Storage
}
