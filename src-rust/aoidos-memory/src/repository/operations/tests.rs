//! 操作提交契约。

use super::super::test_support::*;
use super::*;

#[test]
fn commit_is_idempotent_and_detects_receipt_or_plan_conflicts() {
    let mut f = Fixture::new();
    let version = f.version();
    let mut plan = f.plan(version.clone());
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    plan.processed_materials.push(material);
    assert_eq!(
        f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap(),
        OperationState::Prepared
    );
    assert!(matches!(
        f.repo.entry(&f.conn, &f.run.run_id, &version.entry_id),
        Err(Error::NotFound)
    ));
    assert_eq!(
        f.repo
            .apply(&mut f.conn, &f.port, &plan.operation_id)
            .unwrap(),
        OperationState::Prepared
    );
    f.commit(&plan);
    assert_eq!(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &version.entry_id)
            .unwrap(),
        version
    );
    assert_eq!(
        f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap(),
        OperationState::Applied
    );
    assert_eq!(
        f.repo
            .write_versions(&mut f.conn, &plan.operation_id)
            .unwrap(),
        OperationState::Applied
    );
    assert_eq!(
        f.repo
            .apply(&mut f.conn, &f.port, &plan.operation_id)
            .unwrap(),
        OperationState::Applied
    );
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &version.entry_id)
            .unwrap()
            .state_revision,
        1
    );
    let mut conflict = plan.clone();
    conflict.reason = "changed".into();
    assert!(matches!(
        f.repo.prepare(&mut f.conn, &f.port, &conflict),
        Err(Error::Corrupt)
    ));
    f.conn.execute("DELETE FROM store_applied", []).unwrap();
    assert!(matches!(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn oversized_persisted_plan_is_rejected_before_blob_copy() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.conn
        .execute(
            "UPDATE memory_operations SET plan_json=zeroblob(?1) WHERE operation_id=?2",
            params![MAX_PLAN_BYTES + 1, plan.operation_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.operation(&f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn applied_audit_corruption_persistently_freezes_run_mutations() {
    let mut f = Fixture::new();
    let committed = f.version();
    let plan = f.plan(committed.clone());
    f.commit(&plan);
    f.conn
        .execute(
            "DELETE FROM store_applied WHERE id=?1",
            [format!("memory:{}", plan.operation_id)],
        )
        .unwrap();
    assert!(matches!(
        f.repo.operation(&f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
    assert_eq!(
        f.conn
            .query_row(
                "SELECT operation_id FROM memory_run_faults WHERE run_id=?1",
                [&f.run.run_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        plan.operation_id
    );
    let next = f.plan(f.version());
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &next),
        Reason::RecoveryRequired,
    );
    rejected(
        f.repo.register_material(&f.conn, &f.port, &f.source(), 0),
        Reason::RecoveryRequired,
    );
    assert_eq!(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &committed.entry_id)
            .unwrap(),
        committed
    );
}

#[test]
fn applied_plan_digest_or_targets_corruption_persistently_freezes_run() {
    for mutation in [
        "UPDATE memory_operations SET plan_hash=zeroblob(32)",
        "DELETE FROM memory_operation_targets",
    ] {
        let mut f = Fixture::new();
        let version = f.version();
        let plan = f.plan(version);
        f.commit(&plan);
        f.conn.execute_batch(mutation).unwrap();

        assert!(matches!(
            f.repo.operation(&f.conn, &plan.operation_id),
            Err(Error::Corrupt)
        ));
        assert_eq!(
            f.conn
                .query_row(
                    "SELECT operation_id FROM memory_run_faults WHERE run_id=?1",
                    [&f.run.run_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            plan.operation_id
        );
        let next = f.plan(f.version());
        rejected(
            f.repo.prepare(&mut f.conn, &f.port, &next),
            Reason::RecoveryRequired,
        );
    }
}

#[test]
fn character_effects_require_subject_bound_knowledge_evidence_support() {
    let mut f = Fixture::new();
    let source = f.source();
    let mut version = f.version();
    version.subject = Subject::Character { id: "npc-1".into() };
    version.dimension = Dimension::Story;
    version.evidence = vec![KnowledgeEvidence {
        evidence_id: new_id(),
        subject_id: "npc-1".into(),
        source: source.clone(),
        kind: KnowledgeKind::Observed,
        informant_id: None,
        scope: "scene".into(),
        rule_id: "observed".into(),
    }];
    version.evidence_set_hash = set_hash(&version.evidence).unwrap();
    let mut plan = f.plan(version);
    plan.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source,
        logical_position: 0,
        gain: 10,
    });

    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Reason::PolicyUnavailable,
    );
}

#[test]
fn dependency_group_is_atomic_when_one_target_becomes_stale() {
    let mut f = Fixture::new();
    let a = f.version();
    let b = f.version();
    let mut group = f.plan(a.clone());
    group.targets.extend(f.plan(b.clone()).targets);
    f.repo.prepare(&mut f.conn, &f.port, &group).unwrap();
    f.repo
        .write_versions(&mut f.conn, &group.operation_id)
        .unwrap();
    let mut independent_version = a.clone();
    independent_version.version_id = new_id();
    let independent = f.plan(independent_version.clone());
    f.commit(&independent);
    rejected(
        f.repo.apply(&mut f.conn, &f.port, &group.operation_id),
        Reason::StaleVersion,
    );
    assert!(matches!(
        f.repo.entry(&f.conn, &f.run.run_id, &b.entry_id),
        Err(Error::NotFound)
    ));
    assert_eq!(
        f.repo.operation(&f.conn, &group.operation_id).unwrap().1,
        OperationState::Rejected
    );
    assert_eq!(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &a.entry_id)
            .unwrap(),
        independent_version
    );
}

#[test]
fn failed_database_transaction_publishes_no_partial_entries() {
    let mut f = Fixture::new();
    let a = f.version();
    let b = f.version();
    let mut group = f.plan(a.clone());
    group.targets.extend(f.plan(b.clone()).targets);
    f.repo.prepare(&mut f.conn, &f.port, &group).unwrap();
    f.repo
        .write_versions(&mut f.conn, &group.operation_id)
        .unwrap();
    f.conn.execute_batch("CREATE TRIGGER deny_apply BEFORE INSERT ON store_applied BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
    assert!(matches!(
        f.repo.apply(&mut f.conn, &f.port, &group.operation_id),
        Err(Error::Store(_))
    ));
    for v in [&a, &b] {
        assert!(matches!(
            f.repo.entry(&f.conn, &f.run.run_id, &v.entry_id),
            Err(Error::NotFound)
        ));
    }
    assert_eq!(
        f.repo.operation(&f.conn, &group.operation_id).unwrap().1,
        OperationState::Ready
    );
    f.conn.execute_batch("DROP TRIGGER deny_apply;").unwrap();
    f.repo
        .apply(&mut f.conn, &f.port, &group.operation_id)
        .unwrap();
    assert_eq!(
        f.repo
            .entries(&f.conn, &f.port, &f.run.run_id, 0, None, 32)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn persisted_metadata_and_applied_targets_cannot_override_verified_bytes() {
    for mutation in [
        "UPDATE memory_versions SET body_length=1",
        "UPDATE memory_versions SET subject_json=x'7b7d'",
        "UPDATE memory_versions SET quarantined=1",
        "DELETE FROM memory_version_sources",
    ] {
        let mut f = Fixture::new();
        let v = f.version();
        f.commit(&f.plan(v.clone()));
        f.conn.execute_batch(mutation).unwrap();
        assert!(matches!(
            f.repo.current(&f.conn, &f.port, &f.run.run_id, &v.entry_id),
            Err(Error::Corrupt)
        ));
    }
    let mut f = Fixture::new();
    let v = f.version();
    let plan = f.plan(v);
    f.commit(&plan);
    f.conn
        .execute(
            "UPDATE memory_operation_targets SET next_state_revision=2",
            [],
        )
        .unwrap();
    assert!(matches!(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Err(Error::Corrupt)
    ));
}

#[test]
fn corrupt_operation_digest_or_target_count_blocks_recovery() {
    for mutation in [
        "UPDATE memory_operations SET plan_hash=zeroblob(32)",
        "DELETE FROM memory_operation_targets",
    ] {
        let mut f = Fixture::new();
        let plan = f.plan(f.version());
        f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
        f.conn.execute_batch(mutation).unwrap();
        assert!(matches!(
            f.repo.operation(&f.conn, &plan.operation_id),
            Err(Error::Corrupt)
        ));
    }
    let f = Fixture::new();
    rejected(f.repo.operation(&f.conn, "bad"), Reason::InvalidSchema);
    assert!(matches!(
        f.repo.operation(&f.conn, &new_id()),
        Err(Error::NotFound)
    ));
}

#[test]
fn preparation_rejects_stale_boundary_invalid_sources_and_duplicate_targets() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    let mut bad = plan.clone();
    bad.history_revision += 1;
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &bad),
        Reason::StaleVersion,
    );
    bad = plan.clone();
    bad.targets.push(bad.targets[0].clone());
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &bad),
        Reason::InvalidSchema,
    );
    bad = plan.clone();
    bad.targets.clear();
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &bad),
        Reason::InvalidSchema,
    );
    bad = plan.clone();
    bad.targets[0].expected.state_revision = 1;
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &bad),
        Reason::InvalidSchema,
    );
    f.port.invalid.set(true);
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Reason::InvalidSource,
    );
    f.port.invalid.set(false);
    f.port.view.session_id = new_id();
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Reason::InvalidSource,
    );
}

#[test]
fn processed_material_cannot_be_consumed_by_another_operation() {
    let mut f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let mut plan = f.plan(f.version());
    plan.processed_materials.push(material.clone());
    f.commit(&plan);
    let mut next = f.plan(f.version());
    next.processed_materials.push(material);
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &next),
        Reason::StaleVersion,
    );
    f.conn.execute("DELETE FROM memory_processed", []).unwrap();
    assert!(matches!(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn preparation_rejects_oversized_or_internally_inconsistent_plans() {
    let mut f = Fixture::new();
    let baseline = f.plan(f.version());

    let mut too_many_targets = baseline.clone();
    too_many_targets.targets = (0..17).map(|_| baseline.targets[0].clone()).collect();
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &too_many_targets),
        Reason::InvalidSchema,
    );

    let mut too_many_materials = baseline.clone();
    too_many_materials.processed_materials = (0..129).map(|_| new_id()).collect();
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &too_many_materials),
        Reason::InvalidSchema,
    );

    let mut long_reason = baseline.clone();
    long_reason.reason = "x".repeat(513);
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &long_reason),
        Reason::InvalidSchema,
    );

    let mut too_many_effects = baseline.clone();
    too_many_effects.targets[0].effects = (0..129)
        .map(|_| Effect {
            effect_id: new_id(),
            kind: EffectKind::Gain,
            source: f.source(),
            logical_position: 0,
            gain: 1,
        })
        .collect();
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &too_many_effects),
        Reason::InvalidSchema,
    );

    let mut mismatched_entry = baseline.clone();
    mismatched_entry.targets[0].next.as_mut().unwrap().entry_id = new_id();
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &mismatched_entry),
        Reason::InvalidSchema,
    );

    let mut invalid_effect = baseline;
    invalid_effect.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Revival,
        source: f.source(),
        logical_position: 0,
        gain: 1,
    });
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &invalid_effect),
        Reason::InvalidSchema,
    );
}

#[test]
fn preparation_rejects_duplicate_materials_and_effect_identities() {
    let mut f = Fixture::new();
    let mut plan = f.plan(f.version());
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    plan.processed_materials = vec![material.clone(), material];
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Reason::InvalidSchema,
    );

    plan = f.plan(f.version());
    let effect = Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: f.source(),
        logical_position: 0,
        gain: 1,
    };
    plan.targets[0].effects = vec![effect.clone(), effect];
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Reason::InvalidSchema,
    );
}

#[test]
fn plan_rechecks_existing_entry_source_state_and_effect_uniqueness() {
    let mut f = Fixture::new();
    let original = f.version();
    let mut initial = f.plan(original.clone());
    initial.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: f.source(),
        logical_position: 0,
        gain: 10,
    });
    f.commit(&initial);

    let mut supplement = original.clone();
    supplement.version_id = new_id();
    supplement.parent_version_id = Some(original.version_id.clone());
    supplement.change = ChangeKind::Supplement;
    supplement.summary = "补充理解".into();
    let plan = f.plan(supplement.clone());

    f.conn
        .execute(
            "UPDATE memory_entries SET effect_history_revision=NULL WHERE entry_id=?1",
            [&original.entry_id],
        )
        .unwrap();
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Reason::RecoveryRequired,
    );
    f.conn
        .execute(
            "UPDATE memory_entries SET effect_history_revision=0 WHERE entry_id=?1",
            [&original.entry_id],
        )
        .unwrap();

    let mut wrong_subject = supplement.clone();
    wrong_subject.subject = Subject::Character { id: "npc-1".into() };
    wrong_subject.dimension = Dimension::Story;
    wrong_subject.sources = vec![f.source()];
    wrong_subject.evidence = vec![KnowledgeEvidence {
        evidence_id: new_id(),
        subject_id: "npc-1".into(),
        source: f.source(),
        kind: KnowledgeKind::Observed,
        informant_id: None,
        scope: "scene".into(),
        rule_id: "observed".into(),
    }];
    wrong_subject.source_set_hash = set_hash(&wrong_subject.sources).unwrap();
    wrong_subject.evidence_set_hash = set_hash(&wrong_subject.evidence).unwrap();
    let wrong_subject_plan = f.plan(wrong_subject);
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &wrong_subject_plan),
        Reason::InvalidSchema,
    );

    f.port.invalid.set(true);
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Reason::InvalidSource,
    );
    f.port.invalid.set(false);

    let mut repeated_effect = plan.clone();
    repeated_effect.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: f.source(),
        logical_position: 0,
        gain: 10,
    });
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &repeated_effect),
        Reason::InvalidSource,
    );
}

#[test]
fn applied_audit_rejects_each_inconsistent_receipt_component() {
    let mutations = [
        "UPDATE memory_effects SET operation_id=(SELECT operation_id FROM memory_operations WHERE operation_id<>memory_effects.operation_id LIMIT 1)",
        "UPDATE memory_effects SET gain=99",
        "UPDATE memory_versions SET body_hash=zeroblob(32)",
        "UPDATE memory_entries SET state_revision=0",
        "UPDATE memory_materials SET status='eligible'",
    ];
    for mutation in mutations {
        let mut f = Fixture::new();
        let unrelated = f.plan(f.version());
        f.commit(&unrelated);
        let version = f.version();
        let material = f
            .repo
            .register_material(&f.conn, &f.port, &f.source(), 0)
            .unwrap();
        let mut plan = f.plan(version);
        plan.targets[0].effects.push(Effect {
            effect_id: new_id(),
            kind: EffectKind::Gain,
            source: f.source(),
            logical_position: 0,
            gain: 5,
        });
        plan.processed_materials.push(material);
        f.commit(&plan);
        f.conn.execute_batch(mutation).unwrap();
        assert!(
            matches!(
                f.repo.operation(&f.conn, &plan.operation_id),
                Err(Error::Corrupt)
            ),
            "mutation should invalidate applied audit: {mutation}"
        );
    }
}

#[test]
fn applied_effect_deduplication_indexes_must_match_the_committed_source() {
    for mutation in [
        "UPDATE memory_effects SET record_seq=record_seq+1",
        "UPDATE memory_effects SET session_id='different-session'",
        "UPDATE memory_effects SET run_id=(SELECT run_id FROM memory_runs WHERE run_id<>memory_effects.run_id LIMIT 1)",
    ] {
        let mut f = Fixture::new();
        let original_session = f.port.view.session_id.clone();
        f.port.view.session_id = new_id();
        f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap();
        f.port.view.session_id = original_session;
        let mut plan = f.plan(f.version());
        plan.targets[0].effects.push(Effect {
            effect_id: new_id(),
            kind: EffectKind::Gain,
            source: f.source(),
            logical_position: 0,
            gain: 5,
        });
        f.commit(&plan);
        f.conn.execute_batch(mutation).unwrap();
        assert!(
            matches!(
                f.repo.operation(&f.conn, &plan.operation_id),
                Err(Error::Corrupt)
            ),
            "index corruption must freeze the run: {mutation}"
        );
        let mut duplicate = plan.clone();
        duplicate.operation_id = new_id();
        duplicate.targets[0].expected = Expected {
            version_id: Some(plan.targets[0].next.as_ref().unwrap().version_id.clone()),
            state_revision: 1,
        };
        duplicate.targets[0].next = None;
        duplicate.targets[0].effects[0].effect_id = new_id();
        rejected(
            f.repo.prepare(&mut f.conn, &f.port, &duplicate),
            Reason::RecoveryRequired,
        );
    }
}

#[test]
fn applied_audit_rejects_extra_effects_or_processed_materials() {
    for extra_effect in [true, false] {
        let mut f = Fixture::new();
        let mut plan = f.plan(f.version());
        plan.targets[0].effects.push(Effect {
            effect_id: new_id(),
            kind: EffectKind::Gain,
            source: f.source(),
            logical_position: 0,
            gain: 5,
        });
        f.commit(&plan);
        if extra_effect {
            f.conn.execute(
                "INSERT INTO memory_effects SELECT ?1,operation_id,entry_id,run_id,session_id,record_seq+1,kind,logical_position,gain,source_json,valid FROM memory_effects",
                [new_id()],
            ).unwrap();
        } else {
            let material = f
                .repo
                .register_material(&f.conn, &f.port, &f.source(), 0)
                .unwrap();
            f.conn
                .execute(
                    "INSERT INTO memory_processed VALUES(?1,?2)",
                    params![material, plan.operation_id],
                )
                .unwrap();
            f.conn
                .execute("UPDATE memory_materials SET status='processed'", [])
                .unwrap();
        }
        assert!(matches!(
            f.repo.operation(&f.conn, &plan.operation_id),
            Err(Error::Corrupt)
        ));
        let next = f.plan(f.version());
        rejected(
            f.repo.prepare(&mut f.conn, &f.port, &next),
            Reason::RecoveryRequired,
        );
    }
}

#[test]
fn missing_applied_entry_is_corruption_and_freezes_further_writes() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.commit(&plan);
    f.conn
        .execute("UPDATE memory_entries SET current_version_id=NULL", [])
        .unwrap();
    assert!(matches!(
        f.repo.operation(&f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
    let next = f.plan(f.version());
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &next),
        Reason::RecoveryRequired,
    );
    assert!(matches!(
        f.repo.operation(&f.conn, &new_id()),
        Err(Error::NotFound)
    ));
}

#[test]
fn ready_operation_is_rejected_when_its_causal_boundary_changes() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
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
}

#[test]
fn preparation_propagates_version_metadata_and_database_write_failures() {
    let mut f = Fixture::new();
    let mut bad_version = f.version();
    bad_version.sources[0].end_byte = 5;
    let bad = f.plan(bad_version);
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &bad),
        Reason::InvalidSchema,
    );

    f.conn.execute_batch(
        "CREATE TRIGGER reject_memory_version BEFORE INSERT ON memory_versions BEGIN SELECT RAISE(ABORT, 'test'); END;",
    ).unwrap();
    let valid = f.plan(f.version());
    assert!(matches!(
        f.repo.prepare(&mut f.conn, &f.port, &valid),
        Err(Error::Store(_))
    ));
    assert!(matches!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &valid.targets[0].entry_id),
        Err(Error::NotFound)
    ));
}

#[test]
fn plan_rejects_unpermitted_or_changed_ancestor_restores() {
    let mut f = Fixture::new();
    let original = f.version();
    f.commit(&f.plan(original.clone()));
    let mut supplement = original.clone();
    supplement.version_id = new_id();
    supplement.parent_version_id = Some(original.version_id.clone());
    supplement.change = ChangeKind::Supplement;
    supplement.summary = "补充理解".into();
    let mut supplement_plan = f.plan(supplement.clone());
    supplement_plan.targets[0].expected.state_revision = 1;
    f.commit(&supplement_plan);

    let restored_version = |summary: &str| {
        let mut restored = original.clone();
        restored.version_id = new_id();
        restored.parent_version_id = Some(supplement.version_id.clone());
        restored.restored_from_version_id = Some(original.version_id.clone());
        restored.change = ChangeKind::Restore;
        restored.summary = summary.into();
        restored
    };

    let mut unpermitted = f.plan(restored_version(&original.summary));
    unpermitted.targets[0].expected.state_revision = 2;
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &unpermitted),
        Reason::InvalidSource,
    );

    f.conn
        .execute(
            "UPDATE memory_entries SET status='unavailable', recovery_cursor=?1, recovery_history_revision=0 WHERE entry_id=?2",
            rusqlite::params![original.version_id, original.entry_id],
        )
        .unwrap();
    let mut changed = f.plan(restored_version("改写后的恢复内容"));
    changed.targets[0].expected.state_revision = 2;
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &changed),
        Reason::InvalidSource,
    );
    let mut valid_restore = f.plan(restored_version(&original.summary));
    valid_restore.targets[0].expected.state_revision = 2;
    assert_eq!(
        f.repo
            .prepare(&mut f.conn, &f.port, &valid_restore)
            .unwrap(),
        OperationState::Prepared
    );
}

#[test]
fn state_only_and_material_targets_revalidate_sources_before_prepare() {
    let mut f = Fixture::new();
    let original = f.version();
    f.commit(&f.plan(original.clone()));

    let mut archive = f.plan(original.clone());
    archive.targets[0].expected.version_id = Some(original.version_id.clone());
    archive.targets[0].expected.state_revision = 1;
    archive.targets[0].next = None;
    archive.targets[0].state = EntryState::Archived;
    let mut unavailable_effect_source = f.source();
    unavailable_effect_source.record_seq = 2;
    archive.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: unavailable_effect_source,
        logical_position: 0,
        gain: 1,
    });
    f.port.invalid.set(true);
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &archive),
        Reason::InvalidSource,
    );
    f.port.invalid.set(false);
    f.port.through_record.set(1);
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &archive),
        Reason::InvalidSource,
    );
    archive.targets[0].effects[0].source = f.source();
    f.repo.prepare(&mut f.conn, &f.port, &archive).unwrap();
    f.repo
        .write_versions(&mut f.conn, &archive.operation_id)
        .unwrap();
    f.repo
        .apply(&mut f.conn, &f.port, &archive.operation_id)
        .unwrap();

    let mut material_plan = f.plan(f.version());
    let mut later_source = f.source();
    later_source.record_seq = 2;
    f.port.through_record.set(MAX_INTEGER);
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &later_source, 0)
        .unwrap();
    material_plan.processed_materials.push(material);
    f.port.through_record.set(1);
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &material_plan),
        Reason::InvalidSource,
    );
}

#[test]
fn writing_an_identical_preexisting_body_is_idempotent() {
    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    let path = f.path(&version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, canonical(&version).unwrap()).unwrap();
    assert_eq!(
        f.repo
            .write_versions(&mut f.conn, &plan.operation_id)
            .unwrap(),
        OperationState::Ready
    );
    assert_eq!(std::fs::read(path).unwrap(), canonical(&version).unwrap());
}

#[test]
fn apply_detects_a_body_that_changed_after_preparation() {
    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.conn
        .execute(
            "UPDATE memory_versions SET body_hash=zeroblob(32) WHERE version_id=?1",
            [&version.version_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn uncertainty_is_preserved_when_recovery_markers_cannot_be_written() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.conn.execute_batch(
        "CREATE TRIGGER fail_rejection_marker BEFORE UPDATE OF state ON memory_operations WHEN NEW.state='rejected' BEGIN SELECT RAISE(ABORT, 'test'); END;",
    ).unwrap();
    f.port.invalid.set(true);
    assert!(matches!(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Err(Error::Store(_))
    ));
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Ready
    );
}

#[test]
fn writing_conflicts_and_postwrite_corruption_are_not_silently_accepted() {
    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    let path = f.path(&version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "different bytes").unwrap();
    f.conn.execute_batch(
        "CREATE TRIGGER fail_quarantine_marker BEFORE UPDATE OF state ON memory_operations WHEN NEW.state='quarantined' BEGIN SELECT RAISE(ABORT, 'test'); END;",
    ).unwrap();
    assert!(matches!(
        f.repo.write_versions(&mut f.conn, &plan.operation_id),
        Err(Error::Store(_))
    ));

    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.conn
        .execute(
            "UPDATE memory_versions SET body_hash=zeroblob(32) WHERE version_id=?1",
            [&version.version_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.write_versions(&mut f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn apply_rolls_back_when_a_target_compare_and_swap_changes() {
    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.conn.execute_batch(
        "CREATE TRIGGER skip_memory_target_update BEFORE UPDATE OF current_version_id ON memory_entries BEGIN SELECT RAISE(IGNORE); END;",
    ).unwrap();
    rejected(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Reason::StaleVersion,
    );
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Ready
    );
    assert!(matches!(
        f.repo.entry(&f.conn, &f.run.run_id, &version.entry_id),
        Err(Error::NotFound)
    ));
}

#[test]
fn operation_payload_identity_and_applied_schema_fail_closed() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    let mut different_identity = plan.clone();
    different_identity.operation_id = new_id();
    f.conn
        .execute(
            "UPDATE memory_operations SET plan_json=?1 WHERE operation_id=?2",
            rusqlite::params![canonical(&different_identity).unwrap(), plan.operation_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.operation(&f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));

    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.commit(&plan);
    f.conn.execute_batch("DROP TABLE store_applied").unwrap();
    assert!(matches!(
        f.repo.operation(&f.conn, &plan.operation_id),
        Err(Error::Store(_))
    ));
}

#[test]
fn storage_failure_during_plan_recheck_keeps_operation_ready() {
    let mut f = Fixture::new();
    let mut plan = f.plan(f.version());
    plan.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: f.source(),
        logical_position: 0,
        gain: 1,
    });
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.conn.execute_batch("DROP TABLE memory_effects").unwrap();
    assert!(matches!(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Err(Error::Store(_))
    ));
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Ready
    );
}

#[test]
fn conflicting_store_receipt_freezes_run_with_original_error_class() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.commit(&plan);
    f.conn
        .execute(
            "UPDATE store_applied SET content_hash='different-content'",
            [],
        )
        .unwrap();
    let error = f.repo.operation(&f.conn, &plan.operation_id).unwrap_err();
    assert_eq!(error.code(), "store.corrupt");
    let quarantined: bool = f
        .conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_run_faults WHERE run_id=?1)",
            [&f.run.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(quarantined);
}

#[test]
fn applied_receipt_with_non_applied_operation_state_freezes_future_writes() {
    for state in ["prepared", "ready", "rejected", "quarantined"] {
        let mut f = Fixture::new();
        let plan = f.plan(f.version());
        f.commit(&plan);
        f.conn
            .execute(
                "UPDATE memory_operations SET state=?1 WHERE operation_id=?2",
                params![state, plan.operation_id],
            )
            .unwrap();
        assert!(matches!(
            f.repo.operation(&f.conn, &plan.operation_id),
            Err(Error::Corrupt)
        ));
        let fault: String = f
            .conn
            .query_row(
                "SELECT operation_id FROM memory_run_faults WHERE run_id=?1",
                [&f.run.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fault, plan.operation_id);
        let next = f.plan(f.version());
        rejected(
            f.repo.prepare(&mut f.conn, &f.port, &next),
            Reason::RecoveryRequired,
        );
    }
}

#[test]
fn self_consistent_hashes_do_not_legalize_invalid_applied_plan_structure() {
    let mut f = Fixture::new();
    let mut plan = f.plan(f.version());
    f.commit(&plan);
    plan.reason = "x".repeat(513);
    let raw = canonical(&plan).unwrap();
    let digest = hash(&raw);
    f.conn
        .execute(
            "UPDATE memory_operations SET plan_json=?1,plan_hash=?2 WHERE operation_id=?3",
            params![raw, digest.as_slice(), plan.operation_id],
        )
        .unwrap();
    f.conn
        .execute(
            "UPDATE store_applied SET content_hash=?1 WHERE id=?2",
            params![hex(&digest), format!("memory:{}", plan.operation_id)],
        )
        .unwrap();
    assert!(matches!(
        f.repo.operation(&f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
    let fault: String = f
        .conn
        .query_row(
            "SELECT operation_id FROM memory_run_faults WHERE run_id=?1",
            [&f.run.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fault, plan.operation_id);
    let next = f.plan(f.version());
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &next),
        Reason::RecoveryRequired,
    );
}

#[test]
fn operation_run_column_cannot_reassign_an_applied_plan_to_another_run() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.commit(&plan);
    let original_boundary = f.port.view.clone();
    f.port.view.session_id = new_id();
    let other = f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap();
    f.conn
        .execute(
            "UPDATE memory_operations SET run_id=?1 WHERE operation_id=?2",
            params![other.run_id, plan.operation_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.operation(&f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
    for run_id in [&f.run.run_id, &other.run_id] {
        let fault: String = f
            .conn
            .query_row(
                "SELECT operation_id FROM memory_run_faults WHERE run_id=?1",
                [run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fault, plan.operation_id);
    }
    let mut other_source = f.source();
    other_source.run_id = other.run_id;
    other_source.session_id = other.session_id;
    rejected(
        f.repo.register_material(&f.conn, &f.port, &other_source, 0),
        Reason::RecoveryRequired,
    );
    f.port.view = original_boundary;
    let next = f.plan(f.version());
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &next),
        Reason::RecoveryRequired,
    );
}

#[test]
fn missing_operation_run_row_does_not_prevent_freezing_the_real_plan_run() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.commit(&plan);
    f.conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    f.conn
        .execute(
            "UPDATE memory_operations SET run_id=?1 WHERE operation_id=?2",
            params![new_id(), plan.operation_id],
        )
        .unwrap();
    f.conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    assert!(matches!(
        f.repo.operation(&f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
    let fault: String = f
        .conn
        .query_row(
            "SELECT operation_id FROM memory_run_faults WHERE run_id=?1",
            [&f.run.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fault, plan.operation_id);
    let next = f.plan(f.version());
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &next),
        Reason::RecoveryRequired,
    );
}

#[test]
fn invalid_or_absent_plan_run_identity_freezes_the_registered_operation_run() {
    for identity in ["../untrusted".into(), new_id()] {
        let mut f = Fixture::new();
        let mut plan = f.plan(f.version());
        f.commit(&plan);
        plan.run_id = identity;
        let raw = canonical(&plan).unwrap();
        let digest = hash(&raw);
        f.conn
            .execute(
                "UPDATE memory_operations SET plan_json=?1,plan_hash=?2 WHERE operation_id=?3",
                params![raw, digest.as_slice(), plan.operation_id],
            )
            .unwrap();
        f.conn
            .execute(
                "UPDATE store_applied SET content_hash=?1 WHERE id=?2",
                params![hex(&digest), format!("memory:{}", plan.operation_id)],
            )
            .unwrap();
        assert!(matches!(
            f.repo.operation(&f.conn, &plan.operation_id),
            Err(Error::Corrupt)
        ));
        let fault: String = f
            .conn
            .query_row(
                "SELECT operation_id FROM memory_run_faults WHERE run_id=?1",
                [&f.run.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fault, plan.operation_id);
        let next = f.plan(f.version());
        rejected(
            f.repo.prepare(&mut f.conn, &f.port, &next),
            Reason::RecoveryRequired,
        );
    }
}

#[test]
fn state_only_effect_requires_decodable_persisted_subject_metadata() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    f.conn
        .execute(
            "UPDATE memory_versions SET subject_json=x'ff' WHERE version_id=?1",
            [&version.version_id],
        )
        .unwrap();
    let mut plan = f.plan(version.clone());
    plan.targets[0].next = None;
    plan.targets[0].expected = Expected {
        version_id: Some(version.version_id),
        state_revision: 1,
    };
    plan.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: f.source(),
        logical_position: 0,
        gain: 1,
    });
    assert!(matches!(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Err(Error::Corrupt)
    ));
}

#[test]
fn orphaned_prepared_manifest_is_corrupt_even_when_row_and_plan_agree() {
    let mut f = Fixture::new();
    let mut plan = f.plan(f.version());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    plan.run_id = new_id();
    let raw = canonical(&plan).unwrap();
    let digest = hash(&raw);
    f.conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    f.conn.execute("UPDATE memory_operations SET run_id=?1,plan_json=?2,plan_hash=?3 WHERE operation_id=?4", params![plan.run_id, raw, digest.as_slice(), plan.operation_id]).unwrap();
    f.conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    assert!(matches!(
        f.repo.operation(&f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
    let count: u64 = f
        .conn
        .query_row("SELECT count(*) FROM memory_run_faults", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn processed_membership_swap_freezes_run_even_when_operation_counts_match() {
    let mut f = Fixture::new();
    let mut first = f.plan(f.version());
    let first_material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    first.processed_materials.push(first_material.clone());
    f.commit(&first);

    let mut second = f.plan(f.version());
    let mut other_source = f.source();
    other_source.record_seq = 2;
    let second_material = f
        .repo
        .register_material(&f.conn, &f.port, &other_source, 0)
        .unwrap();
    second.processed_materials.push(second_material);
    f.commit(&second);

    // 数量与处理状态仍正确，但两个操作的素材所有权被互换，不能只审计 count。
    f.conn
        .execute(
            "UPDATE memory_processed SET operation_id=CASE operation_id WHEN ?1 THEN ?2 ELSE ?1 END WHERE operation_id IN (?1,?2)",
            params![first.operation_id, second.operation_id],
        )
        .unwrap();
    for plan in [&first, &second] {
        let count: u64 = f
            .conn
            .query_row(
                "SELECT count(*) FROM memory_processed WHERE operation_id=?1",
                [&plan.operation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    assert!(matches!(
        f.repo.operation(&f.conn, &first.operation_id),
        Err(Error::Corrupt)
    ));
    let fault: String = f
        .conn
        .query_row(
            "SELECT operation_id FROM memory_run_faults WHERE run_id=?1",
            [&f.run.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fault, first.operation_id);
    let next = f.plan(f.version());
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &next),
        Reason::RecoveryRequired,
    );
    let recorded: String = f
        .conn
        .query_row(
            "SELECT operation_id FROM memory_processed WHERE material_id=?1",
            [first_material],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recorded, second.operation_id);
}
