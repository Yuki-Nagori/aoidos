//! 有界恢复、纠正屏障和审计文件保留契约。

use super::super::test_support::*;
use super::*;

#[test]
fn recovery_resumes_ready_but_rejects_missing_or_conflicting_prepared_body() {
    for present in [false, true] {
        let mut f = Fixture::new();
        let v = f.version();
        let plan = f.plan(v.clone());
        f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
        if present {
            f.repo
                .write_versions(&mut f.conn, &plan.operation_id)
                .unwrap();
        }
        let page = f
            .repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, None)
            .unwrap();
        assert_eq!(page.operations, 1);
        assert_eq!(
            f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
            if present {
                OperationState::Applied
            } else {
                OperationState::Rejected
            }
        );
    }
    let mut f = Fixture::new();
    let v = f.version();
    let plan = f.plan(v.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    let path = f.path(&v);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "conflict").unwrap();
    assert!(matches!(
        f.repo.write_versions(&mut f.conn, &plan.operation_id),
        Err(Error::Corrupt)
    ));
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Quarantined
    );
}

#[test]
fn corrupt_supplement_restores_ancestor_without_rewriting_original_versions() {
    let mut f = Fixture::new();
    let original = f.version();
    f.commit(&f.plan(original.clone()));
    let mut supplement = original.clone();
    supplement.version_id = new_id();
    supplement.parent_version_id = Some(original.version_id.clone());
    supplement.change = ChangeKind::Supplement;
    supplement.summary = "新的理解".into();
    f.commit(&f.plan(supplement.clone()));
    std::fs::write(f.path(&supplement), "broken").unwrap();
    assert!(matches!(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &original.entry_id),
        Err(Error::Corrupt)
    ));
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, None)
        .unwrap();
    assert_eq!(page.restored_entries, 1);
    let recovered = f
        .repo
        .current(&f.conn, &f.port, &f.run.run_id, &original.entry_id)
        .unwrap();
    assert_eq!(recovered.summary, original.summary);
    assert_eq!(
        recovered.parent_version_id.as_ref(),
        Some(&supplement.version_id)
    );
    assert_eq!(
        recovered.restored_from_version_id.as_ref(),
        Some(&original.version_id)
    );
    assert_eq!(recovered.change, ChangeKind::Restore);
    assert_eq!(
        std::fs::read(f.path(&original)).unwrap(),
        canonical(&original).unwrap()
    );
    assert_eq!(
        f.repo
            .cleanup(&f.conn, &f.port, &f.run.run_id, 256)
            .unwrap(),
        0
    );
}

#[test]
fn correction_barrier_prevents_resurrection_when_corrected_body_is_corrupt() {
    let mut f = Fixture::new();
    let original = f.version();
    f.commit(&f.plan(original.clone()));
    let mut corrected = original.clone();
    corrected.version_id = new_id();
    corrected.parent_version_id = Some(original.version_id.clone());
    corrected.change = ChangeKind::Correction;
    corrected.summary = "并不喜欢钟声".into();
    f.commit(&f.plan(corrected.clone()));
    std::fs::remove_file(f.path(&corrected)).unwrap();
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, None)
        .unwrap();
    assert_eq!(page.unavailable_entries, 1);
    rejected(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &original.entry_id),
        Reason::RecoveryRequired,
    );
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &original.entry_id)
            .unwrap()
            .current_version_id,
        corrected.version_id
    );
}

#[test]
fn source_loss_marks_unavailable_and_history_bound_queries_reject_stale_views() {
    let mut f = Fixture::new();
    let v = f.version();
    f.commit(&f.plan(v.clone()));
    f.port.invalid.set(true);
    rejected(
        f.repo.current(&f.conn, &f.port, &f.run.run_id, &v.entry_id),
        Reason::InvalidSource,
    );
    assert_eq!(
        f.repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, None)
            .unwrap()
            .unavailable_entries,
        1
    );
    rejected(
        f.repo.entries(&f.conn, &f.port, &f.run.run_id, 1, None, 1),
        Reason::StaleVersion,
    );
    rejected(
        f.repo.entries(&f.conn, &f.port, &f.run.run_id, 0, None, 0),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo
            .entries(&f.conn, &f.port, &f.run.run_id, 0, Some("bad"), 1),
        Reason::InvalidSchema,
    );
    let cursor = ScanCursor {
        through_operations: 0,
        after_operation: 0,
        run_id: f.run.run_id.clone(),
        history_revision: 1,
        through: 0,
        after: 0,
    };
    rejected(
        f.repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, Some(&cursor)),
        Reason::StaleVersion,
    );
}

#[test]
fn cleanup_only_removes_rejected_unreferenced_bodies_and_is_repeatable() {
    let mut f = Fixture::new();
    let v = f.version();
    let plan = f.plan(v.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.port.view.entity_revision += 1;
    rejected(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Reason::StaleVersion,
    );
    assert_eq!(
        f.repo.cleanup(&f.conn, &f.port, &f.run.run_id, 1).unwrap(),
        1
    );
    assert!(!f.path(&v).exists());
    assert_eq!(
        f.repo.cleanup(&f.conn, &f.port, &f.run.run_id, 1).unwrap(),
        0
    );
    rejected(
        f.repo.cleanup(&f.conn, &f.port, &f.run.run_id, 0),
        Reason::InvalidSchema,
    );
}

#[test]
fn cleanup_finishes_when_a_rejected_body_was_already_removed() {
    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.port.view.entity_revision += 1;
    rejected(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Reason::StaleVersion,
    );
    std::fs::remove_file(f.path(&version)).unwrap();
    assert_eq!(
        f.repo.cleanup(&f.conn, &f.port, &f.run.run_id, 1).unwrap(),
        1
    );
    assert_eq!(
        f.repo.cleanup(&f.conn, &f.port, &f.run.run_id, 1).unwrap(),
        0
    );
}

#[test]
fn cleanup_retains_conflicting_or_referenced_prepared_body() {
    let mut f = Fixture::new();
    let v = f.version();
    let plan = f.plan(v.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    assert_eq!(
        f.repo.cleanup(&f.conn, &f.port, &f.run.run_id, 1).unwrap(),
        0
    );
    f.port.view.entity_revision = 2;
    rejected(
        f.repo.apply(&mut f.conn, &f.port, &plan.operation_id),
        Reason::StaleVersion,
    );
    std::fs::write(f.path(&v), "changed after rejection").unwrap();
    assert!(matches!(
        f.repo.cleanup(&f.conn, &f.port, &f.run.run_id, 1),
        Err(Error::Corrupt)
    ));
    assert!(f.path(&v).exists());
}

#[test]
fn recovery_scan_has_fixed_upper_bound_and_rejects_invalid_cursor() {
    let mut f = Fixture::new();
    let v = f.version();
    f.commit(&f.plan(v));
    let cursor = ScanCursor {
        through_operations: 0,
        after_operation: 0,
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        through: 1,
        after: 0,
    };
    let newer = f.version();
    f.commit(&f.plan(newer));
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, Some(&cursor))
        .unwrap();
    assert_eq!(page.checked_entries, 1);
    assert!(page.next.is_none());
    let mut bad = cursor.clone();
    bad.after = 2;
    rejected(
        f.repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, Some(&bad)),
        Reason::StaleVersion,
    );
    bad = cursor;
    bad.through = 3;
    rejected(
        f.repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, Some(&bad)),
        Reason::StaleVersion,
    );
}

#[test]
fn recovery_rejects_oversized_operation_before_decoding_its_plan() {
    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.conn
        .execute(
            "UPDATE memory_operations SET plan_json=zeroblob(?1) WHERE operation_id=?2",
            rusqlite::params![MAX_PLAN_BYTES + 1, plan.operation_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.recover(&mut f.conn, &f.port, &f.run.run_id, None),
        Err(Error::Corrupt)
    ));
}

#[test]
fn recovery_promotes_a_prepared_body_and_propagates_uncertain_apply_errors() {
    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version);
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.conn
        .execute(
            "UPDATE memory_operations SET state='prepared' WHERE operation_id=?1",
            [&plan.operation_id],
        )
        .unwrap();
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, None)
        .unwrap();
    assert_eq!(page.operations, 1);
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Applied
    );

    let mut f = Fixture::new();
    let plan = f.plan(f.version());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.conn.execute_batch(
        "CREATE TRIGGER fail_recovery_apply BEFORE INSERT ON store_applied BEGIN SELECT RAISE(ABORT, 'uncertain'); END;",
    ).unwrap();
    assert!(matches!(
        f.repo.recover(&mut f.conn, &f.port, &f.run.run_id, None),
        Err(Error::Store(_))
    ));
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Ready
    );
}

#[test]
fn recovery_pauses_when_clock_receipts_exceed_the_page_budget() {
    let mut f = Fixture::new();
    for seq in 1..=257 {
        let mut source = f.source();
        source.record_seq = seq;
        f.repo
            .complete_round(&mut f.conn, &f.port, &f.run.run_id, &new_id(), &source)
            .unwrap();
    }
    f.port.view.history_revision = 1;
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, None)
        .unwrap();
    assert_eq!(page.operations, 0);
    assert_eq!(page.checked_entries, 0);
    assert!(page.next.is_some());
    assert!(matches!(
        f.repo.clock(&f.conn, &f.port, &f.run.run_id),
        Err(Error::Rejected(Reason::RecoveryRequired))
    ));
}

#[test]
fn recovery_quarantines_conflicting_prepared_files_and_propagates_io_errors() {
    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    let path = f.path(&version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "different body").unwrap();
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, None)
        .unwrap();
    assert_eq!(page.operations, 1);
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Quarantined
    );
    let reason: String = f
        .conn
        .query_row(
            "SELECT reason FROM memory_operations WHERE operation_id=?1",
            [&plan.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "bodyConflict");

    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    let path = f.path(&version);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "conflicting body").unwrap();
    f.conn.execute_batch(
        "CREATE TRIGGER fail_quarantine BEFORE UPDATE OF state ON memory_operations WHEN NEW.state='quarantined' BEGIN SELECT RAISE(ABORT, 'test'); END;",
    ).unwrap();
    assert!(matches!(
        f.repo.recover(&mut f.conn, &f.port, &f.run.run_id, None),
        Err(Error::Store(_))
    ));

    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    std::fs::create_dir_all(f.path(&version)).unwrap();
    assert!(matches!(
        f.repo.recover(&mut f.conn, &f.port, &f.run.run_id, None),
        Err(Error::Store(_))
    ));
}

#[test]
fn recovery_promotes_a_prepared_material_only_operation() {
    let mut f = Fixture::new();
    let material = f
        .repo
        .register_material(&f.conn, &f.port, &f.source(), 0)
        .unwrap();
    let mut plan = f.plan(f.version());
    plan.targets.clear();
    plan.processed_materials.push(material);
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    assert_eq!(
        f.repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, None)
            .unwrap()
            .operations,
        1
    );
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Applied
    );
}

#[test]
fn recovery_completes_a_prepared_state_only_target() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    let mut plan = f.plan(version.clone());
    plan.targets[0].expected.version_id = Some(version.version_id.clone());
    plan.targets[0].expected.state_revision = 1;
    plan.targets[0].next = None;
    plan.targets[0].state = EntryState::Archived;
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, None)
        .unwrap();
    assert_eq!(page.operations, 2);
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &version.entry_id)
            .unwrap()
            .state,
        EntryState::Archived
    );
}

#[test]
fn entry_recovery_yields_before_reading_when_budget_is_empty() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    assert!(matches!(
        f.repo
            .recover_entry(
                &mut f.conn,
                &f.port,
                &f.run,
                &f.port.view,
                &version.entry_id,
                &mut Budget {
                    versions: 0,
                    bytes: MAX_SCAN_BYTES,
                },
            )
            .unwrap(),
        EntryRecovery::Deferred
    ));
}

#[test]
fn recovery_cursor_replays_only_the_same_history_and_rejects_bad_parent_links() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    f.conn.execute(
        "UPDATE memory_entries SET status='unavailable',recovery_cursor='',recovery_history_revision=0 WHERE entry_id=?1",
        [&version.entry_id],
    ).unwrap();
    let boundary = f.port.view.clone();
    assert!(matches!(
        f.repo
            .recover_entry(
                &mut f.conn,
                &f.port,
                &f.run,
                &boundary,
                &version.entry_id,
                &mut Budget {
                    versions: 1,
                    bytes: MAX_SCAN_BYTES
                },
            )
            .unwrap(),
        EntryRecovery::Unavailable
    ));

    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    f.conn.execute(
        "UPDATE memory_entries SET status='unavailable',recovery_cursor=?1,recovery_history_revision=0 WHERE entry_id=?2",
        rusqlite::params![version.version_id, version.entry_id],
    ).unwrap();
    f.conn
        .execute(
            "UPDATE memory_versions SET parent_version_id=version_id WHERE version_id=?1",
            [&version.version_id],
        )
        .unwrap();
    std::fs::remove_file(f.path(&version)).unwrap();
    assert!(matches!(
        f.repo.recover_entry(
            &mut f.conn,
            &f.port,
            &f.run,
            &f.port.view,
            &version.entry_id,
            &mut Budget {
                versions: 4,
                bytes: MAX_SCAN_BYTES
            },
        ),
        Err(Error::Corrupt)
    ));
}

#[test]
fn recovery_surfaces_future_versions_and_storage_read_failures() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    let bytes = serde_json::to_vec(&serde_json::json!({ "version": 2 })).unwrap();
    f.conn
        .execute(
            "UPDATE memory_versions SET body_hash=?1,body_length=?2 WHERE version_id=?3",
            rusqlite::params![
                crate::model::hash(&bytes).as_slice(),
                bytes.len(),
                version.version_id
            ],
        )
        .unwrap();
    std::fs::write(f.path(&version), bytes).unwrap();
    assert!(matches!(
        f.repo.recover_entry(
            &mut f.conn,
            &f.port,
            &f.run,
            &f.port.view,
            &version.entry_id,
            &mut Budget {
                versions: 4,
                bytes: MAX_SCAN_BYTES
            },
        ),
        Err(Error::Rejected(Reason::PolicyUnavailable))
    ));

    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    std::fs::remove_file(f.path(&version)).unwrap();
    std::fs::create_dir(f.path(&version)).unwrap();
    assert!(matches!(
        f.repo.recover_entry(
            &mut f.conn,
            &f.port,
            &f.run,
            &f.port.view,
            &version.entry_id,
            &mut Budget {
                versions: 4,
                bytes: MAX_SCAN_BYTES
            },
        ),
        Err(Error::Store(_))
    ));
}

#[test]
fn recovery_stops_on_unreadable_ancestor_and_invalid_persistent_cursor() {
    let mut f = Fixture::new();
    let original = f.version();
    f.commit(&f.plan(original.clone()));
    let mut latest = original.clone();
    latest.version_id = new_id();
    latest.parent_version_id = Some(original.version_id.clone());
    latest.change = ChangeKind::Supplement;
    latest.summary = "追加理解".into();
    let mut plan = f.plan(latest.clone());
    plan.targets[0].expected.state_revision = 1;
    f.commit(&plan);
    std::fs::remove_file(f.path(&latest)).unwrap();
    std::fs::remove_file(f.path(&original)).unwrap();
    std::fs::create_dir(f.path(&original)).unwrap();
    assert!(matches!(
        f.repo.recover_entry(
            &mut f.conn,
            &f.port,
            &f.run,
            &f.port.view,
            &latest.entry_id,
            &mut Budget {
                versions: 8,
                bytes: MAX_SCAN_BYTES,
            },
        ),
        Err(Error::Store(_))
    ));

    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    f.conn.execute(
        "UPDATE memory_entries SET status='unavailable',recovery_cursor='bad-id',recovery_history_revision=0 WHERE entry_id=?1",
        [&version.entry_id],
    ).unwrap();
    assert!(matches!(
        f.repo.recover_entry(
            &mut f.conn,
            &f.port,
            &f.run,
            &f.port.view,
            &version.entry_id,
            &mut Budget {
                versions: 1,
                bytes: MAX_SCAN_BYTES,
            },
        ),
        Err(Error::Corrupt)
    ));
}

#[test]
fn correction_without_any_valid_source_stays_unavailable() {
    let mut f = Fixture::new();
    let original = f.version();
    f.commit(&f.plan(original.clone()));
    let mut corrected = original.clone();
    corrected.version_id = new_id();
    corrected.parent_version_id = Some(original.version_id.clone());
    corrected.change = ChangeKind::Correction;
    corrected.summary = "纠正后的偏好".into();
    let mut plan = f.plan(corrected.clone());
    plan.targets[0].expected.state_revision = 1;
    f.commit(&plan);
    std::fs::remove_file(f.path(&corrected)).unwrap();
    f.port.invalid.set(true);
    assert!(matches!(
        f.repo
            .recover_entry(
                &mut f.conn,
                &f.port,
                &f.run,
                &f.port.view,
                &corrected.entry_id,
                &mut Budget {
                    versions: 8,
                    bytes: MAX_SCAN_BYTES,
                },
            )
            .unwrap(),
        EntryRecovery::Unavailable
    ));
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &corrected.entry_id)
            .unwrap()
            .state,
        EntryState::Unavailable
    );
}

#[test]
fn recovery_defers_long_invalid_chains_without_reviving_them() {
    let mut f = Fixture::new();
    let mut current = f.version();
    f.commit(&f.plan(current.clone()));
    for revision in 1..=520 {
        let mut next = current.clone();
        next.version_id = new_id();
        next.parent_version_id = Some(current.version_id.clone());
        next.change = ChangeKind::Supplement;
        next.summary = format!("第 {revision} 次补充");
        let mut plan = f.plan(next.clone());
        plan.targets[0].expected.version_id = Some(current.version_id.clone());
        plan.targets[0].expected.state_revision = revision;
        f.commit(&plan);
        current = next;
    }
    f.port.invalid.set(true);
    let mut cursor = None;
    let mut deferred = None;
    let mut progress = Vec::new();
    for _ in 0..8 {
        let page = f
            .repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, cursor.as_ref())
            .unwrap();
        cursor = page.next.clone();
        progress.push((page.operations, page.checked_entries, page.next.is_some()));
        if page.operations == 33 && page.checked_entries == 0 {
            deferred = Some(page);
            break;
        }
    }
    let page = deferred.unwrap_or_else(|| panic!("recovery progress: {progress:?}"));
    assert!(page.next.is_some());
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &current.entry_id)
            .unwrap()
            .state,
        EntryState::Unavailable
    );
}

#[test]
fn recovery_paginates_fixed_operation_and_entry_upper_bounds() {
    let mut f = Fixture::new();
    for ordinal in 0..257 {
        let mut source = f.source();
        source.record_seq = ordinal + 1;
        let material = f
            .repo
            .register_material(&f.conn, &f.port, &source, ordinal)
            .unwrap();
        let mut plan = f.plan(f.version());
        plan.targets.clear();
        plan.processed_materials.push(material);
        f.commit(&plan);
    }
    let through_operations: u64 = f
        .conn
        .query_row(
            "SELECT max(rowid) FROM memory_operations WHERE run_id=?1",
            [&f.run.run_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut cursor = ScanCursor {
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        through: 0,
        after: 0,
        through_operations,
        after_operation: 0,
    };
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, Some(&cursor))
        .unwrap();
    assert_eq!(page.operations, 256);
    cursor = page
        .next
        .expect("257th operation must remain for next page");
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, Some(&cursor))
        .unwrap();
    assert_eq!(page.operations, 1);
    assert!(page.next.is_none());

    let mut f = Fixture::new();
    for _ in 0..257 {
        let version = f.version();
        f.commit(&f.plan(version));
    }
    f.conn.execute(
        "UPDATE memory_entries SET status='unavailable',recovery_cursor='',recovery_history_revision=0,effect_history_revision=0",
        [],
    ).unwrap();
    let through_operations: u64 = f
        .conn
        .query_row(
            "SELECT max(rowid) FROM memory_operations WHERE run_id=?1",
            [&f.run.run_id],
            |row| row.get(0),
        )
        .unwrap();
    let through: u64 = f
        .conn
        .query_row(
            "SELECT max(rowid) FROM memory_entries WHERE run_id=?1",
            [&f.run.run_id],
            |row| row.get(0),
        )
        .unwrap();
    let cursor = ScanCursor {
        run_id: f.run.run_id.clone(),
        history_revision: 0,
        through,
        after: 0,
        through_operations,
        after_operation: through_operations,
    };
    let page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, Some(&cursor))
        .unwrap();
    assert_eq!(page.checked_entries, 256);
    assert!(page.next.is_some());
}

#[test]
fn impossible_prepared_relation_size_is_corrupt_instead_of_yielding_forever() {
    let mut f = Fixture::new();
    let version = f.version();
    let plan = f.plan(version.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    let tx = f.conn.transaction().unwrap();
    for _ in 0..128 {
        tx.execute(
            "INSERT INTO memory_version_evidence VALUES(?1,?2,?3)",
            params![version.version_id, new_id(), vec![b' '; 32700]],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    assert!(matches!(
        f.repo.recover(&mut f.conn, &f.port, &f.run.run_id, None),
        Err(Error::Corrupt)
    ));
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
fn effect_reconciliation_exhaustion_defers_entry_scan_and_resumes_without_double_revision() {
    let mut f = Fixture::new();
    for _ in 0..33 {
        let mut plan = f.plan(f.version());
        for seq in 1..=6 {
            let mut source = f.source();
            source.record_seq = seq;
            plan.targets[0].effects.push(Effect {
                effect_id: new_id(),
                kind: EffectKind::Gain,
                source,
                logical_position: 0,
                gain: 1,
            });
        }
        f.commit(&plan);
    }
    f.port.view.history_revision = 1;
    let mut page = f
        .repo
        .recover(&mut f.conn, &f.port, &f.run.run_id, None)
        .unwrap();
    assert!(page.next.is_some());
    assert!(page.checked_entries < 33);
    let mut total = page.checked_entries;
    for _ in 0..5 {
        let Some(cursor) = page.next.clone() else {
            break;
        };
        page = f
            .repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, Some(&cursor))
            .unwrap();
        total += page.checked_entries;
    }
    assert!(page.next.is_none());
    assert_eq!(total, 33);
    let revised: u64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM memory_entries WHERE state_revision!=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revised, 0);
}

#[test]
fn malformed_persisted_recovery_identity_is_rejected_before_following_body() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    let invalid = "x".repeat(36);
    f.conn.execute("INSERT INTO memory_versions(version_id,entry_id,parent_version_id,restored_from_version_id,body_hash,body_length,source_set_hash,evidence_set_hash,policy_hash,subject_json,change_kind) SELECT ?1,entry_id,parent_version_id,restored_from_version_id,body_hash,body_length,source_set_hash,evidence_set_hash,policy_hash,subject_json,change_kind FROM memory_versions WHERE version_id=?2", params![invalid, version.version_id]).unwrap();
    f.conn.execute("UPDATE memory_entries SET status='unavailable',recovery_cursor=?1,recovery_history_revision=0 WHERE entry_id=?2", params![invalid, version.entry_id]).unwrap();
    let mut budget = Budget {
        versions: MAX_SCAN,
        bytes: MAX_SCAN_BYTES,
    };
    assert!(matches!(
        f.repo.recover_entry(
            &mut f.conn,
            &f.port,
            &f.run,
            &f.port.view,
            &version.entry_id,
            &mut budget
        ),
        Err(Error::Corrupt)
    ));
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &version.entry_id)
            .unwrap()
            .current_version_id,
        version.version_id
    );
}

#[test]
fn dense_valid_prepared_groups_page_without_partial_dependency_commit() {
    for sources_per_version in [12, 24, 48] {
        let mut f = Fixture::new();
        let mut plans = Vec::new();
        for _ in 0..8 {
            let mut targets = Vec::new();
            for _ in 0..16 {
                let mut version = f.version();
                version.summary = "x".repeat(4096);
                version.sources = (1..=sources_per_version)
                    .map(|seq| {
                        let mut source = f.source();
                        source.record_seq = seq;
                        source
                    })
                    .collect();
                version.source_set_hash = set_hash(&version.sources).unwrap();
                targets.extend(f.plan(version).targets);
            }
            let mut plan = f.plan(f.version());
            plan.targets = targets;
            f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
            f.repo
                .write_versions(&mut f.conn, &plan.operation_id)
                .unwrap();
            plans.push(plan);
        }
        let page = f
            .repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, None)
            .unwrap();
        assert!(page.next.is_some());
        assert!(page.operations > 0 && page.operations < plans.len());
        let next = &plans[page.operations];
        assert_eq!(
            f.repo.operation(&f.conn, &next.operation_id).unwrap().1,
            OperationState::Ready
        );
        for target in &next.targets {
            assert!(matches!(
                f.repo.entry(&f.conn, &f.run.run_id, &target.entry_id),
                Err(Error::NotFound)
            ));
        }
    }
}
