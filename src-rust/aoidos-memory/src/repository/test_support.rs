//! 仓储契约在真实 SQLite 与临时文件上验证；因果端口只授予夹具登记的来源。

use super::*;
use std::cell::Cell;

pub(super) struct Port {
    pub(super) view: Boundary,
    pub(super) invalid: Cell<bool>,
    pub(super) evidence_invalid: Cell<bool>,
    pub(super) through_record: Cell<u64>,
}
impl CausalPort for Port {
    fn boundary(&self) -> Result<Boundary> {
        Ok(self.view.clone())
    }
    fn valid_source(&self, source: &SourceRef) -> Result<bool> {
        Ok(!self.invalid.get()
            && source.record_seq <= self.through_record.get()
            && source.session_id == self.view.session_id
            && source.body_hash == hash("钟声".as_bytes()))
    }
    fn valid_evidence(&self, _: &KnowledgeEvidence) -> Result<bool> {
        Ok(!self.evidence_invalid.get() && !self.invalid.get())
    }
    fn source_text(&self, source: &SourceRef) -> Result<Option<String>> {
        Ok(self.valid_source(source)?.then(|| "钟声".into()))
    }
    fn completed_round(&self, _: &str, source: &SourceRef) -> Result<bool> {
        self.valid_source(source)
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
        let repo = Repository::new(&root);
        let mut conn = Connection::open(root.join("memory.sqlite")).unwrap();
        // 与产品使用同一 WAL 模式；保留同步要求，避免批量夹具反复创建 rollback journal。
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE store_applied(id TEXT PRIMARY KEY,content_hash TEXT NOT NULL);").unwrap();
        conn.execute_batch(crate::schema::SCHEMA).unwrap();
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
