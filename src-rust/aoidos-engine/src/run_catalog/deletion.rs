//! 周目删除前只读核验；实际删除由后续 journal 执行器负责。

use super::{RunState, bump_revision, corrupt, database, stale_revision};
use crate::fault::Fault;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_TRANSCRIPT_MANIFEST_ENTRIES: usize = 8_192;
const MAX_MANIFEST_ENTRIES: usize = MAX_TRANSCRIPT_MANIFEST_ENTRIES + 1;
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_TRANSCRIPT_DIRECTORY_SCAN_ENTRIES: usize = 65_536;

type CatalogDeletionRow = (String, String, Option<String>, Option<String>, String, i64);
type FreezeManifestRow = (String, String, String, String, String, Option<Vec<u8>>);
type QuarantineManifestRow = (String, String, String, String, String, Option<Vec<u8>>);
type CleanupManifestRow = (
    String,
    String,
    Option<Vec<u8>>,
    Option<i64>,
    Option<Vec<u8>>,
);
type PurgeRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
);

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
    pub diagnostic_code: Option<String>,
}

/// 单个冻结文件系统项；身份哈希由 owner 关闭后的文件枚举器计算。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeletionManifestEntry {
    pub source: String,
    pub quarantine: String,
    pub kind: DeletionManifestEntryKind,
    /// Memory run 目录可在尚未写入正文时不存在；`None` 仅表示冻结时确认其缺席。
    pub identity_hash: Option<String>,
}

/// 枚举目标周目的 transcript、partial、recovery backup 与 memory run 目录，并冻结对象身份摘要。
///
/// 调用方必须先持有应用生命周期锁、关闭目标 owner 并移除活动文件镜像。目录枚举有独立硬上限；
/// 超限或发现目标周目的未知旁车时整体失败，不返回部分清单。路径操作不是原子快照，真正移动前仍须复核身份。
///
/// # Errors
/// 路径身份不完整、目录超限、目标文件缺失或存储不可读时失败关闭。
pub fn enumerate_deletion_manifest_entries(
    root: &Path,
    operation_id: &str,
    script_id: &str,
    session_id: &str,
    memory_run_id: &str,
) -> Result<Vec<DeletionManifestEntry>, Fault> {
    enumerate_deletion_manifest_entries_with_limit(
        root,
        operation_id,
        script_id,
        session_id,
        memory_run_id,
        MAX_TRANSCRIPT_DIRECTORY_SCAN_ENTRIES,
    )
}

fn enumerate_deletion_manifest_entries_with_limit(
    root: &Path,
    operation_id: &str,
    script_id: &str,
    session_id: &str,
    memory_run_id: &str,
    scan_limit: usize,
) -> Result<Vec<DeletionManifestEntry>, Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
        || !crate::record::format::valid_uuid(memory_run_id)
        || !aoidos_script::valid_id(script_id)
    {
        return Err(Fault::bad_request());
    }

    let transcript_directory = format!("workspaces/{script_id}/transcript");
    let transcript_prefix = format!("{session_id}.");
    let transcript_name = format!("{session_id}.jsonl");
    let listing = aoidos_store::filesystem::list_directory_entries(
        root,
        Path::new(&transcript_directory),
        scan_limit,
    )?;
    if !listing.complete {
        return Err(Fault::new("app.capacity", "周目文件目录超过扫描上限"));
    }

    let mut entries = Vec::new();
    let mut transcript_found = false;
    for name in listing.entries {
        let encoded_name = name.as_encoded_bytes();
        let transcript_bytes = transcript_name.as_bytes();
        let prefix_bytes = transcript_prefix.as_bytes();
        let (name, kind, quarantine_directory) = if encoded_name == transcript_bytes {
            transcript_found = true;
            (
                transcript_name.as_str(),
                DeletionManifestEntryKind::Transcript,
                "transcript",
            )
        } else if encoded_name.starts_with(prefix_bytes) {
            let Ok(name) = std::str::from_utf8(encoded_name) else {
                return Err(Fault::new("app.bad-request", "周目文件名无法安全识别"));
            };
            let source = format!("{transcript_directory}/{name}");
            let kind = if valid_transcript_sidecar(
                &source,
                &format!("{transcript_directory}/{transcript_prefix}"),
                false,
            ) {
                DeletionManifestEntryKind::Partial
            } else if valid_transcript_sidecar(
                &source,
                &format!("{transcript_directory}/{transcript_prefix}"),
                true,
            ) {
                DeletionManifestEntryKind::RecoveryBackup
            } else {
                return Err(Fault::new(
                    "app.bad-request",
                    "周目目录包含无法识别的旁车文件",
                ));
            };
            (name, kind, "transcript")
        } else {
            continue;
        };
        let source = format!("{transcript_directory}/{name}");
        let identity_hash = aoidos_store::filesystem::file_identity_hash(root, Path::new(&source))?;
        entries.push(manifest_entry(
            operation_id,
            source,
            quarantine_directory,
            kind,
            Some(identity_hash),
        ));
    }
    if !transcript_found {
        return Err(Fault::new("app.conflict", "周目 transcript 文件不存在"));
    }
    if entries.len() > MAX_TRANSCRIPT_MANIFEST_ENTRIES {
        return Err(Fault::new("app.capacity", "周目文件清单超过容量上限"));
    }

    let memory_source = format!("workspaces/{script_id}/memory/runs/{memory_run_id}");
    let memory_identity =
        match aoidos_store::filesystem::directory_identity_hash(root, Path::new(&memory_source)) {
            Ok(identity) => Some(identity),
            Err(aoidos_store::error::StoreError::Io {
                code: "not-found", ..
            }) => None,
            Err(error) => return Err(error.into()),
        };
    entries.push(manifest_entry(
        operation_id,
        memory_source,
        "memory",
        DeletionManifestEntryKind::MemoryRunDirectory,
        memory_identity,
    ));
    entries.sort_by(|left, right| left.source.cmp(&right.source));
    validate_manifest_entries(&entries, operation_id, script_id, session_id, memory_run_id)?;
    Ok(entries)
}

fn manifest_entry(
    operation_id: &str,
    source: String,
    quarantine_directory: &str,
    kind: DeletionManifestEntryKind,
    identity_hash: Option<String>,
) -> DeletionManifestEntry {
    let filename = source.rsplit('/').next().unwrap_or_default().to_owned();
    DeletionManifestEntry {
        source,
        quarantine: format!("deleting/{operation_id}/{quarantine_directory}/{filename}"),
        kind,
        identity_hash,
    }
}

/// 删除清单只接受设计明确覆盖的本局文件类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeletionManifestEntryKind {
    Transcript,
    Partial,
    RecoveryBackup,
    MemoryRunDirectory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FrozenDeletionManifest {
    pub(super) version: u32,
    pub(super) operation_id: String,
    pub(super) session_id: String,
    pub(super) script_id: String,
    pub(super) memory_run_id: String,
    pub(super) entries: Vec<DeletionManifestEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FrozenDeletionEnvelope {
    pub(super) manifest: FrozenDeletionManifest,
    pub(super) hash: String,
}

/// 在同一 SQLite 事务中登记删除意图并封锁目标周目。
///
/// 该底层原语暂不暴露给应用入口；必须与生命周期互斥、owner 关闭和 journal 前向执行器一起接入。
/// 相同操作重放返回现有状态，不同操作不能并发占用同一周目。
///
/// # Errors
/// 缺少未结账务的显式确认、身份或目录 revision 不匹配时拒绝登记。
pub fn request_deletion(
    connection: &mut rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
    expected_revision: u64,
    updated_at: &str,
) -> Result<DeletionOperation, Fault> {
    request_deletion_with_ack(
        connection,
        operation_id,
        session_id,
        expected_revision,
        updated_at,
        false,
    )
}

/// 登记删除意图；只有显式确认时，才允许连同未结算的本局账务一起删除。
pub fn request_deletion_with_ack(
    connection: &mut rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
    expected_revision: u64,
    updated_at: &str,
    acknowledge_run_data_loss: bool,
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
            diagnostic_code: None,
        });
    }
    if let Some(target_hash) = transaction
        .query_row(
            "SELECT target_identity_hash FROM session_deletion_receipts WHERE operation_id=?1",
            [operation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database)?
    {
        if target_hash != target_identity_hash(session_id) {
            return Err(Fault::new("app.conflict", "删除操作编号已用于其他周目"));
        }
        let revision = catalog_revision(&transaction)?;
        transaction.commit().map_err(database)?;
        return Ok(DeletionOperation {
            operation_id: operation_id.to_owned(),
            session_id: session_id.to_owned(),
            state: "completed".into(),
            catalog_revision: revision,
            diagnostic_code: None,
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
    if !check.billing_clear() && !acknowledge_run_data_loss {
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
    transaction
        .execute(
            "INSERT INTO session_deletion_ack(operation_id,acknowledge_run_data_loss) VALUES(?1,?2)",
            rusqlite::params![operation_id, acknowledge_run_data_loss],
        )
        .map_err(database)?;
    transaction
        .execute(
            "INSERT INTO session_deletion_progress(operation_id,manifest_index,updated_at) VALUES(?1,0,?2)",
            rusqlite::params![operation_id, updated_at],
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
        diagnostic_code: None,
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
        diagnostic_code: None,
    })
}

/// 冻结 owner 已关闭后的文件清单，并与 `owner-closed` 阶段原子提交。
///
/// 调用方须在生命周期锁内关闭会话、清除目标活动文件镜像并完成文件身份核验；本函数会再次检查
/// SQLite 活动选择不再指向目标、目录仍处于删除状态，并验证清单只包含目标周目的允许路径。
/// transcript 与 sidecar 合计最多 8,192 项，memory run 目录另计一项；完整清单 envelope 最多 4 MiB。
/// 哈希覆盖规范 manifest 字节。
///
/// # Errors
/// 操作阶段、身份、文件清单或存储状态不一致时拒绝冻结；失败不会推进 journal。
pub(crate) fn freeze_deletion_manifest(
    connection: &mut rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
    entries: Vec<DeletionManifestEntry>,
    updated_at: &str,
) -> Result<DeletionOperation, Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
        || updated_at.is_empty()
        || updated_at.len() > 64
        || entries.len() > MAX_MANIFEST_ENTRIES
    {
        return Err(Fault::bad_request());
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let row: Option<FreezeManifestRow> = transaction
        .query_row(
            "SELECT op.session_id,op.state,c.script_id,c.memory_run_id,c.state,op.manifest FROM session_deletion_ops op JOIN session_catalog c USING(session_id) WHERE op.operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((target_id, state, script_id, memory_run_id, catalog_state, existing_manifest)) = row
    else {
        return Err(Fault::new("app.not-found", "删除操作不存在"));
    };
    if target_id != session_id {
        return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
    }
    if state != "requested" || catalog_state != "deleting" || existing_manifest.is_some() {
        return Err(Fault::new("app.conflict", "删除清单冻结阶段不匹配"));
    }
    let active: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM active_session WHERE id=1 AND session_id=?1)",
            [session_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if active {
        return Err(Fault::new("app.conflict", "活动周目尚未关闭"));
    }
    if !crate::record::format::valid_uuid(&memory_run_id) || !aoidos_script::valid_id(&script_id) {
        return Err(corrupt());
    }
    let mapping_valid: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_run_sessions s JOIN memory_runs r USING(run_id) WHERE s.session_id=?1 AND s.run_id=?2 AND r.script_id=?3)",
            rusqlite::params![session_id, memory_run_id, script_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if !mapping_valid {
        return Err(corrupt());
    }
    validate_manifest_entries(
        &entries,
        operation_id,
        &script_id,
        session_id,
        &memory_run_id,
    )?;
    let manifest = FrozenDeletionManifest {
        version: 1,
        operation_id: operation_id.to_owned(),
        session_id: session_id.to_owned(),
        script_id,
        memory_run_id,
        entries,
    };
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|_| corrupt())?;
    let hash = format!("sha256:{:x}", Sha256::digest(&manifest_bytes));
    let bytes =
        serde_json::to_vec(&FrozenDeletionEnvelope { manifest, hash }).map_err(|_| corrupt())?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(Fault::new("app.capacity", "删除文件清单超过容量上限"));
    }
    let changed = transaction
        .execute(
            "UPDATE session_deletion_ops SET manifest=?1,state='owner-closed',updated_at=?2 WHERE operation_id=?3 AND session_id=?4 AND state='requested' AND manifest IS NULL",
            rusqlite::params![bytes, updated_at, operation_id, session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "删除清单冻结阶段已变化"));
    }
    let revision = catalog_revision(&transaction)?;
    transaction.commit().map_err(database)?;
    Ok(DeletionOperation {
        operation_id: operation_id.to_owned(),
        session_id: session_id.to_owned(),
        state: "owner-closed".into(),
        catalog_revision: revision,
        diagnostic_code: None,
    })
}

/// 读取冻结前可信的剧本和 memory run 身份；文件系统扫描期间不持有 SQLite 锁。
pub(crate) fn deletion_manifest_target(
    connection: &rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
) -> Result<(String, String), Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
    {
        return Err(Fault::bad_request());
    }
    let row: Option<FreezeManifestRow> = connection
        .query_row(
            "SELECT op.session_id,op.state,c.script_id,c.memory_run_id,c.state,op.manifest FROM session_deletion_ops op JOIN session_catalog c USING(session_id) WHERE op.operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((target_id, state, script_id, memory_run_id, catalog_state, existing_manifest)) = row
    else {
        return Err(Fault::new("app.not-found", "删除操作不存在"));
    };
    if target_id != session_id
        || state != "requested"
        || catalog_state != "deleting"
        || existing_manifest.is_some()
    {
        return Err(Fault::new("app.conflict", "删除清单冻结阶段不匹配"));
    }
    if !crate::record::format::valid_uuid(&memory_run_id) || !aoidos_script::valid_id(&script_id) {
        return Err(corrupt());
    }
    let mapping_valid: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_run_sessions s JOIN memory_runs r USING(run_id) WHERE s.session_id=?1 AND s.run_id=?2 AND r.script_id=?3)",
            rusqlite::params![session_id, memory_run_id, script_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if !mapping_valid {
        return Err(corrupt());
    }
    let active: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM active_session WHERE id=1 AND session_id=?1)",
            [session_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if active {
        return Err(Fault::new("app.conflict", "活动周目尚未关闭"));
    }
    Ok((script_id, memory_run_id))
}

/// 读取并校验已冻结的清单；完成隔离的 journal 以 `None` 表示幂等重放。
pub(crate) fn manifest_for_quarantine(
    connection: &rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
) -> Result<Option<(Vec<DeletionManifestEntry>, String)>, Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
    {
        return Err(Fault::bad_request());
    }
    let row: Option<QuarantineManifestRow> = connection
        .query_row(
            "SELECT op.session_id,op.state,c.script_id,c.memory_run_id,c.state,op.manifest FROM session_deletion_ops op JOIN session_catalog c USING(session_id) WHERE op.operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((target_id, state, script_id, memory_run_id, catalog_state, bytes)) = row else {
        return Err(Fault::new("app.not-found", "删除操作不存在"));
    };
    if target_id != session_id {
        return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
    }
    if catalog_state != "deleting" {
        return Err(Fault::new("app.conflict", "周目不处于删除状态"));
    }
    if state == "files-quarantined" {
        return Ok(None);
    }
    if state != "owner-closed" {
        return Err(Fault::new("app.conflict", "删除隔离阶段不匹配"));
    }
    let bytes = bytes.ok_or_else(corrupt)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(corrupt());
    }
    let envelope: FrozenDeletionEnvelope = serde_json::from_slice(&bytes).map_err(|_| corrupt())?;
    let manifest_bytes = serde_json::to_vec(&envelope.manifest).map_err(|_| corrupt())?;
    let expected_hash = format!("sha256:{:x}", Sha256::digest(&manifest_bytes));
    if envelope.hash != expected_hash
        || envelope.manifest.version != 1
        || envelope.manifest.operation_id != operation_id
        || envelope.manifest.session_id != session_id
        || envelope.manifest.script_id != script_id
        || envelope.manifest.memory_run_id != memory_run_id
    {
        return Err(corrupt());
    }
    let mapping_valid: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_run_sessions s JOIN memory_runs r USING(run_id) WHERE s.session_id=?1 AND s.run_id=?2 AND r.script_id=?3)",
            rusqlite::params![session_id, memory_run_id, script_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if !mapping_valid {
        return Err(corrupt());
    }
    validate_manifest_entries(
        &envelope.manifest.entries,
        operation_id,
        &script_id,
        session_id,
        &memory_run_id,
    )?;
    Ok(Some((envelope.manifest.entries, expected_hash)))
}

/// 查询单个删除 journal 的当前阶段。
pub fn operation(
    connection: &rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
) -> Result<DeletionOperation, Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
    {
        return Err(Fault::bad_request());
    }
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT session_id,state FROM session_deletion_ops WHERE operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((target_id, state)) = row else {
        let target_hash: Option<String> = connection
            .query_row(
                "SELECT target_identity_hash FROM session_deletion_receipts WHERE operation_id=?1",
                [operation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database)?;
        let Some(target_hash) = target_hash else {
            return Err(Fault::new("app.not-found", "删除操作不存在"));
        };
        if target_hash != target_identity_hash(session_id) {
            return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
        }
        return Ok(DeletionOperation {
            operation_id: operation_id.to_owned(),
            session_id: session_id.to_owned(),
            state: "completed".into(),
            catalog_revision: catalog_revision(connection)?,
            diagnostic_code: None,
        });
    };
    if target_id != session_id {
        return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
    }
    let revision = catalog_revision(connection)?;
    Ok(DeletionOperation {
        operation_id: operation_id.to_owned(),
        session_id: session_id.to_owned(),
        state,
        catalog_revision: revision,
        diagnostic_code: None,
    })
}

/// 按创建顺序读取尚未完成的删除操作，供启动恢复器有界前向推进。
///
/// 每次最多返回 128 项；调用方完成当前页后重新读取，避免长期保留可变游标。
/// # Errors
/// 页大小无效或 journal / SQLite 数据损坏时返回稳定错误。
pub fn pending_operations(
    connection: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<DeletionOperation>, Fault> {
    if limit == 0 || limit > 128 {
        return Err(Fault::bad_request());
    }
    let revision = catalog_revision(connection)?;
    let mut statement = connection
        .prepare(
            "SELECT operation_id,session_id,state,result_code FROM session_deletion_ops WHERE state<>'completed' ORDER BY created_at,operation_id LIMIT ?1",
        )
        .map_err(database)?;
    let rows = statement
        .query_map(
            [i64::try_from(limit).map_err(|_| Fault::bad_request())?],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .map_err(database)?;
    let mut operations = Vec::new();
    for row in rows {
        let (operation_id, session_id, state, diagnostic_code) = row.map_err(database)?;
        if !crate::record::format::valid_uuid(&operation_id)
            || !crate::record::format::valid_uuid(&session_id)
            || !matches!(
                state.as_str(),
                "requested"
                    | "owner-closed"
                    | "files-quarantined"
                    | "database-purged"
                    | "files-purged"
                    | "needs-repair"
            )
        {
            return Err(corrupt());
        }
        operations.push(DeletionOperation {
            operation_id,
            session_id,
            state,
            catalog_revision: revision,
            diagnostic_code,
        });
    }
    Ok(operations)
}

/// 保留启动恢复失败码；成功推进后可清除旧诊断。
pub(crate) fn set_deletion_diagnostic(
    connection: &rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
    diagnostic_code: Option<&str>,
    updated_at: &str,
) -> Result<(), Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
        || updated_at.is_empty()
        || updated_at.len() > 64
        || diagnostic_code.is_some_and(|code| {
            code.is_empty()
                || code.len() > 64
                || !code.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_".contains(&byte)
                })
        })
    {
        return Err(Fault::bad_request());
    }
    let changed = connection
        .execute(
            "UPDATE session_deletion_ops SET result_code=?1,updated_at=?2 WHERE operation_id=?3 AND session_id=?4 AND state<>'completed'",
            rusqlite::params![diagnostic_code, updated_at, operation_id, session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.not-found", "待恢复删除操作不存在"));
    }
    Ok(())
}

/// 在全部文件已按清单隔离后，原子推进删除阶段。
pub(crate) fn mark_files_quarantined(
    connection: &mut rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
    manifest_hash: &str,
    updated_at: &str,
) -> Result<DeletionOperation, Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
        || !valid_sha256(manifest_hash)
        || updated_at.is_empty()
        || updated_at.len() > 64
    {
        return Err(Fault::bad_request());
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let row: Option<(String, String, String, Option<Vec<u8>>)> = transaction
        .query_row(
            "SELECT op.session_id,op.state,c.state,op.manifest FROM session_deletion_ops op JOIN session_catalog c USING(session_id) WHERE op.operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((target_id, state, catalog_state, bytes)) = row else {
        return Err(Fault::new("app.not-found", "删除操作不存在"));
    };
    if target_id != session_id {
        return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
    }
    if state == "files-quarantined" {
        let revision = catalog_revision(&transaction)?;
        transaction.commit().map_err(database)?;
        return Ok(DeletionOperation {
            operation_id: operation_id.to_owned(),
            session_id: session_id.to_owned(),
            state,
            catalog_revision: revision,
            diagnostic_code: None,
        });
    }
    let bytes = bytes.ok_or_else(corrupt)?;
    let envelope: FrozenDeletionEnvelope = serde_json::from_slice(&bytes).map_err(|_| corrupt())?;
    let manifest_bytes = serde_json::to_vec(&envelope.manifest).map_err(|_| corrupt())?;
    let expected_hash = format!("sha256:{:x}", Sha256::digest(&manifest_bytes));
    if state != "owner-closed"
        || catalog_state != "deleting"
        || envelope.hash != expected_hash
        || envelope.hash != manifest_hash
        || envelope.manifest.operation_id != operation_id
        || envelope.manifest.session_id != session_id
    {
        return Err(Fault::new("app.conflict", "删除清单或阶段已变化"));
    }
    let changed = transaction
        .execute(
            "UPDATE session_deletion_ops SET state='files-quarantined',updated_at=?1 WHERE operation_id=?2 AND session_id=?3 AND state='owner-closed'",
            rusqlite::params![updated_at, operation_id, session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "删除隔离阶段已变化"));
    }
    let revision = catalog_revision(&transaction)?;
    transaction.commit().map_err(database)?;
    Ok(DeletionOperation {
        operation_id: operation_id.to_owned(),
        session_id: session_id.to_owned(),
        state: "files-quarantined".into(),
        catalog_revision: revision,
        diagnostic_code: None,
    })
}

/// 在单个 SQLite 事务中清理已隔离周目的数据库记录，并保留删除墓碑与剧本代数高水位。
///
/// 文件必须已经全部按冻结清单移动到隔离目录；事务再次核对三身份映射，
/// 再按显式外键顺序删除已确认的周目数据。任何一步失败都会回滚数据库更改，journal 保持原阶段。
/// # Errors
/// journal 阶段、manifest 或周目身份不满足删除条件时失败关闭。
pub(crate) fn purge_run_database(
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
    let row: Option<PurgeRow> =
        transaction
            .query_row(
                "SELECT op.session_id,op.state,c.state,c.script_id,c.memory_run_id,c.billing_run_id,op.manifest FROM session_deletion_ops op LEFT JOIN session_catalog c USING(session_id) WHERE op.operation_id=?1",
                [operation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
            )
            .optional()
            .map_err(database)?;
    let Some((
        target_id,
        state,
        catalog_state,
        script_id,
        memory_run_id,
        billing_run_id,
        manifest_bytes,
    )) = row
    else {
        return Err(Fault::new("app.not-found", "删除操作不存在"));
    };
    if target_id != session_id {
        return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
    }
    if matches!(
        state.as_str(),
        "database-purged" | "files-purged" | "completed"
    ) {
        let revision = catalog_revision(&transaction)?;
        transaction.commit().map_err(database)?;
        return Ok(DeletionOperation {
            operation_id: operation_id.to_owned(),
            session_id: session_id.to_owned(),
            state,
            catalog_revision: revision,
            diagnostic_code: None,
        });
    }
    if state != "files-quarantined" || catalog_state.as_deref() != Some("deleting") {
        return Err(Fault::new("app.conflict", "周目数据库清理阶段不匹配"));
    }
    let script_id = script_id.ok_or_else(corrupt)?;
    let memory_run_id = memory_run_id.ok_or_else(corrupt)?;
    if !aoidos_script::valid_id(&script_id)
        || !crate::record::format::valid_uuid(&memory_run_id)
        || !crate::record::format::valid_uuid(session_id)
    {
        return Err(corrupt());
    }
    if billing_run_id.as_deref() != Some(session_id) {
        return Err(Fault::new("app.conflict", "本局账单身份需要修复"));
    }
    let manifest_bytes = manifest_bytes.ok_or_else(corrupt)?;
    validate_frozen_manifest(
        &manifest_bytes,
        operation_id,
        session_id,
        &script_id,
        &memory_run_id,
    )?;

    let active: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM active_session WHERE id=1 AND session_id=?1)",
            [session_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if active {
        return Err(Fault::new("app.conflict", "活动周目尚未关闭"));
    }
    let mapping_valid: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_run_sessions s JOIN memory_runs r USING(run_id) WHERE s.session_id=?1 AND s.run_id=?2 AND r.script_id=?3)",
            rusqlite::params![session_id, memory_run_id, script_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if !mapping_valid {
        return Err(Fault::new("app.conflict", "周目的身份关联需要修复"));
    }
    purge_scoped_data(&transaction, session_id, &memory_run_id, &script_id)?;
    delete_catalog_row(&transaction, session_id)?;
    let changed = transaction
        .execute(
            "UPDATE session_deletion_ops SET state='database-purged',updated_at=?1 WHERE operation_id=?2 AND session_id=?3 AND state='files-quarantined'",
            rusqlite::params![updated_at, operation_id, session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "周目数据库清理阶段已变化"));
    }
    let revision = u64::try_from(bump_revision(&transaction)?).map_err(|_| corrupt())?;
    transaction.commit().map_err(database)?;
    Ok(DeletionOperation {
        operation_id: operation_id.to_owned(),
        session_id: session_id.to_owned(),
        state: "database-purged".into(),
        catalog_revision: revision,
        diagnostic_code: None,
    })
}

/// 读取数据库清理后的冻结隔离清单；该阶段不再依赖已删除的目录行。
pub(crate) struct QuarantineCleanupManifest {
    pub entries: Vec<DeletionManifestEntry>,
    pub index: usize,
    pub directory_stack: Option<Vec<u8>>,
}

pub(crate) fn manifest_for_quarantine_cleanup(
    connection: &rusqlite::Connection,
    operation_id: &str,
    session_id: &str,
) -> Result<QuarantineCleanupManifest, Fault> {
    if !crate::record::format::valid_uuid(operation_id)
        || !crate::record::format::valid_uuid(session_id)
    {
        return Err(Fault::bad_request());
    }
    let row: Option<CleanupManifestRow> = connection
        .query_row(
            "SELECT op.session_id,op.state,op.manifest,p.manifest_index,p.directory_stack FROM session_deletion_ops op LEFT JOIN session_deletion_progress p USING(operation_id) WHERE op.operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((target_id, state, bytes, cleanup_index, directory_stack)) = row else {
        return Err(Fault::new("app.not-found", "删除操作不存在"));
    };
    if target_id != session_id {
        return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
    }
    if state != "database-purged" {
        return Err(Fault::new("app.conflict", "隔离文件清理阶段不匹配"));
    }
    let bytes = bytes.ok_or_else(corrupt)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(corrupt());
    }
    let envelope: FrozenDeletionEnvelope = serde_json::from_slice(&bytes).map_err(|_| corrupt())?;
    validate_frozen_manifest(
        &bytes,
        operation_id,
        session_id,
        &envelope.manifest.script_id,
        &envelope.manifest.memory_run_id,
    )?;
    validate_manifest_entries(
        &envelope.manifest.entries,
        operation_id,
        &envelope.manifest.script_id,
        session_id,
        &envelope.manifest.memory_run_id,
    )?;
    let cleanup_index =
        usize::try_from(cleanup_index.ok_or_else(corrupt)?).map_err(|_| corrupt())?;
    if cleanup_index > envelope.manifest.entries.len() {
        return Err(corrupt());
    }
    if directory_stack
        .as_ref()
        .is_some_and(|cursor| cursor.len() > 262_144)
    {
        return Err(corrupt());
    }
    Ok(QuarantineCleanupManifest {
        entries: envelope.manifest.entries,
        index: cleanup_index,
        directory_stack,
    })
}

pub(crate) struct DeletionCleanupProgress<'a> {
    pub operation_id: &'a str,
    pub session_id: &'a str,
    pub expected_index: usize,
    pub next_index: usize,
    pub expected_stack: Option<&'a [u8]>,
    pub next_stack: Option<&'a [u8]>,
    pub updated_at: &'a str,
}

/// 以 compare-and-set 持久化已清理的 manifest 前缀。
pub(crate) fn advance_deletion_cleanup(
    connection: &mut rusqlite::Connection,
    progress: DeletionCleanupProgress<'_>,
) -> Result<(), Fault> {
    if !crate::record::format::valid_uuid(progress.operation_id)
        || !crate::record::format::valid_uuid(progress.session_id)
        || progress.updated_at.is_empty()
        || progress.updated_at.len() > 64
        || (progress.next_index < progress.expected_index)
        || (progress.next_index == progress.expected_index
            && progress.expected_stack == progress.next_stack)
    {
        return Err(Fault::bad_request());
    }
    let expected_index =
        i64::try_from(progress.expected_index).map_err(|_| Fault::bad_request())?;
    let next_index = i64::try_from(progress.next_index).map_err(|_| Fault::bad_request())?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let changed = transaction
        .execute(
            "UPDATE session_deletion_progress SET manifest_index=?1,directory_stack=?2,updated_at=?3 WHERE operation_id=?4 AND EXISTS(SELECT 1 FROM session_deletion_ops WHERE operation_id=?4 AND session_id=?5 AND state='database-purged') AND manifest_index=?6 AND directory_stack IS ?7",
            rusqlite::params![next_index, progress.next_stack, progress.updated_at, progress.operation_id, progress.session_id, expected_index, progress.expected_stack],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "隔离文件清理游标已变化"));
    }
    let changed = transaction
        .execute(
            "UPDATE session_deletion_ops SET updated_at=?1 WHERE operation_id=?2 AND session_id=?3 AND state='database-purged'",
            rusqlite::params![progress.updated_at, progress.operation_id, progress.session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "删除阶段已变化"));
    }
    transaction.commit().map_err(database)?;
    Ok(())
}

/// 记录隔离树已清空；随后可单独提交完成墓碑。
pub(crate) fn mark_deletion_files_purged(
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
    let state: Option<(String, String)> = transaction
        .query_row(
            "SELECT session_id,state FROM session_deletion_ops WHERE operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((target_id, state)) = state else {
        let target_hash: Option<String> = transaction
            .query_row(
                "SELECT target_identity_hash FROM session_deletion_receipts WHERE operation_id=?1",
                [operation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database)?;
        if target_hash.as_deref() == Some(target_identity_hash(session_id).as_str()) {
            let revision = catalog_revision(&transaction)?;
            transaction.commit().map_err(database)?;
            return Ok(DeletionOperation {
                operation_id: operation_id.to_owned(),
                session_id: session_id.to_owned(),
                state: "completed".into(),
                catalog_revision: revision,
                diagnostic_code: None,
            });
        }
        return Err(Fault::new("app.not-found", "删除操作不存在"));
    };
    if target_id != session_id {
        return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
    }
    if state == "files-purged" {
        let revision = catalog_revision(&transaction)?;
        transaction.commit().map_err(database)?;
        return Ok(DeletionOperation {
            operation_id: operation_id.to_owned(),
            session_id: session_id.to_owned(),
            state,
            catalog_revision: revision,
            diagnostic_code: None,
        });
    }
    if state != "database-purged" {
        return Err(Fault::new("app.conflict", "隔离文件清理阶段不匹配"));
    }
    let cleanup = manifest_for_quarantine_cleanup(&transaction, operation_id, session_id)?;
    if cleanup.index != cleanup.entries.len() {
        return Err(Fault::new("app.conflict", "隔离文件仍未清理完成"));
    }
    let changed = transaction
        .execute(
            "UPDATE session_deletion_ops SET state='files-purged',updated_at=?1 WHERE operation_id=?2 AND session_id=?3 AND state='database-purged'",
            rusqlite::params![updated_at, operation_id, session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "隔离文件清理阶段已变化"));
    }
    let revision = catalog_revision(&transaction)?;
    transaction.commit().map_err(database)?;
    Ok(DeletionOperation {
        operation_id: operation_id.to_owned(),
        session_id: session_id.to_owned(),
        state: "files-purged".into(),
        catalog_revision: revision,
        diagnostic_code: None,
    })
}

/// 在文件清理完成后保留不可见的幂等完成墓碑。
pub(crate) fn complete_deletion(
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
    let state: Option<(String, String)> = transaction
        .query_row(
            "SELECT session_id,state FROM session_deletion_ops WHERE operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((target_id, state)) = state else {
        let target_hash: Option<String> = transaction
            .query_row(
                "SELECT target_identity_hash FROM session_deletion_receipts WHERE operation_id=?1",
                [operation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database)?;
        if target_hash.as_deref() == Some(target_identity_hash(session_id).as_str()) {
            let revision = catalog_revision(&transaction)?;
            transaction.commit().map_err(database)?;
            return Ok(DeletionOperation {
                operation_id: operation_id.to_owned(),
                session_id: session_id.to_owned(),
                state: "completed".into(),
                catalog_revision: revision,
                diagnostic_code: None,
            });
        }
        return Err(Fault::new("app.not-found", "删除操作不存在"));
    };
    if target_id != session_id {
        return Err(Fault::new("app.conflict", "删除操作与周目不匹配"));
    }
    if state == "completed" {
        let revision = catalog_revision(&transaction)?;
        transaction.commit().map_err(database)?;
        return Ok(DeletionOperation {
            operation_id: operation_id.to_owned(),
            session_id: session_id.to_owned(),
            state,
            catalog_revision: revision,
            diagnostic_code: None,
        });
    }
    if state != "files-purged" {
        return Err(Fault::new("app.conflict", "删除完成阶段不匹配"));
    }
    transaction
        .execute(
            "INSERT INTO session_deletion_receipts(operation_id,target_identity_hash,completed_at,result_code) VALUES(?1,?2,?3,'completed')",
            rusqlite::params![operation_id, target_identity_hash(session_id), updated_at],
        )
        .map_err(database)?;
    transaction
        .execute(
            "DELETE FROM session_deletion_progress WHERE operation_id=?1",
            [operation_id],
        )
        .map_err(database)?;
    let changed = transaction
        .execute(
            "DELETE FROM session_deletion_ops WHERE operation_id=?1 AND session_id=?2 AND state='files-purged'",
            rusqlite::params![operation_id, session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "删除完成阶段已变化"));
    }
    let revision = catalog_revision(&transaction)?;
    transaction.commit().map_err(database)?;
    Ok(DeletionOperation {
        operation_id: operation_id.to_owned(),
        session_id: session_id.to_owned(),
        state: "completed".into(),
        catalog_revision: revision,
        diagnostic_code: None,
    })
}

/// 将旧版完整完成记录迁移为仅含幂等身份哈希的墓碑。
pub(crate) fn compact_completed_receipts(
    connection: &mut rusqlite::Connection,
) -> Result<(), Fault> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let legacy = {
        let mut statement = transaction
            .prepare(
                "SELECT operation_id,session_id,updated_at FROM session_deletion_ops WHERE state='completed'",
            )
            .map_err(database)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(database)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(database)?
    };
    for (operation_id, session_id, completed_at) in legacy {
        let expected_hash = target_identity_hash(&session_id);
        let existing_hash: Option<String> = transaction
            .query_row(
                "SELECT target_identity_hash FROM session_deletion_receipts WHERE operation_id=?1",
                [&operation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database)?;
        if existing_hash
            .as_ref()
            .is_some_and(|value| value != &expected_hash)
        {
            return Err(corrupt());
        }
        transaction
            .execute(
                "INSERT OR IGNORE INTO session_deletion_receipts(operation_id,target_identity_hash,completed_at,result_code) VALUES(?1,?2,?3,'completed')",
                rusqlite::params![operation_id, expected_hash, completed_at],
            )
            .map_err(database)?;
        transaction
            .execute(
                "DELETE FROM session_deletion_progress WHERE operation_id=?1",
                [&operation_id],
            )
            .map_err(database)?;
        transaction
            .execute(
                "DELETE FROM session_deletion_ops WHERE operation_id=?1 AND state='completed'",
                [&operation_id],
            )
            .map_err(database)?;
    }
    transaction.commit().map_err(database)
}

fn target_identity_hash(session_id: &str) -> String {
    format!("{:x}", Sha256::digest(session_id.as_bytes()))
}

fn validate_frozen_manifest(
    bytes: &[u8],
    operation_id: &str,
    session_id: &str,
    script_id: &str,
    memory_run_id: &str,
) -> Result<(), Fault> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(corrupt());
    }
    let envelope: FrozenDeletionEnvelope = serde_json::from_slice(bytes).map_err(|_| corrupt())?;
    let manifest_bytes = serde_json::to_vec(&envelope.manifest).map_err(|_| corrupt())?;
    let expected_hash = format!("sha256:{:x}", Sha256::digest(&manifest_bytes));
    if envelope.hash != expected_hash
        || envelope.manifest.version != 1
        || envelope.manifest.operation_id != operation_id
        || envelope.manifest.session_id != session_id
        || envelope.manifest.script_id != script_id
        || envelope.manifest.memory_run_id != memory_run_id
    {
        return Err(corrupt());
    }
    validate_manifest_entries(
        &envelope.manifest.entries,
        operation_id,
        script_id,
        session_id,
        memory_run_id,
    )
}

fn purge_scoped_data(
    connection: &rusqlite::Connection,
    session_id: &str,
    memory_run_id: &str,
    script_id: &str,
) -> Result<(), Fault> {
    let scopes = [session_id, memory_run_id];
    let entity_ids = collect_strings(
        connection,
        "SELECT entity_id FROM entity_registry_entries WHERE session_id=?1 ORDER BY entity_id",
        &[session_id],
    )?;
    let mutation_ids = collect_strings(
        connection,
        "SELECT mutation_id FROM entity_registry_entries WHERE session_id=?1 ORDER BY mutation_id",
        &[session_id],
    )?;
    let cycle_mark_ids = direct_cycle_mark_ids(connection, script_id, memory_run_id, &entity_ids)?;

    // 账单审计只按已核验 scope 或属于本局的物理 requestId 清理。
    connection
        .execute(
            "DELETE FROM billing_pending_settlements WHERE request_id IN (SELECT request_id FROM billing_physical_requests WHERE run_id IN (?1,?2))",
            rusqlite::params![scopes[0], scopes[1]],
        )
        .map_err(database)?;
    connection
        .execute(
            "DELETE FROM billing_audit WHERE (operation_kind IN ('run-budget-update','legacy-budget-resolution','legacy-scope-reconcile') AND target_id IN (?1,?2)) OR (operation_kind='settlement-backfill' AND target_id IN (SELECT request_id FROM billing_physical_requests WHERE run_id IN (?1,?2)))",
            rusqlite::params![scopes[0], scopes[1]],
        )
        .map_err(database)?;
    connection
        .execute(
            "DELETE FROM billing_budget_warnings WHERE run_id IN (?1,?2)",
            rusqlite::params![scopes[0], scopes[1]],
        )
        .map_err(database)?;
    connection
        .execute(
            "DELETE FROM billing_physical_requests WHERE run_id IN (?1,?2)",
            rusqlite::params![scopes[0], scopes[1]],
        )
        .map_err(database)?;
    connection
        .execute(
            "DELETE FROM billing_run_budgets WHERE run_id IN (?1,?2)",
            rusqlite::params![scopes[0], scopes[1]],
        )
        .map_err(database)?;

    // 先删除实体行解除外键，再清除可由实体目录精确归属的 mutation 回执。
    connection
        .execute(
            "DELETE FROM entity_registry_entries WHERE session_id=?1",
            [session_id],
        )
        .map_err(database)?;
    for mutation_id in &mutation_ids {
        connection
            .execute("DELETE FROM store_applied WHERE id=?1", [mutation_id])
            .map_err(database)?;
    }
    connection
        .execute(
            "DELETE FROM entity_registry_runs WHERE session_id=?1",
            [session_id],
        )
        .map_err(database)?;

    delete_cycle_marks(connection, &cycle_mark_ids)?;
    connection
        .execute(
            "DELETE FROM memory_run_sessions WHERE run_id=?1 AND session_id=?2",
            rusqlite::params![memory_run_id, session_id],
        )
        .map_err(database)?;
    delete_memory_run(connection, memory_run_id)?;
    Ok(())
}

fn collect_strings(
    connection: &rusqlite::Connection,
    sql: &str,
    values: &[&str],
) -> Result<Vec<String>, Fault> {
    let mut statement = connection.prepare(sql).map_err(database)?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(values.iter().copied()))
        .map_err(database)?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(database)? {
        result.push(row.get(0).map_err(database)?);
    }
    Ok(result)
}

fn direct_cycle_mark_ids(
    connection: &rusqlite::Connection,
    script_id: &str,
    memory_run_id: &str,
    entity_ids: &[String],
) -> Result<Vec<String>, Fault> {
    let malformed_source: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_cycle_sources s JOIN memory_cycle_marks m USING(mark_id) WHERE m.script_id=?1 AND json_valid(CAST(s.source_json AS TEXT))=0)",
            [script_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    let malformed_subjects: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_cycle_marks WHERE script_id=?1 AND json_valid(CAST(subject_ids_json AS TEXT))=0)",
            [script_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if malformed_source || malformed_subjects {
        return Err(corrupt());
    }
    let invalid_source_shape: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_cycle_sources s JOIN memory_cycle_marks m USING(mark_id) WHERE m.script_id=?1 AND (COALESCE(json_type(CAST(s.source_json AS TEXT)),'')<>'object' OR COALESCE(json_type(CAST(s.source_json AS TEXT),'$.source'),'')<>'object' OR COALESCE(json_type(CAST(s.source_json AS TEXT),'$.source.runId'),'')<>'text' OR COALESCE(json_type(CAST(s.source_json AS TEXT),'$.source.scriptId'),'')<>'text'))",
            [script_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    let invalid_subject_shape: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_cycle_marks m WHERE m.script_id=?1 AND (COALESCE(json_type(CAST(m.subject_ids_json AS TEXT)),'')<>'array' OR EXISTS(SELECT 1 FROM json_each(CAST(m.subject_ids_json AS TEXT)) WHERE type<>'text'))) ",
            [script_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if invalid_source_shape || invalid_subject_shape {
        return Err(corrupt());
    }
    let mut mark_ids = collect_strings(
        connection,
        "SELECT DISTINCT s.mark_id FROM memory_cycle_sources s JOIN memory_cycle_marks m USING(mark_id) WHERE m.script_id=?1 AND json_extract(CAST(s.source_json AS TEXT),'$.source.runId')=?2 ORDER BY s.mark_id",
        &[script_id, memory_run_id],
    )?;
    for entity_id in entity_ids {
        mark_ids.extend(collect_strings(
            connection,
            "SELECT mark_id FROM memory_cycle_marks WHERE script_id=?1 AND EXISTS (SELECT 1 FROM json_each(CAST(subject_ids_json AS TEXT)) WHERE value=?2)",
            &[script_id, entity_id],
        )?);
    }
    mark_ids.sort();
    mark_ids.dedup();
    Ok(mark_ids)
}

fn delete_cycle_marks(connection: &rusqlite::Connection, mark_ids: &[String]) -> Result<(), Fault> {
    for mark_id in mark_ids {
        connection
            .execute(
                "DELETE FROM memory_cycle_sources WHERE mark_id=?1",
                [mark_id],
            )
            .map_err(database)?;
        connection
            .execute("DELETE FROM memory_cycle_marks WHERE mark_id=?1", [mark_id])
            .map_err(database)?;
    }
    Ok(())
}

fn delete_memory_run(connection: &rusqlite::Connection, run_id: &str) -> Result<(), Fault> {
    let run_ids = [run_id];
    for sql in [
        "DELETE FROM memory_alias_events WHERE alias_id IN (SELECT alias_id FROM memory_aliases WHERE run_id=?1)",
        "DELETE FROM memory_alias_sources WHERE alias_id IN (SELECT alias_id FROM memory_aliases WHERE run_id=?1)",
        "DELETE FROM memory_aliases WHERE run_id=?1",
        "DELETE FROM memory_batch_candidate_operations WHERE batch_id IN (SELECT batch_id FROM memory_batches WHERE run_id=?1) OR operation_id IN (SELECT operation_id FROM memory_operations WHERE run_id=?1)",
        "DELETE FROM memory_batch_legacy_completions WHERE batch_id IN (SELECT batch_id FROM memory_batches WHERE run_id=?1)",
        "DELETE FROM memory_batch_receipts WHERE batch_id IN (SELECT batch_id FROM memory_batches WHERE run_id=?1)",
        "DELETE FROM memory_spans WHERE batch_id IN (SELECT batch_id FROM memory_batches WHERE run_id=?1) OR material_id IN (SELECT material_id FROM memory_materials WHERE run_id=?1)",
        "DELETE FROM memory_operation_targets WHERE operation_id IN (SELECT operation_id FROM memory_operations WHERE run_id=?1)",
        "DELETE FROM memory_effects WHERE run_id=?1 OR operation_id IN (SELECT operation_id FROM memory_operations WHERE run_id=?1)",
        "DELETE FROM memory_processed WHERE material_id IN (SELECT material_id FROM memory_materials WHERE run_id=?1) OR operation_id IN (SELECT operation_id FROM memory_operations WHERE run_id=?1)",
        "DELETE FROM memory_effect_evidence_progress WHERE entry_id IN (SELECT entry_id FROM memory_entries WHERE run_id=?1)",
        "DELETE FROM memory_material_positions WHERE material_id IN (SELECT material_id FROM memory_materials WHERE run_id=?1)",
        "DELETE FROM memory_cleanup WHERE run_id=?1",
        "DELETE FROM memory_clock_receipts WHERE run_id=?1",
        "DELETE FROM memory_cycle_scene_evidence WHERE run_id=?1",
        "DELETE FROM memory_cycle_scene_hints WHERE run_id=?1",
        "DELETE FROM memory_cycle_scene_progress WHERE run_id=?1",
        "DELETE FROM memory_cycle_events WHERE run_id=?1",
        "DELETE FROM memory_questions WHERE run_id=?1",
        "DELETE FROM memory_materials WHERE run_id=?1",
        "DELETE FROM memory_batches WHERE run_id=?1",
        "DELETE FROM memory_operations WHERE run_id=?1",
        "DELETE FROM memory_version_sources WHERE version_id IN (SELECT version_id FROM memory_versions WHERE entry_id IN (SELECT entry_id FROM memory_entries WHERE run_id=?1))",
        "DELETE FROM memory_version_evidence WHERE version_id IN (SELECT version_id FROM memory_versions WHERE entry_id IN (SELECT entry_id FROM memory_entries WHERE run_id=?1))",
        "DELETE FROM memory_versions WHERE entry_id IN (SELECT entry_id FROM memory_entries WHERE run_id=?1)",
        "DELETE FROM memory_entries WHERE run_id=?1",
        "DELETE FROM memory_run_faults WHERE run_id=?1",
        "DELETE FROM memory_run_cycle_generations WHERE run_id=?1",
    ] {
        connection.execute(sql, run_ids).map_err(database)?;
    }
    connection
        .execute("DELETE FROM memory_runs WHERE run_id=?1", run_ids)
        .map_err(database)?;
    Ok(())
}

fn delete_catalog_row(connection: &rusqlite::Connection, session_id: &str) -> Result<(), Fault> {
    let changed = connection
        .execute(
            "DELETE FROM session_catalog WHERE session_id=?1 AND state='deleting'",
            [session_id],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(Fault::new("app.conflict", "待删除周目目录状态已变化"));
    }
    Ok(())
}

fn validate_manifest_entries(
    entries: &[DeletionManifestEntry],
    operation_id: &str,
    script_id: &str,
    session_id: &str,
    memory_run_id: &str,
) -> Result<(), Fault> {
    let mut previous_source: Option<&str> = None;
    let mut transcript_count = 0;
    let mut transcript_entry_count = 0;
    let mut memory_directory_count = 0;
    for entry in entries {
        if entry.kind != DeletionManifestEntryKind::MemoryRunDirectory {
            transcript_entry_count += 1;
        }
        let source = entry.source.as_str();
        if source.is_empty()
            || source.len() > 1024
            || source.starts_with('/')
            || source.contains('\\')
            || source
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
            || previous_source.is_some_and(|previous| previous >= source)
            || !match (entry.kind, entry.identity_hash.as_deref()) {
                (DeletionManifestEntryKind::MemoryRunDirectory, None) => true,
                (_, Some(identity_hash)) => valid_sha256(identity_hash),
                _ => false,
            }
        {
            return Err(Fault::bad_request());
        }
        previous_source = Some(source);
        let transcript = format!("workspaces/{script_id}/transcript/{session_id}.jsonl");
        let transcript_prefix = format!("workspaces/{script_id}/transcript/{session_id}.");
        let memory_directory = format!("workspaces/{script_id}/memory/runs/{memory_run_id}");
        let (allowed, quarantine_prefix) = match entry.kind {
            DeletionManifestEntryKind::Transcript => {
                transcript_count += 1;
                (source == transcript, "transcript")
            }
            DeletionManifestEntryKind::Partial => (
                valid_transcript_sidecar(source, &transcript_prefix, false),
                "transcript",
            ),
            DeletionManifestEntryKind::RecoveryBackup => (
                valid_transcript_sidecar(source, &transcript_prefix, true),
                "transcript",
            ),
            DeletionManifestEntryKind::MemoryRunDirectory => {
                memory_directory_count += 1;
                (source == memory_directory, "memory")
            }
        };
        let filename = source.rsplit('/').next().unwrap_or_default();
        let expected_quarantine = format!("deleting/{operation_id}/{quarantine_prefix}/{filename}");
        if !allowed || entry.quarantine != expected_quarantine {
            return Err(Fault::bad_request());
        }
    }
    if transcript_count != 1
        || memory_directory_count != 1
        || transcript_entry_count > MAX_TRANSCRIPT_MANIFEST_ENTRIES
    {
        return Err(Fault::bad_request());
    }
    Ok(())
}

fn valid_transcript_sidecar(source: &str, prefix: &str, recovery: bool) -> bool {
    let Some(tail) = source.strip_prefix(prefix) else {
        return false;
    };
    if recovery {
        tail.strip_suffix(".bak")
            .and_then(|name| name.strip_prefix("recovery-"))
            .is_some_and(crate::record::format::valid_uuid)
    } else {
        tail.strip_suffix(".partial.jsonl")
            .is_some_and(crate::record::format::valid_uuid)
    }
}

fn valid_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_FIXTURE: AtomicU32 = AtomicU32::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aoidos-deletion-manifest-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn prepare_run(&self, session_id: &str, memory_run_id: &str) -> PathBuf {
            let transcript = self.0.join("workspaces/mistbell/transcript");
            let memory = self
                .0
                .join(format!("workspaces/mistbell/memory/runs/{memory_run_id}"));
            std::fs::create_dir_all(&transcript).unwrap();
            std::fs::create_dir_all(&memory).unwrap();
            std::fs::write(
                transcript.join(format!("{session_id}.jsonl")),
                b"transcript",
            )
            .unwrap();
            transcript
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn filesystem_enumeration_freezes_only_the_target_run_files() {
        let fixture = Fixture::new();
        let operation_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let memory_run_id = "00000000-0000-4000-8000-000000000003";
        let transcript = fixture.prepare_run(session_id, memory_run_id);
        std::fs::write(
            transcript.join(format!(
                "{session_id}.00000000-0000-4000-8000-000000000004.partial.jsonl"
            )),
            b"partial",
        )
        .unwrap();
        std::fs::write(
            transcript.join(format!(
                "{session_id}.recovery-00000000-0000-4000-8000-000000000005.bak"
            )),
            b"backup",
        )
        .unwrap();
        std::fs::write(
            transcript.join("00000000-0000-4000-8000-000000000006.jsonl"),
            b"another run",
        )
        .unwrap();

        let entries = enumerate_deletion_manifest_entries(
            &fixture.0,
            operation_id,
            "mistbell",
            session_id,
            memory_run_id,
        )
        .unwrap();

        assert_eq!(entries.len(), 4);
        assert!(
            entries
                .windows(2)
                .all(|pair| pair[0].source < pair[1].source)
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.kind == DeletionManifestEntryKind::Transcript)
                .count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.kind == DeletionManifestEntryKind::Partial)
                .count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.kind == DeletionManifestEntryKind::RecoveryBackup)
                .count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.kind == DeletionManifestEntryKind::MemoryRunDirectory)
                .count(),
            1
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.identity_hash.as_deref().is_some_and(valid_sha256))
        );
        assert!(entries.iter().all(|entry| {
            !entry
                .source
                .contains("00000000-0000-4000-8000-000000000006")
        }));
    }

    #[test]
    fn filesystem_enumeration_refuses_incomplete_or_unknown_target_entries() {
        let fixture = Fixture::new();
        let operation_id = "00000000-0000-4000-8000-000000000011";
        let session_id = "00000000-0000-4000-8000-000000000012";
        let memory_run_id = "00000000-0000-4000-8000-000000000013";
        let transcript = fixture.prepare_run(session_id, memory_run_id);
        std::fs::write(transcript.join("unrelated.jsonl"), b"another run").unwrap();

        let result = enumerate_deletion_manifest_entries_with_limit(
            &fixture.0,
            operation_id,
            "mistbell",
            session_id,
            memory_run_id,
            1,
        );
        assert_eq!(result.unwrap_err().code, "app.capacity");

        std::fs::remove_file(transcript.join("unrelated.jsonl")).unwrap();
        std::fs::write(transcript.join(format!("{session_id}.unknown")), b"unknown").unwrap();
        let result = enumerate_deletion_manifest_entries(
            &fixture.0,
            operation_id,
            "mistbell",
            session_id,
            memory_run_id,
        );
        assert_eq!(result.unwrap_err().code, "app.bad-request");
    }

    #[cfg(unix)]
    #[test]
    fn unrelated_links_and_directories_do_not_block_a_run_without_memory_files() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let operation_id = "00000000-0000-4000-8000-000000000021";
        let session_id = "00000000-0000-4000-8000-000000000022";
        let memory_run_id = "00000000-0000-4000-8000-000000000023";
        let transcript = fixture.0.join("workspaces/mistbell/transcript");
        std::fs::create_dir_all(&transcript).unwrap();
        std::fs::write(
            transcript.join(format!("{session_id}.jsonl")),
            b"transcript",
        )
        .unwrap();
        std::fs::write(fixture.0.join("unrelated-target"), b"other run").unwrap();
        symlink(
            fixture.0.join("unrelated-target"),
            transcript.join("another-run.jsonl"),
        )
        .unwrap();
        std::fs::create_dir(transcript.join("unrelated-directory")).unwrap();

        let entries = enumerate_deletion_manifest_entries(
            &fixture.0,
            operation_id,
            "mistbell",
            session_id,
            memory_run_id,
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        let memory = entries
            .iter()
            .find(|entry| entry.kind == DeletionManifestEntryKind::MemoryRunDirectory)
            .unwrap();
        assert_eq!(memory.identity_hash, None);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.kind == DeletionManifestEntryKind::Transcript)
                .count(),
            1
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unrelated_non_utf8_name_does_not_block_a_run() {
        use std::os::unix::ffi::OsStrExt;

        let fixture = Fixture::new();
        let operation_id = "00000000-0000-4000-8000-000000000031";
        let session_id = "00000000-0000-4000-8000-000000000032";
        let memory_run_id = "00000000-0000-4000-8000-000000000033";
        let transcript = fixture.0.join("workspaces/mistbell/transcript");
        std::fs::create_dir_all(&transcript).unwrap();
        std::fs::write(
            transcript.join(format!("{session_id}.jsonl")),
            b"transcript",
        )
        .unwrap();
        std::fs::write(
            transcript.join(std::ffi::OsStr::from_bytes(b"unrelated-\xff.jsonl")),
            b"other run",
        )
        .unwrap();

        let entries = enumerate_deletion_manifest_entries(
            &fixture.0,
            operation_id,
            "mistbell",
            session_id,
            memory_run_id,
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| {
            entry.kind == DeletionManifestEntryKind::Transcript || entry.identity_hash.is_none()
        }));
    }
}
