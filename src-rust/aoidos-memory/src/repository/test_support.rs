//! 仓储契约在真实 SQLite 与临时文件上验证；因果端口只授予夹具登记的来源。

use super::*;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
};

pub(super) struct Port {
    pub(super) view: Boundary,
    pub(super) invalid: Cell<bool>,
    pub(super) evidence_invalid: Cell<bool>,
    pub(super) through_record: Cell<u64>,
    pub(super) invalid_records: RefCell<BTreeSet<u64>>,
    pub(super) source_text_overrides: RefCell<BTreeMap<u64, String>>,
    pub(super) alias_entities: RefCell<BTreeMap<String, EntityAliasContext>>,
    pub(super) canonical_names: RefCell<BTreeMap<(EntityCategory, String, String), String>>,
}
impl CausalPort for Port {
    fn boundary(&self) -> Result<Boundary> {
        Ok(self.view.clone())
    }
    fn valid_source(&self, source: &SourceRef) -> Result<bool> {
        let expected_hash = self
            .source_text_overrides
            .borrow()
            .get(&source.record_seq)
            .map_or_else(|| hash("钟声".as_bytes()), |text| hash(text.as_bytes()));
        Ok(!self.invalid.get()
            && !self.invalid_records.borrow().contains(&source.record_seq)
            && source.record_seq <= self.through_record.get()
            && source.session_id == self.view.session_id
            && source.body_hash == expected_hash)
    }
    fn valid_evidence(&self, _: &KnowledgeEvidence) -> Result<bool> {
        Ok(!self.evidence_invalid.get() && !self.invalid.get())
    }
    fn source_text(&self, source: &SourceRef) -> Result<Option<String>> {
        let text = self
            .source_text_overrides
            .borrow()
            .get(&source.record_seq)
            .cloned()
            .unwrap_or_else(|| "钟声".into());
        Ok(self.valid_source(source)?.then_some(text))
    }
    fn completed_round(&self, _: &str, source: &SourceRef) -> Result<bool> {
        self.valid_source(source)
    }
    fn valid_scene_entry(&self, _: &str, record_seq: u64) -> Result<bool> {
        Ok(record_seq <= self.through_record.get() && !self.invalid.get())
    }
    fn valid_completed_round(
        &self,
        _: &str,
        _: &str,
        entry_record_seq: u64,
        record_seq: u64,
    ) -> Result<bool> {
        Ok(record_seq > entry_record_seq
            && record_seq <= self.through_record.get()
            && !self.invalid.get())
    }
    fn entity_alias_context(&self, entity_id: &str) -> Result<Option<EntityAliasContext>> {
        Ok(self.alias_entities.borrow().get(entity_id).cloned())
    }
    fn canonical_entity_id(
        &self,
        category: EntityCategory,
        scope: &str,
        name: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .canonical_names
            .borrow()
            .get(&(category, scope.into(), name.into()))
            .cloned())
    }
}
pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) repo: Repository,
    pub(super) conn: Connection,
    pub(super) port: Port,
    pub(super) run: Run,
}
pub(super) fn policy() -> Policy {
    Policy {
        version: 1,
        algorithm_version: 1,
        config_version: 1,
        app_config_version: 1,
        algorithm: Algorithm::Exponential,
        base_strength: 200_000,
        reinforcement_gain: 50_000,
        reinforcement_cap: 500_000,
        strength_cap: 1_000_000,
        half_life_rounds: 100,
        repeat_window_rounds: 5,
        repeat_limit: 2,
        revival_cooldown_rounds: 10,
        active_capacity: 100,
        rate_table_version: 1,
        decay_rate: 990_000,
    }
}
impl Fixture {
    pub(super) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("aoidos-memory-test-{}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("memory.sqlite")).unwrap();
        // 与产品使用同一 WAL 模式；保留同步要求，避免批量夹具反复创建 rollback journal。
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        Self::with_connection(root, conn)
    }
    /// 不涉及文件耐久性的用例使用内存数据库，避免把 SQLite WAL 刷盘成本算进规则测试。
    pub(super) fn new_in_memory() -> Self {
        let root = std::env::temp_dir().join(format!("aoidos-memory-test-{}", new_id()));
        let conn = Connection::open_in_memory().unwrap();
        Self::with_connection(root, conn)
    }
    fn with_connection(root: PathBuf, mut conn: Connection) -> Self {
        let repo = Repository::new(&root);
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE store_applied(id TEXT PRIMARY KEY,content_hash TEXT NOT NULL);").unwrap();
        conn.execute_batch(crate::schema::SCHEMA).unwrap();
        conn.execute_batch(crate::schema::BATCH_RECEIPT_MIGRATION)
            .unwrap();
        conn.execute_batch(crate::schema::QUESTION_MIGRATION)
            .unwrap();
        conn.execute_batch(crate::schema::ALIAS_MIGRATION).unwrap();
        conn.execute_batch(crate::schema::CANDIDATE_OPERATION_MIGRATION)
            .unwrap();
        conn.execute_batch(crate::schema::LEGACY_BATCH_COMPLETION_MIGRATION)
            .unwrap();
        conn.execute_batch(crate::schema::LEGACY_BATCH_COMPLETION_BACKFILL)
            .unwrap();
        conn.execute_batch(crate::schema::CYCLE_MIGRATION).unwrap();
        conn.execute_batch(crate::schema::CYCLE_EVENT_MIGRATION)
            .unwrap();
        conn.execute_batch(crate::schema::CONFIG_MIGRATION).unwrap();
        conn.execute_batch(crate::schema::CYCLE_GENERATION_MIGRATION)
            .unwrap();
        conn.execute_batch(crate::schema::CYCLE_GENERATION_BACKFILL)
            .unwrap();
        let port = Port {
            view: Boundary {
                script_id: "mistbell".into(),
                session_id: new_id(),
                history_revision: 0,
                entity_revision: 1,
                evidence_revision: 1,
            },
            invalid: Cell::new(false),
            evidence_invalid: Cell::new(false),
            through_record: Cell::new(MAX_INTEGER),
            invalid_records: RefCell::new(BTreeSet::new()),
            source_text_overrides: RefCell::new(BTreeMap::new()),
            alias_entities: RefCell::new(BTreeMap::new()),
            canonical_names: RefCell::new(BTreeMap::new()),
        };
        let run = repo.create_run(&mut conn, &port, policy()).unwrap();
        Self {
            root,
            repo,
            conn,
            port,
            run,
        }
    }
    pub(super) fn source(&self) -> SourceRef {
        SourceRef {
            script_id: self.run.script_id.clone(),
            run_id: self.run.run_id.clone(),
            session_id: self.run.session_id.clone(),
            record_seq: 1,
            body_hash: hash("钟声".as_bytes()),
            start_byte: 0,
            end_byte: 6,
            mode: Some(InputMode::InCharacter),
        }
    }
    pub(super) fn version(&self) -> Version {
        let sources = vec![self.source()];
        Version {
            version: 1,
            entry_id: new_id(),
            version_id: new_id(),
            parent_version_id: None,
            restored_from_version_id: None,
            subject: Subject::PlayerPreference,
            dimension: Dimension::PlayerPreference,
            summary: "喜欢钟声".into(),
            attitude: Attitude::Believed,
            inferred: false,
            change: ChangeKind::New,
            source_set_hash: set_hash(&sources).unwrap(),
            evidence_set_hash: set_hash::<KnowledgeEvidence>(&[]).unwrap(),
            sources,
            evidence: vec![],
            policy_hash: self.run.policy_hash,
        }
    }
    pub(super) fn plan(&self, version: Version) -> Plan {
        Plan {
            expected_logical_clock: self
                .conn
                .query_row(
                    "SELECT logical_clock FROM memory_runs WHERE run_id=?1",
                    [&self.run.run_id],
                    |row| row.get(0),
                )
                .unwrap(),
            operation_id: new_id(),
            run_id: self.run.run_id.clone(),
            history_revision: self.port.view.history_revision,
            entity_revision: self.port.view.entity_revision,
            evidence_revision: self.port.view.evidence_revision,
            policy_hash: self.run.policy_hash,
            batch_id: None,
            candidate_ids: vec![],
            targets: vec![Target {
                entry_id: version.entry_id.clone(),
                expected: Expected {
                    version_id: version.parent_version_id.clone(),
                    state_revision: u64::from(version.parent_version_id.is_some()),
                },
                next: Some(version),
                state: EntryState::Active,
                effects: vec![],
            }],
            processed_materials: vec![],
            reason: "fixture".into(),
        }
    }
    pub(super) fn received_batch(&mut self, candidate_id: &str) -> BatchSnapshot {
        let source = self.source();
        let material = self
            .repo
            .register_material(&self.conn, &self.port, &source, 0)
            .unwrap();
        let snapshot = BatchSnapshot {
            logical_clock: 0,
            version: 1,
            batch_id: new_id(),
            run_id: self.run.run_id.clone(),
            history_revision: self.port.view.history_revision,
            entity_revision: self.port.view.entity_revision,
            evidence_revision: self.port.view.evidence_revision,
            policy_hash: self.run.policy_hash,
            spans: vec![Span {
                source_id: "s1".into(),
                material_id: material,
                source,
                text: "钟声".into(),
                context: false,
                continuation: false,
            }],
            input: serde_json::json!({}),
        };
        self.repo
            .freeze_batch(&mut self.conn, &self.port, &snapshot)
            .unwrap();
        let turn_id = new_id();
        let request_id = new_id();
        self.repo
            .dispatch_batch(
                &mut self.conn,
                &self.port,
                &snapshot.batch_id,
                &turn_id,
                &request_id,
            )
            .unwrap();
        let response = crate::model::Proposal {
            version: 1,
            candidates: vec![crate::model::Candidate {
                candidate_id: candidate_id.into(),
                classification: crate::model::Classification::New,
                source_ids: vec!["s1".into()],
                subject_ref: crate::model::SubjectRef::PlayerPreference,
                entity_ids: vec![],
                evidence_ids: vec![],
                target_ref: None,
                dimension: Some(crate::model::Dimension::PlayerPreference),
                summary: Some("喜欢钟声".into()),
                attitude: Some(crate::model::Attitude::Believed),
                attitude_evidence_ids: vec![],
                inferred: Some(false),
                reason: "稳定偏好".into(),
            }],
            cycle_marks: vec![],
        };
        self.repo
            .receive_batch(
                &mut self.conn,
                &snapshot.batch_id,
                &turn_id,
                &request_id,
                &serde_json::to_vec(&response).unwrap(),
            )
            .unwrap();
        snapshot
    }
    pub(super) fn commit(&mut self, plan: &Plan) {
        self.repo.prepare(&mut self.conn, &self.port, plan).unwrap();
        self.repo
            .write_versions(&mut self.conn, &plan.operation_id)
            .unwrap();
        self.repo
            .apply(&mut self.conn, &self.port, &plan.operation_id)
            .unwrap();
    }
    pub(super) fn path(&self, version: &Version) -> PathBuf {
        self.repo
            .version_path(&self.run, &version.entry_id, &version.version_id)
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Windows 不能删除仍被 SQLite 连接占用的数据库。
        self.conn = Connection::open_in_memory().unwrap();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
pub(super) fn rejected<T>(result: Result<T>, reason: Reason) {
    assert!(matches!(result, Err(Error::Rejected(actual)) if actual == reason));
}
