//! 有效路径上的计时与效果重建；原增量与回执不删除、不按撤回后的顺序放大重算。

use super::*;

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
        let (confirmed,pending,mut after)=conn.query_row("SELECT effect_history_revision,effect_recovery_history,effect_recovery_after FROM memory_entries WHERE entry_id=?1 AND run_id=?2",params![entry_id,run.run_id],read_effect_progress).map_err(sql)?;
        if confirmed == Some(boundary.history_revision) && pending.is_none() {
            return Ok(true);
        }
        if pending != Some(boundary.history_revision) {
            conn.execute("UPDATE memory_entries SET effect_history_revision=NULL,effect_recovery_history=?1,effect_recovery_after=0 WHERE entry_id=?2",params![boundary.history_revision,entry_id]).map_err(sql)?;
            after = 0;
        }
        let mut statement=conn.prepare("SELECT rowid,length(source_json),effect_id,valid FROM memory_effects WHERE entry_id=?1 AND rowid>?2 ORDER BY rowid LIMIT 256").map_err(sql)?;
        let rows = statement
            .query_map(params![entry_id, after], read_effect_size)
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        drop(statement);
        let complete = rows.len() < MAX_SCAN;
        for (position, length, id, previous) in rows {
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
            let valid = self.valid_sources(run, &[source], port)?;
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
            }
            tx.execute(
                "UPDATE memory_entries SET effect_recovery_after=?1 WHERE entry_id=?2",
                params![position, entry_id],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        if !complete {
            return Ok(false);
        }
        let updated=conn.execute("UPDATE memory_entries SET effect_history_revision=?1,effect_recovery_history=NULL,effect_recovery_after=0 WHERE entry_id=?2",params![boundary.history_revision,entry_id]).map_err(sql)?;
        if updated != 1 {
            return Err(Error::Corrupt);
        }
        Ok(true)
    }
    /// 分页读取当前有效效果；保留原 gain，不因前项撤回重新计算后项。
    /// 一页最多 32 个效果；关联操作每页只审计一次，最多读取 32 份有界计划。
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
        let mut statement=conn.prepare("SELECT rowid,effect_id,kind,length(source_json),source_json,logical_position,gain,operation_id FROM memory_effects WHERE entry_id=?1 AND rowid>?2 AND valid=1 ORDER BY rowid LIMIT ?3").map_err(sql)?;
        let rows = statement
            .query_map(params![entry_id, after, limit], read_effect)
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        drop(statement);
        let mut effects = Vec::new();
        let mut audited = BTreeSet::new();
        for row in rows {
            if audited.insert(row.operation_id.clone()) {
                let (plan, state) = self.operation(conn, &row.operation_id)?;
                if state != OperationState::Applied || plan.run_id != run_id {
                    self.freeze_corrupt_run(conn, run_id, &row.operation_id)?;
                    return Err(Error::Corrupt);
                }
            }
            let bytes = row.bytes.into_result()?;
            let source: SourceRef = decode(&bytes, 32 * 1024)?;
            if self.valid_sources(&run, std::slice::from_ref(&source), port)? {
                effects.push((
                    row.position,
                    Effect {
                        effect_id: row.id,
                        kind: decode_enum(&row.kind)?,
                        source,
                        logical_position: row.logical_position,
                        gain: row.gain,
                    },
                ));
            }
        }
        Ok(effects)
    }
}
fn read_clock_progress(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, Option<u64>, u64, u64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}
fn read_effect_progress(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(Option<u64>, Option<u64>, u64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}
fn read_clock_size(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, usize, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}
fn read_effect_size(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, usize, String, bool)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
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
