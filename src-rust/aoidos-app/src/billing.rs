//! 入口共用的只读计费视图；账本读写仍由 engine 的计费端口负责。

use aoidos_engine::{
    billing::{BillingError, BillingLedger, BudgetWarning, ModelUsageSummary},
    fault::Fault,
    storage::Storage,
};
use std::sync::Arc;

/// 单周目的有界计费摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunBillingSummary {
    pub models: Vec<ModelUsageSummary>,
    pub warnings: Vec<BudgetWarning>,
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
