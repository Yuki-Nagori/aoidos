//! 周目开局生成的持久化边界；旧周目没有记录时继续使用 legacy 恢复语义。

use rusqlite::OptionalExtension;
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fault::Fault;

pub const SCHEMA: &str = r#"
CREATE TABLE cycle_initializations (
  session_id TEXT PRIMARY KEY REFERENCES session_catalog(session_id) ON DELETE CASCADE,
  state TEXT NOT NULL CHECK(state IN ('initializing','failed','unknown','playable','abandoned')),
  revision INTEGER NOT NULL CHECK(revision > 0),
  current_request_id TEXT NOT NULL CHECK(length(current_request_id) = 36),
  input_snapshot BLOB NOT NULL CHECK(length(input_snapshot) BETWEEN 2 AND 1048576),
  input_hash TEXT NOT NULL CHECK(length(input_hash) = 64),
  output_snapshot BLOB CHECK(output_snapshot IS NULL OR length(output_snapshot) BETWEEN 2 AND 1048576),
  output_hash TEXT CHECK(output_hash IS NULL OR length(output_hash) = 64),
  created_at TEXT NOT NULL CHECK(length(created_at) BETWEEN 1 AND 64),
  updated_at TEXT NOT NULL CHECK(length(updated_at) BETWEEN 1 AND 64),
  CHECK((state = 'playable') = (output_snapshot IS NOT NULL AND output_hash IS NOT NULL)),
  CHECK(state <> 'playable' OR output_snapshot IS NOT NULL)
);
CREATE TABLE cycle_initialization_attempts (
  request_id TEXT PRIMARY KEY CHECK(length(request_id) = 36),
  session_id TEXT NOT NULL REFERENCES cycle_initializations(session_id) ON DELETE CASCADE,
  state TEXT NOT NULL CHECK(state IN ('pending','failed','unknown','accepted')),
  created_at TEXT NOT NULL CHECK(length(created_at) BETWEEN 1 AND 64),
  updated_at TEXT NOT NULL CHECK(length(updated_at) BETWEEN 1 AND 64)
);
CREATE INDEX cycle_initialization_attempts_by_session
  ON cycle_initialization_attempts(session_id, created_at, request_id);
"#;

/// 标记开局 attempt 是否已取得唯一 dispatch 权；迁移旧初始化记录时默认未领取。
pub const DISPATCH_MIGRATION: &str = r#"
ALTER TABLE cycle_initialization_attempts
  ADD COLUMN dispatch_started INTEGER NOT NULL DEFAULT 0 CHECK(dispatch_started IN (0,1));
"#;

const MAX_SNAPSHOT_BYTES: usize = 1_048_576;

/// Installs the first initialization attempt while the creation coordinator owns its
/// transaction. This deliberately accepts a recovery-required catalog entry: the run stays
/// non-playable until an explicit recovery/dispatch workflow claims the request.
pub(crate) fn prepare_in_transaction(
    transaction: &Transaction<'_>,
    session_id: &str,
    request_id: &str,
    input_snapshot: &[u8],
    created_at: &str,
) -> Result<(), Fault> {
    validate_identity(session_id, request_id, created_at)?;
    validate_json(input_snapshot)?;

    let catalog: Option<(String, Option<Vec<u8>>, Option<String>)> = transaction
        .query_row(
            "SELECT state,profile_snapshot,billing_run_id FROM session_catalog WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(database)?;
    if !catalog.is_some_and(|(state, profile, billing_run_id)| {
        state == "recovery-required"
            && profile.is_some()
            && billing_run_id.as_deref() == Some(session_id)
    }) {
        return Err(Fault::new("app.conflict", "周目尚未准备好开局"));
    }
    let budget_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM billing_run_budgets WHERE run_id=?1)",
            [session_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if !budget_exists {
        return Err(Fault::new("budget.not-configured", "请先完成周目预算设置"));
    }
    let already_initialized: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM cycle_initializations WHERE session_id=?1)",
            [session_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    let request_used: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM cycle_initialization_attempts WHERE request_id=?1)",
            [request_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if already_initialized || request_used {
        return Err(Fault::new("app.conflict", "周目开局已登记"));
    }

    let input_hash = hash(input_snapshot);
    transaction
        .execute(
            "INSERT INTO cycle_initializations(session_id,state,revision,current_request_id,input_snapshot,input_hash,created_at,updated_at) VALUES(?1,'initializing',1,?2,?3,?4,?5,?5)",
            rusqlite::params![session_id, request_id, input_snapshot, input_hash, created_at],
        )
        .map_err(database)?;
    transaction
        .execute(
            "INSERT INTO cycle_initialization_attempts(request_id,session_id,state,created_at,updated_at) VALUES(?1,?2,'pending',?3,?3)",
            rusqlite::params![request_id, session_id, created_at],
        )
        .map_err(database)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Initializing,
    Failed,
    Unknown,
    Playable,
    Abandoned,
}
impl State {
    fn as_str(self) -> &'static str {
        match self {
            Self::Initializing => "initializing",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Playable => "playable",
            Self::Abandoned => "abandoned",
        }
    }
    fn parse(value: &str) -> Result<Self, Fault> {
        match value {
            "initializing" => Ok(Self::Initializing),
            "failed" => Ok(Self::Failed),
            "unknown" => Ok(Self::Unknown),
            "playable" => Ok(Self::Playable),
            "abandoned" => Ok(Self::Abandoned),
            _ => Err(corrupt()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub session_id: String,
    pub state: State,
    pub revision: u64,
    pub request_id: String,
    pub input_snapshot: Vec<u8>,
    pub input_hash: String,
    pub output_snapshot: Option<Vec<u8>>,
    pub output_hash: Option<String>,
}

/// 首次创建时必须在请求网络之前保存的不可变输入和物理请求身份。
#[derive(Debug, Clone)]
pub struct Begin {
    pub session_id: String,
    pub expected_catalog_revision: u64,
    pub request_id: String,
    pub input_snapshot: Vec<u8>,
    pub created_at: String,
}

/// 对新周目持久化一次开局尝试；要求 run 已有 profile 和双币预算。
///
/// # Errors
/// 缺失 / 未配置的 run、无效身份、重复冲突或 SQLite 状态不一致时拒绝。
pub fn begin(connection: &mut rusqlite::Connection, begin: &Begin) -> Result<Snapshot, Fault> {
    validate_identity(&begin.session_id, &begin.request_id, &begin.created_at)?;
    validate_json(&begin.input_snapshot)?;
    if begin.expected_catalog_revision == 0 {
        return Err(Fault::bad_request());
    }
    let input_hash = hash(&begin.input_snapshot);
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let (state, profile, catalog_revision): (String, Option<Vec<u8>>, i64) = transaction
        .query_row(
            "SELECT state,profile_snapshot,revision FROM session_catalog WHERE session_id=?1",
            [&begin.session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(|| Fault::new("app.not-found", "周目不存在"))?;
    if u64::try_from(catalog_revision).map_err(|_| corrupt())? != begin.expected_catalog_revision {
        return Err(Fault::new("app.conflict", "周目目录已变化"));
    }
    if profile.is_none() || state != "ready" {
        return Err(Fault::new(
            "budget.not-configured",
            "请先完成周目模型与预算设置",
        ));
    }
    let budget_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM billing_run_budgets WHERE run_id=?1)",
            [&begin.session_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if !budget_exists {
        return Err(Fault::new("budget.not-configured", "请先完成周目预算设置"));
    }
    if let Some(existing) = transaction
        .query_row(
            "SELECT state,current_request_id,input_snapshot,input_hash FROM cycle_initializations WHERE session_id=?1",
            [&begin.session_id],
            |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?,row.get::<_,String>(3)?)),
        )
        .optional()
        .map_err(database)?
    {
        if existing.0 == "initializing"
            && existing.1 == begin.request_id
            && existing.2 == begin.input_snapshot
            && existing.3 == input_hash
        {
            transaction.commit().map_err(database)?;
            return load(connection, &begin.session_id);
        }
        return Err(Fault::new("app.conflict", "周目开局已登记"));
    }
    transaction
        .execute(
            "INSERT INTO cycle_initializations(session_id,state,revision,current_request_id,input_snapshot,input_hash,created_at,updated_at) VALUES(?1,'initializing',1,?2,?3,?4,?5,?5)",
            rusqlite::params![begin.session_id, begin.request_id, begin.input_snapshot, input_hash, begin.created_at],
        )
        .map_err(database)?;
    transaction
        .execute(
            "INSERT INTO cycle_initialization_attempts(request_id,session_id,state,created_at,updated_at) VALUES(?1,?2,'pending',?3,?3)",
            rusqlite::params![begin.request_id, begin.session_id, begin.created_at],
        )
        .map_err(database)?;
    transaction
        .commit()
        .map_err(|_| Fault::new("store.commit-unknown", "周目开局登记结果不确定"))?;
    load(connection, &begin.session_id)
}

/// 读取初始化快照；没有初始化记录代表历史 / legacy 周目。
pub fn load(connection: &rusqlite::Connection, session_id: &str) -> Result<Snapshot, Fault> {
    if !crate::record::format::valid_uuid(session_id) {
        return Err(Fault::bad_request());
    }
    let row = connection
        .query_row(
            "SELECT state,revision,current_request_id,input_snapshot,input_hash,output_snapshot,output_hash FROM cycle_initializations WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?,row.get::<_,Vec<u8>>(3)?,row.get::<_,String>(4)?,row.get::<_,Option<Vec<u8>>>(5)?,row.get::<_,Option<String>>(6)?)),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(|| Fault::new("app.not-found", "该周目没有开局快照"))?;
    let snapshot = Snapshot {
        session_id: session_id.to_owned(),
        state: State::parse(&row.0)?,
        revision: u64::try_from(row.1).map_err(|_| corrupt())?,
        request_id: row.2,
        input_snapshot: row.3,
        input_hash: row.4,
        output_snapshot: row.5,
        output_hash: row.6,
    };
    validate_snapshot(&snapshot)?;
    let attempt_state: Option<String> = connection
        .query_row(
            "SELECT state FROM cycle_initialization_attempts WHERE request_id=?1 AND session_id=?2",
            rusqlite::params![snapshot.request_id, session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?;
    let valid_attempt = match snapshot.state {
        State::Initializing => attempt_state.as_deref() == Some("pending"),
        State::Failed => attempt_state.as_deref() == Some("failed"),
        State::Unknown => attempt_state.as_deref() == Some("unknown"),
        State::Playable => attempt_state.as_deref() == Some("accepted"),
        State::Abandoned => matches!(
            attempt_state.as_deref(),
            Some("pending" | "failed" | "unknown")
        ),
    };
    if !valid_attempt {
        return Err(corrupt());
    }
    Ok(snapshot)
}

/// 将一次可判定失败 / unknown 的结果保留在同一尝试记录中。
pub fn finish_attempt(
    connection: &mut rusqlite::Connection,
    session_id: &str,
    expected_revision: u64,
    request_id: &str,
    state: State,
    updated_at: &str,
) -> Result<u64, Fault> {
    if !matches!(state, State::Failed | State::Unknown)
        || !crate::record::format::valid_uuid(session_id)
        || !crate::record::format::valid_uuid(request_id)
        || expected_revision == 0
        || updated_at.is_empty()
        || updated_at.len() > 64
    {
        return Err(Fault::bad_request());
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let next = expected_revision.checked_add(1).ok_or_else(corrupt)?;
    if let Some((state_now, revision_now, request_now)) = transaction
        .query_row(
            "SELECT state,revision,current_request_id FROM cycle_initializations WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?)),
        )
        .optional()
        .map_err(database)?
        && state_now == state.as_str()
            && u64::try_from(revision_now).map_err(|_| corrupt())? == next
            && request_now == request_id
    {
        let attempt_matches: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cycle_initialization_attempts WHERE request_id=?1 AND session_id=?2 AND state=?3)",
                rusqlite::params![request_id, session_id, state.as_str()],
                |row| row.get(0),
            )
            .map_err(database)?;
        if attempt_matches {
            transaction.commit().map_err(database)?;
            return Ok(next);
        }
        return Err(corrupt());
    }
    let changed = transaction
        .execute(
            "UPDATE cycle_initializations SET state=?1,revision=?2,updated_at=?3 WHERE session_id=?4 AND revision=?5 AND current_request_id=?6 AND state='initializing'",
            rusqlite::params![state.as_str(), next, updated_at, session_id, expected_revision, request_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "开局尝试已变化"));
    }
    let changed = transaction
        .execute(
            "UPDATE cycle_initialization_attempts SET state=?1,updated_at=?2 WHERE request_id=?3 AND session_id=?4 AND state='pending'",
            rusqlite::params![state.as_str(), updated_at, request_id, session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(corrupt());
    }
    transaction.commit().map_err(database)?;
    Ok(next)
}

/// 在调用方的 IMMEDIATE 事务内领取唯一 dispatch 权。
///
/// 调用方应在同一事务中写入物理费用预留，然后提交后才允许访问 Provider。
/// 重复或并发领取一律冲突，即使账本预留本身可以幂等，也不能据此再次发送。
pub(crate) fn claim_dispatch_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    session_id: &str,
    expected_revision: u64,
    request_id: &str,
    updated_at: &str,
) -> Result<(), Fault> {
    if !crate::record::format::valid_uuid(session_id)
        || !crate::record::format::valid_uuid(request_id)
        || expected_revision == 0
        || updated_at.is_empty()
        || updated_at.len() > 64
    {
        return Err(Fault::bad_request());
    }
    let claimed = transaction
        .execute(
            "UPDATE cycle_initialization_attempts SET dispatch_started=1,updated_at=?1 WHERE request_id=?2 AND session_id=?3 AND state='pending' AND dispatch_started=0 AND EXISTS(SELECT 1 FROM cycle_initializations WHERE session_id=?3 AND state='initializing' AND revision=?4 AND current_request_id=?2)",
            rusqlite::params![updated_at, request_id, session_id, expected_revision],
        )
        .map_err(database)?;
    if claimed != 1 {
        return Err(Fault::new("app.conflict", "开局请求已发送或状态已变化"));
    }
    Ok(())
}

/// 显式重试沿用冻结输入 / run / budget，仅产生新的物理 requestId。
pub fn retry(
    connection: &mut rusqlite::Connection,
    session_id: &str,
    expected_revision: u64,
    previous_request_id: &str,
    request_id: &str,
    updated_at: &str,
) -> Result<u64, Fault> {
    if !crate::record::format::valid_uuid(session_id)
        || !crate::record::format::valid_uuid(previous_request_id)
        || !crate::record::format::valid_uuid(request_id)
        || previous_request_id == request_id
        || expected_revision == 0
        || updated_at.is_empty()
        || updated_at.len() > 64
    {
        return Err(Fault::bad_request());
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let next = expected_revision.checked_add(1).ok_or_else(corrupt)?;
    if let Some((state_now, revision_now, request_now)) = transaction
        .query_row(
            "SELECT state,revision,current_request_id FROM cycle_initializations WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?)),
        )
        .optional()
        .map_err(database)?
        && state_now == "initializing"
            && u64::try_from(revision_now).map_err(|_| corrupt())? == next
            && request_now == request_id
            && {
                let pending: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM cycle_initialization_attempts WHERE request_id=?1 AND session_id=?2 AND state='pending')",
                        rusqlite::params![request_id, session_id],
                        |row| row.get(0),
                    )
                    .map_err(database)?;
                pending
            }
    {
        transaction.commit().map_err(database)?;
        return Ok(next);
    }
    let changed = transaction
        .execute(
            "UPDATE cycle_initializations SET state='initializing',revision=revision+1,current_request_id=?1,updated_at=?2 WHERE session_id=?3 AND revision=?4 AND current_request_id=?5 AND state IN ('failed','unknown')",
            rusqlite::params![request_id, updated_at, session_id, expected_revision, previous_request_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "当前开局状态不可重试"));
    }
    transaction
        .execute(
            "INSERT INTO cycle_initialization_attempts(request_id,session_id,state,created_at,updated_at) VALUES(?1,?2,'pending',?3,?3)",
            rusqlite::params![request_id, session_id, updated_at],
        )
        .map_err(database)?;
    transaction.commit().map_err(database)?;
    Ok(next)
}

/// 将完整、规范化的开局产物与 playable 状态在同一 CAS 事务中冻结。
pub fn finalize(
    connection: &mut rusqlite::Connection,
    session_id: &str,
    expected_revision: u64,
    request_id: &str,
    output: &crate::cycle_output::ValidatedOutput,
    updated_at: &str,
) -> Result<u64, Fault> {
    if !crate::record::format::valid_uuid(session_id)
        || !crate::record::format::valid_uuid(request_id)
        || expected_revision == 0
        || updated_at.is_empty()
        || updated_at.len() > 64
    {
        return Err(Fault::bad_request());
    }
    let output_snapshot = output.as_bytes();
    validate_json(output_snapshot)?;
    let output_hash = hash(output_snapshot);
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let next = expected_revision.checked_add(1).ok_or_else(corrupt)?;
    let (input_snapshot, input_hash): (Vec<u8>, String) = transaction
        .query_row(
            "SELECT input_snapshot,input_hash FROM cycle_initializations WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(|| Fault::new("app.not-found", "周目开局记录不存在"))?;
    let (script_id, profile_id, profile_snapshot): (String, Option<String>, Option<Vec<u8>>) =
        transaction
            .query_row(
                "SELECT script_id,profile_id,profile_snapshot FROM session_catalog WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(database)?
            .ok_or_else(|| Fault::new("app.not-found", "周目不存在"))?;
    if input_snapshot.len() > MAX_SNAPSHOT_BYTES || hash(&input_snapshot) != input_hash {
        return Err(corrupt());
    }
    let frozen: crate::run_catalog::FrozenInitializationInput =
        aoidos_json::decode_bytes(&input_snapshot, MAX_SNAPSHOT_BYTES).map_err(|_| corrupt())?;
    let profile_snapshot = profile_snapshot.ok_or_else(corrupt)?;
    if frozen.version != 1
        || !crate::record::format::valid_uuid(&frozen.operation_id)
        || frozen.session_id != session_id
        || frozen.script_id != script_id
        || frozen.profile_id != profile_id.as_deref().unwrap_or_default()
        || frozen.profile_snapshot_hash != hash(&profile_snapshot)
        || !crate::record::format::valid_hash(&frozen.script_revision)
        || !matches!(frozen.locale.as_str(), "zh-Hans" | "en")
        || frozen.prompt_version != "init-v1"
        || frozen.output_schema_version != 2
        || !frozen.provider_request.is_object()
        || frozen.script_snapshot.len() > MAX_SNAPSHOT_BYTES
        || frozen.entity_snapshot.len() > MAX_SNAPSHOT_BYTES
        || frozen.cycle_input_snapshot.len() > 128 * 1024
        || !output.is_bound_to(
            session_id,
            request_id,
            &frozen.locale,
            &frozen.cycle_input_snapshot,
            &frozen.entity_snapshot,
        )
    {
        return Err(Fault::new("app.conflict", "开局输出与冻结输入不匹配"));
    }
    crate::game::assets::validate_frozen_initialization_assets(
        &frozen.script_id,
        &frozen.script_revision,
        &frozen.script_snapshot,
        &frozen.entity_snapshot,
        &frozen.cycle_input_snapshot,
    )
    .map_err(|_| Fault::new("app.conflict", "冻结剧本资源校验失败"))?;

    if let Some((state_now, revision_now, request_now, output_now, hash_now)) = transaction
        .query_row(
            "SELECT state,revision,current_request_id,output_snapshot,output_hash FROM cycle_initializations WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?,row.get::<_,Option<Vec<u8>>>(3)?,row.get::<_,Option<String>>(4)?)),
        )
        .optional()
        .map_err(database)?
        && state_now == "playable"
            && u64::try_from(revision_now).map_err(|_| corrupt())? == next
            && request_now == request_id
            && output_now.as_deref() == Some(output_snapshot)
            && hash_now.as_deref() == Some(output_hash.as_str())
    {
        let attempt_accepted: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cycle_initialization_attempts WHERE request_id=?1 AND session_id=?2 AND state='accepted' AND dispatch_started=1)",
                rusqlite::params![request_id, session_id],
                |row| row.get(0),
            )
            .map_err(database)?;
        if attempt_accepted {
            transaction.commit().map_err(database)?;
            return Ok(next);
        }
        return Err(corrupt());
    }
    let dispatch_claimed: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM cycle_initialization_attempts WHERE request_id=?1 AND session_id=?2 AND state='pending' AND dispatch_started=1)",
            rusqlite::params![request_id, session_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if !dispatch_claimed {
        return Err(Fault::new("app.conflict", "开局请求尚未完成派发"));
    }
    let changed = transaction
        .execute(
            "UPDATE cycle_initializations SET state='playable',revision=?1,output_snapshot=?2,output_hash=?3,updated_at=?4 WHERE session_id=?5 AND revision=?6 AND current_request_id=?7 AND state='initializing'",
            rusqlite::params![next, output_snapshot, output_hash, updated_at, session_id, expected_revision, request_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "开局尝试已变化"));
    }
    let changed = transaction
        .execute(
            "UPDATE cycle_initialization_attempts SET state='accepted',updated_at=?1 WHERE request_id=?2 AND session_id=?3 AND state='pending' AND dispatch_started=1",
            rusqlite::params![updated_at, request_id, session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(corrupt());
    }
    transaction
        .commit()
        .map_err(|_| Fault::new("store.commit-unknown", "开局快照提交结果不确定"))?;
    Ok(next)
}

pub fn abandon(
    connection: &mut rusqlite::Connection,
    session_id: &str,
    expected_revision: u64,
    updated_at: &str,
) -> Result<u64, Fault> {
    if !crate::record::format::valid_uuid(session_id)
        || expected_revision == 0
        || updated_at.is_empty()
        || updated_at.len() > 64
    {
        return Err(Fault::bad_request());
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let next = expected_revision.checked_add(1).ok_or_else(corrupt)?;
    if let Some((state_now, revision_now)) = transaction
        .query_row(
            "SELECT state,revision FROM cycle_initializations WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(database)?
        && state_now == "abandoned"
        && u64::try_from(revision_now).map_err(|_| corrupt())? == next
    {
        transaction.commit().map_err(database)?;
        return Ok(next);
    }
    let changed = transaction
        .execute(
            "UPDATE cycle_initializations SET state='abandoned',revision=?1,updated_at=?2 WHERE session_id=?3 AND revision=?4 AND state IN ('initializing','failed','unknown')",
            rusqlite::params![next, updated_at, session_id, expected_revision],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "当前开局状态不可放弃"));
    }
    transaction.commit().map_err(database)?;
    Ok(next)
}

/// 没有行按 legacy 放行；存在行则只允许已经冻结的 playable 周目打开。
pub(crate) fn require_playable(
    connection: &rusqlite::Connection,
    session_id: &str,
) -> Result<(), Fault> {
    match load(connection, session_id) {
        Ok(snapshot) if snapshot.state == State::Playable => Ok(()),
        Ok(snapshot) if snapshot.state == State::Abandoned => {
            Err(Fault::new("app.conflict", "该周目已放弃"))
        }
        Ok(_) => Err(Fault::new("app.conflict", "周目开局尚未完成")),
        Err(error) if error.code == "app.not-found" => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn verify(connection: &rusqlite::Connection) -> Result<(), aoidos_store::error::StoreError> {
    for statement in SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|sql| sql.starts_with("CREATE "))
    {
        let name = statement
            .split_whitespace()
            .nth(2)
            .ok_or_else(schema_mismatch)?
            .trim_matches('(');
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(aoidos_store::db::sqlite_error)?;
        let expected = if name == "cycle_initialization_attempts" {
            format!(
                "{}, dispatch_started INTEGER NOT NULL DEFAULT 0 CHECK(dispatch_started IN (0,1)))",
                statement.strip_suffix(')').ok_or_else(schema_mismatch)?
            )
        } else {
            statement.to_owned()
        };
        if actual.as_deref().map(normalize_sql) != Some(normalize_sql(&expected)) {
            return Err(schema_mismatch());
        }
    }
    let dispatch_column: Option<(String, i64, Option<String>)> = connection
        .query_row(
            "SELECT type,\"notnull\",dflt_value FROM pragma_table_info('cycle_initialization_attempts') WHERE name='dispatch_started'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(aoidos_store::db::sqlite_error)?;
    if dispatch_column != Some(("INTEGER".into(), 1, Some("0".into()))) {
        return Err(schema_mismatch());
    }
    Ok(())
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}
fn schema_mismatch() -> aoidos_store::error::StoreError {
    aoidos_store::error::StoreError::Corrupt("cycle initialization schema mismatch".into())
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<(), Fault> {
    if snapshot.input_snapshot.len() > MAX_SNAPSHOT_BYTES
        || snapshot
            .output_snapshot
            .as_ref()
            .is_some_and(|value| value.len() > MAX_SNAPSHOT_BYTES)
        || snapshot.input_hash != hash(&snapshot.input_snapshot)
        || snapshot.output_hash.as_deref().is_some_and(|value| {
            snapshot.output_snapshot.as_deref().map(hash).as_deref() != Some(value)
        })
        || (snapshot.state == State::Playable)
            != (snapshot.output_snapshot.is_some() && snapshot.output_hash.is_some())
    {
        return Err(corrupt());
    }
    validate_json(&snapshot.input_snapshot)?;
    if let Some(output) = &snapshot.output_snapshot {
        validate_json(output)?;
    }
    Ok(())
}

fn validate_json(bytes: &[u8]) -> Result<(), Fault> {
    if bytes.len() > MAX_SNAPSHOT_BYTES
        || serde_json::from_slice::<serde_json::Value>(bytes).is_err()
    {
        return Err(Fault::bad_request());
    }
    Ok(())
}

fn validate_identity(session_id: &str, request_id: &str, timestamp: &str) -> Result<(), Fault> {
    if !crate::record::format::valid_uuid(session_id)
        || !crate::record::format::valid_uuid(request_id)
        || timestamp.is_empty()
        || timestamp.len() > 64
    {
        return Err(Fault::bad_request());
    }
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn database(_: rusqlite::Error) -> Fault {
    Fault::new("store.database", "周目开局存储失败")
}

fn corrupt() -> Fault {
    Fault::new("store.corrupt", "周目开局数据损坏")
}
#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> rusqlite::Connection {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        connection
            .execute_batch(aoidos_billing::schema::SCHEMA)
            .unwrap();
        connection
            .execute_batch(aoidos_billing::schema::CACHE_WRITE_MIGRATION)
            .unwrap();
        connection
            .execute_batch(aoidos_billing::schema::SETTLEMENT_RECOVERY_MIGRATION)
            .unwrap();
        connection
            .execute_batch(aoidos_billing::schema::SETTLEMENT_DIAGNOSTICS_MIGRATION)
            .unwrap();
        connection
            .execute_batch(aoidos_memory::schema::SCHEMA)
            .unwrap();
        connection
            .execute_batch(crate::run_catalog::SCHEMA)
            .unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection.execute_batch(DISPATCH_MIGRATION).unwrap();
        connection
    }

    fn ready_run(connection: &rusqlite::Connection, session_id: &str) {
        connection
            .execute(
                "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,profile_id,profile_schema_version,profile_snapshot,state,revision,updated_at) VALUES(?1,'mistbell','2026-10-23T00:00:00Z','00000000-0000-4000-8000-000000000002',?1,'test-profile',1,x'7b7d','ready',1,'2026-10-23T00:00:00Z')",
                [session_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,1000000000,0,1,'2026-10-23T00:00:00Z')",
                [session_id],
            )
            .unwrap();
    }

    fn frozen_input(session_id: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let header = crate::game::builtin::header("mistbell").unwrap();
        let snapshots = crate::game::assets::initialization_asset_snapshots(
            &header.script_id,
            &header.script_revision,
        )
        .unwrap();
        let input = crate::run_catalog::FrozenInitializationInput {
            version: 1,
            operation_id: "00000000-0000-4000-8000-000000000010".into(),
            session_id: session_id.into(),
            script_id: header.script_id,
            script_revision: header.script_revision,
            profile_id: "test-profile".into(),
            profile_snapshot_hash: hash(b"{}"),
            locale: "zh-Hans".into(),
            prompt_version: "init-v1".into(),
            output_schema_version: 2,
            script_snapshot: snapshots.script,
            entity_snapshot: snapshots.entities.clone(),
            cycle_input_snapshot: snapshots.cycle_input.clone(),
            provider_request: serde_json::json!({"model":"test","messages":[]}),
        };
        (
            aoidos_json::to_vec(&input).unwrap(),
            snapshots.cycle_input,
            snapshots.entities,
        )
    }

    #[test]
    fn initialization_transitions_are_revision_scoped_and_idempotent() {
        let mut connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000001";
        let request_id = "00000000-0000-4000-8000-000000000003";
        ready_run(&connection, session_id);
        let (input, cycle_input, entities) = frozen_input(session_id);
        let started = begin(
            &mut connection,
            &Begin {
                session_id: session_id.into(),
                expected_catalog_revision: 1,
                request_id: request_id.into(),
                input_snapshot: input.clone(),
                created_at: "2026-10-23T00:00:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(started.state, State::Initializing);
        assert_eq!(started.input_hash, hash(&input));
        assert!(require_playable(&connection, session_id).is_err());
        let repeated_start = begin(
            &mut connection,
            &Begin {
                session_id: session_id.into(),
                expected_catalog_revision: 1,
                request_id: request_id.into(),
                input_snapshot: input.clone(),
                created_at: "2026-10-23T00:00:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(repeated_start, started);

        let failed_revision = finish_attempt(
            &mut connection,
            session_id,
            started.revision,
            request_id,
            State::Unknown,
            "2026-10-23T00:00:01Z",
        )
        .unwrap();
        assert_eq!(
            retry(
                &mut connection,
                session_id,
                failed_revision,
                "00000000-0000-4000-8000-000000000099",
                "00000000-0000-4000-8000-000000000098",
                "2026-10-23T00:00:02Z",
            )
            .unwrap_err()
            .code,
            "app.conflict"
        );
        assert_eq!(load(&connection, session_id).unwrap().state, State::Unknown);
        assert_eq!(
            finish_attempt(
                &mut connection,
                session_id,
                started.revision,
                request_id,
                State::Unknown,
                "2026-10-23T00:00:01Z",
            )
            .unwrap(),
            failed_revision
        );
        let retry_id = "00000000-0000-4000-8000-000000000004";
        let retry_revision = retry(
            &mut connection,
            session_id,
            failed_revision,
            request_id,
            retry_id,
            "2026-10-23T00:00:02Z",
        )
        .unwrap();
        assert_eq!(retry_revision, failed_revision + 1);
        assert_eq!(
            retry(
                &mut connection,
                session_id,
                failed_revision,
                request_id,
                retry_id,
                "2026-10-23T00:00:02Z",
            )
            .unwrap(),
            retry_revision
        );
        assert_eq!(load(&connection, session_id).unwrap().input_snapshot, input);

        let output = crate::cycle_output::tests::validated_fixture(
            session_id,
            retry_id,
            "zh-Hans",
            &cycle_input,
            &entities,
        );
        let wrong_context = crate::cycle_output::tests::validated_fixture(
            session_id,
            "00000000-0000-4000-8000-000000000005",
            "zh-Hans",
            &cycle_input,
            &entities,
        );
        assert_eq!(
            finalize(
                &mut connection,
                session_id,
                retry_revision,
                retry_id,
                &wrong_context,
                "2026-10-23T00:00:02Z",
            )
            .unwrap_err()
            .code,
            "app.conflict"
        );

        assert_eq!(
            finalize(
                &mut connection,
                session_id,
                retry_revision,
                retry_id,
                &output,
                "2026-10-23T00:00:02Z",
            )
            .unwrap_err()
            .code,
            "app.conflict"
        );
        assert_eq!(
            load(&connection, session_id).unwrap().state,
            State::Initializing
        );

        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        claim_dispatch_in_transaction(
            &transaction,
            session_id,
            retry_revision,
            retry_id,
            "2026-10-23T00:00:02Z",
        )
        .unwrap();
        transaction.commit().unwrap();

        connection
            .execute(
                "UPDATE cycle_initializations SET input_hash=?1 WHERE session_id=?2",
                rusqlite::params!["0".repeat(64), session_id],
            )
            .unwrap();
        assert_eq!(
            finalize(
                &mut connection,
                session_id,
                retry_revision,
                retry_id,
                &output,
                "2026-10-23T00:00:03Z",
            )
            .unwrap_err()
            .code,
            "store.corrupt"
        );
        let unchanged: (String, i64, Option<Vec<u8>>) = connection
            .query_row(
                "SELECT state,revision,output_snapshot FROM cycle_initializations WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(unchanged.0, "initializing");
        assert_eq!(unchanged.1, retry_revision as i64);
        assert_eq!(
            unchanged.2, None,
            "a rejected finalize must not persist output bytes"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT state,dispatch_started FROM cycle_initialization_attempts WHERE request_id=?1",
                    [retry_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
                )
                .unwrap(),
            ("pending".to_owned(), true),
            "a rejected finalize must leave the dispatched attempt recoverable"
        );
        connection
            .execute(
                "UPDATE cycle_initializations SET input_hash=?1 WHERE session_id=?2",
                rusqlite::params![hash(&input), session_id],
            )
            .unwrap();

        let final_revision = finalize(
            &mut connection,
            session_id,
            retry_revision,
            retry_id,
            &output,
            "2026-10-23T00:00:03Z",
        )
        .unwrap();
        let frozen = load(&connection, session_id).unwrap();
        assert_eq!(frozen.revision, final_revision);
        assert_eq!(frozen.state, State::Playable);
        assert_eq!(
            frozen.output_hash.as_deref(),
            Some(hash(output.as_bytes()).as_str())
        );
        assert_eq!(
            finalize(
                &mut connection,
                session_id,
                retry_revision,
                retry_id,
                &output,
                "2026-10-23T00:00:04Z",
            )
            .unwrap(),
            final_revision
        );
        assert_eq!(
            finalize(
                &mut connection,
                session_id,
                retry_revision - 1,
                retry_id,
                &output,
                "2026-10-23T00:00:04Z",
            )
            .unwrap_err()
            .code,
            "app.conflict"
        );
        assert!(require_playable(&connection, session_id).is_ok());
        connection
            .execute(
                "UPDATE cycle_initializations SET output_hash=?1 WHERE session_id=?2",
                rusqlite::params!["0".repeat(64), session_id],
            )
            .unwrap();
        assert!(require_playable(&connection, session_id).is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM cycle_initialization_attempts",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
    }

    #[test]
    fn dispatch_claim_is_single_use_and_rolls_back_with_its_transaction() {
        let mut connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000041";
        let request_id = "00000000-0000-4000-8000-000000000042";
        ready_run(&connection, session_id);
        begin(
            &mut connection,
            &Begin {
                session_id: session_id.into(),
                expected_catalog_revision: 1,
                request_id: request_id.into(),
                input_snapshot: br#"{"script":"mistbell","schema":"v1"}"#.to_vec(),
                created_at: "2026-10-23T00:00:00Z".into(),
            },
        )
        .unwrap();

        {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            claim_dispatch_in_transaction(
                &transaction,
                session_id,
                1,
                request_id,
                "2026-10-23T00:00:01Z",
            )
            .unwrap();
            // Dropping before commit proves the fence rolls back atomically with reservation work.
        }
        let started: i64 = connection
            .query_row(
                "SELECT dispatch_started FROM cycle_initialization_attempts WHERE request_id=?1",
                [request_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(started, 0);

        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        claim_dispatch_in_transaction(
            &transaction,
            session_id,
            1,
            request_id,
            "2026-10-23T00:00:02Z",
        )
        .unwrap();
        transaction.commit().unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            claim_dispatch_in_transaction(
                &transaction,
                session_id,
                1,
                request_id,
                "2026-10-23T00:00:03Z",
            )
            .unwrap_err()
            .code,
            "app.conflict"
        );
    }

    #[test]
    fn initialization_fails_closed_without_budget_and_rolls_back_attempt_insert() {
        let mut connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000011";
        ready_run(&connection, session_id);
        connection
            .execute(
                "DELETE FROM billing_run_budgets WHERE run_id=?1",
                [session_id],
            )
            .unwrap();
        let result = begin(
            &mut connection,
            &Begin {
                session_id: session_id.into(),
                expected_catalog_revision: 1,
                request_id: "00000000-0000-4000-8000-000000000012".into(),
                input_snapshot: br#"{}"#.to_vec(),
                created_at: "2026-10-23T00:00:00Z".into(),
            },
        );
        assert_eq!(result.unwrap_err().code, "budget.not-configured");
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM cycle_initializations", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );

        connection
            .execute(
                "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,1000000000,0,1,'2026-10-23T00:00:00Z')",
                [session_id],
            )
            .unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_cycle_attempt BEFORE INSERT ON cycle_initialization_attempts BEGIN SELECT RAISE(ABORT,'injected failure'); END;",
            )
            .unwrap();
        assert!(
            begin(
                &mut connection,
                &Begin {
                    session_id: session_id.into(),
                    expected_catalog_revision: 1,
                    request_id: "00000000-0000-4000-8000-000000000013".into(),
                    input_snapshot: br#"{}"#.to_vec(),
                    created_at: "2026-10-23T00:00:00Z".into(),
                },
            )
            .is_err()
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM cycle_initializations", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
    }

    #[test]
    fn retry_request_id_collision_rolls_back_the_initialization_cas() {
        let mut connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000021";
        let initial_request = "00000000-0000-4000-8000-000000000022";
        let retry_request = "00000000-0000-4000-8000-000000000023";
        ready_run(&connection, session_id);
        connection
            .execute(
                "INSERT INTO cycle_initializations(session_id,state,revision,current_request_id,input_snapshot,input_hash,created_at,updated_at) VALUES(?1,'unknown',2,?2,x'7b7d',?3,'2026-10-23T00:00:00Z','2026-10-23T00:00:00Z')",
                rusqlite::params![session_id, initial_request, hash(b"{}")],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cycle_initialization_attempts(request_id,session_id,state,created_at,updated_at) VALUES(?1,?2,'unknown','2026-10-23T00:00:00Z','2026-10-23T00:00:00Z')",
                rusqlite::params![initial_request, session_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cycle_initialization_attempts(request_id,session_id,state,created_at,updated_at) VALUES(?1,?2,'failed','2026-10-23T00:00:00Z','2026-10-23T00:00:00Z')",
                rusqlite::params![retry_request, session_id],
            )
            .unwrap();
        let error = retry(
            &mut connection,
            session_id,
            2,
            initial_request,
            retry_request,
            "2026-10-23T00:00:01Z",
        )
        .unwrap_err();
        assert_eq!(error.code, "store.database");
        let snapshot = load(&connection, session_id).unwrap();
        assert_eq!(snapshot.state, State::Unknown);
        assert_eq!(snapshot.revision, 2);
        assert_eq!(snapshot.request_id, initial_request);
    }

    #[test]
    fn deleting_a_run_cascades_its_initialization_and_attempt_history() {
        let connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000031";
        ready_run(&connection, session_id);
        connection
            .execute(
                "INSERT INTO cycle_initializations(session_id,state,revision,current_request_id,input_snapshot,input_hash,created_at,updated_at) VALUES(?1,'failed',2,'00000000-0000-4000-8000-000000000032',x'7b7d',?2,'2026-10-23T00:00:00Z','2026-10-23T00:00:00Z')",
                rusqlite::params![session_id, hash(b"{}")],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cycle_initialization_attempts(request_id,session_id,state,created_at,updated_at) VALUES('00000000-0000-4000-8000-000000000032',?1,'failed','2026-10-23T00:00:00Z','2026-10-23T00:00:00Z')",
                [session_id],
            )
            .unwrap();
        connection
            .execute(
                "DELETE FROM session_catalog WHERE session_id=?1",
                [session_id],
            )
            .unwrap();
        let remaining: i64 = connection
            .query_row(
                "SELECT (SELECT count(*) FROM cycle_initializations)+(SELECT count(*) FROM cycle_initialization_attempts)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
