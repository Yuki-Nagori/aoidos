//! 入口共用的只读计费视图；账本读写仍由 engine 的计费端口负责。

use aoidos_engine::{
    billing::{BillingError, BillingLedger, BudgetSettings, BudgetWarning, ModelUsageSummary},
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
    #[must_use]
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
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

fn billing_fault(error: BillingError) -> Fault {
    match error {
        BillingError::Invalid => Fault::new("app.bad-request", "费用账本查询参数无效"),
        BillingError::Storage => Fault::new("store.database", "费用账本不可用"),
        _ => Fault::new("store.database", "费用账本读取失败"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoidos_engine::{
        migration::{MigrationEvent, MigrationEvents},
        record::session::{Appended, RecordEvents},
    };
    use std::{path::PathBuf, time::SystemTime};

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
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("aoidos-app-billing-{suffix}"));
        std::fs::create_dir_all(&root).unwrap();
        let events = Arc::new(SilentEvents);
        (Arc::new(Storage::new(&root, events.clone(), events)), root)
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
}
