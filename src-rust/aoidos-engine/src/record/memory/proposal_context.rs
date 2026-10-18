//! 从已冻结素材构造受信主体、实体与获知证据白名单。

use super::{
    CycleMarkContextInput, DIRECT_DIALOGUE_RULE, DIRECT_DIALOGUE_SCOPE, SourcePort,
    build_cycle_mark_context,
};
use crate::{game::catalog::EntityDirectory, record::session::Session};
use aoidos_memory::{
    error::{Error, Reason, Result},
    model::{
        CausalPort, EntityCategory, EntitySnapshot, EvidenceSnapshot, InputMode, KnowledgeKind,
        ProposalContext, Run, SourceRef, Subject, SubjectRef, TargetSnapshot,
    },
    repository::BatchSnapshot,
};
use std::collections::BTreeMap;

const MAX_PROPOSAL_ENTITIES: usize = 128;
const MAX_PROPOSAL_EVIDENCE: usize = 128;
// 限制规范名扫描的总字节比较量，防止大目录与长批次组合造成无界 CPU 消耗。
const MAX_ENTITY_MATCH_BYTES: usize = 64 * 1024 * 1024;

/// 同一会话锁区中构造普通提案上下文所需的可信输入。
pub struct ProposalSourceContextInput<'a> {
    pub run: &'a Run,
    pub session: &'a Session,
    pub directory: &'a EntityDirectory,
    pub batch: &'a BatchSnapshot,
    pub source_port: &'a SourcePort<'a>,
}

/// 完整提案上下文所需的当前会话、目录、剧本与冻结批次。
pub struct ProposalContextInput<'a> {
    pub connection: &'a rusqlite::Connection,
    pub repository: &'a aoidos_memory::repository::Repository,
    pub catalog: &'a crate::game::catalog::SceneCatalog,
    pub run: &'a Run,
    pub session: &'a Session,
    pub directory: &'a EntityDirectory,
    pub batch: &'a BatchSnapshot,
    pub source_port: &'a SourcePort<'a>,
}

/// 重建普通提案白名单和刻痕授权；冻结、发请求前和提交前共用此实现。
/// # Errors
/// 任何来源、目录、周目或规则修订不匹配时失败关闭。
pub fn build_proposal_context(input: ProposalContextInput<'_>) -> Result<ProposalContext> {
    build_proposal_context_inner(input, true)
}

/// 重建不含旧记忆目标的授权上下文，供候选操作提交后核验非目标权限。
/// # Errors
/// 任何来源、目录、周目或规则修订不匹配时失败关闭。
pub fn build_proposal_authority_context(
    input: ProposalContextInput<'_>,
) -> Result<ProposalContext> {
    build_proposal_context_inner(input, false)
}

fn build_proposal_context_inner(
    input: ProposalContextInput<'_>,
    include_targets: bool,
) -> Result<ProposalContext> {
    let mut context = build_proposal_source_context(ProposalSourceContextInput {
        run: input.run,
        session: input.session,
        directory: input.directory,
        batch: input.batch,
        source_port: input.source_port,
    })?;
    add_scene_evidence(&mut context, &input)?;
    add_confirmed_aliases(&mut context, &input)?;
    let cycle = build_cycle_mark_context(CycleMarkContextInput {
        catalog: input.catalog,
        run: input.run,
        session: input.session,
        directory: input.directory,
        batch: input.batch,
        source_port: input.source_port,
    })
    .map_err(|_| Error::Rejected(Reason::StaleVersion))?;
    cycle
        .apply_to(&mut context)
        .map_err(|_| Error::Rejected(Reason::InvalidSchema))?;
    if context.entity_ids.len() > MAX_PROPOSAL_ENTITIES
        || context.evidence.len() > MAX_PROPOSAL_EVIDENCE
    {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    add_entity_snapshots(&mut context, input.directory)?;
    if include_targets {
        add_existing_targets(&mut context, &input)?;
    }
    Ok(context)
}

fn add_existing_targets(
    context: &mut ProposalContext,
    input: &ProposalContextInput<'_>,
) -> Result<()> {
    let subjects = context
        .subjects
        .iter()
        .map(|subject| match subject {
            SubjectRef::PlayerPreference => Subject::PlayerPreference,
            SubjectRef::Character { id } => Subject::Character { id: id.clone() },
        })
        .collect::<Vec<_>>();
    let entries = input.repository.entries_for_subjects(
        input.connection,
        input.source_port,
        &input.run.run_id,
        input.batch.history_revision,
        &subjects,
        aoidos_memory::model::MAX_PROPOSAL_TARGETS + 1,
    )?;
    if entries.len() > aoidos_memory::model::MAX_PROPOSAL_TARGETS {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    for entry in entries {
        let version = match input.repository.current(
            input.connection,
            input.source_port,
            &input.run.run_id,
            &entry.entry_id,
        ) {
            Ok(version) => version,
            Err(Error::Rejected(Reason::InvalidSource)) => continue,
            Err(error) => return Err(error),
        };
        context.targets.insert(
            entry.entry_id.clone(),
            TargetSnapshot {
                entry_id: entry.entry_id,
                version_id: version.version_id,
                state_revision: entry.state_revision,
                state: entry.state,
                subject: version.subject,
                dimension: version.dimension,
                summary: version.summary,
                attitude: version.attitude,
                inferred: version.inferred,
            },
        );
    }
    Ok(())
}

fn add_scene_evidence(
    context: &mut ProposalContext,
    input: &ProposalContextInput<'_>,
) -> Result<()> {
    let ranges = input
        .batch
        .spans
        .iter()
        .map(|span| {
            (
                span.source.record_seq,
                hash_hex(&span.source.body_hash),
                span.source.start_byte,
                span.source.end_byte,
            )
        })
        .collect::<Vec<_>>();
    let trusted = crate::game::presence::SceneEvidence::for_source_ranges(
        input.connection,
        input.session,
        input.catalog,
        input.directory,
        &ranges,
    )
    .map_err(|_| Error::Rejected(Reason::InvalidSource))?;
    if context.evidence.len().saturating_add(trusted.len()) > MAX_PROPOSAL_EVIDENCE {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    for evidence in trusted {
        let spans = source_spans_for_evidence(&evidence, &input.batch.spans)
            .ok_or(Error::Rejected(Reason::InvalidSource))?;
        let mut source = spans[0].source.clone();
        source.start_byte = evidence.source.start_byte;
        source.end_byte = evidence.source.end_byte;
        context.entity_ids.insert(evidence.subject_id.clone());
        context.subjects.insert(SubjectRef::Character {
            id: evidence.subject_id.clone(),
        });
        if let Some(informant_id) = &evidence.informant_id {
            context.entity_ids.insert(informant_id.clone());
        }
        let evidence_id = format!("e{}", context.evidence.len() + 1);
        let source_ids = spans.iter().map(|span| span.source_id.clone()).collect();
        context.evidence.insert(
            evidence_id,
            map_scene_evidence(evidence, source_ids, source),
        );
    }
    Ok(())
}

pub(super) fn map_scene_evidence(
    evidence: crate::game::presence::TrustedEvidence,
    source_ids: Vec<String>,
    source: aoidos_memory::model::SourceRef,
) -> EvidenceSnapshot {
    EvidenceSnapshot {
        event_id: Some(evidence.evidence_id),
        subject_id: evidence.subject_id,
        source_ids,
        kind: match evidence.kind {
            crate::game::presence::EvidenceKind::Observed => KnowledgeKind::Observed,
            crate::game::presence::EvidenceKind::Rumor => KnowledgeKind::Rumor,
        },
        uncertainty: evidence.uncertainty.map(|uncertainty| match uncertainty {
            crate::game::presence::RumorUncertainty::Unverified => {
                aoidos_memory::model::RumorUncertainty::Unverified
            }
            crate::game::presence::RumorUncertainty::Disputed => {
                aoidos_memory::model::RumorUncertainty::Disputed
            }
        }),
        informant_id: evidence.informant_id,
        scope: evidence.scope_id,
        rule_id: evidence.rule_id,
        source,
    }
}

pub(super) fn source_spans_for_evidence<'a>(
    evidence: &crate::game::presence::TrustedEvidence,
    spans: &'a [aoidos_memory::repository::Span],
) -> Option<Vec<&'a aoidos_memory::repository::Span>> {
    let mut matching = spans
        .iter()
        .filter(|span| {
            span.source.record_seq == evidence.source.record_seq
                && hash_hex(&span.source.body_hash) == evidence.source.body_hash
                && span.source.start_byte < evidence.source.end_byte
                && span.source.end_byte > evidence.source.start_byte
        })
        .collect::<Vec<_>>();
    matching.sort_by_key(|span| (span.source.start_byte, span.source.end_byte));

    let mut covered_until = evidence.source.start_byte;
    let mut selected = Vec::new();
    for span in matching {
        if span.source.start_byte > covered_until {
            return None;
        }
        if span.source.end_byte > covered_until {
            selected.push(span);
            covered_until = span.source.end_byte;
            if covered_until >= evidence.source.end_byte {
                return Some(selected);
            }
        }
    }
    None
}

fn add_confirmed_aliases(
    context: &mut ProposalContext,
    input: &ProposalContextInput<'_>,
) -> Result<()> {
    let texts = input
        .batch
        .spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<Vec<_>>();
    let aliases = input.repository.aliases_mentioned_in(
        input.connection,
        input.source_port,
        &input.run.run_id,
        &texts,
    )?;
    for alias in aliases {
        context.entity_ids.insert(alias.entity_id.clone());
        let entity = input
            .directory
            .entity(&alias.entity_id)
            .map_err(|_| Error::Rejected(Reason::PolicyUnavailable))?
            .ok_or(Error::Rejected(Reason::InvalidSource))?;
        let snapshot = context
            .entities
            .entry(alias.entity_id.clone())
            .or_insert_with(|| EntitySnapshot {
                kind: entity_category(entity.kind),
                scope: entity.scope.clone(),
                canonical_name: entity.canonical_name.clone(),
                aliases: Default::default(),
            });
        snapshot.aliases.insert(alias.name);
    }
    Ok(())
}

fn hash_hex(hash: &aoidos_memory::model::Hash) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(7 + hash.len() * 2);
    encoded.push_str("sha256:");
    for byte in hash {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

/// 从本批来源构造普通提案的实体、主体与获知证据白名单。
///
/// 只把当前有效分支上的精确规范名和完整定向对话登记为模型可引用对象；
/// 场外输入仅能授权玩家偏好，不会因正文提及角色而授予角色知识。
/// # Errors
/// 批次身份 / 修订过期、来源与冻结正文不符或上下文超过协议上限时拒绝。
pub fn build_proposal_source_context(
    input: ProposalSourceContextInput<'_>,
) -> Result<ProposalContext> {
    let boundary = input.source_port.boundary()?;
    let batch = input.batch;
    if input.session.needs_recovery()
        || batch.version != 1
        || batch.run_id != input.run.run_id
        || batch.history_revision != boundary.history_revision
        || batch.entity_revision != boundary.entity_revision
        || batch.entity_revision != input.directory.revision()
        || batch.evidence_revision != boundary.evidence_revision
        || batch.policy_hash != input.run.policy_hash
        || boundary.script_id != input.run.script_id
        || boundary.session_id != input.run.session_id
        || input.session.header().session_id != input.run.session_id
        || input.session.header().script_id != input.run.script_id
        || input.session.header().script_revision != input.directory.script_revision()
        || !input.directory.is_complete()
        || batch.spans.is_empty()
        || batch.spans.len() > 128
    {
        return Err(Error::Rejected(Reason::StaleVersion));
    }

    let mut context = ProposalContext::default();
    // 一个记录拆成多个素材片段时，定向对话历史只需验证一次。
    let mut source_records = BTreeMap::<u64, (String, SourceRef)>::new();
    let mut source_text_bytes = 0usize;
    for span in &batch.spans {
        if span.source.run_id != input.run.run_id
            || !context.source_ids.insert(span.source_id.clone())
            || !input.source_port.valid_source(&span.source)?
            || input.source_port.source_text(&span.source)?.as_deref() != Some(&span.text)
        {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        source_text_bytes = source_text_bytes.saturating_add(span.text.len());
        if span.source.mode == Some(InputMode::OutOfCharacter) {
            context.subjects.insert(SubjectRef::PlayerPreference);
        }
        if span.source.mode == Some(InputMode::InCharacter) {
            source_records
                .entry(span.source.record_seq)
                .or_insert_with(|| (span.source_id.clone(), span.source.clone()));
        }
    }

    let mut evidence_by_record = BTreeMap::<u64, (String, String, String, SourceRef)>::new();
    for (record_seq, (source_id, source)) in source_records {
        let Some((character_id, player_id)) =
            input.source_port.direct_dialogue_recipient(&source)?
        else {
            continue;
        };
        evidence_by_record.entry(record_seq).or_insert((
            character_id,
            player_id,
            source_id,
            source,
        ));
    }
    if evidence_by_record.len() > MAX_PROPOSAL_EVIDENCE {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    for (index, (character_id, player_id, source_id, source)) in
        evidence_by_record.into_values().enumerate()
    {
        let entity = input
            .directory
            .entity(&character_id)
            .map_err(|_| Error::Rejected(Reason::PolicyUnavailable))?
            .ok_or(Error::Rejected(Reason::InvalidSource))?;
        if entity.kind != crate::game::catalog::EntityKind::Character {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let evidence_id = format!("e{}", index + 1);
        context.entity_ids.insert(character_id.clone());
        context.subjects.insert(SubjectRef::Character {
            id: character_id.clone(),
        });
        context.evidence.insert(
            evidence_id,
            EvidenceSnapshot {
                event_id: None,
                subject_id: character_id,
                source_ids: vec![source_id],
                kind: KnowledgeKind::Told,
                uncertainty: None,
                informant_id: Some(player_id),
                scope: DIRECT_DIALOGUE_SCOPE.into(),
                rule_id: DIRECT_DIALOGUE_RULE.into(),
                source,
            },
        );
    }

    let entities = input.directory.entries().collect::<Vec<_>>();
    let source_text = batch
        .spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<Vec<_>>();
    if source_text_bytes.saturating_mul(entities.len()) > MAX_ENTITY_MATCH_BYTES {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    for entity in entities {
        if source_text
            .iter()
            .any(|text| text.contains(&entity.canonical_name))
        {
            context.entity_ids.insert(entity.id.clone());
        }
    }
    if context.entity_ids.len() > MAX_PROPOSAL_ENTITIES {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    add_entity_snapshots(&mut context, input.directory)?;
    Ok(context)
}

fn add_entity_snapshots(context: &mut ProposalContext, directory: &EntityDirectory) -> Result<()> {
    for entity_id in &context.entity_ids {
        let entity = directory
            .entity(entity_id)
            .map_err(|_| Error::Rejected(Reason::PolicyUnavailable))?
            .ok_or(Error::Rejected(Reason::InvalidSource))?;
        let aliases = context
            .entities
            .get(entity_id)
            .map(|snapshot| snapshot.aliases.clone())
            .unwrap_or_default();
        context.entities.insert(
            entity_id.clone(),
            EntitySnapshot {
                kind: entity_category(entity.kind),
                scope: entity.scope.clone(),
                canonical_name: entity.canonical_name.clone(),
                aliases,
            },
        );
    }
    Ok(())
}

fn entity_category(kind: crate::game::catalog::EntityKind) -> EntityCategory {
    match kind {
        crate::game::catalog::EntityKind::Character => EntityCategory::Character,
        crate::game::catalog::EntityKind::Place => EntityCategory::Place,
        crate::game::catalog::EntityKind::Organization => EntityCategory::Organization,
        crate::game::catalog::EntityKind::Item => EntityCategory::Item,
        crate::game::catalog::EntityKind::Term => EntityCategory::Term,
    }
}
