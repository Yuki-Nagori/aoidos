use super::test_lock as lock;
use super::*;
use crate::game::test_support::Fixture;
use crate::record::facts::DiceMode;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn no_check_and_ooc_pipeline_share_real_records_and_completed_hook() {
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    let mut runner = fixture.accept("查看房间", DiceMode::Manual).await;
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
    assert!(fixture.context.coordinator.acquire().is_err());
    fixture.complete(&mut runner).await;
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.consequences.load(Ordering::SeqCst), 1);
    assert_eq!(lock(&fixture.completed.0).len(), 1);
    assert_eq!(
        fixture
            .context
            .publisher
            .snapshot()
            .state
            .last_operation
            .unwrap()
            .outcome,
        OperationOutcome::Completed
    );
    assert_eq!(lock(&fixture.context.session).in_flight(), None);
    drop(runner);
    let mut ooc = fixture.accept("/ooc 解释规则", DiceMode::Auto).await;
    fixture.complete(&mut ooc).await;
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 3);
    assert_eq!(fixture.consequences.load(Ordering::SeqCst), 1);
    assert_eq!(lock(&fixture.completed.0).len(), 2);
}

#[tokio::test]
async fn waiting_keeps_lease_duplicate_check_does_not_roll_twice_and_resume_reuses_dice() {
    let fixture = Fixture::new(true, "{}");
    let mut runner = fixture.accept("搜索", DiceMode::Manual).await;
    runner.step(CancellationToken::new()).await.unwrap();
    assert_eq!(runner.stage(), Stage::Waiting);
    assert!(fixture.context.coordinator.acquire().is_err());
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        runner.submit_check("wrong").unwrap_err().code,
        "app.bad-request"
    );
    let plan = runner.plan_id().unwrap().to_owned();
    runner.submit_check(&plan).unwrap();
    runner.submit_check(&plan).unwrap();
    runner.step(CancellationToken::new()).await.unwrap();
    let dice_seq = runner.state.dice_seq.unwrap();
    runner.finish(Outcome::Cancelled, None).await.unwrap();
    drop(runner);
    let snapshot = fixture.context.publisher.snapshot();
    assert_eq!(snapshot.state.phase, Phase::Settling);
    assert_eq!(snapshot.state.checkpoint.unwrap().through_seq, dice_seq);
    let mut resumed = Runner::resume(
        fixture.context.clone(),
        fixture.context.coordinator.acquire().unwrap(),
        fixture.frozen(DiceMode::Auto),
    )
    .await
    .unwrap();
    assert_eq!(resumed.stage(), Stage::CheckCommit);
    fixture.complete(&mut resumed).await;
    let session = lock(&fixture.context.session);
    let dice_count = session
        .path_at(session.history_revision())
        .unwrap()
        .into_iter()
        .filter(|seq| {
            matches!(
                session.read(*seq).unwrap().body.unwrap().body,
                Body::Dice { .. }
            )
        })
        .count();
    assert_eq!(dice_count, 1);
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalid_proposal_fails_without_public_text_and_does_not_notify_completed() {
    let fixture = Fixture::new(
        false,
        "{\"kind\":\"check\",\"ruleId\":\"unknown\",\"actorId\":\"player\",\"reason\":\"需要检定\"}",
    );
    let mut runner = fixture.accept("搜索", DiceMode::Manual).await;
    let error = runner.step(CancellationToken::new()).await.unwrap_err();
    assert_eq!(error.code, "llm.bad-response");
    runner.finish(Outcome::Failed, Some(error)).await.unwrap();
    assert!(lock(&fixture.completed.0).is_empty());
    assert!(runner.turn_id().is_none());
    assert_eq!(
        fixture
            .context
            .publisher
            .snapshot()
            .state
            .last_operation
            .unwrap()
            .outcome,
        OperationOutcome::Failed
    );
}

#[tokio::test]
async fn already_cancelled_effect_does_not_prepare_or_start_a_paid_child() {
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    let mut runner = fixture.accept("查看房间", DiceMode::Auto).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        runner.step(cancel).await.unwrap_err().code,
        "engine.interrupted"
    );
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
    runner.finish(Outcome::Cancelled, None).await.unwrap();
}

#[tokio::test]
async fn reopening_manual_waiting_reconstructs_plan_without_rng_or_generation() {
    let fixture = Fixture::new(true, "{}");
    let mut run = fixture.accept("搜索", DiceMode::Manual).await;
    run.step(CancellationToken::new()).await.unwrap();
    let plan = run.plan_id().unwrap().to_owned();
    let epoch = fixture.context.publisher.snapshot().state.state_epoch;
    drop(run);
    let reopened = fixture.reopen().await;
    let snapshot = reopened.publisher.snapshot();
    assert_ne!(snapshot.state.state_epoch, epoch);
    assert_eq!(snapshot.state.phase, Phase::AwaitingCheck);
    assert!(snapshot.state.resume_required);
    assert!(snapshot.state.in_flight.is_none());
    assert_eq!(snapshot.state.check.as_ref().unwrap().plan_id, plan);
    assert_eq!(snapshot.state.check.unwrap().status, CheckStatus::Waiting);
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
    assert!(lock(&fixture.completed.0).is_empty());
    let mut resumed = Runner::resume(
        reopened.clone(),
        reopened.coordinator.acquire().unwrap(),
        fixture.frozen(DiceMode::Auto),
    )
    .await
    .unwrap();
    fixture.complete(&mut resumed).await;
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reopening_completed_round_replays_empty_settlement_without_new_generation_or_hook() {
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    let mut run = fixture.accept("查看房间", DiceMode::Auto).await;
    fixture.complete(&mut run).await;
    drop(run);
    let reopened = fixture.reopen().await;
    let snapshot = reopened.publisher.snapshot();
    assert_eq!(snapshot.state.phase, Phase::Idle);
    assert!(!snapshot.state.needs_recovery);
    assert!(!snapshot.state.resume_required);
    assert_eq!(
        snapshot.state.last_operation.unwrap().outcome,
        OperationOutcome::Completed
    );
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 2);
    assert_eq!(lock(&fixture.completed.0).len(), 1);
}

#[tokio::test]
async fn cancelled_waiting_has_same_plan_and_phase_after_reopen() {
    let fixture = Fixture::new(true, "{}");
    let mut run = fixture.accept("搜索", DiceMode::Manual).await;
    run.step(CancellationToken::new()).await.unwrap();
    run.finish(Outcome::Cancelled, None).await.unwrap();
    let before = fixture.context.publisher.snapshot().state;
    assert_eq!(before.phase, Phase::AwaitingCheck);
    assert_eq!(before.check.as_ref().unwrap().status, CheckStatus::Waiting);
    drop(run);
    let after = fixture.reopen().await.publisher.snapshot().state;
    assert_eq!(before.phase, after.phase);
    assert_eq!(before.check, after.check);
    assert_eq!(before.checkpoint, after.checkpoint);
}

#[tokio::test]
async fn accepted_delivery_failure_keeps_identity_but_starts_no_request() {
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    fixture.events.fail.store(true, Ordering::SeqCst);
    let mut run = fixture.accept("查看房间", DiceMode::Auto).await;
    assert_eq!(
        fixture
            .context
            .publisher
            .snapshot()
            .state
            .last_operation
            .unwrap()
            .operation_id,
        run.identity.operation_id
    );
    assert_eq!(
        run.step(CancellationToken::new()).await.unwrap_err().code,
        "app.event-failed"
    );
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
    fixture.events.fail.store(false, Ordering::SeqCst);
    run.finish(
        Outcome::Failed,
        Some(Fault::new("app.event-failed", "事件投递失败")),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn public_identity_delivery_failure_prevents_provider_start() {
    let fixture = Fixture::new(false, "{}");
    let mut run = fixture.accept("/ooc 解释规则", DiceMode::Auto).await;
    while run.stage() != Stage::Narration {
        run.step(CancellationToken::new()).await.unwrap();
    }
    fixture.events.fail_public.store(true, Ordering::SeqCst);
    assert_eq!(
        run.step(CancellationToken::new()).await.unwrap_err().code,
        "app.event-failed"
    );
    let snapshot = fixture
        .context
        .coordinator
        .snapshot(run.turn_id().unwrap())
        .unwrap();
    assert_eq!(snapshot.text, "");
    assert_eq!(snapshot.outcome, Some(Outcome::Failed));
    assert_eq!(snapshot.error.unwrap().code, "app.event-failed");
    fixture.events.fail_public.store(false, Ordering::SeqCst);
    run.finish(
        Outcome::Failed,
        Some(Fault::new("app.event-failed", "事件投递失败")),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn sealed_body_delivery_failure_preserves_completed_child_and_narration_audit() {
    let fixture = Fixture::new(false, "{}");
    let mut run = fixture.accept("/ooc 解释规则", DiceMode::Auto).await;
    while run.stage() != Stage::Narration {
        run.step(CancellationToken::new()).await.unwrap();
    }
    fixture.events.fail_settling.store(true, Ordering::SeqCst);
    let error = run.step(CancellationToken::new()).await.unwrap_err();
    assert_eq!(error.code, "app.event-failed");
    assert_eq!(run.child_outcome(), Some(Outcome::Completed));
    let flight = fixture
        .context
        .publisher
        .snapshot()
        .state
        .in_flight
        .unwrap();
    assert_eq!(
        fixture
            .context
            .coordinator
            .snapshot(&flight.turn_id.unwrap())
            .unwrap()
            .outcome,
        Some(Outcome::Completed)
    );
    let narrative = run.state.narrative_seq.unwrap();
    assert!(matches!(
        lock(&fixture.context.session)
            .read(narrative)
            .unwrap()
            .body
            .unwrap()
            .body,
        Body::Narration { .. }
    ));
    run.finish(Outcome::Failed, Some(error)).await.unwrap();
    let phase = fixture.context.publisher.snapshot().state;
    assert_eq!(
        phase.checkpoint.unwrap().stage,
        crate::game::state::CheckpointStage::Settle
    );
    let session = lock(&fixture.context.session);
    let Body::System { code, data, .. } = session
        .read(session.next_sequence() - 1)
        .unwrap()
        .body
        .unwrap()
        .body
    else {
        panic!("round end");
    };
    let Fact::RoundEnded {
        completed_steps, ..
    } = Fact::decode(&code, &data).unwrap()
    else {
        panic!("round end");
    };
    assert!(
        completed_steps
            .iter()
            .any(|step| matches!(step, Step::Narration))
    );
}

struct ScenarioDomain {
    base: crate::game::test_support::TestDomain,
    applied: std::collections::BTreeMap<String, String>,
    fail_once: bool,
    exit: bool,
    no_baseline: bool,
    fail_history: bool,
    external_revision: Arc<std::sync::atomic::AtomicUsize>,
}
impl crate::record::world::WorldPort for ScenarioDomain {
    fn registered(&self, kind: &str, version: u32) -> bool {
        version == 1 && matches!(kind, "fixtureEffect" | "sceneAdvanced" | "sessionEnded")
    }
    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        match self.applied.get(id) {
            None => Ok(false),
            Some(saved) if saved == hash => Ok(true),
            Some(_) => Err(Fault::new("store.corrupt", "夹具操作 hash 冲突")),
        }
    }
    fn apply(
        &mut self,
        mutation: &crate::record::facts::WorldMutation,
        hash: &str,
    ) -> Result<(), Fault> {
        if self.fail_once && self.applied.len() == 1 {
            self.fail_once = false;
            return Err(Fault::new("store.database", "夹具第二项提交失败"));
        }
        self.applied
            .insert(mutation.mutation_id.clone(), hash.into());
        Ok(())
    }
}
impl crate::record::history::HistoryPort for ScenarioDomain {
    fn verify_target(&self, session: &Session, target: u64) -> Result<(), Fault> {
        self.base.verify_target(session, target)
    }
    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        crate::record::world::WorldPort::applied(self, id, hash)
    }
    fn rebuild(
        &mut self,
        session: &Session,
        path: &[u64],
        id: &str,
        hash: &str,
    ) -> Result<(), Fault> {
        if self.fail_history {
            return Err(Fault::new("store.database", "夹具历史重建失败"));
        }
        self.base.rebuild(session, path, id, hash)
    }
}
impl super::super::domain::Domain for ScenarioDomain {
    fn catalog(&self) -> &crate::game::catalog::SceneCatalog {
        self.base.catalog()
    }
    fn baseline_scene(&self) -> Option<crate::record::facts::ScenePosition> {
        if self.no_baseline {
            None
        } else {
            self.base.baseline_scene()
        }
    }
    fn confirmed(
        &self,
        session: &Session,
    ) -> Result<crate::game::catalog::ConfirmedWorldView, Fault> {
        crate::game::catalog::ConfirmedWorldView::capture(
            session,
            &(self.applied.len() + self.external_revision.load(Ordering::SeqCst)).to_string(),
            std::collections::BTreeMap::new(),
        )
    }
    fn context(&self, view: &crate::game::catalog::ConfirmedWorldView) -> Result<String, Fault> {
        self.base.context(view)
    }
    fn check_plan(
        &self,
        rule: &str,
        actor: &str,
        view: &crate::game::catalog::ConfirmedWorldView,
    ) -> Result<CheckPlan, Fault> {
        self.base.check_plan(rule, actor, view)
    }
    fn consequences(
        &self,
        session: &Session,
        round: &crate::game::recovery::RoundFacts,
        view: &crate::game::catalog::ConfirmedWorldView,
    ) -> Result<Vec<crate::record::facts::WorldMutation>, Fault> {
        self.base.consequences(session, round, view)?;
        Ok((0..2)
            .map(|_| crate::record::facts::WorldMutation {
                mutation_id: uuid::Uuid::new_v4().to_string(),
                data: crate::record::facts::MutationData {
                    kind: "fixtureEffect".into(),
                    version: 1,
                    payload: serde_json::json!({"fixture":true}),
                },
            })
            .collect())
    }
    fn exit_reason(
        &self,
        _: &str,
        _: &crate::game::catalog::ConfirmedWorldView,
    ) -> Result<Option<String>, Fault> {
        Ok(self.exit.then(|| "已登记夹具出口".into()))
    }
}
fn scenario(
    fixture: &Fixture,
    advance: bool,
    exit: bool,
    fail_once: bool,
) -> Arc<std::sync::atomic::AtomicUsize> {
    use crate::game::catalog::*;
    use crate::record::facts::{SceneNode, ScenePosition};
    use std::collections::BTreeMap;
    let header = lock(&fixture.context.session).header().clone();
    let mut base = crate::game::test_support::domain(
        &header.script_revision,
        true,
        fixture.consequences.clone(),
    );
    if advance {
        let mut root = base.catalog.scene("room").unwrap().clone();
        root.rules.advance_rule_ids = vec!["advance".into()];
        root.advance = vec![Advance {
            rule_id: "advance".into(),
            target_scene_id: "hall".into(),
            priority: 1,
        }];
        let hall_position = ScenePosition {
            scene_id: "hall".into(),
            path: vec![SceneNode {
                id: "hall".into(),
                kind: "scene".into(),
                title: "走廊".into(),
            }],
        };
        let mut hall = root.clone();
        hall.position = hall_position;
        hall.rules.scene_id = "hall".into();
        hall.rules.advance_rule_ids.clear();
        hall.advance.clear();
        base.catalog = SceneCatalog::new(
            "1".into(),
            header.script_revision,
            BTreeMap::from([
                (
                    "room".into(),
                    Node {
                        kind: "scene".into(),
                        title: "房间".into(),
                        parent: None,
                    },
                ),
                (
                    "hall".into(),
                    Node {
                        kind: "scene".into(),
                        title: "走廊".into(),
                        parent: None,
                    },
                ),
            ]),
            BTreeMap::from([("room".into(), root), ("hall".into(), hall)]),
            BTreeMap::from([("advance".into(), Condition::Always {})]),
        )
        .unwrap();
    }
    let external_revision = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    *lock(&fixture.context.domain) = Box::new(ScenarioDomain {
        base,
        applied: BTreeMap::new(),
        fail_once,
        exit,
        no_baseline: false,
        fail_history: false,
        external_revision: external_revision.clone(),
    });
    external_revision
}

#[tokio::test]
async fn completed_world_effects_then_private_scene_choice_advance_or_stay_and_exit_are_confirmed()
{
    for (proposal, advance, exit, expected) in [
        (
            "{\"kind\":\"scene\",\"sceneId\":\"hall\"}",
            true,
            false,
            Some("hall"),
        ),
        ("{\"kind\":\"stay\"}", true, false, Some("room")),
        ("{}", false, true, None),
    ] {
        let fixture = Fixture::new(true, proposal);
        scenario(&fixture, advance, exit, false);
        let mut run = fixture.accept("搜索", DiceMode::Auto).await;
        fixture.complete(&mut run).await;
        let snapshot = fixture.context.publisher.snapshot().state;
        assert_eq!(
            snapshot.scene.as_ref().map(|scene| scene.scene_id.as_str()),
            expected
        );
        assert_eq!(fixture.consequences.load(Ordering::SeqCst), 1);
        assert_eq!(lock(&fixture.completed.0).len(), 1);
        assert!(!lock(&fixture.context.session).needs_recovery());
    }
}

#[tokio::test]
async fn partial_world_commit_replays_fixed_intents_then_resume_only_missing_steps() {
    let fixture = Fixture::new(true, "{}");
    scenario(&fixture, false, false, true);
    let mut run = fixture.accept("搜索", DiceMode::Auto).await;
    while run.stage() != Stage::Settlement {
        run.step(CancellationToken::new()).await.unwrap();
    }
    let error = run.step(CancellationToken::new()).await.unwrap_err();
    assert_eq!(error.code, "store.database");
    assert!(lock(&fixture.context.session).needs_recovery());
    assert!(
        crate::game::catalog::ConfirmedWorldView::capture(
            &lock(&fixture.context.session),
            "0",
            std::collections::BTreeMap::new()
        )
        .is_err()
    );
    assert!(run.finish(Outcome::Failed, Some(error)).await.is_err());
    drop(run);
    let old = fixture.context.clone();
    let header = lock(&old.session).header().clone();
    let domain = std::mem::replace(
        &mut *lock(&old.domain),
        Box::new(crate::game::test_support::domain(
            &header.script_revision,
            true,
            fixture.consequences.clone(),
        )),
    );
    old.publisher.retire();
    let context = Context::open(
        old.session.clone(),
        domain,
        "player".into(),
        "player".into(),
        old.coordinator.clone(),
        fixture.events.clone(),
        fixture.completed.clone(),
    )
    .await
    .unwrap();
    assert!(!context.publisher.snapshot().state.needs_recovery);
    let mut resumed = Runner::resume(
        context.clone(),
        context.coordinator.acquire().unwrap(),
        fixture.frozen(DiceMode::Auto),
    )
    .await
    .unwrap();
    fixture.complete(&mut resumed).await;
    assert_eq!(fixture.consequences.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 1);
    assert_eq!(lock(&fixture.completed.0).len(), 1);
}

#[tokio::test]
async fn trusted_private_check_selects_only_registered_rules_and_actors() {
    let fixture = Fixture::new(
        false,
        "{\"kind\":\"check\",\"ruleId\":\"search\",\"actorId\":\"player\",\"reason\":\"搜索需要判定\"}",
    );
    let mut run = fixture.accept("搜索", DiceMode::Auto).await;
    fixture.complete(&mut run).await;
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 2);
    assert!(run.state.check_seq.is_some());
    assert_eq!(lock(&fixture.completed.0).len(), 1);
}

#[tokio::test]
async fn unfinished_acceptance_is_sealed_on_open_without_generating_or_reusing_its_owner() {
    let fixture = Fixture::new(false, "{}");
    let run = fixture.accept("查看", DiceMode::Auto).await;
    let round = run.identity.round_id.clone();
    drop(run);
    let context = fixture.reopen().await;
    let snapshot = context.publisher.snapshot().state;
    assert_eq!(snapshot.phase, Phase::Idle);
    assert!(!snapshot.resume_required);
    assert_eq!(snapshot.last_operation.unwrap().round_id, Some(round));
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn scene_proposal_outside_confirmed_candidates_does_not_commit_advance() {
    let fixture = Fixture::new(true, "{\"kind\":\"scene\",\"sceneId\":\"unknown\"}");
    scenario(&fixture, true, false, false);
    let mut run = fixture.accept("搜索", DiceMode::Auto).await;
    while run.stage() != Stage::SceneProposal {
        run.step(CancellationToken::new()).await.unwrap();
    }
    let error = run.step(CancellationToken::new()).await.unwrap_err();
    assert_eq!(error.code, "llm.bad-response");
    run.finish(Outcome::Failed, Some(error)).await.unwrap();
    let snapshot = fixture.context.publisher.snapshot().state;
    assert_eq!(snapshot.phase, Phase::Advancing);
    assert_eq!(snapshot.scene.unwrap().scene_id, "room");
    assert!(lock(&fixture.completed.0).is_empty());
}

#[tokio::test]
async fn fresh_input_abandons_the_paused_checkpoint_without_resuming_its_plan() {
    let fixture = Fixture::new(true, "{}");
    let mut old = fixture.accept("搜索", DiceMode::Manual).await;
    old.step(CancellationToken::new()).await.unwrap();
    old.finish(Outcome::Cancelled, None).await.unwrap();
    drop(old);
    let fresh = fixture.accept("新的行动", DiceMode::Manual).await;
    assert!(fresh.recovered.round.as_ref().unwrap().dice.is_none());
    let session = lock(&fixture.context.session);
    assert!(session.path_at(session.history_revision()).unwrap().iter().any(|seq|matches!(session.read(*seq).unwrap().body.unwrap().body,Body::System {code,..} if code=="abandonCheckpoint")));
}

#[tokio::test]
async fn malformed_driver_receipts_are_rejected_before_any_new_record() {
    let fixture = Fixture::new(true, "{}");
    let mut run = fixture.accept("搜索", DiceMode::Auto).await;
    assert_eq!(
        run.finish(Outcome::Completed, None).await.unwrap_err().code,
        "app.bad-request"
    );
    assert_eq!(
        run.finish(Outcome::Failed, None).await.unwrap_err().code,
        "app.bad-request"
    );
    assert_eq!(
        run.commit(Action::CommitInput).await.unwrap_err().code,
        "engine.invalid-phase"
    );
    while run.stage() != Stage::CheckCommit {
        run.step(CancellationToken::new()).await.unwrap();
    }
    let before = lock(&fixture.context.session).next_sequence();
    run.state.plan.as_mut().unwrap().plan_id = "foreign-plan".into();
    assert_eq!(
        run.step(CancellationToken::new()).await.unwrap_err().code,
        "engine.invalid-phase"
    );
    assert_eq!(lock(&fixture.context.session).next_sequence(), before);
    run.state.current = None;
    assert!(run.step(CancellationToken::new()).await.is_err());
    assert!(complete_effect(&run.state, Receipt::Ended).is_err());
}

#[tokio::test]
async fn invalid_actor_and_player_registration_never_replace_the_current_owner() {
    let fixture = Fixture::new(true, "{}");
    let header = lock(&fixture.context.session).header().clone();
    let result = Context::open(
        fixture.context.session.clone(),
        Box::new(crate::game::test_support::domain(
            &header.script_revision,
            true,
            fixture.consequences.clone(),
        )),
        "/".into(),
        "player".into(),
        fixture.context.coordinator.clone(),
        fixture.events.clone(),
        fixture.completed.clone(),
    )
    .await;
    assert_eq!(result.err().unwrap().code, "app.bad-request");
    let context = Arc::new(Context {
        actor_id: "foreign".into(),
        player_id: "player".into(),
        session: fixture.context.session.clone(),
        domain: fixture.context.domain.clone(),
        publisher: fixture.context.publisher.clone(),
        coordinator: fixture.context.coordinator.clone(),
        completed: fixture.completed.clone(),
    });
    let result = Runner::accept(
        context,
        fixture.context.coordinator.acquire().unwrap(),
        fixture.frozen(DiceMode::Auto),
        super::super::input::parse_input("搜索").unwrap(),
    )
    .await;
    assert_eq!(result.err().unwrap().code, "engine.invalid-phase");
    assert_eq!(lock(&fixture.context.session).next_sequence(), 1);
}

#[test]
fn round_acceptance_reserves_room_for_terminal_state_and_all_four_streams() {
    let publisher = Publisher::new(
        crate::game::state::PhaseState {
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
        },
        Arc::new(crate::game::test_support::PhaseEventsFixture::default()),
    )
    .unwrap();
    let mut snapshot = publisher.snapshot();
    snapshot.state.phase_revision = crate::record::format::MAX_SEQ;
    assert!(reserve_round(&snapshot).is_err());
    snapshot.state.phase_revision = 0;
    snapshot.seq.operation_failed = crate::record::format::MAX_SEQ;
    assert!(reserve_round(&snapshot).is_err());
    assert_eq!(
        projection_error(crate::record::projection::ProjectionError::InvalidConfig).code,
        "app.bad-request"
    );
}

#[tokio::test]
async fn context_without_a_scene_baseline_opens_only_as_an_idle_read_view() {
    let fixture = Fixture::new(true, "{}");
    let header = lock(&fixture.context.session).header().clone();
    let domain = ScenarioDomain {
        base: crate::game::test_support::domain(
            &header.script_revision,
            true,
            fixture.consequences.clone(),
        ),
        applied: std::collections::BTreeMap::new(),
        exit: false,
        fail_once: false,
        no_baseline: true,
        fail_history: false,
        external_revision: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let context = Context::open(
        fixture.context.session.clone(),
        Box::new(domain),
        "player".into(),
        "player".into(),
        fixture.context.coordinator.clone(),
        fixture.events.clone(),
        fixture.completed.clone(),
    )
    .await
    .unwrap();
    assert!(context.publisher.snapshot().state.scene.is_none());
    let result = Runner::accept(
        context.clone(),
        context.coordinator.acquire().unwrap(),
        fixture.frozen(DiceMode::Auto),
        crate::game::input::parse_input("搜索").unwrap(),
    )
    .await;
    assert_eq!(result.err().unwrap().code, "engine.no-scene");
    assert_eq!(lock(&context.session).next_sequence(), 1);
}

#[tokio::test]
async fn failed_history_rebuild_on_open_keeps_pending_without_a_fee_or_checkpoint() {
    use crate::record::facts::ForkMode;
    let fixture = Fixture::new(true, "{}");
    let mut run = fixture.accept("搜索", DiceMode::Manual).await;
    run.step(CancellationToken::new()).await.unwrap();
    run.finish(Outcome::Cancelled, None).await.unwrap();
    drop(run);
    let header = lock(&fixture.context.session).header().clone();
    let mut domain = ScenarioDomain {
        base: crate::game::test_support::domain(
            &header.script_revision,
            true,
            fixture.consequences.clone(),
        ),
        applied: std::collections::BTreeMap::new(),
        exit: false,
        fail_once: false,
        no_baseline: false,
        fail_history: true,
        external_revision: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    {
        let mut session = lock(&fixture.context.session);
        let target = fixture
            .context
            .publisher
            .snapshot()
            .state
            .checkpoint
            .unwrap()
            .through_seq;
        let body = crate::game::control::prepare_fork(
            &session,
            &uuid::Uuid::new_v4().to_string(),
            ForkMode::Rewind,
            target,
            None,
        )
        .unwrap();
        assert_eq!(
            session
                .commit_history_fork(body, &mut domain)
                .unwrap_err()
                .code,
            "store.database"
        );
    }
    let context = Context::open(
        fixture.context.session.clone(),
        Box::new(domain),
        "player".into(),
        "player".into(),
        fixture.context.coordinator.clone(),
        fixture.events.clone(),
        fixture.completed.clone(),
    )
    .await
    .unwrap();
    let phase = context.publisher.snapshot().state;
    assert!(phase.needs_recovery);
    assert!(!phase.resume_required);
    assert!(phase.checkpoint.is_none());
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn private_transport_failure_preserves_its_real_outcome_without_public_narration() {
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    fixture.generation.fail_auth.store(true, Ordering::SeqCst);
    let mut run = fixture.accept("查看", DiceMode::Auto).await;
    let error = run.step(CancellationToken::new()).await.unwrap_err();
    assert_eq!(error.code, "llm.auth");
    assert_eq!(run.child_outcome(), Some(Outcome::Failed));
    assert!(run.turn_id().is_none());
    run.finish(Outcome::Failed, Some(error)).await.unwrap();
    assert!(lock(&fixture.completed.0).is_empty());
}

#[tokio::test]
async fn an_unfinished_unresumable_acceptance_cannot_be_overwritten_by_new_input() {
    let fixture = Fixture::new(false, "{}");
    let run = fixture.accept("原输入", DiceMode::Auto).await;
    drop(run);
    let before = lock(&fixture.context.session).next_sequence();
    let result = Runner::accept(
        fixture.context.clone(),
        fixture.context.coordinator.acquire().unwrap(),
        fixture.frozen(DiceMode::Auto),
        crate::game::input::parse_input("新输入").unwrap(),
    )
    .await;
    assert_eq!(result.err().unwrap().code, "engine.invalid-phase");
    assert_eq!(lock(&fixture.context.session).next_sequence(), before);
}

#[tokio::test]
async fn malformed_effect_and_persisted_reference_kinds_fail_before_commit() {
    let fixture = Fixture::new(true, "{}");
    let mut run = fixture.accept("搜索", DiceMode::Auto).await;
    let action = run.state.current.as_ref().unwrap().action;
    run.state.current.as_mut().unwrap().action = Action::CommitInput;
    assert_eq!(
        run.step(CancellationToken::new()).await.unwrap_err().code,
        "engine.invalid-phase"
    );
    run.state.current.as_mut().unwrap().action = action;
    while run.stage() != Stage::CheckCommit {
        run.step(CancellationToken::new()).await.unwrap();
    }
    let real_dice = run.state.dice_seq;
    run.state.dice_seq = Some(run.recovered.round.as_ref().unwrap().input_seq);
    assert_eq!(
        run.step(CancellationToken::new()).await.unwrap_err().code,
        "engine.invalid-phase"
    );
    run.state.dice_seq = real_dice;
    while run.stage() != Stage::Settlement {
        run.step(CancellationToken::new()).await.unwrap();
    }
    let before = lock(&fixture.context.session).next_sequence();
    for seq in [
        run.recovered.round.as_ref().unwrap().input_seq,
        run.recovered
            .round
            .as_ref()
            .unwrap()
            .plan
            .as_ref()
            .unwrap()
            .0,
    ] {
        run.recovered.round.as_mut().unwrap().settlement = Some(seq);
        assert_eq!(
            run.step(CancellationToken::new()).await.unwrap_err().code,
            "engine.invalid-phase"
        );
        assert_eq!(lock(&fixture.context.session).next_sequence(), before);
    }
}

#[tokio::test]
async fn terminal_prepare_failure_does_not_append_a_round_end() {
    let fixture = Fixture::new(true, "{}");
    let mut run = fixture.accept("搜索", DiceMode::Manual).await;
    run.step(CancellationToken::new()).await.unwrap();
    let before = lock(&fixture.context.session).next_sequence();
    fixture.events.fail_prepare.store(true, Ordering::SeqCst);
    assert_eq!(
        run.finish(Outcome::Cancelled, None).await.unwrap_err().code,
        "app.event-failed"
    );
    assert_eq!(lock(&fixture.context.session).next_sequence(), before);
    fixture.events.fail_prepare.store(false, Ordering::SeqCst);
}

struct SceneGenerationFault {
    inner: Arc<crate::game::test_support::TestGeneration>,
    revision: Arc<std::sync::atomic::AtomicUsize>,
    low_budget: bool,
}
impl crate::game::domain::Generation for SceneGenerationFault {
    fn shape(&self) -> projection::Shape {
        self.inner.shape()
    }
    fn projection_budget(&self) -> projection::Budget {
        let mut budget = self.inner.projection_budget();
        if self.low_budget {
            budget.context_limit = Some(1);
        }
        budget
    }
    fn prepare(
        &self,
        input: aoidos_llm::provider::ProviderInput,
        guard: aoidos_llm::guard::GuardSpec,
        automatic: bool,
    ) -> Result<PreparedGeneration, Fault> {
        if matches!(&input,aoidos_llm::provider::ProviderInput::Completion(prompt) if prompt.prompt.ends_with("[AOIDOS:SCENE-PROPOSAL]\n"))
        {
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.prepare(input, guard, automatic)
    }
}
#[tokio::test]
async fn scene_proposal_budget_failure_or_changed_world_refuses_the_advance() {
    for low_budget in [false, true] {
        let fixture = Fixture::new(true, "{\"kind\":\"scene\",\"sceneId\":\"hall\"}");
        let revision = scenario(&fixture, true, false, false);
        let mut run = fixture.accept("搜索", DiceMode::Auto).await;
        while run.stage() != Stage::SceneProposal {
            run.step(CancellationToken::new()).await.unwrap();
        }
        run.frozen.generation = Arc::new(SceneGenerationFault {
            inner: fixture.generation.clone(),
            revision,
            low_budget,
        });
        let calls = fixture.generation.calls.load(Ordering::SeqCst);
        let error = run.step(CancellationToken::new()).await.unwrap_err();
        assert_eq!(
            error.code,
            if low_budget {
                "app.bad-request"
            } else {
                "engine.invalid-phase"
            }
        );
        if low_budget {
            assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), calls);
        }
        assert_eq!(
            fixture
                .context
                .publisher
                .snapshot()
                .state
                .scene
                .unwrap()
                .scene_id,
            "room"
        );
        assert!(lock(&fixture.completed.0).is_empty());
    }
}

#[tokio::test]
async fn dice_disabled_scene_skips_proposal_and_sampling_but_completes_normal_settlement() {
    use crate::game::catalog::*;
    use std::collections::BTreeMap;
    let fixture = Fixture::new(true, "{}");
    let header = lock(&fixture.context.session).header().clone();
    let mut domain = crate::game::test_support::domain(
        &header.script_revision,
        true,
        fixture.consequences.clone(),
    );
    let mut room = domain.catalog.scene("room").unwrap().clone();
    room.dice_disabled = true;
    room.forced_check = None;
    domain.catalog = SceneCatalog::new(
        "1".into(),
        header.script_revision,
        BTreeMap::from([(
            "room".into(),
            Node {
                kind: "scene".into(),
                title: "房间".into(),
                parent: None,
            },
        )]),
        BTreeMap::from([("room".into(), room)]),
        BTreeMap::new(),
    )
    .unwrap();
    *lock(&fixture.context.domain) = Box::new(domain);
    let mut run = fixture.accept("查看", DiceMode::Auto).await;
    fixture.complete(&mut run).await;
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 1);
    assert!(run.state.dice_seq.is_none());
    assert_eq!(fixture.consequences.load(Ordering::SeqCst), 1);
    let session = lock(&fixture.context.session);
    let skipped = session
        .read(run.state.skip_seq.unwrap())
        .unwrap()
        .body
        .unwrap()
        .body;
    let Body::System { code, data, .. } = skipped else {
        panic!("skip");
    };
    assert!(matches!(
        Fact::decode(&code, &data).unwrap(),
        Fact::CheckSkipped {
            reason: SkipReason::Disabled,
            ..
        }
    ));
}
