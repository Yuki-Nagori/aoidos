//! 共享业务存储所有者；壳只注入根目录和事件端口，不持有领域连接。

use crate::{
    fault::Fault,
    migration::{Migration, MigrationEvents},
    preferences::Preferences,
    record::session::RecordEvents,
    records::Records,
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// 供桌面壳保持 engine 所有权边界使用的记忆配置 IPC 类型。
pub use aoidos_memory::model::{
    AppConfig, Classification, ConfigSaved, MemoryQuestion, PolicyOverrides, PolicyParameters,
    QuestionAnswer, QuestionCandidate, QuestionDraft, QuestionIdentity, QuestionKind,
    QuestionResolution, QuestionStatus, ScriptConfig, SourceRef,
};
pub use aoidos_memory::repository::{QuestionCursor, QuestionPage};

/// 翻页锚定历史修订；旧页面不能混入回退或新事件后的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryEntryCursor {
    pub run_id: String,
    pub history_revision: u64,
    pub snapshot_max_sequence: u64,
    pub after_sequence: u64,
}

/// 记忆条目的有界快照及可选后续页游标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntryPage {
    pub entries: Vec<aoidos_memory::model::Entry>,
    pub next_cursor: Option<MemoryEntryCursor>,
    pub history_revision: u64,
}

/// 条目索引及其当前不可变正文版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntryDetails {
    pub entry: aoidos_memory::model::Entry,
    pub version: aoidos_memory::model::Version,
}

/// 素材扫描游标固定周目、历史修订和首次扫描的记录上界。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryMaterialCursor {
    pub run_id: String,
    pub history_revision: u64,
    pub snapshot_max_record_seq: u64,
    pub after_record_seq: u64,
}

/// 一页素材登记结果；正文留在本地记录，不通过 IPC 返回。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMaterialPage {
    pub run_id: String,
    pub history_revision: u64,
    pub eligible_count: usize,
    pub next_cursor: Option<MemoryMaterialCursor>,
}

/// Migration position is persisted in SQLite `user_version`; only append entries.
const STORAGE_MIGRATIONS: &[&str] = &[
    aoidos_store::applied::SCHEMA,
    aoidos_memory::schema::SCHEMA,
    aoidos_theme::schema::SCHEMA,
    aoidos_locale::schema::SCHEMA,
    aoidos_billing::schema::SCHEMA,
    aoidos_billing::schema::CACHE_WRITE_MIGRATION,
    aoidos_billing::schema::SETTLEMENT_RECOVERY_MIGRATION,
    aoidos_billing::schema::SETTLEMENT_DIAGNOSTICS_MIGRATION,
    aoidos_memory::schema::BATCH_RECEIPT_MIGRATION,
    aoidos_memory::schema::QUESTION_MIGRATION,
    aoidos_memory::schema::CANDIDATE_OPERATION_MIGRATION,
    aoidos_memory::schema::LEGACY_BATCH_COMPLETION_MIGRATION,
    aoidos_memory::schema::LEGACY_BATCH_COMPLETION_BACKFILL,
    crate::game::catalog::ENTITY_REGISTRY_MIGRATION,
    aoidos_memory::schema::ALIAS_MIGRATION,
    aoidos_memory::schema::CYCLE_MIGRATION,
    aoidos_memory::schema::CYCLE_EVENT_MIGRATION,
    aoidos_memory::schema::CONFIG_MIGRATION,
    aoidos_memory::schema::CYCLE_GENERATION_MIGRATION,
    aoidos_memory::schema::CYCLE_GENERATION_BACKFILL,
    aoidos_memory::schema::MATERIAL_CURSOR_MIGRATION,
    aoidos_memory::schema::EFFECT_EVIDENCE_PROGRESS_MIGRATION,
    aoidos_memory::schema::MATERIAL_POSITION_MIGRATION,
    crate::run_catalog::SCHEMA,
];

pub struct Storage {
    pub records: Arc<Records>,
    pub migration: Arc<Migration>,
    pub preferences: Arc<Preferences>,
    database: Mutex<Option<rusqlite::Connection>>,
    memory: aoidos_memory::repository::Repository,
}
impl Storage {
    /// 共享迁移编号只有一个来源；失败保留诊断但拒绝业务入口。
    pub fn new(
        root: &Path,
        migration_events: Arc<dyn MigrationEvents>,
        record_events: Arc<dyn RecordEvents>,
    ) -> Self {
        let migration = Arc::new(Migration::new(migration_events));
        let database = migration
            .open_verified(
                &root.join("storage.sqlite"),
                STORAGE_MIGRATIONS,
                |connection| {
                    let valid:i64=connection.query_row("SELECT count(*)=2 AND sum(name='id' AND type='TEXT' AND pk=1)=1 AND sum(name='content_hash' AND type='TEXT' AND \"notnull\"=1)=1 FROM pragma_table_info('store_applied')",[],|row|row.get(0)).map_err(aoidos_store::db::sqlite_error)?;
                    if valid!=1 {return Err(aoidos_store::error::StoreError::Corrupt("business applied schema mismatch".into()));}
                    aoidos_memory::schema::verify(connection)?;
                    crate::game::catalog::verify_entity_registry_schema(connection)?;
                    crate::run_catalog::verify(connection)?;
                    aoidos_theme::schema::verify(connection)?;
                    aoidos_locale::schema::verify(connection)?;
                    aoidos_billing::schema::verify(connection)
                        .map_err(aoidos_store::db::sqlite_error)?;
                    Ok(())
                },
            )
            .ok();
        Self {
            records: Arc::new(Records::new(root.to_owned(), record_events)),
            migration,
            preferences: Arc::new(Preferences::new(root)),
            database: Mutex::new(database),
            memory: aoidos_memory::repository::Repository::new(root),
        }
    }
    /// 在会话锁 → 数据库锁内绑定活动场景目录和当前实体快照，再借用记忆仓储。
    /// 回调不能递归取得这两个锁或 await。
    /// # Errors
    /// 未登记会话、待恢复状态、目录版本不符、证据策略不完整或持久化失败时拒绝。
    pub fn with_memory<T>(
        &self,
        session_id: &str,
        scene_catalog: &crate::game::catalog::SceneCatalog,
        job: impl FnOnce(
            &aoidos_memory::repository::Repository,
            &mut rusqlite::Connection,
            &crate::record::memory::SourcePort<'_>,
        ) -> aoidos_memory::error::Result<T>,
    ) -> aoidos_memory::error::Result<T> {
        self.with_memory_context(
            session_id,
            scene_catalog,
            |repository, connection, port, _, _| job(repository, connection, port),
        )
    }

    /// 在同一锁区内向可信 engine 工作流提供活动会话和完整实体快照。
    /// 壳层仍只使用 `with_memory`，避免把内部恢复与身份上下文暴露成 IPC。
    /// # Errors
    /// 会话、恢复状态、目录或存储无效时拒绝执行回调。
    pub(crate) fn with_memory_context<T>(
        &self,
        session_id: &str,
        scene_catalog: &crate::game::catalog::SceneCatalog,
        job: impl FnOnce(
            &aoidos_memory::repository::Repository,
            &mut rusqlite::Connection,
            &crate::record::memory::SourcePort<'_>,
            &crate::record::session::Session,
            &crate::game::catalog::EntityDirectory,
        ) -> aoidos_memory::error::Result<T>,
    ) -> aoidos_memory::error::Result<T> {
        let session = self.records.get(session_id).map_err(memory_context_error)?;
        let session = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_memory_context_for_session(&session, scene_catalog, job)
    }

    /// 调用方已持有会话锁时复用同一会话快照，避免回调递归获取非重入锁。
    ///
    /// 锁顺序仍为会话 → 数据库；不得在回调中再次获取会话锁或跨越 `await`。
    pub(crate) fn with_memory_context_locked<T>(
        &self,
        session: &crate::record::session::Session,
        scene_catalog: &crate::game::catalog::SceneCatalog,
        job: impl FnOnce(
            &aoidos_memory::repository::Repository,
            &mut rusqlite::Connection,
            &crate::record::memory::SourcePort<'_>,
            &crate::record::session::Session,
            &crate::game::catalog::EntityDirectory,
        ) -> aoidos_memory::error::Result<T>,
    ) -> aoidos_memory::error::Result<T> {
        self.with_memory_context_for_session(session, scene_catalog, job)
    }

    fn with_memory_context_for_session<T>(
        &self,
        session: &crate::record::session::Session,
        scene_catalog: &crate::game::catalog::SceneCatalog,
        job: impl FnOnce(
            &aoidos_memory::repository::Repository,
            &mut rusqlite::Connection,
            &crate::record::memory::SourcePort<'_>,
            &crate::record::session::Session,
            &crate::game::catalog::EntityDirectory,
        ) -> aoidos_memory::error::Result<T>,
    ) -> aoidos_memory::error::Result<T> {
        let seed = crate::game::assets::entity_directory(&session.header().script_id)
            .map_err(memory_context_error)?;
        let mut database = self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let connection = database
            .as_mut()
            .ok_or(aoidos_memory::error::Error::Rejected(
                aoidos_memory::error::Reason::RecoveryRequired,
            ))?;
        let entity_directory = crate::game::catalog::EntityRegistry
            .snapshot(connection, session, &seed)
            .map_err(memory_context_error)?;
        let port = crate::record::memory::SourcePort::with_evidence_context(
            session,
            &entity_directory,
            scene_catalog,
        )?;
        job(&self.memory, connection, &port, session, &entity_directory)
    }

    /// 在不取会话锁时借用记忆仓储；仅用于已有会话锁的执行路径预写待处理事件。
    /// # Errors
    /// 存储未开放、事件不合法或 SQLite 操作失败时返回稳定错误。
    pub(crate) fn with_memory_repository<T>(
        &self,
        job: impl FnOnce(
            &aoidos_memory::repository::Repository,
            &mut rusqlite::Connection,
        ) -> aoidos_memory::error::Result<T>,
    ) -> Result<T, Fault> {
        self.with_database(|connection| job(&self.memory, connection).map_err(memory_config_error))
    }

    /// 在会话持久化后冻结其已确认的记忆策略；重开时保留现有快照。
    /// # Errors
    /// 配置不合法、会话边界未登记或存储不可用时拒绝初始化。
    pub fn initialize_memory_run(
        &self,
        session_id: &str,
        scene_catalog: &crate::game::catalog::SceneCatalog,
    ) -> Result<aoidos_memory::model::Run, Fault> {
        use aoidos_memory::model::CausalPort;
        // 已冻结周目独立于当前设置；重开或崩溃恢复不能被新配置阻断。
        match self.memory_run_for_session(session_id) {
            Ok(run) => return Ok(run),
            Err(error) if error.code == "app.not-found" => {}
            Err(error) => return Err(error),
        }
        self.with_memory(session_id, scene_catalog, |repository, connection, port| {
            let boundary = port.boundary()?;
            let config = repository.script_config(connection, &boundary.script_id)?;
            let policy = config
                .effective
                .into_policy(config.config_version, config.app_config_version)?;
            repository.create_run(connection, port, policy)
        })
        .map_err(memory_config_error)
    }

    /// 返回会话对应的已冻结周目；设置变化不会替换该策略。
    /// # Errors
    /// 没有已初始化周目的会话返回 not-found。
    pub fn memory_run_for_session(
        &self,
        session_id: &str,
    ) -> Result<aoidos_memory::model::Run, Fault> {
        let session = self.records.get(session_id)?;
        let script_id = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .header()
            .script_id
            .clone();
        let run = self.with_database(|connection| {
            self.memory
                .run_for_session(connection, session_id)
                .map_err(memory_config_error)
        })?;
        if run.session_id != session_id || run.script_id != script_id {
            return Err(memory_config_error(aoidos_memory::error::Error::Corrupt));
        }
        Ok(run)
    }

    /// 读取与指定周目绑定的批次终态；私有模型原始响应不离开仓储边界。
    /// # Errors
    /// 周目或批次身份不匹配、回执损坏及存储故障均拒绝返回状态。
    pub fn memory_batch_status(
        &self,
        run_id: &str,
        batch_id: &str,
    ) -> Result<MemoryBatchStatus, Fault> {
        let (run, catalog) = self.memory_catalog_for_run(run_id)?;
        self.with_memory(
            &run.session_id,
            &catalog,
            |repository, connection, _port| {
                let (snapshot, state) = repository.batch(connection, batch_id)?;
                if snapshot.run_id != run_id {
                    return Err(aoidos_memory::error::Error::NotFound);
                }
                let receipt = match state {
                    aoidos_memory::repository::BatchState::Queued => None,
                    _ => Some(repository.batch_receipt(connection, batch_id)?),
                };
                let completion = receipt
                    .as_ref()
                    .and_then(|receipt| receipt.completion.as_deref())
                    .map(|bytes| {
                        aoidos_json::decode_bytes(bytes, aoidos_memory::model::MAX_PLAN_BYTES)
                            .map_err(|_| aoidos_memory::error::Error::Corrupt)
                    })
                    .transpose()?
                    .map(
                        |mut completion: aoidos_memory::repository::BatchCompletion| {
                            for outcome in &mut completion.outcomes {
                                outcome.reason_code = outcome
                                    .reason_code
                                    .as_deref()
                                    .map(memory_batch_diagnostic)
                                    .map(str::to_owned);
                            }
                            completion
                        },
                    );
                let error_code = receipt
                    .as_ref()
                    .and_then(|receipt| receipt.error_code.as_deref())
                    .map(memory_batch_diagnostic);
                Ok(MemoryBatchStatus {
                    batch_id: snapshot.batch_id,
                    run_id: snapshot.run_id,
                    history_revision: snapshot.history_revision,
                    state,
                    completion,
                    diagnostic_code: error_code,
                })
            },
        )
        .map_err(memory_config_error)
    }

    /// 查找周目最新的未终结批次，供窗口重开后恢复现有处理状态。
    /// # Errors
    /// 周目、批次或持久回执无法通过仓储完整性校验时拒绝返回。
    pub fn active_memory_batch_status(
        &self,
        run_id: &str,
    ) -> Result<Option<MemoryBatchStatus>, Fault> {
        let (run, catalog) = self.memory_catalog_for_run(run_id)?;
        let batch_id = self
            .with_memory(&run.session_id, &catalog, |repository, connection, _| {
                repository.latest_pending_batch(connection, run_id)
            })
            .map_err(memory_config_error)?;
        batch_id
            .map(|batch_id| self.memory_batch_status(run_id, &batch_id))
            .transpose()
    }

    /// 读取仍处于 queued 的冻结提案，供显式发送入口构造 Provider 输入。
    /// 原始响应和正文只保留在本地，不由此状态视图导出。
    /// # Errors
    /// runId / batchId 不匹配、状态已变化或快照损坏时拒绝返回。
    pub fn queued_memory_batch(
        &self,
        run_id: &str,
        batch_id: &str,
    ) -> Result<
        (
            aoidos_memory::model::Run,
            crate::game::catalog::SceneCatalog,
            aoidos_memory::repository::BatchSnapshot,
        ),
        Fault,
    > {
        let (run, catalog) = self.memory_catalog_for_run(run_id)?;
        let snapshot = self
            .with_memory(&run.session_id, &catalog, |repository, connection, _| {
                let (snapshot, state) = repository.batch(connection, batch_id)?;
                if snapshot.run_id != run_id
                    || state != aoidos_memory::repository::BatchState::Queued
                {
                    return Err(aoidos_memory::error::Error::Rejected(
                        aoidos_memory::error::Reason::StaleVersion,
                    ));
                }
                Ok(snapshot)
            })
            .map_err(memory_config_error)?;
        Ok((run, catalog, snapshot))
    }

    /// 按固定历史上界扫描当前分支，将有效原文登记为稳定素材；不冻结批次或发送请求。
    /// 每页最多扫描 `MAX_SCAN` 条 / `MAX_SCAN_BYTES`，后续页游标绑定 run 和 historyRevision。
    /// # Errors
    /// 旧游标、来源恢复状态、超出扫描预算或记忆存储失败时拒绝。
    pub fn collect_memory_materials(
        &self,
        run_id: &str,
        cursor: Option<MemoryMaterialCursor>,
    ) -> Result<MemoryMaterialPage, Fault> {
        use aoidos_memory::model::{CausalPort, MAX_INTEGER};

        let (run, catalog) = self.memory_catalog_for_run(run_id)?;
        self.with_memory_context(
            &run.session_id,
            &catalog,
            |repository, connection, port, session, _directory| {
                let boundary = port.boundary()?;
                if let Some(cursor) = &cursor
                    && (cursor.run_id != run_id
                        || cursor.history_revision != boundary.history_revision
                        || cursor.after_record_seq > cursor.snapshot_max_record_seq
                        || cursor.snapshot_max_record_seq > MAX_INTEGER)
                {
                    return Err(aoidos_memory::error::Error::Rejected(
                        aoidos_memory::error::Reason::StaleVersion,
                    ));
                }
                let snapshot_max_record_seq = cursor.as_ref().map_or_else(
                    || session.next_sequence().saturating_sub(1),
                    |cursor| cursor.snapshot_max_record_seq,
                );
                let page = port.material_source_page(
                    run_id,
                    cursor.as_ref().map(|cursor| cursor.after_record_seq),
                    snapshot_max_record_seq,
                )?;
                let mut eligible_count = 0;
                for source in page.sources {
                    let material_id = repository.register_material(
                        connection,
                        port,
                        &source.source,
                        source.ordinal,
                    )?;
                    if repository.material_is_collectible(connection, port, run_id, &material_id)? {
                        eligible_count += 1;
                    }
                }
                let next_cursor =
                    page.next_record_seq
                        .map(|after_record_seq| MemoryMaterialCursor {
                            run_id: run_id.into(),
                            history_revision: boundary.history_revision,
                            snapshot_max_record_seq,
                            after_record_seq,
                        });
                Ok(MemoryMaterialPage {
                    run_id: run_id.into(),
                    history_revision: boundary.history_revision,
                    eligible_count,
                    next_cursor,
                })
            },
        )
        .map_err(memory_config_error)
    }

    /// 在会话与数据库共享锁区内重建提案白名单，并原子冻结可信输入。
    /// 调用方只能提交候选批次身份和来源；实体、证据与刻痕权限均由当前引擎状态装配。
    /// # Errors
    /// 批次边界过期、来源无效、上下文超限或事务写入失败时拒绝冻结。
    pub fn freeze_memory_batch(
        &self,
        catalog: &crate::game::catalog::SceneCatalog,
        mut snapshot: aoidos_memory::repository::BatchSnapshot,
    ) -> Result<aoidos_memory::model::Hash, Fault> {
        let run = self.with_database(|connection| {
            self.memory
                .run(connection, &snapshot.run_id)
                .map_err(memory_config_error)
        })?;
        self.with_memory_context(
            &run.session_id,
            catalog,
            |repository, connection, port, session, directory| {
                let current_run = repository.run_for_session(connection, &run.session_id)?;
                if current_run.run_id != snapshot.run_id {
                    return Err(aoidos_memory::error::Error::Rejected(
                        aoidos_memory::error::Reason::StaleVersion,
                    ));
                }
                MemoryProposalContext {
                    connection,
                    repository,
                    source_port: port,
                    session,
                    directory,
                    catalog,
                    run: &current_run,
                }
                .attach(&mut snapshot)?;
                repository.freeze_batch(connection, port, &snapshot)
            },
        )
        .map_err(memory_config_error)
    }

    /// 从已登记的最早可处理素材组装一批稳定快照；此步骤只准备数据，不占用素材或发送请求。
    /// 每批最多选 16 个素材单元，后续显式发送仍须调用 `freeze_memory_batch` 重新核验并占用。
    /// # Errors
    /// 没有可处理素材、记录来源无效或上下文 / Prompt 超限时拒绝构造快照。
    pub fn prepare_collectible_memory_batch(
        &self,
        run_id: &str,
    ) -> Result<aoidos_memory::repository::BatchSnapshot, Fault> {
        use aoidos_memory::model::{CausalPort, new_id};

        const MAX_BATCH_MATERIALS: usize = 16;
        const MATERIAL_PAGE: usize = aoidos_memory::repository::MAX_COLLECTIBLE_MATERIAL_SCAN;

        let (run, catalog) = self.memory_catalog_for_run(run_id)?;
        self.with_memory_context(
            &run.session_id,
            &catalog,
            |repository, connection, port, session, directory| {
                if repository
                    .latest_pending_batch(connection, run_id)?
                    .is_some()
                {
                    return Err(aoidos_memory::error::Error::Rejected(
                        aoidos_memory::error::Reason::StaleVersion,
                    ));
                }
                let current_run = repository.run_for_session(connection, &run.session_id)?;
                if current_run.run_id != run_id {
                    return Err(aoidos_memory::error::Error::Rejected(
                        aoidos_memory::error::Reason::StaleVersion,
                    ));
                }
                let mut cursor = None;
                let mut spans = Vec::with_capacity(MAX_BATCH_MATERIALS);
                for _ in 0..16 {
                    let page = repository.collectible_materials_page(
                        connection,
                        port,
                        run_id,
                        cursor,
                        MATERIAL_PAGE,
                    )?;
                    for material in page.materials {
                        let Some(text) = port.source_text(&material.source)? else {
                            return Err(aoidos_memory::error::Error::Rejected(
                                aoidos_memory::error::Reason::InvalidSource,
                            ));
                        };
                        let number = spans.len() + 1;
                        spans.push(aoidos_memory::repository::Span {
                            source_id: format!("s{number}"),
                            material_id: material.material_id,
                            source: material.source,
                            text,
                            context: false,
                            continuation: material.ordinal > 0,
                        });
                        if spans.len() == MAX_BATCH_MATERIALS {
                            break;
                        }
                    }
                    if spans.len() == MAX_BATCH_MATERIALS {
                        break;
                    }
                    cursor = page.next;
                    if cursor.is_none() {
                        break;
                    }
                }
                if spans.is_empty() {
                    return Err(aoidos_memory::error::Error::NotFound);
                }
                let boundary = port.boundary()?;
                let mut snapshot = aoidos_memory::repository::BatchSnapshot {
                    logical_clock: repository.clock(connection, port, run_id)?,
                    version: 1,
                    batch_id: new_id(),
                    run_id: run_id.into(),
                    history_revision: boundary.history_revision,
                    entity_revision: boundary.entity_revision,
                    evidence_revision: boundary.evidence_revision,
                    policy_hash: current_run.policy_hash,
                    spans,
                    input: serde_json::Value::Object(serde_json::Map::new()),
                };
                MemoryProposalContext {
                    connection,
                    repository,
                    source_port: port,
                    session,
                    directory,
                    catalog: &catalog,
                    run: &current_run,
                }
                .attach(&mut snapshot)?;
                Ok(snapshot)
            },
        )
        .map_err(memory_config_error)
    }

    /// 读取当前历史版本下的待确认问题；失效问题由仓储持久标记。
    /// # Errors
    /// 游标过期、来源失效读取失败或周目身份不匹配时拒绝返回结果。
    pub fn memory_questions(
        &self,
        run_id: &str,
        cursor: Option<aoidos_memory::repository::QuestionCursor>,
        limit: usize,
    ) -> Result<
        (
            aoidos_memory::repository::QuestionPage,
            std::collections::BTreeMap<String, String>,
        ),
        Fault,
    > {
        let (run, catalog) = self.memory_catalog_for_run(run_id)?;
        self.with_memory(&run.session_id, &catalog, |repository, connection, port| {
            let page =
                repository.pending_questions(connection, port, run_id, cursor.as_ref(), limit)?;
            let mut names = std::collections::BTreeMap::new();
            for question in &page.questions {
                for candidate in &question.draft.candidates {
                    if let aoidos_memory::model::QuestionCandidate::Entity { entity_id } = candidate
                        && let Some(name) = port.canonical_entity_name(entity_id)?
                    {
                        names.insert(entity_id.clone(), name);
                    }
                }
            }
            Ok((page, names))
        })
        .map_err(memory_config_error)
    }

    /// 按当前 revision 回答待确认问题；实体别名和答案由仓储原子提交。
    /// # Errors
    /// 旧回答、来源 / 目录变化或非法候选不修改既有问题。
    pub fn resolve_memory_question(
        &self,
        run_id: &str,
        resolution: aoidos_memory::model::QuestionResolution,
    ) -> Result<
        (
            aoidos_memory::model::MemoryQuestion,
            std::collections::BTreeMap<String, String>,
        ),
        Fault,
    > {
        let (run, catalog) = self.memory_catalog_for_run(run_id)?;
        self.with_memory(&run.session_id, &catalog, |repository, connection, port| {
            let question = repository.resolve_question(connection, port, run_id, resolution)?;
            let mut names = std::collections::BTreeMap::new();
            for candidate in &question.draft.candidates {
                if let aoidos_memory::model::QuestionCandidate::Entity { entity_id } = candidate
                    && let Some(name) = port.canonical_entity_name(entity_id)?
                {
                    names.insert(entity_id.clone(), name);
                }
            }
            Ok((question, names))
        })
        .map_err(memory_config_error)
    }

    /// 按稳定 ID 有界分页读取当前条目索引；游标绑定当前因果历史修订。
    /// # Errors
    /// 过期游标或非法页大小拒绝读取，不混合不同历史分支。
    pub fn memory_entries(
        &self,
        run_id: &str,
        cursor: Option<MemoryEntryCursor>,
        limit: usize,
    ) -> Result<MemoryEntryPage, Fault> {
        use aoidos_memory::model::CausalPort;
        let (run, catalog) = self.memory_catalog_for_run(run_id)?;
        self.with_memory(&run.session_id, &catalog, |repository, connection, port| {
            let history_revision = port.boundary()?.history_revision;
            if cursor.as_ref().is_some_and(|cursor| {
                cursor.run_id != run_id || cursor.history_revision != history_revision
            }) {
                return Err(aoidos_memory::error::Error::Rejected(
                    aoidos_memory::error::Reason::StaleVersion,
                ));
            }
            let cursor = cursor
                .as_ref()
                .map(|cursor| (cursor.snapshot_max_sequence, cursor.after_sequence));
            let (entries, snapshot_max_sequence, next_sequence) = repository.entries_page(
                connection,
                port,
                run_id,
                history_revision,
                cursor,
                limit,
            )?;
            let next_cursor = next_sequence.map(|after_sequence| MemoryEntryCursor {
                run_id: run_id.into(),
                history_revision,
                snapshot_max_sequence,
                after_sequence,
            });
            Ok(MemoryEntryPage {
                entries,
                next_cursor,
                history_revision,
            })
        })
        .map_err(memory_config_error)
    }

    /// 读取指定的不可变历史版本；过期来源不能被历史查询绕过。
    /// # Errors
    /// 条目不存在、版本已变化或正文来源不可用时返回稳定错误。
    pub fn memory_entry(
        &self,
        run_id: &str,
        entry_id: &str,
        expected_version_id: &str,
    ) -> Result<MemoryEntryDetails, Fault> {
        let (run, catalog) = self.memory_catalog_for_run(run_id)?;
        self.with_memory(&run.session_id, &catalog, |repository, connection, port| {
            let entry = repository.entry(connection, run_id, entry_id)?;
            let version =
                repository.version(connection, port, run_id, entry_id, expected_version_id)?;
            Ok(MemoryEntryDetails { entry, version })
        })
        .map_err(memory_config_error)
    }

    pub fn memory_catalog_for_run(
        &self,
        run_id: &str,
    ) -> Result<
        (
            aoidos_memory::model::Run,
            crate::game::catalog::SceneCatalog,
        ),
        Fault,
    > {
        let run = self.with_database(|connection| {
            self.memory
                .run(connection, run_id)
                .map_err(memory_config_error)
        })?;
        let session = self.records.get(&run.session_id)?;
        let session_script = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .header()
            .script_id
            .clone();
        if session_script != run.script_id {
            return Err(memory_config_error(aoidos_memory::error::Error::Corrupt));
        }
        let scenario = crate::game::assets::scenario(&run.script_id)?;
        let entities = crate::game::assets::entity_directory(&run.script_id)?;
        let catalog = crate::game::builtin::memory_scene_catalog(scenario.title, entities)?;
        Ok((run, catalog))
    }

    /// 回合生产者退出后释放 SQLite 和记录句柄，偏好已在写入时确认。
    /// # Errors
    /// 记录关闭同步失败返回 store.*；不删除存档。
    pub fn close(&self) -> Result<(), Fault> {
        self.database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.records.close_all()
    }
    /// # Errors
    /// 初始化 / 迁移失败的业务服务返回 not-ready；诊断查询不经过此门禁。
    pub fn ready(&self) -> Result<(), Fault> {
        self.with_database(|_| Ok(()))
    }

    /// 返回 SQLite 权威周目目录的一页摘要；不打开记录文件或恢复任何状态。
    /// # Errors
    /// 游标过期、页大小非法或目录数据损坏时拒绝返回。
    pub fn run_catalog_page(
        &self,
        cursor: Option<&crate::run_catalog::RunCursor>,
        page_size: usize,
    ) -> Result<crate::run_catalog::RunPage, Fault> {
        self.with_database(|connection| crate::run_catalog::page(connection, cursor, page_size))
    }

    /// 登记已成功创建并初始化的周目；账本范围固定为 sessionId。
    /// # Errors
    /// 身份冲突、无效 profile 快照或持久化失败时拒绝登记。
    pub fn register_new_run(&self, run: &crate::run_catalog::NewRun) -> Result<(), Fault> {
        self.with_database(|connection| crate::run_catalog::register_new(connection, run))
    }

    /// 按目录 revision CAS 为旧周目绑定公开 profile 快照。
    /// # Errors
    /// 周目状态或 revision 已变化、profile 无效或数据库不可用时拒绝。
    pub fn bind_run_profile(
        &self,
        session_id: &str,
        expected_revision: u64,
        profile: &aoidos_llm::config::LlmProfile,
    ) -> Result<u64, Fault> {
        self.with_database(|connection| {
            crate::run_catalog::bind_profile(connection, session_id, expected_revision, profile)
        })
    }
    /// 登记解释器借用唯一连接；领域条件、投影与 applied 标记在回调内同事务提交。
    /// 回调不递归获取本存储锁；阻塞调用由引擎 blocking 边界调度，不在持锁期间 await。
    /// # Errors
    /// 未开放 / 已关闭为 not-ready；原领域错误原样返回，不另建连接或迁移生命周期。
    pub fn with_database<T>(
        &self,
        job: impl FnOnce(&mut rusqlite::Connection) -> Result<T, Fault>,
    ) -> Result<T, Fault> {
        let mut database = self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let connection = database.as_mut().ok_or_else(not_ready)?;
        job(connection)
    }

    /// 读取应用记忆默认配置，不创建新的数据库连接。
    /// # Errors
    /// 存储未开放或持久配置损坏时返回稳定错误。
    pub fn memory_app_config(&self) -> Result<aoidos_memory::model::AppConfig, Fault> {
        self.with_database(|connection| {
            self.memory
                .app_config(connection)
                .map_err(memory_config_error)
        })
    }

    /// CAS 保存应用记忆默认参数，并保留每个已确认版本。
    /// # Errors
    /// 版本冲突、参数不合法或存储失败时拒绝保存。
    pub fn save_memory_app_config(
        &self,
        expected_version: u64,
        parameters: aoidos_memory::model::PolicyParameters,
    ) -> Result<aoidos_memory::model::ConfigSaved, Fault> {
        self.with_database(|connection| {
            self.memory
                .save_app_config(connection, expected_version, parameters)
                .map_err(memory_config_error)
        })
    }

    /// 读取剧本记忆覆盖及按字段继承得到的有效参数。
    /// # Errors
    /// 剧本身份无效、配置损坏或存储未开放时失败。
    pub fn memory_script_config(
        &self,
        script_id: &str,
    ) -> Result<aoidos_memory::model::ScriptConfig, Fault> {
        crate::game::assets::scenario(script_id)?;
        self.with_database(|connection| {
            self.memory
                .script_config(connection, script_id)
                .map_err(memory_config_error)
        })
    }

    /// CAS 保存剧本覆盖，并同时核对应用默认版本。
    /// # Errors
    /// 任一版本冲突、合并策略不合法或存储失败时拒绝保存。
    pub fn save_memory_script_config(
        &self,
        script_id: &str,
        expected_version: u64,
        expected_app_version: u64,
        overrides: aoidos_memory::model::PolicyOverrides,
    ) -> Result<aoidos_memory::model::ConfigSaved, Fault> {
        crate::game::assets::scenario(script_id)?;
        self.with_database(|connection| {
            self.memory
                .save_script_config(
                    connection,
                    script_id,
                    expected_version,
                    expected_app_version,
                    overrides,
                )
                .map_err(memory_config_error)
        })
    }
}

struct MemoryProposalContext<'a> {
    connection: &'a rusqlite::Connection,
    repository: &'a aoidos_memory::repository::Repository,
    source_port: &'a crate::record::memory::SourcePort<'a>,
    session: &'a crate::record::session::Session,
    directory: &'a crate::game::catalog::EntityDirectory,
    catalog: &'a crate::game::catalog::SceneCatalog,
    run: &'a aoidos_memory::model::Run,
}

impl MemoryProposalContext<'_> {
    fn attach(
        &self,
        snapshot: &mut aoidos_memory::repository::BatchSnapshot,
    ) -> aoidos_memory::error::Result<()> {
        let context = crate::record::memory::build_proposal_context(
            crate::record::memory::ProposalContextInput {
                connection: self.connection,
                repository: self.repository,
                catalog: self.catalog,
                run: self.run,
                session: self.session,
                directory: self.directory,
                batch: snapshot,
                source_port: self.source_port,
            },
        )?;
        snapshot.input = aoidos_json::to_value(serde_json::json!({
            "proposalContext": context,
        }))
        .map_err(|_| {
            aoidos_memory::error::Error::Rejected(aoidos_memory::error::Reason::InvalidSchema)
        })?;
        // 冻结素材前验证最终转义结果可用于两种 Provider 输入，避免 queued 批次无法发送。
        for mode in [
            aoidos_llm::provider::RequestMode::Completion,
            aoidos_llm::provider::RequestMode::Chat,
        ] {
            crate::record::memory::prompt::build_memory_prompt(snapshot, mode)?;
        }
        Ok(())
    }
}

/// 可安全跨 IPC 返回的批次视图；不包含私有 prompt 或供应商原始响应。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryBatchStatus {
    pub batch_id: String,
    pub run_id: String,
    pub history_revision: u64,
    pub state: aoidos_memory::repository::BatchState,
    pub completion: Option<aoidos_memory::repository::BatchCompletion>,
    pub diagnostic_code: Option<&'static str>,
}

fn memory_batch_diagnostic(code: &str) -> &'static str {
    match code {
        "response.schema" => "response.schema",
        "response.capacity" => "response.capacity",
        "response.version" => "response.version",
        "policy.capacity_blocked" => "policy.capacity_blocked",
        "source.invalid" => "source.invalid",
        "memory.candidate.review" => "memory.candidate.review",
        "memory.candidate.shared_target" => "memory.candidate.shared_target",
        "memory.candidate.duplicate_anchor" => "memory.candidate.duplicate_anchor",
        "memory.operation.rejected" => "memory.operation.rejected",
        "memory.receipt-failed" => "memory.receipt-failed",
        _ => "memory.unknown",
    }
}
fn not_ready() -> Fault {
    Fault::new("app.not-ready", "业务存储未开放，请查看迁移诊断")
}

fn memory_context_error(error: Fault) -> aoidos_memory::error::Error {
    use aoidos_memory::error::{Error, Reason};
    match error.code.as_str() {
        "app.not-found" => Error::NotFound,
        "app.bad-request" => Error::Rejected(Reason::InvalidSchema),
        _ => Error::Rejected(Reason::RecoveryRequired),
    }
}

pub(crate) fn memory_config_error(error: aoidos_memory::error::Error) -> Fault {
    let code = error.code();
    let message = match code.as_str() {
        "app.bad-request" => "记忆参数不合法或配置已变化",
        "app.not-found" => "记忆配置不存在",
        "app.not-ready" => "记忆配置当前不可用",
        "store.corrupt" => "记忆配置数据损坏",
        _ if code.starts_with("store.") => "记忆配置存储失败",
        _ => "记忆配置读取失败",
    };
    Fault::new(code, message)
}

#[cfg(test)]
mod tests;
