//! 将已校验候选转换为可重放的记忆操作计划。

use aoidos_memory::{
    algorithm::{CandidateWork, dependency_groups},
    error::{Error, Reason, Result},
    model::{
        Attitude, Candidate, ChangeKind, Classification, EntryState, Expected, Hash,
        KnowledgeEvidence, Plan, Proposal, ProposalContext, Run, SourceRef, Subject, Target,
        Version, hash, set_hash,
    },
    repository::{BatchCompletion, BatchSnapshot, CandidateOutcome, CandidateResult},
};
use std::collections::{BTreeMap, BTreeSet};

const REVIEW_REASON: &str = "memory.candidate.review";
const TARGET_CONFLICT_REASON: &str = "memory.candidate.shared_target";
const DUPLICATE_ANCHOR_REASON: &str = "memory.candidate.duplicate_anchor";

pub(super) struct PlannedGroup {
    pub plan: Plan,
    pub no_op_candidates: BTreeSet<String>,
}

pub(super) struct PlanSet {
    pub operations: Vec<PlannedGroup>,
    pub immediate_results: Vec<CandidateResult>,
}

pub(super) fn completion(
    mut results: Vec<CandidateResult>,
    cycle_marks: Vec<aoidos_memory::repository::CycleMarkResult>,
) -> BatchCompletion {
    results.sort_by(|left, right| {
        left.candidate_id
            .as_bytes()
            .cmp(right.candidate_id.as_bytes())
    });
    BatchCompletion {
        version: 1,
        outcomes: results,
        cycle_marks,
    }
}

/// 依冻结来源、目标和策略为候选建立稳定的依赖操作组。
///
/// 重复 / 不确定候选暂留待处理状态；它们与其他候选共享素材或目标时，整组一并延后，
/// 避免先标记素材已处理、再丢失尚未解决的候选。
pub(super) fn build_plans(
    snapshot: &BatchSnapshot,
    proposal: &Proposal,
    context: &ProposalContext,
    run: &Run,
    previous_versions: &BTreeMap<String, Version>,
) -> Result<PlanSet> {
    let spans = snapshot
        .spans
        .iter()
        .map(|span| (span.source_id.as_str(), span))
        .collect::<BTreeMap<_, _>>();
    if spans.len() != snapshot.spans.len()
        || spans.len() != context.source_ids.len()
        || spans.keys().any(|id| !context.source_ids.contains(*id))
    {
        return Err(Error::Rejected(Reason::InvalidSource));
    }

    let candidates = proposal
        .candidates
        .iter()
        .map(|candidate| (candidate.candidate_id.as_str(), candidate))
        .collect::<BTreeMap<_, _>>();
    let work = proposal
        .candidates
        .iter()
        .map(|candidate| {
            let mut material_ids = candidate
                .source_ids
                .iter()
                .map(|source_id| {
                    spans
                        .get(source_id.as_str())
                        .map(|span| span.material_id.clone())
                        .ok_or(Error::Rejected(Reason::InvalidSource))
                })
                .collect::<Result<Vec<_>>>()?;
            material_ids.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            material_ids.dedup();

            let mut target_entry_ids = match &candidate.target_ref {
                Some(target) => vec![target.entry_id.clone()],
                None => Vec::new(),
            };
            if candidate.classification == Classification::New {
                target_entry_ids.push(stable_id(
                    "entry",
                    &snapshot.batch_id,
                    &candidate.candidate_id,
                ));
            }
            target_entry_ids.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            target_entry_ids.dedup();

            let earliest_source_position = candidate
                .source_ids
                .iter()
                .filter_map(|source_id| spans.get(source_id.as_str()))
                .map(|span| span.source.record_seq)
                .min()
                .ok_or(Error::Rejected(Reason::InvalidSource))?;
            Ok(CandidateWork {
                candidate_id: candidate.candidate_id.clone(),
                earliest_source_position,
                material_ids,
                target_entry_ids,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let groups = dependency_groups(&work)?;
    let mut result = PlanSet {
        operations: Vec::new(),
        immediate_results: Vec::new(),
    };

    for candidate_ids in groups {
        let group = candidate_ids
            .iter()
            .map(|id| {
                candidates
                    .get(id.as_str())
                    .copied()
                    .ok_or(Error::Rejected(Reason::InvalidSchema))
            })
            .collect::<Result<Vec<_>>>()?;
        if group.iter().any(|candidate| {
            matches!(
                candidate.classification,
                Classification::Repeat | Classification::Uncertain
            )
        }) {
            result
                .immediate_results
                .extend(candidate_ids.iter().map(|candidate_id| CandidateResult {
                    candidate_id: candidate_id.clone(),
                    outcome: CandidateOutcome::Deferred,
                    reason_code: Some(REVIEW_REASON.into()),
                }));
            continue;
        }

        if has_shared_existing_target(&group) {
            result
                .immediate_results
                .extend(candidate_ids.iter().map(|candidate_id| CandidateResult {
                    candidate_id: candidate_id.clone(),
                    outcome: CandidateOutcome::Deferred,
                    reason_code: Some(TARGET_CONFLICT_REASON.into()),
                }));
            continue;
        }

        if has_duplicate_new_anchor(&group, &spans, previous_versions) {
            result
                .immediate_results
                .extend(candidate_ids.iter().map(|candidate_id| CandidateResult {
                    candidate_id: candidate_id.clone(),
                    outcome: CandidateOutcome::Deferred,
                    reason_code: Some(DUPLICATE_ANCHOR_REASON.into()),
                }));
            continue;
        }

        let mut targets = Vec::new();
        let mut no_op_candidates = BTreeSet::new();
        let mut processed_materials = BTreeSet::new();
        for candidate in &group {
            for source_id in &candidate.source_ids {
                let span = spans
                    .get(source_id.as_str())
                    .ok_or(Error::Rejected(Reason::InvalidSource))?;
                processed_materials.insert(span.material_id.clone());
            }
            if candidate.classification == Classification::NotWorthy {
                no_op_candidates.insert(candidate.candidate_id.clone());
                continue;
            }

            let (target, previous) =
                build_target(candidate, snapshot, context, run, previous_versions, &spans)?;
            if let Some(previous) = previous
                && targets
                    .iter()
                    .any(|target: &Target| target.entry_id == previous.entry_id)
            {
                return Err(Error::Rejected(Reason::AmbiguousEntity));
            }
            targets.push(target);
        }
        targets.sort_by(|left, right| left.entry_id.as_bytes().cmp(right.entry_id.as_bytes()));

        result.operations.push(PlannedGroup {
            plan: Plan {
                expected_logical_clock: snapshot.logical_clock,
                operation_id: stable_id("operation", &snapshot.batch_id, &candidate_ids.join("\0")),
                run_id: run.run_id.clone(),
                history_revision: snapshot.history_revision,
                entity_revision: snapshot.entity_revision,
                evidence_revision: snapshot.evidence_revision,
                policy_hash: run.policy_hash,
                batch_id: Some(snapshot.batch_id.clone()),
                candidate_ids,
                targets,
                processed_materials: processed_materials.into_iter().collect(),
                reason: "memory proposal".into(),
            },
            no_op_candidates,
        });
    }

    Ok(result)
}

type AnchorIdentity = (u8, String, &'static str, String, String, u64, Hash);

fn has_duplicate_new_anchor(
    candidates: &[&Candidate],
    spans: &BTreeMap<&str, &aoidos_memory::repository::Span>,
    previous_versions: &BTreeMap<String, Version>,
) -> bool {
    let mut existing: BTreeMap<AnchorIdentity, Vec<(u64, u64)>> = BTreeMap::new();
    for version in previous_versions.values() {
        let (subject_kind, subject_id) = subject_key(&version.subject);
        for source in &version.sources {
            existing
                .entry(anchor_identity(
                    subject_kind,
                    &subject_id,
                    version.dimension,
                    source,
                ))
                .or_default()
                .push((source.start_byte, source.end_byte));
        }
    }
    for ranges in existing.values_mut() {
        merge_ranges(ranges);
    }

    let mut proposed: BTreeMap<AnchorIdentity, Vec<(u64, u64, String)>> = BTreeMap::new();
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.classification == Classification::New)
    {
        let Some(dimension) = candidate.dimension else {
            continue;
        };
        let (subject_kind, subject_id) = subject_key(&candidate_subject(candidate));
        for source_id in &candidate.source_ids {
            let Some(span) = spans.get(source_id.as_str()) else {
                continue;
            };
            let identity = anchor_identity(subject_kind, &subject_id, dimension, &span.source);
            if existing.get(&identity).is_some_and(|ranges| {
                intersects(ranges, span.source.start_byte, span.source.end_byte)
            }) {
                return true;
            }
            proposed.entry(identity).or_default().push((
                span.source.start_byte,
                span.source.end_byte,
                candidate.candidate_id.clone(),
            ));
        }
    }

    proposed.values_mut().any(|ranges| {
        ranges.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        let mut previous: Option<(u64, String)> = None;
        for (start, end, candidate_id) in ranges.iter() {
            if previous
                .as_ref()
                .is_some_and(|(previous_end, previous_id)| {
                    *start < *previous_end && candidate_id != previous_id
                })
            {
                return true;
            }
            if previous
                .as_ref()
                .is_none_or(|(previous_end, _)| *end > *previous_end)
            {
                previous = Some((*end, candidate_id.clone()));
            }
        }
        false
    })
}

fn candidate_subject(candidate: &Candidate) -> Subject {
    match &candidate.subject_ref {
        aoidos_memory::model::SubjectRef::PlayerPreference => Subject::PlayerPreference,
        aoidos_memory::model::SubjectRef::Character { id } => Subject::Character { id: id.clone() },
    }
}

fn subject_key(subject: &Subject) -> (u8, String) {
    match subject {
        Subject::PlayerPreference => (0, String::new()),
        Subject::Character { id } => (1, id.clone()),
    }
}

fn anchor_identity(
    subject_kind: u8,
    subject_id: &str,
    dimension: aoidos_memory::model::Dimension,
    source: &SourceRef,
) -> AnchorIdentity {
    (
        subject_kind,
        subject_id.into(),
        dimension_key(dimension),
        source.script_id.clone(),
        source.run_id.clone(),
        source.record_seq,
        source.body_hash,
    )
}

fn dimension_key(dimension: aoidos_memory::model::Dimension) -> &'static str {
    use aoidos_memory::model::Dimension;
    match dimension {
        Dimension::Story => "story",
        Dimension::Relationship => "relationship",
        Dimension::PlayerPreference => "playerPreference",
        Dimension::Foreshadowing => "foreshadowing",
    }
}

fn merge_ranges(ranges: &mut Vec<(u64, u64)>) {
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges.drain(..) {
        if let Some((_, previous_end)) = merged.last_mut()
            && start <= *previous_end
        {
            *previous_end = (*previous_end).max(end);
        } else {
            merged.push((start, end));
        }
    }
    *ranges = merged;
}

fn intersects(ranges: &[(u64, u64)], start: u64, end: u64) -> bool {
    let index = ranges.partition_point(|(_, range_end)| *range_end <= start);
    ranges
        .get(index)
        .is_some_and(|(range_start, _)| *range_start < end)
}

fn has_shared_existing_target(group: &[&Candidate]) -> bool {
    let mut seen = BTreeSet::new();
    group
        .iter()
        .filter_map(|candidate| candidate.target_ref.as_ref())
        .any(|target| !seen.insert(target.entry_id.as_str()))
}

fn build_target(
    candidate: &Candidate,
    snapshot: &BatchSnapshot,
    context: &ProposalContext,
    run: &Run,
    previous_versions: &BTreeMap<String, Version>,
    spans: &BTreeMap<&str, &aoidos_memory::repository::Span>,
) -> Result<(Target, Option<Version>)> {
    let dimension = candidate
        .dimension
        .ok_or(Error::Rejected(Reason::InvalidSchema))?;
    let summary = candidate
        .summary
        .as_ref()
        .ok_or(Error::Rejected(Reason::InvalidSchema))?
        .clone();
    let inferred = candidate
        .inferred
        .ok_or(Error::Rejected(Reason::InvalidSchema))?;
    let (entry_id, expected, previous) = match &candidate.target_ref {
        Some(reference) => {
            let frozen = context
                .targets
                .get(&reference.entry_id)
                .ok_or(Error::Rejected(Reason::StaleVersion))?;
            if frozen.version_id != reference.expected_version_id
                || frozen.state_revision != reference.expected_state_revision
            {
                return Err(Error::Rejected(Reason::StaleVersion));
            }
            let previous = previous_versions
                .get(&reference.entry_id)
                .filter(|version| version.version_id == reference.expected_version_id)
                .cloned()
                .ok_or(Error::Rejected(Reason::StaleVersion))?;
            (
                reference.entry_id.clone(),
                Expected {
                    version_id: Some(reference.expected_version_id.clone()),
                    state_revision: reference.expected_state_revision,
                },
                Some(previous),
            )
        }
        None => (
            stable_id("entry", &snapshot.batch_id, &candidate.candidate_id),
            Expected {
                version_id: None,
                state_revision: 0,
            },
            None,
        ),
    };

    let subject = match &candidate.subject_ref {
        aoidos_memory::model::SubjectRef::PlayerPreference => Subject::PlayerPreference,
        aoidos_memory::model::SubjectRef::Character { id } => Subject::Character { id: id.clone() },
    };
    let mut sources = previous
        .as_ref()
        .map_or_else(Vec::new, |version| version.sources.clone());
    for source_id in &candidate.source_ids {
        sources.push(
            spans
                .get(source_id.as_str())
                .ok_or(Error::Rejected(Reason::InvalidSource))?
                .source
                .clone(),
        );
    }
    sources = unique_sources(sources)?;

    let mut evidence = previous
        .as_ref()
        .map_or_else(Vec::new, |version| version.evidence.clone());
    for evidence_id in &candidate.evidence_ids {
        let snapshot = context
            .evidence
            .get(evidence_id)
            .ok_or(Error::Rejected(Reason::InvalidSource))?;
        evidence.push(KnowledgeEvidence {
            evidence_id: evidence_id.clone(),
            subject_id: snapshot.subject_id.clone(),
            source: snapshot.source.clone(),
            kind: snapshot.kind,
            informant_id: snapshot.informant_id.clone(),
            scope: snapshot.scope.clone(),
            rule_id: snapshot.rule_id.clone(),
        });
    }
    evidence = unique_evidence(evidence)?;

    let version = Version {
        version: 1,
        entry_id: entry_id.clone(),
        version_id: stable_id("version", &snapshot.batch_id, &candidate.candidate_id),
        parent_version_id: expected.version_id.clone(),
        restored_from_version_id: None,
        subject,
        dimension,
        summary,
        attitude: candidate.attitude.unwrap_or(Attitude::Unexpressed),
        inferred,
        change: match candidate.classification {
            Classification::New => ChangeKind::New,
            Classification::Supplement => ChangeKind::Supplement,
            Classification::Correction => ChangeKind::Correction,
            _ => return Err(Error::Rejected(Reason::InvalidSchema)),
        },
        source_set_hash: set_hash(&sources)?,
        evidence_set_hash: set_hash(&evidence)?,
        sources,
        evidence,
        policy_hash: run.policy_hash,
    };
    version.validate()?;
    Ok((
        Target {
            entry_id,
            expected,
            next: Some(version),
            state: EntryState::Active,
            effects: Vec::new(),
        },
        previous,
    ))
}

fn unique_sources(mut sources: Vec<SourceRef>) -> Result<Vec<SourceRef>> {
    sources.sort_by(|left, right| {
        left.record_seq
            .cmp(&right.record_seq)
            .then_with(|| left.start_byte.cmp(&right.start_byte))
            .then_with(|| left.end_byte.cmp(&right.end_byte))
    });
    sources.dedup();
    if sources.is_empty() || sources.len() > 128 {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    Ok(sources)
}

fn unique_evidence(mut evidence: Vec<KnowledgeEvidence>) -> Result<Vec<KnowledgeEvidence>> {
    evidence.sort_by(|left, right| {
        left.evidence_id
            .as_bytes()
            .cmp(right.evidence_id.as_bytes())
    });
    for pair in evidence.windows(2) {
        if pair[0].evidence_id == pair[1].evidence_id && pair[0] != pair[1] {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
    }
    evidence.dedup_by(|left, right| left.evidence_id == right.evidence_id);
    if evidence.len() > 128 {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    Ok(evidence)
}

fn stable_id(kind: &str, batch_id: &str, identity: &str) -> String {
    let digest: Hash = hash(format!("aoidos-memory-v1\0{kind}\0{batch_id}\0{identity}").as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

#[cfg(test)]
mod tests;
