//! 周目删除前只读核验；实际删除由后续 journal 执行器负责。

use super::{RunState, corrupt, database, stale_revision};
use crate::fault::Fault;
use rusqlite::OptionalExtension;
use serde::Serialize;

type CatalogDeletionRow = (String, String, Option<String>, Option<String>, String, i64);

/// 删除确认前的只读计费门禁结果；不包含账单正文或磁盘路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDeletionCheck {
    pub session_id: String,
    pub script_id: String,
    pub created_at: String,
    pub revision: u64,
    pub active: bool,
    pub unresolved_request_count: u64,
    pub pending_settlement_count: u64,
}

impl RunDeletionCheck {
    /// 只有账本中不存在未决义务时才允许进入后续删除确认。
    #[must_use]
    pub const fn billing_clear(&self) -> bool {
        self.unresolved_request_count == 0 && self.pending_settlement_count == 0
    }
}

/// 核验周目身份并检查所有已知账本范围的未决义务；只读，不启动删除。
///
/// # Errors
/// 目录 revision 过期、三身份映射不一致或 SQLite 状态损坏时失败关闭。
pub fn deletion_check(
    connection: &rusqlite::Connection,
    session_id: &str,
    expected_revision: u64,
) -> Result<RunDeletionCheck, Fault> {
    if !crate::record::format::valid_uuid(session_id) || expected_revision == 0 {
        return Err(Fault::bad_request());
    }
    let row: Option<CatalogDeletionRow> = connection
        .query_row(
            "SELECT script_id,created_at,memory_run_id,billing_run_id,state,revision FROM session_catalog WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((script_id, created_at, memory_run_id, billing_run_id, state, revision)) = row else {
        return Err(Fault::new("app.not-found", "周目不存在"));
    };
    let revision = u64::try_from(revision).map_err(|_| corrupt())?;
    if revision != expected_revision {
        return Err(stale_revision());
    }
    let state = RunState::parse(&state)?;
    if matches!(state, RunState::Deleting | RunState::NeedsRepair) {
        return Err(Fault::new("app.conflict", "该周目当前不能删除"));
    }
    if !aoidos_script::valid_id(&script_id)
        || created_at.is_empty()
        || created_at.len() > 64
        || billing_run_id.as_deref() != Some(session_id)
    {
        return Err(corrupt());
    }

    // 始终反查会话映射，避免目录字段丢失时漏过 memoryRunId 账本。
    let mapping: Option<(String, String, Option<String>)> = connection
        .query_row(
            "SELECT sessions.run_id,sessions.session_id,runs.script_id FROM memory_run_sessions sessions LEFT JOIN memory_runs runs ON runs.run_id=sessions.run_id WHERE sessions.session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(database)?;
    let mut scopes = vec![session_id.to_owned()];
    match (memory_run_id, mapping) {
        (Some(memory_run_id), Some((mapped_run, mapped_session, Some(mapped_script))))
            if crate::record::format::valid_uuid(&memory_run_id)
                && memory_run_id == mapped_run
                && mapped_session == session_id
                && mapped_script == script_id =>
        {
            if memory_run_id != session_id {
                scopes.push(memory_run_id);
            }
        }
        (None, None) => {}
        _ => return Err(Fault::new("app.conflict", "周目的身份关联需要修复")),
    }

    let scope_refs = scopes.iter().map(String::as_str).collect::<Vec<_>>();
    let obligations = crate::billing::BillingLedger::deletion_obligations(connection, &scope_refs)
        .map_err(|error| match error {
            crate::billing::BillingError::Invalid => Fault::bad_request(),
            _ => Fault::new("store.database", "费用账本检查失败"),
        })?;
    let active = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM active_session WHERE id=1 AND session_id=?1)",
            [session_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database)?;

    Ok(RunDeletionCheck {
        session_id: session_id.to_owned(),
        script_id,
        created_at,
        revision,
        active,
        unresolved_request_count: obligations.unresolved_request_count,
        pending_settlement_count: obligations.pending_settlement_count,
    })
}
