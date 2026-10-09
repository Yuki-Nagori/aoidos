//! 分页恢复只使用持久计划与祖先版本；不发请求，不以文件时间决定有效因果路径。

use super::reconciliation::Budget;
use super::*;

/// 扫描游标绑定周目 / 历史修订与固定 rowid 上界；新记录不挤入旧分页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanCursor {
    pub run_id: String,
    pub history_revision: u64,
    pub through: u64,
    pub after: u64,
    pub through_operations: u64,
    pub after_operation: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryPage {
    pub operations: usize,
    pub checked_entries: usize,
    pub restored_entries: usize,
    pub unavailable_entries: usize,
    pub next: Option<ScanCursor>,
}
impl Repository {
    /// # Errors
    /// 恢复与写入在同一单写者边界内；游标过期拒绝，未知策略只读返回 not-ready。
    /// 一次最多核验 256 版本 / 4 MiB 元信息，未扫完的条目保留持久恢复游标。
    pub fn recover(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        run_id: &str,
        cursor: Option<&ScanCursor>,
    ) -> Result<RecoveryPage> {
        self.ensure_writable(conn, run_id)?;
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        let through = conn
            .query_row(
                "SELECT COALESCE(max(rowid),0) FROM memory_entries WHERE run_id=?1",
                [run_id],
                read_number,
            )
            .map_err(sql)?;
        let through_operations = conn
            .query_row(
                "SELECT COALESCE(max(rowid),0) FROM memory_operations WHERE run_id=?1",
                [run_id],
                read_number,
            )
            .map_err(sql)?;
        let mut scan = cursor.cloned().unwrap_or(ScanCursor {
            run_id: run_id.into(),
            history_revision: boundary.history_revision,
            through,
            after: 0,
            through_operations,
            after_operation: 0,
        });
        if scan.run_id != run_id
            || scan.history_revision != boundary.history_revision
            || scan.after > scan.through
            || scan.through > through
            || scan.after_operation > scan.through_operations
            || scan.through_operations > through_operations
        {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let mut budget = Budget {
            versions: MAX_SCAN,
            bytes: MAX_SCAN_BYTES,
        };
        let mut page = RecoveryPage {
            operations: 0,
            checked_entries: 0,
            restored_entries: 0,
            unavailable_entries: 0,
            next: None,
        };
        if !self.reconcile_clock(conn, port, &run, &boundary, &mut budget)? {
            page.next = Some(scan);
            return Ok(page);
        }
        let mut statement=conn.prepare("SELECT o.rowid,o.operation_id,length(o.plan_json),(SELECT count(*) FROM memory_operation_targets t WHERE t.operation_id=o.operation_id) FROM memory_operations o WHERE o.run_id=?1 AND o.rowid>?2 AND o.rowid<=?3 AND o.state IN ('prepared','ready','applied') ORDER BY o.rowid LIMIT 256").map_err(sql)?;
        let operations = statement
            .query_map(
                params![run_id, scan.after_operation, scan.through_operations],
                read_operation_size,
            )
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        drop(statement);
        let operation_complete = operations.len() < MAX_SCAN;
        for (position, id, bytes, targets) in operations {
            if bytes > MAX_PLAN_BYTES || targets > 16 {
                return Err(Error::Corrupt);
            }
            if !budget.take(targets + 1, bytes + targets * MAX_VERSION_BYTES) {
                page.next = Some(scan);
                return Ok(page);
            }
            let (plan, state) = self.operation(conn, &id)?;
            if state == OperationState::Applied {
                self.verify_applied(conn, &plan)?;
            } else {
                let mut complete = true;
                for target in &plan.targets {
                    if let Some(version) = &target.next {
                        let relation_bytes =
                            self.version_relation_bytes(conn, &version.version_id)?;
                        if !budget.take(0, relation_bytes) {
                            page.next = Some(scan);
                            return Ok(page);
                        }
                        match self.read_version(conn, &run, &target.entry_id, &version.version_id) {
                            Ok(_) => {}
                            Err(Error::Corrupt) => {
                                let exists = self
                                    .version_path(&run, &target.entry_id, &version.version_id)?
                                    .try_exists()
                                    .map_err(aoidos_store::error::StoreError::from_io)?;
                                self.set_state(
                                    conn,
                                    &id,
                                    if exists {
                                        OperationState::Quarantined
                                    } else {
                                        OperationState::Rejected
                                    },
                                    if exists {
                                        "bodyConflict"
                                    } else {
                                        "missingPreparedBody"
                                    },
                                )?;
                                complete = false;
                                break;
                            }
                            Err(error) => return Err(error),
                        }
                    }
                }
                if complete {
                    if state == OperationState::Prepared {
                        self.set_state(conn, &id, OperationState::Ready, "")?;
                    }
                    if let Err(error) = self.apply(conn, port, &id)
                        && !matches!(error, Error::Rejected(_))
                    {
                        return Err(error);
                    }
                }
            }
            page.operations += 1;
            scan.after_operation = position;
        }
        if operation_complete {
            scan.after_operation = scan.through_operations;
        }
        if scan.after_operation < scan.through_operations {
            page.next = Some(scan);
            return Ok(page);
        }
        let mut statement=conn.prepare("SELECT rowid,entry_id FROM memory_entries WHERE run_id=?1 AND current_version_id IS NOT NULL AND rowid>?2 AND rowid<=?3 ORDER BY rowid LIMIT 256").map_err(sql)?;
        let entries = statement
            .query_map(
                params![run_id, scan.after, scan.through],
                read_entry_position,
            )
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        drop(statement);
        let entries_complete = entries.len() < MAX_SCAN;
        for (position, id) in entries {
            if budget.versions == 0
                || budget.bytes < MAX_VERSION_BYTES
                || !self.reconcile_effects(conn, port, &run, &boundary, &id, &mut budget)?
            {
                page.next = Some(scan);
                return Ok(page);
            }
            match self.recover_entry(conn, port, &run, &boundary, &id, &mut budget)? {
                EntryRecovery::Healthy => {}
                EntryRecovery::Restored => page.restored_entries += 1,
                EntryRecovery::Unavailable => page.unavailable_entries += 1,
                EntryRecovery::Deferred => {
                    page.next = Some(scan);
                    return Ok(page);
                }
            }
            page.checked_entries += 1;
            scan.after = position;
        }
        if entries_complete {
            scan.after = scan.through;
        }
        if scan.after < scan.through {
            page.next = Some(scan);
        }
        Ok(page)
    }
    fn recover_entry(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        run: &Run,
        boundary: &Boundary,
        id: &str,
        budget: &mut Budget,
    ) -> Result<EntryRecovery> {
        let entry = self.entry(conn, &run.run_id, id)?;
        let (stored_cursor,stored_history,restore_state)=conn.query_row("SELECT recovery_cursor,recovery_history_revision,restore_status FROM memory_entries WHERE entry_id=?1",[id],read_recovery).map_err(sql)?;
        let mut cursor = if stored_history == Some(boundary.history_revision) {
            stored_cursor
        } else {
            None
        };
        if entry.state != EntryState::Unavailable || cursor.is_none() {
            let bytes = self.version_read_bytes(conn, id, &entry.current_version_id)?;
            if !budget.take(1, bytes) {
                return Ok(EntryRecovery::Deferred);
            }
            match self.read_version(conn, run, id, &entry.current_version_id) {
                Ok(version)
                    if self.valid_version(run, &version, port)?
                        && entry.state != EntryState::Unavailable =>
                {
                    return Ok(EntryRecovery::Healthy);
                }
                Err(Error::Rejected(Reason::PolicyUnavailable)) => {
                    return Err(Error::Rejected(Reason::PolicyUnavailable));
                }
                Err(Error::Store(error)) => return Err(Error::Store(error)),
                _ => {}
            }
            conn.execute("UPDATE memory_entries SET restore_status=CASE WHEN status IN ('active','archived') THEN status ELSE restore_status END,status='unavailable',state_revision=state_revision+CASE WHEN status!='unavailable' THEN 1 ELSE 0 END,recovery_cursor=?1,recovery_history_revision=?2,recovery_visited=NULL WHERE entry_id=?3",params![entry.current_version_id,boundary.history_revision,id]).map_err(sql)?;
            cursor = Some(entry.current_version_id.clone());
        }
        if cursor.as_deref() == Some("") {
            return Ok(EntryRecovery::Unavailable);
        }
        let mut current = cursor.ok_or(Error::Corrupt)?;
        let mut visited = BTreeSet::new();
        while !current.is_empty() {
            let bytes = self.version_read_bytes(conn, id, &current)?;
            if !budget.take(1, bytes) {
                return Ok(EntryRecovery::Deferred);
            }
            if !valid_id(&current) || !visited.insert(current.clone()) {
                return Err(Error::Corrupt);
            }
            let metadata = self.version_metadata(conn, id, &current)?;
            let candidate = match self.read_version(conn, run, id, &current) {
                Ok(candidate) => Some(candidate),
                Err(Error::Corrupt | Error::Rejected(Reason::InvalidSchema)) => None,
                Err(error) => return Err(error),
            };
            let corrupt = candidate.is_none();
            if let Some(candidate) = candidate
                && self.valid_version(run, &candidate, port)?
            {
                let expected = self.entry(conn, &run.run_id, id)?;
                let mut restored = candidate;
                restored.version_id = new_id();
                restored.parent_version_id = Some(expected.current_version_id.clone());
                restored.restored_from_version_id = Some(current.clone());
                restored.change = ChangeKind::Restore;
                let plan = Plan {
                    expected_logical_clock: self.clock(conn, port, &run.run_id)?,
                    operation_id: new_id(),
                    run_id: run.run_id.clone(),
                    history_revision: boundary.history_revision,
                    entity_revision: boundary.entity_revision,
                    evidence_revision: boundary.evidence_revision,
                    policy_hash: run.policy_hash,
                    targets: vec![Target {
                        entry_id: id.into(),
                        expected: Expected {
                            version_id: Some(expected.current_version_id),
                            state_revision: expected.state_revision,
                        },
                        next: Some(restored),
                        state: decode_enum(&restore_state)?,
                        effects: vec![],
                    }],
                    processed_materials: vec![],
                    reason: "restoreHealthyAncestor".into(),
                };
                self.prepare(conn, port, &plan)?;
                self.write_versions(conn, &plan.operation_id)?;
                self.apply(conn, port, &plan.operation_id)?;
                return Ok(EntryRecovery::Restored);
            }
            let sources = self.version_sources(conn, &current)?;
            if metadata.change == "correction" && self.any_valid_source(run, &sources, port)? {
                self.finish_entry_recovery(conn, id)?;
                return Ok(EntryRecovery::Unavailable);
            }
            if corrupt {
                conn.execute(
                    "UPDATE memory_versions SET quarantined=1 WHERE version_id=?1",
                    [&current],
                )
                .map_err(sql)?;
            }
            let next = match metadata.parent {
                Some(parent) => {
                    let legal:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM memory_versions parent JOIN memory_versions child ON child.version_id=?1 WHERE parent.version_id=?2 AND parent.entry_id=child.entry_id AND parent.rowid<child.rowid)",params![current,parent],read_flag).map_err(sql)?;
                    if !legal {
                        return Err(Error::Corrupt);
                    }
                    parent
                }
                None => String::new(),
            };
            conn.execute(
                "UPDATE memory_entries SET recovery_cursor=?1 WHERE entry_id=?2",
                params![next, id],
            )
            .map_err(sql)?;
            current = next;
        }
        Ok(EntryRecovery::Unavailable)
    }
    fn any_valid_source(
        &self,
        run: &Run,
        sources: &[SourceRef],
        port: &dyn CausalPort,
    ) -> Result<bool> {
        for source in sources {
            if self.valid_sources(run, std::slice::from_ref(source), port)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn finish_entry_recovery(&self, conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE memory_entries SET recovery_cursor='' WHERE entry_id=?1",
            [id],
        )
        .map_err(sql)?;
        Ok(())
    }
    /// 仅回收被拒绝且完全无引用的准备正文；所有已应用因果版本保留供审计。
    /// # Errors
    /// 清理与提交 / 恢复共用单写者；文件内容冲突或 I/O 失败保留清理资格待核验。
    pub fn cleanup(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        run_id: &str,
        limit: usize,
    ) -> Result<usize> {
        let (run, _) = self.boundary(conn, run_id, port)?;
        self.ensure_writable(conn, &run.run_id)?;
        if limit == 0 || limit > MAX_SCAN {
            return Err(reject());
        }
        let mut statement=conn.prepare("SELECT v.version_id,v.entry_id FROM memory_versions v JOIN memory_entries e ON e.entry_id=v.entry_id WHERE e.run_id=?1 AND NOT EXISTS(SELECT 1 FROM memory_entries x WHERE x.current_version_id=v.version_id OR x.recovery_cursor=v.version_id) AND NOT EXISTS(SELECT 1 FROM memory_versions x WHERE x.parent_version_id=v.version_id OR x.restored_from_version_id=v.version_id) AND EXISTS(SELECT 1 FROM memory_operation_targets t JOIN memory_operations o USING(operation_id) WHERE t.next_version_id=v.version_id AND o.state='rejected') AND NOT EXISTS(SELECT 1 FROM memory_operation_targets t JOIN memory_operations o USING(operation_id) WHERE (t.next_version_id=v.version_id OR t.expected_version_id=v.version_id) AND o.state!='rejected') AND NOT EXISTS(SELECT 1 FROM memory_cleanup c WHERE c.version_id=v.version_id AND c.status='removed') ORDER BY v.rowid LIMIT ?2").map_err(sql)?;
        let candidates = statement
            .query_map(params![run_id, limit], read_cleanup)
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        drop(statement);
        let mut removed = 0;
        for (version_id, entry_id) in candidates {
            conn.execute(
                "INSERT OR IGNORE INTO memory_cleanup VALUES(?1,?2,?3,'pending')",
                params![version_id, run_id, entry_id],
            )
            .map_err(sql)?;
            let path = self.version_path(&run, &entry_id, &version_id)?;
            if let Some(bytes) =
                aoidos_store::read::read_text_bounded(&path, MAX_VERSION_BYTES as u32)?
            {
                if hash(bytes.as_bytes())
                    != self.version_metadata(conn, &entry_id, &version_id)?.hash
                {
                    return Err(Error::Corrupt);
                }
                std::fs::remove_file(&path).map_err(aoidos_store::error::StoreError::from_io)?;
                aoidos_store::atomic::sync_parent(&path)?;
            }
            conn.execute(
                "UPDATE memory_cleanup SET status='removed' WHERE version_id=?1",
                [version_id],
            )
            .map_err(sql)?;
            removed += 1;
        }
        Ok(removed)
    }
}
enum EntryRecovery {
    Healthy,
    Restored,
    Unavailable,
    Deferred,
}
fn read_number(row: &rusqlite::Row<'_>) -> rusqlite::Result<u64> {
    row.get(0)
}
fn read_flag(row: &rusqlite::Row<'_>) -> rusqlite::Result<bool> {
    row.get(0)
}
fn read_operation_size(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, String, usize, usize)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}
fn read_entry_position(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, String)> {
    Ok((row.get(0)?, row.get(1)?))
}
fn read_recovery(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(Option<String>, Option<u64>, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}
fn read_cleanup(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String)> {
    Ok((row.get(0)?, row.get(1)?))
}

#[cfg(test)]
mod tests;
