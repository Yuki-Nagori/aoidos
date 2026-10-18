use super::*;
use aoidos_memory::model::{
    Attitude, Dimension, Effect, EffectKind, EntryState, InputMode, Policy, Subject, SubjectRef,
    TargetSnapshot, hash, new_id,
};
use aoidos_memory::repository::Span;

fn run(run_id: &str, session_id: &str) -> Run {
    Run {
        run_id: run_id.into(),
        script_id: "mistbell".into(),
        session_id: session_id.into(),
        cycle_gen: 1,
        policy: Policy {
            version: 1,
            algorithm_version: 1,
            config_version: 0,
            app_config_version: 0,
            algorithm: aoidos_memory::model::Algorithm::Exponential,
            base_strength: 100_000,
            reinforcement_gain: 50_000,
            reinforcement_cap: 400_000,
            strength_cap: 1_000_000,
            half_life_rounds: 100,
            repeat_window_rounds: 5,
            repeat_limit: 2,
            revival_cooldown_rounds: 10,
            active_capacity: 100,
            rate_table_version: 1,
            decay_rate: 100_000,
        },
        policy_hash: [7; 32],
    }
}

fn batch(run_id: &str, session_id: &str) -> BatchSnapshot {
    let source = SourceRef {
        script_id: "mistbell".into(),
        run_id: run_id.into(),
        session_id: session_id.into(),
        record_seq: 1,
        body_hash: hash("我喜欢钟声".as_bytes()),
        start_byte: 0,
        end_byte: "我喜欢钟声".len() as u64,
        mode: Some(InputMode::OutOfCharacter),
    };
    BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: run_id.into(),
        history_revision: 1,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: [7; 32],
        spans: vec![Span {
            source_id: "s1".into(),
            material_id: new_id(),
            source,
            text: "我喜欢钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    }
}

fn preference_candidate() -> Candidate {
    Candidate {
        candidate_id: "new_1".into(),
        classification: Classification::New,
        source_ids: vec!["s1".into()],
        subject_ref: SubjectRef::PlayerPreference,
        entity_ids: vec![],
        evidence_ids: vec![],
        target_ref: None,
        dimension: Some(Dimension::PlayerPreference),
        summary: Some("玩家明确表示喜欢钟声".into()),
        attitude: None,
        attitude_evidence_ids: vec![],
        inferred: Some(false),
        reason: "明确表达".into(),
    }
}

fn context() -> ProposalContext {
    ProposalContext {
        source_ids: ["s1".into()].into(),
        subjects: [SubjectRef::PlayerPreference].into(),
        ..ProposalContext::default()
    }
}

#[test]
fn new_candidate_plan_is_deterministic_and_uses_only_frozen_sources() {
    let snapshot = batch(&new_id(), &new_id());
    let run = run(&snapshot.run_id, &snapshot.spans[0].source.session_id);
    let proposal = Proposal {
        version: 1,
        candidates: vec![preference_candidate()],
        cycle_marks: vec![],
    };
    let first = build_plans(&snapshot, &proposal, &context(), &run, &BTreeMap::new()).unwrap();
    let second = build_plans(&snapshot, &proposal, &context(), &run, &BTreeMap::new()).unwrap();

    assert_eq!(first.operations.len(), 1);
    assert_eq!(first.operations[0].plan, second.operations[0].plan);
    let plan = &first.operations[0].plan;
    assert_eq!(plan.batch_id.as_deref(), Some(snapshot.batch_id.as_str()));
    assert_eq!(plan.candidate_ids, ["new_1"]);
    assert_eq!(
        plan.processed_materials,
        [snapshot.spans[0].material_id.clone()]
    );
    assert_eq!(plan.targets.len(), 1);
    let version = plan.targets[0].next.as_ref().unwrap();
    assert_eq!(version.sources, [snapshot.spans[0].source.clone()]);
    assert_eq!(version.summary, "玩家明确表示喜欢钟声");
    assert!(version.validate().is_ok());
    assert!(first.immediate_results.is_empty());
}

#[test]
fn not_worthy_candidate_still_gets_a_processed_material_operation() {
    let snapshot = batch(&new_id(), &new_id());
    let run = run(&snapshot.run_id, &snapshot.spans[0].source.session_id);
    let candidate = Candidate {
        classification: Classification::NotWorthy,
        dimension: None,
        summary: None,
        inferred: None,
        ..preference_candidate()
    };
    let proposal = Proposal {
        version: 1,
        candidates: vec![candidate],
        cycle_marks: vec![],
    };

    let plans = build_plans(&snapshot, &proposal, &context(), &run, &BTreeMap::new()).unwrap();
    assert_eq!(plans.operations.len(), 1);
    assert!(plans.operations[0].plan.targets.is_empty());
    assert_eq!(plans.operations[0].plan.processed_materials.len(), 1);
    assert_eq!(plans.operations[0].plan.candidate_ids, ["new_1"]);
    assert!(plans.operations[0].no_op_candidates.contains("new_1"));
}

#[test]
fn new_candidates_cannot_duplicate_an_existing_subject_dimension_source_anchor() {
    let snapshot = batch(&new_id(), &new_id());
    let run = run(&snapshot.run_id, &snapshot.spans[0].source.session_id);
    let proposal = Proposal {
        version: 1,
        candidates: vec![preference_candidate()],
        cycle_marks: vec![],
    };
    let original = build_plans(&snapshot, &proposal, &context(), &run, &BTreeMap::new()).unwrap();
    let mut existing = original.operations[0].plan.targets[0]
        .next
        .as_ref()
        .unwrap()
        .clone();
    existing.entry_id = new_id();
    existing.version_id = new_id();
    let previous = BTreeMap::from([(existing.entry_id.clone(), existing)]);

    let duplicate = build_plans(&snapshot, &proposal, &context(), &run, &previous).unwrap();
    assert!(duplicate.operations.is_empty());
    assert_eq!(duplicate.immediate_results.len(), 1);
    assert_eq!(
        duplicate.immediate_results[0].outcome,
        CandidateOutcome::Deferred
    );
    assert_eq!(
        duplicate.immediate_results[0].reason_code.as_deref(),
        Some("memory.candidate.duplicate_anchor")
    );

    let mut unrelated = previous.values().next().unwrap().clone();
    unrelated.dimension = Dimension::Story;
    let different_dimension = BTreeMap::from([(unrelated.entry_id.clone(), unrelated)]);
    let allowed =
        build_plans(&snapshot, &proposal, &context(), &run, &different_dimension).unwrap();
    assert_eq!(allowed.operations.len(), 1);

    let mut second = preference_candidate();
    second.candidate_id = "new_2".into();
    second.summary = Some("同一来源重复提交同维度事实".into());
    let duplicate_batch = Proposal {
        version: 1,
        candidates: vec![preference_candidate(), second],
        cycle_marks: vec![],
    };
    let same_batch = build_plans(
        &snapshot,
        &duplicate_batch,
        &context(),
        &run,
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(same_batch.operations.is_empty());
    assert_eq!(same_batch.immediate_results.len(), 2);
}

#[test]
fn unresolved_candidate_defers_its_entire_shared_material_group() {
    let snapshot = batch(&new_id(), &new_id());
    let run = run(&snapshot.run_id, &snapshot.spans[0].source.session_id);
    let mut repeat = preference_candidate();
    repeat.candidate_id = "repeat_1".into();
    repeat.classification = Classification::Repeat;
    repeat.target_ref = Some(aoidos_memory::model::TargetRef {
        entry_id: new_id(),
        expected_version_id: new_id(),
        expected_state_revision: 1,
    });
    repeat.dimension = None;
    repeat.summary = None;
    repeat.inferred = None;
    let proposal = Proposal {
        version: 1,
        candidates: vec![preference_candidate(), repeat],
        cycle_marks: vec![],
    };
    let plans = build_plans(&snapshot, &proposal, &context(), &run, &BTreeMap::new()).unwrap();

    assert!(plans.operations.is_empty());
    assert_eq!(plans.immediate_results.len(), 2);
    assert!(
        plans
            .immediate_results
            .iter()
            .all(|result| result.outcome == CandidateOutcome::Deferred)
    );
}

#[test]
fn repeat_sources_across_candidates_are_applied_in_global_material_order() {
    let seed = batch(&new_id(), &new_id());
    let run = run(&seed.run_id, &seed.spans[0].source.session_id);
    let mut previous = build_plans(
        &seed,
        &Proposal {
            version: 1,
            candidates: vec![preference_candidate()],
            cycle_marks: vec![],
        },
        &context(),
        &run,
        &BTreeMap::new(),
    )
    .unwrap()
    .operations
    .remove(0)
    .plan
    .targets
    .remove(0)
    .next
    .unwrap();
    previous.entry_id = new_id();
    previous.version_id = new_id();

    let mut snapshot = seed.clone();
    snapshot.batch_id = new_id();
    snapshot.logical_clock = 10;
    snapshot.spans.clear();
    for (source_id, record_seq) in [("s1", 1), ("s2", 2), ("s10", 10)] {
        let mut span = seed.spans[0].clone();
        span.source_id = source_id.into();
        span.material_id = new_id();
        span.source.record_seq = record_seq;
        snapshot.spans.push(span);
    }

    let reference = aoidos_memory::model::TargetRef {
        entry_id: previous.entry_id.clone(),
        expected_version_id: previous.version_id.clone(),
        expected_state_revision: 3,
    };
    let target_snapshot = TargetSnapshot {
        entry_id: previous.entry_id.clone(),
        version_id: previous.version_id.clone(),
        state_revision: 3,
        state: EntryState::Active,
        subject: Subject::PlayerPreference,
        dimension: Dimension::PlayerPreference,
        summary: previous.summary.clone(),
        attitude: Attitude::Believed,
        inferred: false,
    };
    let mut proposal_context = context();
    proposal_context.source_ids = ["s1".into(), "s2".into(), "s10".into()].into();
    proposal_context
        .targets
        .insert(previous.entry_id.clone(), target_snapshot);

    let mut first = preference_candidate();
    first.candidate_id = "repeat_1".into();
    first.classification = Classification::Repeat;
    first.source_ids = vec!["s1".into(), "s10".into()];
    first.target_ref = Some(reference.clone());
    first.dimension = None;
    first.summary = None;
    first.attitude = None;
    first.inferred = None;

    let mut second = first.clone();
    second.candidate_id = "repeat_2".into();
    second.source_ids = vec!["s2".into()];

    let material_positions = snapshot
        .spans
        .iter()
        .map(|span| (span.material_id.clone(), span.source.record_seq))
        .collect();
    let plan_set = build_plans_with_history(
        &snapshot,
        &Proposal {
            version: 1,
            candidates: vec![first, second],
            cycle_marks: vec![],
        },
        &proposal_context,
        &run,
        &BTreeMap::from([(previous.entry_id.clone(), previous)]),
        &BTreeMap::new(),
        &material_positions,
    )
    .unwrap();

    assert_eq!(plan_set.operations.len(), 1);
    let effects = &plan_set.operations[0].plan.targets[0].effects;
    assert_eq!(
        effects
            .iter()
            .map(|effect| effect.source.record_seq)
            .collect::<Vec<_>>(),
        [1, 2, 10]
    );
}

#[test]
fn repeat_candidate_creates_one_deterministic_effect_at_the_material_anchor() {
    let snapshot = batch(&new_id(), &new_id());
    let run = run(&snapshot.run_id, &snapshot.spans[0].source.session_id);
    let entry_id = new_id();
    let mut previous = build_plans(
        &snapshot,
        &Proposal {
            version: 1,
            candidates: vec![preference_candidate()],
            cycle_marks: vec![],
        },
        &context(),
        &run,
        &BTreeMap::new(),
    )
    .unwrap()
    .operations
    .remove(0)
    .plan
    .targets
    .remove(0)
    .next
    .unwrap();
    previous.entry_id = entry_id.clone();
    previous.version_id = new_id();
    let target_snapshot = TargetSnapshot {
        entry_id: entry_id.clone(),
        version_id: previous.version_id.clone(),
        state_revision: 3,
        state: EntryState::Active,
        subject: Subject::PlayerPreference,
        dimension: Dimension::PlayerPreference,
        summary: previous.summary.clone(),
        attitude: Attitude::Believed,
        inferred: false,
    };
    let mut context = context();
    context.targets.insert(entry_id.clone(), target_snapshot);
    let mut candidate = preference_candidate();
    candidate.candidate_id = "repeat_1".into();
    candidate.classification = Classification::Repeat;
    candidate.target_ref = Some(aoidos_memory::model::TargetRef {
        entry_id: entry_id.clone(),
        expected_version_id: previous.version_id.clone(),
        expected_state_revision: 3,
    });
    candidate.dimension = None;
    candidate.summary = None;
    candidate.attitude = None;
    candidate.inferred = None;
    let proposal = Proposal {
        version: 1,
        candidates: vec![candidate],
        cycle_marks: vec![],
    };
    let positions = BTreeMap::from([(snapshot.spans[0].material_id.clone(), 7)]);
    let previous_versions = BTreeMap::from([(entry_id.clone(), previous)]);

    let first = build_plans_with_history(
        &snapshot,
        &proposal,
        &context,
        &run,
        &previous_versions,
        &BTreeMap::new(),
        &positions,
    )
    .unwrap();
    let second = build_plans_with_history(
        &snapshot,
        &proposal,
        &context,
        &run,
        &previous_versions,
        &BTreeMap::new(),
        &positions,
    )
    .unwrap();
    assert_eq!(first.operations[0].plan, second.operations[0].plan);
    let target = &first.operations[0].plan.targets[0];
    assert_eq!(target.entry_id, entry_id);
    assert!(target.expected.version_id.is_some());
    assert_eq!(target.expected.state_revision, 3);
    assert!(target.next.is_none());
    assert_eq!(target.state, EntryState::Active);
    assert_eq!(target.effects.len(), 1);
    assert_eq!(target.effects[0].logical_position, 7);
    assert_eq!(target.effects[0].gain, run.policy.reinforcement_gain);
    assert!(first.operations[0].no_op_candidates.is_empty());

    let mut archived_snapshot = snapshot.clone();
    archived_snapshot.spans[0].source.record_seq = 2;
    archived_snapshot.spans[0].material_id = new_id();
    let mut archived_context = context;
    archived_context.targets.get_mut(&entry_id).unwrap().state = EntryState::Archived;
    let prior_effect = Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: snapshot.spans[0].source.clone(),
        logical_position: 0,
        gain: run.policy.reinforcement_gain,
        evidence: vec![],
    };
    let effects = BTreeMap::from([(entry_id.clone(), vec![(1, prior_effect)])]);
    let positions = BTreeMap::from([(archived_snapshot.spans[0].material_id.clone(), 0)]);
    let revived = build_plans_with_history(
        &archived_snapshot,
        &proposal,
        &archived_context,
        &run,
        &previous_versions,
        &effects,
        &positions,
    )
    .unwrap();
    let revived_target = &revived.operations[0].plan.targets[0];
    assert_eq!(revived_target.state, EntryState::Active);
    assert_eq!(revived_target.effects.len(), 1);
    assert_eq!(revived_target.effects[0].kind, EffectKind::Revival);
    assert_eq!(revived_target.effects[0].gain, 0);
}
