//! 模型候选的有界协议与纯白名单校验；不查询存储、不提交记忆。

use super::{
    Attitude, CycleKind, CycleMarkV1, CycleSourceRef, DetailPolicy, Dimension,
    MAX_CYCLE_MARK_PROPOSALS, MAX_CYCLE_SOURCES, MAX_CYCLE_SUBJECTS, MAX_CYCLE_SUMMARIES_BYTES,
    MAX_CYCLE_SUMMARY_BYTES, MAX_INTEGER, SourceRef, Subject, reject, valid_id, valid_name,
    valid_script, valid_short_id,
};
use crate::error::{Error, Reason, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PROPOSAL_BYTES: usize = 64 * 1024;
pub const MAX_CANDIDATES: usize = 128;
pub const MAX_REFERENCES: usize = 128;
pub const MAX_PROPOSAL_TARGETS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Classification {
    Repeat,
    Supplement,
    New,
    Correction,
    NotWorthy,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum SubjectRef {
    PlayerPreference,
    Character { id: String },
}
impl SubjectRef {
    fn matches_subject(&self, subject: &Subject) -> bool {
        match (self, subject) {
            (Self::PlayerPreference, Subject::PlayerPreference) => true,
            (Self::Character { id: left }, Subject::Character { id: right }) => left == right,
            _ => false,
        }
    }
    fn evidence_subject(&self) -> Option<&str> {
        match self {
            Self::PlayerPreference => None,
            Self::Character { id } => Some(id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetRef {
    pub entry_id: String,
    pub expected_version_id: String,
    pub expected_state_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Candidate {
    pub candidate_id: String,
    pub classification: Classification,
    pub source_ids: Vec<String>,
    pub subject_ref: SubjectRef,
    #[serde(default)]
    pub entity_ids: Vec<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<TargetRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension: Option<Dimension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attitude: Option<Attitude>,
    #[serde(default)]
    pub attitude_evidence_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred: Option<bool>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Proposal {
    pub version: u32,
    pub candidates: Vec<Candidate>,
    /// 与普通记忆候选共用同一次请求；没有获授权规则时必须为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycle_marks: Vec<CycleMarkCandidate>,
}

/// 只包含冻结批次中已经登记且可信的身份，不接收来自候选的目录或证据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposalContext {
    pub source_ids: BTreeSet<String>,
    pub entity_ids: BTreeSet<String>,
    /// 与实体 ID 一一对应的可信目录信息，供模型关联正文名称而不猜测身份。
    #[serde(default)]
    pub entities: BTreeMap<String, EntitySnapshot>,
    pub subjects: BTreeSet<SubjectRef>,
    pub evidence: BTreeMap<String, EvidenceSnapshot>,
    pub targets: BTreeMap<String, TargetSnapshot>,
    /// 只包含当前剧本、当前周目及当前记忆批次可使用的刻痕授权。
    pub cycle_rules: BTreeMap<String, CycleMarkAuthorization>,
    /// 批次短 ID 到已核验原始来源的映射。
    pub cycle_sources: BTreeMap<String, CycleSourceRef>,
}

/// 由当前完整实体目录冻结的可展示身份信息；map key 是稳定 entityId。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntitySnapshot {
    pub kind: crate::model::EntityCategory,
    pub scope: String,
    pub canonical_name: String,
    /// 仅包含该周目经明确确认、来源仍有效的精确别名。
    #[serde(default)]
    pub aliases: BTreeSet<String>,
}

/// 模型只提供摘要和白名单引用；持久刻痕身份及权限字段由引擎生成。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleMarkCandidate {
    pub candidate_id: String,
    pub rule_id: String,
    pub source_ids: Vec<String>,
    pub subject_ids: Vec<String>,
    pub summary: String,
}

/// 由可信剧本与当前周目构造的授权快照，绝不能直接由模型填充。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleMarkAuthorization {
    pub rule_id: String,
    pub script_id: String,
    pub run_id: String,
    pub cycle_gen: u64,
    pub rule_version: u32,
    pub allowed_subject_ids: BTreeSet<String>,
    pub imprint_key: String,
    pub kind: CycleKind,
    pub detail_policy: DetailPolicy,
    pub priority: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceSnapshot {
    /// 场景观察 / 传闻对应的世界 mutation ID；定向对话的引擎内证据没有独立事件身份。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    pub subject_id: String,
    /// 为完整引用该证据来源范围所需的批次素材；顺序按原文区间排列。
    pub source_ids: Vec<String>,
    pub kind: super::KnowledgeKind,
    /// 传闻的不确定状态；观察和定向对话不允许携带此字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<RumorUncertainty>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub informant_id: Option<String>,
    pub scope: String,
    pub rule_id: String,
    pub source: SourceRef,
}

/// 当前仍有效的传闻可信状态，不等同于世界事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RumorUncertainty {
    Unverified,
    Disputed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetSnapshot {
    pub entry_id: String,
    pub version_id: String,
    pub state_revision: u64,
    pub state: super::EntryState,
    pub subject: Subject,
    pub dimension: Dimension,
    pub summary: String,
    pub attitude: Attitude,
    pub inferred: bool,
}

impl Proposal {
    /// 先执行有界 JSON 解码，再针对本批冻结白名单校验候选。
    ///
    /// # Errors
    /// 非法 JSON、重复键、未知字段、越界候选或任一不满足 schema 的候选均拒绝整份响应。
    pub fn parse(bytes: &[u8], context: &ProposalContext) -> Result<Self> {
        let proposal: Self =
            aoidos_json::decode_bytes(bytes, MAX_PROPOSAL_BYTES).map_err(|_| reject())?;
        proposal.validate(context)?;
        Ok(proposal)
    }

    /// # Errors
    /// 未支持版本、重复候选身份、未知来源 / 证据 / 实体和过期目标均拒绝。
    pub fn validate(&self, context: &ProposalContext) -> Result<()> {
        if self.version != 1 {
            return Err(Error::Rejected(Reason::PolicyUnavailable));
        }
        validate_proposal_identity(self)?;
        for candidate in &self.candidates {
            validate_candidate(candidate, context)?;
        }
        let mut mark_identities = BTreeSet::new();
        for candidate in &self.cycle_marks {
            let rule = context
                .cycle_rules
                .get(&candidate.rule_id)
                .ok_or_else(reject)?;
            validate_cycle_mark_candidate(candidate, rule, context)?;
            let mut subject_ids = candidate.subject_ids.clone();
            subject_ids.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            if !mark_identities.insert((candidate.rule_id.as_str(), subject_ids)) {
                return Err(reject());
            }
        }
        Ok(())
    }
}

/// 校验与本批白名单无关的基本协议结构，供回执写入前拒绝损坏候选。
pub(crate) fn validate_proposal_identity(proposal: &Proposal) -> Result<()> {
    if proposal.version != 1 {
        return Err(Error::Rejected(Reason::PolicyUnavailable));
    }
    if proposal.candidates.len() + proposal.cycle_marks.len() > MAX_CANDIDATES
        || proposal.cycle_marks.len() > MAX_CYCLE_MARK_PROPOSALS
    {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    if proposal.cycle_marks.iter().fold(0usize, |total, mark| {
        total.saturating_add(mark.summary.len())
    }) > MAX_CYCLE_SUMMARIES_BYTES
    {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    let mut candidate_ids = BTreeSet::new();
    for id in proposal
        .candidates
        .iter()
        .map(|candidate| candidate.candidate_id.as_str())
        .chain(
            proposal
                .cycle_marks
                .iter()
                .map(|candidate| candidate.candidate_id.as_str()),
        )
    {
        if !valid_short_id(id) || !candidate_ids.insert(id) {
            return Err(reject());
        }
    }
    for candidate in &proposal.cycle_marks {
        validate_cycle_mark_shape(candidate)?;
    }
    Ok(())
}

impl CycleMarkCandidate {
    /// 在完整授权白名单下生成持久刻痕；模型不能指定权重、类型或稳定身份。
    /// # Errors
    /// 规则、实体、来源无权或载荷超限时拒绝生成。
    pub fn materialize(&self, context: &ProposalContext) -> Result<CycleMarkV1> {
        let rule = context.cycle_rules.get(&self.rule_id).ok_or_else(reject)?;
        validate_cycle_mark_candidate(self, rule, context)?;
        let mut subject_ids = self.subject_ids.clone();
        subject_ids.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        let mut source_refs = self
            .source_ids
            .iter()
            .map(|source_id| {
                context
                    .cycle_sources
                    .get(source_id)
                    .cloned()
                    .ok_or_else(reject)
            })
            .collect::<Result<Vec<_>>>()?;
        source_refs.sort_by(|left, right| source_order(&left.source, &right.source));
        let mut mark = CycleMarkV1 {
            version: 1,
            mark_id: String::new(),
            script_id: rule.script_id.clone(),
            cycle_gen: rule.cycle_gen,
            rule_id: rule.rule_id.clone(),
            rule_version: rule.rule_version,
            subject_ids,
            imprint_key: rule.imprint_key.clone(),
            source_refs,
            kind: rule.kind,
            detail_policy: rule.detail_policy,
            priority: rule.priority,
            summary: self.summary.clone(),
        };
        mark.mark_id = mark.stable_mark_id()?;
        mark.validate()?;
        Ok(mark)
    }
}

fn validate_cycle_mark_candidate(
    candidate: &CycleMarkCandidate,
    rule: &CycleMarkAuthorization,
    context: &ProposalContext,
) -> Result<()> {
    validate_cycle_mark_shape(candidate)?;
    if rule.rule_id != candidate.rule_id
        || !valid_name(&rule.rule_id)
        || !valid_script(&rule.script_id)
        || !valid_id(&rule.run_id)
        || rule.cycle_gen == 0
        || rule.cycle_gen > MAX_INTEGER
        || rule.rule_version == 0
        || rule.rule_version as u64 > MAX_INTEGER
        || !valid_name(&rule.imprint_key)
        || rule.priority > 1_000
        || rule.allowed_subject_ids.is_empty()
        || rule.allowed_subject_ids.len() > MAX_CYCLE_SUBJECTS * MAX_CANDIDATES
        || rule
            .allowed_subject_ids
            .iter()
            .any(|id| !valid_name(id) || !context.entity_ids.contains(id))
        || candidate
            .subject_ids
            .iter()
            .any(|id| !rule.allowed_subject_ids.contains(id))
        || candidate.source_ids.iter().any(|id| {
            !context.source_ids.contains(id)
                || context.cycle_sources.get(id).is_none_or(|source| {
                    source.source.script_id != rule.script_id
                        || source.source.run_id != rule.run_id
                        || source.history_revision > MAX_INTEGER
                        || source.source.validate().is_err()
                })
        })
    {
        return Err(reject());
    }
    Ok(())
}

fn validate_cycle_mark_shape(candidate: &CycleMarkCandidate) -> Result<()> {
    if !valid_short_id(&candidate.candidate_id)
        || !valid_name(&candidate.rule_id)
        || candidate.source_ids.is_empty()
        || candidate.source_ids.len() > MAX_CYCLE_SOURCES
        || candidate.subject_ids.is_empty()
        || candidate.subject_ids.len() > MAX_CYCLE_SUBJECTS
        || candidate.source_ids.iter().any(|id| !valid_short_id(id))
        || candidate.subject_ids.iter().any(|id| !valid_name(id))
        || candidate.source_ids.iter().collect::<BTreeSet<_>>().len() != candidate.source_ids.len()
        || candidate.subject_ids.iter().collect::<BTreeSet<_>>().len()
            != candidate.subject_ids.len()
        || candidate.summary.trim().is_empty()
        || candidate.summary.len() > MAX_CYCLE_SUMMARY_BYTES
        || candidate.summary.contains('\0')
    {
        return Err(reject());
    }
    Ok(())
}

fn source_order(left: &SourceRef, right: &SourceRef) -> std::cmp::Ordering {
    left.run_id
        .as_bytes()
        .cmp(right.run_id.as_bytes())
        .then_with(|| left.session_id.as_bytes().cmp(right.session_id.as_bytes()))
        .then_with(|| left.record_seq.cmp(&right.record_seq))
        .then_with(|| left.start_byte.cmp(&right.start_byte))
        .then_with(|| left.end_byte.cmp(&right.end_byte))
        .then_with(|| left.body_hash.cmp(&right.body_hash))
}

fn validate_candidate(candidate: &Candidate, context: &ProposalContext) -> Result<()> {
    if !valid_short_id(&candidate.candidate_id)
        || candidate.reason.trim().is_empty()
        || candidate.reason.len() > 512
        || candidate.reason.contains('\0')
        || candidate.source_ids.is_empty()
        || candidate.source_ids.len() > MAX_REFERENCES
        || candidate.entity_ids.len() > MAX_REFERENCES
        || candidate.evidence_ids.len() > MAX_REFERENCES
        || candidate.attitude_evidence_ids.len() > MAX_REFERENCES
        || !unique_subset(&candidate.source_ids, &context.source_ids)
        || !unique_subset(&candidate.entity_ids, &context.entity_ids)
        || !unique_map_subset(&candidate.evidence_ids, &context.evidence)
        || !unique_map_subset(&candidate.attitude_evidence_ids, &context.evidence)
        || !context.subjects.contains(&candidate.subject_ref)
        || matches!(&candidate.subject_ref, SubjectRef::Character { id } if !context.entity_ids.contains(id))
    {
        return Err(reject());
    }
    validate_evidence(candidate, context)?;
    validate_target(candidate, context)?;
    validate_shape(candidate, context)?;
    Ok(())
}

fn validate_evidence(candidate: &Candidate, context: &ProposalContext) -> Result<()> {
    let expected = candidate.subject_ref.evidence_subject();
    let required = !matches!(
        candidate.classification,
        Classification::NotWorthy | Classification::Uncertain
    );
    if expected.is_none()
        && (!candidate.evidence_ids.is_empty() || !candidate.attitude_evidence_ids.is_empty())
    {
        return Err(reject());
    }
    if let Some(subject) = expected
        && ((required && candidate.evidence_ids.is_empty())
            || candidate
                .evidence_ids
                .iter()
                .chain(&candidate.attitude_evidence_ids)
                .any(|id| {
                    context.evidence.get(id).is_none_or(|evidence| {
                        evidence.subject_id != subject
                            || evidence.source_ids.is_empty()
                            || evidence.source_ids.len() > MAX_REFERENCES
                            || evidence.source_ids.iter().collect::<BTreeSet<_>>().len()
                                != evidence.source_ids.len()
                            || evidence
                                .source_ids
                                .iter()
                                .any(|id| !candidate.source_ids.contains(id))
                    })
                })
            || candidate
                .attitude_evidence_ids
                .iter()
                .any(|id| !candidate.evidence_ids.contains(id)))
    {
        return Err(reject());
    }
    Ok(())
}

fn validate_target(candidate: &Candidate, context: &ProposalContext) -> Result<()> {
    let Some(reference) = &candidate.target_ref else {
        if matches!(
            candidate.classification,
            Classification::Repeat | Classification::Supplement | Classification::Correction
        ) {
            return Err(reject());
        }
        return Ok(());
    };
    if !valid_id(&reference.entry_id)
        || !valid_id(&reference.expected_version_id)
        || reference.expected_state_revision > MAX_INTEGER
    {
        return Err(reject());
    }
    let Some(snapshot) = context.targets.get(&reference.entry_id) else {
        return Err(reject());
    };
    if snapshot.entry_id != reference.entry_id
        || snapshot.version_id != reference.expected_version_id
        || snapshot.state_revision != reference.expected_state_revision
        || snapshot.state == super::EntryState::Unavailable
        || (snapshot.state == super::EntryState::Archived
            && matches!(
                candidate.classification,
                Classification::Supplement | Classification::Correction
            ))
        || !candidate.subject_ref.matches_subject(&snapshot.subject)
        || candidate
            .dimension
            .is_some_and(|dimension| dimension != snapshot.dimension)
    {
        return Err(Error::Rejected(Reason::StaleVersion));
    }
    Ok(())
}

fn validate_shape(candidate: &Candidate, context: &ProposalContext) -> Result<()> {
    let has_body = candidate
        .summary
        .as_ref()
        .is_some_and(|summary| valid_summary(summary));
    let is_character = matches!(candidate.subject_ref, SubjectRef::Character { .. });
    let invalid_inferred_preference = matches!(candidate.subject_ref, SubjectRef::PlayerPreference)
        && candidate.inferred != Some(false);
    let previous_attitude = candidate
        .target_ref
        .as_ref()
        .and_then(|target| context.targets.get(&target.entry_id))
        .map(|target| target.attitude);
    let attitude_evidence_required = is_character
        && candidate.attitude.is_some_and(|attitude| {
            attitude != Attitude::Unexpressed && previous_attitude != Some(attitude)
        });
    if attitude_evidence_required && candidate.attitude_evidence_ids.is_empty() {
        return Err(reject());
    }
    match candidate.classification {
        Classification::New => {
            if candidate.target_ref.is_some()
                || !has_body
                || candidate.dimension.is_none()
                || candidate.inferred.is_none()
                || invalid_inferred_preference
                || (is_character && candidate.attitude.is_none())
                || !valid_dimension_for_subject(&candidate.subject_ref, candidate.dimension)
            {
                return Err(reject());
            }
        }
        Classification::Supplement | Classification::Correction => {
            if candidate.target_ref.is_none()
                || !has_body
                || candidate.dimension.is_none()
                || candidate.inferred.is_none()
                || invalid_inferred_preference
                || (is_character && candidate.attitude.is_none())
                || !valid_dimension_for_subject(&candidate.subject_ref, candidate.dimension)
            {
                return Err(reject());
            }
        }
        Classification::Repeat => {
            if candidate.summary.is_some()
                || candidate.dimension.is_some()
                || candidate.attitude.is_some()
                || !candidate.attitude_evidence_ids.is_empty()
                || candidate.inferred.is_some()
                || candidate.target_ref.is_none()
            {
                return Err(reject());
            }
        }
        Classification::NotWorthy => {
            if candidate.target_ref.is_some()
                || candidate.dimension.is_some()
                || candidate.summary.is_some()
                || candidate.attitude.is_some()
                || !candidate.attitude_evidence_ids.is_empty()
                || candidate.inferred.is_some()
                || !candidate.evidence_ids.is_empty()
            {
                return Err(reject());
            }
        }
        Classification::Uncertain => {
            if !has_body
                || candidate.dimension.is_some()
                || candidate.attitude.is_some()
                || !candidate.attitude_evidence_ids.is_empty()
                || candidate.inferred.is_some()
            {
                return Err(reject());
            }
        }
    }
    Ok(())
}

fn valid_dimension_for_subject(subject: &SubjectRef, dimension: Option<Dimension>) -> bool {
    matches!(
        (subject, dimension),
        (
            SubjectRef::PlayerPreference,
            Some(Dimension::PlayerPreference)
        ) | (
            SubjectRef::Character { .. },
            Some(Dimension::Story | Dimension::Relationship | Dimension::Foreshadowing)
        )
    )
}

fn valid_summary(summary: &str) -> bool {
    !summary.trim().is_empty() && summary.len() <= 4096 && !summary.contains('\0')
}

fn unique_subset(values: &[String], allowed: &BTreeSet<String>) -> bool {
    let unique = values.iter().collect::<BTreeSet<_>>();
    unique.len() == values.len() && values.iter().all(|value| allowed.contains(value))
}

fn unique_map_subset<T>(values: &[String], allowed: &BTreeMap<String, T>) -> bool {
    let unique = values.iter().collect::<BTreeSet<_>>();
    unique.len() == values.len() && values.iter().all(|value| allowed.contains_key(value))
}

#[cfg(test)]
mod tests;
