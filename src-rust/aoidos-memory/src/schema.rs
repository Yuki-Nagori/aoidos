//! 业务表只通过共享迁移列表创建；连接与迁移生命周期由引擎持有。

/// 029 初始 schema；后续门控 / 设置表由对应任务追加迁移，不抢占编号。
pub const SCHEMA: &str = r#"
CREATE TABLE memory_runs (
 run_id TEXT PRIMARY KEY CHECK(length(run_id)=36),
 script_id TEXT NOT NULL CHECK(length(script_id) BETWEEN 1 AND 64),
 policy_json BLOB NOT NULL,
 policy_hash BLOB NOT NULL CHECK(length(policy_hash)=32),
 logical_clock INTEGER NOT NULL DEFAULT 0 CHECK(logical_clock BETWEEN 0 AND 9007199254740991),
 clock_history_revision INTEGER NOT NULL DEFAULT 0,
 clock_recovery_history INTEGER,
 clock_recovery_after INTEGER NOT NULL DEFAULT 0,
 clock_recovery_total INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE memory_run_sessions (
 run_id TEXT PRIMARY KEY REFERENCES memory_runs(run_id),
 session_id TEXT NOT NULL UNIQUE CHECK(length(session_id)=36)
);
CREATE TABLE memory_run_faults (
 run_id TEXT PRIMARY KEY REFERENCES memory_runs(run_id),
 operation_id TEXT NOT NULL CHECK(length(operation_id)=36),
 reason TEXT NOT NULL CHECK(reason IN ('appliedAuditMismatch'))
);
CREATE TABLE memory_materials (
 material_id TEXT PRIMARY KEY CHECK(length(material_id)=36),
 run_id TEXT NOT NULL REFERENCES memory_runs(run_id),
 session_id TEXT NOT NULL,
 record_seq INTEGER NOT NULL CHECK(record_seq BETWEEN 1 AND 9007199254740991),
 unit_ordinal INTEGER NOT NULL CHECK(unit_ordinal BETWEEN 0 AND 9007199254740991),
 source_json BLOB NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('eligible','deferred','processed','rejected')),
 UNIQUE(run_id,session_id,record_seq,unit_ordinal)
);
CREATE TABLE memory_batches (
 batch_id TEXT PRIMARY KEY CHECK(length(batch_id)=36),
 run_id TEXT NOT NULL REFERENCES memory_runs(run_id),
 history_revision INTEGER NOT NULL CHECK(history_revision BETWEEN 0 AND 9007199254740991),
 input_digest BLOB NOT NULL CHECK(length(input_digest)=32),
 policy_hash BLOB NOT NULL CHECK(length(policy_hash)=32),
 snapshot_json BLOB NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('queued','dispatched','received','completed','rejected','unknown')),
 result_hash BLOB CHECK(result_hash IS NULL OR length(result_hash)=32)
);
CREATE TABLE memory_spans (
 batch_id TEXT NOT NULL REFERENCES memory_batches(batch_id),
 source_id TEXT NOT NULL,
 material_id TEXT NOT NULL REFERENCES memory_materials(material_id),
 PRIMARY KEY(batch_id,source_id)
);
CREATE TABLE memory_entries (
 entry_id TEXT PRIMARY KEY CHECK(length(entry_id)=36),
 run_id TEXT NOT NULL REFERENCES memory_runs(run_id),
 current_version_id TEXT,
 state_revision INTEGER NOT NULL CHECK(state_revision BETWEEN 0 AND 9007199254740991),
 status TEXT NOT NULL CHECK(status IN ('active','archived','unavailable')),
 restore_status TEXT NOT NULL DEFAULT 'active' CHECK(restore_status IN ('active','archived')),
 recovery_cursor TEXT,
 recovery_history_revision INTEGER,
 recovery_visited BLOB,
 effect_history_revision INTEGER,
 effect_recovery_history INTEGER,
 effect_recovery_after INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE memory_versions (
 version_id TEXT PRIMARY KEY CHECK(length(version_id)=36),
 entry_id TEXT NOT NULL REFERENCES memory_entries(entry_id),
 parent_version_id TEXT REFERENCES memory_versions(version_id),
 restored_from_version_id TEXT REFERENCES memory_versions(version_id),
 body_hash BLOB NOT NULL CHECK(length(body_hash)=32),
 body_length INTEGER NOT NULL CHECK(body_length BETWEEN 1 AND 32768),
 source_set_hash BLOB NOT NULL CHECK(length(source_set_hash)=32),
 evidence_set_hash BLOB NOT NULL CHECK(length(evidence_set_hash)=32),
 policy_hash BLOB NOT NULL CHECK(length(policy_hash)=32),
 subject_json BLOB NOT NULL,
 change_kind TEXT NOT NULL CHECK(change_kind IN ('new','supplement','correction','restore')),
 quarantined INTEGER NOT NULL DEFAULT 0 CHECK(quarantined IN (0,1))
);
CREATE TABLE memory_version_sources (
 version_id TEXT NOT NULL REFERENCES memory_versions(version_id),
 ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 127),
 source_json BLOB NOT NULL,
 PRIMARY KEY(version_id,ordinal)
);
CREATE TABLE memory_version_evidence (
 version_id TEXT NOT NULL REFERENCES memory_versions(version_id),
 evidence_id TEXT NOT NULL,
 evidence_json BLOB NOT NULL,
 PRIMARY KEY(version_id,evidence_id)
);
CREATE TABLE memory_operations (
 operation_id TEXT PRIMARY KEY CHECK(length(operation_id)=36),
 run_id TEXT NOT NULL REFERENCES memory_runs(run_id),
 plan_hash BLOB NOT NULL CHECK(length(plan_hash)=32),
 input_history_revision INTEGER NOT NULL CHECK(input_history_revision BETWEEN 0 AND 9007199254740991),
 state TEXT NOT NULL CHECK(state IN ('prepared','ready','applied','rejected','quarantined')),
 plan_json BLOB NOT NULL,
 reason TEXT NOT NULL DEFAULT '' CHECK(length(reason)<=512)
);
CREATE TABLE memory_operation_targets (
 operation_id TEXT NOT NULL REFERENCES memory_operations(operation_id),
 entry_id TEXT NOT NULL,
 expected_version_id TEXT,
 expected_state_revision INTEGER NOT NULL CHECK(expected_state_revision BETWEEN 0 AND 9007199254740990),
 next_version_id TEXT,
 next_state_revision INTEGER NOT NULL CHECK(next_state_revision BETWEEN 1 AND 9007199254740991),
 PRIMARY KEY(operation_id,entry_id)
);
CREATE TABLE memory_effects (
 effect_id TEXT PRIMARY KEY CHECK(length(effect_id)=36),
 operation_id TEXT NOT NULL REFERENCES memory_operations(operation_id),
 entry_id TEXT NOT NULL REFERENCES memory_entries(entry_id),
 run_id TEXT NOT NULL REFERENCES memory_runs(run_id),
 session_id TEXT NOT NULL,
 record_seq INTEGER NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('gain','revival')),
 logical_position INTEGER NOT NULL CHECK(logical_position BETWEEN 0 AND 9007199254740991),
 gain INTEGER NOT NULL CHECK(gain BETWEEN 0 AND 1000000),
 source_json BLOB NOT NULL,
 valid INTEGER NOT NULL DEFAULT 1 CHECK(valid IN (0,1)),
 UNIQUE(run_id,entry_id,session_id,record_seq,kind)
);
CREATE TABLE memory_processed (
 material_id TEXT PRIMARY KEY REFERENCES memory_materials(material_id),
 operation_id TEXT NOT NULL REFERENCES memory_operations(operation_id)
);
CREATE TABLE memory_clock_receipts (
 run_id TEXT NOT NULL REFERENCES memory_runs(run_id),
 round_id TEXT NOT NULL CHECK(length(round_id)=36),
 source_json BLOB NOT NULL,
 logical_position INTEGER NOT NULL CHECK(logical_position BETWEEN 1 AND 9007199254740991),
 valid INTEGER NOT NULL DEFAULT 1 CHECK(valid IN (0,1)),
 PRIMARY KEY(run_id,round_id)
);
CREATE TABLE memory_cleanup (
 version_id TEXT PRIMARY KEY,
 run_id TEXT NOT NULL REFERENCES memory_runs(run_id),
 entry_id TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('pending','removed'))
);
CREATE INDEX memory_entries_page ON memory_entries(run_id,status,entry_id);
CREATE INDEX memory_operations_page ON memory_operations(run_id,state,operation_id);
CREATE INDEX memory_materials_page ON memory_materials(run_id,status,material_id);
CREATE INDEX memory_versions_entry ON memory_versions(entry_id,version_id);
CREATE INDEX memory_effects_entry ON memory_effects(entry_id,valid,effect_id);
CREATE INDEX memory_effects_operation ON memory_effects(operation_id);
CREATE INDEX memory_processed_operation ON memory_processed(operation_id);
"#;

/// 030 批次回执：物理请求身份、原始响应和本地逐项结果与状态原子持久化。
pub const BATCH_RECEIPT_MIGRATION: &str = r#"
CREATE TABLE memory_batch_receipts (
 batch_id TEXT PRIMARY KEY REFERENCES memory_batches(batch_id),
 turn_id TEXT NOT NULL UNIQUE CHECK(length(turn_id)=36),
 request_id TEXT NOT NULL UNIQUE CHECK(length(request_id)=36),
 response_hash BLOB CHECK(response_hash IS NULL OR length(response_hash)=32),
 response_json BLOB CHECK(response_json IS NULL OR length(response_json)<=65536),
 completion_json BLOB CHECK(completion_json IS NULL OR length(completion_json)<=65536),
 completion_hash BLOB CHECK(completion_hash IS NULL OR length(completion_hash)=32),
 error_code TEXT CHECK(error_code IS NULL OR length(error_code)<=64),
 CHECK((response_hash IS NULL)=(response_json IS NULL)),
 CHECK((completion_hash IS NULL)=(completion_json IS NULL)),
 CHECK(completion_json IS NULL OR response_json IS NOT NULL)
)
"#;

/// 030 持久问题队列：候选变化递增 revision，回答只写入当前 pending 版本。
pub const QUESTION_MIGRATION: &str = r#"
CREATE TABLE memory_questions (
 question_id TEXT PRIMARY KEY CHECK(length(question_id)=36),
 run_id TEXT NOT NULL REFERENCES memory_runs(run_id),
 question_key BLOB NOT NULL CHECK(length(question_key)=32),
 revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
 history_revision INTEGER NOT NULL CHECK(history_revision BETWEEN 0 AND 9007199254740991),
 entity_revision INTEGER NOT NULL CHECK(entity_revision BETWEEN 0 AND 9007199254740991),
 evidence_revision INTEGER NOT NULL CHECK(evidence_revision BETWEEN 0 AND 9007199254740991),
 draft_json BLOB NOT NULL CHECK(length(draft_json)<=65536),
 status TEXT NOT NULL CHECK(status IN ('pending','resolved','invalidated')),
 answer_json BLOB CHECK(answer_json IS NULL OR length(answer_json)<=8192),
 CHECK((status='resolved')=(answer_json IS NOT NULL))
);
CREATE UNIQUE INDEX memory_questions_pending_key
 ON memory_questions(run_id,question_key) WHERE status='pending';
CREATE INDEX memory_questions_page
 ON memory_questions(run_id,status,question_id);
"#;

/// 030 将模型候选与同组操作绑定；完成结果只能声明已应用的持久操作。
pub const CANDIDATE_OPERATION_MIGRATION: &str = r#"
CREATE TABLE memory_batch_candidate_operations (
 batch_id TEXT NOT NULL REFERENCES memory_batches(batch_id),
 candidate_id TEXT NOT NULL CHECK(length(candidate_id) BETWEEN 1 AND 32),
 operation_id TEXT NOT NULL REFERENCES memory_operations(operation_id),
 PRIMARY KEY(batch_id,candidate_id),
 UNIQUE(operation_id,candidate_id)
);
CREATE INDEX memory_batch_candidate_operations_operation
 ON memory_batch_candidate_operations(operation_id,candidate_id);
"#;

/// 将升级前已经终结的回执标记为历史记录；其旧格式没有候选—操作映射可安全重建。
pub const LEGACY_BATCH_COMPLETION_MIGRATION: &str = r#"
CREATE TABLE memory_batch_legacy_completions (
 batch_id TEXT PRIMARY KEY REFERENCES memory_batches(batch_id)
)
"#;

/// 该一次性数据迁移只保留旧完成结果的可读性，不为旧 `applied` 推造操作关联。
pub const LEGACY_BATCH_COMPLETION_BACKFILL: &str = r#"
INSERT OR IGNORE INTO memory_batch_legacy_completions(batch_id)
 SELECT r.batch_id FROM memory_batch_receipts r
 JOIN memory_batches b USING(batch_id)
 WHERE b.status='completed' AND r.completion_json IS NOT NULL
"#;

/// 核验迁移后的领域表 / 索引定义，不把同名但缺约束的旧库当合法 manifest。
/// # Errors
/// 缺少或改变已登记结构为 corrupt，SQL 读取错误保持存储类别。
pub fn verify(conn: &rusqlite::Connection) -> aoidos_store::error::Result<()> {
    use rusqlite::OptionalExtension;
    for migration in [
        SCHEMA,
        BATCH_RECEIPT_MIGRATION,
        QUESTION_MIGRATION,
        CANDIDATE_OPERATION_MIGRATION,
        LEGACY_BATCH_COMPLETION_MIGRATION,
    ] {
        for statement in migration
            .split(';')
            .map(str::trim)
            .filter(|sql| !sql.is_empty())
        {
            let name = schema_object_name(statement).ok_or_else(invalid_schema)?;
            let actual: Option<String> = conn
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name=?1",
                    [name],
                    read_definition,
                )
                .optional()
                .map_err(aoidos_store::db::sqlite_error)?;
            if actual.as_deref().map(normalize_sql) != Some(normalize_sql(statement)) {
                return Err(invalid_schema());
            }
        }
    }
    Ok(())
}
fn schema_object_name(statement: &str) -> Option<&str> {
    let mut words = statement.split_whitespace();
    match (words.next()?, words.next()?) {
        ("CREATE", "UNIQUE") => {
            if words.next()? != "INDEX" {
                return None;
            }
            words.next()
        }
        ("CREATE", _) => {
            let mut words = statement.split_whitespace();
            words.nth(2)
        }
        _ => None,
    }
}
fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn read_definition(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get(0)
}
fn invalid_schema() -> aoidos_store::error::StoreError {
    aoidos_store::error::StoreError::Corrupt("memory schema mismatch".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_rejects_missing_and_weakened_domain_schema() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        assert!(verify(&conn).is_err());
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(BATCH_RECEIPT_MIGRATION).unwrap();
        conn.execute_batch(QUESTION_MIGRATION).unwrap();
        conn.execute_batch(CANDIDATE_OPERATION_MIGRATION).unwrap();
        conn.execute_batch(LEGACY_BATCH_COMPLETION_MIGRATION)
            .unwrap();
        conn.execute_batch(LEGACY_BATCH_COMPLETION_BACKFILL)
            .unwrap();
        verify(&conn).unwrap();
        conn.execute_batch("DROP INDEX memory_entries_page")
            .unwrap();
        assert!(verify(&conn).is_err());
        conn.execute_batch("CREATE INDEX memory_entries_page ON memory_entries(run_id,entry_id)")
            .unwrap();
        assert!(verify(&conn).is_err());
        let weakened = rusqlite::Connection::open_in_memory().unwrap();
        weakened
            .execute_batch(&SCHEMA.replace("CHECK(length(run_id)=36)", "CHECK(length(run_id)>0)"))
            .unwrap();
        assert!(verify(&weakened).is_err());

        let missing_receipts = rusqlite::Connection::open_in_memory().unwrap();
        missing_receipts.execute_batch(SCHEMA).unwrap();
        missing_receipts
            .execute_batch(BATCH_RECEIPT_MIGRATION)
            .unwrap();
        missing_receipts.execute_batch(QUESTION_MIGRATION).unwrap();
        missing_receipts
            .execute_batch(CANDIDATE_OPERATION_MIGRATION)
            .unwrap();
        missing_receipts
            .execute_batch(LEGACY_BATCH_COMPLETION_MIGRATION)
            .unwrap();
        missing_receipts
            .execute_batch(LEGACY_BATCH_COMPLETION_BACKFILL)
            .unwrap();
        missing_receipts
            .execute_batch("DROP TABLE memory_batch_receipts")
            .unwrap();
        assert!(verify(&missing_receipts).is_err());

        let missing_questions = rusqlite::Connection::open_in_memory().unwrap();
        missing_questions.execute_batch(SCHEMA).unwrap();
        missing_questions
            .execute_batch(BATCH_RECEIPT_MIGRATION)
            .unwrap();
        missing_questions.execute_batch(QUESTION_MIGRATION).unwrap();
        missing_questions
            .execute_batch(CANDIDATE_OPERATION_MIGRATION)
            .unwrap();
        missing_questions
            .execute_batch(LEGACY_BATCH_COMPLETION_MIGRATION)
            .unwrap();
        missing_questions
            .execute_batch(LEGACY_BATCH_COMPLETION_BACKFILL)
            .unwrap();
        missing_questions
            .execute_batch("DROP TABLE memory_questions")
            .unwrap();
        assert!(verify(&missing_questions).is_err());

        let missing_candidate_links = rusqlite::Connection::open_in_memory().unwrap();
        missing_candidate_links.execute_batch(SCHEMA).unwrap();
        missing_candidate_links
            .execute_batch(BATCH_RECEIPT_MIGRATION)
            .unwrap();
        missing_candidate_links
            .execute_batch(QUESTION_MIGRATION)
            .unwrap();
        missing_candidate_links
            .execute_batch(CANDIDATE_OPERATION_MIGRATION)
            .unwrap();
        missing_candidate_links
            .execute_batch(LEGACY_BATCH_COMPLETION_MIGRATION)
            .unwrap();
        missing_candidate_links
            .execute_batch(LEGACY_BATCH_COMPLETION_BACKFILL)
            .unwrap();
        missing_candidate_links
            .execute_batch("DROP TABLE memory_batch_candidate_operations")
            .unwrap();
        assert!(verify(&missing_candidate_links).is_err());
    }

    #[test]
    fn definitions_accept_whitespace_only_and_propagate_sql_errors() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&SCHEMA.replace('\n', "  ")).unwrap();
        conn.execute_batch(BATCH_RECEIPT_MIGRATION).unwrap();
        conn.execute_batch(QUESTION_MIGRATION).unwrap();
        conn.execute_batch(CANDIDATE_OPERATION_MIGRATION).unwrap();
        conn.execute_batch(LEGACY_BATCH_COMPLETION_MIGRATION)
            .unwrap();
        conn.execute_batch(LEGACY_BATCH_COMPLETION_BACKFILL)
            .unwrap();
        verify(&conn).unwrap();
        conn.execute_batch(
            "PRAGMA writable_schema=ON; UPDATE sqlite_schema SET sql=NULL WHERE name='memory_runs'",
        )
        .unwrap();
        assert!(verify(&conn).is_err());
    }

    #[test]
    fn object_name_parser_rejects_non_schema_and_malformed_unique_statements() {
        assert_eq!(schema_object_name("CREATE UNIQUE TABLE x"), None);
        assert_eq!(schema_object_name("DROP TABLE x"), None);
        assert_eq!(schema_object_name("CREATE"), None);
        assert_eq!(schema_object_name("CREATE UNIQUE INDEX"), None);
        assert_eq!(
            schema_object_name("CREATE UNIQUE INDEX x ON y(z)"),
            Some("x")
        );
    }

    #[test]
    fn definition_reader_reports_wrong_column_type() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let error = conn.query_row("SELECT 1", [], read_definition).unwrap_err();
        assert!(matches!(error, rusqlite::Error::InvalidColumnType(..)));
    }
}
