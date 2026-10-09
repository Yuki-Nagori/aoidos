//! 稳定素材与冻结批次的身份、来源和预算契约。

use super::super::test_support::*;
use super::*;

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
