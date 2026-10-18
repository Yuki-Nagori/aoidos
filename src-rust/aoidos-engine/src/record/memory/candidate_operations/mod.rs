//! 将已校验候选转换为可重放的记忆操作计划。

use aoidos_memory::{
    algorithm::{
        AcceptedReinforcement, CandidateWork, ReinforcementKind, dependency_groups,
        effect_source_id, reinforcement_increment, revival_allowed,
    },
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
    pub persisted: bool,
}

pub(super) struct PlanSet {
    pub operations: Vec<PlannedGroup>,
    pub immediate_results: Vec<CandidateResult>,
}

pub(super) fn persisted_group(
    snapshot: &BatchSnapshot,
    plan: Plan,
    candidates: &[&Candidate],
) -> Result<PlannedGroup> {
    if plan.batch_id.as_deref() != Some(snapshot.batch_id.as_str())
        || candidates.is_empty()
        || candidates
            .iter()
            .any(|candidate| !plan.candidate_ids.contains(&candidate.candidate_id))
    {
        return Err(Error::Rejected(Reason::InvalidSchema));
    }

    let effects = plan
        .targets
        .iter()
        .flat_map(|target| &target.effects)
        .map(|effect| effect.effect_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut no_op_candidates = BTreeSet::new();
    for candidate in candidates {
        let has_effect = candidate.classification == Classification::Repeat
            && candidate.source_ids.iter().any(|source_id| {
                [
                    aoidos_memory::model::EffectKind::Gain,
                    aoidos_memory::model::EffectKind::Revival,
                ]
                .into_iter()
                .any(|kind| {
                    let id = stable_id(
                        "effect",
                        &snapshot.batch_id,
                        &format!("{}\0{}\0{kind:?}", candidate.candidate_id, source_id),
                    );
                    effects.contains(id.as_str())
                })
            });
        if candidate.classification == Classification::NotWorthy
            || (candidate.classification == Classification::Repeat && !has_effect)
        {
            no_op_candidates.insert(candidate.candidate_id.clone());
        }
    }

    Ok(PlannedGroup {
        plan,
        no_op_candidates,
        persisted: true,
    })
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
#[cfg(test)]
pub(super) fn build_plans(
    snapshot: &BatchSnapshot,
    proposal: &Proposal,
    context: &ProposalContext,
    run: &Run,
    previous_versions: &BTreeMap<String, Version>,
) -> Result<PlanSet> {
    build_plans_with_history(
        snapshot,
        proposal,
        context,
        run,
        previous_versions,
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
}

pub(super) fn build_plans_with_history(
    snapshot: &BatchSnapshot,
    proposal: &Proposal,
    context: &ProposalContext,
    run: &Run,
    previous_versions: &BTreeMap<String, Version>,
    previous_effects: &BTreeMap<String, Vec<(u64, aoidos_memory::model::Effect)>>,
    material_positions: &BTreeMap<String, u64>,
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
        if group
            .iter()
            .any(|candidate| candidate.classification == Classification::Uncertain)
        {
            result
                .immediate_results
                .extend(candidate_ids.iter().map(|candidate_id| CandidateResult {
                    candidate_id: candidate_id.clone(),
                    outcome: CandidateOutcome::Deferred,
                    reason_code: Some(REVIEW_REASON.into()),
                }));
            continue;
        }

        if group
            .iter()
            .all(|candidate| candidate.classification == Classification::Repeat)
        {
            if group.iter().any(|candidate| {
                candidate
                    .target_ref
                    .as_ref()
                    .map(|target| target.entry_id.as_str())
                    != group[0]
                        .target_ref
                        .as_ref()
                        .map(|target| target.entry_id.as_str())
            }) {
                result
                    .immediate_results
                    .extend(candidate_ids.iter().map(|candidate_id| CandidateResult {
                        candidate_id: candidate_id.clone(),
                        outcome: CandidateOutcome::Deferred,
                        reason_code: Some(TARGET_CONFLICT_REASON.into()),
                    }));
                continue;
            }
            let (target, no_op_candidates, processed_materials) = build_repeat_group(
                &group,
                RepeatPlanContext {
                    snapshot,
                    proposal_context: context,
                    run,
                    previous_versions,
                    previous_effects,
                    material_positions,
                    spans: &spans,
                },
            )?;
            result.operations.push(PlannedGroup {
                plan: Plan {
                    expected_logical_clock: snapshot.logical_clock,
                    operation_id: stable_id(
                        "operation",
                        &snapshot.batch_id,
                        &candidate_ids.join("\0"),
                    ),
                    run_id: run.run_id.clone(),
                    history_revision: snapshot.history_revision,
                    entity_revision: snapshot.entity_revision,
                    evidence_revision: snapshot.evidence_revision,
                    policy_hash: run.policy_hash,
                    batch_id: Some(snapshot.batch_id.clone()),
                    candidate_ids,
                    targets: target.into_iter().collect(),
                    processed_materials,
                    reason: "memory repeat".into(),
                },
                no_op_candidates,
                persisted: false,
            });
            continue;
        }

        if group
            .iter()
            .any(|candidate| candidate.classification == Classification::Repeat)
        {
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
            persisted: false,
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

struct RepeatPlanContext<'a> {
    snapshot: &'a BatchSnapshot,
    proposal_context: &'a ProposalContext,
    run: &'a Run,
    previous_versions: &'a BTreeMap<String, Version>,
    previous_effects: &'a BTreeMap<String, Vec<(u64, aoidos_memory::model::Effect)>>,
    material_positions: &'a BTreeMap<String, u64>,
    spans: &'a BTreeMap<&'a str, &'a aoidos_memory::repository::Span>,
}

fn build_repeat_group(
    candidates: &[&Candidate],
    plan_context: RepeatPlanContext<'_>,
) -> Result<(Option<Target>, BTreeSet<String>, Vec<String>)> {
    let RepeatPlanContext {
        snapshot,
        proposal_context: context,
        run,
        previous_versions,
        previous_effects,
        material_positions,
        spans,
    } = plan_context;
    let first = candidates
        .first()
        .ok_or(Error::Rejected(Reason::InvalidSchema))?;
    let reference = first
        .target_ref
        .as_ref()
        .ok_or(Error::Rejected(Reason::InvalidSchema))?;
    let target_snapshot = context
        .targets
        .get(&reference.entry_id)
        .ok_or(Error::Rejected(Reason::StaleVersion))?;
    if target_snapshot.version_id != reference.expected_version_id
        || target_snapshot.state_revision != reference.expected_state_revision
    {
        return Err(Error::Rejected(Reason::StaleVersion));
    }
    let previous = previous_versions
        .get(&reference.entry_id)
        .filter(|version| version.version_id == reference.expected_version_id)
        .ok_or(Error::Rejected(Reason::StaleVersion))?;
    let subject_matches = match (&first.subject_ref, &previous.subject) {
        (aoidos_memory::model::SubjectRef::PlayerPreference, Subject::PlayerPreference) => true,
        (
            aoidos_memory::model::SubjectRef::Character { id: left },
            Subject::Character { id: right },
        ) => left == right,
        _ => false,
    };
    if !subject_matches {
        return Err(Error::Rejected(Reason::InvalidSchema));
    }

    let mut accepted = previous_effects
        .get(&reference.entry_id)
        .into_iter()
        .flatten()
        .map(|(_, effect)| {
            Ok(AcceptedReinforcement {
                source_id: effect_source_id(&run.run_id, &effect.source)?,
                logical_position: effect.logical_position,
                kind: match effect.kind {
                    aoidos_memory::model::EffectKind::Gain => ReinforcementKind::Gain {
                        amount: effect.gain,
                    },
                    aoidos_memory::model::EffectKind::Revival => ReinforcementKind::Revival,
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut effects = Vec::new();
    let mut no_op_candidates = BTreeSet::new();
    let mut processed_materials = BTreeSet::new();
    let mut state = target_snapshot.state;
    let mut evidence_by_candidate = BTreeMap::new();
    let mut candidate_sources = Vec::new();
    for candidate in candidates {
        let evidence = candidate
            .evidence_ids
            .iter()
            .map(|evidence_id| {
                let evidence = context
                    .evidence
                    .get(evidence_id)
                    .ok_or(Error::Rejected(Reason::InvalidSource))?;
                Ok(KnowledgeEvidence {
                    evidence_id: evidence_id.clone(),
                    subject_id: evidence.subject_id.clone(),
                    source: evidence.source.clone(),
                    kind: evidence.kind,
                    informant_id: evidence.informant_id.clone(),
                    scope: evidence.scope.clone(),
                    rule_id: evidence.rule_id.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        evidence_by_candidate.insert(candidate.candidate_id.as_str(), evidence);
        for source_id in &candidate.source_ids {
            let span = spans
                .get(source_id.as_str())
                .copied()
                .ok_or(Error::Rejected(Reason::InvalidSource))?;
            let position = material_positions
                .get(&span.material_id)
                .copied()
                .ok_or(Error::Rejected(Reason::RecoveryRequired))?;
            processed_materials.insert(span.material_id.clone());
            candidate_sources.push((candidate, source_id.as_str(), span, position));
        }
    }
    candidate_sources.sort_by(|left, right| {
        left.3
            .cmp(&right.3)
            .then_with(|| left.2.source.record_seq.cmp(&right.2.source.record_seq))
            .then_with(|| left.2.source.start_byte.cmp(&right.2.source.start_byte))
            .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
            .then_with(|| {
                left.0
                    .candidate_id
                    .as_bytes()
                    .cmp(right.0.candidate_id.as_bytes())
            })
    });

    let mut seen_records = BTreeSet::new();
    let mut accepted_candidates = BTreeSet::new();
    for (candidate, source_id, span, position) in candidate_sources {
        if !seen_records.insert((span.source.session_id.as_str(), span.source.record_seq)) {
            continue;
        }
        let id = effect_source_id(&run.run_id, &span.source)?;
        let (kind, gain) = match state {
            EntryState::Active => {
                let Some(gain) = reinforcement_increment(&run.policy, &id, position, &accepted)?
                else {
                    continue;
                };
                (aoidos_memory::model::EffectKind::Gain, gain)
            }
            EntryState::Archived => {
                if !revival_allowed(&run.policy, &id, position, &accepted)? {
                    continue;
                }
                state = EntryState::Active;
                (aoidos_memory::model::EffectKind::Revival, 0)
            }
            EntryState::Unavailable => {
                return Err(Error::Rejected(Reason::StaleVersion));
            }
        };
        effects.push(aoidos_memory::model::Effect {
            effect_id: stable_id(
                "effect",
                &snapshot.batch_id,
                &format!("{}\0{}\0{:?}", candidate.candidate_id, source_id, kind),
            ),
            kind,
            source: span.source.clone(),
            logical_position: position,
            gain,
            evidence: evidence_by_candidate
                .get(candidate.candidate_id.as_str())
                .cloned()
                .ok_or(Error::Rejected(Reason::InvalidSource))?,
        });
        accepted.push(AcceptedReinforcement {
            source_id: id,
            logical_position: position,
            kind: match kind {
                aoidos_memory::model::EffectKind::Gain => ReinforcementKind::Gain { amount: gain },
                aoidos_memory::model::EffectKind::Revival => ReinforcementKind::Revival,
            },
        });
        accepted_candidates.insert(candidate.candidate_id.as_str());
    }
    no_op_candidates.extend(
        candidates
            .iter()
            .filter(|candidate| !accepted_candidates.contains(candidate.candidate_id.as_str()))
            .map(|candidate| candidate.candidate_id.clone()),
    );

    if effects.len() > 128 {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    let target = (!effects.is_empty()).then(|| Target {
        entry_id: reference.entry_id.clone(),
        expected: Expected {
            version_id: Some(reference.expected_version_id.clone()),
            state_revision: reference.expected_state_revision,
        },
        next: None,
        state: EntryState::Active,
        effects,
    });
    Ok((
        target,
        no_op_candidates,
        processed_materials.into_iter().collect(),
    ))
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
                || frozen.state != EntryState::Active
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
