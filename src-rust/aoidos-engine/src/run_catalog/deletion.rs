//! 周目删除前只读核验；实际删除由后续 journal 执行器负责。

use super::{RunState, bump_revision, corrupt, database, stale_revision};
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

/// 已持久化的删除操作状态；manifest 在 owner 关闭后才冻结。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletionOperation {
    pub operation_id: String,
    pub session_id: String,
    pub state: String,
    pub catalog_revision: u64,
}

/// 在同一 SQLite 事务中登记删除意图并封锁目标周目。
///
/// 该底层原语暂不暴露给应用入口；必须与生命周期互斥、owner 关闭和 journal 前向执行器一起接入。
/// 相同操作重放返回现有状态，不同操作不能并发占用同一周目。
///
/// # Errors
/// 身份、目录 revision 或账单门禁失效时拒绝登记。
pub fn request_deletion(
    connection: &mut rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
    expected_revision: u64,
    updated_at: &str,
) -> Result<DeletionOperation, Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
        || expected_revision == 0
        || updated_at.is_empty()
        || updated_at.len() > 64
    {
        return Err(Fault::bad_request());
    }

    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    if let Some(operation) = transaction
        .query_row(
            "SELECT session_id,state FROM session_deletion_ops WHERE operation_id=?1",
            [operation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(database)?
    {
        if operation.0 != session_id {
            return Err(Fault::new("app.conflict", "删除操作编号已用于其他周目"));
        }
        let revision = catalog_revision(&transaction)?;
        transaction.commit().map_err(database)?;
        return Ok(DeletionOperation {
            operation_id: operation_id.to_owned(),
            session_id: session_id.to_owned(),
            state: operation.1,
            catalog_revision: revision,
        });
    }

    let pending_for_session: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session_deletion_ops WHERE session_id=?1 AND state<>'completed')",
            [session_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if pending_for_session {
        return Err(Fault::new("app.conflict", "该周目已有删除操作待恢复"));
    }

    let check = deletion_check(&transaction, session_id, expected_revision)?;
    if !check.billing_clear() {
        return Err(Fault::new(
            "billing.settlement-pending",
            "该周目仍有未完成的费用结算",
        ));
    }
    let next_revision = expected_revision.checked_add(1).ok_or_else(corrupt)?;
    transaction
        .execute(
            "INSERT INTO session_deletion_ops(operation_id,session_id,state,created_at,updated_at) VALUES(?1,?2,'requested',?3,?3)",
            rusqlite::params![operation_id, session_id, updated_at],
        )
        .map_err(database)?;
    let changed = transaction
        .execute(
            "UPDATE session_catalog SET state='deleting',revision=?1,updated_at=?2 WHERE session_id=?3 AND revision=?4",
            rusqlite::params![next_revision, updated_at, session_id, expected_revision],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(stale_revision());
    }
    let catalog_revision = u64::try_from(bump_revision(&transaction)?).map_err(|_| corrupt())?;
    transaction.commit().map_err(database)?;
    Ok(DeletionOperation {
        operation_id: operation_id.to_owned(),
        session_id: session_id.to_owned(),
        state: "requested".into(),
        catalog_revision,
    })
}

/// owner 已由 app 生命周期门关闭后，清除指向目标的 SQLite 权威活动选择。
///
/// 调用方必须先持有共享生命周期锁、确认没有在飞回合并关闭 owner；本函数不推进 journal，
/// 因为 `owner-closed` 必须与冻结删除 manifest 在同一事务提交。清除活动文件镜像由 app 完成。
/// 重放时只在活动选择仍指向目标的情况下更新 revision。
///
/// # Errors
/// 操作身份不匹配、阶段不合法或 SQLite 写入失败时拒绝修改。
pub fn clear_active_selection_for_deletion(
    connection: &mut rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
    updated_at: &str,
) -> Result<DeletionOperation, Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
        || updated_at.is_empty()
        || updated_at.len() > 64
    {
        return Err(Fault::bad_request());
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let (target_id, state): (String, String) = transaction
        .query_row(
            "SELECT session_id,state FROM session_deletion_ops WHERE operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(|| Fault::new("app.not-found", "删除操作不存在"))?;
    if target_id != session_id {
        return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
    }
    if state == "requested" {
        let changed = transaction
            .execute(
                "UPDATE active_session SET session_id=NULL,revision=revision+1,updated_at=?1 WHERE id=1 AND session_id=?2",
                rusqlite::params![updated_at, session_id],
            )
            .map_err(database)?;
        if changed > 0 {
            bump_revision(&transaction)?;
        }
    } else if !matches!(
        state.as_str(),
        "owner-closed" | "files-quarantined" | "database-purged" | "files-purged" | "completed"
    ) {
        return Err(corrupt());
    }
    let revision = catalog_revision(&transaction)?;
    transaction.commit().map_err(database)?;
    Ok(DeletionOperation {
        operation_id: operation_id.to_owned(),
        session_id: session_id.to_owned(),
        state,
        catalog_revision: revision,
    })
}

fn catalog_revision(connection: &rusqlite::Connection) -> Result<u64, Fault> {
    connection
        .query_row(
            "SELECT revision FROM session_catalog_meta WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database)
        .and_then(|revision| u64::try_from(revision).map_err(|_| corrupt()))
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
