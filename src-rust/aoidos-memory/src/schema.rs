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

/// 核验迁移后的领域表 / 索引定义，不把同名但缺约束的旧库当合法 manifest。
/// # Errors
/// 缺少或改变已登记结构为 corrupt，SQL 读取错误保持存储类别。
pub fn verify(conn: &rusqlite::Connection) -> aoidos_store::error::Result<()> {
    use rusqlite::OptionalExtension;
    for statement in SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|sql| !sql.is_empty())
    {
        let name = statement
            .split_whitespace()
            .nth(2)
            .ok_or_else(invalid_schema)?;
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
    Ok(())
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
    }

    #[test]
    fn definitions_accept_whitespace_only_and_propagate_sql_errors() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&SCHEMA.replace('\n', "  ")).unwrap();
        verify(&conn).unwrap();
        conn.execute_batch(
            "PRAGMA writable_schema=ON; UPDATE sqlite_schema SET sql=NULL WHERE name='memory_runs'",
        )
        .unwrap();
        assert!(verify(&conn).is_err());
    }

    #[test]
    fn definition_reader_reports_wrong_column_type() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let error = conn.query_row("SELECT 1", [], read_definition).unwrap_err();
        assert!(matches!(error, rusqlite::Error::InvalidColumnType(..)));
    }
}
