//! 无连接所有权的记忆仓储；所有方法在调用方同一记录 / 数据库临界区内执行。

use crate::{
    error::{Error, Reason, Result, invalid_json, sql},
    model::*,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

mod operations;
mod reconciliation;
mod recovery;
pub use recovery::{RecoveryPage, ScanCursor};
mod materials;
pub use materials::{BatchSnapshot, BatchState, Span};
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

/// 根目录由引擎注入；公开结果不返回磁盘路径。
pub struct Repository {
    root: PathBuf,
}
impl Repository {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
        }
    }
    /// 已应用回执不一致时，该周目的后续写入全部拒绝。
    pub(super) fn ensure_writable(&self, conn: &Connection, run_id: &str) -> Result<()> {
        let frozen: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_run_faults WHERE run_id=?1)",
                [run_id],
                |row| row.get(0),
            )
            .map_err(sql)?;
        if frozen {
            return Err(Error::Rejected(Reason::RecoveryRequired));
        }
        Ok(())
    }
    pub(super) fn freeze_corrupt_run(
        &self,
        conn: &Connection,
        run_id: &str,
        operation_id: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO memory_run_faults(run_id,operation_id,reason) VALUES(?1,?2,'appliedAuditMismatch')",
            params![run_id, operation_id],
        )
        .map_err(sql)?;
        Ok(())
    }
    pub(super) fn freeze_corrupt_run_if_present(
        &self,
        conn: &Connection,
        run_id: &str,
        operation_id: &str,
    ) -> Result<()> {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_runs WHERE run_id=?1)",
                [run_id],
                |row| row.get(0),
            )
            .map_err(sql)?;
        if exists {
            self.freeze_corrupt_run(conn, run_id, operation_id)?;
        }
        Ok(())
    }
    /// # Errors
    /// 周目创建须有已登记会话边界及完整合法策略；同会话不创建第二个周目。
    pub fn create_run(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        policy: Policy,
    ) -> Result<Run> {
        policy.validate()?;
        let boundary = port.boundary()?;
        if !valid_script(&boundary.script_id)
            || !valid_id(&boundary.session_id)
            || boundary.history_revision > MAX_INTEGER
        {
            return Err(reject());
        }
        if let Some(id) = conn
            .query_row(
                "SELECT run_id FROM memory_run_sessions WHERE session_id=?1",
                [&boundary.session_id],
                read_string,
            )
            .optional()
            .map_err(sql)?
        {
            let run = self.run(conn, &id)?;
            // 重开只能读取已冻结策略，不用最新配置替换。
            if run.script_id != boundary.script_id {
                return Err(Error::Corrupt);
            }
            return Ok(run);
        }
        let run = Run {
            run_id: new_id(),
            script_id: boundary.script_id,
            session_id: boundary.session_id,
            policy_hash: hash(&canonical(&policy)?),
            policy,
        };
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        tx.execute(
            "INSERT INTO memory_runs(run_id,script_id,policy_json,policy_hash,clock_history_revision) VALUES(?1,?2,?3,?4,?5)",
            params![
                run.run_id,
                run.script_id,
                canonical(&run.policy)?,
                run.policy_hash.as_slice(),
                boundary.history_revision
            ],
        )
        .map_err(sql)?;
        tx.execute(
            "INSERT INTO memory_run_sessions VALUES(?1,?2)",
            params![run.run_id, run.session_id],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(run)
    }
    /// # Errors
    /// 未知身份为 not-found；未知策略只读拒绝，不套当前默认。
    pub fn run(&self, conn: &Connection, id: &str) -> Result<Run> {
        if !valid_id(id) {
            return Err(reject());
        }
        let row=conn.query_row("SELECT r.script_id,s.session_id,length(r.policy_json),r.policy_json,length(r.policy_hash),r.policy_hash FROM memory_runs r JOIN memory_run_sessions s USING(run_id) WHERE run_id=?1",[id],read_run).optional().map_err(sql)?.ok_or(Error::NotFound)?;
        let policy_bytes = row.2.into_result()?;
        let policy_hash = row.3.into_result()?;
        let policy: Policy = decode_versioned(&policy_bytes, 32 * 1024)?;
        policy.validate()?;
        if hash(&policy_bytes).as_slice() != policy_hash
            || !valid_script(&row.0)
            || !valid_id(&row.1)
        {
            return Err(Error::Corrupt);
        }
        Ok(Run {
            run_id: id.into(),
            script_id: row.0,
            session_id: row.1,
            policy,
            policy_hash: as_hash(policy_hash)?,
        })
    }
    pub(super) fn boundary(
        &self,
        conn: &Connection,
        run_id: &str,
        port: &dyn CausalPort,
    ) -> Result<(Run, Boundary)> {
        let run = self.run(conn, run_id)?;
        let view = port.boundary()?;
        if view.script_id != run.script_id || view.session_id != run.session_id {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        Ok((run, view))
    }
    pub(super) fn valid_sources(
        &self,
        run: &Run,
        sources: &[SourceRef],
        port: &dyn CausalPort,
    ) -> Result<bool> {
        for source in sources {
            source.validate()?;
            if source.run_id != run.run_id
                || source.script_id != run.script_id
                || source.session_id != run.session_id
                || !port.valid_source(source)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(super) fn valid_version(
        &self,
        run: &Run,
        version: &Version,
        port: &dyn CausalPort,
    ) -> Result<bool> {
        if version.policy_hash != run.policy_hash
            || !self.valid_sources(run, &version.sources, port)?
        {
            return Ok(false);
        }
        for evidence in &version.evidence {
            if !port.valid_evidence(evidence)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// # Errors
    /// 不返回未提交占位条目；未知身份、库中非法状态分别拒绝。
    pub fn entry(&self, conn: &Connection, run_id: &str, entry_id: &str) -> Result<Entry> {
        if !valid_id(run_id) || !valid_id(entry_id) {
            return Err(reject());
        }
        let row=conn.query_row("SELECT current_version_id,state_revision,status FROM memory_entries WHERE run_id=?1 AND entry_id=?2 AND current_version_id IS NOT NULL",params![run_id,entry_id],read_entry).optional().map_err(sql)?.ok_or(Error::NotFound)?;
        Ok(Entry {
            entry_id: entry_id.into(),
            current_version_id: row.0,
            state_revision: row.1,
            state: decode_enum(&row.2)?,
        })
    }
    /// 有界查询绑定历史版本，旧分支游标不能覆盖当前结果。
    /// # Errors
    /// 非法预算 / 身份或历史变化返回拒绝；未知策略不写数据。
    pub fn entries(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        run_id: &str,
        history_revision: u64,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Entry>> {
        let (_, view) = self.boundary(conn, run_id, port)?;
        if view.history_revision != history_revision {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        if limit == 0 || limit > 32 || after.is_some_and(|id| !valid_id(id)) {
            return Err(reject());
        }
        let mut statement=conn.prepare("SELECT entry_id FROM memory_entries WHERE run_id=?1 AND current_version_id IS NOT NULL AND entry_id>?2 ORDER BY entry_id LIMIT ?3").map_err(sql)?;
        let ids = statement
            .query_map(params![run_id, after.unwrap_or(""), limit], read_string)
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        ids.iter().map(|id| self.entry(conn, run_id, id)).collect()
    }
    pub(super) fn version_path(
        &self,
        run: &Run,
        entry_id: &str,
        version_id: &str,
    ) -> Result<PathBuf> {
        if !valid_id(entry_id) || !valid_id(version_id) {
            return Err(reject());
        }
        Ok(aoidos_store::paths::join_under_root(
            &self.root,
            &[
                "workspaces",
                &run.script_id,
                "memory",
                "runs",
                &run.run_id,
                "entries",
                entry_id,
                &format!("{version_id}.json"),
            ],
        )?)
    }
    /// 当前正文始终重新核验原始字节；损坏和来源失效不能被当成正常空内容。
    /// # Errors
    /// 条目停用、未知 payload 或 hash / 身份 / 来源错误时拒绝消费。
    pub fn current(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        run_id: &str,
        entry_id: &str,
    ) -> Result<Version> {
        let (run, _) = self.boundary(conn, run_id, port)?;
        let entry = self.entry(conn, run_id, entry_id)?;
        if entry.state == EntryState::Unavailable {
            return Err(Error::Rejected(Reason::RecoveryRequired));
        }
        let version = self.read_version(conn, &run, entry_id, &entry.current_version_id)?;
        if !self.valid_version(&run, &version, port)? {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        Ok(version)
    }
    pub(super) fn read_version(
        &self,
        conn: &Connection,
        run: &Run,
        entry_id: &str,
        version_id: &str,
    ) -> Result<Version> {
        let metadata = self.version_metadata(conn, entry_id, version_id)?;
        if metadata.quarantined {
            return Err(Error::Corrupt);
        }
        let raw = aoidos_store::read::read_text_bounded(
            &self.version_path(run, entry_id, version_id)?,
            MAX_VERSION_BYTES as u32,
        )?
        .ok_or(Error::Corrupt)?;
        if raw.len() != metadata.length || hash(raw.as_bytes()) != metadata.hash {
            return Err(Error::Corrupt);
        }
        let version: Version = decode_versioned(raw.as_bytes(), MAX_VERSION_BYTES)?;
        version.validate()?;
        if version.entry_id != entry_id
            || version.version_id != version_id
            || version.parent_version_id != metadata.parent
            || version.restored_from_version_id != metadata.restored_from
            || version.source_set_hash != metadata.sources_hash
            || version.evidence_set_hash != metadata.evidence_hash
            || version.policy_hash != metadata.policy_hash
            || canonical(&version.subject)? != metadata.subject
            || enum_text(version.change)? != metadata.change
        {
            return Err(Error::Corrupt);
        }
        let sources = self.version_sources(conn, version_id)?;
        let evidence = self.version_evidence(conn, version_id)?;
        if version.sources != sources || set_hash(&version.evidence)? != set_hash(&evidence)? {
            return Err(Error::Corrupt);
        }
        Ok(version)
    }
    pub(super) fn version_metadata(
        &self,
        conn: &Connection,
        entry_id: &str,
        version_id: &str,
    ) -> Result<Metadata> {
        let row = conn.query_row("SELECT parent_version_id,restored_from_version_id,length(body_hash),body_hash,body_length,length(source_set_hash),source_set_hash,length(evidence_set_hash),evidence_set_hash,length(policy_hash),policy_hash,length(subject_json),subject_json,change_kind,quarantined FROM memory_versions WHERE version_id=?1 AND entry_id=?2",params![version_id,entry_id],read_metadata).optional().map_err(sql)?.ok_or(Error::Corrupt)?;
        Ok(Metadata {
            parent: row.parent,
            restored_from: row.restored_from,
            hash: as_hash(row.hash.into_result()?)?,
            length: row.length,
            sources_hash: as_hash(row.sources_hash.into_result()?)?,
            evidence_hash: as_hash(row.evidence_hash.into_result()?)?,
            policy_hash: as_hash(row.policy_hash.into_result()?)?,
            subject: row.subject.into_result()?,
            change: row.change,
            quarantined: row.quarantined,
        })
    }
    /// 计算与单个版本正文一同读取的持久关联数据字节数。
    pub(super) fn version_relation_bytes(&self, conn: &Connection, id: &str) -> Result<usize> {
        let bytes: i64 = conn
            .query_row(
                "SELECT COALESCE((SELECT sum(length(source_json)) FROM memory_version_sources WHERE version_id=?1),0)+COALESCE((SELECT sum(length(evidence_json)) FROM memory_version_evidence WHERE version_id=?1),0)",
                [id],
                |row| row.get(0),
            )
            .map_err(sql)?;
        let bytes = usize::try_from(bytes).map_err(|_| Error::Corrupt)?;
        // 关联行是版本 JSON 的重复索引，合法总长不能超过正文上限。
        if bytes > MAX_VERSION_BYTES {
            return Err(Error::Corrupt);
        }
        Ok(bytes)
    }
    pub(super) fn version_read_bytes(
        &self,
        conn: &Connection,
        entry_id: &str,
        version_id: &str,
    ) -> Result<usize> {
        let body_bytes = self.version_metadata(conn, entry_id, version_id)?.length;
        body_bytes
            .checked_add(self.version_relation_bytes(conn, version_id)?)
            .filter(|bytes| *bytes <= MAX_SCAN_BYTES)
            .ok_or(Error::Corrupt)
    }
    pub(super) fn version_sources(&self, conn: &Connection, id: &str) -> Result<Vec<SourceRef>> {
        let rows = read_blobs(
            conn,
            "SELECT length(source_json),source_json FROM memory_version_sources WHERE version_id=?1 ORDER BY ordinal",
            id,
            128,
        )?;
        let sources = rows
            .iter()
            .map(|bytes| decode(bytes, 32 * 1024))
            .collect::<Result<Vec<SourceRef>>>()?;
        let expected = conn
            .query_row(
                "SELECT length(source_set_hash),source_set_hash FROM memory_versions WHERE version_id=?1",
                [id],
                |row| read_bounded_blob(row, 0, 1, 32),
            )
            .map_err(sql)?
            .into_result()?;
        if sources.is_empty() || set_hash(&sources)?.as_slice() != expected {
            return Err(Error::Corrupt);
        }
        for source in &sources {
            source.validate().map_err(stored_invalid)?;
        }
        Ok(sources)
    }
    pub(super) fn version_evidence(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Vec<KnowledgeEvidence>> {
        let rows = read_blobs(
            conn,
            "SELECT length(evidence_json),evidence_json FROM memory_version_evidence WHERE version_id=?1 ORDER BY evidence_id",
            id,
            128,
        )?;
        let evidence = rows
            .iter()
            .map(|bytes| decode(bytes, 32 * 1024))
            .collect::<Result<Vec<KnowledgeEvidence>>>()?;
        let expected = conn
            .query_row(
                "SELECT length(evidence_set_hash),evidence_set_hash FROM memory_versions WHERE version_id=?1",
                [id],
                |row| read_bounded_blob(row, 0, 1, 32),
            )
            .map_err(sql)?
            .into_result()?;
        if set_hash(&evidence)?.as_slice() != expected {
            return Err(Error::Corrupt);
        }
        Ok(evidence)
    }
}
struct MetadataRow {
    pub parent: Option<String>,
    pub restored_from: Option<String>,
    hash: BoundedBlob,
    pub length: usize,
    sources_hash: BoundedBlob,
    evidence_hash: BoundedBlob,
    policy_hash: BoundedBlob,
    subject: BoundedBlob,
    pub change: String,
    pub quarantined: bool,
}
pub(super) struct Metadata {
    pub parent: Option<String>,
    pub restored_from: Option<String>,
    pub hash: Hash,
    pub length: usize,
    pub sources_hash: Hash,
    pub evidence_hash: Hash,
    pub policy_hash: Hash,
    pub subject: Vec<u8>,
    pub change: String,
    pub quarantined: bool,
}
fn read_metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<MetadataRow> {
    Ok(MetadataRow {
        parent: row.get(0)?,
        restored_from: row.get(1)?,
        hash: read_bounded_blob(row, 2, 3, 32)?,
        length: row.get(4)?,
        sources_hash: read_bounded_blob(row, 5, 6, 32)?,
        evidence_hash: read_bounded_blob(row, 7, 8, 32)?,
        policy_hash: read_bounded_blob(row, 9, 10, 32)?,
        subject: read_bounded_blob(row, 11, 12, 4096)?,
        change: row.get(13)?,
        quarantined: row.get(14)?,
    })
}
fn read_run(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, BoundedBlob, BoundedBlob)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        read_bounded_blob(row, 2, 3, 32 * 1024)?,
        read_bounded_blob(row, 4, 5, 32)?,
    ))
}
fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, u64, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}
pub(super) fn read_string(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get(0)
}
#[derive(Debug)]
pub(super) struct BoundedBlob(Option<Vec<u8>>);
impl BoundedBlob {
    pub(super) fn into_result(self) -> Result<Vec<u8>> {
        self.0.ok_or(Error::Corrupt)
    }
}
pub(super) fn read_bounded_blob(
    row: &rusqlite::Row<'_>,
    length_index: usize,
    blob_index: usize,
    limit: usize,
) -> rusqlite::Result<BoundedBlob> {
    use rusqlite::types::ValueRef;
    let length: i64 = row.get(length_index)?;
    if length < 0 || length as usize > limit {
        return Ok(BoundedBlob(None));
    }
    let bytes = match row.get_ref(blob_index)? {
        ValueRef::Blob(bytes) if length as usize == bytes.len() => Some(bytes.to_vec()),
        _ => None,
    };
    Ok(BoundedBlob(bytes))
}
pub(super) fn read_blobs(
    conn: &Connection,
    query: &str,
    id: &str,
    limit: usize,
) -> Result<Vec<Vec<u8>>> {
    let mut statement = conn.prepare(query).map_err(sql)?;
    let mut rows = statement.query([id]).map_err(sql)?;
    let mut result = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = rows.next().map_err(sql)? {
        let length: i64 = row.get(0).map_err(sql)?;
        if length < 0
            || length as usize > 32 * 1024
            || result.len() >= limit
            || bytes.saturating_add(length as usize) > MAX_SCAN_BYTES
        {
            return Err(Error::Corrupt);
        }
        let value: rusqlite::types::ValueRef<'_> = row.get_ref(1).map_err(sql)?;
        let rusqlite::types::ValueRef::Blob(value) = value else {
            return Err(Error::Corrupt);
        };
        if value.len() != length as usize {
            return Err(Error::Corrupt);
        }
        bytes += value.len();
        result.push(value.to_vec());
    }
    Ok(result)
}
pub(super) fn decode<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T> {
    aoidos_json::decode_bytes(bytes, limit).map_err(invalid_json)
}
pub(super) fn enum_text<T: Serialize>(value: T) -> Result<String> {
    aoidos_json::to_value(value)
        .map_err(invalid_json)?
        .as_str()
        .map(str::to_owned)
        .ok_or(Error::Corrupt)
}
pub(super) fn decode_enum<T: DeserializeOwned>(value: &str) -> Result<T> {
    aoidos_json::from_value(serde_json::Value::String(value.into())).map_err(invalid_json)
}
fn as_hash(bytes: Vec<u8>) -> Result<Hash> {
    bytes.try_into().map_err(bad_hash)
}
fn bad_hash(_: Vec<u8>) -> Error {
    Error::Corrupt
}

pub(super) fn decode_versioned<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T> {
    let value: serde_json::Value = decode(bytes, limit)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or(Error::Corrupt)?;
    if version != 1 {
        return Err(Error::Rejected(Reason::PolicyUnavailable));
    }
    if let Some(algorithm) = value.get("algorithmVersion") {
        let algorithm = algorithm.as_u64().ok_or(Error::Corrupt)?;
        if algorithm != 1 {
            return Err(Error::Rejected(Reason::PolicyUnavailable));
        }
    }
    aoidos_json::from_value(value).map_err(invalid_json)
}

impl Repository {
    /// # Errors
    /// 回退后须完成有界回执恢复再消费时钟，不使用未核验的旧计数。
    pub fn clock(&self, conn: &Connection, port: &dyn CausalPort, run_id: &str) -> Result<u64> {
        let (_, boundary) = self.boundary(conn, run_id, port)?;
        let (clock, history, pending) = conn.query_row("SELECT logical_clock,clock_history_revision,clock_recovery_history FROM memory_runs WHERE run_id=?1", [run_id], read_clock_state).map_err(sql)?;
        if history != boundary.history_revision || pending.is_some() {
            return Err(Error::Rejected(Reason::RecoveryRequired));
        }
        Ok(clock)
    }
}
fn read_clock_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, u64, Option<u64>)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

fn stored_invalid(_: Error) -> Error {
    Error::Corrupt
}
