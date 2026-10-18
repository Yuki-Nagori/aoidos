use super::*;
use aoidos_memory::model::{Dimension, InputMode, Policy, SubjectRef, hash, new_id};
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
