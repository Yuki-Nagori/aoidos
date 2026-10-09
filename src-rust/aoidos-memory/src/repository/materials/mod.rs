//! 稳定素材及批次短引用映射；恢复复用原始身份，不重新切片或发请求。

use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Span {
    pub source_id: String,
    pub material_id: String,
    pub source: SourceRef,
    pub text: String,
    pub context: bool,
    pub continuation: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchSnapshot {
    pub logical_clock: u64,
    pub version: u32,
    pub batch_id: String,
    pub run_id: String,
    pub history_revision: u64,
    pub entity_revision: u64,
    pub evidence_revision: u64,
    pub policy_hash: Hash,
    pub spans: Vec<Span>,
    /// 已冻结输入包含完整映射及旧条目 / 证据快照；下阶段组装，不存自由 SQL 或路径。
    pub input: serde_json::Value,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BatchState {
    Queued,
    Dispatched,
    Received,
    Completed,
    Rejected,
    Unknown,
}
impl Repository {
    /// # Errors
    /// 同原记录 / ordinal 不同区间为身份冲突，不能通过重新切片制造新素材。
    pub fn register_material(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        source: &SourceRef,
        ordinal: u64,
    ) -> Result<String> {
        let (run, _) = self.boundary(conn, &source.run_id, port)?;
        self.ensure_writable(conn, &run.run_id)?;
        if ordinal > MAX_INTEGER || !self.valid_sources(&run, std::slice::from_ref(source), port)? {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let encoded = canonical(source)?;
        let previous=conn.query_row("SELECT material_id,length(source_json),source_json FROM memory_materials WHERE run_id=?1 AND session_id=?2 AND record_seq=?3 AND unit_ordinal=?4",params![source.run_id,source.session_id,source.record_seq,ordinal],read_material).optional().map_err(sql)?;
        if let Some((id, bytes)) = previous {
            if bytes.into_result()? != encoded {
                return Err(Error::Corrupt);
            }
            return Ok(id);
        }
        let duplicate = conn
            .query_row(
                "SELECT material_id FROM memory_materials WHERE run_id=?1 AND source_json=?2",
                params![source.run_id, encoded],
                read_string,
            )
            .optional()
            .map_err(sql)?;
        if duplicate.is_some() {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let id = new_id();
        conn.execute(
            "INSERT INTO memory_materials VALUES(?1,?2,?3,?4,?5,?6,'eligible')",
            params![
                id,
                source.run_id,
                source.session_id,
                source.record_seq,
                ordinal,
                encoded
            ],
        )
        .map_err(sql)?;
        Ok(id)
    }
    pub(super) fn material_source(
        &self,
        conn: &Connection,
        run_id: &str,
        id: &str,
    ) -> Result<SourceRef> {
        let (raw,session,seq)=conn.query_row("SELECT length(source_json),source_json,session_id,record_seq FROM memory_materials WHERE run_id=?1 AND material_id=?2",params![run_id,id],read_material_source).optional().map_err(sql)?.ok_or(Error::NotFound)?;
        let raw = raw.into_result()?;
        let source: SourceRef = decode(&raw, 32 * 1024)?;
        source.validate().map_err(stored_source_error)?;
        if source.run_id != run_id || source.session_id != session || source.record_seq != seq {
            return Err(Error::Corrupt);
        }
        Ok(source)
    }
    /// # Errors
    /// 映射、来源或冻结前提不合法则拒绝；同 batchId 不同输入不得替换。
    pub fn freeze_batch(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        snapshot: &BatchSnapshot,
    ) -> Result<Hash> {
        let (run, view) = self.boundary(conn, &snapshot.run_id, port)?;
        self.ensure_writable(conn, &run.run_id)?;
        if snapshot.version != 1
            || !valid_id(&snapshot.batch_id)
            || snapshot.history_revision != view.history_revision
            || snapshot.entity_revision != view.entity_revision
            || snapshot.evidence_revision != view.evidence_revision
            || snapshot.policy_hash != run.policy_hash
            || snapshot.logical_clock != self.clock(conn, port, &snapshot.run_id)?
            || snapshot.spans.is_empty()
            || snapshot.spans.len() > 128
        {
            return Err(reject());
        }
        let mut names = BTreeSet::new();
        for span in &snapshot.spans {
            let number = span
                .source_id
                .strip_prefix('s')
                .and_then(|id| id.parse::<usize>().ok())
                .ok_or_else(reject)?;
            if !(1..=128).contains(&number)
                || span.source_id != format!("s{number}")
                || !names.insert(&span.source_id)
                || !valid_id(&span.material_id)
                || span.text.is_empty()
                || span.text.len() > if span.context { 1024 } else { 2048 }
            {
                return Err(reject());
            }
            let source = self.material_source(conn, &run.run_id, &span.material_id)?;
            if source != span.source
                || !self.valid_sources(&run, std::slice::from_ref(&source), port)?
                || port.source_text(&source)?.as_deref() != Some(&span.text)
            {
                return Err(Error::Rejected(Reason::InvalidSource));
            }
        }
        let raw = canonical(snapshot)?;
        if raw.len() > 64 * 1024 {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let digest = hash(&raw);
        if let Some(previous) = conn
            .query_row(
                "SELECT length(snapshot_json),snapshot_json FROM memory_batches WHERE batch_id=?1",
                [&snapshot.batch_id],
                |row| read_bounded_blob(row, 0, 1, 64 * 1024),
            )
            .optional()
            .map_err(sql)?
        {
            if previous.into_result()? != raw {
                return Err(Error::Corrupt);
            }
            return Ok(digest);
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        tx.execute("INSERT INTO memory_batches(batch_id,run_id,history_revision,input_digest,policy_hash,snapshot_json,status) VALUES(?1,?2,?3,?4,?5,?6,'queued')",params![snapshot.batch_id,snapshot.run_id,snapshot.history_revision,digest.as_slice(),snapshot.policy_hash.as_slice(),raw]).map_err(sql)?;
        for span in &snapshot.spans {
            tx.execute(
                "INSERT INTO memory_spans VALUES(?1,?2,?3)",
                params![snapshot.batch_id, span.source_id, span.material_id],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)?;
        Ok(digest)
    }
    /// # Errors
    /// 原批次 hash 或短引用映射不一致为损坏，不使用当前批次替换旧身份。
    pub fn batch(&self, conn: &Connection, id: &str) -> Result<(BatchSnapshot, BatchState)> {
        if !valid_id(id) {
            return Err(reject());
        }
        let (raw, digest, state) = conn
            .query_row(
                "SELECT length(snapshot_json),snapshot_json,length(input_digest),input_digest,status FROM memory_batches WHERE batch_id=?1",
                [id],
                read_batch,
            )
            .optional()
            .map_err(sql)?
            .ok_or(Error::NotFound)?;
        let raw = raw.into_result()?;
        let digest = digest.into_result()?;
        if digest != hash(&raw) {
            return Err(Error::Corrupt);
        }
        let snapshot: BatchSnapshot = decode_versioned(&raw, 64 * 1024)?;
        if snapshot.batch_id != id {
            return Err(Error::Corrupt);
        }
        for span in &snapshot.spans {
            let mapped = conn
                .query_row(
                    "SELECT material_id FROM memory_spans WHERE batch_id=?1 AND source_id=?2",
                    params![id, span.source_id],
                    read_string,
                )
                .optional()
                .map_err(sql)?;
            if mapped.as_deref() != Some(&span.material_id)
                || self.material_source(conn, &snapshot.run_id, &span.material_id)? != span.source
            {
                return Err(Error::Corrupt);
            }
        }
        Ok((snapshot, decode_enum(&state)?))
    }
    /// 只登记已确认的场内完成回执；回退时钟由有效来源重建，不按墙钟增加。
    /// # Errors
    /// 调用方须从可信 completed 场内回合构造来源，重复 round 不重新计时。
    pub fn complete_round(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        run_id: &str,
        round_id: &str,
        source: &SourceRef,
    ) -> Result<u64> {
        let (run, _) = self.boundary(conn, run_id, port)?;
        self.ensure_writable(conn, &run.run_id)?;
        self.clock(conn, port, run_id)?;
        if !valid_id(round_id)
            || source.mode == Some(InputMode::OutOfCharacter)
            || !self.valid_sources(&run, std::slice::from_ref(source), port)?
            || !port.completed_round(round_id, source)?
        {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        if let Some((raw,position))=tx.query_row("SELECT length(source_json),source_json,logical_position FROM memory_clock_receipts WHERE run_id=?1 AND round_id=?2",params![run_id,round_id],read_receipt).optional().map_err(sql)? {
            let raw = raw.into_result()?;
            if raw!=canonical(source)? {return Err(Error::Corrupt);}
            return Ok(position);
        }
        let clock: u64 = tx
            .query_row(
                "SELECT logical_clock FROM memory_runs WHERE run_id=?1",
                [run_id],
                read_clock,
            )
            .map_err(sql)?;
        if clock >= MAX_INTEGER {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let next = clock + 1;
        tx.execute(
            "INSERT INTO memory_clock_receipts(run_id,round_id,source_json,logical_position) VALUES(?1,?2,?3,?4)",
            params![run_id, round_id, canonical(source)?, next],
        )
        .map_err(sql)?;
        tx.execute(
            "UPDATE memory_runs SET logical_clock=?1 WHERE run_id=?2",
            params![next, run_id],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(next)
    }
}
fn read_material(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, BoundedBlob)> {
    Ok((row.get(0)?, read_bounded_blob(row, 1, 2, 32 * 1024)?))
}
fn read_batch(row: &rusqlite::Row<'_>) -> rusqlite::Result<(BoundedBlob, BoundedBlob, String)> {
    Ok((
        read_bounded_blob(row, 0, 1, 64 * 1024)?,
        read_bounded_blob(row, 2, 3, 32)?,
        row.get(4)?,
    ))
}
fn read_receipt(row: &rusqlite::Row<'_>) -> rusqlite::Result<(BoundedBlob, u64)> {
    Ok((read_bounded_blob(row, 0, 1, 32 * 1024)?, row.get(2)?))
}
fn read_clock(row: &rusqlite::Row<'_>) -> rusqlite::Result<u64> {
    row.get(0)
}

#[cfg(test)]
mod tests;

fn read_material_source(row: &rusqlite::Row<'_>) -> rusqlite::Result<(BoundedBlob, String, u64)> {
    Ok((
        read_bounded_blob(row, 0, 1, 32 * 1024)?,
        row.get(2)?,
        row.get(3)?,
    ))
}
fn stored_source_error(_: Error) -> Error {
    Error::Corrupt
}
