//! 新周目 record 与 SQLite 目录之间的可恢复创建意图。

use super::*;
use crate::record::format::Header;
use aoidos_billing::money::NanoMoney;
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

/// Optional cycle preparation is persisted separately so databases created before this slice
/// retain their exact legacy journal semantics.
pub const PREPARATION_MIGRATION: &str = r#"
CREATE TABLE session_creation_preparations (
  operation_id TEXT PRIMARY KEY REFERENCES session_creation_ops(operation_id) ON DELETE CASCADE,
  request_id TEXT NOT NULL UNIQUE CHECK(length(request_id) = 36),
  snapshot BLOB NOT NULL CHECK(length(snapshot) BETWEEN 2 AND 1048576),
  snapshot_hash TEXT NOT NULL CHECK(length(snapshot_hash) = 64)
);
"#;

#[derive(Debug, Clone)]
pub struct InactiveRunIntent {
    pub operation_id: String,
    pub header: Header,
    pub profile: LlmProfile,
    pub preparation: Option<InitializationPreparation>,
}

#[derive(Debug, Clone)]
pub struct InactiveRunCreation {
    pub operation_id: String,
    pub header: Header,
    pub profile: LlmProfile,
    pub preparation: Option<InitializationPreparation>,
    pub state: String,
}

/// Exact provider input and confirmed per-currency limits frozen before writing the record header.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializationPreparation {
    pub version: u32,
    pub request_id: String,
    pub input: FrozenInitializationInput,
    pub cny_limit: NanoMoney,
    pub usd_limit: NanoMoney,
}

/// A validated immutable opening request that is eligible for its first dispatch.
/// Loading this value does not claim dispatch rights; the billing port must still
/// atomically claim the attempt and reserve its budget before calling a Provider.
#[derive(Debug, Clone)]
pub struct InitializationDispatchCandidate {
    pub session_id: String,
    pub revision: u64,
    pub request_id: String,
    pub profile: LlmProfile,
    pub input: FrozenInitializationInput,
    pub provider_request: aoidos_llm::provider::ProviderRequest,
}

/// Versioned request envelope. `provider_request` is the canonical serialized request body;
/// `script_revision` and `profile_snapshot_hash` bind it to the immutable run inputs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenInitializationInput {
    pub version: u32,
    pub operation_id: String,
    pub session_id: String,
    pub script_id: String,
    pub script_revision: String,
    pub profile_id: String,
    pub profile_snapshot_hash: String,
    pub locale: String,
    pub prompt_version: String,
    pub output_schema_version: u32,
    pub script_snapshot: Vec<u8>,
    pub entity_snapshot: Vec<u8>,
    pub cycle_input_snapshot: Vec<u8>,
    /// Canonical request captured for durable preparation. Creation re-derives it from the
    /// frozen snapshots and rejects any mismatch; dispatch still requires the coordinator.
    pub provider_request: serde_json::Value,
}

struct StoredCreation {
    operation_id: String,
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

type DispatchCreationRow = (String, String, Vec<u8>, String, Vec<u8>, String, String);
type DispatchCatalogRow = (
    String,
    Option<String>,
    Option<i64>,
    Option<Vec<u8>>,
    Option<String>,
    Option<String>,
);

/// 先登记不可变 intent，再允许写 transcript，重试只能重放相同身份与快照。
pub(crate) fn begin_inactive_creation(
    connection: &mut rusqlite::Connection,
    intent: &InactiveRunIntent,
) -> Result<(), Fault> {
    validate_intent(intent)?;
    let header = aoidos_json::to_vec(&intent.header).map_err(|_| Fault::bad_request())?;
    let profile = aoidos_json::to_vec(&intent.profile).map_err(|_| Fault::bad_request())?;
    let preparation = intent
        .preparation
        .as_ref()
        .map(aoidos_json::to_vec)
        .transpose()
        .map_err(|_| Fault::bad_request())?;
    if preparation
        .as_ref()
        .is_some_and(|bytes| bytes.len() > 1_048_576)
    {
        return Err(Fault::bad_request());
    }
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
        let stored_preparation = load_preparation(&transaction, &intent.operation_id)?;
        if existing.0 == intent.operation_id
            && existing.1 == intent.header.script_id
            && existing.2 == header
            && existing.3 == hash(&header)
            && existing.4 == profile
            && existing.5 == hash(&profile)
            && stored_preparation
                .as_ref()
                .map(|stored| stored.0.as_slice())
                == preparation.as_deref()
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
    if let (Some(preparation), Some(preparation_bytes)) =
        (intent.preparation.as_ref(), preparation.as_ref())
    {
        transaction
            .execute(
                "INSERT INTO session_creation_preparations(operation_id,request_id,snapshot,snapshot_hash) VALUES(?1,?2,?3,?4)",
                rusqlite::params![intent.operation_id, preparation.request_id, preparation_bytes, hash(preparation_bytes)],
            )
            .map_err(database)?;
    }
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
            "SELECT operation_id,script_id,header_snapshot,header_hash,profile_snapshot,profile_hash,state,created_at FROM session_creation_ops WHERE session_id=?1",
            [session_id],
            |row| {
                Ok(StoredCreation {
                    operation_id: row.get(0)?,
                    script_id: row.get(1)?,
                    header_snapshot: row.get(2)?,
                    header_hash: row.get(3)?,
                    profile_snapshot: row.get(4)?,
                    profile_hash: row.get(5)?,
                    state: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(database)?;
    let Some(row) = row else {
        return Err(Fault::new("app.not-found", "周目创建意图不存在"));
    };
    let preparation = load_preparation(&transaction, &row.operation_id)?;
    if row.state == "registered" {
        if let Some((snapshot, snapshot_hash, preparation)) = preparation {
            if hash(&snapshot) != snapshot_hash {
                return Err(corrupt());
            }
            let input_snapshot = aoidos_json::to_vec(&preparation.input).map_err(|_| corrupt())?;
            verify_registered_preparation(&transaction, session_id, &preparation, &input_snapshot)?;
        }
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
        operation_id: row.operation_id.clone(),
        header: header.clone(),
        profile: profile.clone(),
        preparation: preparation.as_ref().map(|(_, _, value)| value.clone()),
    })?;
    if header.session_id != session_id
        || header.script_id != row.script_id
        || header.created_at != row.created_at
    {
        return Err(corrupt());
    }
    let preparation = preparation
        .map(|(snapshot, snapshot_hash, preparation)| {
            if hash(&snapshot) != snapshot_hash {
                return Err(corrupt());
            }
            Ok((snapshot, preparation))
        })
        .transpose()?;
    bump_revision(&transaction)?;
    transaction
        .execute(
            "INSERT INTO session_catalog(session_id,script_id,created_at,memory_run_id,billing_run_id,profile_id,profile_schema_version,profile_snapshot,state,revision,updated_at) VALUES(?1,?2,?3,NULL,?6,?4,1,?5,'recovery-required',1,?3)",
            rusqlite::params![session_id, row.script_id, row.created_at, profile.profile_id, row.profile_snapshot, preparation.as_ref().map(|_| session_id)],
        )
        .map_err(database)?;
    if let Some((_snapshot, preparation)) = preparation {
        let timestamp = row.created_at.as_str();
        crate::billing::BillingLedger::initialize_settings_in_transaction(
            &transaction,
            session_id,
            preparation.cny_limit,
            preparation.usd_limit,
            timestamp,
        )
        .map_err(|_| Fault::new("store.database", "初始化预算写入失败"))?;
        crate::cycle_initialization::prepare_in_transaction(
            &transaction,
            session_id,
            &preparation.request_id,
            &aoidos_json::to_vec(&preparation.input).map_err(|_| corrupt())?,
            timestamp,
        )?;
    }
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

pub(crate) fn initialization_dispatch_candidate(
    connection: &rusqlite::Connection,
    session_id: &str,
) -> Result<InitializationDispatchCandidate, Fault> {
    if !crate::record::format::valid_uuid(session_id) {
        return Err(Fault::bad_request());
    }
    let creation: Option<DispatchCreationRow> = connection
        .query_row(
            "SELECT operation_id,script_id,header_snapshot,header_hash,profile_snapshot,profile_hash,state FROM session_creation_ops WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((
        operation_id,
        script_id,
        header_bytes,
        header_hash,
        profile_bytes,
        profile_hash,
        creation_state,
    )) = creation
    else {
        return Err(Fault::new("app.not-found", "周目创建意图不存在"));
    };
    match creation_state.as_str() {
        "registered" => {}
        "intent" | "record-created" => {
            return Err(Fault::new("app.conflict", "周目创建尚未登记完成"));
        }
        "needs-repair" => {
            return Err(Fault::new("store.needs-repair", "周目创建意图需要修复"));
        }
        _ => return Err(corrupt()),
    }
    if header_bytes.len() > 2_097_152
        || profile_bytes.len() > 65_536
        || hash(&header_bytes) != header_hash
        || hash(&profile_bytes) != profile_hash
    {
        return Err(corrupt());
    }
    let header: Header = decode(&header_bytes)?;
    let profile: LlmProfile = decode(&profile_bytes)?;
    if header.session_id != session_id || header.script_id != script_id {
        return Err(corrupt());
    }
    let Some((preparation_bytes, preparation_hash, preparation)) =
        load_preparation(connection, &operation_id)?
    else {
        return Err(Fault::new("app.conflict", "该周目没有待派发的开局请求"));
    };
    validate_intent(&InactiveRunIntent {
        operation_id,
        header,
        profile: profile.clone(),
        preparation: Some(preparation.clone()),
    })
    .map_err(|_| corrupt())?;

    let catalog: Option<DispatchCatalogRow> = connection
        .query_row(
            "SELECT state,profile_id,profile_schema_version,profile_snapshot,billing_run_id,memory_run_id FROM session_catalog WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((state, profile_id, schema_version, snapshot, billing_run_id, memory_run_id)) =
        catalog
    else {
        return Err(corrupt());
    };
    if state != "recovery-required" {
        return Err(Fault::new(
            "app.conflict",
            "周目状态已变化，不能派发开局请求",
        ));
    }
    if profile_id.as_deref() != Some(profile.profile_id.as_str())
        || schema_version != Some(1)
        || snapshot.as_deref() != Some(profile_bytes.as_slice())
        || billing_run_id.as_deref() != Some(session_id)
        || memory_run_id.is_some()
    {
        return Err(corrupt());
    }

    let initialization = crate::cycle_initialization::load(connection, session_id)?;
    if initialization.state != crate::cycle_initialization::State::Initializing {
        return Err(Fault::new("app.conflict", "开局请求已变化或不再等待派发"));
    }
    if initialization.input_snapshot
        != aoidos_json::to_vec(&preparation.input).map_err(|_| corrupt())?
        || initialization.input_hash != hash(&initialization.input_snapshot)
    {
        return Err(corrupt());
    }
    let attempt: Option<(String, i64)> = connection
        .query_row(
            "SELECT state,dispatch_started FROM cycle_initialization_attempts WHERE request_id=?1 AND session_id=?2",
            rusqlite::params![initialization.request_id, session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    if attempt
        .as_ref()
        .is_none_or(|(state, dispatch_started)| state != "pending" || *dispatch_started != 0)
    {
        return Err(Fault::new("app.conflict", "开局请求已领取或状态已变化"));
    }
    let budget: Option<(i64, i64)> = connection
        .query_row(
            "SELECT cny_limit_nanos,usd_limit_nanos FROM billing_run_budgets WHERE run_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    if budget != Some((preparation.cny_limit.nanos(), preparation.usd_limit.nanos())) {
        return Err(Fault::new(
            "app.conflict",
            "周目预算已变化，不能派发开局请求",
        ));
    }
    if hash(&preparation_bytes) != preparation_hash {
        return Err(corrupt());
    }
    let provider_request = aoidos_json::from_value(preparation.input.provider_request.clone())
        .map_err(|_| corrupt())?;
    Ok(InitializationDispatchCandidate {
        session_id: session_id.to_owned(),
        revision: initialization.revision,
        request_id: initialization.request_id,
        profile,
        input: preparation.input,
        provider_request,
    })
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
            preparation: load_preparation(connection, &row.operation_id)?
                .map(|(_, _, preparation)| preparation),
        })?;
        if header.session_id != row.session_id
            || header.script_id != row.script_id
            || header.created_at != row.created_at
            || !matches!(row.state.as_str(), "intent" | "record-created")
        {
            return Err(corrupt());
        }
        pending.push(InactiveRunCreation {
            operation_id: row.operation_id.clone(),
            header,
            profile,
            preparation: load_preparation(connection, &row.operation_id)?
                .map(|(_, _, preparation)| preparation),
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
    intent
        .profile
        .validate()
        .map_err(|_| Fault::bad_request())?;
    if let Some(preparation) = &intent.preparation {
        if preparation.version != 1
            || !crate::record::format::valid_uuid(&preparation.request_id)
            || preparation.input.version != 1
            || preparation.input.operation_id != intent.operation_id
            || preparation.input.session_id != intent.header.session_id
            || preparation.input.script_id != intent.header.script_id
            || preparation.input.script_revision != intent.header.script_revision
            || preparation.input.profile_id != intent.profile.profile_id
            || preparation.input.profile_snapshot_hash
                != hash(&aoidos_json::to_vec(&intent.profile).map_err(|_| Fault::bad_request())?)
            || !crate::record::format::valid_hash(&preparation.input.script_revision)
            || !matches!(preparation.input.locale.as_str(), "zh-Hans" | "en")
            || preparation.input.prompt_version != "init-v1"
            || preparation.input.output_schema_version != 2
            || validate_provider_request(
                &preparation.input.provider_request,
                &intent.profile,
                &intent.header.session_id,
            )
            .is_err()
        {
            return Err(Fault::bad_request());
        }
        let input = aoidos_json::to_vec(&preparation.input).map_err(|_| Fault::bad_request())?;
        if input.len() > 1_048_576
            || preparation.input.script_snapshot.len() > 1_048_576
            || preparation.input.entity_snapshot.len() > 1_048_576
            || preparation.input.cycle_input_snapshot.len() > 128 * 1024
        {
            return Err(Fault::bad_request());
        }
        crate::game::assets::validate_frozen_initialization_assets(
            &preparation.input.script_id,
            &preparation.input.script_revision,
            &preparation.input.script_snapshot,
            &preparation.input.entity_snapshot,
            &preparation.input.cycle_input_snapshot,
        )
        .map_err(|_| Fault::bad_request())?;
        let canonical_request =
            crate::game::initialization_prompt::build_request(&preparation.input, &intent.profile)
                .map_err(|_| Fault::bad_request())?;
        let canonical_value =
            serde_json::to_value(canonical_request).map_err(|_| Fault::bad_request())?;
        if preparation.input.provider_request != canonical_value {
            return Err(Fault::bad_request());
        }
    }
    Ok(())
}

fn validate_provider_request(
    value: &serde_json::Value,
    profile: &LlmProfile,
    session_id: &str,
) -> Result<(), Fault> {
    use aoidos_llm::provider::{ProviderInput, ProviderRequest, RequestMode};

    let sampling = value
        .get("sampling")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(Fault::bad_request)?;
    if sampling.len() != 2
        || !sampling.contains_key("temperature")
        || !sampling.contains_key("maxTokens")
    {
        return Err(Fault::bad_request());
    }
    let request: ProviderRequest =
        aoidos_json::from_value(value.clone()).map_err(|_| Fault::bad_request())?;
    if request.model != profile.model
        || request.mode() != RequestMode::from(profile.mode)
        || request.sampling != profile.sampling
        || request
            .session_id
            .as_deref()
            .is_some_and(|request_session_id| request_session_id != session_id)
        || request.stops.len() > 64
        || request
            .stops
            .iter()
            .any(|stop| stop.is_empty() || stop.len() > 256)
    {
        return Err(Fault::bad_request());
    }
    let input_bytes = match &request.input {
        ProviderInput::Completion(input) if !input.prompt.trim().is_empty() => input.prompt.len(),
        ProviderInput::Chat(input) if !input.messages.is_empty() && input.messages.len() <= 128 => {
            let mut bytes = 0usize;
            for message in &input.messages {
                if message.content.trim().is_empty() {
                    return Err(Fault::bad_request());
                }
                bytes = bytes
                    .checked_add(message.content.len())
                    .ok_or_else(Fault::bad_request)?;
                if bytes > 256 * 1024 {
                    return Err(Fault::bad_request());
                }
            }
            if let Some(prefix) = &input.assistant_prefix {
                if prefix.trim().is_empty() {
                    return Err(Fault::bad_request());
                }
                bytes = bytes
                    .checked_add(prefix.len())
                    .ok_or_else(Fault::bad_request)?;
            }
            bytes
        }
        _ => return Err(Fault::bad_request()),
    };
    if input_bytes > 256 * 1024
        || serde_json::to_vec(&request.input)
            .map_err(|_| Fault::bad_request())?
            .len()
            > 256 * 1024
    {
        return Err(Fault::bad_request());
    }
    Ok(())
}

fn load_preparation(
    connection: &rusqlite::Connection,
    operation_id: &str,
) -> Result<Option<(Vec<u8>, String, InitializationPreparation)>, Fault> {
    let stored: Option<(String, Vec<u8>, String)> = connection
        .query_row(
            "SELECT request_id,snapshot,snapshot_hash FROM session_creation_preparations WHERE operation_id=?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(database)?;
    let Some((request_id, snapshot, snapshot_hash)) = stored else {
        return Ok(None);
    };
    if snapshot.len() > 1_048_576 || hash(&snapshot) != snapshot_hash {
        return Err(corrupt());
    }
    let preparation: InitializationPreparation = decode(&snapshot)?;
    if preparation.request_id != request_id {
        return Err(corrupt());
    }
    Ok(Some((snapshot, snapshot_hash, preparation)))
}

fn verify_registered_preparation(
    transaction: &rusqlite::Transaction<'_>,
    session_id: &str,
    preparation: &InitializationPreparation,
    snapshot: &[u8],
) -> Result<(), Fault> {
    let initialization: Option<(String, String, Vec<u8>, String)> = transaction
        .query_row(
            "SELECT state,current_request_id,input_snapshot,input_hash FROM cycle_initializations WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(database)?;
    let budget: Option<(i64, i64)> = transaction
        .query_row(
            "SELECT cny_limit_nanos,usd_limit_nanos FROM billing_run_budgets WHERE run_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database)?;
    let dispatch_started: Option<i64> = transaction
        .query_row(
            "SELECT dispatch_started FROM cycle_initialization_attempts WHERE request_id=?1 AND session_id=?2",
            rusqlite::params![preparation.request_id, session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?;
    if !initialization.as_ref().is_some_and(|value| {
        value.0 == "initializing"
            && value.1 == preparation.request_id
            && value.2 == snapshot
            && value.3 == hash(snapshot)
    }) || budget != Some((preparation.cny_limit.nanos(), preparation.usd_limit.nanos()))
        || dispatch_started != Some(0)
    {
        return Err(corrupt());
    }
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Fault> {
    let text = std::str::from_utf8(bytes).map_err(|_| corrupt())?;
    let value = aoidos_json::parse(text, 2_097_152).map_err(|_| corrupt())?;
    aoidos_json::from_value(value).map_err(|_| corrupt())
}
