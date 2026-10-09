use super::super::state::{InFlight, Operation, Phase};
use super::*;
use crate::record::facts::{SceneNode, ScenePosition};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
struct Events {
    seq: [AtomicU64; 4],
    prepare_failure: AtomicBool,
    delivery_failure: AtomicBool,
    delivered: Mutex<Vec<(u64, PhaseEvent)>>,
    retired: AtomicBool,
}
impl PhaseEvents for Events {
    fn prepare(&self, event: &PhaseEvent) -> Result<u64, Fault> {
        if self.prepare_failure.load(Ordering::SeqCst) {
            return Err(Fault::event());
        }
        let index = match event {
            PhaseEvent::Changed(_) => 0,
            PhaseEvent::Scene { .. } => 1,
            PhaseEvent::Done { .. } => 2,
            PhaseEvent::Failed { .. } => 3,
        };
        Ok(self.seq[index].fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn deliver(&self, seq: u64, event: PhaseEvent) -> Result<(), Fault> {
        if self.delivery_failure.load(Ordering::SeqCst) {
            return Err(Fault::event());
        }
        self.delivered.lock().unwrap().push((seq, event));
        Ok(())
    }
    fn retire(&self, _: &str) {
        self.retired.store(true, Ordering::SeqCst);
        for seq in &self.seq {
            seq.store(0, Ordering::SeqCst);
        }
    }
}
#[test]
fn explicit_retirement_rejects_old_reservations_and_late_drop_keeps_the_new_stream() {
    let events = Arc::new(Events::default());
    let old = Publisher::new(state(), events.clone()).unwrap();
    let baseline = old.snapshot().state;
    let pending = old.prepare(baseline.clone(), None).unwrap();
    old.retire();
    assert_eq!(old.confirm(pending).unwrap_err().code, "app.not-ready");
    assert_eq!(
        old.prepare(baseline.clone(), None).err().unwrap().code,
        "app.not-ready"
    );
    let new = Publisher::new(baseline, events.clone()).unwrap();
    new.confirm(new.prepare(new.snapshot().state, None).unwrap())
        .unwrap();
    assert_eq!(new.snapshot().seq.phase_changed, 1);
    drop(old);
    new.confirm(new.prepare(new.snapshot().state, None).unwrap())
        .unwrap();
    assert_eq!(new.snapshot().seq.phase_changed, 2);
}
fn state() -> PhaseState {
    PhaseState {
        session_id: uuid::Uuid::new_v4().to_string(),
        state_epoch: String::new(),
        phase_revision: 0,
        history_revision: 0,
        phase: Phase::Idle,
        scene: None,
        in_flight: None,
        check: None,
        needs_recovery: false,
        resume_required: false,
        checkpoint: None,
        last_operation: None,
    }
}
fn scene() -> ScenePosition {
    ScenePosition {
        scene_id: "room".into(),
        path: vec![SceneNode {
            kind: "scene".into(),
            id: "room".into(),
            title: "房间".into(),
        }],
    }
}
#[test]
fn full_state_and_four_baselines_commit_together_before_delivery() {
    let events = Arc::new(Events::default());
    let publisher = Publisher::new(state(), events.clone()).unwrap();
    let initial = publisher.snapshot();
    assert!(format::valid_uuid(&initial.state.state_epoch));
    let id = uuid::Uuid::new_v4().to_string();
    let mut next = initial.state.clone();
    next.scene = Some(scene());
    next.last_operation = Some(Operation {
        operation_id: id.clone(),
        round_id: None,
        outcome: OperationOutcome::Accepted,
        error: None,
    });
    let prepared = publisher
        .prepare(
            next,
            Some(Finished {
                operation_id: id,
                outcome: Outcome::Completed,
                error: None,
            }),
        )
        .unwrap();
    assert_eq!(publisher.snapshot(), initial);
    publisher.confirm(prepared).unwrap();
    let snapshot = publisher.snapshot();
    assert_eq!(snapshot.state.phase_revision, 1);
    assert_eq!(
        snapshot.seq,
        Sequences {
            phase_changed: 1,
            scene_advanced: 1,
            operation_done: 1,
            operation_failed: 0
        }
    );
    assert_eq!(
        snapshot.state.last_operation.unwrap().outcome,
        OperationOutcome::Completed
    );
    let delivered = events.delivered.lock().unwrap();
    assert_eq!(delivered.len(), 3);
    for (_, event) in delivered.iter() {
        assert_eq!(event.state().phase_revision, 1);
        assert!(event.name().starts_with("engine:"));
    }
    drop(delivered);
    events.delivery_failure.store(true, Ordering::SeqCst);
    let mut next = publisher.snapshot().state;
    next.scene = None;
    let prepared = publisher.prepare(next, None).unwrap();
    assert_eq!(
        publisher.confirm(prepared).unwrap_err().code,
        "app.event-failed"
    );
    assert_eq!(publisher.snapshot().state.phase_revision, 2);
    assert_eq!(publisher.snapshot().seq.scene_advanced, 2);
    drop(publisher);
    assert!(events.retired.load(Ordering::SeqCst));
}
#[test]
fn old_operation_terminal_keeps_new_operation_and_turn_discovery_changes_revision() {
    let events = Arc::new(Events::default());
    let publisher = Publisher::new(state(), events.clone()).unwrap();
    let new = uuid::Uuid::new_v4().to_string();
    let mut next = publisher.snapshot().state;
    next.phase = Phase::Generating;
    next.in_flight = Some(InFlight {
        operation_id: new.clone(),
        round_id: Some(uuid::Uuid::new_v4().to_string()),
        turn_id: None,
    });
    next.last_operation = Some(Operation {
        operation_id: new.clone(),
        round_id: None,
        outcome: OperationOutcome::Accepted,
        error: None,
    });
    publisher
        .confirm(publisher.prepare(next, None).unwrap())
        .unwrap();
    let mut next = publisher.snapshot().state;
    next.in_flight.as_mut().unwrap().turn_id = Some(uuid::Uuid::new_v4().to_string());
    publisher
        .confirm(
            publisher
                .prepare(
                    next,
                    Some(Finished {
                        operation_id: uuid::Uuid::new_v4().to_string(),
                        outcome: Outcome::Failed,
                        error: Some(Fault::new("engine.invalid-phase", "旧操作失败")),
                    }),
                )
                .unwrap(),
        )
        .unwrap();
    let snapshot = publisher.snapshot();
    assert_eq!(snapshot.state.phase_revision, 2);
    assert!(snapshot.state.in_flight.unwrap().turn_id.is_some());
    assert_eq!(snapshot.state.last_operation.unwrap().operation_id, new);
    assert_eq!(snapshot.seq.operation_failed, 1);
    let delivered = events.delivered.lock().unwrap();
    assert_eq!(
        delivered.last().unwrap().1.name(),
        "engine:operation:failed"
    );
    assert_eq!(
        delivered
            .last()
            .unwrap()
            .1
            .state()
            .last_operation
            .as_ref()
            .unwrap()
            .operation_id,
        new
    );
    drop(delivered);
    let mut stale = publisher.snapshot().state;
    stale.phase_revision = 0;
    assert!(publisher.prepare(stale, None).is_err());
    let next = publisher.snapshot().state;
    let old = publisher.prepare(next.clone(), None).unwrap();
    let fresh = publisher.prepare(next, None).unwrap();
    publisher.confirm(fresh).unwrap();
    assert!(publisher.confirm(old).is_err());
}
#[test]
fn preparation_and_invalid_states_never_change_confirmation() {
    let events = Arc::new(Events::default());
    let publisher = Publisher::new(state(), events.clone()).unwrap();
    let initial = publisher.snapshot();
    events.prepare_failure.store(true, Ordering::SeqCst);
    assert!(publisher.prepare(initial.state.clone(), None).is_err());
    assert_eq!(publisher.snapshot(), initial);
    events.prepare_failure.store(false, Ordering::SeqCst);
    let mut invalid = initial.state.clone();
    invalid.scene = Some(scene());
    let duplicate = invalid.scene.as_ref().unwrap().path[0].clone();
    invalid.scene.as_mut().unwrap().path.push(duplicate);
    assert!(publisher.prepare(invalid, None).is_err());
    assert!(
        publisher
            .prepare(
                initial.state.clone(),
                Some(Finished {
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    outcome: Outcome::Failed,
                    error: None
                })
            )
            .is_err()
    );
    assert!(validate_error("engine.invalid-phase", &"x".repeat(513)).is_err());
    assert!(validate_error("/", "故障").is_err());
    let json = aoidos_json::to_string(&initial).unwrap();
    assert!(!json.contains("inFlight"));
    assert_eq!(
        serde_json::from_str::<PhaseSnapshot>(&json).unwrap(),
        initial
    );
}

#[test]
fn quarantine_preserves_confirmed_sequences_and_rejects_work_after_retirement() {
    let events = Arc::new(Events::default());
    let mut initial = state();
    initial.last_operation = Some(Operation {
        operation_id: uuid::Uuid::new_v4().to_string(),
        round_id: None,
        outcome: OperationOutcome::Accepted,
        error: None,
    });
    let publisher = Publisher::new(initial, events).unwrap();
    publisher
        .confirm(publisher.prepare(publisher.snapshot().state, None).unwrap())
        .unwrap();
    let before = publisher.snapshot();
    publisher.quarantine(Fault::event());
    let after = publisher.snapshot();
    assert_eq!(after.seq, before.seq);
    assert_eq!(after.state.phase_revision, before.state.phase_revision + 1);
    assert!(after.state.needs_recovery);
    assert!(!after.state.resume_required);
    assert_eq!(
        after.state.last_operation.unwrap().outcome,
        OperationOutcome::Failed
    );
    publisher.retire();
    let before = publisher.snapshot();
    publisher.quarantine(Fault::bad_request());
    assert_eq!(publisher.snapshot(), before);
    let empty = Publisher::new(state(), Arc::new(Events::default())).unwrap();
    empty.quarantine(Fault::event());
    assert!(empty.snapshot().state.needs_recovery);
}

#[test]
fn invalid_public_identities_and_checkpoints_are_rejected_before_events_are_reserved() {
    use super::super::state::{CheckStatus, CheckSummary, Checkpoint, CheckpointStage};
    use crate::record::facts::DiceMode;
    let events = Arc::new(Events::default());
    let publisher = Publisher::new(state(), events.clone()).unwrap();
    for invalid in 0..6 {
        let mut next = publisher.snapshot().state;
        match invalid {
            0 => {
                next.in_flight = Some(InFlight {
                    operation_id: "bad".into(),
                    round_id: None,
                    turn_id: None,
                })
            }
            1 => {
                next.check = Some(CheckSummary {
                    plan_id: "/".into(),
                    rule_id: "search".into(),
                    actor_id: "player".into(),
                    expression: "2d6".into(),
                    modifier_total: 0,
                    status: CheckStatus::Waiting,
                    mode: DiceMode::Manual,
                })
            }
            2 => {
                next.resume_required = true;
                next.checkpoint = Some(Checkpoint {
                    source_round_id: "bad".into(),
                    through_seq: 1,
                    stage: CheckpointStage::Check,
                });
            }
            3 => {
                next.last_operation = Some(Operation {
                    operation_id: "bad".into(),
                    round_id: None,
                    outcome: OperationOutcome::Accepted,
                    error: None,
                })
            }
            4 => {
                next.scene = Some(ScenePosition {
                    scene_id: "room".into(),
                    path: vec![],
                })
            }
            5 => next.history_revision = format::MAX_SEQ + 1,
            _ => unreachable!(),
        }
        assert!(publisher.prepare(next, None).is_err());
    }
    assert!(events.seq.iter().all(|seq| seq.load(Ordering::SeqCst) == 0));
    assert!(validate_error("no-domain", "failure").is_err());
    assert!(validate_error("app.event-failed", &"x".repeat(513)).is_err());
    assert_eq!(
        encode_error(aoidos_json::Error::InvalidEncoding).code,
        "app.bad-request"
    );
    events.seq[0].store(format::MAX_SEQ, Ordering::SeqCst);
    assert_eq!(
        publisher
            .prepare(publisher.snapshot().state, None)
            .err()
            .unwrap()
            .code,
        "app.event-failed"
    );
}

#[test]
fn oversized_foreign_operation_terminal_payload_is_rejected_before_any_reservation() {
    let events = Arc::new(Events::default());
    let publisher = Publisher::new(state(), events.clone()).unwrap();
    let result = publisher.prepare(
        publisher.snapshot().state,
        Some(Finished {
            operation_id: uuid::Uuid::new_v4().to_string(),
            outcome: Outcome::Failed,
            error: Some(Fault::new("app.event-failed", "x".repeat(MAX_EVENT_BYTES))),
        }),
    );
    assert_eq!(result.err().unwrap().code, "app.bad-request");
    assert!(events.seq.iter().all(|seq| seq.load(Ordering::SeqCst) == 0));
}
