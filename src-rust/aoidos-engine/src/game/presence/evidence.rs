//! 观察与传闻作为有序世界 mutation；正文和 UI 都不能直接授予角色知识。

use super::{
    EntityDirectory, EntityKind, MAX_EVIDENCE_BYTES, MAX_EVIDENCE_RECORDS, ObservationRule,
    PresencePolicy, RumorRule, SceneCatalog, State,
};
use crate::{
    fault::Fault,
    ports::Terminal,
    record::{
        facts::{Fact, MutationData, WorldMutation},
        format::{self, Body},
        session::Session,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SOURCE_RANGE: u64 = 32 * 1024;
const MAX_RUMOR_HOPS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// 获知事件类别；观察与传闻保留独立证明字段。
pub enum EvidenceKind {
    Observed,
    Rumor,
}

/// 已提交记录中的有界正文区间；所属日志隐含其 session 身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// 绑定到已提交记录正文的场景获知来源。
pub struct EvidenceSource {
    pub record_seq: u64,
    pub body_hash: String,
    pub start_byte: u64,
    pub end_byte: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// 一条由场景规则登记的观察事件。
pub struct ObservedEvent {
    pub version: u32,
    pub evidence_id: String,
    pub session_id: String,
    pub script_revision: String,
    pub catalog_revision: String,
    pub subject_id: String,
    pub source: EvidenceSource,
    pub scene_id: String,
    pub scope_id: String,
    pub presence_revision: String,
    pub rule_id: String,
    pub rule_version: u32,
}

/// 已授权世界规则为单个观察事件提供的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedEvidenceInput {
    pub subject_id: String,
    pub source: EvidenceSource,
    pub scene_id: String,
    pub scope_id: String,
    pub presence_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// 传闻当前的认知状态，不等于已确认的世界事实。
pub enum RumorUncertainty {
    Unverified,
    Disputed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// 一次独立的定向传闻交付及其父事件链接。
pub struct RumorDelivery {
    pub version: u32,
    pub evidence_id: String,
    pub session_id: String,
    pub script_revision: String,
    pub catalog_revision: String,
    pub recipient_id: String,
    pub informant_id: String,
    pub delivery_source: EvidenceSource,
    pub root_source: EvidenceSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_evidence_id: Option<String>,
    pub scene_id: String,
    pub scope_id: String,
    pub uncertainty: RumorUncertainty,
    pub rule_id: String,
    pub rule_version: u32,
}

/// 返回给 030 的已验证事件数据；不含正文，也不具有写入引擎状态的权限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedEvidence {
    pub evidence_id: String,
    pub subject_id: String,
    pub informant_id: Option<String>,
    pub kind: EvidenceKind,
    pub scope_id: String,
    pub rule_id: String,
    pub source: EvidenceSource,
    pub uncertainty: Option<RumorUncertainty>,
}

/// 供记忆适配器使用的有界只读查询；不会写入事实或回执。
pub struct SceneEvidence;

impl SceneEvidence {
    /// 解析一个当前有效、已应用且所属回合完整的事件及其全部传闻链。
    /// 索引、来源回合与传闻链共用一次读取的缓存，总量最多 4096 条 / 16 MiB。
    /// 窗口内已应用事件的在场证明来自 commit / recovery 的完整校验；窗口外来源不消费。
    /// # Errors
    /// 记录损坏、待恢复或证据不可用时失败关闭。
    pub fn get(
        connection: &rusqlite::Connection,
        session: &Session,
        catalog: &SceneCatalog,
        directory: &EntityDirectory,
        evidence_id: &str,
    ) -> Result<Option<TrustedEvidence>, Fault> {
        if !format::valid_uuid(evidence_id)
            || session.needs_recovery()
            || session.is_read_only()
            || directory.script_revision() != session.header().script_revision
            || catalog.script_revision() != session.header().script_revision
            || !directory.is_complete()
        {
            return Err(super::invalid_phase());
        }
        let Some(found) =
            find_trusted_evidence(connection, session, catalog, directory, evidence_id, None)?
        else {
            return Ok(None);
        };
        let (event_id, subject_id, informant_id, kind, scope_id, rule_id, source, uncertainty) =
            match found {
                TrustedPayload::Observed(event) => (
                    event.evidence_id,
                    event.subject_id,
                    None,
                    EvidenceKind::Observed,
                    event.scope_id,
                    event.rule_id,
                    event.source,
                    None,
                ),
                TrustedPayload::Rumor { event, .. } => (
                    event.evidence_id,
                    event.recipient_id,
                    Some(event.informant_id),
                    EvidenceKind::Rumor,
                    event.scope_id,
                    event.rule_id,
                    event.delivery_source,
                    Some(event.uncertainty),
                ),
            };
        Ok(Some(TrustedEvidence {
            evidence_id: event_id,
            subject_id,
            informant_id,
            kind,
            scope_id,
            rule_id,
            source,
            uncertainty,
        }))
    }
}

impl ObservedEvent {
    /// 从可信规则输入构造带唯一事件身份的版本化 mutation。
    /// # Errors
    /// 事件无法编码为规范载荷时拒绝。
    pub fn mutation(
        session: &Session,
        catalog: &SceneCatalog,
        input: ObservedEvidenceInput,
        rule: &ObservationRule,
    ) -> Result<WorldMutation, Fault> {
        let mutation_id = uuid::Uuid::new_v4().to_string();
        let event = Self {
            version: 1,
            evidence_id: mutation_id.clone(),
            session_id: session.header().session_id.clone(),
            script_revision: session.header().script_revision.clone(),
            catalog_revision: catalog.revision().to_owned(),
            subject_id: input.subject_id,
            source: input.source,
            scene_id: input.scene_id,
            scope_id: input.scope_id,
            presence_revision: input.presence_revision,
            rule_id: rule.rule_id.clone(),
            rule_version: rule.rule_version,
        };
        Ok(WorldMutation {
            mutation_id,
            data: MutationData {
                version: 1,
                kind: "observed".into(),
                payload: aoidos_json::to_value(event).map_err(|_| Fault::bad_request())?,
            },
        })
    }
}

impl RumorDelivery {
    /// 构造单次带父链链接的传闻交付 mutation。
    /// # Errors
    /// 事件无法编码为规范载荷时拒绝。
    #[allow(clippy::too_many_arguments)]
    pub fn mutation(
        session: &Session,
        catalog: &SceneCatalog,
        recipient_id: String,
        informant_id: String,
        delivery_source: EvidenceSource,
        root_source: EvidenceSource,
        previous_evidence_id: Option<String>,
        scene_id: String,
        scope_id: String,
        uncertainty: RumorUncertainty,
        rule: &RumorRule,
    ) -> Result<WorldMutation, Fault> {
        let mutation_id = uuid::Uuid::new_v4().to_string();
        let event = Self {
            version: 1,
            evidence_id: mutation_id.clone(),
            session_id: session.header().session_id.clone(),
            script_revision: session.header().script_revision.clone(),
            catalog_revision: catalog.revision().to_owned(),
            recipient_id,
            informant_id,
            delivery_source,
            root_source,
            previous_evidence_id,
            scene_id,
            scope_id,
            uncertainty,
            rule_id: rule.rule_id.clone(),
            rule_version: rule.rule_version,
        };
        Ok(WorldMutation {
            mutation_id,
            data: MutationData {
                version: 1,
                kind: "rumorDelivered".into(),
                payload: aoidos_json::to_value(event).map_err(|_| Fault::bad_request())?,
            },
        })
    }
}

pub(super) fn validate_ordered(
    connection: &rusqlite::Connection,
    state: &mut State,
    session: &Session,
    catalog: &SceneCatalog,
    directory: &EntityDirectory,
    policy: &PresencePolicy,
    validation_revision: u64,
    lookup_before_seq: Option<u64>,
    mutations: &[WorldMutation],
) -> Result<(), Fault> {
    let path = session.path_at(session.history_revision())?;
    let positions = path
        .iter()
        .enumerate()
        .map(|(index, seq)| (*seq, index))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeMap::<String, PendingRumor>::new();
    let mut identities = BTreeSet::new();
    for mutation in mutations {
        match mutation.data.kind.as_str() {
            "scenePresenceChanged" => super::apply_presence_change(
                state,
                mutation,
                policy,
                directory,
                session,
                catalog,
                validation_revision,
            )?,
            "observed" => {
                let event: ObservedEvent = decode(mutation)?;
                let context = EvidenceValidationContext {
                    connection,
                    session,
                    catalog,
                    directory,
                    policy,
                    positions: &positions,
                    successful_sources: None,
                    records: None,
                    lookup_before_seq,
                };
                validate_observed(&context, &event, mutation, state)?;
                if !identities.insert(event.evidence_id) {
                    return Err(super::invalid_phase());
                }
            }
            "rumorDelivered" => {
                let event: RumorDelivery = decode(mutation)?;
                let context = EvidenceValidationContext {
                    connection,
                    session,
                    catalog,
                    directory,
                    policy,
                    positions: &positions,
                    successful_sources: None,
                    records: None,
                    lookup_before_seq,
                };
                let pending =
                    validate_rumor(&context, &event, mutation, state, &seen, &identities)?;
                seen.insert(event.evidence_id.clone(), pending);
                if !identities.insert(event.evidence_id) {
                    return Err(super::invalid_phase());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

struct EvidenceValidationContext<'a> {
    connection: &'a rusqlite::Connection,
    session: &'a Session,
    catalog: &'a SceneCatalog,
    directory: &'a EntityDirectory,
    policy: &'a PresencePolicy,
    positions: &'a BTreeMap<u64, usize>,
    successful_sources: Option<&'a BTreeSet<u64>>,
    /// 只读索引在 applied/hash 与成功回合核验后提供缓存，信任应用边界的在场证明。
    records: Option<&'a BTreeMap<u64, crate::record::format::Parsed>>,
    lookup_before_seq: Option<u64>,
}

impl EvidenceValidationContext<'_> {
    fn record(&self, seq: u64) -> Result<crate::record::format::Record, Fault> {
        match self.records {
            Some(records) => records
                .get(&seq)
                .and_then(|parsed| parsed.body.clone())
                .ok_or_else(super::invalid_phase),
            None => self
                .session
                .read(seq)?
                .body
                .ok_or_else(super::invalid_phase),
        }
    }
}

fn validate_observed(
    context: &EvidenceValidationContext<'_>,
    event: &ObservedEvent,
    mutation: &WorldMutation,
    state: &State,
) -> Result<(), Fault> {
    let rule = context
        .policy
        .observation_rule(&event.rule_id)
        .ok_or_else(super::invalid_phase)?;
    if rule.source_kind != super::ObservationSourceKind::SceneNarration {
        return Err(super::invalid_phase());
    }
    if mutation.data.version != 1
        || event.version != 1
        || event.evidence_id != mutation.mutation_id
        || event.session_id != context.session.header().session_id
        || event.script_revision != context.session.header().script_revision
        || event.catalog_revision != context.catalog.revision()
        || event.rule_version != rule.rule_version
        || event.scene_id != state.scene_id
        || event.scene_id != rule.scene_id
        || event.scope_id != rule.scope_id
        || event.presence_revision != state.revision
        || !state.members.contains(&event.subject_id)
        || context
            .directory
            .entity(&event.subject_id)?
            .is_none_or(|entity| entity.kind != EntityKind::Character)
    {
        return Err(super::invalid_phase());
    }
    validate_observation_source(
        context,
        &event.source,
        context.session.next_sequence(),
        &event.scene_id,
        &event.subject_id,
    )?;
    Ok(())
}

fn validate_rumor(
    context: &EvidenceValidationContext<'_>,
    event: &RumorDelivery,
    mutation: &WorldMutation,
    state: &State,
    same_group: &BTreeMap<String, PendingRumor>,
    group_ids: &BTreeSet<String>,
) -> Result<PendingRumor, Fault> {
    let rule = context
        .policy
        .rumor_rule(&event.rule_id)
        .ok_or_else(super::invalid_phase)?;
    let receiver_present = state.members.contains(&event.recipient_id);
    let sender_present =
        !rule.require_informant_present || state.members.contains(&event.informant_id);
    if mutation.data.version != 1
        || event.version != 1
        || event.evidence_id != mutation.mutation_id
        || event.session_id != context.session.header().session_id
        || event.script_revision != context.session.header().script_revision
        || event.catalog_revision != context.catalog.revision()
        || event.rule_version != rule.rule_version
        || event.scene_id != state.scene_id
        || event.scene_id != rule.scene_id
        || event.scope_id != rule.scope_id
        || !receiver_present
        || !sender_present
        || context
            .directory
            .entity(&event.recipient_id)?
            .is_none_or(|entity| entity.kind != EntityKind::Character)
        || context
            .directory
            .entity(&event.informant_id)?
            .is_none_or(|entity| entity.kind != EntityKind::Character)
    {
        return Err(super::invalid_phase());
    }
    let delivery_author = validate_rumor_source(
        context,
        &event.delivery_source,
        context.session.next_sequence(),
        &event.scene_id,
        &event.informant_id,
    )?;
    if delivery_author != event.informant_id {
        return Err(super::invalid_phase());
    }
    let root_author = validate_source(
        context,
        &event.root_source,
        context.session.next_sequence(),
        None,
    )?
    .and_then(|record| author_id(&record.body));
    let pending = match event.previous_evidence_id.as_deref() {
        None if root_author.as_deref() == Some(&event.informant_id) => {
            validate_rumor_source(
                context,
                &event.root_source,
                context.session.next_sequence(),
                &event.scene_id,
                &event.informant_id,
            )?;
            PendingRumor {
                recipient_id: event.recipient_id.clone(),
                root_source: event.root_source.clone(),
                scope_id: event.scope_id.clone(),
                hops: 1,
                root_informant: event.informant_id.clone(),
                chain: BTreeSet::from([event.informant_id.clone(), event.recipient_id.clone()]),
            }
        }
        None => return Err(super::invalid_phase()),
        Some(parent_id) if group_ids.contains(parent_id) => {
            let parent = same_group.get(parent_id).ok_or_else(super::invalid_phase)?;
            if parent.recipient_id != event.informant_id
                || parent.root_source != event.root_source
                || parent.scope_id != event.scope_id
                || parent.hops >= MAX_RUMOR_HOPS
                || parent.chain.contains(&event.recipient_id)
            {
                return Err(super::invalid_phase());
            }
            extend_pending_rumor(parent, event)?
        }
        Some(parent_id) => {
            let Some(parent) = find_trusted_evidence(
                context.connection,
                context.session,
                context.catalog,
                context.directory,
                parent_id,
                context.lookup_before_seq,
            )?
            else {
                return Err(super::invalid_phase());
            };
            let TrustedPayload::Rumor {
                event: parent,
                root_informant,
                hops,
                chain,
            } = parent
            else {
                return Err(super::invalid_phase());
            };
            if parent.recipient_id != event.informant_id
                || parent.root_source != event.root_source
                || parent.scope_id != event.scope_id
                || hops >= MAX_RUMOR_HOPS
                || chain.contains(&event.recipient_id)
                || root_author.as_deref() != Some(root_informant.as_str())
            {
                return Err(super::invalid_phase());
            }
            extend_pending_rumor(
                &PendingRumor {
                    recipient_id: parent.recipient_id,
                    root_source: parent.root_source,
                    scope_id: parent.scope_id,
                    hops,
                    root_informant,
                    chain,
                },
                event,
            )?
        }
    };
    Ok(pending)
}

fn validate_observation_source(
    context: &EvidenceValidationContext<'_>,
    source: &EvidenceSource,
    before_seq: u64,
    scene_id: &str,
    subject_id: &str,
) -> Result<(), Fault> {
    let record =
        validate_source(context, source, before_seq, None)?.ok_or_else(super::invalid_phase)?;
    if !matches!(
        record.body,
        Body::Narration {
            terminal: Terminal::Completed { .. },
            ..
        }
    ) {
        return Err(super::invalid_phase());
    }
    if context.records.is_none() {
        let state = super::derive_before(
            context.connection,
            context.session,
            context.catalog,
            context.directory,
            source.record_seq,
            record.branch_seq.unwrap_or(0),
        )?;
        if state.scene_id != scene_id || !state.members.contains(subject_id) {
            return Err(super::invalid_phase());
        }
    }
    Ok(())
}

fn validate_rumor_source(
    context: &EvidenceValidationContext<'_>,
    source: &EvidenceSource,
    before_seq: u64,
    scene_id: &str,
    character_id: &str,
) -> Result<String, Fault> {
    let record =
        validate_source(context, source, before_seq, None)?.ok_or_else(super::invalid_phase)?;
    let author = author_id(&record.body).ok_or_else(super::invalid_phase)?;
    if author != character_id {
        return Err(super::invalid_phase());
    }
    if context.records.is_none() {
        let state = super::derive_before(
            context.connection,
            context.session,
            context.catalog,
            context.directory,
            source.record_seq,
            record.branch_seq.unwrap_or(0),
        )?;
        if state.scene_id != scene_id || !state.members.contains(character_id) {
            return Err(super::invalid_phase());
        }
    }
    Ok(author)
}

fn extend_pending_rumor(
    parent: &PendingRumor,
    event: &RumorDelivery,
) -> Result<PendingRumor, Fault> {
    let mut chain = parent.chain.clone();
    if !chain.insert(event.recipient_id.clone()) || parent.hops >= MAX_RUMOR_HOPS {
        return Err(super::invalid_phase());
    }
    Ok(PendingRumor {
        recipient_id: event.recipient_id.clone(),
        root_source: event.root_source.clone(),
        scope_id: event.scope_id.clone(),
        hops: parent.hops + 1,
        root_informant: parent.root_informant.clone(),
        chain,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRumor {
    recipient_id: String,
    root_source: EvidenceSource,
    scope_id: String,
    hops: usize,
    root_informant: String,
    chain: BTreeSet<String>,
}
fn decode<T: for<'de> Deserialize<'de>>(mutation: &WorldMutation) -> Result<T, Fault> {
    if mutation.data.version != 1 {
        return Err(Fault::bad_request());
    }
    aoidos_json::from_value(mutation.data.payload.clone()).map_err(|_| Fault::bad_request())
}

fn validate_source(
    context: &EvidenceValidationContext<'_>,
    source: &EvidenceSource,
    before_seq: u64,
    expected_author: Option<&str>,
) -> Result<Option<crate::record::format::Record>, Fault> {
    if !context.positions.contains_key(&source.record_seq) {
        return Err(super::invalid_phase());
    }
    if source.record_seq >= before_seq
        || !format::valid_hash(&source.body_hash)
        || source.start_byte >= source.end_byte
        || source.end_byte - source.start_byte > MAX_SOURCE_RANGE
    {
        return Err(super::invalid_phase());
    }
    let record = context.record(source.record_seq)?;
    let text = body_text(&record.body).ok_or_else(super::invalid_phase)?;
    if format::hash(text.as_bytes()) != source.body_hash
        || source.end_byte > text.len() as u64
        || text
            .get(source.start_byte as usize..source.end_byte as usize)
            .is_none()
        || expected_author
            .is_some_and(|expected| author_id(&record.body).as_deref() != Some(expected))
    {
        return Err(super::invalid_phase());
    }
    Ok(Some(record))
}

fn body_text(body: &Body) -> Option<&str> {
    match body {
        Body::PlayerSpeech { text, .. } => Some(text),
        Body::Narration {
            text,
            terminal: Terminal::Completed { .. },
            ..
        }
        | Body::CharacterSpeech {
            text,
            terminal: Terminal::Completed { .. },
            ..
        } => Some(text),
        _ => None,
    }
}

fn author_id(body: &Body) -> Option<String> {
    match body {
        Body::PlayerSpeech {
            mode: Some(crate::record::format::InputMode::OutOfCharacter),
            ..
        } => None,
        Body::PlayerSpeech { player_id, .. } => Some(player_id.clone()),
        Body::CharacterSpeech {
            speaker_id,
            terminal: Terminal::Completed { .. },
            ..
        } => Some(speaker_id.clone()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
enum TrustedPayload {
    Observed(ObservedEvent),
    Rumor {
        event: RumorDelivery,
        root_informant: String,
        hops: usize,
        chain: BTreeSet<String>,
    },
}

#[derive(Debug, Clone)]
struct IndexedEvent {
    seq: u64,
    order_index: usize,
    payload: TrustedPayload,
}

fn find_trusted_evidence(
    connection: &rusqlite::Connection,
    session: &Session,
    catalog: &SceneCatalog,
    directory: &EntityDirectory,
    evidence_id: &str,
    before_seq: Option<u64>,
) -> Result<Option<TrustedPayload>, Fault> {
    let records = match before_seq {
        None => session.bounded_current_records(MAX_EVIDENCE_RECORDS, MAX_EVIDENCE_BYTES)?,
        Some(before) => session.bounded_records_before(
            MAX_EVIDENCE_RECORDS,
            MAX_EVIDENCE_BYTES,
            Some(before),
        )?,
    };
    let path = records.keys().copied().collect::<Vec<_>>();
    let path = path.as_slice();
    let positions = path
        .iter()
        .enumerate()
        .map(|(index, seq)| (*seq, index))
        .collect::<BTreeMap<_, _>>();
    let policy = catalog.presence_policy().ok_or_else(super::invalid_phase)?;
    let mut events = BTreeMap::<String, IndexedEvent>::new();
    let mut accepted = BTreeMap::<(String, String), (u64, u64)>::new();
    let mut ended = Vec::<(
        u64,
        crate::record::facts::RoundIdentity,
        crate::ports::Outcome,
        u64,
    )>::new();
    let mut candidates = Vec::new();
    for (seq, record) in &records {
        let Some(record) = &record.body else { continue };
        let Body::System { code, data, .. } = &record.body else {
            continue;
        };
        match Fact::decode(code, data).map_err(Fault::from)? {
            Fact::RoundAccepted {
                identity,
                input_seq,
                ..
            } => {
                accepted.insert(
                    (identity.round_id, identity.operation_id),
                    (*seq, input_seq),
                );
            }
            Fact::RoundEnded {
                identity,
                outcome,
                through_seq,
                ..
            } => {
                ended.push((*seq, identity, outcome, through_seq));
            }
            Fact::SettlementPlanned {
                identity,
                mutations,
                ..
            } => {
                for (order_index, mutation) in mutations.into_iter().enumerate() {
                    let payload = match mutation.data.kind.as_str() {
                        "observed" => TrustedPayload::Observed(decode(&mutation)?),
                        "rumorDelivered" => TrustedPayload::Rumor {
                            event: decode(&mutation)?,
                            root_informant: String::new(),
                            hops: 0,
                            chain: BTreeSet::new(),
                        },
                        _ => continue,
                    };
                    candidates.push((*seq, order_index, identity.clone(), mutation, payload));
                }
            }
            _ => {}
        }
    }
    for (seq, order_index, identity, mutation, payload) in candidates {
        if mutation.data.version != 1
            || !aoidos_store::applied::contains(
                connection,
                &mutation.mutation_id,
                &crate::record::world::mutation_hash(&mutation)?,
            )
            .map_err(Fault::from)?
            || !successful_round(
                &records, path, &positions, seq, &identity, &accepted, &ended,
            )?
        {
            continue;
        }
        if events
            .insert(
                mutation.mutation_id.clone(),
                IndexedEvent {
                    seq,
                    order_index,
                    payload,
                },
            )
            .is_some()
        {
            return Err(super::invalid_phase());
        }
    }
    let Some(found) = events.get(evidence_id).cloned() else {
        return Ok(None);
    };
    let mut visiting = BTreeSet::new();
    let mut recipient_chain = BTreeSet::new();
    let successful_sources =
        successful_source_records(&records, path, &positions, &accepted, &ended)?;
    let context = EvidenceValidationContext {
        connection,
        session,
        catalog,
        directory,
        policy,
        positions: &positions,
        successful_sources: Some(&successful_sources),
        records: Some(&records),
        lookup_before_seq: before_seq,
    };
    let chain_info = validate_indexed_event(
        evidence_id,
        &events,
        &context,
        &mut visiting,
        &mut recipient_chain,
        0,
    )?;
    let payload = match found.payload {
        TrustedPayload::Observed(event) => TrustedPayload::Observed(event),
        TrustedPayload::Rumor { event, .. } => {
            let (root_informant, hops) = chain_info.ok_or_else(super::invalid_phase)?;
            TrustedPayload::Rumor {
                event,
                root_informant,
                hops,
                chain: recipient_chain,
            }
        }
    };
    Ok(Some(payload))
}

fn validate_indexed_event(
    evidence_id: &str,
    events: &BTreeMap<String, IndexedEvent>,
    context: &EvidenceValidationContext<'_>,
    visiting: &mut BTreeSet<String>,
    recipients: &mut BTreeSet<String>,
    depth: usize,
) -> Result<Option<(String, usize)>, Fault> {
    if depth > MAX_RUMOR_HOPS || !visiting.insert(evidence_id.to_owned()) {
        return Err(super::invalid_phase());
    }
    let indexed = events.get(evidence_id).ok_or_else(super::invalid_phase)?;
    match &indexed.payload {
        TrustedPayload::Observed(event) => {
            let rule = context
                .policy
                .observation_rule(&event.rule_id)
                .ok_or_else(super::invalid_phase)?;
            if event.version != 1
                || event.evidence_id != evidence_id
                || event.session_id != context.session.header().session_id
                || event.script_revision != context.session.header().script_revision
                || event.catalog_revision != context.catalog.revision()
                || event.rule_version != rule.rule_version
                || event.scene_id != rule.scene_id
                || event.scope_id != rule.scope_id
                || rule.source_kind != super::ObservationSourceKind::SceneNarration
                || !format::valid_hash(&event.presence_revision)
                || context
                    .directory
                    .entity(&event.subject_id)?
                    .is_none_or(|entity| entity.kind != EntityKind::Character)
            {
                return Err(super::invalid_phase());
            }
            if !context
                .successful_sources
                .is_some_and(|sources| sources.contains(&event.source.record_seq))
            {
                return Err(super::invalid_phase());
            }
            validate_observation_source(
                context,
                &event.source,
                indexed.seq,
                &event.scene_id,
                &event.subject_id,
            )?;
            visiting.remove(evidence_id);
            Ok(None)
        }
        TrustedPayload::Rumor { event, .. } => {
            let rule = context
                .policy
                .rumor_rule(&event.rule_id)
                .ok_or_else(super::invalid_phase)?;
            if event.version != 1
                || event.evidence_id != evidence_id
                || event.session_id != context.session.header().session_id
                || event.script_revision != context.session.header().script_revision
                || event.catalog_revision != context.catalog.revision()
                || event.rule_version != rule.rule_version
                || event.scene_id != rule.scene_id
                || event.scope_id != rule.scope_id
                || context
                    .directory
                    .entity(&event.recipient_id)?
                    .is_none_or(|entity| entity.kind != EntityKind::Character)
                || context
                    .directory
                    .entity(&event.informant_id)?
                    .is_none_or(|entity| entity.kind != EntityKind::Character)
                || !recipients.insert(event.recipient_id.clone())
            {
                return Err(super::invalid_phase());
            }
            let delivery = validate_rumor_source(
                context,
                &event.delivery_source,
                indexed.seq,
                &event.scene_id,
                &event.informant_id,
            )?;
            let root = validate_source(context, &event.root_source, indexed.seq, None)?
                .and_then(|record| author_id(&record.body));
            if delivery != event.informant_id {
                return Err(super::invalid_phase());
            }
            if !context.successful_sources.is_some_and(|sources| {
                sources.contains(&event.delivery_source.record_seq)
                    && sources.contains(&event.root_source.record_seq)
            }) {
                return Err(super::invalid_phase());
            }
            let chain_info = if let Some(parent_id) = &event.previous_evidence_id {
                let parent = events.get(parent_id).ok_or_else(super::invalid_phase)?;
                if (indexed.seq, indexed.order_index) <= (parent.seq, parent.order_index) {
                    return Err(super::invalid_phase());
                }
                let TrustedPayload::Rumor { event: parent, .. } = &parent.payload else {
                    return Err(super::invalid_phase());
                };
                if parent.recipient_id != event.informant_id
                    || parent.root_source != event.root_source
                    || parent.scope_id != event.scope_id
                {
                    return Err(super::invalid_phase());
                }
                let parent_chain = validate_indexed_event(
                    parent_id,
                    events,
                    context,
                    visiting,
                    recipients,
                    depth + 1,
                )?
                .ok_or_else(super::invalid_phase)?;
                if root.as_deref() != Some(parent_chain.0.as_str()) {
                    return Err(super::invalid_phase());
                }
                (parent_chain.0, parent_chain.1 + 1)
            } else if root.as_deref() != Some(&event.informant_id) {
                return Err(super::invalid_phase());
            } else {
                if !recipients.insert(event.informant_id.clone()) {
                    return Err(super::invalid_phase());
                }
                validate_rumor_source(
                    context,
                    &event.root_source,
                    indexed.seq,
                    &event.scene_id,
                    &event.informant_id,
                )?;
                (event.informant_id.clone(), 1)
            };
            if chain_info.1 > MAX_RUMOR_HOPS {
                return Err(super::invalid_phase());
            }
            visiting.remove(evidence_id);
            Ok(Some(chain_info))
        }
    }
}

fn successful_source_records(
    records: &BTreeMap<u64, crate::record::format::Parsed>,
    path: &[u64],
    positions: &BTreeMap<u64, usize>,
    accepted: &BTreeMap<(String, String), (u64, u64)>,
    ended: &[(
        u64,
        crate::record::facts::RoundIdentity,
        crate::ports::Outcome,
        u64,
    )],
) -> Result<BTreeSet<u64>, Fault> {
    let mut sources = BTreeSet::new();
    for ((round_id, operation_id), &(accepted_seq, input_seq)) in accepted {
        let Some(&input_index) = positions.get(&input_seq) else {
            continue;
        };
        let Some(&accepted_index) = positions.get(&accepted_seq) else {
            continue;
        };
        if accepted_index < input_index {
            continue;
        }
        let Some((end_seq, _, crate::ports::Outcome::Completed, through_seq)) =
            ended.iter().find(|(_, identity, outcome, _)| {
                *outcome == crate::ports::Outcome::Completed
                    && identity.round_id == *round_id
                    && identity.operation_id == *operation_id
            })
        else {
            continue;
        };
        let Some(&end_index) = positions.get(end_seq) else {
            continue;
        };
        if end_index < accepted_index {
            continue;
        }
        let round_path = path[input_index..=end_index].to_vec();
        let round = crate::game::recovery::derive_evidence_records(records, round_path, *end_seq)?;
        if round.round.is_some_and(|round| {
            round.identity.round_id == *round_id
                && round.identity.operation_id == *operation_id
                && round.input_seq == input_seq
                && round.advance == Some(*through_seq)
                && round
                    .ended
                    .as_ref()
                    .is_some_and(|(_, outcome, _)| *outcome == crate::ports::Outcome::Completed)
        }) {
            sources.extend(path[input_index..=end_index].iter().copied());
        }
    }
    Ok(sources)
}

fn successful_round(
    records: &BTreeMap<u64, crate::record::format::Parsed>,
    path: &[u64],
    positions: &BTreeMap<u64, usize>,
    settlement_seq: u64,
    identity: &crate::record::facts::RoundIdentity,
    accepted: &BTreeMap<(String, String), (u64, u64)>,
    ended: &[(
        u64,
        crate::record::facts::RoundIdentity,
        crate::ports::Outcome,
        u64,
    )],
) -> Result<bool, Fault> {
    let key = (identity.round_id.clone(), identity.operation_id.clone());
    let Some(&(accepted_seq, input_seq)) = accepted.get(&key) else {
        return Ok(false);
    };
    if accepted_seq >= settlement_seq {
        return Ok(false);
    }
    let Some(&input_index) = positions.get(&input_seq) else {
        return Ok(false);
    };
    let Some((end_seq, _, crate::ports::Outcome::Completed, through_seq)) =
        ended.iter().find(|(seq, ended_id, outcome, _)| {
            *seq > settlement_seq
                && *outcome == crate::ports::Outcome::Completed
                && ended_id.round_id == identity.round_id
                && ended_id.operation_id == identity.operation_id
        })
    else {
        return Ok(false);
    };
    let Some(&end_index) = positions.get(end_seq) else {
        return Ok(false);
    };
    if end_index < input_index {
        return Ok(false);
    }
    let evidence_path = path[input_index..=end_index].to_vec();
    let recovered =
        crate::game::recovery::derive_evidence_records(records, evidence_path, *end_seq)?;
    Ok(recovered.round.is_some_and(|round| {
        round.identity.round_id == identity.round_id
            && round.identity.operation_id == identity.operation_id
            && round.advance == Some(*through_seq)
            && round
                .ended
                .as_ref()
                .is_some_and(|(_, outcome, _)| *outcome == crate::ports::Outcome::Completed)
    }))
}
