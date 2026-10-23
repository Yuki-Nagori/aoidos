//! 新周目 record 与 SQLite 目录之间的可恢复创建意图。

use super::*;
use crate::record::format::Header;
use aoidos_llm::config::LlmProfile;
use sha2::{Digest, Sha256};

pub const MIGRATION: &str = r#"
CREATE TABLE session_creation_ops (
  operation_id TEXT PRIMARY KEY CHECK(length(operation_id) = 36),
  session_id TEXT NOT NULL UNIQUE CHECK(length(session_id) = 36),
  script_id TEXT NOT NULL CHECK(length(script_id) BETWEEN 1 AND 64),
  header_snapshot BLOB NOT NULL CHECK(length(header_snapshot) BETWEEN 2 AND 2097152),
  header_hash TEXT NOT NULL CHECK(length(header_hash) = 64),
  profile_snapshot BLOB NOT NULL CHECK(length(profile_snapshot) BETWEEN 2 AND 65536),
  profile_hash TEXT NOT NULL CHECK(length(profile_hash) = 64),
  state TEXT NOT NULL CHECK(state IN ('intent','record-created','registered','needs-repair')),
  created_at TEXT NOT NULL CHECK(length(created_at) BETWEEN 1 AND 64),
  updated_at TEXT NOT NULL CHECK(length(updated_at) BETWEEN 1 AND 64)
);
CREATE INDEX session_creation_pending ON session_creation_ops(state, created_at, operation_id)
  WHERE state IN ('intent','record-created');
"#;

#[derive(Debug, Clone)]
pub struct InactiveRunIntent {
    pub operation_id: String,
    pub header: Header,
    pub profile: LlmProfile,
}

#[derive(Debug, Clone)]
pub struct InactiveRunCreation {
    pub operation_id: String,
    pub header: Header,
    pub profile: LlmProfile,
    pub state: String,
}

struct StoredCreation {
    script_id: String,
    header_snapshot: Vec<u8>,
    header_hash: String,
    profile_snapshot: Vec<u8>,
    profile_hash: String,
    state: String,
    created_at: String,
}

struct PendingCreation {
    operation_id: String,
    session_id: String,
    script_id: String,
    header_snapshot: Vec<u8>,
    header_hash: String,
    profile_snapshot: Vec<u8>,
    profile_hash: String,
    state: String,
    created_at: String,
}

/// 先登记不可变 intent，再允许写 transcript，重试只能重放相同身份与快照。
pub(crate) fn begin_inactive_creation(
    connection: &mut rusqlite::Connection,
    intent: &InactiveRunIntent,
) -> Result<(), Fault> {
    validate_intent(intent)?;
    let header = aoidos_json::to_vec(&intent.header).map_err(|_| Fault::bad_request())?;
    let profile = aoidos_json::to_vec(&intent.profile).map_err(|_| Fault::bad_request())?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    if let Some(existing) = transaction
        .query_row(
            "SELECT operation_id,script_id,header_snapshot,header_hash,profile_snapshot,profile_hash FROM session_creation_ops WHERE session_id=?1",
            [&intent.header.session_id],
            |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?,row.get::<_,String>(3)?,row.get::<_,Vec<u8>>(4)?,row.get::<_,String>(5)?)),
        )
        .optional()
        .map_err(database)?
    {
        if existing.0 == intent.operation_id
            && existing.1 == intent.header.script_id
            && existing.2 == header
            && existing.3 == hash(&header)
            && existing.4 == profile
            && existing.5 == hash(&profile)
        {
            return transaction.commit().map_err(database);
        }
        return Err(Fault::new("app.conflict", "周目创建身份已被占用"));
    }
    let catalog_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session_catalog WHERE session_id=?1)",
            [&intent.header.session_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    if catalog_exists {
        return Err(Fault::new("app.conflict", "周目已存在"));
    }
    transaction
        .execute(
            "INSERT INTO session_creation_ops(operation_id,session_id,script_id,header_snapshot,header_hash,profile_snapshot,profile_hash,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'intent',?8,?8)",
            rusqlite::params![intent.operation_id, intent.header.session_id, intent.header.script_id, header, hash(&header), profile, hash(&profile), intent.header.created_at],
        )
        .map_err(database)?;
    transaction.commit().map_err(database)
}

pub(crate) fn mark_inactive_record_created(
    connection: &mut rusqlite::Connection,
    session_id: &str,
    header: &Header,
) -> Result<(), Fault> {
    let bytes = aoidos_json::to_vec(header).map_err(|_| Fault::bad_request())?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let row: Option<(Vec<u8>, String)> = transaction
        .query_row(
            "SELECT header_snapshot,state FROM session_creation_ops WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((snapshot, state)) = row else {
        return Err(Fault::new("app.not-found", "周目创建意图不存在"));
    };
    if snapshot != bytes || header.session_id != session_id {
        return Err(corrupt());
    }
    match state.as_str() {
        "intent" => {
            transaction
                .execute(
                    "UPDATE session_creation_ops SET state='record-created',updated_at=?1 WHERE session_id=?2 AND state='intent'",
                    rusqlite::params![header.created_at, session_id],
                )
                .map_err(database)?;
        }
        "record-created" | "registered" => {}
        _ => return Err(Fault::new("store.needs-repair", "周目创建意图需要修复")),
    }
    transaction.commit().map_err(database)
}

pub(crate) fn complete_inactive_creation(
    connection: &mut rusqlite::Connection,
    session_id: &str,
) -> Result<(), Fault> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(database)?;
    let row: Option<StoredCreation> = transaction
        .query_row(
            "SELECT script_id,header_snapshot,header_hash,profile_snapshot,profile_hash,state,created_at FROM session_creation_ops WHERE session_id=?1",
            [session_id],
            |row| {
                Ok(StoredCreation {
                    script_id: row.get(0)?,
                    header_snapshot: row.get(1)?,
                    header_hash: row.get(2)?,
                    profile_snapshot: row.get(3)?,
                    profile_hash: row.get(4)?,
                    state: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(database)?;
    let Some(row) = row else {
        return Err(Fault::new("app.not-found", "周目创建意图不存在"));
    };
    if row.state == "registered" {
        return transaction.commit().map_err(database);
    }
    if row.state != "record-created"
        || hash(&row.header_snapshot) != row.header_hash
        || hash(&row.profile_snapshot) != row.profile_hash
    {
        return Err(corrupt());
    }
    let header: Header = decode(&row.header_snapshot)?;
    let profile: LlmProfile = decode(&row.profile_snapshot)?;
    validate_intent(&InactiveRunIntent {
        operation_id: "00000000-0000-4000-8000-000000000000".into(),
        header: header.clone(),
        profile: profile.clone(),
    })?;
    if header.session_id != session_id
        || header.script_id != row.script_id
        || header.created_at != row.created_at
    {
        return Err(corrupt());
    }
    bump_revision(&transaction)?;
    transaction
        .execute(
            "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,profile_id,profile_schema_version,profile_snapshot,state,revision,updated_at) VALUES(?1,?2,?3,NULL,NULL,?4,1,?5,'recovery-required',1,?3)",
            rusqlite::params![session_id, row.script_id, row.created_at, profile.profile_id, row.profile_snapshot],
        )
        .map_err(database)?;
    transaction
        .execute(
            "UPDATE session_creation_ops SET state='registered',updated_at=?1 WHERE session_id=?2 AND state='record-created'",
            rusqlite::params![row.created_at, session_id],
        )
        .map_err(database)?;
    transaction
        .commit()
        .map_err(|_| Fault::new("store.commit-unknown", "周目创建登记结果不确定"))
}

pub(crate) fn pending_inactive_creations(
    connection: &rusqlite::Connection,
) -> Result<Vec<InactiveRunCreation>, Fault> {
    let mut statement = connection
        .prepare("SELECT operation_id,session_id,script_id,header_snapshot,header_hash,profile_snapshot,profile_hash,state,created_at FROM session_creation_ops WHERE state IN ('intent','record-created') ORDER BY created_at,operation_id")
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok(PendingCreation {
                operation_id: row.get(0)?,
                session_id: row.get(1)?,
                script_id: row.get(2)?,
                header_snapshot: row.get(3)?,
                header_hash: row.get(4)?,
                profile_snapshot: row.get(5)?,
                profile_hash: row.get(6)?,
                state: row.get(7)?,
                created_at: row.get(8)?,
            })
        })
        .map_err(database)?;
    let mut pending = Vec::new();
    for row in rows {
        let row = row.map_err(database)?;
        if hash(&row.header_snapshot) != row.header_hash
            || hash(&row.profile_snapshot) != row.profile_hash
        {
            return Err(corrupt());
        }
        let header: Header = decode(&row.header_snapshot)?;
        let profile: LlmProfile = decode(&row.profile_snapshot)?;
        validate_intent(&InactiveRunIntent {
            operation_id: row.operation_id.clone(),
            header: header.clone(),
            profile: profile.clone(),
        })?;
        if header.session_id != row.session_id
            || header.script_id != row.script_id
            || header.created_at != row.created_at
            || !matches!(row.state.as_str(), "intent" | "record-created")
        {
            return Err(corrupt());
        }
        pending.push(InactiveRunCreation {
            operation_id: row.operation_id,
            header,
            profile,
            state: row.state,
        });
    }
    Ok(pending)
}

fn validate_intent(intent: &InactiveRunIntent) -> Result<(), Fault> {
    if !crate::record::format::valid_uuid(&intent.operation_id) {
        return Err(Fault::bad_request());
    }
    intent.header.validate().map_err(|_| Fault::bad_request())?;
    intent.profile.validate().map_err(|_| Fault::bad_request())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Fault> {
    let text = std::str::from_utf8(bytes).map_err(|_| corrupt())?;
    let value = aoidos_json::parse(text, 2_097_152).map_err(|_| corrupt())?;
    aoidos_json::from_value(value).map_err(|_| corrupt())
}
