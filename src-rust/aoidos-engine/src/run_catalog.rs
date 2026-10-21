//! 已开周目的 SQLite 权威目录与生命周期操作记录。

use aoidos_billing::money::NanoMoney;
use aoidos_llm::config::LlmProfile;
use aoidos_store::error::StoreError;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::fault::Fault;

/// 060 为周目索引、当前选择及可恢复删除流程追加的首个 schema。
pub const SCHEMA: &str = r#"
CREATE TABLE session_catalog_meta (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  revision INTEGER NOT NULL CHECK(revision > 0),
  scan_incomplete INTEGER NOT NULL DEFAULT 1 CHECK(scan_incomplete IN (0, 1))
);
INSERT INTO session_catalog_meta(id, revision) VALUES(1, 1);
CREATE TABLE session_catalog (
  session_id TEXT PRIMARY KEY CHECK(length(session_id) = 36),
  script_id TEXT NOT NULL CHECK(length(script_id) BETWEEN 1 AND 64),
  created_at TEXT NOT NULL CHECK(length(created_at) BETWEEN 1 AND 64),
  memory_run_id TEXT UNIQUE CHECK(memory_run_id IS NULL OR length(memory_run_id) = 36),
  billing_run_id TEXT CHECK(billing_run_id IS NULL OR length(billing_run_id) = 36),
  profile_id TEXT CHECK(profile_id IS NULL OR length(profile_id) BETWEEN 1 AND 256),
  profile_schema_version INTEGER CHECK(profile_schema_version IS NULL OR profile_schema_version > 0),
  profile_snapshot BLOB CHECK(profile_snapshot IS NULL OR length(profile_snapshot) <= 65536),
  state TEXT NOT NULL CHECK(state IN (
    'ready', 'needs-profile-binding', 'billing-not-configured',
    'billing-reconciliation-required', 'recovery-required', 'needs-repair',
    'script-unavailable', 'settlement-pending', 'deleting'
  )),
  revision INTEGER NOT NULL CHECK(revision > 0),
  updated_at TEXT NOT NULL CHECK(length(updated_at) BETWEEN 1 AND 64)
);
CREATE INDEX session_catalog_order ON session_catalog(created_at DESC, session_id DESC);
CREATE TABLE active_session (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  session_id TEXT CHECK(session_id IS NULL OR length(session_id) = 36),
  revision INTEGER NOT NULL CHECK(revision > 0),
  updated_at TEXT NOT NULL CHECK(length(updated_at) BETWEEN 1 AND 64)
);
CREATE TABLE session_deletion_ops (
  operation_id TEXT PRIMARY KEY CHECK(length(operation_id) = 36),
  session_id TEXT NOT NULL CHECK(length(session_id) = 36),
  state TEXT NOT NULL CHECK(state IN (
    'requested', 'owner-closed', 'files-quarantined',
    'database-purged', 'files-purged', 'completed', 'needs-repair'
  )),
  manifest BLOB CHECK(manifest IS NULL OR length(manifest) <= 4194304),
  result_code TEXT CHECK(result_code IS NULL OR length(result_code) <= 64),
  created_at TEXT NOT NULL CHECK(length(created_at) BETWEEN 1 AND 64),
  updated_at TEXT NOT NULL CHECK(length(updated_at) BETWEEN 1 AND 64)
);
CREATE INDEX session_deletion_pending ON session_deletion_ops(state, updated_at, operation_id)
  WHERE state <> 'completed';
"#;

pub const DEFAULT_PAGE_SIZE: usize = 50;
pub const MAX_PAGE_SIZE: usize = 200;
pub(crate) const MAX_SCAN_ISSUES: i64 = 256;
const MAX_PROFILE_SNAPSHOT_BYTES: usize = 65_536;

pub(crate) mod legacy_scan;
pub use legacy_scan::LegacyScanResult;
mod deletion;
pub use deletion::{RunDeletionCheck, deletion_check};

/// 目录行的可执行状态；只反映恢复与配置能力，不是游戏事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    Ready,
    NeedsProfileBinding,
    BillingNotConfigured,
    BillingReconciliationRequired,
    RecoveryRequired,
    NeedsRepair,
    ScriptUnavailable,
    SettlementPending,
    Deleting,
}

impl RunState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeedsProfileBinding => "needs-profile-binding",
            Self::BillingNotConfigured => "billing-not-configured",
            Self::BillingReconciliationRequired => "billing-reconciliation-required",
            Self::RecoveryRequired => "recovery-required",
            Self::NeedsRepair => "needs-repair",
            Self::ScriptUnavailable => "script-unavailable",
            Self::SettlementPending => "settlement-pending",
            Self::Deleting => "deleting",
        }
    }

    fn parse(value: &str) -> Result<Self, Fault> {
        match value {
            "ready" => Ok(Self::Ready),
            "needs-profile-binding" => Ok(Self::NeedsProfileBinding),
            "billing-not-configured" => Ok(Self::BillingNotConfigured),
            "billing-reconciliation-required" => Ok(Self::BillingReconciliationRequired),
            "recovery-required" => Ok(Self::RecoveryRequired),
            "needs-repair" => Ok(Self::NeedsRepair),
            "script-unavailable" => Ok(Self::ScriptUnavailable),
            "settlement-pending" => Ok(Self::SettlementPending),
            "deleting" => Ok(Self::Deleting),
            _ => Err(corrupt()),
        }
    }
}

/// 目录查询游标锚定目录 revision，目录发生变化后必须重新从第一页读取。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCursor {
    pub catalog_revision: u64,
    pub after_created_at: String,
    pub after_session_id: String,
}

/// 不包含 transcript、凭据或绝对路径的周目摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub session_id: String,
    pub script_id: String,
    pub created_at: String,
    pub memory_run_id: Option<String>,
    pub billing_run_id: Option<String>,
    pub profile_id: Option<String>,
    pub revision: u64,
    pub state: RunState,
    pub active: bool,
}

/// 有界目录页及旧数据发现进度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPage {
    pub entries: Vec<RunSummary>,
    pub next_cursor: Option<RunCursor>,
    pub catalog_revision: u64,
    pub scan_incomplete: bool,
    pub scan_issue_count: u64,
}

/// 迁移历史账务后的目录 revision 与账本合并摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunBillingReconciliation {
    pub revision: u64,
    pub merge: crate::billing::LegacyScopeMerge,
}

/// 从 SQLite 权威选择行读取的活动周目元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSelection {
    pub session_id: String,
    pub profile_id: String,
    pub script_id: String,
}

/// 经目录身份与 revision 核验、可由玩家显式恢复的周目目标。
#[derive(Debug, Clone, PartialEq)]
pub struct OpenTarget {
    pub session_id: String,
    pub script_id: String,
    pub created_at: String,
    pub memory_run_id: String,
    pub profile: LlmProfile,
    pub revision: u64,
}

struct OpenTargetRow {
    script_id: String,
    created_at: String,
    memory_run_id: Option<String>,
    billing_run_id: Option<String>,
    profile_id: Option<String>,
    schema_version: Option<i64>,
    snapshot: Option<Vec<u8>>,
    state: String,
    revision: i64,
}

/// 读取一个周目的冻结配置，仅允许显式恢复可运行状态的目标。
///
/// # Errors
/// 目录版本过期、身份映射不一致、配置缺失或目标仍需修复时拒绝打开。
pub fn open_target(
    connection: &rusqlite::Connection,
    session_id: &str,
    expected_revision: u64,
) -> Result<OpenTarget, Fault> {
    if !crate::record::format::valid_uuid(session_id) || expected_revision == 0 {
        return Err(Fault::bad_request());
    }
    let row: Option<OpenTargetRow> = connection
        .query_row(
            "SELECT script_id,created_at,memory_run_id,billing_run_id,profile_id,profile_schema_version,profile_snapshot,state,revision FROM session_catalog WHERE session_id=?1",
            [session_id],
            |row| {
                Ok(OpenTargetRow {
                    script_id: row.get(0)?,
                    created_at: row.get(1)?,
                    memory_run_id: row.get(2)?,
                    billing_run_id: row.get(3)?,
                    profile_id: row.get(4)?,
                    schema_version: row.get(5)?,
                    snapshot: row.get(6)?,
                    state: row.get(7)?,
                    revision: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(database)?;
    let Some(row) = row else {
        return Err(Fault::new("app.not-found", "周目不存在"));
    };
    let OpenTargetRow {
        script_id,
        created_at,
        memory_run_id,
        billing_run_id,
        profile_id,
        schema_version,
        snapshot,
        state,
        revision,
    } = row;
    let revision = u64::try_from(revision).map_err(|_| corrupt())?;
    if revision != expected_revision {
        return Err(stale_revision());
    }
    if !matches!(
        RunState::parse(&state)?,
        RunState::Ready
            | RunState::BillingNotConfigured
            | RunState::BillingReconciliationRequired
            | RunState::RecoveryRequired
            | RunState::SettlementPending
    ) {
        return Err(Fault::new("app.conflict", "该周目尚不能恢复"));
    }
    let memory_run_id = memory_run_id.ok_or_else(corrupt)?;
    if billing_run_id.as_deref() != Some(session_id)
        || !crate::record::format::valid_uuid(&memory_run_id)
        || !aoidos_script::valid_id(&script_id)
        || created_at.is_empty()
        || created_at.len() > 64
    {
        return Err(corrupt());
    }
    let mapped_identity: Option<(String, String)> = connection
        .query_row(
            "SELECT sessions.run_id,runs.script_id FROM memory_run_sessions sessions JOIN memory_runs runs ON runs.run_id=sessions.run_id WHERE sessions.session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?,row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    if !mapped_identity
        .as_ref()
        .is_some_and(|(run_id, mapped_script)| {
            run_id == &memory_run_id && mapped_script == &script_id
        })
    {
        return Err(Fault::new("app.conflict", "周目的身份关联需要修复"));
    }
    if schema_version != Some(1) {
        return Err(Fault::new("app.conflict", "周目的模型配置版本不受支持"));
    }
    let snapshot = snapshot.ok_or_else(|| Fault::new("app.conflict", "周目尚未绑定模型配置"))?;
    if snapshot.len() > MAX_PROFILE_SNAPSHOT_BYTES {
        return Err(corrupt());
    }
    let profile: LlmProfile =
        aoidos_json::decode_bytes(&snapshot, MAX_PROFILE_SNAPSHOT_BYTES).map_err(|_| corrupt())?;
    if profile.validate().is_err()
        || aoidos_llm::config::validate_identifier("profileId", &profile.profile_id).is_err()
        || profile_id.as_deref() != Some(profile.profile_id.as_str())
    {
        return Err(corrupt());
    }
    Ok(OpenTarget {
        session_id: session_id.to_owned(),
        script_id,
        created_at,
        memory_run_id,
        profile,
        revision,
    })
}

/// 在同一 SQLite 连接锁区内读取当前目录 revision 并解析恢复目标。
pub fn current_open_target(
    connection: &rusqlite::Connection,
    session_id: &str,
) -> Result<OpenTarget, Fault> {
    let revision: i64 = connection
        .query_row(
            "SELECT revision FROM session_catalog WHERE session_id=?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(|| Fault::new("app.not-found", "周目不存在"))?;
    let revision = u64::try_from(revision).map_err(|_| corrupt())?;
    open_target(connection, session_id, revision)
}

/// 在目标 owner 成功登记后，按原目录 revision 提交 SQLite 活动选择。
///
/// # Errors
/// 目录发生变化或目标已进入不可打开状态时不切换当前选择。
pub fn activate(
    connection: &mut rusqlite::Connection,
    target: &OpenTarget,
    updated_at: &str,
) -> Result<(), Fault> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let current = open_target(&transaction, &target.session_id, target.revision)?;
    if current != *target {
        return Err(stale_revision());
    }
    transaction
        .execute(
            "INSERT INTO active_session(id,session_id,revision,updated_at) VALUES(1,?1,1,?2) ON CONFLICT(id) DO UPDATE SET session_id=excluded.session_id,revision=active_session.revision+1,updated_at=excluded.updated_at",
            rusqlite::params![target.session_id, updated_at],
        )
        .map_err(database)?;
    transaction
        .commit()
        .map_err(|_| Fault::new("store.commit-unknown", "活动周目提交结果不确定"))
}

/// 创建周目时冻结的公开配置与三种独立身份。
#[derive(Debug, Clone)]
pub struct NewRun {
    pub session_id: String,
    pub script_id: String,
    pub created_at: String,
    pub memory_run_id: String,
    pub profile: LlmProfile,
}

/// 将新建周目登记为 canonical 账本身份与不可变公开 profile 快照。
///
/// # Errors
/// 任一身份、profile 或 SQLite 状态不合法时拒绝登记。
pub fn register_new(connection: &mut rusqlite::Connection, run: &NewRun) -> Result<(), Fault> {
    if !crate::record::format::valid_uuid(&run.session_id)
        || !crate::record::format::valid_uuid(&run.memory_run_id)
        || !aoidos_script::valid_id(&run.script_id)
        || run.created_at.is_empty()
        || run.created_at.len() > 64
    {
        return Err(Fault::bad_request());
    }
    run.profile.validate().map_err(|_| Fault::bad_request())?;
    let snapshot = aoidos_json::to_vec(&run.profile).map_err(|_| Fault::bad_request())?;
    if snapshot.len() > MAX_PROFILE_SNAPSHOT_BYTES {
        return Err(Fault::bad_request());
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let has_legacy_ledger_data = has_ledger_data_for_scope(&transaction, &run.memory_run_id)?;
    let state = if has_legacy_ledger_data {
        RunState::BillingReconciliationRequired
    } else if billing_configured(&transaction, &run.session_id)? {
        RunState::Ready
    } else {
        RunState::BillingNotConfigured
    };
    if let Some(existing) = transaction
        .query_row(
            "SELECT script_id, created_at, memory_run_id, billing_run_id, profile_id, profile_snapshot FROM session_catalog WHERE session_id=?1",
            [&run.session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, Option<String>>(4)?, row.get::<_, Option<Vec<u8>>>(5)?)),
        )
        .optional()
        .map_err(database)?
    {
        if existing.0 == run.script_id
            && existing.1 == run.created_at
            && existing.2.as_deref() == Some(&run.memory_run_id)
            && existing.3.as_deref() == Some(&run.session_id)
            && existing.4.as_deref() == Some(&run.profile.profile_id)
            && existing.5.as_deref() == Some(snapshot.as_slice())
        {
            return transaction.commit().map_err(database);
        }
        return Err(corrupt());
    }
    bump_revision(&transaction)?;
    transaction
        .execute(
            "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,profile_id,profile_schema_version,profile_snapshot,state,revision,updated_at) VALUES(?1,?2,?3,?4,?1,?5,1,?6,?7,1,?3)",
            rusqlite::params![run.session_id, run.script_id, run.created_at, run.memory_run_id, run.profile.profile_id, snapshot, state.as_str()],
        )
        .map_err(database)?;
    transaction
        .execute(
            "INSERT INTO active_session(id,session_id,revision,updated_at) VALUES(1,?1,1,?2) ON CONFLICT(id) DO UPDATE SET session_id=excluded.session_id,revision=active_session.revision+1,updated_at=excluded.updated_at",
            rusqlite::params![run.session_id, run.created_at],
        )
        .map_err(database)?;
    transaction.commit().map_err(database)
}

/// 按固定排序返回一页周目摘要；不会读取或恢复 transcript。
///
/// # Errors
/// 非法页大小、旧 revision 游标或目录损坏时失败关闭。
pub fn page(
    connection: &rusqlite::Connection,
    cursor: Option<&RunCursor>,
    page_size: usize,
) -> Result<RunPage, Fault> {
    if !(1..=MAX_PAGE_SIZE).contains(&page_size) {
        return Err(Fault::bad_request());
    }
    let (revision, incomplete): (i64, i64) = connection
        .query_row(
            "SELECT revision, scan_incomplete FROM session_catalog_meta WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database)?;
    let scan_issue_count = connection
        .query_row(
            "SELECT min(count(*), ?1) FROM session_scan_issues",
            [MAX_SCAN_ISSUES],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database)?;
    let revision = u64::try_from(revision).map_err(|_| corrupt())?;
    if cursor.is_some_and(|cursor| cursor.catalog_revision != revision) {
        return Err(stale_revision());
    }
    if let Some(cursor) = cursor
        && (!crate::record::format::valid_uuid(&cursor.after_session_id)
            || cursor.after_created_at.is_empty()
            || cursor.after_created_at.len() > 64)
    {
        return Err(Fault::bad_request());
    }
    let active_id = connection
        .query_row(
            "SELECT session_id FROM active_session WHERE id=1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(database)?
        .flatten();
    let fetch_limit = i64::try_from(page_size + 1).map_err(|_| Fault::bad_request())?;
    let mut statement = connection
        .prepare(
            "SELECT session_id, script_id, created_at, memory_run_id, billing_run_id, profile_id, state, revision FROM session_catalog WHERE (?1 IS NULL OR created_at < ?1 OR (created_at = ?1 AND session_id < ?2)) ORDER BY created_at DESC, session_id DESC LIMIT ?3",
        )
        .map_err(database)?;
    let mut rows = statement
        .query(rusqlite::params![
            cursor.map(|value| value.after_created_at.as_str()),
            cursor.map(|value| value.after_session_id.as_str()),
            fetch_limit
        ])
        .map_err(database)?;
    let mut entries = Vec::with_capacity(page_size);
    while let Some(row) = rows.next().map_err(database)? {
        if entries.len() == page_size {
            break;
        }
        let session_id: String = row.get(0).map_err(database)?;
        let state: String = row.get(6).map_err(database)?;
        let row_revision =
            u64::try_from(row.get::<_, i64>(7).map_err(database)?).map_err(|_| corrupt())?;
        entries.push(RunSummary {
            active: active_id.as_deref() == Some(&session_id),
            session_id,
            script_id: row.get(1).map_err(database)?,
            created_at: row.get(2).map_err(database)?,
            memory_run_id: row.get(3).map_err(database)?,
            billing_run_id: row.get(4).map_err(database)?,
            profile_id: row.get(5).map_err(database)?,
            revision: row_revision,
            state: RunState::parse(&state)?,
        });
    }
    let next_cursor = if entries.len() == page_size {
        let last = entries.last().expect("full page has a last item");
        let has_more: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog WHERE created_at < ?1 OR (created_at = ?1 AND session_id < ?2))",
                [&last.created_at, &last.session_id],
                |row| row.get(0),
            )
            .map_err(database)?;
        has_more.then(|| RunCursor {
            catalog_revision: revision,
            after_created_at: last.created_at.clone(),
            after_session_id: last.session_id.clone(),
        })
    } else {
        None
    };
    Ok(RunPage {
        entries,
        next_cursor,
        catalog_revision: revision,
        scan_incomplete: incomplete != 0,
        scan_issue_count: u64::try_from(scan_issue_count).map_err(|_| corrupt())?,
    })
}

/// 读取 SQLite 中的活动周目；兼容文件只作为首次迁移前的回退来源。
pub fn active_selection(
    connection: &rusqlite::Connection,
) -> Result<Option<ActiveSelection>, Fault> {
    let session_id: Option<String> = connection
        .query_row(
            "SELECT session_id FROM active_session WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?
        .flatten();
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    let selection = connection
        .query_row(
            "SELECT profile_id, script_id FROM session_catalog WHERE session_id=?1",
            [&session_id],
            |row| {
                Ok(ActiveSelection {
                    session_id: session_id.clone(),
                    profile_id: row.get(0)?,
                    script_id: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    if !crate::record::format::valid_uuid(&selection.session_id)
        || !aoidos_script::valid_id(&selection.script_id)
        || aoidos_llm::config::validate_identifier("profileId", &selection.profile_id).is_err()
    {
        return Err(corrupt());
    }
    Ok(Some(selection))
}

/// 比较已冻结的公开 profile；未登记或仍待绑定的旧局返回 `None`。
pub fn profile_matches(
    connection: &rusqlite::Connection,
    session_id: &str,
    profile: &LlmProfile,
) -> Result<Option<bool>, Fault> {
    if !crate::record::format::valid_uuid(session_id) {
        return Err(Fault::bad_request());
    }
    let Some(snapshot) = connection
        .query_row(
            "SELECT profile_snapshot FROM session_catalog WHERE session_id=?1",
            [session_id],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map_err(database)?
        .flatten()
    else {
        return Ok(None);
    };
    let candidate = aoidos_json::to_vec(profile).map_err(|_| Fault::bad_request())?;
    Ok(Some(snapshot == candidate))
}

/// 将玩家明确选择的公开 profile 绑定到尚无快照的旧周目。
///
/// 如果该周目已经存在任一账本数据，绑定后先进入 reconciliation 状态，
/// 防止旧 scope 尚未核对时把周目误报为可继续收费。
///
/// # Errors
/// 目录项缺失、状态不符、revision 过期或公开 profile 无效时拒绝写入。
pub fn bind_profile(
    connection: &mut rusqlite::Connection,
    session_id: &str,
    expected_revision: u64,
    profile: &LlmProfile,
) -> Result<u64, Fault> {
    if !crate::record::format::valid_uuid(session_id) || expected_revision == 0 {
        return Err(Fault::bad_request());
    }
    profile.validate().map_err(|_| Fault::bad_request())?;
    let snapshot = aoidos_json::to_vec(profile).map_err(|_| Fault::bad_request())?;
    if snapshot.len() > MAX_PROFILE_SNAPSHOT_BYTES {
        return Err(Fault::bad_request());
    }

    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let (memory_run_id, billing_run_id, script_id, state, revision): (
        Option<String>,
        Option<String>,
        String,
        String,
        i64,
    ) = transaction
        .query_row(
            "SELECT memory_run_id, billing_run_id, script_id, state, revision FROM session_catalog WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(|| Fault::new("app.not-found", "周目不存在"))?;
    let revision = u64::try_from(revision).map_err(|_| corrupt())?;
    if revision != expected_revision {
        return Err(stale_revision());
    }
    if state != RunState::NeedsProfileBinding.as_str() {
        return Err(Fault::new("app.conflict", "该周目不需要绑定模型配置"));
    }

    let mapped_identity: Option<(String, String)> = transaction
        .query_row(
            "SELECT sessions.run_id,runs.script_id FROM memory_run_sessions sessions JOIN memory_runs runs ON runs.run_id=sessions.run_id WHERE sessions.session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    let identity_matches = memory_run_id
        .as_deref()
        .is_some_and(crate::record::format::valid_uuid)
        && billing_run_id.as_deref() == Some(session_id)
        && mapped_identity
            .as_ref()
            .is_some_and(|(mapped_run, mapped_script)| {
                Some(mapped_run.as_str()) == memory_run_id.as_deref() && mapped_script == &script_id
            });
    if !identity_matches {
        let next_revision = revision.checked_add(1).ok_or_else(corrupt)?;
        transaction
            .execute(
                "UPDATE session_catalog SET state='needs-repair',revision=?1,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE session_id=?2 AND revision=?3 AND state='needs-profile-binding'",
                rusqlite::params![next_revision, session_id, revision],
            )
            .map_err(database)?;
        bump_revision(&transaction)?;
        transaction.commit().map_err(database)?;
        return Err(Fault::new(
            "app.conflict",
            "周目的身份关联已变化，需要修复后才能绑定模型配置",
        ));
    }

    let has_legacy_ledger_data = memory_run_id
        .as_deref()
        .filter(|run_id| *run_id != session_id)
        .map(|run_id| has_ledger_data_for_scope(&transaction, run_id))
        .transpose()?
        .unwrap_or(false);
    let next_state = if has_legacy_ledger_data {
        RunState::BillingReconciliationRequired
    } else if billing_configured(&transaction, session_id)? {
        RunState::Ready
    } else {
        RunState::BillingNotConfigured
    };
    let next_revision = revision.checked_add(1).ok_or_else(corrupt)?;
    transaction
        .execute(
            "UPDATE session_catalog SET profile_id=?1,profile_schema_version=1,profile_snapshot=?2,state=?3,revision=?4,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE session_id=?5 AND revision=?6 AND state='needs-profile-binding'",
            rusqlite::params![profile.profile_id, snapshot, next_state.as_str(), next_revision, session_id, revision],
        )
        .map_err(database)?;
    bump_revision(&transaction)?;
    transaction.commit().map_err(database)?;
    Ok(next_revision)
}

/// 在一个 SQLite 事务内核验身份、合并旧账务 scope 并解除目录计费门禁。
///
/// `selected_limits` 仅用于玩家明确解决 canonical 与 legacy 预算冲突；没有冲突时
/// 可省略。账本和目录状态同事务提交；普通失败会回滚，身份映射不匹配时会提交
/// `needs-repair` 诊断状态后返回错误，避免继续使用已失真的关联。
///
/// # Errors
/// 目录 revision、三身份映射或账本预算冲突不满足迁移条件时拒绝变更。
pub fn reconcile_legacy_billing(
    connection: &mut rusqlite::Connection,
    session_id: &str,
    expected_revision: u64,
    selected_limits: Option<(NanoMoney, NanoMoney)>,
    updated_at: &str,
) -> Result<RunBillingReconciliation, Fault> {
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
    let (memory_run_id, billing_run_id, script_id, state, revision): (
        Option<String>,
        Option<String>,
        String,
        String,
        i64,
    ) = transaction
        .query_row(
            "SELECT memory_run_id,billing_run_id,script_id,state,revision FROM session_catalog WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(|| Fault::new("app.not-found", "周目不存在"))?;
    let revision = u64::try_from(revision).map_err(|_| corrupt())?;
    if revision != expected_revision {
        return Err(stale_revision());
    }
    if state != RunState::BillingReconciliationRequired.as_str() {
        return Err(Fault::new("app.conflict", "该周目不需要迁移历史费用"));
    }

    let mapped_identity: Option<(String, String)> = transaction
        .query_row(
            "SELECT sessions.run_id,runs.script_id FROM memory_run_sessions sessions JOIN memory_runs runs ON runs.run_id=sessions.run_id WHERE sessions.session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    let identity_matches = memory_run_id
        .as_deref()
        .is_some_and(crate::record::format::valid_uuid)
        && billing_run_id.as_deref() == Some(session_id)
        && mapped_identity
            .as_ref()
            .is_some_and(|(mapped_run, mapped_script)| {
                Some(mapped_run.as_str()) == memory_run_id.as_deref() && mapped_script == &script_id
            });
    if !identity_matches {
        let next_revision = revision.checked_add(1).ok_or_else(corrupt)?;
        transaction
            .execute(
                "UPDATE session_catalog SET state='needs-repair',revision=?1,updated_at=?2 WHERE session_id=?3 AND revision=?4 AND state='billing-reconciliation-required'",
                rusqlite::params![next_revision, updated_at, session_id, revision],
            )
            .map_err(database)?;
        bump_revision(&transaction)?;
        transaction.commit().map_err(database)?;
        return Err(Fault::new(
            "app.conflict",
            "周目的身份关联已变化，需要修复后才能迁移费用",
        ));
    }
    let legacy_run_id = memory_run_id.as_deref().ok_or_else(corrupt)?;
    if legacy_run_id == session_id {
        return Err(corrupt());
    }

    let merge = crate::billing::BillingLedger::reconcile_legacy_scope_in_transaction(
        &transaction,
        session_id,
        legacy_run_id,
        selected_limits,
        updated_at,
    )
    .map_err(|error| match error {
        crate::billing::BillingError::Invalid => Fault::bad_request(),
        crate::billing::BillingError::ReconciliationRequired => Fault::new(
            "budget.reconciliation-required",
            "历史费用上限不同，请明确选择本局最终上限",
        ),
        _ => Fault::new("store.database", "历史费用迁移失败"),
    })?;
    let next_state = if billing_configured(&transaction, session_id)? {
        RunState::Ready
    } else {
        RunState::BillingNotConfigured
    };
    let next_revision = revision.checked_add(1).ok_or_else(corrupt)?;
    let changed = transaction
        .execute(
            "UPDATE session_catalog SET state=?1,revision=?2,updated_at=?3 WHERE session_id=?4 AND revision=?5 AND state='billing-reconciliation-required'",
            rusqlite::params![next_state.as_str(), next_revision, updated_at, session_id, revision],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(stale_revision());
    }
    bump_revision(&transaction)?;
    transaction.commit().map_err(database)?;
    Ok(RunBillingReconciliation {
        revision: next_revision,
        merge,
    })
}

fn has_ledger_data_for_scope(
    connection: &rusqlite::Connection,
    run_id: &str,
) -> Result<bool, Fault> {
    crate::billing::BillingLedger::has_run_scope_data(connection, run_id)
        .map_err(|_| Fault::new("store.database", "费用账本读取失败"))
}

fn stale_revision() -> Fault {
    Fault::new("app.conflict", "周目目录已变化，请重新加载")
        .with_detail(serde_json::json!({ "reason": "staleRevision" }))
}

fn billing_configured(connection: &rusqlite::Connection, run_id: &str) -> Result<bool, Fault> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM billing_run_budgets WHERE run_id=?1)",
            [run_id],
            |row| row.get(0),
        )
        .map_err(database)
}

fn bump_revision(connection: &rusqlite::Connection) -> Result<i64, Fault> {
    connection
        .execute(
            "UPDATE session_catalog_meta SET revision=revision+1 WHERE id=1",
            [],
        )
        .map_err(database)?;
    connection
        .query_row(
            "SELECT revision FROM session_catalog_meta WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(database)
}

fn database(_: rusqlite::Error) -> Fault {
    Fault::new("store.database", "周目目录存储失败")
}

fn corrupt() -> Fault {
    Fault::new("store.corrupt", "周目目录数据损坏")
}

/// 对照迁移定义逐项检查表与索引，拒绝缺失或被削弱的目录 schema。
pub fn verify(connection: &rusqlite::Connection) -> Result<(), StoreError> {
    for statement in SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|sql| sql.starts_with("CREATE "))
    {
        let Some(name) = schema_object_name(statement) else {
            return Err(schema_mismatch());
        };
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(aoidos_store::db::sqlite_error)?;
        if actual.as_deref().map(normalize_sql) != Some(normalize_sql(statement)) {
            return Err(schema_mismatch());
        }
    }
    Ok(())
}

fn schema_object_name(statement: &str) -> Option<&str> {
    let mut words = statement.split_whitespace();
    match (words.next()?, words.next()?) {
        ("CREATE", "TABLE") => words.next(),
        ("CREATE", "INDEX") => words.next(),
        _ => None,
    }
}

fn normalize_sql(statement: &str) -> String {
    statement.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn schema_mismatch() -> StoreError {
    StoreError::Corrupt("session catalog schema mismatch".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> LlmProfile {
        LlmProfile {
            profile_id: "test-profile".into(),
            provider_id: "deepseek".into(),
            model: "deepseek-v4.1-flash".into(),
            mode: aoidos_llm::config::ProfileMode::Completion,
            thinking: false,
            sampling: aoidos_llm::sampling::Sampling {
                temperature: 1.0,
                max_tokens: 1024,
            },
            proxy: aoidos_llm::config::ProxyConfig::System,
        }
    }

    fn connection() -> rusqlite::Connection {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
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
        connection.execute_batch(SCHEMA).unwrap();
        connection.execute_batch(legacy_scan::MIGRATION).unwrap();
        connection
    }

    fn insert_memory_mapping(
        connection: &rusqlite::Connection,
        session_id: &str,
        memory_run_id: &str,
        script_id: &str,
    ) {
        connection
            .execute(
                "INSERT INTO memory_runs(run_id,script_id,policy_json,policy_hash) VALUES(?1,?2,'{}',zeroblob(32))",
                rusqlite::params![memory_run_id, script_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO memory_run_sessions(run_id,session_id) VALUES(?1,?2)",
                rusqlite::params![memory_run_id, session_id],
            )
            .unwrap();
    }

    fn register_test_run(connection: &mut rusqlite::Connection) -> NewRun {
        let run = NewRun {
            session_id: "00000000-0000-4000-8000-000000000011".into(),
            script_id: "mistbell".into(),
            created_at: "2026-10-21T00:00:00Z".into(),
            memory_run_id: "00000000-0000-4000-8000-000000000012".into(),
            profile: profile(),
        };
        insert_memory_mapping(
            connection,
            &run.session_id,
            &run.memory_run_id,
            &run.script_id,
        );
        register_new(connection, &run).unwrap();
        run
    }

    #[test]
    fn schema_creates_catalog_selection_and_deletion_journal() {
        let connection = connection();

        verify(&connection).unwrap();
        let revision: i64 = connection
            .query_row(
                "SELECT revision FROM session_catalog_meta WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revision, 1);
    }

    #[test]
    fn explicit_open_requires_the_current_revision_and_verified_identity_mapping() {
        let mut connection = connection();
        let run = register_test_run(&mut connection);
        let target = open_target(&connection, &run.session_id, 1).unwrap();

        assert_eq!(target.session_id, run.session_id);
        assert_eq!(target.script_id, run.script_id);
        assert_eq!(target.memory_run_id, run.memory_run_id);
        assert_eq!(target.profile, run.profile);
        assert_eq!(
            open_target(&connection, &run.session_id, 2)
                .unwrap_err()
                .code,
            "app.conflict"
        );

        connection
            .execute(
                "DELETE FROM memory_run_sessions WHERE session_id=?1",
                [&run.session_id],
            )
            .unwrap();
        assert_eq!(
            open_target(&connection, &run.session_id, 1)
                .unwrap_err()
                .code,
            "app.conflict"
        );
    }

    #[test]
    fn deletion_check_counts_canonical_and_verified_legacy_obligations() {
        let mut connection = connection();
        let run = register_test_run(&mut connection);
        connection
            .execute(
                "INSERT INTO billing_price_versions(version_id,provider_id,model_id,route_policy,currency,unit_tokens,input_uncached_price,input_cached_price,output_price,usage_mapping_version,source_kind,checked_at,effective_from,created_at) VALUES('price','deepseek','deepseek-v4.1-flash','default','USD',1000000,'1','1','1','v1','manual','2026-10-21','2026-10-21','2026-10-21')",
                [],
            )
            .unwrap();
        for scope in [&run.session_id, &run.memory_run_id] {
            connection
                .execute(
                    "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,0,0,1,'2026-10-21')",
                    [scope],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO billing_physical_requests(request_id,run_id,turn_id,profile_id,attempt,provider_id,model_id,route_policy,price_version_id,currency,dispatch_at,state,reserved_nanos,updated_at) VALUES('legacy-request',?1,'turn','test-profile',1,'deepseek','deepseek-v4.1-flash','default','price','USD','2026-10-21','unconfirmed',1,'2026-10-21')",
                [&run.memory_run_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO billing_physical_requests(request_id,run_id,turn_id,profile_id,attempt,provider_id,model_id,route_policy,price_version_id,currency,dispatch_at,state,reserved_nanos,updated_at) VALUES('canonical-request',?1,'turn','test-profile',1,'deepseek','deepseek-v4.1-flash','default','price','USD','2026-10-21','settled',1,'2026-10-21')",
                [&run.session_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO billing_pending_settlements(request_id,input_total,output_total,result_digest,created_at,updated_at) VALUES('canonical-request',1,1,?1,'2026-10-21','2026-10-21')",
                [format!("sha256:{}", "0".repeat(64))],
            )
            .unwrap();

        let check = deletion_check(&connection, &run.session_id, 1).unwrap();
        assert!(check.active);
        assert_eq!(check.unresolved_request_count, 1);
        assert_eq!(check.pending_settlement_count, 1);
        assert!(!check.billing_clear());
    }

    #[test]
    fn deletion_check_rejects_stale_revision_and_broken_identity() {
        let mut connection = connection();
        let run = register_test_run(&mut connection);
        assert_eq!(
            deletion_check(&connection, &run.session_id, 2)
                .unwrap_err()
                .code,
            "app.conflict"
        );

        connection
            .execute(
                "UPDATE memory_runs SET script_id='other' WHERE run_id=?1",
                [&run.memory_run_id],
            )
            .unwrap();
        assert_eq!(
            deletion_check(&connection, &run.session_id, 1)
                .unwrap_err()
                .code,
            "app.conflict"
        );

        connection
            .execute(
                "UPDATE memory_runs SET script_id='mistbell' WHERE run_id=?1",
                [&run.memory_run_id],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE session_catalog SET memory_run_id=NULL WHERE session_id=?1",
                [&run.session_id],
            )
            .unwrap();
        assert_eq!(
            deletion_check(&connection, &run.session_id, 1)
                .unwrap_err()
                .code,
            "app.conflict"
        );
        connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        connection
            .execute(
                "DELETE FROM memory_runs WHERE run_id=?1",
                [&run.memory_run_id],
            )
            .unwrap();
        assert_eq!(
            deletion_check(&connection, &run.session_id, 1)
                .unwrap_err()
                .code,
            "app.conflict"
        );
    }

    #[test]
    fn activation_rechecks_the_target_before_changing_authoritative_selection() {
        let mut connection = connection();
        let run = register_test_run(&mut connection);
        let target = open_target(&connection, &run.session_id, 1).unwrap();
        activate(&mut connection, &target, "2026-10-21T00:01:00Z").unwrap();
        assert_eq!(
            active_selection(&connection).unwrap().unwrap().session_id,
            run.session_id
        );

        connection
            .execute(
                "UPDATE session_catalog SET state='deleting',revision=revision+1 WHERE session_id=?1",
                [&run.session_id],
            )
            .unwrap();
        let error = activate(&mut connection, &target, "2026-10-21T00:02:00Z").unwrap_err();
        assert_eq!(error.code, "app.conflict");
        assert_eq!(
            active_selection(&connection).unwrap().unwrap().session_id,
            run.session_id
        );
    }

    #[test]
    fn schema_verification_rejects_missing_or_weakened_objects() {
        let connection = connection();
        connection
            .execute("DROP INDEX session_catalog_order", [])
            .unwrap();
        assert!(matches!(verify(&connection), Err(StoreError::Corrupt(_))));

        let weakened = rusqlite::Connection::open_in_memory().unwrap();
        weakened
            .execute_batch(&SCHEMA.replace("CHECK(revision > 0)", "CHECK(revision >= 0)"))
            .unwrap();
        assert!(matches!(verify(&weakened), Err(StoreError::Corrupt(_))));
    }

    #[test]
    fn catalog_rejects_secret_sized_or_unknown_state_values() {
        let connection = connection();
        let id = "00000000-0000-4000-8000-000000000001";

        assert!(connection.execute(
            "INSERT INTO session_catalog(session_id,script_id,created_at,state,revision,updated_at,profile_snapshot) VALUES(?1,'mistbell','2026-10-21T00:00:00Z','ready',1,'2026-10-21T00:00:00Z',zeroblob(65537))",
            [id],
        ).is_err());
        assert!(connection.execute(
            "INSERT INTO session_catalog(session_id,script_id,created_at,state,revision,updated_at) VALUES(?1,'mistbell','2026-10-21T00:00:00Z','unknown',1,'2026-10-21T00:00:00Z')",
            [id],
        ).is_err());
    }

    #[test]
    fn new_run_uses_session_identity_for_billing_and_freezes_only_public_profile() {
        let mut connection = connection();
        let run = NewRun {
            session_id: "00000000-0000-4000-8000-000000000001".into(),
            script_id: "mistbell".into(),
            created_at: "2026-10-21T00:00:00Z".into(),
            memory_run_id: "00000000-0000-4000-8000-000000000002".into(),
            profile: profile(),
        };

        register_new(&mut connection, &run).unwrap();
        register_new(&mut connection, &run).unwrap();
        let page = page(&connection, None, DEFAULT_PAGE_SIZE).unwrap();

        assert_eq!(page.entries.len(), 1);
        assert_eq!(
            page.entries[0].billing_run_id.as_deref(),
            Some(run.session_id.as_str())
        );
        assert_eq!(
            page.entries[0].memory_run_id.as_deref(),
            Some(run.memory_run_id.as_str())
        );
        assert_eq!(page.entries[0].state, RunState::BillingNotConfigured);
        let persisted_profile: Vec<u8> = connection
            .query_row(
                "SELECT profile_snapshot FROM session_catalog WHERE session_id=?1",
                [&run.session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            String::from_utf8(persisted_profile)
                .unwrap()
                .contains("test-profile")
        );
        assert_eq!(page.catalog_revision, 2);
    }

    #[test]
    fn new_run_with_legacy_memory_scope_requires_billing_reconciliation() {
        let mut connection = connection();
        let run = NewRun {
            session_id: "00000000-0000-4000-8000-000000000011".into(),
            script_id: "mistbell".into(),
            created_at: "2026-10-21T00:00:00Z".into(),
            memory_run_id: "00000000-0000-4000-8000-000000000012".into(),
            profile: profile(),
        };
        connection
            .execute(
                "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,100,0,1,'2026-10-21T00:00:00Z')",
                [&run.memory_run_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,100,0,1,'2026-10-21T00:00:00Z')",
                [&run.session_id],
            )
            .unwrap();

        register_new(&mut connection, &run).unwrap();

        let page = page(&connection, None, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(
            page.entries[0].state,
            RunState::BillingReconciliationRequired
        );
        assert_eq!(
            page.entries[0].billing_run_id.as_deref(),
            Some(run.session_id.as_str())
        );
    }

    #[test]
    fn legacy_billing_reconciliation_is_explicit_atomic_and_updates_catalog_state() {
        use crate::billing::BillingLedger;
        use aoidos_billing::money::NanoMoney;

        let mut connection = connection();
        let run = NewRun {
            session_id: "00000000-0000-4000-8000-000000000021".into(),
            script_id: "mistbell".into(),
            created_at: "2026-10-21T00:00:00Z".into(),
            memory_run_id: "00000000-0000-4000-8000-000000000022".into(),
            profile: profile(),
        };
        insert_memory_mapping(
            &connection,
            &run.session_id,
            &run.memory_run_id,
            &run.script_id,
        );
        connection
            .execute(
                "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,100,0,1,'2026-10-21T00:00:00Z')",
                [&run.memory_run_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,200,0,1,'2026-10-21T00:00:00Z')",
                [&run.session_id],
            )
            .unwrap();
        register_new(&mut connection, &run).unwrap();
        let catalog_revision = page(&connection, None, 1).unwrap().entries[0].revision;

        assert_eq!(
            reconcile_legacy_billing(
                &mut connection,
                &run.session_id,
                catalog_revision,
                None,
                "2026-10-21T00:01:00Z",
            )
            .unwrap_err()
            .code,
            "budget.reconciliation-required"
        );
        assert_eq!(
            page(&connection, None, 1).unwrap().entries[0].state,
            RunState::BillingReconciliationRequired
        );
        assert!(BillingLedger::has_run_scope_data(&connection, &run.memory_run_id).unwrap());

        let result = reconcile_legacy_billing(
            &mut connection,
            &run.session_id,
            catalog_revision,
            Some((
                NanoMoney::parse("3").unwrap(),
                NanoMoney::parse("2").unwrap(),
            )),
            "2026-10-21T00:02:00Z",
        )
        .unwrap();
        assert_eq!(result.revision, catalog_revision + 1);
        assert_eq!(
            result.merge.settings.unwrap().cny_limit,
            NanoMoney::parse("3").unwrap()
        );
        assert!(!BillingLedger::has_run_scope_data(&connection, &run.memory_run_id).unwrap());
        assert_eq!(
            page(&connection, None, 1).unwrap().entries[0].state,
            RunState::Ready
        );
    }

    #[test]
    fn active_selection_rejects_a_dangling_catalog_reference() {
        let connection = connection();
        connection
            .execute(
                "INSERT INTO active_session(id,session_id,revision,updated_at) VALUES(1,'00000000-0000-4000-8000-000000000099',1,'2026-10-21T00:00:00Z')",
                [],
            )
            .unwrap();

        assert_eq!(
            active_selection(&connection).unwrap_err().code,
            "store.corrupt"
        );
    }

    #[test]
    fn profile_snapshot_comparison_detects_changed_public_configuration() {
        let mut connection = connection();
        let run = NewRun {
            session_id: "00000000-0000-4000-8000-000000000041".into(),
            script_id: "mistbell".into(),
            created_at: "2026-10-21T00:00:00Z".into(),
            memory_run_id: "00000000-0000-4000-8000-000000000042".into(),
            profile: profile(),
        };
        register_new(&mut connection, &run).unwrap();
        assert_eq!(
            profile_matches(&connection, &run.session_id, &run.profile).unwrap(),
            Some(true)
        );

        let mut changed = run.profile.clone();
        changed.model.push_str("-changed");
        assert_eq!(
            profile_matches(&connection, &run.session_id, &changed).unwrap(),
            Some(false)
        );
    }

    #[test]
    fn directory_pages_are_keyset_ordered_and_revision_bound() {
        let connection = connection();
        for (id, created_at) in [
            (
                "00000000-0000-4000-8000-000000000001",
                "2026-10-19T00:00:00Z",
            ),
            (
                "00000000-0000-4000-8000-000000000002",
                "2026-10-20T00:00:00Z",
            ),
            (
                "00000000-0000-4000-8000-000000000003",
                "2026-10-20T00:00:00Z",
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO session_catalog(session_id,script_id,created_at,state,revision,updated_at) VALUES(?1,'mistbell',?2,'needs-profile-binding',1,?2)",
                    rusqlite::params![id, created_at],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO active_session(id,session_id,revision,updated_at) VALUES(1,'00000000-0000-4000-8000-000000000003',1,'2026-10-21T00:00:00Z')",
                [],
            )
            .unwrap();

        let first = page(&connection, None, 2).unwrap();
        assert_eq!(
            first.entries[0].session_id,
            "00000000-0000-4000-8000-000000000003"
        );
        assert!(first.entries[0].active);
        assert_eq!(
            first.entries[1].session_id,
            "00000000-0000-4000-8000-000000000002"
        );
        let second = page(&connection, first.next_cursor.as_ref(), 2).unwrap();
        assert_eq!(
            second.entries[0].session_id,
            "00000000-0000-4000-8000-000000000001"
        );
        assert!(second.next_cursor.is_none());

        let stale = RunCursor {
            catalog_revision: 0,
            after_created_at: "2026-10-20T00:00:00Z".into(),
            after_session_id: "00000000-0000-4000-8000-000000000002".into(),
        };
        assert_eq!(
            page(&connection, Some(&stale), 2).unwrap_err().code,
            "app.conflict"
        );
    }

    #[test]
    fn profile_binding_is_explicit_revisioned_and_never_treats_old_ledger_as_ready() {
        let mut connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000011";
        let memory_run_id = "00000000-0000-4000-8000-000000000012";
        connection
            .execute(
                "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,state,revision,updated_at) VALUES(?1,'mistbell','2026-10-20T00:00:00Z',?2,?1,'needs-profile-binding',1,'2026-10-20T00:00:00Z')",
                rusqlite::params![session_id, memory_run_id],
            )
            .unwrap();
        insert_memory_mapping(&connection, session_id, memory_run_id, "mistbell");

        let revision = bind_profile(&mut connection, session_id, 1, &profile()).unwrap();
        assert_eq!(revision, 2);
        let (state, profile_id, snapshot): (String, String, Vec<u8>) = connection
            .query_row(
                "SELECT state,profile_id,profile_snapshot FROM session_catalog WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, RunState::BillingNotConfigured.as_str());
        assert_eq!(profile_id, "test-profile");
        assert!(
            String::from_utf8(snapshot)
                .unwrap()
                .contains("test-profile")
        );
        assert_eq!(page(&connection, None, 10).unwrap().catalog_revision, 2);

        assert_eq!(
            bind_profile(&mut connection, session_id, 1, &profile())
                .unwrap_err()
                .detail
                .as_ref()
                .and_then(|detail| detail.get("reason"))
                .and_then(serde_json::Value::as_str),
            Some("staleRevision")
        );
        assert_eq!(
            bind_profile(&mut connection, session_id, 2, &profile())
                .unwrap_err()
                .code,
            "app.conflict"
        );
    }

    #[test]
    fn profile_binding_with_legacy_scope_stays_blocked_for_billing_reconciliation() {
        let mut connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000021";
        let memory_run_id = "00000000-0000-4000-8000-000000000022";
        connection
            .execute(
                "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,state,revision,updated_at) VALUES(?1,'mistbell','2026-10-20T00:00:00Z',?2,?1,'needs-profile-binding',1,'2026-10-20T00:00:00Z')",
                rusqlite::params![session_id, memory_run_id],
            )
            .unwrap();
        insert_memory_mapping(&connection, session_id, memory_run_id, "mistbell");
        connection
            .execute(
                "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,100,0,1,'2026-10-20T00:00:00Z')",
                [memory_run_id],
            )
            .unwrap();

        bind_profile(&mut connection, session_id, 1, &profile()).unwrap();
        let state: String = connection
            .query_row(
                "SELECT state FROM session_catalog WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, RunState::BillingReconciliationRequired.as_str());
    }

    #[test]
    fn profile_binding_detects_a_legacy_audit_scope_without_a_budget_row() {
        let mut connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000061";
        let memory_run_id = "00000000-0000-4000-8000-000000000062";
        connection
            .execute(
                "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,state,revision,updated_at) VALUES(?1,'mistbell','2026-10-20T00:00:00Z',?2,?1,'needs-profile-binding',1,'2026-10-20T00:00:00Z')",
                rusqlite::params![session_id, memory_run_id],
            )
            .unwrap();
        insert_memory_mapping(&connection, session_id, memory_run_id, "mistbell");
        connection
            .execute(
                "INSERT INTO billing_audit(operation_id,operation_kind,target_id,details_json,created_at) VALUES('00000000-0000-4000-8000-000000000063','run-budget-update',?1,'{}','2026-10-20T00:00:00Z')",
                [memory_run_id],
            )
            .unwrap();

        bind_profile(&mut connection, session_id, 1, &profile()).unwrap();
        let state: String = connection
            .query_row(
                "SELECT state FROM session_catalog WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, RunState::BillingReconciliationRequired.as_str());
    }

    #[test]
    fn profile_binding_keeps_a_canonical_budget_ready() {
        let mut connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000031";
        let memory_run_id = "00000000-0000-4000-8000-000000000032";
        connection
            .execute(
                "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,state,revision,updated_at) VALUES(?1,'mistbell','2026-10-20T00:00:00Z',?2,?1,'needs-profile-binding',1,'2026-10-20T00:00:00Z')",
                rusqlite::params![session_id, memory_run_id],
            )
            .unwrap();
        insert_memory_mapping(&connection, session_id, memory_run_id, "mistbell");
        connection
            .execute(
                "INSERT INTO billing_run_budgets(run_id,cny_limit_nanos,usd_limit_nanos,revision,updated_at) VALUES(?1,100,0,1,'2026-10-20T00:00:00Z')",
                [session_id],
            )
            .unwrap();

        bind_profile(&mut connection, session_id, 1, &profile()).unwrap();
        let state: String = connection
            .query_row(
                "SELECT state FROM session_catalog WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, RunState::Ready.as_str());
    }

    #[test]
    fn profile_binding_quarantines_a_mapping_that_changed_after_scan() {
        let mut connection = connection();
        let session_id = "00000000-0000-4000-8000-000000000051";
        let memory_run_id = "00000000-0000-4000-8000-000000000052";
        connection
            .execute(
                "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,state,revision,updated_at) VALUES(?1,'mistbell','2026-10-20T00:00:00Z',?2,?1,'needs-profile-binding',1,'2026-10-20T00:00:00Z')",
                rusqlite::params![session_id, memory_run_id],
            )
            .unwrap();
        insert_memory_mapping(&connection, session_id, memory_run_id, "mistbell");
        connection
            .execute(
                "DELETE FROM memory_run_sessions WHERE session_id=?1",
                [session_id],
            )
            .unwrap();

        let error = bind_profile(&mut connection, session_id, 1, &profile()).unwrap_err();
        let (state, revision, profile_snapshot): (String, i64, Option<Vec<u8>>) = connection
            .query_row(
                "SELECT state,revision,profile_snapshot FROM session_catalog WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(error.code, "app.conflict");
        assert_eq!(state, RunState::NeedsRepair.as_str());
        assert_eq!(revision, 2);
        assert!(profile_snapshot.is_none());
    }
}
