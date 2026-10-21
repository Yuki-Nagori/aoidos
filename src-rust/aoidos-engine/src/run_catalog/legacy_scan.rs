//! 显式推进的旧 transcript 发现；只读 header，不打开或恢复会话。

use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs::{self, ReadDir},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

pub const MIGRATION: &str = r#"
CREATE TABLE session_scan_issues (
  candidate_hash TEXT PRIMARY KEY CHECK(length(candidate_hash) = 71),
  script_id TEXT CHECK(script_id IS NULL OR length(script_id) BETWEEN 1 AND 64),
  reason TEXT NOT NULL CHECK(reason IN (
    'invalid-path', 'symlink', 'not-regular', 'header-too-large',
    'invalid-header', 'identity-mismatch', 'unreadable'
  )),
  updated_at TEXT NOT NULL CHECK(length(updated_at) BETWEEN 1 AND 64)
);
"#;

const MAX_TRAVERSAL_ITEMS: usize = 128;
const MAX_HEADER_BYTES: usize = crate::record::format::MAX_LINE;
const MAX_BATCH_HEADER_BYTES: usize = 8 * 1024 * 1024;

/// 一次显式扫描推进的数量；不包含文件路径或 header 正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyScanResult {
    pub discovered: usize,
    pub diagnosed: usize,
    pub scan_incomplete: bool,
    pub scan_issue_count: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    root: PathBuf,
    root_path: PathBuf,
    path: PathBuf,
    key: String,
    script_id: String,
    session_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Issue {
    key: String,
    script_id: Option<String>,
    reason: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) enum Item {
    Candidate(Candidate),
    Issue(Issue),
}

enum PreparedItem {
    Issue(Issue),
    Candidate(Box<PreparedCandidate>),
}

struct PreparedCandidate {
    candidate: Candidate,
    header: Option<crate::record::format::Header>,
    failure: Option<&'static str>,
}

pub(crate) struct PreparedBatch {
    items: Vec<PreparedItem>,
    deferred: Vec<Item>,
}

impl Item {
    fn key(&self) -> &str {
        match self {
            Self::Candidate(candidate) => &candidate.key,
            Self::Issue(issue) => &issue.key,
        }
    }
}

#[derive(Debug, Clone)]
enum DirectoryKind {
    Workspaces,
    Script(String),
    Transcript(String),
}

struct OpenDirectory {
    path: PathBuf,
    kind: DirectoryKind,
    entries: ReadDir,
}

/// 进程内保存 ReadDir 游标；重启后从根目录重扫，已登记条目以身份键幂等跳过。
pub(crate) struct LegacyScanner {
    root: PathBuf,
    canonical_root: Option<PathBuf>,
    directories: Vec<OpenDirectory>,
    pending: VecDeque<Item>,
    traversal_finished: bool,
    scan_started: bool,
}

impl LegacyScanner {
    pub(crate) fn new(root: &Path) -> Self {
        let workspaces = root.join("workspaces");
        let canonical_root = fs::canonicalize(root).ok();
        let mut scanner = Self {
            root: root.to_owned(),
            canonical_root,
            directories: Vec::new(),
            pending: VecDeque::new(),
            traversal_finished: false,
            scan_started: false,
        };
        match fs::symlink_metadata(&workspaces) {
            Ok(metadata) if metadata.file_type().is_dir() => {
                match scanner.open_directory(workspaces, DirectoryKind::Workspaces) {
                    Ok(directory) => scanner.directories.push(directory),
                    Err(reason) => scanner.pending.push_back(Item::Issue(Issue {
                        key: "workspaces".into(),
                        script_id: None,
                        reason,
                    })),
                }
            }
            Ok(metadata) => {
                scanner.pending.push_back(Item::Issue(Issue {
                    key: "workspaces".into(),
                    script_id: None,
                    reason: if metadata.file_type().is_symlink() {
                        "symlink"
                    } else {
                        "not-regular"
                    },
                }));
                scanner.traversal_finished = true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                scanner.traversal_finished = true;
            }
            Err(_) => {
                scanner.pending.push_back(Item::Issue(Issue {
                    key: "workspaces".into(),
                    script_id: None,
                    reason: "unreadable",
                }));
                scanner.traversal_finished = true;
            }
        }
        scanner
    }

    /// 返回一个遍历项有界的批次，并把候选稳定排序后交给数据库层。
    pub(crate) fn next_batch(&mut self) -> Vec<Item> {
        let mut batch = Vec::new();
        let mut traversed = 0;
        while traversed < MAX_TRAVERSAL_ITEMS && batch.len() < MAX_TRAVERSAL_ITEMS {
            if let Some(item) = self.pending.pop_front() {
                batch.push(item);
                continue;
            }
            if self.traversal_finished {
                break;
            }
            let Some(current) = self.directories.last_mut() else {
                self.traversal_finished = true;
                continue;
            };
            let (next, current_path, current_kind) = (
                current.entries.next(),
                current.path.clone(),
                current.kind.clone(),
            );
            match next {
                Some(Ok(entry)) => {
                    traversed += 1;
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let kind = current.kind.clone();
                    self.visit_entry(&path, &name, kind, &mut batch);
                }
                Some(Err(_)) => {
                    traversed += 1;
                    let issue = Item::Issue(Issue {
                        key: self.relative_key(&current_path),
                        script_id: match current_kind {
                            DirectoryKind::Script(ref id) | DirectoryKind::Transcript(ref id) => {
                                Some(id.clone())
                            }
                            DirectoryKind::Workspaces => None,
                        },
                        reason: "unreadable",
                    });
                    batch.push(issue);
                }
                None => {
                    self.directories.pop();
                }
            }
        }
        batch.sort_by(|left, right| left.key().cmp(right.key()));
        batch
    }

    fn visit_entry(&mut self, path: &Path, name: &str, kind: DirectoryKind, batch: &mut Vec<Item>) {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => {
                batch.push(Item::Issue(Issue {
                    key: self.relative_key(path),
                    script_id: kind_script_id(&kind),
                    reason: "unreadable",
                }));
                return;
            }
        };
        if metadata.file_type().is_symlink() {
            if matches!(kind, DirectoryKind::Transcript(_)) && name.ends_with(".jsonl")
                || matches!(kind, DirectoryKind::Script(_)) && name == "transcript"
                || matches!(kind, DirectoryKind::Workspaces) && aoidos_script::valid_id(name)
            {
                batch.push(Item::Issue(Issue {
                    key: self.relative_key(path),
                    script_id: kind_script_id(&kind),
                    reason: "symlink",
                }));
            }
            return;
        }
        match kind {
            DirectoryKind::Workspaces => {
                if metadata.is_dir() {
                    if aoidos_script::valid_id(name) {
                        self.open_child(path, DirectoryKind::Script(name.to_owned()), batch);
                    } else {
                        batch.push(Item::Issue(Issue {
                            key: self.relative_key(path),
                            script_id: None,
                            reason: "invalid-path",
                        }));
                    }
                }
            }
            DirectoryKind::Script(script_id) => {
                if name == "transcript" {
                    if metadata.is_dir() {
                        self.open_child(path, DirectoryKind::Transcript(script_id), batch);
                    } else {
                        batch.push(Item::Issue(Issue {
                            key: self.relative_key(path),
                            script_id: Some(script_id),
                            reason: "not-regular",
                        }));
                    }
                }
            }
            DirectoryKind::Transcript(script_id) => {
                if !name.ends_with(".jsonl") {
                    return;
                }
                let Some(session_id) = name.strip_suffix(".jsonl") else {
                    return;
                };
                if !crate::record::format::valid_uuid(session_id) {
                    batch.push(Item::Issue(Issue {
                        key: self.relative_key(path),
                        script_id: Some(script_id),
                        reason: "invalid-path",
                    }));
                } else if !metadata.is_file() {
                    batch.push(Item::Issue(Issue {
                        key: self.relative_key(path),
                        script_id: Some(script_id),
                        reason: "not-regular",
                    }));
                } else {
                    batch.push(Item::Candidate(Candidate {
                        root: self
                            .canonical_root
                            .clone()
                            .unwrap_or_else(|| self.root.clone()),
                        root_path: self.root.clone(),
                        path: path.to_owned(),
                        key: self.relative_key(path),
                        script_id,
                        session_id: session_id.to_owned(),
                    }));
                }
            }
        }
    }

    fn open_child(&mut self, path: &Path, kind: DirectoryKind, batch: &mut Vec<Item>) {
        match self.open_directory(path.to_owned(), kind.clone()) {
            Ok(directory) => self.directories.push(directory),
            Err(reason) => batch.push(Item::Issue(Issue {
                key: self.relative_key(path),
                script_id: kind_script_id(&kind),
                reason,
            })),
        }
    }

    fn open_directory(
        &self,
        path: PathBuf,
        kind: DirectoryKind,
    ) -> Result<OpenDirectory, &'static str> {
        let metadata = fs::symlink_metadata(&path).map_err(|_| "unreadable")?;
        if metadata.file_type().is_symlink() {
            return Err("symlink");
        }
        if !metadata.is_dir() {
            return Err("not-regular");
        }
        let Some(root) = self.canonical_root.as_deref() else {
            return Err("unreadable");
        };
        let canonical = fs::canonicalize(&path).map_err(|_| "unreadable")?;
        if !canonical.starts_with(root) {
            return Err("invalid-path");
        }
        let entries = fs::read_dir(&path).map_err(|_| "unreadable")?;
        // 读取目录项前再次确认路径仍解析到数据根目录内。
        let canonical_after_open = fs::canonicalize(&path).map_err(|_| "unreadable")?;
        if !canonical_after_open.starts_with(root) {
            return Err("invalid-path");
        }
        Ok(OpenDirectory {
            path,
            kind,
            entries,
        })
    }

    fn relative_key(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub(crate) fn finished(&self) -> bool {
        self.traversal_finished && self.directories.is_empty()
    }

    pub(crate) fn needs_start(&self) -> bool {
        !self.scan_started
    }

    pub(crate) fn mark_started(&mut self) {
        self.scan_started = true;
    }

    pub(crate) fn requeue(&mut self, batch: Vec<Item>) {
        for item in batch.into_iter().rev() {
            self.pending.push_front(item);
        }
    }
}

fn kind_script_id(kind: &DirectoryKind) -> Option<String> {
    match kind {
        DirectoryKind::Script(id) | DirectoryKind::Transcript(id) => Some(id.clone()),
        DirectoryKind::Workspaces => None,
    }
}

pub(crate) fn prepare_batch(items: &[Item]) -> PreparedBatch {
    let mut prepared = Vec::with_capacity(items.len());
    let mut deferred = Vec::new();
    let mut header_bytes = 0;
    for (index, item) in items.iter().enumerate() {
        match item {
            Item::Issue(issue) => prepared.push(PreparedItem::Issue(issue.clone())),
            Item::Candidate(candidate) => {
                let remaining = MAX_BATCH_HEADER_BYTES.saturating_sub(header_bytes);
                if remaining == 0 {
                    deferred.extend_from_slice(&items[index..]);
                    break;
                }
                let (header, bytes, failure, budget_exhausted) = read_header(candidate, remaining);
                if budget_exhausted {
                    deferred.extend_from_slice(&items[index..]);
                    break;
                }
                header_bytes += bytes;
                prepared.push(PreparedItem::Candidate(Box::new(PreparedCandidate {
                    candidate: candidate.clone(),
                    header,
                    failure,
                })));
            }
        }
    }
    PreparedBatch {
        items: prepared,
        deferred,
    }
}

pub(crate) fn begin_scan(connection: &mut rusqlite::Connection) -> Result<(), Fault> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    transaction
        .execute("DELETE FROM session_scan_issues", [])
        .map_err(database)?;
    transaction
        .execute(
            "UPDATE session_catalog_meta SET scan_incomplete=1 WHERE id=1",
            [],
        )
        .map_err(database)?;
    transaction.commit().map_err(database)
}

#[cfg(test)]
fn apply_batch(
    connection: &mut rusqlite::Connection,
    items: &[Item],
    complete: bool,
) -> Result<(LegacyScanResult, Vec<Item>), Fault> {
    apply_prepared_batch(connection, prepare_batch(items), complete)
}

pub(crate) fn apply_prepared_batch(
    connection: &mut rusqlite::Connection,
    batch: PreparedBatch,
    complete: bool,
) -> Result<(LegacyScanResult, Vec<Item>), Fault> {
    let PreparedBatch { items, deferred } = batch;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let mut discovered = 0;
    let mut diagnosed = 0;
    for item in items {
        match item {
            PreparedItem::Issue(issue) => {
                record_issue(&transaction, &issue)?;
                diagnosed += 1;
            }
            PreparedItem::Candidate(prepared) => {
                let PreparedCandidate {
                    candidate,
                    header,
                    failure,
                } = *prepared;
                if let Some(reason) = failure {
                    record_issue(
                        &transaction,
                        &Issue {
                            key: candidate.key.clone(),
                            script_id: Some(candidate.script_id.clone()),
                            reason,
                        },
                    )?;
                    diagnosed += 1;
                    continue;
                }
                let Some(header) = header else {
                    record_issue(
                        &transaction,
                        &Issue {
                            key: candidate.key.clone(),
                            script_id: Some(candidate.script_id.clone()),
                            reason: "invalid-header",
                        },
                    )?;
                    diagnosed += 1;
                    continue;
                };
                if header.session_id != candidate.session_id
                    || header.script_id != candidate.script_id
                    || header.validate().is_err()
                {
                    record_issue(
                        &transaction,
                        &Issue {
                            key: candidate.key.clone(),
                            script_id: Some(candidate.script_id.clone()),
                            reason: "identity-mismatch",
                        },
                    )?;
                    diagnosed += 1;
                    continue;
                }
                clear_issue(&transaction, &candidate.key)?;
                if register_legacy(&transaction, &header, &candidate.key)? {
                    discovered += 1;
                }
            }
        }
    }
    let scan_complete = complete && deferred.is_empty();
    if scan_complete {
        transaction
            .execute(
                "UPDATE session_catalog_meta SET scan_incomplete=0 WHERE id=1",
                [],
            )
            .map_err(database)?;
    }
    let scan_issue_count: i64 = transaction
        .query_row(
            "SELECT min(count(*), ?1) FROM session_scan_issues",
            [super::MAX_SCAN_ISSUES],
            |row| row.get(0),
        )
        .map_err(database)?;
    transaction.commit().map_err(database)?;
    Ok((
        LegacyScanResult {
            discovered,
            diagnosed,
            scan_incomplete: !scan_complete,
            scan_issue_count: u64::try_from(scan_issue_count).map_err(|_| corrupt())?,
        },
        deferred,
    ))
}

fn read_header(
    candidate: &Candidate,
    byte_budget: usize,
) -> (
    Option<crate::record::format::Header>,
    usize,
    Option<&'static str>,
    bool,
) {
    if let Some(reason) = validate_parent_chain(candidate) {
        return (None, 0, Some(reason), false);
    }
    let metadata = match fs::symlink_metadata(&candidate.path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return (None, 0, Some("symlink"), false);
        }
        Ok(_) => return (None, 0, Some("not-regular"), false),
        Err(_) => return (None, 0, Some("unreadable"), false),
    };
    let canonical = match fs::canonicalize(&candidate.path) {
        Ok(canonical) if canonical.starts_with(&candidate.root) => canonical,
        Ok(_) => return (None, 0, Some("invalid-path"), false),
        Err(_) => return (None, 0, Some("unreadable"), false),
    };
    let _ = (metadata, canonical);
    let file = match fs::File::open(&candidate.path) {
        Ok(file) => file,
        Err(_) => return (None, 0, Some("unreadable"), false),
    };
    if !file.metadata().is_ok_and(|metadata| metadata.is_file())
        || !fs::canonicalize(&candidate.path)
            .is_ok_and(|canonical| canonical.starts_with(&candidate.root))
    {
        return (None, 0, Some("invalid-path"), false);
    }
    let mut line = Vec::new();
    let read_limit = (MAX_HEADER_BYTES + 1).min(byte_budget);
    let read = BufReader::new(file)
        .take(read_limit as u64)
        .read_until(b'\n', &mut line);
    let Ok(bytes) = read else {
        return (None, line.len(), Some("unreadable"), false);
    };
    if bytes > MAX_HEADER_BYTES {
        return (None, bytes, Some("header-too-large"), false);
    }
    if !line.ends_with(b"\n") {
        let budget_exhausted = bytes == read_limit && read_limit <= MAX_HEADER_BYTES;
        return (
            None,
            bytes,
            (!budget_exhausted).then_some("invalid-header"),
            budget_exhausted,
        );
    }
    match aoidos_json::decode_bytes::<crate::record::format::Header>(&line, MAX_HEADER_BYTES) {
        Ok(header) => (Some(header), bytes, None, false),
        Err(_) => (None, bytes, Some("invalid-header"), false),
    }
}

fn validate_parent_chain(candidate: &Candidate) -> Option<&'static str> {
    let Some(parent) = candidate.path.parent() else {
        return Some("invalid-path");
    };
    if !parent.starts_with(&candidate.root_path) {
        return Some("invalid-path");
    }
    let mut ancestors = Vec::new();
    let mut current = Some(parent);
    while let Some(path) = current {
        if path == candidate.root_path {
            break;
        }
        ancestors.push(path);
        current = path.parent();
    }
    if current.is_none() {
        return Some("invalid-path");
    }
    for path in ancestors.into_iter().rev() {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => return Some("unreadable"),
        };
        if metadata.file_type().is_symlink() {
            return Some("symlink");
        }
        if !metadata.is_dir() {
            return Some("not-regular");
        }
        if !fs::canonicalize(path).is_ok_and(|canonical| canonical.starts_with(&candidate.root)) {
            return Some("invalid-path");
        }
    }
    None
}

fn record_issue(connection: &rusqlite::Connection, issue: &Issue) -> Result<(), Fault> {
    let key_hash = encode_hash(Sha256::digest(issue.key.as_bytes()));
    connection
        .execute(
            "INSERT INTO session_scan_issues(candidate_hash,script_id,reason,updated_at) SELECT ?1,?2,?3,strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE EXISTS(SELECT 1 FROM session_scan_issues WHERE candidate_hash=?1) OR (SELECT count(*) FROM session_scan_issues) < ?4 ON CONFLICT(candidate_hash) DO UPDATE SET script_id=excluded.script_id,reason=excluded.reason,updated_at=excluded.updated_at",
            rusqlite::params![key_hash, issue.script_id, issue.reason, super::MAX_SCAN_ISSUES],
        )
        .map_err(database)?;
    Ok(())
}

fn clear_issue(connection: &rusqlite::Connection, key: &str) -> Result<(), Fault> {
    let key_hash = encode_hash(Sha256::digest(key.as_bytes()));
    connection
        .execute(
            "DELETE FROM session_scan_issues WHERE candidate_hash=?1",
            [key_hash],
        )
        .map_err(database)?;
    Ok(())
}

fn encode_hash(hash: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(71);
    encoded.push_str("sha256:");
    for byte in hash.as_ref() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn register_legacy(
    connection: &rusqlite::Connection,
    header: &crate::record::format::Header,
    key: &str,
) -> Result<bool, Fault> {
    if let Some(existing) = connection
        .query_row(
            "SELECT script_id,created_at,memory_run_id,billing_run_id,state FROM session_catalog WHERE session_id=?1",
            [&header.session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
    {
        // 删除 journal 是单向流程；扫描不能因文件头变化改写目录删除状态。
        if existing.4 == RunState::Deleting.as_str() {
            return Ok(false);
        }
        if existing.0 == header.script_id && existing.1 == header.created_at {
            let mapped_run: Option<String> = connection
                .query_row(
                    "SELECT run_id FROM memory_run_sessions WHERE session_id=?1",
                    [&header.session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database)?;
            let mapped_script: Option<String> = mapped_run
                .as_deref()
                .map(|run_id| {
                    connection
                        .query_row(
                            "SELECT script_id FROM memory_runs WHERE run_id=?1",
                            [run_id],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(database)
                })
                .transpose()?
                .flatten();
            let identity_matches = existing.2.as_deref() == mapped_run.as_deref()
                && existing.3.as_deref() == Some(header.session_id.as_str())
                && mapped_run
                    .as_deref()
                    .is_some_and(crate::record::format::valid_uuid)
                && mapped_script.as_deref() == Some(header.script_id.as_str());
            if identity_matches {
                return Ok(false);
            }
            if existing.4 != RunState::NeedsRepair.as_str() {
                let revision = bump_revision(connection)?;
                connection
                    .execute(
                        "UPDATE session_catalog SET state='needs-repair',revision=?1,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE session_id=?2",
                        rusqlite::params![revision, header.session_id],
                    )
                    .map_err(database)?;
            }
            record_issue(
                connection,
                &Issue {
                    key: key.to_owned(),
                    script_id: Some(header.script_id.clone()),
                    reason: "identity-mismatch",
                },
            )?;
            return Ok(false);
        }
        if existing.4 != RunState::NeedsRepair.as_str() {
            let revision = bump_revision(connection)?;
            connection
                .execute(
                    "UPDATE session_catalog SET state='needs-repair',revision=?1,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE session_id=?2",
                    rusqlite::params![revision, header.session_id],
                )
                .map_err(database)?;
        }
        record_issue(
            connection,
            &Issue {
                key: key.to_owned(),
                script_id: Some(header.script_id.clone()),
                reason: "identity-mismatch",
            },
        )?;
        return Ok(false);
    }

    let mapping: Option<String> = connection
        .query_row(
            "SELECT run_id FROM memory_run_sessions WHERE session_id=?1",
            [&header.session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?;
    let mut state = RunState::NeedsProfileBinding;
    let mut memory_run_id = None;
    if let Some(run_id) = mapping {
        let memory_script_id: Option<String> = connection
            .query_row(
                "SELECT script_id FROM memory_runs WHERE run_id=?1",
                [&run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database)?;
        if !crate::record::format::valid_uuid(&run_id)
            || memory_script_id.as_deref() != Some(header.script_id.as_str())
        {
            state = RunState::NeedsRepair;
        } else {
            let claimed: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_catalog WHERE memory_run_id=?1)",
                    [&run_id],
                    |row| row.get(0),
                )
                .map_err(database)?;
            if claimed {
                state = RunState::NeedsRepair;
            } else {
                memory_run_id = Some(run_id);
            }
        }
    } else {
        // sessionId 不能推导独立 memoryRunId；缺少映射意味着身份无法核验。
        state = RunState::NeedsRepair;
    }
    if state == RunState::NeedsRepair {
        record_issue(
            connection,
            &Issue {
                key: key.to_owned(),
                script_id: Some(header.script_id.clone()),
                reason: "identity-mismatch",
            },
        )?;
    }
    if state != RunState::NeedsRepair {
        // 缺少历史 profile 时先绑定；绑定事务再检查旧账本 scope 并设置计费 reconciliation 门。
        state = RunState::NeedsProfileBinding;
    }
    let next_revision = bump_revision(connection)?;
    connection
        .execute(
            "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,profile_id,profile_schema_version,profile_snapshot,state,revision,updated_at) VALUES(?1,?2,?3,?4,?1,NULL,NULL,NULL,?5,?6,?3)",
            rusqlite::params![header.session_id, header.script_id, header.created_at, memory_run_id, state.as_str(), next_revision],
        )
        .map_err(database)?;
    Ok(true)
}

pub fn verify(connection: &rusqlite::Connection) -> Result<(), StoreError> {
    for statement in MIGRATION
        .split(';')
        .map(str::trim)
        .filter(|sql| sql.starts_with("CREATE "))
    {
        let Some(name) = super::schema_object_name(statement) else {
            return Err(StoreError::Corrupt("legacy scan schema mismatch".into()));
        };
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(aoidos_store::db::sqlite_error)?;
        if actual.as_deref().map(super::normalize_sql) != Some(super::normalize_sql(statement)) {
            return Err(StoreError::Corrupt("legacy scan schema mismatch".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> rusqlite::Connection {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(aoidos_billing::schema::SCHEMA)
            .unwrap();
        connection
            .execute_batch(aoidos_memory::schema::SCHEMA)
            .unwrap();
        connection.execute_batch(super::super::SCHEMA).unwrap();
        connection.execute_batch(MIGRATION).unwrap();
        connection
    }

    fn fixture_root() -> PathBuf {
        std::env::temp_dir().join(format!("aoidos-legacy-scan-{}", uuid::Uuid::new_v4()))
    }

    fn write_header(root: &Path, header: &crate::record::format::Header) {
        let path = root
            .join("workspaces")
            .join(&header.script_id)
            .join("transcript")
            .join(format!("{}.jsonl", header.session_id));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = aoidos_json::to_vec(header).unwrap();
        bytes.push(b'\n');
        bytes.extend_from_slice(b"not-read-by-the-indexer\n");
        fs::write(path, bytes).unwrap();
    }

    fn map_memory_run(connection: &rusqlite::Connection, session_id: &str, script_id: &str) {
        let run_id = uuid::Uuid::new_v4().to_string();
        connection
            .execute(
                "INSERT INTO memory_runs(run_id,script_id,policy_json,policy_hash) VALUES(?1,?2,'{}',zeroblob(32))",
                rusqlite::params![run_id, script_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO memory_run_sessions(run_id,session_id) VALUES(?1,?2)",
                rusqlite::params![run_id, session_id],
            )
            .unwrap();
    }

    fn scan_all(root: &Path, connection: &mut rusqlite::Connection) -> Vec<LegacyScanResult> {
        let mut scanner = LegacyScanner::new(root);
        begin_scan(connection).unwrap();
        let mut results = Vec::new();
        loop {
            let batch = scanner.next_batch();
            assert!(scanner.directories.len() <= 3);
            let complete = scanner.finished();
            let (result, deferred) = apply_batch(connection, &batch, complete).unwrap();
            scanner.requeue(deferred);
            let finished = !result.scan_incomplete;
            results.push(result);
            if finished {
                return results;
            }
        }
    }

    #[test]
    fn scan_registers_valid_headers_and_isolates_invalid_transcripts() {
        let root = fixture_root();
        let valid = crate::record::test_support::header();
        write_header(&root, &valid);
        let bad_path = root
            .join("workspaces/demo/transcript")
            .join(format!("{}.jsonl", uuid::Uuid::new_v4()));
        fs::write(&bad_path, b"{broken}\nbody stays untouched\n").unwrap();
        let mut connection = database();
        map_memory_run(&connection, &valid.session_id, &valid.script_id);

        let results = scan_all(&root, &mut connection);
        let page = page(&connection, None, DEFAULT_PAGE_SIZE).unwrap();

        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].session_id, valid.session_id);
        assert_eq!(page.entries[0].script_id, "demo");
        assert_eq!(page.entries[0].state, RunState::NeedsProfileBinding);
        assert!(!page.scan_incomplete);
        assert_eq!(page.scan_issue_count, 1);
        assert!(results.last().unwrap().scan_issue_count == 1);
        assert!(
            fs::read(bad_path)
                .unwrap()
                .ends_with(b"body stays untouched\n")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_memory_identity_is_isolated_for_repair() {
        let root = fixture_root();
        let header = crate::record::test_support::header();
        write_header(&root, &header);
        let mut connection = database();

        scan_all(&root, &mut connection);
        let first_page = page(&connection, None, DEFAULT_PAGE_SIZE).unwrap();

        assert_eq!(first_page.entries[0].state, RunState::NeedsRepair);
        assert_eq!(first_page.entries[0].memory_run_id, None);
        assert_eq!(first_page.scan_issue_count, 1);

        // 下一轮扫描会重置旧诊断；仍需从权威身份映射重新发现并报告修复原因。
        scan_all(&root, &mut connection);
        let rescanned = page(&connection, None, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(rescanned.entries[0].state, RunState::NeedsRepair);
        assert_eq!(rescanned.scan_issue_count, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_broken_mapping_on_an_existing_catalog_row_is_marked_for_repair() {
        let root = fixture_root();
        let header = crate::record::test_support::header();
        write_header(&root, &header);
        let mut connection = database();
        map_memory_run(&connection, &header.session_id, &header.script_id);
        scan_all(&root, &mut connection);
        connection
            .execute(
                "DELETE FROM memory_run_sessions WHERE session_id=?1",
                [&header.session_id],
            )
            .unwrap();

        scan_all(&root, &mut connection);
        let page = page(&connection, None, DEFAULT_PAGE_SIZE).unwrap();

        assert_eq!(page.entries[0].state, RunState::NeedsRepair);
        assert_eq!(page.scan_issue_count, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scanning_a_changed_header_never_overwrites_a_deleting_state() {
        let root = fixture_root();
        let header = crate::record::test_support::header();
        write_header(&root, &header);
        let mut connection = database();
        connection
            .execute(
                "INSERT INTO session_catalog(session_id,script_id,created_at,billing_run_id,state,revision,updated_at) VALUES(?1,?2,'2026-10-20T00:00:00Z',?1,'deleting',1,'2026-10-20T00:00:00Z')",
                rusqlite::params![header.session_id, header.script_id],
            )
            .unwrap();

        scan_all(&root, &mut connection);
        let page = page(&connection, None, DEFAULT_PAGE_SIZE).unwrap();

        assert_eq!(page.entries[0].state, RunState::Deleting);
        assert_eq!(page.scan_issue_count, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_new_scan_clears_stale_issues_and_marks_progress_incomplete() {
        let mut connection = database();
        record_issue(
            &connection,
            &Issue {
                key: "workspaces/removed/transcript/bad.jsonl".into(),
                script_id: None,
                reason: "invalid-header",
            },
        )
        .unwrap();
        connection
            .execute(
                "UPDATE session_catalog_meta SET scan_incomplete=0 WHERE id=1",
                [],
            )
            .unwrap();

        begin_scan(&mut connection).unwrap();
        let page = page(&connection, None, DEFAULT_PAGE_SIZE).unwrap();

        assert!(page.scan_incomplete);
        assert_eq!(page.scan_issue_count, 0);
    }

    #[test]
    fn issue_storage_has_a_fixed_bound() {
        let connection = database();
        for index in 0..(super::super::MAX_SCAN_ISSUES as usize + 10) {
            record_issue(
                &connection,
                &Issue {
                    key: format!("workspaces/{index}/bad.jsonl"),
                    script_id: None,
                    reason: "invalid-header",
                },
            )
            .unwrap();
        }

        let page = page(&connection, None, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(page.scan_issue_count, super::super::MAX_SCAN_ISSUES as u64);
    }

    #[cfg(unix)]
    #[test]
    fn candidate_replaced_by_symlink_is_rejected_before_header_read() {
        use std::os::unix::fs::symlink;

        let root = fixture_root();
        let header = crate::record::test_support::header();
        write_header(&root, &header);
        let mut scanner = LegacyScanner::new(&root);
        let batch = scanner.next_batch();
        let candidate = batch
            .iter()
            .find(|item| matches!(item, Item::Candidate(_)))
            .expect("single transcript is discovered");
        let Item::Candidate(candidate) = candidate else {
            unreachable!();
        };
        let external = fixture_root();
        fs::write(&external, b"external header must not be read\n").unwrap();
        fs::remove_file(&candidate.path).unwrap();
        symlink(&external, &candidate.path).unwrap();

        let PreparedBatch { items, .. } = prepare_batch(&[Item::Candidate(candidate.clone())]);
        assert!(matches!(
            items.as_slice(),
            [PreparedItem::Candidate(prepared)] if prepared.failure == Some("symlink")
        ));
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(external).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn transcript_parent_replaced_by_in_root_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = fixture_root();
        let header = crate::record::test_support::header();
        write_header(&root, &header);
        let mut scanner = LegacyScanner::new(&root);
        let batch = scanner.next_batch();
        let candidate = batch
            .iter()
            .find_map(|item| match item {
                Item::Candidate(candidate) => Some(candidate.clone()),
                Item::Issue(_) => None,
            })
            .expect("single transcript is discovered");
        let transcript = candidate.path.parent().unwrap().to_owned();
        let script_dir = transcript.parent().unwrap();
        let saved_transcript = script_dir.join("transcript-saved");
        let alternate = script_dir.join("alternate");
        fs::rename(&transcript, &saved_transcript).unwrap();
        fs::create_dir(&alternate).unwrap();
        fs::write(
            alternate.join(format!("{}.jsonl", header.session_id)),
            b"in-root target must not be read\n",
        )
        .unwrap();
        symlink(&alternate, &transcript).unwrap();

        let PreparedBatch { items, .. } = prepare_batch(&[Item::Candidate(candidate)]);
        assert!(matches!(
            items.as_slice(),
            [PreparedItem::Candidate(prepared)] if prepared.failure == Some("symlink")
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_limits_each_traversal_batch_and_resumes_until_complete() {
        let root = fixture_root();
        for _ in 0..130 {
            write_header(&root, &crate::record::test_support::header());
        }
        let mut connection = database();
        let results = scan_all(&root, &mut connection);
        let page = page(&connection, None, MAX_PAGE_SIZE).unwrap();

        assert!(results.len() >= 2);
        assert!(
            results[..results.len() - 1]
                .iter()
                .all(|result| result.scan_incomplete)
        );
        assert!(!results.last().unwrap().scan_incomplete);
        assert_eq!(page.entries.len(), 130);
        assert!(!page.scan_incomplete);
        fs::remove_dir_all(root).unwrap();
    }
}
