//! 已开周目的共享应用查询与生命周期入口。

use aoidos_engine::{fault::Fault, game::product::ProfileFactory, run_catalog, storage::Storage};
use std::sync::Arc;

/// GUI 与 CLI/TUI 共用的目录查询服务。
#[derive(Clone)]
pub struct RunLifecycleService {
    storage: Arc<Storage>,
    profiles: Arc<ProfileFactory>,
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
