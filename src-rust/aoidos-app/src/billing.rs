//! 入口共用的计费设置与查询服务；账本事务仍由 engine 的计费模块负责。

pub use aoidos_billing::money::{Currency, NanoMoney};
pub use aoidos_billing::price::PriceVersion;
use aoidos_engine::{
    billing::{
        BillingError, BillingLedger, BudgetSettings, BudgetWarning, ModelUsageSummary,
        SuggestedLimits,
    },
    fault::Fault,
    storage::Storage,
};
use std::sync::Arc;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// 单周目的有界计费摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunBillingSummary {
    pub models: Vec<ModelUsageSummary>,
    pub warnings: Vec<BudgetWarning>,
}

/// 单次请求发送前从本地账本读取的价格身份与本局双币种上限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestBillingPreflight {
    pub budget: Option<BudgetSettings>,
    pub selected_price: Option<SelectedPrice>,
}

/// 当前 profile 实际选中的不可变价格版本摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedPrice {
    pub version_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub currency: String,
    pub source_kind: String,
}

/// 通过 engine 共享存储读取当前周目的费用与预算提醒。
pub struct BillingService {
    storage: Arc<Storage>,
}

impl BillingService {
    /// 返回应用内置的 DeepSeek 参考价目；不会访问数据库、网络或凭据库。
    #[must_use]
    pub fn builtin_price_templates() -> Vec<PriceVersion> {
        aoidos_billing::providers::deepseek::flash_prices()
    }

    #[must_use]
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    /// 返回应用随版本发布的参考价目；读取不会触碰或修改账本。
    #[must_use]
    pub fn price_templates(&self) -> Vec<PriceVersion> {
        Self::builtin_price_templates()
    }

    /// 将用户确认的内置价格登记为不可变版本，并选给指定 profile。
    ///
    /// # Errors
    /// profile / 价格冲突或数据库不可用时返回稳定错误。
    pub fn confirm_deepseek_price(
        &self,
        profile_id: &str,
        model_id: &str,
        currency: Currency,
    ) -> Result<PriceVersion, Fault> {
        if model_id != "deepseek-flash" {
            return Err(Fault::new(
                "budget.price-missing",
                "当前只有 DeepSeek Flash 提供内置价格",
            ));
        }
        let price = self
            .price_templates()
            .into_iter()
            .find(|price| price.currency == currency)
            .ok_or_else(|| Fault::new("app.bad-request", "没有对应币种的内置价格"))?;
        let created_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| Fault::new("store.database", "读取计价时间失败"))?;
        self.storage.with_database(|connection| {
            BillingLedger::register_price(connection, &price, &created_at)
                .map_err(billing_fault)?;
            BillingLedger::select_profile_price_version(
                connection,
                profile_id,
                &price.version_id,
                &created_at,
            )
            .map_err(billing_fault)
        })
    }

    /// 保存周目内彼此独立的 CNY / USD 上限。
    ///
    /// # Errors
    /// revision 过期、金额非法或数据库不可用时返回稳定错误。
    pub fn set_run_limits(
        &self,
        run_id: &str,
        cny_limit: NanoMoney,
        usd_limit: NanoMoney,
        expected_revision: Option<u64>,
    ) -> Result<BudgetSettings, Fault> {
        let updated_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| Fault::new("store.database", "读取计价时间失败"))?;
        self.storage.with_database(|connection| {
            BillingLedger::set_settings(
                connection,
                run_id,
                cny_limit,
                usd_limit,
                expected_revision,
                &updated_at,
            )
            .map_err(billing_fault)
        })
    }

    /// 按当前有效价目计算同币种建议值，不进行汇率换算。
    ///
    /// # Errors
    /// 价格数据异常或数据库不可用时返回稳定错误。
    pub fn suggested_limits(
        &self,
        provider_id: &str,
        model_id: &str,
        route_policy: &str,
    ) -> Result<SuggestedLimits, Fault> {
        let dispatch_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| Fault::new("store.database", "读取计价时间失败"))?;
        self.storage.with_database(|connection| {
            BillingLedger::suggested_limits(
                connection,
                provider_id,
                model_id,
                route_policy,
                &dispatch_at,
            )
            .map_err(billing_fault)
        })
    }

    /// 读取本局双币种额度；尚未设置时返回 `None`。
    pub fn run_limits(&self, run_id: &str) -> Result<Option<BudgetSettings>, Fault> {
        self.storage.with_database(|connection| {
            match BillingLedger::get_settings(connection, run_id) {
                Ok(settings) => Ok(Some(settings)),
                Err(BillingError::LimitMissing) => Ok(None),
                Err(error) => Err(billing_fault(error)),
            }
        })
    }

    /// 读取 profile 当前绑定且仍有效的价格版本。
    pub fn selected_price(
        &self,
        profile_id: &str,
        provider_id: &str,
        model_id: &str,
        route_policy: &str,
    ) -> Result<Option<PriceVersion>, Fault> {
        let dispatch_at = now()?;
        self.storage.with_database(|connection| {
            BillingLedger::profile_price(
                connection,
                profile_id,
                provider_id,
                model_id,
                route_policy,
                &dispatch_at,
            )
            .map_err(billing_fault)
        })
    }

    /// 读取指定币种当前有效的不可变价格版本。
    pub fn active_price(
        &self,
        provider_id: &str,
        model_id: &str,
        route_policy: &str,
        currency: Currency,
    ) -> Result<Option<PriceVersion>, Fault> {
        let dispatch_at = now()?;
        self.storage.with_database(|connection| {
            BillingLedger::active_price_for_currency(
                connection,
                provider_id,
                model_id,
                route_policy,
                &dispatch_at,
                currency,
            )
            .map_err(billing_fault)
        })
    }

    /// 选择与 profile、模型和币种匹配的有效价格版本。
    pub fn select_profile_price(
        &self,
        profile_id: &str,
        provider_id: &str,
        model_id: &str,
        route_policy: &str,
        currency: Currency,
    ) -> Result<PriceVersion, Fault> {
        let selected_at = now()?;
        self.storage.with_database(|connection| {
            BillingLedger::select_profile_price(
                connection,
                profile_id,
                provider_id,
                model_id,
                route_policy,
                currency,
                &selected_at,
            )
            .map_err(billing_fault)
        })
    }

    /// 登记调用方明确提交的不可变价格版本。
    pub fn register_price(&self, price: PriceVersion) -> Result<PriceVersion, Fault> {
        let created_at = now()?;
        price
            .validate()
            .map_err(|_| Fault::new("app.bad-request", "金额或价格参数不合法"))?;
        self.storage.with_database(|connection| {
            BillingLedger::register_price(connection, &price, &created_at)
                .map_err(billing_fault)?;
            Ok(price)
        })
    }

    /// 读取按供应商、币种和模型分组的用量及预算提醒。
    ///
    /// # Errors
    /// 数据库不可用时返回稳定存储错误；非法周目身份返回请求错误。
    pub fn run_summary(&self, run_id: &str) -> Result<RunBillingSummary, Fault> {
        self.storage.with_database(|connection| {
            let models = BillingLedger::run_summary(connection, run_id).map_err(billing_fault)?;
            let warnings = BillingLedger::warnings(connection, run_id).map_err(billing_fault)?;
            Ok(RunBillingSummary { models, warnings })
        })
    }

    /// 读取 CLI 发送确认需要展示的当前价格与双币种额度，不创建或修改账本数据。
    ///
    /// # Errors
    /// 数据库不可用、账本损坏或当前时刻无法格式化时返回稳定存储错误。
    pub fn request_preflight(
        &self,
        run_id: &str,
        profile_id: &str,
        provider_id: &str,
        model_id: &str,
        route_policy: &str,
    ) -> Result<RequestBillingPreflight, Fault> {
        let dispatch_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| Fault::new("store.database", "读取计价时间失败"))?;
        self.storage.with_database(|connection| {
            let budget = match BillingLedger::get_settings(connection, run_id) {
                Ok(settings) => Some(settings),
                Err(BillingError::LimitMissing) => None,
                Err(error) => return Err(billing_fault(error)),
            };
            let selected_price = BillingLedger::profile_price(
                connection,
                profile_id,
                provider_id,
                model_id,
                route_policy,
                &dispatch_at,
            )
            .map_err(billing_fault)?
            .map(|price| SelectedPrice {
                version_id: price.version_id,
                provider_id: price.provider_id,
                model_id: price.model_id,
                currency: price.currency.code().to_owned(),
                source_kind: price.source_kind,
            });
            Ok(RequestBillingPreflight {
                budget,
                selected_price,
            })
        })
    }
}

fn now() -> Result<String, Fault> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| Fault::new("store.database", "读取计价时间失败"))
}

pub fn billing_fault(error: BillingError) -> Fault {
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
        BillingError::SettlementBacklog => Fault::new(
            "budget.settlement-backlog",
            "待恢复费用达到安全上限，请先重试已有结算",
        ),
        BillingError::NotFound => Fault::new("app.not-found", "费用信息不存在"),
        BillingError::Conflict => Fault::new("budget.conflict", "费用记录发生冲突"),
        BillingError::Storage => Fault::new("store.database", "费用存储不可用"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoidos_engine::{
        billing::BillingLedger,
        migration::{MigrationEvent, MigrationEvents},
        record::session::{Appended, RecordEvents},
    };
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_STORAGE_ID: AtomicU64 = AtomicU64::new(0);

    struct SilentEvents;

    impl MigrationEvents for SilentEvents {
        fn prepare(&self, event: &MigrationEvent) -> Result<u64, Fault> {
            let _ = event;
            Ok(0)
        }

        fn deliver(&self, sequence: u64, event: MigrationEvent) -> Result<(), Fault> {
            let _ = (sequence, event);
            Ok(())
        }

        fn retire(&self, id: &str) {
            let _ = id;
        }
    }

    impl RecordEvents for SilentEvents {
        fn prepare(&self, event: &Appended) -> Result<u64, Fault> {
            let _ = event;
            Ok(0)
        }

        fn deliver(&self, sequence: u64, event: Appended) -> Result<(), Fault> {
            let _ = (sequence, event);
            Ok(())
        }
    }

    fn storage() -> (Arc<Storage>, PathBuf) {
        let suffix = NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "aoidos-app-billing-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let events = Arc::new(SilentEvents);
        let storage = Arc::new(Storage::new(&root, events.clone(), events));
        storage.ready().unwrap();
        (storage, root)
    }

    #[test]
    fn summary_reads_empty_run_and_rejects_invalid_identity() {
        let (storage, root) = storage();
        let service = BillingService::new(storage);

        let summary = service.run_summary("run-1").unwrap();
        assert!(summary.models.is_empty());
        assert!(summary.warnings.is_empty());
        assert_eq!(service.run_summary("").unwrap_err().code, "app.bad-request");

        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn confirmed_template_is_pinned_to_profile_without_currency_conversion() {
        let (storage, root) = storage();
        let service = BillingService::new(storage);

        let selected = service
            .confirm_deepseek_price("profile-1", "deepseek-flash", Currency::Cny)
            .unwrap();
        assert_eq!(selected.currency, Currency::Cny);
        let cny = service
            .request_preflight("run-1", "profile-1", "deepseek", "deepseek-flash", "direct")
            .unwrap()
            .selected_price
            .unwrap();
        assert_eq!(cny.currency, "CNY");
        assert_eq!(cny.version_id, selected.version_id);

        let selected = service
            .confirm_deepseek_price("profile-1", "deepseek-flash", Currency::Usd)
            .unwrap();
        assert_eq!(selected.currency, Currency::Usd);
        let usd = service
            .request_preflight("run-1", "profile-1", "deepseek", "deepseek-flash", "direct")
            .unwrap()
            .selected_price
            .unwrap();
        assert_eq!(usd.currency, "USD");
        assert_eq!(usd.version_id, selected.version_id);
        assert_ne!(cny.version_id, usd.version_id);

        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn confirmation_pins_the_exact_template_even_when_a_newer_manual_price_exists() {
        let (storage, root) = storage();
        let service = BillingService::new(storage);
        let template = service
            .price_templates()
            .into_iter()
            .find(|price| price.currency == Currency::Cny)
            .unwrap();
        let mut manual = template.clone();
        manual.version_id = "zzzz-manual-price".into();
        manual.source_kind = "user-entered".into();
        let created_at = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        service
            .storage
            .with_database(|connection| {
                BillingLedger::register_price(connection, &manual, &created_at)
                    .map_err(billing_fault)
            })
            .unwrap();

        let selected = service
            .confirm_deepseek_price("profile-1", "deepseek-flash", Currency::Cny)
            .unwrap();
        assert_eq!(selected.version_id, template.version_id);

        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn built_in_deepseek_price_rejects_models_outside_its_catalog() {
        let (storage, root) = storage();
        let service = BillingService::new(storage);
        assert_eq!(
            service
                .confirm_deepseek_price("profile-1", "deepseek-v4-pro", Currency::Cny)
                .unwrap_err()
                .code,
            "budget.price-missing"
        );
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
}
