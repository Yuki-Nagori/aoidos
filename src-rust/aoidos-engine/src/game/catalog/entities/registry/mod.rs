//! 动态身份只随可信世界 mutation 原子提交；目录从当前有效因果路径重建。

use super::{
    CanonicalEntity, EntityCreationRule, EntityDirectory, EntityKind, MAX_ENTITIES, MAX_NAME_BYTES,
    MAX_SCOPE_BYTES, valid_label,
};
use crate::{
    fault::Fault,
    game::catalog::SceneCatalog,
    record::{
        facts::{Fact, MutationData, WorldMutation},
        format::{self, MAX_SEQ},
        session::Session,
        world::mutation_hash,
    },
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
const MAX_DYNAMIC_ENTITIES: usize = MAX_ENTITIES;
const ENTITY_REGISTRY_SCHEMA: &str = r#"
CREATE TABLE entity_registry_runs (
 session_id TEXT PRIMARY KEY CHECK(length(session_id)=36),
 script_id TEXT NOT NULL CHECK(length(script_id) BETWEEN 1 AND 64),
 script_revision TEXT NOT NULL CHECK(length(script_revision)=71),
 seed_revision INTEGER NOT NULL CHECK(seed_revision BETWEEN 1 AND 9007199254740991),
 entity_revision INTEGER NOT NULL CHECK(entity_revision BETWEEN seed_revision AND 9007199254740991)
);
CREATE TABLE entity_registry_entries (
 session_id TEXT NOT NULL REFERENCES entity_registry_runs(session_id),
 entity_id TEXT NOT NULL CHECK(length(entity_id) BETWEEN 1 AND 64),
 kind TEXT NOT NULL CHECK(kind IN ('character','place','organization','item','term')),
 scope TEXT NOT NULL CHECK(length(scope) BETWEEN 1 AND 128),
 canonical_name TEXT NOT NULL CHECK(length(canonical_name) BETWEEN 1 AND 256),
 rule_id TEXT NOT NULL CHECK(length(rule_id) BETWEEN 1 AND 64),
 catalog_revision TEXT NOT NULL CHECK(length(catalog_revision) BETWEEN 1 AND 256),
 mutation_id TEXT NOT NULL UNIQUE REFERENCES store_applied(id),
 mutation_hash TEXT NOT NULL CHECK(length(mutation_hash)=71),
 record_seq INTEGER NOT NULL CHECK(record_seq BETWEEN 1 AND 9007199254740991),
 entity_revision INTEGER NOT NULL CHECK(entity_revision BETWEEN 2 AND 9007199254740991),
 PRIMARY KEY(session_id,entity_id),
 UNIQUE(session_id,entity_revision)
);
"#;

/// 串联固定 seed、周目持久化和当前有效历史路径的只读实体快照。
#[derive(Debug, Clone, Copy, Default)]
pub struct EntityRegistry;

/// Immutable source identity supplied when the trusted world operation is applied or replayed.
pub struct EntityCreationCommit<'a> {
    pub record_seq: u64,
    pub mutation: &'a WorldMutation,
    pub content_hash: &'a str,
}

/// 由可信场景目录许可生成的一次动态身份创建意图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityCreationIntent {
    pub version: u32,
    pub session_id: String,
    pub script_revision: String,
    pub catalog_revision: String,
    pub rule_id: String,
    pub entity_id: String,
    pub kind: EntityKind,
    pub scope: String,
    pub canonical_name: String,
}

#[derive(Debug, Clone)]
struct StoredEntity {
    entity: CanonicalEntity,
    rule_id: String,
    catalog_revision: String,
    mutation_id: String,
    mutation_hash: String,
    record_seq: u64,
    revision: u64,
}

#[derive(Debug)]
struct RegistryRows {
    revision: u64,
    seed_revision: u64,
    all: Vec<StoredEntity>,
    visible: Vec<StoredEntity>,
    path: BTreeSet<u64>,
}

impl EntityRegistry {
    /// 与场景目录许可绑定地生成一组稳定、不可由名称推导的身份和 mutation ID。
    /// # Errors
    /// 规则与快照不匹配、名称重复、快照不完整或输入超限时拒绝。
    pub(super) fn prepare(
        rule: &EntityCreationRule,
        catalog_revision: &str,
        session_id: &str,
        script_revision: &str,
        canonical_name: &str,
        directory: &EntityDirectory,
    ) -> Result<WorldMutation, Fault> {
        if !format::valid_uuid(session_id)
            || !format::valid_hash(script_revision)
            || !format::valid_id(&rule.rule_id)
            || catalog_revision.is_empty()
            || catalog_revision.len() > 256
            || !directory.is_complete()
            || directory.script_revision() != script_revision
            || !valid_label(canonical_name, MAX_NAME_BYTES)
        {
            return Err(Fault::bad_request());
        }
        if directory
            .resolve_exact(rule.kind, &rule.scope, canonical_name)?
            .is_some()
        {
            return Err(Fault::bad_request());
        }
        let entity_id = dynamic_id();
        let intent = EntityCreationIntent {
            version: 1,
            session_id: session_id.into(),
            script_revision: script_revision.into(),
            catalog_revision: catalog_revision.into(),
            rule_id: rule.rule_id.clone(),
            entity_id,
            kind: rule.kind,
            scope: rule.scope.clone(),
            canonical_name: canonical_name.into(),
        };
        Ok(WorldMutation {
            mutation_id: uuid::Uuid::new_v4().to_string(),
            data: MutationData {
                version: 1,
                kind: "entityCreated".into(),
                payload: serde_json::json!({
                    "version": intent.version,
                    "sessionId": intent.session_id,
                    "scriptRevision": intent.script_revision,
                    "catalogRevision": intent.catalog_revision,
                    "ruleId": intent.rule_id,
                    "entityId": intent.entity_id,
                    "kind": intent.kind,
                    "scope": intent.scope,
                    "canonicalName": intent.canonical_name,
                }),
            },
        })
    }

    /// Merges the static seed with committed dynamic identities visible on this session's path.
    /// # Errors
    /// A mismatched session, pending recovery, corrupt receipt, stale script, or oversized registry is rejected.
    pub fn snapshot(
        &self,
        connection: &mut Connection,
        session: &Session,
        seed: &EntityDirectory,
    ) -> Result<EntityDirectory, Fault> {
        if session.needs_recovery() {
            return Err(invalid_phase());
        }
        ensure_run(connection, session, seed)?;
        let rows = read_rows(connection, session, seed)?;
        seed.with_dynamic(
            rows.revision,
            rows.visible
                .into_iter()
                .map(|stored| stored.entity)
                .collect(),
        )
    }

    /// Revalidates a complete planned group against a fresh snapshot before the record is appended.
    /// # Errors
    /// Unknown rules, identity conflicts, cross-session reuse, inactive provenance, or exhausted bounds are rejected.
    pub fn validate_group(
        &self,
        connection: &mut Connection,
        session: &Session,
        seed: &EntityDirectory,
        catalog: &SceneCatalog,
        mutations: &[WorldMutation],
    ) -> Result<(), Fault> {
        ensure_run(connection, session, seed)?;
        let rows = read_rows(connection, session, seed)?;
        validate_group_rows(&rows, session, seed, catalog, mutations)
    }

    /// Atomically persists one registered identity, advances the entity revision and writes its applied receipt.
    /// # Errors
    /// If the intent no longer belongs to the active history or any CAS / SQL check fails, no partial state is confirmed.
    pub fn apply(
        &self,
        connection: &mut Connection,
        session: &Session,
        seed: &EntityDirectory,
        catalog: &SceneCatalog,
        commit: EntityCreationCommit<'_>,
    ) -> Result<(), Fault> {
        let EntityCreationCommit {
            record_seq,
            mutation,
            content_hash,
        } = commit;
        ensure_run(connection, session, seed)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let rows = read_rows(&transaction, session, seed)?;
        validate_group_rows(
            &rows,
            session,
            seed,
            catalog,
            std::slice::from_ref(mutation),
        )?;
        let intent = decode_intent(mutation)?;
        if mutation_hash(mutation)? != content_hash
            || intent.session_id != session.header().session_id
            || !rows.path.contains(&record_seq)
            || !record_contains_mutation(session, record_seq, mutation, content_hash)?
        {
            return Err(invalid_phase());
        }
        if aoidos_store::applied::contains(&transaction, &mutation.mutation_id, content_hash)
            .map_err(Fault::from)?
        {
            // These rows were audited before the transaction wrote anything.
            if !rows
                .all
                .iter()
                .any(|stored| stored.mutation_id == mutation.mutation_id)
            {
                return Err(corrupt());
            }
            transaction.commit().map_err(sql)?;
            return Ok(());
        }
        // Group validation bounds both the row count and the JS-safe revision in this transaction.
        let next_revision = rows.revision + 1;
        let updated = transaction
            .execute(
                "UPDATE entity_registry_runs SET entity_revision=?1 WHERE session_id=?2 AND entity_revision=?3",
                params![next_revision as i64, session.header().session_id, rows.revision as i64],
            )
            .map_err(sql)?;
        if updated != 1 {
            return Err(corrupt());
        }
        // The entry has an immediate foreign key to this receipt, so write the receipt first.
        transaction
            .execute(
                "INSERT INTO store_applied(id,content_hash) VALUES(?1,?2)",
                params![mutation.mutation_id, content_hash],
            )
            .map_err(sql)?;
        transaction
            .execute(
                "INSERT INTO entity_registry_entries(session_id,entity_id,kind,scope,canonical_name,rule_id,catalog_revision,mutation_id,mutation_hash,record_seq,entity_revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    session.header().session_id,
                    intent.entity_id,
                    kind_name(intent.kind),
                    intent.scope,
                    intent.canonical_name,
                    intent.rule_id,
                    intent.catalog_revision,
                    mutation.mutation_id,
                    content_hash,
                    record_seq as i64,
                    next_revision as i64,
                ],
            )
            .map_err(sql)?;
        transaction.commit().map_err(sql)?;
        Ok(())
    }
}

impl SceneCatalog {
    /// Prepare a dynamic identity using a rule already owned by this catalog.
    /// # Errors
    /// Unknown rule or incompatible snapshot is rejected; generated identifiers never depend on names.
    pub fn prepare_entity_creation_for_session(
        &self,
        session_id: &str,
        rule_id: &str,
        canonical_name: &str,
        directory: &EntityDirectory,
    ) -> Result<WorldMutation, Fault> {
        if directory.script_revision() != self.script_revision() {
            return Err(invalid_phase());
        }
        let rule = self
            .entity_creation_rule(rule_id)
            .ok_or_else(invalid_phase)?;
        EntityRegistry::prepare(
            rule,
            self.revision(),
            session_id,
            directory.script_revision(),
            canonical_name,
            directory,
        )
    }
}

/// Schema appended to the shared engine migration sequence; the world registry stays engine-owned.
pub const MIGRATION: &str = ENTITY_REGISTRY_SCHEMA;

/// Rejects a missing or weakened registry schema during storage startup.
/// # Errors
/// A table definition that differs from the registered migration is corrupt.
pub fn verify_schema(connection: &Connection) -> aoidos_store::error::Result<()> {
    for (name, expected) in [
        ("entity_registry_runs", RUN_SCHEMA),
        ("entity_registry_entries", ENTRIES_SCHEMA),
    ] {
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(aoidos_store::db::sqlite_error)?;
        let Some(actual) = actual else {
            return Err(aoidos_store::error::StoreError::Corrupt(
                "entity registry schema missing".into(),
            ));
        };
        if compact_sql(&actual) != compact_sql(expected) {
            return Err(aoidos_store::error::StoreError::Corrupt(
                "entity registry schema mismatch".into(),
            ));
        }
    }
    Ok(())
}

const RUN_SCHEMA: &str = "CREATE TABLE entity_registry_runs (session_id TEXT PRIMARY KEY CHECK(length(session_id)=36), script_id TEXT NOT NULL CHECK(length(script_id) BETWEEN 1 AND 64), script_revision TEXT NOT NULL CHECK(length(script_revision)=71), seed_revision INTEGER NOT NULL CHECK(seed_revision BETWEEN 1 AND 9007199254740991), entity_revision INTEGER NOT NULL CHECK(entity_revision BETWEEN seed_revision AND 9007199254740991))";
const ENTRIES_SCHEMA: &str = "CREATE TABLE entity_registry_entries (session_id TEXT NOT NULL REFERENCES entity_registry_runs(session_id), entity_id TEXT NOT NULL CHECK(length(entity_id) BETWEEN 1 AND 64), kind TEXT NOT NULL CHECK(kind IN ('character','place','organization','item','term')), scope TEXT NOT NULL CHECK(length(scope) BETWEEN 1 AND 128), canonical_name TEXT NOT NULL CHECK(length(canonical_name) BETWEEN 1 AND 256), rule_id TEXT NOT NULL CHECK(length(rule_id) BETWEEN 1 AND 64), catalog_revision TEXT NOT NULL CHECK(length(catalog_revision) BETWEEN 1 AND 256), mutation_id TEXT NOT NULL UNIQUE REFERENCES store_applied(id), mutation_hash TEXT NOT NULL CHECK(length(mutation_hash)=71), record_seq INTEGER NOT NULL CHECK(record_seq BETWEEN 1 AND 9007199254740991), entity_revision INTEGER NOT NULL CHECK(entity_revision BETWEEN 2 AND 9007199254740991), PRIMARY KEY(session_id,entity_id), UNIQUE(session_id,entity_revision))";

fn ensure_run(
    connection: &mut Connection,
    session: &Session,
    seed: &EntityDirectory,
) -> Result<(), Fault> {
    if !format::valid_uuid(&session.header().session_id)
        || !format::valid_hash(&session.header().script_revision)
        || seed.script_revision() != session.header().script_revision
        || !seed.is_complete()
        || seed.revision() > MAX_SAFE_INTEGER
    {
        return Err(invalid_phase());
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql)?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO entity_registry_runs(session_id,script_id,script_revision,seed_revision,entity_revision) VALUES(?1,?2,?3,?4,?4)",
            params![
                session.header().session_id,
                session.header().script_id,
                session.header().script_revision,
                seed.revision() as i64,
            ],
        )
        .map_err(sql)?;
    let stored: (String, String, i64, i64) = transaction
        .query_row(
            "SELECT script_id,script_revision,seed_revision,entity_revision FROM entity_registry_runs WHERE session_id=?1",
            [&session.header().session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(sql)?;
    if stored.0 != session.header().script_id
        || stored.1 != session.header().script_revision
        || u64::try_from(stored.2).ok() != Some(seed.revision())
        || stored.3 < stored.2
        || u64::try_from(stored.3).is_err()
        || u64::try_from(stored.3).is_ok_and(|revision| revision > MAX_SAFE_INTEGER)
    {
        return Err(corrupt());
    }
    transaction.commit().map_err(sql)
}

fn read_rows(
    connection: &Connection,
    session: &Session,
    seed: &EntityDirectory,
) -> Result<RegistryRows, Fault> {
    let (seed_revision, revision): (i64, i64) = connection
        .query_row(
            "SELECT seed_revision,entity_revision FROM entity_registry_runs WHERE session_id=?1 AND script_id=?2 AND script_revision=?3",
            params![session.header().session_id, session.header().script_id, session.header().script_revision],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql)?
        .ok_or_else(corrupt)?;
    let seed_revision = u64::try_from(seed_revision).map_err(|_| corrupt())?;
    let revision = u64::try_from(revision).map_err(|_| corrupt())?;
    if seed_revision != seed.revision() || revision < seed_revision || revision > MAX_SAFE_INTEGER {
        return Err(corrupt());
    }
    let path = session
        .path_at(session.history_revision())?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut statement = connection
        .prepare(
            "SELECT entity_id,kind,scope,canonical_name,rule_id,catalog_revision,mutation_id,mutation_hash,record_seq,entity_revision FROM entity_registry_entries WHERE session_id=?1 ORDER BY entity_revision LIMIT ?2",
        )
        .map_err(sql)?;
    let mut rows = statement
        .query(params![
            session.header().session_id,
            (MAX_DYNAMIC_ENTITIES + 1) as i64
        ])
        .map_err(sql)?;
    let mut all = Vec::new();
    while let Some(row) = rows.next().map_err(sql)? {
        if all.len() >= MAX_DYNAMIC_ENTITIES {
            return Err(corrupt());
        }
        let kind = parse_kind(&row.get::<_, String>(1).map_err(sql)?)?;
        let stored = StoredEntity {
            entity: CanonicalEntity {
                id: row.get(0).map_err(sql)?,
                kind,
                scope: row.get(2).map_err(sql)?,
                canonical_name: row.get(3).map_err(sql)?,
            },
            rule_id: row.get(4).map_err(sql)?,
            catalog_revision: row.get(5).map_err(sql)?,
            mutation_id: row.get(6).map_err(sql)?,
            mutation_hash: row.get(7).map_err(sql)?,
            record_seq: u64::try_from(row.get::<_, i64>(8).map_err(sql)?).map_err(|_| corrupt())?,
            revision: u64::try_from(row.get::<_, i64>(9).map_err(sql)?).map_err(|_| corrupt())?,
        };
        if !valid_dynamic_id(&stored.entity.id)
            || !valid_label(&stored.entity.scope, MAX_SCOPE_BYTES)
            || !valid_label(&stored.entity.canonical_name, MAX_NAME_BYTES)
            || !format::valid_id(&stored.rule_id)
            || stored.catalog_revision.is_empty()
            || stored.catalog_revision.len() > 256
            || !format::valid_uuid(&stored.mutation_id)
            || !format::valid_hash(&stored.mutation_hash)
            || stored.record_seq == 0
            || stored.record_seq > MAX_SEQ
            || stored.revision <= seed_revision
            || !aoidos_store::applied::contains(
                connection,
                &stored.mutation_id,
                &stored.mutation_hash,
            )
            .map_err(Fault::from)?
            || !record_contains_persisted(session, &stored)?
        {
            return Err(corrupt());
        }
        all.push(stored);
    }
    if revision
        != seed_revision
            .checked_add(all.len() as u64)
            .ok_or_else(corrupt)?
        || all
            .iter()
            .enumerate()
            .any(|(index, entry)| entry.revision != seed_revision + index as u64 + 1)
    {
        return Err(corrupt());
    }
    let visible = all
        .iter()
        .filter(|stored| path.contains(&stored.record_seq))
        .cloned()
        .collect();
    Ok(RegistryRows {
        revision,
        seed_revision,
        all,
        visible,
        path,
    })
}

fn validate_group_rows(
    rows: &RegistryRows,
    session: &Session,
    seed: &EntityDirectory,
    catalog: &SceneCatalog,
    mutations: &[WorldMutation],
) -> Result<(), Fault> {
    if rows.revision < rows.seed_revision || !seed.is_complete() {
        return Err(invalid_phase());
    }
    let mut entries = rows
        .visible
        .iter()
        .map(|stored| stored.entity.clone())
        .collect::<Vec<_>>();
    let seed_entities = seed.entries().collect::<Vec<_>>();
    let mut ids = seed_entities
        .iter()
        .map(|entity| entity.id.clone())
        .chain(rows.all.iter().map(|stored| stored.entity.id.clone()))
        .collect::<BTreeSet<_>>();
    let mut counts = BTreeMap::<String, usize>::new();
    for stored in &rows.visible {
        *counts.entry(stored.rule_id.clone()).or_default() += 1;
    }
    let mut planned_ids = BTreeSet::new();
    let mut planned_count = 0_usize;
    for mutation in mutations {
        if mutation.data.kind != "entityCreated" {
            continue;
        }
        let intent = decode_intent(mutation)?;
        if intent.session_id != session.header().session_id
            || intent.script_revision != session.header().script_revision
            || intent.catalog_revision != catalog.revision()
        {
            return Err(invalid_phase());
        }
        let rule = catalog
            .entity_creation_rule(&intent.rule_id)
            .ok_or_else(invalid_phase)?;
        if rule.kind != intent.kind
            || rule.scope != intent.scope
            || !valid_dynamic_id(&intent.entity_id)
            || !planned_ids.insert(intent.entity_id.clone())
        {
            return Err(Fault::bad_request());
        }
        if let Some(previous) = rows
            .all
            .iter()
            .find(|stored| stored.mutation_id == mutation.mutation_id)
        {
            if !same_intent(previous, &intent, &previous.mutation_hash)
                || !rows.path.contains(&previous.record_seq)
            {
                return Err(invalid_phase());
            }
            continue;
        }
        if !valid_label(&intent.canonical_name, MAX_NAME_BYTES)
            || ids.contains(&intent.entity_id)
            || entries.iter().any(|entity| {
                entity.kind == intent.kind
                    && entity.scope == intent.scope
                    && entity.canonical_name == intent.canonical_name
            })
        {
            return Err(Fault::bad_request());
        }
        let count = counts.entry(intent.rule_id.clone()).or_default();
        let projected_revision = rows.revision + planned_count as u64;
        if *count >= rule.max_instances
            || rows.all.len() + seed_entities.len() + planned_count >= MAX_ENTITIES
            || projected_revision >= MAX_SAFE_INTEGER
        {
            return Err(Fault::bad_request());
        }
        *count += 1;
        planned_count += 1;
        ids.insert(intent.entity_id.clone());
        entries.push(CanonicalEntity {
            id: intent.entity_id,
            kind: intent.kind,
            scope: intent.scope,
            canonical_name: intent.canonical_name,
        });
    }
    let _ = seed.with_dynamic(rows.revision, entries)?;
    Ok(())
}

fn record_contains_persisted(session: &Session, stored: &StoredEntity) -> Result<bool, Fault> {
    let Some(mutations) = record_mutations(session, stored.record_seq)? else {
        return Ok(false);
    };
    for mutation in mutations {
        if mutation.mutation_id == stored.mutation_id {
            let intent = decode_intent(&mutation)?;
            return Ok(mutation_hash(&mutation)? == stored.mutation_hash
                && intent.entity_id == stored.entity.id
                && intent.kind == stored.entity.kind
                && intent.scope == stored.entity.scope
                && intent.canonical_name == stored.entity.canonical_name
                && intent.rule_id == stored.rule_id
                && intent.catalog_revision == stored.catalog_revision
                && intent.session_id == session.header().session_id);
        }
    }
    Ok(false)
}

fn record_contains_mutation(
    session: &Session,
    record_seq: u64,
    mutation: &WorldMutation,
    content_hash: &str,
) -> Result<bool, Fault> {
    let Some(mutations) = record_mutations(session, record_seq)? else {
        return Ok(false);
    };
    Ok(mutations.iter().any(|candidate| {
        candidate.mutation_id == mutation.mutation_id
            && mutation_hash(candidate).is_ok_and(|hash| hash == content_hash)
    }))
}

// Both recovery and apply read the same validated causal fact; future bodies grant no identity.
fn record_mutations(
    session: &Session,
    record_seq: u64,
) -> Result<Option<Vec<WorldMutation>>, Fault> {
    let parsed = session.read(record_seq)?;
    let Some(record) = parsed.body else {
        return Ok(None);
    };
    let crate::record::format::Body::System { code, data, .. } = record.body else {
        return Ok(None);
    };
    match Fact::decode(&code, &data).map_err(Fault::from)? {
        Fact::SettlementPlanned { mutations, .. } => Ok(Some(mutations)),
        _ => Ok(None),
    }
}

fn same_intent(stored: &StoredEntity, intent: &EntityCreationIntent, hash: &str) -> bool {
    stored.entity.id == intent.entity_id
        && stored.entity.kind == intent.kind
        && stored.entity.scope == intent.scope
        && stored.entity.canonical_name == intent.canonical_name
        && stored.rule_id == intent.rule_id
        && stored.catalog_revision == intent.catalog_revision
        && stored.mutation_id.len() == 36
        && stored.mutation_hash == hash
}

fn decode_intent(mutation: &WorldMutation) -> Result<EntityCreationIntent, Fault> {
    if mutation.data.kind != "entityCreated"
        || mutation.data.version != 1
        || !format::valid_uuid(&mutation.mutation_id)
    {
        return Err(invalid_phase());
    }
    let intent: EntityCreationIntent =
        aoidos_json::from_value(mutation.data.payload.clone()).map_err(|_| Fault::bad_request())?;
    if intent.version != 1
        || !format::valid_uuid(&intent.session_id)
        || !format::valid_hash(&intent.script_revision)
        || intent.catalog_revision.is_empty()
        || intent.catalog_revision.len() > 256
        || !format::valid_id(&intent.rule_id)
        || !valid_dynamic_id(&intent.entity_id)
        || !valid_label(&intent.scope, MAX_SCOPE_BYTES)
        || !valid_label(&intent.canonical_name, MAX_NAME_BYTES)
    {
        return Err(Fault::bad_request());
    }
    Ok(intent)
}

fn dynamic_id() -> String {
    format!("dynamic-{}", uuid::Uuid::new_v4().simple())
}

fn valid_dynamic_id(id: &str) -> bool {
    id.strip_prefix("dynamic-").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn kind_name(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Character => "character",
        EntityKind::Place => "place",
        EntityKind::Organization => "organization",
        EntityKind::Item => "item",
        EntityKind::Term => "term",
    }
}

fn parse_kind(kind: &str) -> Result<EntityKind, Fault> {
    match kind {
        "character" => Ok(EntityKind::Character),
        "place" => Ok(EntityKind::Place),
        "organization" => Ok(EntityKind::Organization),
        "item" => Ok(EntityKind::Item),
        "term" => Ok(EntityKind::Term),
        _ => Err(corrupt()),
    }
}

fn compact_sql(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn sql(error: rusqlite::Error) -> Fault {
    Fault::from(aoidos_store::db::sqlite_error(error))
}

fn corrupt() -> Fault {
    Fault::new("store.corrupt", "实体注册表回执或身份不一致")
}

fn invalid_phase() -> Fault {
    Fault::new("engine.invalid-phase", "实体身份规则或因果边界不可用")
}
