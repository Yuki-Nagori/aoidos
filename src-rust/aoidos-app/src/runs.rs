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
    deletion_gate: Arc<tokio::sync::Mutex<()>>,
}

/// 显式恢复结果携带该周目冻结的公开 profile，供入口展示准确的模型与计价信息。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedRun {
    pub session: OpenedSession,
    pub profile: LlmProfile,
}

/// 启动恢复只隔离失败的删除目标，其余操作仍可完成，应用也可进入只读目录界面。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletionRecoveryReport {
    pub completed: usize,
    pub failed: Vec<DeletionRecoveryFailure>,
}

/// 未能安全继续的单个删除操作；详细错误保留在调用者的稳定诊断中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionRecoveryFailure {
    pub operation_id: String,
    pub session_id: String,
    pub code: String,
}

impl RunLifecycleService {
    #[must_use]
    pub fn new(storage: Arc<Storage>, profiles: Arc<ProfileFactory>) -> Self {
        Self {
            storage,
            profiles,
            deletion_gate: Arc::new(tokio::sync::Mutex::new(())),
        }
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

    /// 显式确认并前向推进一个周目删除操作；同一 operation ID 可安全重试。
    ///
    /// 每次最多递归清理 128 个文件系统节点。较大的记忆目录会返回仍待清理的阶段，
    /// 调用方可再次调用或交由下次启动恢复；数据库目录项已移除，不会重新显示为可打开周目。
    /// # Errors
    /// 计费门禁、身份校验、阶段恢复或文件系统清理失败时保留 journal 并返回错误。
    pub async fn delete_run(
        &self,
        product: &Product,
        runtime: &Service,
        operation_id: String,
        session_id: String,
        expected_revision: u64,
    ) -> Result<run_catalog::DeletionOperation, Fault> {
        let _gate = self.deletion_gate.lock().await;
        let result = self
            .advance_deletion(
                product,
                runtime,
                operation_id.clone(),
                session_id.clone(),
                expected_revision,
            )
            .await;
        match result {
            Ok(operation) => {
                if operation.state != "completed" {
                    let _ = self
                        .set_deletion_diagnostic(&operation_id, &session_id, None)
                        .await;
                }
                Ok(operation)
            }
            Err(error) => {
                let _ = self
                    .set_deletion_diagnostic(&operation_id, &session_id, Some(error.code.clone()))
                    .await;
                Err(error)
            }
        }
    }

    /// 启动时读取固定批次的未完成 journal，并只向前推进删除状态。
    ///
    /// 每次文件系统清理仍受 128 项上限约束；启动器逐批推进直至本地 journal 全部完成。
    /// 单个操作无法安全恢复时保留 journal 并隔离该目标，不阻止其他周目使用。
    /// # Errors
    /// journal 查询失败时返回错误；单项恢复失败收录在返回报告中。
    pub async fn recover_pending_deletions(
        &self,
        product: &Product,
        runtime: &Service,
    ) -> Result<DeletionRecoveryReport, Fault> {
        let _gate = self.deletion_gate.lock().await;
        let mut report = DeletionRecoveryReport::default();
        loop {
            let storage = self.storage.clone();
            let operations =
                aoidos_engine::blocking::run(move || storage.pending_run_deletions(128)).await?;
            if operations.is_empty() {
                return Ok(report);
            }
            let mut batch_failed = false;
            for operation in operations {
                let operation_id = operation.operation_id;
                let session_id = operation.session_id;
                let mut revision = operation.catalog_revision.max(1);
                loop {
                    let result = self
                        .advance_deletion(
                            product,
                            runtime,
                            operation_id.clone(),
                            session_id.clone(),
                            revision,
                        )
                        .await;
                    let updated = match result {
                        Ok(updated) => {
                            if updated.state != "completed" {
                                let _ = self
                                    .set_deletion_diagnostic(&operation_id, &session_id, None)
                                    .await;
                            }
                            updated
                        }
                        Err(error) => {
                            let _ = self
                                .set_deletion_diagnostic(
                                    &operation_id,
                                    &session_id,
                                    Some(error.code.clone()),
                                )
                                .await;
                            report.failed.push(DeletionRecoveryFailure {
                                operation_id: operation_id.clone(),
                                session_id: session_id.clone(),
                                code: error.code,
                            });
                            batch_failed = true;
                            break;
                        }
                    };
                    revision = updated.catalog_revision.max(1);
                    if updated.state == "completed" {
                        report.completed += 1;
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            }
            // 失败项仍会排在下一页首部；先返回诊断与重试入口，避免对同一项空转。
            if batch_failed {
                return Ok(report);
            }
        }
    }

    async fn set_deletion_diagnostic(
        &self,
        operation_id: &str,
        session_id: &str,
        diagnostic_code: Option<String>,
    ) -> Result<(), Fault> {
        let storage = self.storage.clone();
        let operation_id = operation_id.to_owned();
        let session_id = session_id.to_owned();
        let updated_at = deletion_timestamp()?;
        aoidos_engine::blocking::run(move || {
            storage.set_run_deletion_diagnostic(
                &operation_id,
                &session_id,
                diagnostic_code.as_deref(),
                &updated_at,
            )
        })
        .await
    }

    async fn advance_deletion(
        &self,
        product: &Product,
        runtime: &Service,
        operation_id: String,
        session_id: String,
        expected_revision: u64,
    ) -> Result<run_catalog::DeletionOperation, Fault> {
        let storage = self.storage.clone();
        let op = operation_id.clone();
        let session = session_id.clone();
        let existing =
            aoidos_engine::blocking::run(move || storage.run_deletion_operation(&op, &session))
                .await;
        let mut operation = match existing {
            Ok(operation) if operation.state != "requested" => operation,
            Ok(_) => {
                product
                    .close_and_freeze_run_deletion(
                        runtime,
                        operation_id.clone(),
                        session_id.clone(),
                        expected_revision,
                    )
                    .await?
            }
            Err(error) if error.code == "app.not-found" => {
                product
                    .close_and_freeze_run_deletion(
                        runtime,
                        operation_id.clone(),
                        session_id.clone(),
                        expected_revision,
                    )
                    .await?
            }
            Err(error) => return Err(error),
        };
        if operation.state == "needs-repair" {
            return Err(Fault::new("app.conflict", "删除操作需要修复后才能继续"));
        }
        if operation.state == "owner-closed" {
            let storage = self.storage.clone();
            let op = operation_id.clone();
            let session = session_id.clone();
            let updated_at = deletion_timestamp()?;
            operation = aoidos_engine::blocking::run(move || {
                storage.quarantine_run_deletion_files(&op, &session, &updated_at)
            })
            .await?;
        }
        if operation.state == "files-quarantined" {
            let storage = self.storage.clone();
            let op = operation_id.clone();
            let session = session_id.clone();
            let updated_at = deletion_timestamp()?;
            operation = aoidos_engine::blocking::run(move || {
                storage.purge_run_deletion_database(&op, &session, &updated_at)
            })
            .await?;
        }
        if operation.state == "database-purged" {
            let storage = self.storage.clone();
            let op = operation_id.clone();
            let session = session_id.clone();
            let updated_at = deletion_timestamp()?;
            operation = aoidos_engine::blocking::run(move || {
                storage.purge_run_deletion_files(&op, &session, &updated_at)
            })
            .await?;
        }
        if operation.state == "files-purged" {
            let storage = self.storage.clone();
            let op = operation_id;
            let session = session_id;
            let updated_at = deletion_timestamp()?;
            operation = aoidos_engine::blocking::run(move || {
                storage.complete_run_deletion(&op, &session, &updated_at)
            })
            .await?;
        }
        Ok(operation)
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

fn deletion_timestamp() -> Result<String, Fault> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| Fault::new("store.database", "读取周目删除时间失败"))
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
