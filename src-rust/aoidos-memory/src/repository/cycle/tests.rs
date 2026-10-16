use super::super::test_support::*;
use super::*;
use crate::model::{BeginSceneProgress, ClaimSceneHint, CompleteSceneProgress, SceneNodeKey};

fn mark(fixture: &Fixture, imprint_key: &str, priority: u16) -> CycleMarkV1 {
    let mut mark = CycleMarkV1 {
        version: 1,
        mark_id: String::new(),
        script_id: fixture.run.script_id.clone(),
        cycle_gen: 1,
        rule_id: "npc-familiarity".into(),
        rule_version: 1,
        subject_ids: vec!["keeper".into(), "player".into()],
        imprint_key: imprint_key.into(),
        source_refs: vec![CycleSourceRef {
            source: fixture.source(),
            history_revision: fixture.port.view.history_revision,
        }],
        kind: CycleKind::Familiarity,
        detail_policy: DetailPolicy::Vague,
        priority,
        summary: "对某个陌生人有模糊的熟悉感。".into(),
    };
    mark.mark_id = mark.stable_mark_id().unwrap();
    mark
}

fn archive_node(history_revision: u64, entry_record_seq: u64) -> SceneNodeKey<'static> {
    SceneNodeKey {
        scene_id: "archive",
        history_revision,
        entry_record_seq,
    }
}

fn begin_archive(
    fixture: &mut Fixture,
    history_revision: u64,
    entry_record_seq: u64,
    baseline: &[SceneRuleEvidence],
) -> SceneProgress {
    fixture
        .repo
        .begin_scene_progress(
            &mut fixture.conn,
            &fixture.port,
            BeginSceneProgress {
                run_id: &fixture.run.run_id,
                node: archive_node(history_revision, entry_record_seq),
                catalog_revision: "catalog-v1",
                baseline,
            },
        )
        .unwrap()
}

fn complete_archive(
    fixture: &mut Fixture,
    history_revision: u64,
    entry_record_seq: u64,
    round_id: &str,
    completed_record_seq: u64,
    evidence: &[SceneRuleEvidence],
) -> SceneProgressUpdate {
    fixture
        .repo
        .complete_scene_progress(
            &mut fixture.conn,
            &fixture.port,
            CompleteSceneProgress {
                run_id: &fixture.run.run_id,
                node: archive_node(history_revision, entry_record_seq),
                catalog_revision: "catalog-v1",
                round_id,
                completed_record_seq,
                evidence,
            },
        )
        .unwrap()
}

#[test]
fn authorized_source_is_persisted_once_and_duplicate_identity_only_adds_sources() {
    let mut fixture = Fixture::new_in_memory();
    let mut first = mark(&fixture, "first-rescue", 500);
    let stored = fixture
        .repo
        .record_cycle_mark(&mut fixture.conn, &fixture.port, &first)
        .unwrap();
    assert_eq!(stored.mark, first);
    assert_eq!(stored.status, CycleMarkStatus::Active);

    first.summary = "重复请求不能改写已确认的摘要。".into();
    let duplicate = fixture
        .repo
        .record_cycle_mark(&mut fixture.conn, &fixture.port, &first)
        .unwrap();
    assert_eq!(duplicate.mark.summary, "对某个陌生人有模糊的熟悉感。");
    assert_eq!(duplicate.mark.source_refs.len(), 1);

    let mut additional = first.clone();
    additional.source_refs[0].source.record_seq = 2;
    additional.source_refs[0].source.body_hash = hash("钟声".as_bytes());
    let merged = fixture
        .repo
        .record_cycle_mark(&mut fixture.conn, &fixture.port, &additional)
        .unwrap();
    assert_eq!(merged.mark.mark_id, stored.mark.mark_id);
    assert_eq!(merged.mark.source_refs.len(), 2);
    assert_eq!(
        fixture
            .repo
            .cycle_marks(&fixture.conn, &fixture.run.script_id, true)
            .unwrap(),
        vec![merged]
    );
}

#[test]
fn merging_sources_revalidates_existing_anchors_and_rejects_revision_conflicts() {
    let mut fixture = Fixture::new_in_memory();
    fixture.port.view.history_revision = 1;
    let mut candidate = mark(&fixture, "first-rescue", 500);
    candidate.source_refs[0].history_revision = 0;
    fixture
        .repo
        .record_cycle_mark(&mut fixture.conn, &fixture.port, &candidate)
        .unwrap();

    let mut conflicting = candidate.clone();
    conflicting.source_refs[0].history_revision = 1;
    assert!(matches!(
        fixture
            .repo
            .record_cycle_mark(&mut fixture.conn, &fixture.port, &conflicting),
        Err(Error::Rejected(Reason::InvalidSource))
    ));

    fixture.port.invalid_records.borrow_mut().insert(1);
    let mut additional = candidate.clone();
    additional.source_refs[0].source.record_seq = 2;
    assert!(matches!(
        fixture
            .repo
            .record_cycle_mark(&mut fixture.conn, &fixture.port, &additional),
        Err(Error::Rejected(Reason::InvalidSource))
    ));
    assert_eq!(
        fixture
            .repo
            .cycle_marks(&fixture.conn, &fixture.run.script_id, true)
            .unwrap()[0]
            .mark
            .source_refs
            .len(),
        1
    );
}

#[test]
fn cycle_mark_rejects_stale_cross_run_or_invalid_sources_without_writing() {
    let mut fixture = Fixture::new_in_memory();
    fixture.port.view.history_revision = 1;
    let historical_source = mark(&fixture, "earlier-current-branch", 500);
    assert!(
        fixture
            .repo
            .record_cycle_mark(&mut fixture.conn, &fixture.port, &historical_source)
            .is_ok()
    );

    let mut fixture = Fixture::new_in_memory();
    let mut invalid = mark(&fixture, "first-rescue", 500);
    invalid.source_refs[0].history_revision += 1;
    assert!(matches!(
        fixture
            .repo
            .record_cycle_mark(&mut fixture.conn, &fixture.port, &invalid),
        Err(Error::Rejected(Reason::InvalidSource))
    ));

    invalid = mark(&fixture, "first-rescue", 500);
    invalid.source_refs[0].source.run_id = new_id();
    assert!(
        fixture
            .repo
            .record_cycle_mark(&mut fixture.conn, &fixture.port, &invalid)
            .is_err()
    );
    assert!(
        fixture
            .repo
            .cycle_marks(&fixture.conn, &fixture.run.script_id, true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn active_marks_are_bounded_and_archived_by_priority_generation_and_id() {
    let mut fixture = Fixture::new_in_memory();
    let mut results = Vec::new();
    for index in 0..=ACTIVE_CYCLE_CAPACITY {
        let priority = u16::try_from(index).unwrap();
        let candidate = mark(&fixture, &format!("imprint-{index}"), priority);
        results.push(
            fixture
                .repo
                .record_cycle_mark(&mut fixture.conn, &fixture.port, &candidate)
                .unwrap(),
        );
    }

    let active = fixture
        .repo
        .cycle_marks(&fixture.conn, &fixture.run.script_id, false)
        .unwrap();
    let all = fixture
        .repo
        .cycle_marks(&fixture.conn, &fixture.run.script_id, true)
        .unwrap();
    assert_eq!(active.len(), ACTIVE_CYCLE_CAPACITY);
    assert_eq!(all.len(), ACTIVE_CYCLE_CAPACITY + 1);
    assert!(
        all.iter()
            .any(|mark| mark.status == CycleMarkStatus::Archived)
    );
    assert!(
        results
            .iter()
            .max_by_key(|stored| stored.mark.priority)
            .is_some_and(|highest| highest.status == CycleMarkStatus::Active)
    );
}

#[test]
fn corrupt_payload_or_source_digest_fails_closed() {
    let mut fixture = Fixture::new_in_memory();
    let candidate = mark(&fixture, "first-rescue", 500);
    let stored = fixture
        .repo
        .record_cycle_mark(&mut fixture.conn, &fixture.port, &candidate)
        .unwrap();
    fixture
        .conn
        .execute(
            "UPDATE memory_cycle_marks SET payload_hash=zeroblob(32) WHERE mark_id=?1",
            [&stored.mark.mark_id],
        )
        .unwrap();
    assert!(matches!(
        fixture
            .repo
            .cycle_marks(&fixture.conn, &fixture.run.script_id, true),
        Err(Error::Corrupt)
    ));
}

#[test]
fn cycle_source_revision_column_must_match_its_snapshot() {
    let mut fixture = Fixture::new_in_memory();
    let candidate = mark(&fixture, "first-rescue", 500);
    let stored = fixture
        .repo
        .record_cycle_mark(&mut fixture.conn, &fixture.port, &candidate)
        .unwrap();
    fixture
        .conn
        .execute(
            "UPDATE memory_cycle_sources SET history_revision=1 WHERE mark_id=?1",
            [&stored.mark.mark_id],
        )
        .unwrap();
    assert!(matches!(
        fixture
            .repo
            .cycle_marks(&fixture.conn, &fixture.run.script_id, true),
        Err(Error::Corrupt)
    ));
}

#[test]
fn absolute_cycle_capacity_rejects_new_marks_without_archiving_history_away() {
    let mut fixture = Fixture::new_in_memory();
    for index in 0..ABSOLUTE_CYCLE_CAPACITY {
        let candidate = mark(&fixture, &format!("imprint-{index}"), 500);
        fixture
            .repo
            .record_cycle_mark(&mut fixture.conn, &fixture.port, &candidate)
            .unwrap();
    }
    let rejected_mark = mark(&fixture, "imprint-over-capacity", 500);
    assert!(matches!(
        fixture
            .repo
            .record_cycle_mark(&mut fixture.conn, &fixture.port, &rejected_mark),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
    assert_eq!(
        fixture
            .repo
            .cycle_marks(&fixture.conn, &fixture.run.script_id, true)
            .unwrap()
            .len(),
        ABSOLUTE_CYCLE_CAPACITY
    );
}

#[test]
fn scene_progress_freezes_entry_baseline_and_counts_only_new_evidence_once() {
    let mut fixture = Fixture::new_in_memory();
    let baseline = SceneRuleEvidence {
        rule_id: "found-key".into(),
        evidence_json: br#""initial key state""#.to_vec(),
        evidence_hash: hash(br#""initial key state""#),
    };
    let entered = begin_archive(&mut fixture, 0, 0, std::slice::from_ref(&baseline));
    assert_eq!(entered.progress_count, 0);
    assert_eq!(entered.stagnation_rounds, 0);

    let newly_found = SceneRuleEvidence {
        rule_id: "found-key".into(),
        evidence_json: br#""key used at archive""#.to_vec(),
        evidence_hash: hash(br#""key used at archive""#),
    };
    let first_round = new_id();
    let first = complete_archive(
        &mut fixture,
        0,
        0,
        &first_round,
        1,
        std::slice::from_ref(&newly_found),
    );
    assert!(first.made_progress);
    assert_eq!(first.state.progress_count, 1);
    assert_eq!(first.state.stagnation_rounds, 0);
    assert_eq!(first.newly_observed_rules, vec!["found-key"]);

    let replay = complete_archive(
        &mut fixture,
        0,
        0,
        first.state.last_round_id.as_deref().unwrap(),
        1,
        std::slice::from_ref(&newly_found),
    );
    assert!(!replay.made_progress);
    assert_eq!(replay.state.revision, first.state.revision);
    assert_eq!(replay.state.completed_rounds, 1);

    let conflicting_evidence = SceneRuleEvidence {
        rule_id: "found-key".into(),
        evidence_json: br#""different evidence for the same round""#.to_vec(),
        evidence_hash: hash(br#""different evidence for the same round""#),
    };
    assert!(matches!(
        fixture.repo.complete_scene_progress(
            &mut fixture.conn,
            &fixture.port,
            CompleteSceneProgress {
                run_id: &fixture.run.run_id,
                node: archive_node(0, 0),
                catalog_revision: "catalog-v1",
                round_id: first.state.last_round_id.as_deref().unwrap(),
                completed_record_seq: 1,
                evidence: std::slice::from_ref(&conflicting_evidence),
            },
        ),
        Err(Error::Rejected(Reason::InvalidSource))
    ));

    let repeated = complete_archive(
        &mut fixture,
        0,
        0,
        &new_id(),
        2,
        std::slice::from_ref(&newly_found),
    );
    assert!(!repeated.made_progress);
    assert_eq!(repeated.state.progress_count, 1);
    assert_eq!(repeated.state.stagnation_rounds, 1);
}

#[test]
fn scene_reentry_gets_a_new_baseline_without_advancing_history_revision() {
    let mut fixture = Fixture::new_in_memory();
    let first = begin_archive(&mut fixture, 0, 0, &[]);
    let advanced = begin_archive(&mut fixture, 0, 10, &[]);
    assert_eq!(first.entry_history_revision, 0);
    assert_eq!(advanced.entry_history_revision, 0);
    assert_ne!(first.entry_record_seq, advanced.entry_record_seq);
    assert_eq!(advanced.completed_rounds, 0);
    assert_eq!(advanced.hints_shown, 0);
    assert_eq!(
        fixture
            .repo
            .scene_progress(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                archive_node(0, 0)
            )
            .unwrap()
            .unwrap(),
        first
    );
}

#[test]
fn scene_progress_requires_current_boundary_and_hint_claims_are_bounded() {
    let mut fixture = Fixture::new_in_memory();
    let mut progress = begin_archive(&mut fixture, 0, 0, &[]);

    let stale = fixture.repo.complete_scene_progress(
        &mut fixture.conn,
        &fixture.port,
        CompleteSceneProgress {
            run_id: &fixture.run.run_id,
            node: archive_node(1, 0),
            catalog_revision: "catalog-v1",
            round_id: &new_id(),
            completed_record_seq: 1,
            evidence: &[],
        },
    );
    assert!(matches!(stale, Err(Error::Rejected(Reason::InvalidSource))));

    for completed_record_seq in 1..=NODE_HINT_AFTER_STAGNANT_ROUNDS {
        progress = complete_archive(&mut fixture, 0, 0, &new_id(), completed_record_seq, &[]).state;
    }
    assert_eq!(progress.stagnation_rounds, NODE_HINT_AFTER_STAGNANT_ROUNDS);
    let claimed = fixture
        .repo
        .claim_scene_hint(
            &mut fixture.conn,
            &fixture.port,
            ClaimSceneHint {
                run_id: &fixture.run.run_id,
                node: archive_node(0, 0),
                expected_revision: progress.revision,
                hint_id: "find-the-hidden-door",
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(claimed.hints_shown, 1);
    assert_eq!(
        claimed.last_hint_id.as_deref(),
        Some("find-the-hidden-door")
    );
    assert!(
        fixture
            .repo
            .claim_scene_hint(
                &mut fixture.conn,
                &fixture.port,
                ClaimSceneHint {
                    run_id: &fixture.run.run_id,
                    node: archive_node(0, 0),
                    expected_revision: claimed.revision,
                    hint_id: "find-the-hidden-door",
                },
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn scene_progress_is_scoped_to_the_current_history_revision() {
    let mut fixture = Fixture::new_in_memory();
    let original = begin_archive(&mut fixture, 0, 0, &[]);

    fixture.port.view.history_revision = 1;
    assert!(matches!(
        fixture.repo.scene_progress(
            &fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            archive_node(0, 0),
        ),
        Err(Error::Rejected(Reason::InvalidSource))
    ));

    let forked = begin_archive(&mut fixture, 1, 0, &[]);
    assert_eq!(forked.entry_record_seq, original.entry_record_seq);
    assert_eq!(forked.entry_history_revision, 1);
    assert_eq!(forked.completed_rounds, 0);
    assert_eq!(forked.revision, 0);
}
