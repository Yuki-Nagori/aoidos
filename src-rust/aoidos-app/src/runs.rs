//! 已开周目的共享应用查询与生命周期入口。

use aoidos_engine::{
    fault::Fault,
    game::{
        product::{OpenedSession, Product, ProfileFactory},
        runtime::Service,
    },
    run_catalog,
    storage::Storage,
};
use aoidos_llm::config::LlmProfile;
use std::sync::Arc;

/// GUI 与 CLI/TUI 共用的目录查询服务。
#[derive(Clone)]
pub struct RunLifecycleService {
    storage: Arc<Storage>,
    profiles: Arc<ProfileFactory>,
}

/// 显式恢复结果携带该周目冻结的公开 profile，供入口展示准确的模型与计价信息。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedRun {
    pub session: OpenedSession,
    pub profile: LlmProfile,
}

impl RunLifecycleService {
    #[must_use]
    pub fn new(storage: Arc<Storage>, profiles: Arc<ProfileFactory>) -> Self {
        Self { storage, profiles }
    }

    /// 读取一页 SQLite 周目目录摘要，不访问 transcript 文件。
    /// # Errors
    /// 游标过期、页大小非法或存储损坏时返回稳定 Fault。
    pub fn page(
        &self,
        cursor: Option<&run_catalog::RunCursor>,
        page_size: usize,
    ) -> Result<run_catalog::RunPage, Fault> {
        self.storage.run_catalog_page(cursor, page_size)
    }

    /// 读取删除确认所需的只读计费门禁；不打开周目或启动删除流程。
    /// # Errors
    /// 目录 revision 过期、身份映射不一致或存储损坏时失败关闭。
    pub async fn deletion_check(
        &self,
        session_id: String,
        expected_revision: u64,
    ) -> Result<run_catalog::RunDeletionCheck, Fault> {
        let storage = self.storage.clone();
        aoidos_engine::blocking::run(move || {
            storage.run_deletion_check(&session_id, expected_revision)
        })
        .await
    }

    /// 通过共享应用服务恢复玩家指定的周目，并返回冻结配置供调用方使用。
    /// # Errors
    /// 目录 revision、身份映射、恢复状态或活动选择提交不合法时返回稳定 Fault。
    pub async fn open_selected(
        &self,
        product: &Product,
        runtime: &Service,
        session_id: String,
        expected_revision: u64,
    ) -> Result<OpenedRun, Fault> {
        let storage = self.storage.clone();
        let requested_id = session_id.clone();
        let target = aoidos_engine::blocking::run(move || {
            storage.run_open_target(&requested_id, expected_revision)
        })
        .await?;
        let session = product
            .open_selected(runtime, session_id, expected_revision)
            .await?;
        Ok(OpenedRun {
            session,
            profile: target.profile,
        })
    }

    /// 读取已打开周目的冻结公开 profile，供普通重开入口同步展示和计费预检。
    /// # Errors
    /// 周目目录、身份映射或冻结快照损坏时返回稳定 Fault。
    pub async fn profile_snapshot(&self, session_id: String) -> Result<LlmProfile, Fault> {
        let storage = self.storage.clone();
        aoidos_engine::blocking::run(move || storage.current_run_open_target(&session_id))
            .await
            .map(|target| target.profile)
    }

    /// 玩家主动继续发现旧周目；列表刷新本身不会触发磁盘扫描。
    /// # Errors
    /// 存储不可用或扫描批次无法登记时返回稳定 Fault。
    pub fn scan_legacy(&self) -> Result<run_catalog::LegacyScanResult, Fault> {
        self.storage.scan_legacy_runs()
    }

    /// 玩家明确选择 profile 后，为旧周目保存不含密钥的配置快照。
    /// # Errors
    /// 目录 revision 已变化、周目不需要绑定或 profile 不可用时拒绝。
    pub fn bind_profile(
        &self,
        session_id: &str,
        expected_revision: u64,
        profile_id: &str,
    ) -> Result<u64, Fault> {
        let profile = self.profiles.public_profile(profile_id)?;
        self.storage
            .bind_run_profile(session_id, expected_revision, &profile)
    }

    /// 玩家确认冲突上限后，在共享生命周期服务中原子迁移历史计费身份。
    /// # Errors
    /// revision / 身份映射已变化、冲突上限未选择或存储失败时拒绝迁移。
    pub fn reconcile_billing(
        &self,
        session_id: &str,
        expected_revision: u64,
        selected_limits: Option<(
            aoidos_billing::money::NanoMoney,
            aoidos_billing::money::NanoMoney,
        )>,
    ) -> Result<run_catalog::RunBillingReconciliation, Fault> {
        let updated_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| Fault::new("store.database", "读取迁移时间失败"))?;
        self.storage.reconcile_run_billing(
            session_id,
            expected_revision,
            selected_limits,
            &updated_at,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoidos_engine::{
        migration::MigrationEvent, ports::TurnEvent, record::session::RecordEvents,
    };

    struct Events;

    impl aoidos_engine::migration::MigrationEvents for Events {
        fn prepare(&self, _: &MigrationEvent) -> Result<u64, Fault> {
            Ok(1)
        }
        fn deliver(&self, _: u64, _: MigrationEvent) -> Result<(), Fault> {
            Ok(())
        }
        fn retire(&self, _: &str) {}
    }

    impl RecordEvents for Events {
        fn prepare(&self, _: &aoidos_engine::record::session::Appended) -> Result<u64, Fault> {
            Ok(1)
        }
        fn deliver(
            &self,
            _: u64,
            _: aoidos_engine::record::session::Appended,
        ) -> Result<(), Fault> {
            Ok(())
        }
        fn retire(&self, _: &str) {}
    }

    impl aoidos_engine::ports::EventPort for Events {
        fn prepare(&self, _: TurnEvent) -> Result<aoidos_engine::ports::PreparedEvent, Fault> {
            unreachable!("目录查询不会投递回合事件")
        }
        fn deliver(&self, _: aoidos_engine::ports::PreparedEvent) -> Result<(), Fault> {
            unreachable!("目录查询不会投递回合事件")
        }
        fn retire(&self, _: &str) {}
    }

    #[test]
    fn page_reports_an_empty_incomplete_catalog_without_opening_a_session() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("aoidos-runs-{nonce}"));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Storage::new(&root, Arc::new(Events), Arc::new(Events)));
        let source = Arc::new(crate::profiles::Profiles::new(&root.join("llm")));
        let turns = crate::TurnRuntime::new(
            Arc::new(Events),
            aoidos_llm::proxy::SystemProxySnapshot::capture(),
        )
        .unwrap();
        let factory = aoidos_engine::game::product::ProfileFactory::new(
            source,
            storage.clone(),
            turns.proxy_snapshot.clone(),
            turns.diagnostics.clone(),
        );
        let service = RunLifecycleService::new(storage.clone(), factory);

        let page = service.page(None, run_catalog::DEFAULT_PAGE_SIZE).unwrap();
        assert!(page.entries.is_empty());
        assert!(page.scan_incomplete);
        assert!(!root.join("active-session.json").exists());

        storage.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
