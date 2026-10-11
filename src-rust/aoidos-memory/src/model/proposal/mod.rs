//! 模型候选的有界协议与纯白名单校验；不查询存储、不提交记忆。

use super::{Attitude, Dimension, MAX_INTEGER, Subject, reject, valid_id};
use crate::error::{Error, Reason, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PROPOSAL_BYTES: usize = 64 * 1024;
pub const MAX_CANDIDATES: usize = 128;
pub const MAX_REFERENCES: usize = 128;

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
}

/// 只包含冻结批次中已经登记且可信的身份，不接收来自候选的目录或证据。
#[derive(Debug, Clone, Default)]
pub struct ProposalContext {
    pub source_ids: BTreeSet<String>,
    pub entity_ids: BTreeSet<String>,
    pub subjects: BTreeSet<SubjectRef>,
    pub evidence: BTreeMap<String, EvidenceSnapshot>,
    pub targets: BTreeMap<String, TargetSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceSnapshot {
    pub subject_id: String,
    pub source_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSnapshot {
    pub entry_id: String,
    pub version_id: String,
    pub state_revision: u64,
    pub subject: Subject,
    pub dimension: Dimension,
    pub attitude: Attitude,
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
        if self.candidates.len() > MAX_CANDIDATES {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let mut candidate_ids = BTreeSet::new();
        for candidate in &self.candidates {
            validate_candidate(candidate, context)?;
            if !candidate_ids.insert(&candidate.candidate_id) {
                return Err(reject());
            }
        }
        Ok(())
    }
}

fn validate_candidate(candidate: &Candidate, context: &ProposalContext) -> Result<()> {
    if !short_id(&candidate.candidate_id)
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
                            || !candidate.source_ids.contains(&evidence.source_id)
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

fn short_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
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
