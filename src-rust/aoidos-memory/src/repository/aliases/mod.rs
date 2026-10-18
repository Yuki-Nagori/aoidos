//! 明确确认的别名提交、撤销与按有效来源重建的精确查找。

use super::*;
use aho_corasick::AhoCorasick;
use std::collections::BTreeMap;

const MAX_ALIASES_PER_RUN: u64 = 4096;
const MAX_ALIAS_SOURCES: usize = 128;
const MAX_ALIAS_NAME_BYTES: usize = 256;
const MAX_ALIAS_MATCH_TEXT_BYTES: usize = 256 * 1024;
const MAX_ALIAS_MATCHES: usize = 16_384;

impl Repository {
    /// 按当前完整目录和有效因果来源解析精确别名；相似名称不在此处合并。
    /// # Errors
    /// 目录不完整、存储损坏或 run / session 边界不匹配时拒绝查找。
    pub fn resolve_alias(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        run_id: &str,
        category: EntityCategory,
        scope: &str,
        name: &str,
    ) -> Result<Option<EntityAlias>> {
        validate_alias_name(name, scope)?;
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        let row = conn
            .query_row(
                "SELECT alias_id,run_id,entity_id,category,scope,alias_name,status,revision,registered_entity_revision FROM memory_aliases WHERE run_id=?1 AND category=?2 AND scope=?3 AND alias_name=?4 AND status='active'",
                params![run_id, category.as_db(), scope, name],
                read_alias,
            )
            .optional()
            .map_err(sql)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let alias = decode_alias(row)?;
        let Some(sources) = effective_alias_sources(conn, &alias, &run, &boundary, port)? else {
            return Ok(None);
        };
        Ok(Some(EntityAlias { sources, ..alias }))
    }

    /// 按原文精确筛选并重建当前周目仍有效的已确认别名。
    /// 原始别名名称不单独作为授权返回；仅返回来源仍有效且目录未发生冲突的绑定。
    /// # Errors
    /// 文本窗口、别名表或匹配数量超限，或来源 / 目录状态损坏时拒绝查询。
    pub fn aliases_mentioned_in(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        run_id: &str,
        texts: &[&str],
    ) -> Result<Vec<EntityAlias>> {
        if texts.is_empty() || texts.len() > 128 || !valid_id(run_id) {
            return Err(reject());
        }
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        let mut statement = conn
            .prepare("SELECT alias_id,run_id,entity_id,category,scope,alias_name,status,revision,registered_entity_revision FROM memory_aliases WHERE run_id=?1 AND status='active' ORDER BY category,scope,alias_name LIMIT ?2")
            .map_err(sql)?;
        let rows = statement
            .query_map(params![run_id, MAX_ALIASES_PER_RUN + 1], read_alias)
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        if rows.len() as u64 > MAX_ALIASES_PER_RUN {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let aliases = rows
            .into_iter()
            .map(decode_alias)
            .collect::<Result<Vec<_>>>()?;
        let text_bytes = texts
            .iter()
            .fold(0usize, |total, text| total.saturating_add(text.len()));
        if text_bytes > MAX_ALIAS_MATCH_TEXT_BYTES {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        if aliases.is_empty() {
            return Ok(Vec::new());
        }
        let matcher = AhoCorasick::new(aliases.iter().map(|alias| alias.name.as_str()))
            .map_err(|_| Error::Rejected(Reason::CapacityBlocked))?;
        let mut matched_patterns = BTreeSet::new();
        let mut match_count = 0usize;
        for text in texts {
            for matched in matcher.find_overlapping_iter(text) {
                match_count = match_count.saturating_add(1);
                if match_count > MAX_ALIAS_MATCHES {
                    return Err(Error::Rejected(Reason::CapacityBlocked));
                }
                matched_patterns.insert(matched.pattern().as_usize());
            }
        }
        let mut matches = Vec::new();
        for index in matched_patterns {
            let alias = &aliases[index];
            if let Some(sources) = effective_alias_sources(conn, alias, &run, &boundary, port)? {
                matches.push(EntityAlias {
                    sources,
                    ..alias.clone()
                });
            }
        }
        let mut names = BTreeMap::<String, usize>::new();
        for alias in &matches {
            *names.entry(alias.name.clone()).or_default() += 1;
        }
        matches.retain(|alias| names.get(&alias.name) == Some(&1));
        if matches.len() > 128 {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        Ok(matches)
    }

    /// 以别名身份和 revision 作 CAS 撤销；重复提交同一已完成撤销保持幂等。
    /// # Errors
    /// 过期实体 / 别名版本、未知别名或损坏回执拒绝，不删除来源历史。
    pub fn revoke_alias(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        run_id: &str,
        revocation: AliasRevocation,
    ) -> Result<EntityAlias> {
        self.ensure_writable(conn, run_id)?;
        if !valid_id(run_id)
            || !valid_id(&revocation.alias_id)
            || revocation.expected_revision == 0
            || revocation.expected_revision > MAX_INTEGER
            || revocation.expected_entity_revision > MAX_INTEGER
        {
            return Err(reject());
        }
        let (_run, boundary) = self.boundary(conn, run_id, port)?;
        if boundary.entity_revision != revocation.expected_entity_revision {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let alias = read_alias_by_id(&tx, run_id, &revocation.alias_id)?.ok_or(Error::NotFound)?;
        if alias.status == AliasStatus::Revoked {
            if revocation.expected_revision == alias.revision
                || revocation.expected_revision.checked_add(1) == Some(alias.revision)
            {
                tx.commit().map_err(sql)?;
                return Ok(EntityAlias {
                    sources: alias_sources(conn, &alias.alias_id)?,
                    ..alias
                });
            }
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        if alias.revision != revocation.expected_revision {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let next_revision = alias
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_INTEGER)
            .ok_or(Error::Rejected(Reason::CapacityBlocked))?;
        let changed = tx
            .execute(
                "UPDATE memory_aliases SET status='revoked',revision=?1 WHERE alias_id=?2 AND run_id=?3 AND status='active' AND revision=?4",
                params![next_revision, revocation.alias_id, run_id, alias.revision],
            )
            .map_err(sql)?;
        if changed != 1 {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        insert_alias_event(
            &tx,
            &alias.alias_id,
            next_revision,
            "revoked",
            None,
            boundary.entity_revision,
        )?;
        tx.commit().map_err(sql)?;
        let mut revoked = alias;
        revoked.status = AliasStatus::Revoked;
        revoked.revision = next_revision;
        revoked.sources = alias_sources(conn, &revoked.alias_id)?;
        Ok(revoked)
    }
}

/// 被明确确认的问题答案与别名 source / event 同事务提交。
pub(super) fn apply_confirmed_alias(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    run: &Run,
    question: &MemoryQuestion,
    entity_id: &str,
    boundary: &Boundary,
    port: &dyn CausalPort,
) -> Result<()> {
    let Some(name) = question.draft.mentioned_name.as_deref() else {
        return Ok(());
    };
    validate_alias_name(name, "global")?;
    let Some(context) = port.entity_alias_context(entity_id)? else {
        return Err(Error::Rejected(Reason::PolicyUnavailable));
    };
    if context.entity_id != entity_id
        || !valid_name(entity_id)
        || !valid_scope(&context.scope)
        || !valid_alias_text(&context.canonical_name)
    {
        return Err(reject());
    }
    if let Some(canonical_id) = port.canonical_entity_id(context.category, &context.scope, name)? {
        if canonical_id != entity_id {
            return Err(Error::Rejected(Reason::AmbiguousEntity));
        }
        return Ok(());
    }
    let mut supporting_sources = Vec::new();
    for source in &question.draft.identity.sources {
        if !port.valid_source(source)? {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        if port
            .source_text(source)?
            .is_some_and(|text| text.contains(name))
        {
            supporting_sources.push(source);
        }
    }
    if supporting_sources.is_empty() {
        return Err(Error::Rejected(Reason::InvalidSource));
    }
    let existing_for_name = tx
        .query_row(
            "SELECT alias_id,run_id,entity_id,category,scope,alias_name,status,revision,registered_entity_revision FROM memory_aliases WHERE run_id=?1 AND category=?2 AND scope=?3 AND alias_name=?4 AND status='active'",
            params![run_id, context.category.as_db(), context.scope, name],
            read_alias,
        )
        .optional()
        .map_err(sql)?
        .map(decode_alias)
        .transpose()?;
    if let Some(existing) = existing_for_name.filter(|alias| alias.entity_id != entity_id) {
        if boundary.entity_revision < existing.registered_entity_revision {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        if effective_alias_sources(tx, &existing, run, boundary, port)?.is_some() {
            return Err(Error::Rejected(Reason::AmbiguousEntity));
        }
        revoke_stale_alias(tx, &existing, boundary.entity_revision)?;
    }
    let pending_aliases: u64 = tx
        .query_row(
            "SELECT count(*) FROM memory_aliases WHERE run_id=?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(sql)?;
    let mut alias = find_alias_by_key(
        tx,
        run_id,
        context.category,
        &context.scope,
        name,
        entity_id,
    )?;
    let created = alias.is_none();
    if created {
        if pending_aliases >= MAX_ALIASES_PER_RUN {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        alias = Some(EntityAlias {
            alias_id: new_id(),
            run_id: run_id.into(),
            entity_id: entity_id.into(),
            category: context.category,
            scope: context.scope.clone(),
            name: name.into(),
            status: AliasStatus::Active,
            revision: 1,
            registered_entity_revision: boundary.entity_revision,
            sources: Vec::new(),
        });
        let alias = alias.as_ref().expect("new alias assigned");
        tx.execute(
            "INSERT INTO memory_aliases(alias_id,run_id,entity_id,category,scope,alias_name,status,revision,registered_entity_revision) VALUES(?1,?2,?3,?4,?5,?6,'active',1,?7)",
            params![alias.alias_id, run_id, entity_id, context.category.as_db(), context.scope, name, boundary.entity_revision],
        )
        .map_err(sql)?;
    }
    let mut alias = alias.expect("alias created or found");
    let mut existing_source_count = alias_source_count(tx, &alias.alias_id)?;
    if existing_source_count > MAX_ALIAS_SOURCES as u64 {
        return Err(Error::Corrupt);
    }
    let mut inserted = false;
    for source in supporting_sources {
        let source_json = canonical(source)?;
        let source_hash = hash(&source_json);
        let already_registered: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_alias_sources WHERE alias_id=?1 AND question_id=?2 AND source_hash=?3)",
                params![alias.alias_id, question.question_id, source_hash.as_slice()],
                |row| row.get(0),
            )
            .map_err(sql)?;
        if already_registered {
            continue;
        }
        if existing_source_count >= MAX_ALIAS_SOURCES as u64 {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let changed = tx.execute(
            "INSERT OR IGNORE INTO memory_alias_sources(alias_id,question_id,source_hash,source_json) VALUES(?1,?2,?3,?4)",
            params![alias.alias_id, question.question_id, source_hash.as_slice(), source_json],
        )
        .map_err(sql)?;
        inserted |= changed == 1;
        existing_source_count += u64::from(changed == 1);
    }
    let event = if created {
        Some("registered")
    } else if inserted && alias.status == AliasStatus::Revoked {
        Some("reactivated")
    } else if inserted {
        Some("supported")
    } else {
        None
    };
    if let Some(event) = event {
        if !created {
            alias.revision = alias
                .revision
                .checked_add(1)
                .filter(|revision| *revision <= MAX_INTEGER)
                .ok_or(Error::Rejected(Reason::CapacityBlocked))?;
            alias.status = AliasStatus::Active;
            tx.execute(
                "UPDATE memory_aliases SET status='active',revision=?1,registered_entity_revision=?2 WHERE alias_id=?3 AND revision=?4",
                params![alias.revision, boundary.entity_revision, alias.alias_id, alias.revision - 1],
            )
            .map_err(sql)?;
        }
        insert_alias_event(
            tx,
            &alias.alias_id,
            alias.revision,
            event,
            Some(&question.question_id),
            boundary.entity_revision,
        )?;
    }
    Ok(())
}

fn find_alias_by_key(
    conn: &Connection,
    run_id: &str,
    category: EntityCategory,
    scope: &str,
    name: &str,
    entity_id: &str,
) -> Result<Option<EntityAlias>> {
    let row = conn
        .query_row(
            "SELECT alias_id,run_id,entity_id,category,scope,alias_name,status,revision,registered_entity_revision FROM memory_aliases WHERE run_id=?1 AND category=?2 AND scope=?3 AND alias_name=?4 AND entity_id=?5",
            params![run_id, category.as_db(), scope, name, entity_id],
            read_alias,
        )
        .optional()
        .map_err(sql)?;
    row.map(decode_alias).transpose()
}

fn read_alias_by_id(
    conn: &Connection,
    run_id: &str,
    alias_id: &str,
) -> Result<Option<EntityAlias>> {
    let row = conn
        .query_row(
            "SELECT alias_id,run_id,entity_id,category,scope,alias_name,status,revision,registered_entity_revision FROM memory_aliases WHERE run_id=?1 AND alias_id=?2",
            params![run_id, alias_id],
            read_alias,
        )
        .optional()
        .map_err(sql)?;
    row.map(decode_alias).transpose()
}

fn alias_sources(conn: &Connection, alias_id: &str) -> Result<Vec<SourceRef>> {
    let count = alias_source_count(conn, alias_id)?;
    if count > MAX_ALIAS_SOURCES as u64 {
        return Err(Error::Corrupt);
    }
    let mut statement = conn
        .prepare("SELECT length(source_json),source_json,source_hash FROM memory_alias_sources WHERE alias_id=?1 ORDER BY question_id,source_hash")
        .map_err(sql)?;
    let rows = statement
        .query_map([alias_id], |row| {
            Ok((
                read_bounded_blob(row, 0, 1, MAX_VERSION_BYTES)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(sql)?;
    rows.map(|row| {
        let (bounded, stored_hash) = row.map_err(sql)?;
        let bytes = bounded.into_result()?;
        let source: SourceRef = decode(&bytes, MAX_VERSION_BYTES)?;
        source.validate().map_err(|_| Error::Corrupt)?;
        let source_json = canonical(&source)?;
        if hash(&source_json).as_slice() != stored_hash.as_slice() {
            return Err(Error::Corrupt);
        }
        Ok(source)
    })
    .collect()
}

fn effective_alias_sources(
    conn: &Connection,
    alias: &EntityAlias,
    run: &Run,
    boundary: &Boundary,
    port: &dyn CausalPort,
) -> Result<Option<Vec<SourceRef>>> {
    if boundary.entity_revision < alias.registered_entity_revision {
        return Err(Error::Rejected(Reason::StaleVersion));
    }
    let Some(context) = port.entity_alias_context(&alias.entity_id)? else {
        return Ok(None);
    };
    if context.entity_id != alias.entity_id
        || context.category != alias.category
        || context.scope != alias.scope
        || port
            .canonical_entity_id(alias.category, &alias.scope, &alias.name)?
            .is_some()
    {
        return Ok(None);
    }
    let mut effective = Vec::new();
    for source in alias_sources(conn, &alias.alias_id)? {
        if source.run_id == run.run_id
            && source.script_id == run.script_id
            && source.session_id == run.session_id
            && port.valid_source(&source)?
        {
            effective.push(source);
        }
    }
    Ok((!effective.is_empty()).then_some(effective))
}

fn revoke_stale_alias(
    tx: &rusqlite::Transaction<'_>,
    alias: &EntityAlias,
    entity_revision: u64,
) -> Result<()> {
    let revision = alias
        .revision
        .checked_add(1)
        .filter(|revision| *revision <= MAX_INTEGER)
        .ok_or(Error::Rejected(Reason::CapacityBlocked))?;
    let changed = tx
        .execute(
            "UPDATE memory_aliases SET status='revoked',revision=?1 WHERE alias_id=?2 AND status='active' AND revision=?3",
            params![revision, alias.alias_id, alias.revision],
        )
        .map_err(sql)?;
    if changed != 1 {
        return Err(Error::Rejected(Reason::StaleVersion));
    }
    insert_alias_event(
        tx,
        &alias.alias_id,
        revision,
        "revoked",
        None,
        entity_revision,
    )
}

fn alias_source_count(conn: &Connection, alias_id: &str) -> Result<u64> {
    conn.query_row(
        "SELECT count(*) FROM memory_alias_sources WHERE alias_id=?1",
        [alias_id],
        |row| row.get(0),
    )
    .map_err(sql)
}

fn insert_alias_event(
    conn: &Connection,
    alias_id: &str,
    revision: u64,
    event_kind: &str,
    question_id: Option<&str>,
    entity_revision: u64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO memory_alias_events(event_id,alias_id,revision,event_kind,question_id,entity_revision) VALUES(?1,?2,?3,?4,?5,?6)",
        params![new_id(), alias_id, revision, event_kind, question_id, entity_revision],
    )
    .map_err(sql)?;
    Ok(())
}

fn validate_alias_name(name: &str, scope: &str) -> Result<()> {
    if !valid_scope(scope) || !valid_alias_text(name) {
        return Err(reject());
    }
    Ok(())
}
fn valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= 128
        && scope.trim() == scope
        && !scope.chars().any(char::is_control)
}
fn valid_alias_text(name: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= MAX_ALIAS_NAME_BYTES
        && name.trim() == name
        && !name.chars().any(char::is_control)
}

type AliasRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    u64,
    u64,
);
fn read_alias(row: &rusqlite::Row<'_>) -> rusqlite::Result<AliasRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}
fn decode_alias(row: AliasRow) -> Result<EntityAlias> {
    let category = EntityCategory::from_db(&row.3).ok_or(Error::Corrupt)?;
    let status = match row.6.as_str() {
        "active" => AliasStatus::Active,
        "revoked" => AliasStatus::Revoked,
        _ => return Err(Error::Corrupt),
    };
    if !valid_id(&row.0)
        || !valid_id(&row.1)
        || !valid_name(&row.2)
        || !valid_scope(&row.4)
        || !valid_alias_text(&row.5)
        || row.7 == 0
        || row.7 > MAX_INTEGER
        || row.8 > MAX_INTEGER
    {
        return Err(Error::Corrupt);
    }
    Ok(EntityAlias {
        alias_id: row.0,
        run_id: row.1,
        entity_id: row.2,
        category,
        scope: row.4,
        name: row.5,
        status,
        revision: row.7,
        registered_entity_revision: row.8,
        sources: Vec::new(),
    })
}

#[cfg(test)]
mod tests;
