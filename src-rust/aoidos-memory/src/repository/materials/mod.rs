//! 稳定素材及批次短引用映射；恢复复用原始身份，不重新切片或发请求。

use super::*;
use crate::model::MAX_PROPOSAL_BYTES;
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

/// 与物理请求绑定的持久回执；响应和完成摘要只允许通过仓储读取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchReceipt {
    pub state: BatchState,
    pub turn_id: String,
    pub request_id: String,
    pub response: Option<Vec<u8>>,
    pub completion: Option<Vec<u8>>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CandidateOutcome {
    Applied,
    NoOp,
    Rejected,
    Pending,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateResult {
    pub candidate_id: String,
    pub outcome: CandidateOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchCompletion {
    pub version: u32,
    pub outcomes: Vec<CandidateResult>,
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

    /// 在任何网络发送前登记唯一 turn / 计费请求身份。仅首次返回 `true`，调用方才可发送。
    /// 重复调用只允许恢复原身份，并返回 `false`，防止命令重放造成二次收费。
    /// # Errors
    /// 非 queued 批次、身份冲突或非法 UUID 拒绝；数据库失败不返回可发送许可。
    pub fn dispatch_batch(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        batch_id: &str,
        turn_id: &str,
        request_id: &str,
    ) -> Result<bool> {
        if !valid_id(batch_id) || !valid_id(turn_id) || !valid_id(request_id) {
            return Err(reject());
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let (snapshot, _) = self.batch(&tx, batch_id)?;
        self.ensure_writable(&tx, &snapshot.run_id)?;
        let (_, boundary) = self.boundary(&tx, &snapshot.run_id, port)?;
        if snapshot.history_revision != boundary.history_revision
            || snapshot.entity_revision != boundary.entity_revision
            || snapshot.evidence_revision != boundary.evidence_revision
        {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let state: String = tx
            .query_row(
                "SELECT status FROM memory_batches WHERE batch_id=?1",
                [batch_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?
            .ok_or(Error::NotFound)?;
        if state != "queued" {
            if let Some((stored_turn, stored_request)) = tx
                .query_row(
                    "SELECT turn_id,request_id FROM memory_batch_receipts WHERE batch_id=?1",
                    [batch_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(sql)?
                && stored_turn == turn_id
                && stored_request == request_id
            {
                return Ok(false);
            }
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        tx.execute(
            "INSERT INTO memory_batch_receipts(batch_id,turn_id,request_id) VALUES(?1,?2,?3)",
            params![batch_id, turn_id, request_id],
        )
        .map_err(sql)?;
        let changed = tx
            .execute(
                "UPDATE memory_batches SET status='dispatched' WHERE batch_id=?1 AND status='queued'",
                [batch_id],
            )
            .map_err(sql)?;
        if changed != 1 {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        tx.commit().map_err(sql)?;
        Ok(true)
    }

    /// 保存完整、合法且有界的 provider 响应，并在同一 SQLite 事务标记 received。
    /// # Errors
    /// 非法 JSON / 超限响应拒绝；迟到或身份不匹配的响应不能替换已登记的物理请求。
    pub fn receive_batch(
        &self,
        conn: &mut Connection,
        batch_id: &str,
        turn_id: &str,
        request_id: &str,
        response: &[u8],
    ) -> Result<bool> {
        if !valid_id(batch_id) || !valid_id(turn_id) || !valid_id(request_id) {
            return Err(reject());
        }
        if response.len() > MAX_PROPOSAL_BYTES {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let digest = hash(response);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let state: String = tx
            .query_row(
                "SELECT status FROM memory_batches WHERE batch_id=?1",
                [batch_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?
            .ok_or(Error::NotFound)?;
        let identity = tx.query_row(
            "SELECT turn_id,request_id,length(response_json),response_json,length(response_hash),response_hash FROM memory_batch_receipts WHERE batch_id=?1",
            [batch_id],
            read_batch_receipt_identity,
        ).optional().map_err(sql)?.ok_or(Error::Rejected(Reason::StaleVersion))?;
        if identity.0 != turn_id || identity.1 != request_id {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        if state == "received" || state == "completed" {
            let previous = identity.2.ok_or(Error::Corrupt)?.into_result()?;
            let previous_hash = identity.3.ok_or(Error::Corrupt)?.into_result()?;
            if previous_hash != hash(&previous) {
                return Err(Error::Corrupt);
            }
            if previous == response {
                return Ok(false);
            }
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        if state != "dispatched" || identity.2.is_some() {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        decode_proposal_response(response)?;
        let saved = tx.execute(
            "UPDATE memory_batch_receipts SET response_hash=?1,response_json=?2 WHERE batch_id=?3 AND turn_id=?4 AND request_id=?5 AND response_json IS NULL",
            params![digest.as_slice(), response, batch_id, turn_id, request_id],
        )
        .map_err(sql)?;
        if saved != 1 {
            return Err(Error::Corrupt);
        }
        let changed = tx
            .execute(
                "UPDATE memory_batches SET status='received',result_hash=?1 WHERE batch_id=?2 AND status='dispatched'",
                params![digest.as_slice(), batch_id],
            )
            .map_err(sql)?;
        if changed != 1 {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        tx.commit().map_err(sql)?;
        Ok(true)
    }

    /// 持久化本地候选处理摘要；重复提交相同摘要幂等，不能改写已完成结果。
    /// # Errors
    /// 摘要必须是有界合法 JSON，且批次必须已有 received 响应。
    pub fn complete_batch(
        &self,
        conn: &mut Connection,
        batch_id: &str,
        completion: &[u8],
    ) -> Result<bool> {
        if !valid_id(batch_id) {
            return Err(reject());
        }
        if completion.len() > MAX_PROPOSAL_BYTES {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let result: BatchCompletion =
            aoidos_json::decode_bytes(completion, MAX_PROPOSAL_BYTES).map_err(|_| reject())?;
        let completion_hash = hash(completion);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let state: String = tx
            .query_row(
                "SELECT status FROM memory_batches WHERE batch_id=?1",
                [batch_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?
            .ok_or(Error::NotFound)?;
        if state == "completed" {
            let previous = tx
                .query_row(
                    "SELECT length(completion_json),completion_json FROM memory_batch_receipts WHERE batch_id=?1",
                    [batch_id],
                    |row| read_bounded_blob(row, 0, 1, MAX_PROPOSAL_BYTES),
                )
                .optional()
                .map_err(sql)?
                .ok_or(Error::Corrupt)?
                .into_result()?;
            if previous == completion {
                return Ok(false);
            }
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        if state != "received" {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let receipt = self.batch_receipt(&tx, batch_id)?;
        validate_received_receipt(&receipt)?;
        validate_completion(&tx, batch_id, &result)?;
        let changed = tx
            .execute(
                "UPDATE memory_batch_receipts SET completion_json=?1,completion_hash=?2 WHERE batch_id=?3 AND response_json IS NOT NULL AND completion_json IS NULL",
                params![completion, completion_hash.as_slice(), batch_id],
            )
            .map_err(sql)?;
        if changed != 1 {
            return Err(Error::Corrupt);
        }
        let changed = tx
            .execute(
                "UPDATE memory_batches SET status='completed' WHERE batch_id=?1 AND status='received'",
                [batch_id],
            )
            .map_err(sql)?;
        if changed != 1 {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        tx.commit().map_err(sql)?;
        Ok(true)
    }

    /// 明确记录无法判定请求是否已发送的结果；此状态禁止同批次重发。
    /// # Errors
    /// 只允许登记过物理请求的 dispatched 批次进入 unknown。
    pub fn mark_batch_unknown(
        &self,
        conn: &mut Connection,
        batch_id: &str,
        turn_id: &str,
        request_id: &str,
        error_code: &str,
    ) -> Result<bool> {
        if !valid_id(batch_id)
            || !valid_id(turn_id)
            || !valid_id(request_id)
            || !(1..=64).contains(&error_code.len())
            || !error_code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(reject());
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let identity: (String, String, Option<String>) = tx
            .query_row(
                "SELECT turn_id,request_id,error_code FROM memory_batch_receipts WHERE batch_id=?1",
                [batch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(sql)?
            .ok_or(Error::NotFound)?;
        if identity.0 != turn_id || identity.1 != request_id {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let status: String = tx
            .query_row(
                "SELECT status FROM memory_batches WHERE batch_id=?1",
                [batch_id],
                |row| row.get(0),
            )
            .map_err(sql)?;
        if status == "unknown" && identity.2.as_deref() == Some(error_code) {
            return Ok(false);
        }
        if status != "dispatched" {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        tx.execute(
            "UPDATE memory_batch_receipts SET error_code=?1 WHERE batch_id=?2",
            params![error_code, batch_id],
        )
        .map_err(sql)?;
        tx.execute(
            "UPDATE memory_batches SET status='unknown' WHERE batch_id=?1 AND status='dispatched'",
            [batch_id],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(true)
    }

    /// 只在主动重试 / 新的有效输入下创建新 batch；旧批次仍保留 unknown / rejected。
    /// # Errors
    /// 仅 queued / dispatched 批次可显式拒绝，且错误码不携带供应商正文。
    pub fn reject_batch(
        &self,
        conn: &mut Connection,
        batch_id: &str,
        error_code: &str,
    ) -> Result<bool> {
        if !valid_id(batch_id)
            || !(1..=64).contains(&error_code.len())
            || !error_code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(reject());
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let status: String = tx
            .query_row(
                "SELECT status FROM memory_batches WHERE batch_id=?1",
                [batch_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?
            .ok_or(Error::NotFound)?;
        if status == "rejected" {
            let previous: Option<String> = tx
                .query_row(
                    "SELECT error_code FROM memory_batch_receipts WHERE batch_id=?1",
                    [batch_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql)?
                .flatten();
            if previous.as_deref() == Some(error_code) {
                return Ok(false);
            }
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        if status != "dispatched" && status != "received" {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let updated = tx
            .execute(
                "UPDATE memory_batch_receipts SET error_code=?1 WHERE batch_id=?2",
                params![error_code, batch_id],
            )
            .map_err(sql)?;
        if updated != 1 {
            return Err(Error::Corrupt);
        }
        tx.execute(
            "UPDATE memory_batches SET status='rejected' WHERE batch_id=?1",
            [batch_id],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(true)
    }

    /// 读取已持久化的私有结果；数据库长度在复制 BLOB 前先核验。
    /// # Errors
    /// 非法身份、未知批次、hash 不符或损坏字段拒绝。
    pub fn batch_receipt(&self, conn: &Connection, batch_id: &str) -> Result<BatchReceipt> {
        if !valid_id(batch_id) {
            return Err(reject());
        }
        let row = conn
            .query_row(
                "SELECT b.status,r.turn_id,r.request_id,length(r.response_json),r.response_json,length(r.completion_json),r.completion_json,length(r.response_hash),r.response_hash,length(r.completion_hash),r.completion_hash,r.error_code,length(b.result_hash),b.result_hash FROM memory_batch_receipts r JOIN memory_batches b USING(batch_id) WHERE b.batch_id=?1",
                [batch_id],
                read_batch_receipt,
            )
            .optional()
            .map_err(sql)?
            .ok_or(Error::NotFound)?;
        let state: BatchState = decode_enum(&row.0)?;
        if !valid_id(&row.1) || !valid_id(&row.2) {
            return Err(Error::Corrupt);
        }
        let response = row.3.map(BoundedBlob::into_result).transpose()?;
        let completion = row.4.map(BoundedBlob::into_result).transpose()?;
        let digest = row.5.map(BoundedBlob::into_result).transpose()?;
        let completion_digest = row.6.map(BoundedBlob::into_result).transpose()?;
        let indexed_digest = row.8.map(BoundedBlob::into_result).transpose()?;
        if let Some(bytes) = &response {
            if digest.as_deref() != Some(hash(bytes).as_slice())
                || indexed_digest.as_deref() != digest.as_deref()
                || decode_proposal_response(bytes).is_err()
            {
                return Err(Error::Corrupt);
            }
        } else if digest.is_some() || indexed_digest.is_some() {
            return Err(Error::Corrupt);
        }
        if let Some(bytes) = &completion
            && (response.is_none() || completion_digest.as_deref() != Some(hash(bytes).as_slice()))
        {
            return Err(Error::Corrupt);
        }
        if let Some(bytes) = &completion {
            let parsed: BatchCompletion =
                aoidos_json::decode_bytes(bytes, MAX_PROPOSAL_BYTES).map_err(|_| Error::Corrupt)?;
            validate_completion(conn, batch_id, &parsed).map_err(|_| Error::Corrupt)?;
        }
        if completion.is_none() && completion_digest.is_some() {
            return Err(Error::Corrupt);
        }
        let state_is_consistent = match state {
            BatchState::Queued => false,
            BatchState::Dispatched => response.is_none() && completion.is_none() && row.7.is_none(),
            BatchState::Received => response.is_some() && completion.is_none() && row.7.is_none(),
            BatchState::Completed => response.is_some() && completion.is_some() && row.7.is_none(),
            BatchState::Rejected => row.7.is_some() && completion.is_none(),
            BatchState::Unknown => response.is_none() && completion.is_none() && row.7.is_some(),
        };
        if !state_is_consistent {
            return Err(Error::Corrupt);
        }
        Ok(BatchReceipt {
            state,
            turn_id: row.1,
            request_id: row.2,
            response,
            completion,
            error_code: row.7,
        })
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
fn read_batch_receipt_identity(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, Option<BoundedBlob>, Option<BoundedBlob>)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        read_optional_bounded_blob(row, 2, 3, MAX_PROPOSAL_BYTES)?,
        read_optional_bounded_blob(row, 4, 5, 32)?,
    ))
}
type BatchReceiptRow = (
    String,
    String,
    String,
    Option<BoundedBlob>,
    Option<BoundedBlob>,
    Option<BoundedBlob>,
    Option<BoundedBlob>,
    Option<String>,
    Option<BoundedBlob>,
);
fn read_batch_receipt(row: &rusqlite::Row<'_>) -> rusqlite::Result<BatchReceiptRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        read_optional_bounded_blob(row, 3, 4, MAX_PROPOSAL_BYTES)?,
        read_optional_bounded_blob(row, 5, 6, MAX_PROPOSAL_BYTES)?,
        read_optional_bounded_blob(row, 7, 8, 32)?,
        read_optional_bounded_blob(row, 9, 10, 32)?,
        row.get(11)?,
        read_optional_bounded_blob(row, 12, 13, 32)?,
    ))
}
fn read_optional_bounded_blob(
    row: &rusqlite::Row<'_>,
    length_index: usize,
    blob_index: usize,
    limit: usize,
) -> rusqlite::Result<Option<BoundedBlob>> {
    let length: Option<i64> = row.get(length_index)?;
    let bytes: Option<Vec<u8>> = row.get(blob_index)?;
    match (length, bytes) {
        (None, None) => Ok(None),
        (Some(length), Some(bytes))
            if length >= 0 && length as usize <= limit && length as usize == bytes.len() =>
        {
            Ok(Some(BoundedBlob(Some(bytes))))
        }
        _ => Ok(Some(BoundedBlob(None))),
    }
}
fn validate_completion(
    conn: &Connection,
    batch_id: &str,
    completion: &BatchCompletion,
) -> Result<()> {
    if completion.version != 1 || completion.outcomes.len() > MAX_CANDIDATES {
        return Err(reject());
    }
    let raw = conn
        .query_row(
            "SELECT length(response_json),response_json FROM memory_batch_receipts WHERE batch_id=?1",
            [batch_id],
            |row| read_bounded_blob(row, 0, 1, MAX_PROPOSAL_BYTES),
        )
        .optional()
        .map_err(sql)?
        .ok_or(Error::Corrupt)?
        .into_result()?;
    let proposal = decode_proposal_response(&raw).map_err(|_| Error::Corrupt)?;
    validate_proposal_identity(&proposal).map_err(stored_source_error)?;
    let expected = proposal
        .candidates
        .iter()
        .map(|candidate| candidate.candidate_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut actual = BTreeSet::new();
    for outcome in &completion.outcomes {
        if !valid_short_id(&outcome.candidate_id)
            || !actual.insert(outcome.candidate_id.as_str())
            || !expected.contains(outcome.candidate_id.as_str())
            || outcome.reason_code.as_deref().is_some_and(|code| {
                !(1..=64).contains(&code.len())
                    || !code.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                    })
            })
        {
            return Err(reject());
        }
        let needs_reason = matches!(
            outcome.outcome,
            CandidateOutcome::Rejected | CandidateOutcome::Pending | CandidateOutcome::Deferred
        );
        if needs_reason != outcome.reason_code.is_some() {
            return Err(reject());
        }
    }
    if actual != expected {
        return Err(reject());
    }
    Ok(())
}
fn decode_proposal_response(bytes: &[u8]) -> Result<crate::model::Proposal> {
    let proposal: crate::model::Proposal =
        aoidos_json::decode_bytes(bytes, MAX_PROPOSAL_BYTES).map_err(|_| reject())?;
    if proposal.version != 1 {
        return Err(Error::Rejected(Reason::PolicyUnavailable));
    }
    if proposal.candidates.len() > MAX_CANDIDATES {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    validate_proposal_identity(&proposal)?;
    Ok(proposal)
}
fn validate_proposal_identity(proposal: &crate::model::Proposal) -> Result<()> {
    if proposal.version != 1 {
        return Err(Error::Rejected(Reason::PolicyUnavailable));
    }
    let mut seen = BTreeSet::new();
    if proposal.candidates.iter().any(|candidate| {
        !valid_short_id(&candidate.candidate_id) || !seen.insert(candidate.candidate_id.as_str())
    }) {
        return Err(reject());
    }
    Ok(())
}
fn validate_received_receipt(receipt: &BatchReceipt) -> Result<()> {
    if receipt.state != BatchState::Received || receipt.response.is_none() {
        return Err(Error::Corrupt);
    }
    Ok(())
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
