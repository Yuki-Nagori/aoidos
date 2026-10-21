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
