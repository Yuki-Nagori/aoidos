//! 回退重建复用原始增量与完成回执，分页期间拒绝旧计划与消费。

use super::super::test_support::*;
use super::*;

fn source_at(f: &Fixture, seq: u64) -> SourceRef {
    let mut source = f.source();
    source.record_seq = seq;
    source
}
fn budget(versions: usize) -> Budget {
    Budget {
        versions,
        bytes: MAX_SCAN_BYTES,
    }
}
fn finish_recovery(f: &mut Fixture) -> Vec<RecoveryPage> {
    let mut pages = Vec::new();
    let mut cursor = None;
    for _ in 0..20 {
        let page = f
            .repo
            .recover(&mut f.conn, &f.port, &f.run.run_id, cursor.as_ref())
            .unwrap();
        cursor = page.next.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
    }
    panic!("bounded recovery did not finish");
}

#[test]
fn withdrawn_round_receipts_reduce_clock_and_old_plan_stays_stale() {
    let mut f = Fixture::new();
    for seq in 1..=2 {
        let source = source_at(&f, seq);
        f.repo
            .complete_round(&mut f.conn, &f.port, &f.run.run_id, &new_id(), &source)
            .unwrap();
    }
    assert_eq!(f.repo.clock(&f.conn, &f.port, &f.run.run_id).unwrap(), 2);
    let plan = f.plan(f.version());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.repo
        .write_versions(&mut f.conn, &plan.operation_id)
        .unwrap();
    f.port.view.history_revision = 1;
    f.port.through_record.set(1);
    rejected(
        f.repo.clock(&f.conn, &f.port, &f.run.run_id),
        Reason::RecoveryRequired,
    );
    finish_recovery(&mut f);
    assert_eq!(f.repo.clock(&f.conn, &f.port, &f.run.run_id).unwrap(), 1);
    assert_eq!(
        f.repo.operation(&f.conn, &plan.operation_id).unwrap().1,
        OperationState::Rejected
    );
    let count: u64 = f
        .conn
        .query_row("SELECT count(*) FROM memory_clock_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn withdrawn_effect_increments_state_revision_without_recalculating_other_gain() {
    let mut f = Fixture::new();
    let version = f.version();
    let mut plan = f.plan(version.clone());
    for (seq, gain) in [(1, 50_000), (2, 10_000)] {
        plan.targets[0].effects.push(Effect {
            effect_id: new_id(),
            kind: EffectKind::Gain,
            source: source_at(&f, seq),
            logical_position: 0,
            gain,
        });
    }
    f.commit(&plan);
    assert_eq!(
        f.repo
            .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 32)
            .unwrap()
            .len(),
        2
    );
    f.port.view.history_revision = 1;
    f.port.through_record.set(1);
    rejected(
        f.repo
            .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 32),
        Reason::RecoveryRequired,
    );
    finish_recovery(&mut f);
    let effects = f
        .repo
        .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 32)
        .unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].1.gain, 50_000);
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &version.entry_id)
            .unwrap()
            .state_revision,
        2
    );
    let gain: u64 = f
        .conn
        .query_row(
            "SELECT gain FROM memory_effects WHERE record_seq=2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(gain, 10_000);
    finish_recovery(&mut f);
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &version.entry_id)
            .unwrap()
            .state_revision,
        2
    );
    rejected(
        f.repo
            .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 0),
        Reason::InvalidSchema,
    );
}

#[test]
fn direct_effect_reads_audit_their_committed_operation_before_returning_values() {
    for mutation in [
        "UPDATE memory_effects SET record_seq=record_seq+1",
        "UPDATE memory_effects SET gain=gain+1",
        "INSERT INTO memory_effects SELECT '00000000-0000-0000-0000-000000000001',operation_id,entry_id,run_id,session_id,record_seq+1,kind,logical_position,gain,source_json,valid FROM memory_effects",
    ] {
        let mut f = Fixture::new();
        let version = f.version();
        let mut plan = f.plan(version.clone());
        plan.targets[0].effects.push(Effect {
            effect_id: new_id(),
            kind: EffectKind::Gain,
            source: f.source(),
            logical_position: 0,
            gain: 5,
        });
        f.commit(&plan);
        f.conn.execute_batch(mutation).unwrap();
        assert!(matches!(
            f.repo
                .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 32),
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
fn a_prepared_operation_cannot_expose_an_uncommitted_effect() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    let mut plan = f.plan(version.clone());
    plan.targets[0].expected = Expected {
        version_id: Some(version.version_id.clone()),
        state_revision: 1,
    };
    plan.targets[0].next = None;
    let effect = Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: f.source(),
        logical_position: 0,
        gain: 5,
    };
    plan.targets[0].effects.push(effect.clone());
    f.repo.prepare(&mut f.conn, &f.port, &plan).unwrap();
    f.conn.execute(
        "INSERT INTO memory_effects(effect_id,operation_id,entry_id,run_id,session_id,record_seq,kind,logical_position,gain,source_json) VALUES(?1,?2,?3,?4,?5,?6,'gain',0,5,?7)",
        params![effect.effect_id, plan.operation_id, version.entry_id, f.run.run_id, effect.source.session_id, effect.source.record_seq, canonical(&effect.source).unwrap()],
    ).unwrap();
    assert!(matches!(
        f.repo
            .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 32),
        Err(Error::Corrupt)
    ));
    rejected(
        f.repo.prepare(&mut f.conn, &f.port, &plan),
        Reason::RecoveryRequired,
    );
}

#[test]
fn clock_progress_survives_budget_exhaustion_and_restarts_on_new_history() {
    let mut f = Fixture::new();
    for seq in 1..=3 {
        let source = source_at(&f, seq);
        f.repo
            .complete_round(&mut f.conn, &f.port, &f.run.run_id, &new_id(), &source)
            .unwrap();
    }
    f.port.view.history_revision = 1;
    let view = f.port.view.clone();
    assert!(
        !f.repo
            .reconcile_clock(&mut f.conn, &f.port, &f.run, &view, &mut budget(1))
            .unwrap()
    );
    rejected(
        f.repo.clock(&f.conn, &f.port, &f.run.run_id),
        Reason::RecoveryRequired,
    );
    f.port.view.history_revision = 2;
    f.port.through_record.set(1);
    let view = f.port.view.clone();
    assert!(
        !f.repo
            .reconcile_clock(&mut f.conn, &f.port, &f.run, &view, &mut budget(1))
            .unwrap()
    );
    assert!(
        f.repo
            .reconcile_clock(&mut f.conn, &f.port, &f.run, &view, &mut budget(256))
            .unwrap()
    );
    assert_eq!(f.repo.clock(&f.conn, &f.port, &f.run.run_id).unwrap(), 1);
    assert!(
        f.repo
            .reconcile_clock(&mut f.conn, &f.port, &f.run, &view, &mut budget(0))
            .unwrap()
    );
}

#[test]
fn effect_progress_is_persistent_and_unchanged_gains_are_idempotent() {
    let mut f = Fixture::new();
    let version = f.version();
    let mut plan = f.plan(version.clone());
    for seq in 1..=3 {
        plan.targets[0].effects.push(Effect {
            effect_id: new_id(),
            kind: EffectKind::Gain,
            source: source_at(&f, seq),
            logical_position: 0,
            gain: 5_000,
        });
    }
    f.commit(&plan);
    f.port.view.history_revision = 1;
    f.port.through_record.set(1);
    let view = f.port.view.clone();
    f.repo
        .reconcile_clock(&mut f.conn, &f.port, &f.run, &view, &mut budget(256))
        .unwrap();
    assert!(
        !f.repo
            .reconcile_effects(
                &mut f.conn,
                &f.port,
                &f.run,
                &view,
                &version.entry_id,
                &mut budget(1)
            )
            .unwrap()
    );
    rejected(
        f.repo
            .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 32),
        Reason::RecoveryRequired,
    );
    assert!(
        f.repo
            .reconcile_effects(
                &mut f.conn,
                &f.port,
                &f.run,
                &view,
                &version.entry_id,
                &mut budget(256)
            )
            .unwrap()
    );
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &version.entry_id)
            .unwrap()
            .state_revision,
        3
    );
    assert!(
        f.repo
            .reconcile_effects(
                &mut f.conn,
                &f.port,
                &f.run,
                &view,
                &version.entry_id,
                &mut budget(0)
            )
            .unwrap()
    );
    assert_eq!(
        f.repo
            .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 1)
            .unwrap()[0]
            .1
            .gain,
        5_000
    );
}

#[test]
fn recovery_audits_applied_receipts_before_consuming_entries() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version));
    f.conn.execute("DELETE FROM store_applied", []).unwrap();
    assert!(matches!(
        f.repo.recover(&mut f.conn, &f.port, &f.run.run_id, None),
        Err(Error::Corrupt)
    ));
}

#[test]
fn full_recovery_continues_across_operation_and_entry_pages() {
    let mut f = Fixture::new();
    for _ in 0..140 {
        let plan = f.plan(f.version());
        f.commit(&plan);
    }
    let pages = finish_recovery(&mut f);
    assert!(pages.len() > 1);
    assert_eq!(pages.iter().map(|page| page.operations).sum::<usize>(), 140);
    assert_eq!(
        pages.iter().map(|page| page.checked_entries).sum::<usize>(),
        140
    );
    assert!(
        pages
            .iter()
            .all(|page| page.operations + page.checked_entries <= 256)
    );
}

#[test]
fn withdrawn_healthy_body_can_become_valid_again_without_permanent_quarantine() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    f.port.view.history_revision = 1;
    f.port.through_record.set(0);
    finish_recovery(&mut f);
    rejected(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &version.entry_id),
        Reason::RecoveryRequired,
    );
    let quarantine: bool = f
        .conn
        .query_row(
            "SELECT quarantined FROM memory_versions WHERE version_id=?1",
            [&version.version_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!quarantine);
    f.port.view.history_revision = 2;
    f.port.through_record.set(1);
    finish_recovery(&mut f);
    let restored = f
        .repo
        .current(&f.conn, &f.port, &f.run.run_id, &version.entry_id)
        .unwrap();
    assert_eq!(restored.summary, version.summary);
    assert_eq!(restored.change, ChangeKind::Restore);
}

#[test]
fn oversized_reconciliation_metadata_and_revision_exhaustion_fail_closed() {
    let mut f = Fixture::new();
    let source = f.source();
    f.repo
        .complete_round(&mut f.conn, &f.port, &f.run.run_id, &new_id(), &source)
        .unwrap();
    f.conn
        .execute(
            "UPDATE memory_clock_receipts SET source_json=?1",
            [vec![b'x'; 32769]],
        )
        .unwrap();
    f.port.view.history_revision = 1;
    let view = f.port.view.clone();
    assert!(matches!(
        f.repo
            .reconcile_clock(&mut f.conn, &f.port, &f.run, &view, &mut budget(256)),
        Err(Error::Corrupt)
    ));
    let mut f = Fixture::new();
    let version = f.version();
    let mut plan = f.plan(version.clone());
    plan.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: f.source(),
        logical_position: 0,
        gain: 1,
    });
    f.commit(&plan);
    f.conn
        .execute(
            "UPDATE memory_effects SET source_json=?1",
            [vec![b'x'; 32769]],
        )
        .unwrap();
    f.port.view.history_revision = 1;
    let view = f.port.view.clone();
    assert!(matches!(
        f.repo.reconcile_effects(
            &mut f.conn,
            &f.port,
            &f.run,
            &view,
            &version.entry_id,
            &mut budget(256),
        ),
        Err(Error::Corrupt)
    ));
    let mut f = Fixture::new();
    let version = f.version();
    let mut plan = f.plan(version.clone());
    plan.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: f.source(),
        logical_position: 0,
        gain: 1,
    });
    f.commit(&plan);
    f.conn
        .execute("UPDATE memory_entries SET state_revision=?1", [MAX_INTEGER])
        .unwrap();
    f.port.invalid.set(true);
    f.port.view.history_revision = 1;
    let view = f.port.view.clone();
    rejected(
        f.repo.reconcile_effects(
            &mut f.conn,
            &f.port,
            &f.run,
            &view,
            &version.entry_id,
            &mut budget(256),
        ),
        Reason::CapacityBlocked,
    );
}

#[test]
fn empty_receipt_sets_and_valid_effect_rows_finish_reconciliation() {
    let mut f = Fixture::new();
    let mut boundary = f.port.view.clone();
    boundary.history_revision = 1;
    assert!(
        f.repo
            .reconcile_clock(&mut f.conn, &f.port, &f.run, &boundary, &mut budget(1),)
            .unwrap()
    );
    f.port.view.history_revision = 1;
    assert_eq!(f.repo.clock(&f.conn, &f.port, &f.run.run_id).unwrap(), 0);

    let mut f = Fixture::new();
    let version = f.version();
    let mut plan = f.plan(version.clone());
    plan.targets[0].effects.push(Effect {
        effect_id: new_id(),
        kind: EffectKind::Gain,
        source: f.source(),
        logical_position: 0,
        gain: 25,
    });
    f.commit(&plan);
    let mut boundary = f.port.view.clone();
    boundary.history_revision = 1;
    assert!(
        f.repo
            .reconcile_clock(&mut f.conn, &f.port, &f.run, &boundary, &mut budget(1),)
            .unwrap()
    );
    assert!(
        f.repo
            .reconcile_effects(
                &mut f.conn,
                &f.port,
                &f.run,
                &boundary,
                &version.entry_id,
                &mut budget(1),
            )
            .unwrap()
    );
    f.port.view.history_revision = 1;
    assert_eq!(
        f.repo
            .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 32)
            .unwrap()[0]
            .1
            .gain,
        25
    );

    f.port.invalid.set(true);
    assert!(
        f.repo
            .effects(&f.conn, &f.port, &f.run.run_id, &version.entry_id, 0, 32)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn failed_completion_markers_leave_reconciliation_unconfirmed() {
    let mut f = Fixture::new();
    let mut boundary = f.port.view.clone();
    boundary.history_revision = 1;
    f.conn.execute_batch(
        "CREATE TRIGGER fail_clock_confirmation BEFORE UPDATE OF clock_history_revision ON memory_runs BEGIN SELECT RAISE(ABORT, 'test'); END;",
    ).unwrap();
    assert!(matches!(
        f.repo
            .reconcile_clock(&mut f.conn, &f.port, &f.run, &boundary, &mut budget(1),),
        Err(Error::Store(_))
    ));

    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    let mut boundary = f.port.view.clone();
    boundary.history_revision = 1;
    f.conn.execute_batch(
        "CREATE TRIGGER fail_effect_confirmation BEFORE UPDATE OF effect_history_revision ON memory_entries WHEN NEW.effect_history_revision IS NOT NULL BEGIN SELECT RAISE(ABORT, 'test'); END;",
    ).unwrap();
    assert!(matches!(
        f.repo.reconcile_effects(
            &mut f.conn,
            &f.port,
            &f.run,
            &boundary,
            &version.entry_id,
            &mut budget(1),
        ),
        Err(Error::Store(_))
    ));

    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    let mut boundary = f.port.view.clone();
    boundary.history_revision = 1;
    f.conn.execute_batch(
        "CREATE TRIGGER ignore_effect_confirmation BEFORE UPDATE OF effect_history_revision ON memory_entries WHEN NEW.effect_history_revision IS NOT NULL BEGIN SELECT RAISE(IGNORE); END;",
    ).unwrap();
    assert!(matches!(
        f.repo.reconcile_effects(
            &mut f.conn,
            &f.port,
            &f.run,
            &boundary,
            &version.entry_id,
            &mut budget(1),
        ),
        Err(Error::Corrupt)
    ));
}

#[test]
fn effect_reconciliation_stops_at_its_fixed_row_page() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    let current_version = version.version_id.clone();
    for (state_revision, start) in (1_u64..).zip([1_u64, 129, 257]) {
        let end = (start + 127).min(257);
        let effects = (start..=end)
            .map(|seq| {
                let mut source = f.source();
                source.record_seq = seq;
                Effect {
                    effect_id: new_id(),
                    kind: EffectKind::Gain,
                    source,
                    logical_position: 0,
                    gain: seq,
                }
            })
            .collect::<Vec<_>>();
        let mut plan = f.plan(f.version());
        plan.targets[0].entry_id = version.entry_id.clone();
        plan.targets[0].expected.version_id = Some(current_version.clone());
        plan.targets[0].expected.state_revision = state_revision;
        plan.targets[0].next = None;
        plan.targets[0].effects = effects;
        f.commit(&plan);
    }

    let mut boundary = f.port.view.clone();
    boundary.history_revision = 1;
    f.repo
        .reconcile_clock(&mut f.conn, &f.port, &f.run, &boundary, &mut budget(1))
        .unwrap();
    assert!(
        !f.repo
            .reconcile_effects(
                &mut f.conn,
                &f.port,
                &f.run,
                &boundary,
                &version.entry_id,
                &mut budget(256),
            )
            .unwrap()
    );
    assert!(
        f.repo
            .reconcile_effects(
                &mut f.conn,
                &f.port,
                &f.run,
                &boundary,
                &version.entry_id,
                &mut budget(256),
            )
            .unwrap()
    );
}
