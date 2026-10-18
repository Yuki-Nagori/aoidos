//! 有效路径上的计时与效果重建；原增量与回执不删除、不按撤回后的顺序放大重算。

use super::*;
use std::collections::BTreeMap;

pub(super) struct Budget {
    pub versions: usize,
    pub bytes: usize,
}
impl Budget {
    pub(super) fn take(&mut self, versions: usize, bytes: usize) -> bool {
        if versions > self.versions || bytes > self.bytes {
            false
        } else {
            self.versions -= versions;
            self.bytes -= bytes;
            true
        }
    }
}
impl Repository {
    pub(super) fn reconcile_clock(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        run: &Run,
        boundary: &Boundary,
        budget: &mut Budget,
    ) -> Result<bool> {
        let (confirmed,pending,mut after,mut total)=conn.query_row("SELECT clock_history_revision,clock_recovery_history,clock_recovery_after,clock_recovery_total FROM memory_runs WHERE run_id=?1",[&run.run_id],read_clock_progress).map_err(sql)?;
        if confirmed == boundary.history_revision && pending.is_none() {
            return Ok(true);
        }
        if pending != Some(boundary.history_revision) {
            conn.execute("UPDATE memory_runs SET clock_recovery_history=?1,clock_recovery_after=0,clock_recovery_total=0 WHERE run_id=?2",params![boundary.history_revision,run.run_id]).map_err(sql)?;
            after = 0;
            total = 0;
        }
        let mut statement=conn.prepare("SELECT rowid,length(source_json),round_id FROM memory_clock_receipts WHERE run_id=?1 AND rowid>?2 ORDER BY rowid LIMIT 256").map_err(sql)?;
        let rows = statement
            .query_map(params![run.run_id, after], read_clock_size)
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        drop(statement);
        let complete = rows.len() < MAX_SCAN;
        for (position, length, round_id) in rows {
            if length > 32 * 1024 {
                return Err(Error::Corrupt);
            }
            if !budget.take(1, length) {
                return Ok(false);
            }
            let raw = conn
                .query_row(
                    "SELECT length(source_json),source_json FROM memory_clock_receipts WHERE run_id=?1 AND round_id=?2",
                    params![run.run_id, round_id],
                    |row| read_bounded_blob(row, 0, 1, 32 * 1024),
                )
                .map_err(sql)?
                .into_result()?;
            let source: SourceRef = decode(&raw, 32 * 1024)?;
            let valid = self.valid_sources(run, std::slice::from_ref(&source), port)?
                && port.completed_round(&round_id, &source)?;
            total = total
                .checked_add(u64::from(valid))
                .filter(|value| *value <= MAX_INTEGER)
                .ok_or(Error::Corrupt)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql)?;
            tx.execute(
                "UPDATE memory_clock_receipts SET valid=?1 WHERE run_id=?2 AND round_id=?3",
                params![valid, run.run_id, round_id],
            )
            .map_err(sql)?;
            tx.execute("UPDATE memory_runs SET clock_recovery_after=?1,clock_recovery_total=?2 WHERE run_id=?3",params![position,total,run.run_id]).map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        if complete {
            conn.execute("UPDATE memory_runs SET logical_clock=?1,clock_history_revision=?2,clock_recovery_history=NULL,clock_recovery_after=0,clock_recovery_total=0 WHERE run_id=?3",params![total,boundary.history_revision,run.run_id]).map_err(sql)?;
        }
        Ok(complete)
    }
    pub(super) fn reconcile_effects(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        run: &Run,
        boundary: &Boundary,
        entry_id: &str,
        budget: &mut Budget,
    ) -> Result<bool> {
        let confirmed_history: Option<u64> = conn
            .query_row(
                "SELECT effect_history_revision FROM memory_entries WHERE entry_id=?1 AND run_id=?2",
                params![entry_id, run.run_id],
                read_history,
            )
            .map_err(sql)?;
        let progress = conn
            .query_row(
                "SELECT confirmed_history_revision,confirmed_evidence_revision,recovery_history_revision,recovery_evidence_revision,recovery_after FROM memory_effect_evidence_progress WHERE entry_id=?1",
                [entry_id],
                read_effect_progress,
            )
            .optional()
            .map_err(sql)?;
        if confirmed_history == Some(boundary.history_revision)
            && progress.is_some_and(
                |(history, evidence, pending_history, pending_evidence, _)| {
                    history == Some(boundary.history_revision)
                        && evidence == Some(boundary.evidence_revision)
                        && pending_history.is_none()
                        && pending_evidence.is_none()
                },
            )
        {
            return Ok(true);
        }
        let after = match progress {
            Some((_, _, Some(history), Some(evidence), after))
                if history == boundary.history_revision
                    && evidence == boundary.evidence_revision =>
            {
                after
            }
            _ => {
                conn.execute(
                    "UPDATE memory_entries SET effect_history_revision=NULL WHERE entry_id=?1",
                    [entry_id],
                )
                .map_err(sql)?;
                conn.execute(
                    "INSERT INTO memory_effect_evidence_progress(entry_id,recovery_history_revision,recovery_evidence_revision,recovery_after) VALUES(?1,?2,?3,0) ON CONFLICT(entry_id) DO UPDATE SET confirmed_history_revision=NULL,confirmed_evidence_revision=NULL,recovery_history_revision=excluded.recovery_history_revision,recovery_evidence_revision=excluded.recovery_evidence_revision,recovery_after=0",
                    params![entry_id, boundary.history_revision, boundary.evidence_revision],
                )
                .map_err(sql)?;
                0
            }
        };
        let mut statement=conn.prepare("SELECT rowid,length(source_json),effect_id,valid,operation_id FROM memory_effects WHERE entry_id=?1 AND rowid>?2 ORDER BY rowid LIMIT 256").map_err(sql)?;
        let rows = statement
            .query_map(params![entry_id, after], read_effect_recovery_row)
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        drop(statement);
        let complete = rows.len() < MAX_SCAN;
        let mut audited_operations = BTreeMap::new();
        for (position, length, id, previous, operation_id) in rows {
            if length > 32 * 1024 {
                return Err(Error::Corrupt);
            }
            if !budget.take(1, length) {
                return Ok(false);
            }
            let raw = conn
                .query_row(
                    "SELECT length(source_json),source_json FROM memory_effects WHERE effect_id=?1",
                    [&id],
                    |row| read_bounded_blob(row, 0, 1, 32 * 1024),
                )
                .map_err(sql)?
                .into_result()?;
            let source: SourceRef = decode(&raw, 32 * 1024)?;
            if !audited_operations.contains_key(&operation_id) {
                let plan_bytes: i64 = conn
                    .query_row(
                        "SELECT length(plan_json) FROM memory_operations WHERE operation_id=?1",
                        [&operation_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(sql)?
                    .ok_or(Error::Corrupt)?;
                let plan_bytes = usize::try_from(plan_bytes).map_err(|_| Error::Corrupt)?;
                if plan_bytes > MAX_PLAN_BYTES {
                    return Err(Error::Corrupt);
                }
                if !budget.take(0, plan_bytes) {
                    return Ok(false);
                }
                let (plan, state) = self.operation(conn, &operation_id)?;
                if state != OperationState::Applied || plan.run_id != run.run_id {
                    self.freeze_corrupt_run(conn, &run.run_id, &operation_id)?;
                    return Err(Error::Corrupt);
                }
                audited_operations.insert(operation_id.clone(), plan);
            }
            let plan = audited_operations
                .get(&operation_id)
                .ok_or(Error::Corrupt)?;
            let target = plan
                .targets
                .iter()
                .find(|target| target.entry_id == entry_id)
                .ok_or(Error::Corrupt)?;
            let effect = target
                .effects
                .iter()
                .find(|effect| effect.effect_id == id)
                .ok_or(Error::Corrupt)?;
            let subject = self.target_subject(conn, target)?;
            let valid = source == effect.source
                && self.valid_sources(run, std::slice::from_ref(&source), port)?
                && self.valid_effect_evidence(conn, port, &subject, effect)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql)?;
            if valid != previous {
                let updated=tx.execute("UPDATE memory_entries SET state_revision=state_revision+1 WHERE entry_id=?1 AND state_revision<9007199254740991",[entry_id]).map_err(sql)?;
                if updated != 1 {
                    return Err(Error::Rejected(Reason::CapacityBlocked));
                }
                tx.execute(
                    "UPDATE memory_effects SET valid=?1 WHERE effect_id=?2",
                    params![valid, id],
                )
                .map_err(sql)?;
                if effect.kind == EffectKind::Revival {
                    let current_status: String = tx
                        .query_row(
                            "SELECT status FROM memory_entries WHERE entry_id=?1",
                            [entry_id],
                            |row| row.get(0),
                        )
                        .map_err(sql)?;
                    let later_revival: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM memory_effects WHERE entry_id=?1 AND rowid>?2 AND kind='revival' AND valid=1)",
                            params![entry_id, position],
                            |row| row.get(0),
                        )
                        .map_err(sql)?;
                    if previous && !valid && current_status == "active" && !later_revival {
                        tx.execute(
                            "UPDATE memory_entries SET status='archived' WHERE entry_id=?1 AND status='active'",
                            [entry_id],
                        )
                        .map_err(sql)?;
                    } else if !previous && valid && current_status == "archived" {
                        let Some(latest_state) =
                            self.latest_planned_state(&tx, entry_id, &operation_id, plan, budget)?
                        else {
                            return Ok(false);
                        };
                        if latest_state == EntryState::Active {
                            let active: u64 = tx
                                .query_row(
                                    "SELECT count(*) FROM memory_entries WHERE run_id=?1 AND status='active'",
                                    [&run.run_id],
                                    |row| row.get(0),
                                )
                                .map_err(sql)?;
                            if active < run.policy.active_capacity {
                                tx.execute(
                                    "UPDATE memory_entries SET status='active' WHERE entry_id=?1 AND status='archived'",
                                    [entry_id],
                                )
                                .map_err(sql)?;
                            }
                        }
                    }
                }
            }
            tx.execute(
                "UPDATE memory_effect_evidence_progress SET recovery_after=?1 WHERE entry_id=?2",
                params![position, entry_id],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        if !complete {
            return Ok(false);
        }
        let updated = conn
            .execute(
                "UPDATE memory_entries SET effect_history_revision=?1 WHERE entry_id=?2",
                params![boundary.history_revision, entry_id],
            )
            .map_err(sql)?;
        if updated != 1 {
            return Err(Error::Corrupt);
        }
        conn.execute(
            "UPDATE memory_effect_evidence_progress SET confirmed_history_revision=?1,confirmed_evidence_revision=?2,recovery_history_revision=NULL,recovery_evidence_revision=NULL,recovery_after=0 WHERE entry_id=?3",
            params![boundary.history_revision, boundary.evidence_revision, entry_id],
        )
        .map_err(sql)?;
        Ok(true)
    }

    fn valid_effect_evidence(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        subject: &Subject,
        effect: &Effect,
    ) -> Result<bool> {
        match subject {
            Subject::PlayerPreference => Ok(effect.evidence.is_empty()),
            Subject::Character { id } => {
                if effect.evidence.is_empty() {
                    return Ok(false);
                }
                for evidence in &effect.evidence {
                    if !valid_evidence(evidence, id)
                        || !port.valid_evidence_with_connection(evidence, conn)?
                    {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }

    fn target_subject(&self, conn: &Connection, target: &Target) -> Result<Subject> {
        if let Some(version) = &target.next {
            return Ok(version.subject.clone());
        }
        let version_id = target
            .expected
            .version_id
            .as_deref()
            .ok_or(Error::Corrupt)?;
        let metadata = self.version_metadata(conn, &target.entry_id, version_id)?;
        decode(&metadata.subject, 4096)
    }

    /// 读取该条目最近一次已应用操作声明的状态，用于恢复复活效果的状态变更。
    fn latest_planned_state(
        &self,
        conn: &Connection,
        entry_id: &str,
        current_operation_id: &str,
        current_plan: &Plan,
        budget: &mut Budget,
    ) -> Result<Option<EntryState>> {
        let latest_operation: String = conn
            .query_row(
                "SELECT o.operation_id FROM memory_operation_targets t
                 JOIN memory_operations o USING(operation_id)
                 WHERE t.entry_id=?1 AND o.state='applied'
                 ORDER BY t.next_state_revision DESC LIMIT 1",
                [entry_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?
            .ok_or(Error::Corrupt)?;
        if latest_operation == current_operation_id {
            let target = current_plan
                .targets
                .iter()
                .find(|target| target.entry_id == entry_id)
                .ok_or(Error::Corrupt)?;
            return Ok(Some(target.state));
        }
        let plan_bytes: i64 = conn
            .query_row(
                "SELECT length(plan_json) FROM memory_operations WHERE operation_id=?1",
                [&latest_operation],
                |row| row.get(0),
            )
            .map_err(sql)?;
        let plan_bytes = usize::try_from(plan_bytes).map_err(|_| Error::Corrupt)?;
        if plan_bytes > MAX_PLAN_BYTES {
            return Err(Error::Corrupt);
        }
        if !budget.take(0, plan_bytes) {
            return Ok(None);
        }
        let (plan, state) = self.operation(conn, &latest_operation)?;
        if state != OperationState::Applied {
            return Err(Error::Corrupt);
        }
        let target = plan
            .targets
            .iter()
            .find(|target| target.entry_id == entry_id)
            .ok_or(Error::Corrupt)?;
        Ok(Some(target.state))
    }
    /// 分页读取当前有效效果；保留原 gain，不因前项撤回重新计算后项。
    /// 一页最多 32 个效果；关联操作每页只审计一次。回退重建另有独立的恢复预算。
    /// # Errors
    /// 回退重建未完成拒绝消费；来源原文不可读取时返回错误，不能用旧 valid 缓存放行。
    pub fn effects(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        run_id: &str,
        entry_id: &str,
        after: u64,
        limit: usize,
    ) -> Result<Vec<(u64, Effect)>> {
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        self.clock(conn, port, run_id)?;
        self.entry(conn, run_id, entry_id)?;
        if limit == 0 || limit > 32 || after > MAX_INTEGER {
            return Err(reject());
        }
        let confirmed: Option<u64> = conn
            .query_row(
                "SELECT effect_history_revision FROM memory_entries WHERE entry_id=?1",
                [entry_id],
                read_history,
            )
            .map_err(sql)?;
        if confirmed != Some(boundary.history_revision) {
            return Err(Error::Rejected(Reason::RecoveryRequired));
        }
        let evidence_confirmed: Option<u64> = conn
            .query_row(
                "SELECT confirmed_evidence_revision FROM memory_effect_evidence_progress WHERE entry_id=?1 AND confirmed_history_revision=?2",
                params![entry_id, boundary.history_revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?
            .flatten();
        if evidence_confirmed != Some(boundary.evidence_revision) {
            return Err(Error::Rejected(Reason::RecoveryRequired));
        }
        let mut statement=conn.prepare("SELECT rowid,effect_id,kind,length(source_json),source_json,logical_position,gain,operation_id FROM memory_effects WHERE entry_id=?1 AND rowid>?2 AND valid=1 ORDER BY rowid LIMIT ?3").map_err(sql)?;
        let rows = statement
            .query_map(params![entry_id, after, limit], read_effect)
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        drop(statement);
        let mut effects = Vec::new();
        let mut audited = BTreeMap::new();
        for row in rows {
            if !audited.contains_key(&row.operation_id) {
                let (plan, state) = self.operation(conn, &row.operation_id)?;
                if state != OperationState::Applied || plan.run_id != run_id {
                    self.freeze_corrupt_run(conn, run_id, &row.operation_id)?;
                    return Err(Error::Corrupt);
                }
                audited.insert(row.operation_id.clone(), plan);
            }
            let bytes = row.bytes.into_result()?;
            let source: SourceRef = decode(&bytes, 32 * 1024)?;
            let plan = audited.get(&row.operation_id).ok_or(Error::Corrupt)?;
            let target = plan
                .targets
                .iter()
                .find(|target| target.entry_id == entry_id)
                .ok_or(Error::Corrupt)?;
            let effect = target
                .effects
                .iter()
                .find(|effect| effect.effect_id == row.id)
                .ok_or(Error::Corrupt)?;
            let subject = self.target_subject(conn, target)?;
            if source == effect.source
                && effect.kind == decode_enum(&row.kind)?
                && effect.logical_position == row.logical_position
                && effect.gain == row.gain
                && self.valid_sources(&run, std::slice::from_ref(&source), port)?
                && self.valid_effect_evidence(conn, port, &subject, effect)?
            {
                let mut current = effect.clone();
                current.logical_position =
                    self.source_logical_position(conn, port, &run, &boundary, &source)?;
                effects.push((row.position, current));
            }
        }
        Ok(effects)
    }
}
fn read_clock_progress(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, Option<u64>, u64, u64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}
type EffectProgress = (Option<u64>, Option<u64>, Option<u64>, Option<u64>, u64);
fn read_effect_progress(row: &rusqlite::Row<'_>) -> rusqlite::Result<EffectProgress> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}
fn read_clock_size(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, usize, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}
fn read_effect_recovery_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(u64, usize, String, bool, String)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}
struct EffectRow {
    position: u64,
    id: String,
    kind: String,
    bytes: BoundedBlob,
    logical_position: u64,
    gain: u64,
    operation_id: String,
}
fn read_effect(row: &rusqlite::Row<'_>) -> rusqlite::Result<EffectRow> {
    Ok(EffectRow {
        position: row.get(0)?,
        id: row.get(1)?,
        kind: row.get(2)?,
        bytes: read_bounded_blob(row, 3, 4, 32 * 1024)?,
        logical_position: row.get(5)?,
        gain: row.get(6)?,
        operation_id: row.get(7)?,
    })
}
fn read_history(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<u64>> {
    row.get(0)
}

#[cfg(test)]
mod tests;
