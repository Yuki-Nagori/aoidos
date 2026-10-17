//! 稳定素材与冻结批次的身份、来源和预算契约。

use super::super::test_support::*;
use super::*;
use crate::model::{CycleKind, CycleMarkV1, CycleSourceRef, DetailPolicy};

fn cycle_mark_commit(f: &Fixture) -> CycleMarkCommit {
    let mut mark = CycleMarkV1 {
        version: 1,
        mark_id: String::new(),
        script_id: f.run.script_id.clone(),
        cycle_gen: 1,
        rule_id: "rescue".into(),
        rule_version: 1,
        subject_ids: vec!["guard".into()],
        imprint_key: "rescue-imprint".into(),
        source_refs: vec![CycleSourceRef {
            source: f.source(),
            history_revision: 0,
        }],
        kind: CycleKind::Fragment,
        detail_policy: DetailPolicy::Vague,
        priority: 10,
        summary: "Guard remembers help".into(),
    };
    mark.mark_id = mark.stable_mark_id().unwrap();
    CycleMarkCommit {
        candidate_id: "mark_1".into(),
        mark,
    }
}

#[test]
fn material_identity_and_frozen_batch_survive_replays() {
    let mut f = Fixture::new();
    let source = f.source();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &source, 0)
        .unwrap();
    assert_eq!(
        f.repo
            .register_material(&f.conn, &f.port, &source, 0)
            .unwrap(),
        material
    );
    rejected(
        f.repo.register_material(&f.conn, &f.port, &source, 1),
        Reason::InvalidSource,
    );
    let mut different = source.clone();
    different.end_byte = 3;
    assert!(matches!(
        f.repo.register_material(&f.conn, &f.port, &different, 0),
        Err(Error::Corrupt)
    ));
    let snapshot = BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: f.run.policy_hash,
        spans: vec![Span {
            source: f.source(),
            source_id: "s1".into(),
            material_id: material,
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({"previous": []}),
    };
    let digest = f
        .repo
        .freeze_batch(&mut f.conn, &f.port, &snapshot)
        .unwrap();
    assert_eq!(
        f.repo
            .freeze_batch(&mut f.conn, &f.port, &snapshot)
            .unwrap(),
        digest
    );
    assert_eq!(
        f.repo.batch(&f.conn, &snapshot.batch_id).unwrap(),
        (snapshot.clone(), BatchState::Queued)
    );
    let mut conflict = snapshot.clone();
    conflict.input = serde_json::json!({"previous": [1]});
    assert!(matches!(
        f.repo.freeze_batch(&mut f.conn, &f.port, &conflict),
        Err(Error::Corrupt)
    ));
    conflict = snapshot.clone();
    conflict.spans[0].source_id = "s01".into();
    rejected(
        f.repo.freeze_batch(&mut f.conn, &f.port, &conflict),
        Reason::InvalidSchema,
    );
    conflict = snapshot.clone();
    conflict.spans[0].text = "伪造".into();
    rejected(
        f.repo.freeze_batch(&mut f.conn, &f.port, &conflict),
        Reason::InvalidSource,
    );
    f.conn
        .execute(
            "DELETE FROM memory_spans WHERE batch_id=?1",
            [&snapshot.batch_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.batch(&f.conn, &snapshot.batch_id),
        Err(Error::Corrupt)
    ));
    rejected(f.repo.batch(&f.conn, "bad"), Reason::InvalidSchema);
    assert!(matches!(
        f.repo.batch(&f.conn, &new_id()),
        Err(Error::NotFound)
    ));
}

#[test]
fn queued_batch_reserves_material_until_rejected_and_processed_material_cannot_be_frozen() {
    let mut f = Fixture::new();
    let source = f.source();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &source, 0)
        .unwrap();
    let snapshot = |batch_id| BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id,
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: f.run.policy_hash,
        spans: vec![Span {
            source_id: "s1".into(),
            material_id: material.clone(),
            source: source.clone(),
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    };
    let first = snapshot(new_id());
    assert!(
        f.repo
            .material_is_collectible(&f.conn, &f.port, &f.run.run_id, &material)
            .unwrap()
    );
    let digest = f.repo.freeze_batch(&mut f.conn, &f.port, &first).unwrap();
    assert!(
        !f.repo
            .material_is_collectible(&f.conn, &f.port, &f.run.run_id, &material)
            .unwrap()
    );
    assert!(matches!(
        f.repo
            .freeze_batch(&mut f.conn, &f.port, &snapshot(new_id())),
        Err(Error::Rejected(Reason::InvalidSource))
    ));

    f.conn
        .execute(
            "UPDATE memory_batches SET status='rejected' WHERE batch_id=?1",
            [&first.batch_id],
        )
        .unwrap();
    assert!(
        f.repo
            .material_is_collectible(&f.conn, &f.port, &f.run.run_id, &material)
            .unwrap()
    );
    let retry = snapshot(new_id());
    f.repo.freeze_batch(&mut f.conn, &f.port, &retry).unwrap();
    f.conn
        .execute(
            "UPDATE memory_materials SET status='processed' WHERE material_id=?1",
            [&material],
        )
        .unwrap();
    assert!(
        !f.repo
            .material_is_collectible(&f.conn, &f.port, &f.run.run_id, &material)
            .unwrap()
    );
    assert_eq!(
        f.repo.freeze_batch(&mut f.conn, &f.port, &first).unwrap(),
        digest
    );
    assert!(matches!(
        f.repo
            .freeze_batch(&mut f.conn, &f.port, &snapshot(new_id())),
        Err(Error::Rejected(Reason::InvalidSource))
    ));
}

#[test]
fn deferred_candidates_are_unavailable_before_batch_completion() {
    let mut f = Fixture::new();
    let snapshot = f.received_batch("candidate_1");

    rejected(
        f.repo.deferred_candidates(&f.conn, &snapshot.batch_id),
        Reason::StaleVersion,
    );
}

#[test]
fn logical_clock_counts_only_confirmed_rounds_once() {
    let mut f = Fixture::new();
    let source = f.source();
    let id = new_id();
    assert_eq!(
        f.repo
            .complete_round(&mut f.conn, &f.port, &f.run.run_id, &id, &source)
            .unwrap(),
        1
    );
    assert_eq!(
        f.repo
            .complete_round(&mut f.conn, &f.port, &f.run.run_id, &id, &source)
            .unwrap(),
        1
    );
    let mut changed = source.clone();
    changed.record_seq += 1;
    assert!(matches!(
        f.repo
            .complete_round(&mut f.conn, &f.port, &f.run.run_id, &id, &changed),
        Err(Error::Corrupt)
    ));
    changed.mode = Some(InputMode::OutOfCharacter);
    rejected(
        f.repo
            .complete_round(&mut f.conn, &f.port, &f.run.run_id, &new_id(), &changed),
        Reason::InvalidSource,
    );
}

#[test]
fn batch_digest_and_capacity_reject_untrusted_snapshots() {
    let mut f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let mut snapshot = BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: f.run.policy_hash,
        spans: vec![Span {
            source: f.source(),
            source_id: "s1".into(),
            material_id: material,
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    };
    snapshot.input = serde_json::json!({"oversize": "x".repeat(65536)});
    rejected(
        f.repo.freeze_batch(&mut f.conn, &f.port, &snapshot),
        Reason::CapacityBlocked,
    );
    snapshot.input = serde_json::json!({});
    f.repo
        .freeze_batch(&mut f.conn, &f.port, &snapshot)
        .unwrap();
    f.conn
        .execute("UPDATE memory_batches SET input_digest=zeroblob(32)", [])
        .unwrap();
    assert!(matches!(
        f.repo.batch(&f.conn, &snapshot.batch_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn oversized_persisted_snapshot_is_rejected_before_blob_copy() {
    let mut f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let snapshot = BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: f.run.policy_hash,
        spans: vec![Span {
            source: f.source(),
            source_id: "s1".into(),
            material_id: material,
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    };
    f.repo
        .freeze_batch(&mut f.conn, &f.port, &snapshot)
        .unwrap();
    f.conn
        .execute(
            "UPDATE memory_batches SET snapshot_json=zeroblob(?1) WHERE batch_id=?2",
            params![64 * 1024 + 1, snapshot.batch_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.batch(&f.conn, &snapshot.batch_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn identical_text_cannot_replace_frozen_record_identity() {
    let mut f = Fixture::new();
    let source = f.source();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &source, 0)
        .unwrap();
    let snapshot = BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: f.run.policy_hash,
        spans: vec![Span {
            source_id: "s1".into(),
            material_id: material.clone(),
            source: source.clone(),
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    };

    f.repo
        .freeze_batch(&mut f.conn, &f.port, &snapshot)
        .unwrap();
    let mut same_text_source = source.clone();
    same_text_source.record_seq = 2;
    assert_eq!(
        f.port.source_text(&same_text_source).unwrap(),
        f.port.source_text(&source).unwrap()
    );
    let mut replacement = snapshot.clone();
    replacement.batch_id = new_id();
    replacement.spans[0].source = same_text_source.clone();
    rejected(
        f.repo.freeze_batch(&mut f.conn, &f.port, &replacement),
        Reason::InvalidSource,
    );

    // 即使 SQL 中的原文仍一致，也要分别校验素材行身份与已冻结来源。
    f.conn
        .execute(
            "UPDATE memory_materials SET source_json=?1 WHERE material_id=?2",
            params![canonical(&same_text_source).unwrap(), material],
        )
        .unwrap();
    assert!(matches!(
        f.repo.batch(&f.conn, &snapshot.batch_id),
        Err(Error::Corrupt)
    ));
    f.conn
        .execute(
            "UPDATE memory_materials SET record_seq=2 WHERE material_id=?1",
            [&material],
        )
        .unwrap();
    assert!(matches!(
        f.repo.batch(&f.conn, &snapshot.batch_id),
        Err(Error::Corrupt)
    ));
    rejected(
        f.repo.freeze_batch(&mut f.conn, &f.port, &snapshot),
        Reason::InvalidSource,
    );
}

#[test]
fn batch_request_identity_and_results_are_durable_and_idempotent() {
    let mut f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let snapshot = BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: f.run.policy_hash,
        spans: vec![Span {
            source_id: "s1".into(),
            material_id: material,
            source: f.source(),
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    };
    f.repo
        .freeze_batch(&mut f.conn, &f.port, &snapshot)
        .unwrap();
    let turn_id = new_id();
    let request_id = new_id();

    assert!(
        f.repo
            .dispatch_batch(
                &mut f.conn,
                &f.port,
                &snapshot.batch_id,
                &turn_id,
                &request_id
            )
            .unwrap()
    );
    assert!(
        !f.repo
            .dispatch_batch(
                &mut f.conn,
                &f.port,
                &snapshot.batch_id,
                &turn_id,
                &request_id
            )
            .unwrap()
    );
    rejected(
        f.repo.dispatch_batch(
            &mut f.conn,
            &f.port,
            &snapshot.batch_id,
            &new_id(),
            &new_id(),
        ),
        Reason::StaleVersion,
    );

    rejected(
        f.repo.receive_batch(
            &mut f.conn,
            &snapshot.batch_id,
            &turn_id,
            &request_id,
            b"{bad}",
        ),
        Reason::InvalidSchema,
    );
    let response = br#"{"version":1,"candidates":[]}"#;
    assert!(
        f.repo
            .receive_batch(
                &mut f.conn,
                &snapshot.batch_id,
                &turn_id,
                &request_id,
                response
            )
            .unwrap()
    );
    assert!(
        !f.repo
            .receive_batch(
                &mut f.conn,
                &snapshot.batch_id,
                &turn_id,
                &request_id,
                response
            )
            .unwrap()
    );
    rejected(
        f.repo.receive_batch(
            &mut f.conn,
            &snapshot.batch_id,
            &turn_id,
            &request_id,
            br#"{ "version": 1, "candidates": [] }"#,
        ),
        Reason::StaleVersion,
    );
    let receipt = f.repo.batch_receipt(&f.conn, &snapshot.batch_id).unwrap();
    assert_eq!(receipt.turn_id, turn_id);
    assert_eq!(receipt.request_id, request_id);
    assert_eq!(receipt.response.as_deref(), Some(response.as_slice()));
    assert!(receipt.completion.is_none());
    assert_eq!(
        f.repo.batch(&f.conn, &snapshot.batch_id).unwrap().1,
        BatchState::Received
    );

    let completion = br#"{"version":1,"outcomes":[]}"#;
    assert!(
        f.repo
            .complete_batch(&mut f.conn, &snapshot.batch_id, completion)
            .unwrap()
    );
    assert!(
        !f.repo
            .complete_batch(&mut f.conn, &snapshot.batch_id, completion)
            .unwrap()
    );
    rejected(
        f.repo.complete_batch(
            &mut f.conn,
            &snapshot.batch_id,
            br#"{ "version": 1, "outcomes": [] }"#,
        ),
        Reason::StaleVersion,
    );
    assert_eq!(
        f.repo.batch(&f.conn, &snapshot.batch_id).unwrap().1,
        BatchState::Completed
    );
    assert_eq!(
        f.repo
            .batch_receipt(&f.conn, &snapshot.batch_id)
            .unwrap()
            .completion
            .as_deref(),
        Some(completion.as_slice())
    );
}

#[test]
fn unknown_and_rejected_batches_cannot_be_redispatched_or_overwritten() {
    let mut f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let source = f.source();
    let run_id = f.run.run_id.clone();
    let policy_hash = f.run.policy_hash;
    let make_snapshot = || BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash,
        spans: vec![Span {
            source_id: "s1".into(),
            material_id: material.clone(),
            source: source.clone(),
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    };
    let unknown = make_snapshot();
    f.repo.freeze_batch(&mut f.conn, &f.port, &unknown).unwrap();
    let turn_id = new_id();
    let request_id = new_id();
    f.port.view.history_revision += 1;
    rejected(
        f.repo.dispatch_batch(
            &mut f.conn,
            &f.port,
            &unknown.batch_id,
            &turn_id,
            &request_id,
        ),
        Reason::StaleVersion,
    );
    f.port.view.history_revision -= 1;
    assert!(
        f.repo
            .dispatch_batch(
                &mut f.conn,
                &f.port,
                &unknown.batch_id,
                &turn_id,
                &request_id
            )
            .unwrap()
    );
    assert!(
        f.repo
            .mark_batch_unknown(
                &mut f.conn,
                &unknown.batch_id,
                &turn_id,
                &request_id,
                "network.ambiguous",
            )
            .unwrap()
    );
    assert!(
        !f.repo
            .mark_batch_unknown(
                &mut f.conn,
                &unknown.batch_id,
                &turn_id,
                &request_id,
                "network.ambiguous",
            )
            .unwrap()
    );
    assert_eq!(
        f.repo.batch(&f.conn, &unknown.batch_id).unwrap().1,
        BatchState::Unknown
    );
    assert!(matches!(
        f.repo.freeze_batch(&mut f.conn, &f.port, &make_snapshot()),
        Err(Error::Rejected(Reason::InvalidSource))
    ));
    assert!(f.repo.freeze_batch(&mut f.conn, &f.port, &unknown).is_ok());
    assert!(
        !f.repo
            .dispatch_batch(
                &mut f.conn,
                &f.port,
                &unknown.batch_id,
                &turn_id,
                &request_id
            )
            .unwrap()
    );
    rejected(
        f.repo.receive_batch(
            &mut f.conn,
            &unknown.batch_id,
            &turn_id,
            &request_id,
            br#"{}"#,
        ),
        Reason::StaleVersion,
    );

    let mut rejected_source = source.clone();
    rejected_source.record_seq += 1;
    let rejected_material = f
        .repo
        .register_material(&f.conn, &f.port, &rejected_source, 0)
        .unwrap();
    let mut rejected_batch = make_snapshot();
    rejected_batch.batch_id = new_id();
    rejected_batch.spans[0].source = rejected_source;
    rejected_batch.spans[0].material_id = rejected_material;
    f.repo
        .freeze_batch(&mut f.conn, &f.port, &rejected_batch)
        .unwrap();
    let turn_id = new_id();
    let request_id = new_id();
    f.repo
        .dispatch_batch(
            &mut f.conn,
            &f.port,
            &rejected_batch.batch_id,
            &turn_id,
            &request_id,
        )
        .unwrap();
    assert!(
        f.repo
            .reject_batch(&mut f.conn, &rejected_batch.batch_id, "response.schema")
            .unwrap()
    );
    assert!(
        !f.repo
            .reject_batch(&mut f.conn, &rejected_batch.batch_id, "response.schema")
            .unwrap()
    );
    rejected(
        f.repo
            .reject_batch(&mut f.conn, &rejected_batch.batch_id, "other.error"),
        Reason::StaleVersion,
    );
    assert_eq!(
        f.repo.batch(&f.conn, &rejected_batch.batch_id).unwrap().1,
        BatchState::Rejected
    );
    assert_eq!(
        f.repo
            .batch_receipt(&f.conn, &rejected_batch.batch_id)
            .unwrap()
            .error_code
            .as_deref(),
        Some("response.schema")
    );
}

#[test]
fn completion_must_resolve_exactly_each_persisted_candidate_once() {
    let mut f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let snapshot = BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: f.run.policy_hash,
        spans: vec![Span {
            source_id: "s1".into(),
            material_id: material,
            source: f.source(),
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    };
    f.repo
        .freeze_batch(&mut f.conn, &f.port, &snapshot)
        .unwrap();
    let turn_id = new_id();
    let request_id = new_id();
    f.repo
        .dispatch_batch(
            &mut f.conn,
            &f.port,
            &snapshot.batch_id,
            &turn_id,
            &request_id,
        )
        .unwrap();
    let proposal = crate::model::Proposal {
        version: 1,
        candidates: vec![Candidate {
            candidate_id: "candidate_1".into(),
            classification: crate::model::Classification::NotWorthy,
            source_ids: vec!["s1".into()],
            subject_ref: crate::model::SubjectRef::PlayerPreference,
            entity_ids: vec![],
            evidence_ids: vec![],
            target_ref: None,
            dimension: None,
            summary: None,
            attitude: None,
            attitude_evidence_ids: vec![],
            inferred: None,
            reason: "没有新的稳定信息".into(),
        }],
        cycle_marks: vec![],
    };
    let response = serde_json::to_vec(&proposal).unwrap();
    f.repo
        .receive_batch(
            &mut f.conn,
            &snapshot.batch_id,
            &turn_id,
            &request_id,
            &response,
        )
        .unwrap();

    rejected(
        f.repo.complete_batch(
            &mut f.conn,
            &snapshot.batch_id,
            br#"{"version":1,"outcomes":[]}"#,
        ),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo.complete_batch(
            &mut f.conn,
            &snapshot.batch_id,
            br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"rejected"}]}"#,
        ),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo.complete_batch(
            &mut f.conn,
            &snapshot.batch_id,
            br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"applied"}]}"#,
        ),
        Reason::InvalidSchema,
    );
    let completion =
        br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"noOp"}]}"#;
    assert!(
        f.repo
            .complete_batch(&mut f.conn, &snapshot.batch_id, completion)
            .unwrap()
    );
    assert_eq!(
        f.repo
            .batch_receipt(&f.conn, &snapshot.batch_id)
            .unwrap()
            .state,
        BatchState::Completed
    );
}

#[test]
fn material_registration_requires_a_valid_source_and_bounded_ordinal() {
    let f = Fixture::new();
    let source = f.source();

    f.port.invalid.set(true);
    rejected(
        f.repo.register_material(&f.conn, &f.port, &source, 0),
        Reason::InvalidSource,
    );

    f.port.invalid.set(false);
    rejected(
        f.repo
            .register_material(&f.conn, &f.port, &source, MAX_INTEGER + 1),
        Reason::InvalidSource,
    );
}

#[test]
fn batch_reads_reject_unsupported_versions_and_identity_mismatch() {
    let mut f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let snapshot = BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: f.run.policy_hash,
        spans: vec![Span {
            source: f.source(),
            source_id: "s1".into(),
            material_id: material,
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    };

    let mut invalid = snapshot.clone();
    invalid.version += 1;
    rejected(
        f.repo.freeze_batch(&mut f.conn, &f.port, &invalid),
        Reason::InvalidSchema,
    );

    f.repo
        .freeze_batch(&mut f.conn, &f.port, &snapshot)
        .unwrap();

    let mut future = snapshot.clone();
    future.version += 1;
    let bytes = canonical(&future).unwrap();
    f.conn
        .execute(
            "UPDATE memory_batches SET snapshot_json=?1,input_digest=?2 WHERE batch_id=?3",
            params![bytes.clone(), hash(&bytes).as_slice(), snapshot.batch_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.batch(&f.conn, &snapshot.batch_id),
        Err(Error::Rejected(Reason::PolicyUnavailable))
    ));

    let stored_id = snapshot.batch_id.clone();
    let mut mismatched = snapshot;
    mismatched.batch_id = new_id();
    let bytes = canonical(&mismatched).unwrap();
    f.conn
        .execute(
            "UPDATE memory_batches SET snapshot_json=?1,input_digest=?2 WHERE batch_id=?3",
            params![bytes.clone(), hash(&bytes).as_slice(), stored_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.batch(&f.conn, &stored_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn completed_round_clock_stops_at_safe_integer_limit() {
    let mut f = Fixture::new();
    f.conn
        .execute(
            "UPDATE memory_runs SET logical_clock=?1 WHERE run_id=?2",
            params![MAX_INTEGER, f.run.run_id],
        )
        .unwrap();
    let source = f.source();

    rejected(
        f.repo
            .complete_round(&mut f.conn, &f.port, &f.run.run_id, &new_id(), &source),
        Reason::CapacityBlocked,
    );
}

#[test]
fn malformed_persisted_material_source_is_reported_as_corruption() {
    let f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let mut invalid = f.source();
    invalid.end_byte = invalid.start_byte;
    let invalid_bytes = canonical(&invalid).unwrap();
    f.conn
        .execute(
            "UPDATE memory_materials SET source_json=?1 WHERE material_id=?2",
            params![invalid_bytes, material],
        )
        .unwrap();

    assert!(matches!(
        f.repo.material_source(&f.conn, &f.run.run_id, &material),
        Err(Error::Corrupt)
    ));
}

fn queued_fixture() -> (Fixture, String, String, String) {
    let mut f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let snapshot = BatchSnapshot {
        logical_clock: 0,
        version: 1,
        batch_id: new_id(),
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        entity_revision: 1,
        evidence_revision: 1,
        policy_hash: f.run.policy_hash,
        spans: vec![Span {
            source_id: "s1".into(),
            material_id: material,
            source: f.source(),
            text: "钟声".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::json!({}),
    };
    f.repo
        .freeze_batch(&mut f.conn, &f.port, &snapshot)
        .unwrap();
    (f, snapshot.batch_id, new_id(), new_id())
}
fn dispatched_fixture() -> (Fixture, String, String, String) {
    let (mut f, batch, turn, request) = queued_fixture();
    f.repo
        .dispatch_batch(&mut f.conn, &f.port, &batch, &turn, &request)
        .unwrap();
    (f, batch, turn, request)
}
const EMPTY_RESPONSE: &[u8] = br#"{"version":1,"candidates":[]}"#;
const EMPTY_COMPLETION: &[u8] = br#"{"version":1,"outcomes":[]}"#;

#[test]
fn batch_operations_reject_invalid_identity_limits_and_wrong_states() {
    let (mut f, batch, turn, request) = queued_fixture();
    rejected(
        f.repo.complete_batch(&mut f.conn, &batch, b"bad json"),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo
            .dispatch_batch(&mut f.conn, &f.port, "bad", &turn, &request),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo
            .receive_batch(&mut f.conn, "bad", &turn, &request, EMPTY_RESPONSE),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo.receive_batch(
            &mut f.conn,
            &batch,
            &turn,
            &request,
            &vec![0; MAX_PROPOSAL_BYTES + 1],
        ),
        Reason::CapacityBlocked,
    );
    rejected(
        f.repo.complete_batch(&mut f.conn, "bad", EMPTY_COMPLETION),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo
            .complete_batch(&mut f.conn, &batch, &vec![0; MAX_PROPOSAL_BYTES + 1]),
        Reason::CapacityBlocked,
    );
    rejected(
        f.repo.complete_batch(&mut f.conn, &batch, EMPTY_COMPLETION),
        Reason::StaleVersion,
    );
    rejected(
        f.repo.reject_batch(&mut f.conn, "bad", "schema"),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo.reject_batch(&mut f.conn, &batch, "bad/code"),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo.reject_batch(&mut f.conn, &batch, "schema"),
        Reason::StaleVersion,
    );
    rejected(
        f.repo
            .mark_batch_unknown(&mut f.conn, &batch, &turn, &request, "bad/code"),
        Reason::InvalidSchema,
    );
    rejected(f.repo.batch_receipt(&f.conn, "bad"), Reason::InvalidSchema);
    f.repo
        .dispatch_batch(&mut f.conn, &f.port, &batch, &turn, &request)
        .unwrap();
    assert_eq!(
        f.repo.batch_receipt(&f.conn, &batch).unwrap().state,
        BatchState::Dispatched
    );
    rejected(
        f.repo
            .receive_batch(&mut f.conn, &batch, &new_id(), &request, EMPTY_RESPONSE),
        Reason::StaleVersion,
    );
    rejected(
        f.repo
            .mark_batch_unknown(&mut f.conn, &batch, &turn, &new_id(), "network"),
        Reason::StaleVersion,
    );
    f.repo
        .receive_batch(&mut f.conn, &batch, &turn, &request, EMPTY_RESPONSE)
        .unwrap();
    rejected(
        f.repo
            .mark_batch_unknown(&mut f.conn, &batch, &turn, &request, "network"),
        Reason::StaleVersion,
    );
    f.repo.reject_batch(&mut f.conn, &batch, "schema").unwrap();
    assert_eq!(
        f.repo.batch_receipt(&f.conn, &batch).unwrap().state,
        BatchState::Rejected
    );
    let (mut f, batch, turn, request) = dispatched_fixture();
    f.repo
        .mark_batch_unknown(&mut f.conn, &batch, &turn, &request, "network")
        .unwrap();
    assert_eq!(
        f.repo.batch_receipt(&f.conn, &batch).unwrap().state,
        BatchState::Unknown
    );
}

#[test]
fn batch_transitions_roll_back_when_sql_does_not_update_the_expected_row() {
    for (target, state, operation) in [
        ("memory_batches", "dispatched", "dispatch"),
        ("memory_batch_receipts", "", "receive"),
        ("memory_batches", "received", "receive"),
        ("memory_batch_receipts", "", "complete"),
        ("memory_batches", "completed", "complete"),
    ] {
        let (mut f, batch, turn, request) = queued_fixture();
        if operation != "dispatch" {
            f.repo
                .dispatch_batch(&mut f.conn, &f.port, &batch, &turn, &request)
                .unwrap();
        }
        if operation == "complete" {
            f.repo
                .receive_batch(&mut f.conn, &batch, &turn, &request, EMPTY_RESPONSE)
                .unwrap();
        }
        let before = f.repo.batch(&f.conn, &batch).unwrap().1;
        let condition = if state.is_empty() {
            "1".to_string()
        } else {
            format!("NEW.status='{state}'")
        };
        f.conn.execute_batch(&format!("CREATE TRIGGER prevent_transition BEFORE UPDATE ON {target} WHEN {condition} BEGIN SELECT RAISE(IGNORE); END;")).unwrap();
        let result = match operation {
            "dispatch" => f
                .repo
                .dispatch_batch(&mut f.conn, &f.port, &batch, &turn, &request),
            "receive" => f
                .repo
                .receive_batch(&mut f.conn, &batch, &turn, &request, EMPTY_RESPONSE),
            _ => f.repo.complete_batch(&mut f.conn, &batch, EMPTY_COMPLETION),
        };
        assert!(matches!(
            result,
            Err(Error::Corrupt | Error::Rejected(Reason::StaleVersion))
        ));
        assert_eq!(f.repo.batch(&f.conn, &batch).unwrap().1, before);
    }
    let (mut f, batch, _, _) = dispatched_fixture();
    f.conn
        .execute("DELETE FROM memory_batch_receipts", [])
        .unwrap();
    assert!(matches!(
        f.repo.reject_batch(&mut f.conn, &batch, "schema"),
        Err(Error::Corrupt)
    ));
}

#[test]
fn receipt_integrity_rejects_corrupt_hashes_payloads_and_state_combinations() {
    for mutation in [
        "UPDATE memory_batch_receipts SET turn_id='xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'",
        "UPDATE memory_batch_receipts SET response_hash=zeroblob(32),response_json=x'7b7d'",
        "UPDATE memory_batches SET result_hash=zeroblob(32)",
        "UPDATE memory_batches SET status='queued'",
        "UPDATE memory_batches SET status='received'",
        "UPDATE memory_batch_receipts SET completion_hash=zeroblob(32)",
        "UPDATE memory_batch_receipts SET response_hash=zeroblob(32)",
        "UPDATE memory_batch_receipts SET response_json=zeroblob(65537),response_hash=zeroblob(32)",
    ] {
        let (f, batch, _, _) = dispatched_fixture();
        f.conn
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        f.conn.execute_batch(mutation).unwrap();
        assert!(
            matches!(f.repo.batch_receipt(&f.conn, &batch), Err(Error::Corrupt)),
            "{mutation}"
        );
    }
    let (mut f, batch, turn, request) = dispatched_fixture();
    f.repo
        .receive_batch(&mut f.conn, &batch, &turn, &request, EMPTY_RESPONSE)
        .unwrap();
    f.conn
        .execute(
            "UPDATE memory_batch_receipts SET response_hash=zeroblob(32)",
            [],
        )
        .unwrap();
    assert!(matches!(
        f.repo
            .receive_batch(&mut f.conn, &batch, &turn, &request, EMPTY_RESPONSE),
        Err(Error::Corrupt)
    ));
    let (mut f, batch, turn, request) = dispatched_fixture();
    f.repo
        .receive_batch(&mut f.conn, &batch, &turn, &request, EMPTY_RESPONSE)
        .unwrap();
    f.conn.execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE memory_batch_receipts SET response_json=NULL,response_hash=NULL; UPDATE memory_batches SET result_hash=NULL;").unwrap();
    assert!(matches!(
        f.repo.complete_batch(&mut f.conn, &batch, EMPTY_COMPLETION),
        Err(Error::Corrupt)
    ));
}

#[test]
fn completion_and_response_protocols_validate_versions_identity_sets_and_reason_codes() {
    let candidate = serde_json::json!({"candidateId":"c1","classification":"notWorthy","sourceIds":["s1"],"subjectRef":{"kind":"playerPreference"},"reason":"skip"});
    let valid = serde_json::json!({"version":1,"candidates":[candidate.clone()]});
    let cycle_mark = serde_json::json!({"candidateId":"mark_1","ruleId":"rescue","sourceIds":["s1"],"subjectIds":["guard"],"summary":"守卫记得一次援助"});
    let valid_with_cycle_mark =
        serde_json::json!({"version":1,"candidates":[],"cycleMarks":[cycle_mark.clone()]});
    rejected(
        validate_proposal_identity(&crate::model::Proposal {
            version: 2,
            candidates: vec![],
            cycle_marks: vec![],
        }),
        Reason::PolicyUnavailable,
    );
    for (proposal, reason) in [
        (
            serde_json::json!({"version":2,"candidates":[]}),
            Reason::PolicyUnavailable,
        ),
        (
            serde_json::json!({"version":1,"candidates":vec![candidate.clone(); MAX_CANDIDATES+1]}),
            Reason::CapacityBlocked,
        ),
        (
            serde_json::json!({"version":1,"candidates":[candidate.clone(),candidate.clone()]}),
            Reason::InvalidSchema,
        ),
        (
            serde_json::json!({"version":1,"candidates":[],"cycleMarks":vec![cycle_mark.clone(); crate::model::MAX_CYCLE_MARK_PROPOSALS+1]}),
            Reason::CapacityBlocked,
        ),
        (
            serde_json::json!({"version":1,"candidates":[],"cycleMarks":[cycle_mark.clone(),cycle_mark.clone()]}),
            Reason::InvalidSchema,
        ),
    ] {
        rejected(
            decode_proposal_response(&serde_json::to_vec(&proposal).unwrap()),
            reason,
        );
    }
    assert!(decode_proposal_response(&serde_json::to_vec(&valid_with_cycle_mark).unwrap()).is_ok());
    let (mut f, batch, turn, request) = dispatched_fixture();
    f.repo
        .receive_batch(
            &mut f.conn,
            &batch,
            &turn,
            &request,
            &serde_json::to_vec(&valid).unwrap(),
        )
        .unwrap();
    for completion in [
        serde_json::json!({"version":2,"outcomes":[]}),
        serde_json::json!({"version":1,"outcomes":[{"candidateId":"unknown","outcome":"noOp"}]}),
        serde_json::json!({"version":1,"outcomes":[{"candidateId":"c1","outcome":"rejected","reasonCode":"bad/code"}]}),
        serde_json::json!({"version":1,"outcomes":[{"candidateId":"c1","outcome":"rejected","reasonCode":""}]}),
        serde_json::json!({"version":1,"outcomes":[{"candidateId":"c1","outcome":"noOp"},{"candidateId":"c1","outcome":"noOp"}]}),
    ] {
        rejected(
            f.repo.complete_batch(
                &mut f.conn,
                &batch,
                &serde_json::to_vec(&completion).unwrap(),
            ),
            Reason::InvalidSchema,
        );
    }
    let completion = serde_json::json!({"version":1,"outcomes":[{"candidateId":"c1","outcome":"deferred","reasonCode":"policy.capacity_blocked-1"}]});
    f.repo
        .complete_batch(
            &mut f.conn,
            &batch,
            &serde_json::to_vec(&completion).unwrap(),
        )
        .unwrap();
    assert_eq!(
        f.repo.batch_receipt(&f.conn, &batch).unwrap().state,
        BatchState::Completed
    );
    for proposal in [
        serde_json::json!({"version":2,"candidates":[]}),
        serde_json::json!({"version":1,"candidates":[candidate.clone(),candidate]}),
    ] {
        f.conn
            .execute(
                "UPDATE memory_batch_receipts SET response_json=?1",
                [serde_json::to_vec(&proposal).unwrap()],
            )
            .unwrap();
        assert!(matches!(
            f.repo.validate_completion(
                &f.conn,
                &batch,
                &BatchCompletion {
                    version: 1,
                    outcomes: vec![],
                    cycle_marks: vec![],
                }
            ),
            Err(Error::Corrupt)
        ));
    }
}

#[test]
fn ordinary_completion_rejects_cycle_mark_response_without_mark_commits() {
    let (mut f, batch, turn, request) = dispatched_fixture();
    let response = br#"{"version":1,"candidates":[],"cycleMarks":[{"candidateId":"mark_1","ruleId":"rescue","sourceIds":["s1"],"subjectIds":["guard"],"summary":"Guard remembers help"}]}"#;
    let commit = cycle_mark_commit(&f);
    f.repo
        .record_cycle_mark(&mut f.conn, &f.port, &commit.mark)
        .unwrap();
    f.repo
        .receive_batch(&mut f.conn, &batch, &turn, &request, response)
        .unwrap();

    rejected(
        f.repo.complete_batch(&mut f.conn, &batch, EMPTY_COMPLETION),
        Reason::RecoveryRequired,
    );
    let forged_completion = BatchCompletion {
        version: 1,
        outcomes: vec![],
        cycle_marks: vec![CycleMarkResult {
            candidate_id: "mark_1".into(),
            mark_id: commit.mark.mark_id,
        }],
    };
    rejected(
        f.repo.complete_batch(
            &mut f.conn,
            &batch,
            &serde_json::to_vec(&forged_completion).unwrap(),
        ),
        Reason::RecoveryRequired,
    );
    assert_eq!(
        f.repo.batch_receipt(&f.conn, &batch).unwrap().state,
        BatchState::Received,
        "普通入口不能借用已有刻痕伪造批次提交记录"
    );
}

#[test]
fn authorized_cycle_mark_and_batch_completion_commit_atomically_and_replay_idempotently() {
    let (mut f, batch, turn, request) = dispatched_fixture();
    let response = br#"{"version":1,"candidates":[],"cycleMarks":[{"candidateId":"mark_1","ruleId":"rescue","sourceIds":["s1"],"subjectIds":["guard"],"summary":"Guard remembers help"}]}"#;
    f.repo
        .receive_batch(&mut f.conn, &batch, &turn, &request, response)
        .unwrap();
    let commit = cycle_mark_commit(&f);
    let completion = BatchCompletion {
        version: 1,
        outcomes: vec![],
        cycle_marks: vec![],
    };

    assert!(
        f.repo
            .complete_batch_with_cycle_marks(
                &mut f.conn,
                &f.port,
                &batch,
                &completion,
                std::slice::from_ref(&commit),
            )
            .unwrap()
    );
    let receipt = f.repo.batch_receipt(&f.conn, &batch).unwrap();
    assert_eq!(receipt.state, BatchState::Completed);
    let persisted: BatchCompletion =
        aoidos_json::decode_bytes(receipt.completion.as_deref().unwrap(), MAX_PROPOSAL_BYTES)
            .unwrap();
    assert_eq!(
        persisted.cycle_marks,
        vec![CycleMarkResult {
            candidate_id: "mark_1".into(),
            mark_id: commit.mark.mark_id.clone(),
        }]
    );
    assert_eq!(
        f.repo
            .cycle_marks(&f.conn, &f.run.script_id, false)
            .unwrap()
            .len(),
        1
    );
    assert!(
        !f.repo
            .complete_batch_with_cycle_marks(&mut f.conn, &f.port, &batch, &completion, &[commit],)
            .unwrap()
    );
    f.conn
        .execute(
            "UPDATE memory_cycle_marks SET priority=priority+1 WHERE mark_id=?1",
            [persisted.cycle_marks[0].mark_id.as_str()],
        )
        .unwrap();
    assert!(matches!(
        f.repo.batch_receipt(&f.conn, &batch),
        Err(Error::Corrupt)
    ));
}

#[test]
fn unauthorized_cycle_mark_source_rolls_back_batch_completion_and_mark_write() {
    let (mut f, batch, turn, request) = dispatched_fixture();
    let response = br#"{"version":1,"candidates":[],"cycleMarks":[{"candidateId":"mark_1","ruleId":"rescue","sourceIds":["s1"],"subjectIds":["guard"],"summary":"Guard remembers help"}]}"#;
    f.repo
        .receive_batch(&mut f.conn, &batch, &turn, &request, response)
        .unwrap();
    let mut commit = cycle_mark_commit(&f);
    commit.mark.source_refs[0].source.record_seq = 2;
    let completion = BatchCompletion {
        version: 1,
        outcomes: vec![],
        cycle_marks: vec![],
    };

    rejected(
        f.repo.complete_batch_with_cycle_marks(
            &mut f.conn,
            &f.port,
            &batch,
            &completion,
            &[commit],
        ),
        Reason::InvalidSource,
    );
    assert_eq!(
        f.repo.batch_receipt(&f.conn, &batch).unwrap().state,
        BatchState::Received
    );
    assert!(
        f.repo
            .cycle_marks(&f.conn, &f.run.script_id, true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cycle_mark_from_another_generation_cannot_complete_the_batch() {
    let (mut f, batch, turn, request) = dispatched_fixture();
    let response = br#"{"version":1,"candidates":[],"cycleMarks":[{"candidateId":"mark_1","ruleId":"rescue","sourceIds":["s1"],"subjectIds":["guard"],"summary":"Guard remembers help"}]}"#;
    f.repo
        .receive_batch(&mut f.conn, &batch, &turn, &request, response)
        .unwrap();
    let mut commit = cycle_mark_commit(&f);
    commit.mark.cycle_gen += 1;
    commit.mark.mark_id = commit.mark.stable_mark_id().unwrap();
    let completion = BatchCompletion {
        version: 1,
        outcomes: vec![],
        cycle_marks: vec![],
    };

    rejected(
        f.repo.complete_batch_with_cycle_marks(
            &mut f.conn,
            &f.port,
            &batch,
            &completion,
            &[commit],
        ),
        Reason::InvalidSource,
    );
    assert_eq!(
        f.repo.batch_receipt(&f.conn, &batch).unwrap().state,
        BatchState::Received
    );
    assert!(
        f.repo
            .cycle_marks(&f.conn, &f.run.script_id, true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn completion_validation_failure_rolls_back_cycle_marks_already_inserted_in_transaction() {
    let (mut f, batch, turn, request) = dispatched_fixture();
    let response = br#"{"version":1,"candidates":[{"candidateId":"fact_1","classification":"notWorthy","sourceIds":["s1"],"subjectRef":{"kind":"playerPreference"},"reason":"No durable change"}],"cycleMarks":[{"candidateId":"mark_1","ruleId":"rescue","sourceIds":["s1"],"subjectIds":["guard"],"summary":"Guard remembers help"}]}"#;
    f.repo
        .receive_batch(&mut f.conn, &batch, &turn, &request, response)
        .unwrap();
    let commit = cycle_mark_commit(&f);
    let completion = BatchCompletion {
        version: 1,
        outcomes: vec![],
        cycle_marks: vec![],
    };

    rejected(
        f.repo.complete_batch_with_cycle_marks(
            &mut f.conn,
            &f.port,
            &batch,
            &completion,
            &[commit],
        ),
        Reason::InvalidSchema,
    );
    assert_eq!(
        f.repo.batch_receipt(&f.conn, &batch).unwrap().state,
        BatchState::Received
    );
    assert!(
        f.repo
            .cycle_marks(&f.conn, &f.run.script_id, true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn receipt_completion_validation_requires_received_response() {
    let receipt = BatchReceipt {
        state: BatchState::Received,
        turn_id: new_id(),
        request_id: new_id(),
        response: Some(EMPTY_RESPONSE.to_vec()),
        completion: None,
        error_code: None,
    };
    validate_received_receipt(&receipt).unwrap();
    let mut missing_response = receipt.clone();
    missing_response.response = None;
    assert!(matches!(
        validate_received_receipt(&missing_response),
        Err(Error::Corrupt)
    ));
    let mut dispatched = receipt;
    dispatched.state = BatchState::Dispatched;
    assert!(matches!(
        validate_received_receipt(&dispatched),
        Err(Error::Corrupt)
    ));
}

#[test]
fn completion_payload_and_hash_must_be_present_and_valid_together() {
    for (payload, digest) in [
        (EMPTY_COMPLETION.to_vec(), vec![0; 32]),
        (b"bad json".to_vec(), hash(b"bad json").to_vec()),
        (
            br#"{"version":2,"outcomes":[]}"#.to_vec(),
            hash(br#"{"version":2,"outcomes":[]}"#).to_vec(),
        ),
    ] {
        let (mut f, batch, turn, request) = dispatched_fixture();
        f.repo
            .receive_batch(&mut f.conn, &batch, &turn, &request, EMPTY_RESPONSE)
            .unwrap();
        f.conn
            .execute(
                "UPDATE memory_batch_receipts SET completion_json=?1,completion_hash=?2",
                params![payload, digest],
            )
            .unwrap();
        assert!(matches!(
            f.repo.batch_receipt(&f.conn, &batch),
            Err(Error::Corrupt)
        ));
    }
    let (f, batch, _, _) = dispatched_fixture();
    f.conn
        .execute_batch("PRAGMA ignore_check_constraints=ON")
        .unwrap();
    f.conn
        .execute(
            "UPDATE memory_batch_receipts SET completion_json=?1,completion_hash=?2",
            params![EMPTY_COMPLETION, hash(EMPTY_COMPLETION).as_slice()],
        )
        .unwrap();
    assert!(matches!(
        f.repo.batch_receipt(&f.conn, &batch),
        Err(Error::Corrupt)
    ));
}

#[test]
fn claimed_candidate_cannot_complete_with_a_non_applied_outcome() {
    let mut f = Fixture::new();
    let snapshot = f.received_batch("candidate_1");
    let mut plan = f.plan(f.version());
    plan.batch_id = Some(snapshot.batch_id.clone());
    plan.candidate_ids = vec!["candidate_1".into()];
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();

    let deferred = br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"deferred","reasonCode":"policy.capacity_blocked"}]}"#;
    assert!(matches!(
        f.repo
            .complete_batch(&mut f.conn, &snapshot.batch_id, deferred),
        Err(Error::Rejected(Reason::RecoveryRequired))
    ));
    assert_eq!(
        f.repo
            .batch_receipt(&f.conn, &snapshot.batch_id)
            .unwrap()
            .state,
        BatchState::Received
    );
}

#[test]
fn quarantined_candidate_operation_requires_recovery_before_completion() {
    let mut f = Fixture::new();
    let snapshot = f.received_batch("candidate_1");
    let version = f.version();
    let mut plan = f.plan(version.clone());
    plan.batch_id = Some(snapshot.batch_id.clone());
    plan.candidate_ids = vec!["candidate_1".into()];
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();

    let path = f.path(&version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "conflicting body").unwrap();
    assert!(matches!(
        f.repo.write_versions(&mut f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Quarantined
    );

    let applied =
        br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"applied"}]}"#;
    assert!(matches!(
        f.repo
            .complete_batch(&mut f.conn, &snapshot.batch_id, applied),
        Err(Error::Rejected(Reason::RecoveryRequired))
    ));
}

#[test]
fn rejected_candidate_operation_cannot_complete_as_applied() {
    let mut f = Fixture::new();
    let snapshot = f.received_batch("candidate_1");
    let mut plan = f.plan(f.version());
    plan.batch_id = Some(snapshot.batch_id.clone());
    plan.candidate_ids = vec!["candidate_1".into()];
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.port.invalid.set(true);
    assert!(matches!(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Err(Error::Rejected(Reason::InvalidSource))
    ));
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Rejected
    );

    let applied =
        br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"applied"}]}"#;
    assert!(matches!(
        f.repo
            .complete_batch(&mut f.conn, &snapshot.batch_id, applied),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
}

#[test]
fn malformed_candidate_operation_is_skipped_during_completion_refreeze() {
    let mut f = Fixture::new();
    let snapshot = f.received_batch("candidate_1");
    let mut plan = f.plan(f.version());
    plan.batch_id = Some(snapshot.batch_id.clone());
    plan.candidate_ids = vec!["candidate_1".into()];
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();

    f.conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    f.conn
        .execute(
            "UPDATE memory_batch_candidate_operations SET operation_id=?1 WHERE batch_id=?2 AND candidate_id=?3",
            params!["x".repeat(36), snapshot.batch_id, "candidate_1"],
        )
        .unwrap();
    let applied =
        br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"applied"}]}"#;
    assert!(matches!(
        f.repo
            .complete_batch(&mut f.conn, &snapshot.batch_id, applied),
        Err(Error::Corrupt)
    ));
    assert_eq!(
        f.repo
            .batch_receipt(&f.conn, &snapshot.batch_id)
            .unwrap()
            .state,
        BatchState::Received
    );
    let faults: usize = f
        .conn
        .query_row(
            "SELECT count(*) FROM memory_run_faults WHERE run_id=?1",
            [&f.run.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(faults, 0);
}

#[test]
fn rejected_operation_can_only_finish_its_candidate_as_rejected() {
    let mut f = Fixture::new();
    let snapshot = f.received_batch("candidate_1");
    let mut plan = f.plan(f.version());
    plan.batch_id = Some(snapshot.batch_id.clone());
    plan.candidate_ids = vec!["candidate_1".into()];
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.port.invalid.set(true);
    rejected(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Reason::InvalidSource,
    );

    let rejected_result = br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"rejected","reasonCode":"source.invalid"}]}"#;
    assert!(
        f.repo
            .complete_batch(&mut f.conn, &snapshot.batch_id, rejected_result)
            .unwrap()
    );
    assert_eq!(
        f.repo
            .batch_receipt(&f.conn, &snapshot.batch_id)
            .unwrap()
            .state,
        BatchState::Completed
    );
}

#[test]
fn corrupt_applied_receipt_freezes_run_after_completion_transaction_rolls_back() {
    let applied =
        br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"applied"}]}"#;
    for replay in [false, true] {
        let mut f = Fixture::new();
        let snapshot = f.received_batch("candidate_1");
        let mut plan = f.plan(f.version());
        plan.batch_id = Some(snapshot.batch_id.clone());
        plan.candidate_ids = vec!["candidate_1".into()];
        f.commit(&plan);
        if replay {
            assert!(
                f.repo
                    .complete_batch(&mut f.conn, &snapshot.batch_id, applied)
                    .unwrap()
            );
        }
        f.conn
            .execute(
                "UPDATE store_applied SET content_hash='tampered' WHERE id=?1",
                [format!("memory:{}", plan.operation_id)],
            )
            .unwrap();

        assert!(matches!(
            f.repo
                .complete_batch(&mut f.conn, &snapshot.batch_id, applied),
            Err(Error::Corrupt)
        ));
        let frozen: bool = f
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_run_faults WHERE run_id=?1)",
                [&f.run.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(frozen);
        let subsequent = f.plan(f.version());
        rejected(
            f.repo.prepare(&mut f.conn, &f.port, &subsequent),
            Reason::RecoveryRequired,
        );
    }
}

#[test]
fn upgrade_preserves_completed_legacy_applied_results_without_inventing_links() {
    let mut f = Fixture::new();
    let snapshot = f.received_batch("candidate_1");
    let no_op = br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"noOp"}]}"#;
    f.repo
        .complete_batch(&mut f.conn, &snapshot.batch_id, no_op)
        .unwrap();

    let legacy_applied =
        br#"{"version":1,"outcomes":[{"candidateId":"candidate_1","outcome":"applied"}]}"#;
    f.conn
        .execute(
            "UPDATE memory_batch_receipts SET completion_json=?1,completion_hash=?2 WHERE batch_id=?3",
            params![legacy_applied, hash(legacy_applied).as_slice(), snapshot.batch_id],
        )
        .unwrap();
    f.conn
        .execute_batch("DROP TABLE memory_batch_legacy_completions")
        .unwrap();
    f.conn
        .execute_batch(crate::schema::LEGACY_BATCH_COMPLETION_MIGRATION)
        .unwrap();
    f.conn
        .execute_batch(crate::schema::LEGACY_BATCH_COMPLETION_BACKFILL)
        .unwrap();
    let receipt = f.repo.batch_receipt(&f.conn, &snapshot.batch_id).unwrap();
    assert_eq!(receipt.state, BatchState::Completed);
    assert_eq!(
        receipt.completion.as_deref(),
        Some(legacy_applied.as_slice())
    );
    assert_eq!(
        f.conn
            .query_row(
                "SELECT count(*) FROM memory_batch_candidate_operations WHERE batch_id=?1",
                [&snapshot.batch_id],
                |row| row.get::<_, usize>(0),
            )
            .unwrap(),
        0
    );
}
