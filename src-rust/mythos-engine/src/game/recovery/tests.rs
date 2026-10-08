use super::*;
use crate::{
    game::dice::{self, PlanSpec},
    record::{
        facts::{GenerationTarget, SkipReason, Step, WorldMutation},
        format::InputMode,
        test_support::Fixture,
        world::WorldPort,
    },
};
use mythos_llm::schedule::FinishReason;

fn identity() -> RoundIdentity {
    RoundIdentity {
        version: 1,
        round_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    }
}
fn append(session: &mut Session, fact: Fact) -> u64 {
    let body = fact.body("确认事实").unwrap();
    if matches!(
        &body,
        Body::System{code,..} if matches!(code.as_str(),"settlementPlanned" | "sceneAdvanced" | "sessionEnded")
    ) {
        session.commit_world(body, &mut World).unwrap()
    } else {
        session.append(body).unwrap()
    }
}
fn begin(session: &mut Session, dice_mode: DiceMode) -> RoundIdentity {
    let input_seq = session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "搜索".into(),
            mode: Some(InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let id = identity();
    accept(session, id.clone(), input_seq, dice_mode, None);
    id
}
fn accept(
    session: &mut Session,
    identity: RoundIdentity,
    input_seq: u64,
    dice_mode: DiceMode,
    source: Option<(String, u64)>,
) {
    append(
        session,
        Fact::RoundAccepted {
            identity,
            input_seq,
            mode: InputMode::InCharacter,
            dice_mode,
            profile_id: "default".into(),
            profile_revision: "1".into(),
            target: GenerationTarget::Narration,
            scene_id: "room".into(),
            source_round_id: source.as_ref().map(|(id, _)| id.clone()),
            checkpoint_seq: source.map(|(_, seq)| seq),
        },
    );
}
fn plan(session: &mut Session, id: &RoundIdentity) -> (u64, CheckPlan) {
    let plan = dice::freeze_pbta(PlanSpec {
        rule_id: "search",
        actor_id: "player",
        modifiers: vec![],
        branches: ["s".into(), "c".into(), "f".into()],
        world_revision: "1",
    })
    .unwrap();
    let seq = append(
        session,
        Fact::CheckPlanned {
            identity: id.clone(),
            dice_mode: DiceMode::Manual,
            plan: plan.clone(),
        },
    );
    (seq, plan)
}
fn skip(session: &mut Session, id: &RoundIdentity) -> u64 {
    append(
        session,
        Fact::CheckSkipped {
            identity: id.clone(),
            reason: SkipReason::NoCheck,
            world_revision: "1".into(),
        },
    )
}
fn narrative(session: &mut Session, terminal: Terminal) -> u64 {
    session
        .append(Body::Narration {
            text: "已提交前文".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal,
        })
        .unwrap()
}
fn assert_checkpoint(
    session: &Session,
    id: &RoundIdentity,
    phase: Phase,
    stage: CheckpointStage,
    seq: u64,
) {
    let state = derive(session, None).unwrap();
    assert_eq!(state.phase, phase);
    assert_eq!(
        state.checkpoint,
        Some(Checkpoint {
            source_round_id: id.round_id.clone(),
            through_seq: seq,
            stage
        })
    );
    assert!(state.close.is_none());
    use crate::game::{
        reducer::{Action, Stage, State},
        state::CheckpointStage,
    };
    let mut invalid = state.clone();
    invalid.checkpoint.as_mut().unwrap().through_seq += 1;
    assert!(
        State::resume(
            "new-owner".into(),
            uuid::Uuid::new_v4().to_string(),
            &invalid,
            DiceMode::Manual
        )
        .is_err()
    );
    if state.round.as_ref().unwrap().check.is_some() {
        let mut invalid = state.clone();
        invalid.checkpoint.as_mut().unwrap().stage = CheckpointStage::Check;
        invalid.checkpoint.as_mut().unwrap().through_seq =
            invalid.round.as_ref().unwrap().dice.unwrap();
        assert!(
            State::resume(
                "new-owner".into(),
                uuid::Uuid::new_v4().to_string(),
                &invalid,
                DiceMode::Manual
            )
            .is_err()
        );
    }
    for dice_mode in [DiceMode::Manual, DiceMode::Auto] {
        let resumed = State::resume(
            "new-owner".into(),
            uuid::Uuid::new_v4().to_string(),
            &state,
            dice_mode,
        )
        .unwrap();
        let round = state.round.as_ref().unwrap();
        let expected = match stage {
            CheckpointStage::Check if round.dice.is_some() => Some(Action::CommitCheck),
            CheckpointStage::Check if dice_mode == DiceMode::Auto => Some(Action::RollDice),
            CheckpointStage::Check => None,
            CheckpointStage::Narration => Some(Action::StartNarration),
            CheckpointStage::Settle => Some(Action::ApplySettlement),
            CheckpointStage::Advance if round.advance.is_some() => Some(Action::EndRound),
            CheckpointStage::Advance => Some(Action::SelectScene),
        };
        assert_eq!(
            resumed.current.as_ref().map(|effect| effect.action),
            expected
        );
        assert_eq!(resumed.dice_seq, round.dice);
        assert_eq!(resumed.check_seq, round.check);
        if expected.is_none() {
            assert_eq!(resumed.stage, Stage::Waiting);
        }
    }
}
#[test]
fn confirmed_steps_select_precise_anchors_without_repeating_dice_or_body() {
    let mut fixture = Fixture::new();
    let session = &mut fixture.session;
    let id = begin(session, DiceMode::Manual);
    let (planned, p) = plan(session, &id);
    assert_eq!(
        derive(session, None)
            .unwrap()
            .last_operation
            .unwrap()
            .outcome,
        OperationOutcome::Accepted
    );
    assert_checkpoint(
        session,
        &id,
        Phase::AwaitingCheck,
        CheckpointStage::Check,
        planned,
    );
    let dice_seq = session
        .append(dice::roll(&p, "1", [0; 32]).unwrap())
        .unwrap();
    assert_checkpoint(
        session,
        &id,
        Phase::Settling,
        CheckpointStage::Check,
        dice_seq,
    );
    let check_seq = session
        .append(Body::Check {
            dice_seq,
            dc: None,
            result: dice::classify(2),
            rule_id: p.rule_id.clone(),
            plan_id: p.plan_id.clone(),
        })
        .unwrap();
    assert_checkpoint(
        session,
        &id,
        Phase::Settling,
        CheckpointStage::Narration,
        check_seq,
    );
    narrative(session, Terminal::Cancelled);
    assert_checkpoint(
        session,
        &id,
        Phase::Settling,
        CheckpointStage::Narration,
        check_seq,
    );
    let body = narrative(
        session,
        Terminal::Completed {
            finish_reason: FinishReason::Length,
        },
    );
    assert_checkpoint(session, &id, Phase::Settling, CheckpointStage::Settle, body);
    let planned = append(
        session,
        Fact::SettlementPlanned {
            identity: id.clone(),
            narrative_seq: body,
            world_revision: "1".into(),
            mutations: vec![],
        },
    );
    assert_checkpoint(
        session,
        &id,
        Phase::Settling,
        CheckpointStage::Settle,
        planned,
    );
    let settled = append(
        session,
        Fact::RoundSettled {
            identity: id.clone(),
            narrative_seq: body,
            settlement_seq: planned,
            applied_through_mutation_id: None,
        },
    );
    assert_checkpoint(
        session,
        &id,
        Phase::Advancing,
        CheckpointStage::Advance,
        settled,
    );
    let advance = append(
        session,
        Fact::SceneStayed {
            identity: id.clone(),
            scene_id: "room".into(),
            world_revision: "1".into(),
        },
    );
    assert_checkpoint(
        session,
        &id,
        Phase::Advancing,
        CheckpointStage::Advance,
        advance,
    );
    append(
        session,
        Fact::RoundEnded {
            identity: id,
            outcome: Outcome::Completed,
            through_seq: advance,
            completed_steps: vec![
                Step::Check,
                Step::Narration,
                Step::Settlement,
                Step::Advance,
            ],
            error: None,
        },
    );
    let recovered = derive(session, None).unwrap();
    assert_eq!(recovered.phase, Phase::Idle);
    assert!(recovered.checkpoint.is_none());
    assert_eq!(
        recovered.last_operation.unwrap().outcome,
        super::super::state::OperationOutcome::Completed
    );
}
#[test]
fn cancelled_round_keeps_checkpoint_and_resume_inherits_steps_under_new_identity() {
    let mut fixture = Fixture::new();
    let session = &mut fixture.session;
    let id = begin(session, DiceMode::Manual);
    let (anchor, _) = plan(session, &id);
    append(
        session,
        Fact::RoundEnded {
            identity: id.clone(),
            outcome: Outcome::Cancelled,
            through_seq: anchor,
            completed_steps: vec![],
            error: None,
        },
    );
    assert_checkpoint(
        session,
        &id,
        Phase::AwaitingCheck,
        CheckpointStage::Check,
        anchor,
    );
    let input = derive(session, None).unwrap().round.unwrap().input_seq;
    let new = identity();
    accept(
        session,
        new.clone(),
        input,
        DiceMode::Manual,
        Some((id.round_id, anchor)),
    );
    assert_checkpoint(
        session,
        &new,
        Phase::AwaitingCheck,
        CheckpointStage::Check,
        anchor,
    );
    append(
        session,
        Fact::AbandonCheckpoint {
            identity: identity(),
            source_round_id: new.round_id,
            checkpoint_seq: anchor,
        },
    );
    let recovered = derive(session, None).unwrap();
    assert!(recovered.checkpoint.is_none());
    assert!(recovered.close.is_none());
}
#[test]
fn interrupted_acceptance_closes_once_and_failed_body_is_never_a_settlement_anchor() {
    let mut fixture = Fixture::new();
    let session = &mut fixture.session;
    let id = begin(session, DiceMode::Auto);
    skip(session, &id);
    narrative(
        session,
        Terminal::Failed {
            error: Fault::new("engine.interrupted", "中断"),
            finish_reason: None,
        },
    );
    let close = derive(session, None).unwrap().close.unwrap();
    append(session, close);
    let recovered = derive(session, None).unwrap();
    assert!(recovered.close.is_none());
    assert!(recovered.checkpoint.is_none());
    assert_eq!(
        recovered.last_operation.unwrap().error.unwrap().code,
        "engine.interrupted"
    );
    let mut empty = Fixture::new();
    assert!(derive(&empty.session, None).unwrap().round.is_none());
    begin(&mut empty.session, DiceMode::Auto);
    assert!(derive(&empty.session, None).unwrap().close.is_some());
}

struct World;
impl WorldPort for World {
    fn registered(&self, _: &str, _: u32) -> bool {
        true
    }
    fn applied(&self, _: &str, _: &str) -> Result<bool, Fault> {
        Ok(true)
    }
    fn apply(&mut self, _: &WorldMutation, _: &str) -> Result<(), Fault> {
        Ok(())
    }
}
#[test]
fn unopened_world_confirmation_blocks_all_checkpoint_derivation() {
    let mut fixture = Fixture::new();
    let id = begin(&mut fixture.session, DiceMode::Auto);
    skip(&mut fixture.session, &id);
    let body = narrative(
        &mut fixture.session,
        Terminal::Completed {
            finish_reason: FinishReason::Stop,
        },
    );
    append(
        &mut fixture.session,
        Fact::SettlementPlanned {
            identity: id,
            narrative_seq: body,
            world_revision: "1".into(),
            mutations: vec![],
        },
    );
    let mut reopened = Session::open(fixture.path.clone(), fixture.events.clone()).unwrap();
    let pending = derive(&reopened, None).unwrap();
    assert!(pending.needs_recovery);
    assert!(pending.checkpoint.is_none());
    reopened.recover_world(&mut World).unwrap();
    assert_eq!(
        derive(&reopened, None).unwrap().checkpoint.unwrap().stage,
        CheckpointStage::Settle
    );
}

#[test]
fn corrupt_incremental_causality_never_produces_a_recovery_checkpoint() {
    let mut fixture = Fixture::new();
    let id = begin(&mut fixture.session, DiceMode::Manual);
    let base = derive(&fixture.session, None).unwrap();
    let (_, p) = plan(&mut fixture.session, &id);
    let planned = derive(&fixture.session, None).unwrap();
    let dice = dice::roll(&p, "1", [0; 32]).unwrap();
    let mut rolled = planned.clone();
    apply_record(&mut rolled, 10, dice.clone()).unwrap();
    for (mut state, body) in [
        (base.clone(), dice),
        (
            planned.clone(),
            Body::Check {
                dice_seq: 999,
                plan_id: p.plan_id.clone(),
                result: crate::record::format::CheckResult::Failure,
                dc: None,
                rule_id: p.rule_id.clone(),
            },
        ),
        (
            base.clone(),
            Body::Narration {
                text: "越过判定".into(),
                turn_id: uuid::Uuid::new_v4().to_string(),
                terminal: Terminal::Completed {
                    finish_reason: FinishReason::Stop,
                },
            },
        ),
        (
            planned.clone(),
            Fact::CheckSkipped {
                identity: id.clone(),
                reason: SkipReason::NoCheck,
                world_revision: "1".into(),
            }
            .body("不允许跳过已有计划")
            .unwrap(),
        ),
        (
            planned.clone(),
            Fact::CheckPlanned {
                identity: id.clone(),
                dice_mode: DiceMode::Manual,
                plan: p.clone(),
            }
            .body("重复计划")
            .unwrap(),
        ),
        (
            base.clone(),
            Fact::CheckSkipped {
                identity: identity(),
                reason: SkipReason::NoCheck,
                world_revision: "1".into(),
            }
            .body("其他回合")
            .unwrap(),
        ),
        (
            base.clone(),
            Fact::SettlementPlanned {
                identity: id.clone(),
                narrative_seq: 999,
                world_revision: "1".into(),
                mutations: vec![],
            }
            .body("缺正文")
            .unwrap(),
        ),
        (
            base.clone(),
            Fact::RoundSettled {
                identity: id.clone(),
                narrative_seq: 999,
                settlement_seq: 998,
                applied_through_mutation_id: None,
            }
            .body("缺结算")
            .unwrap(),
        ),
        (
            base.clone(),
            Fact::SceneStayed {
                identity: id.clone(),
                scene_id: "room".into(),
                world_revision: "1".into(),
            }
            .body("提前推进")
            .unwrap(),
        ),
        (
            base.clone(),
            Fact::RoundEnded {
                identity: id.clone(),
                outcome: Outcome::Completed,
                through_seq: 1,
                completed_steps: vec![],
                error: None,
            }
            .body("提前结束")
            .unwrap(),
        ),
    ] {
        assert_eq!(
            apply_record(&mut state, 11, body).unwrap_err().code,
            "store.corrupt"
        );
    }
    let mut duplicate = rolled.clone();
    assert!(apply_record(&mut duplicate, 11, dice::roll(&p, "1", [0; 32]).unwrap()).is_err());
    for source in [id.round_id.clone(), uuid::Uuid::new_v4().to_string()] {
        let mut state = base.clone();
        let accepted = Fact::RoundAccepted {
            identity: identity(),
            input_seq: base.round.as_ref().unwrap().input_seq,
            mode: InputMode::InCharacter,
            dice_mode: DiceMode::Manual,
            profile_id: "default".into(),
            profile_revision: "1".into(),
            target: GenerationTarget::Narration,
            scene_id: "room".into(),
            source_round_id: Some(source),
            checkpoint_seq: Some(999),
        };
        assert!(apply_record(&mut state, 12, accepted.body("错误恢复引用").unwrap()).is_err());
    }
    let mut state = base.clone();
    let accepted = Fact::RoundAccepted {
        identity: identity(),
        input_seq: base.round.as_ref().unwrap().input_seq,
        mode: InputMode::InCharacter,
        dice_mode: DiceMode::Manual,
        profile_id: "default".into(),
        profile_revision: "1".into(),
        target: GenerationTarget::Narration,
        scene_id: "room".into(),
        source_round_id: None,
        checkpoint_seq: None,
    };
    assert!(apply_record(&mut state, 12, accepted.body("覆盖活跃回合").unwrap()).is_err());
    let mut state = base;
    assert!(
        apply_record(
            &mut state,
            12,
            Fact::AbandonCheckpoint {
                identity: identity(),
                source_round_id: id.round_id,
                checkpoint_seq: 999
            }
            .body("错误放弃引用")
            .unwrap()
        )
        .is_err()
    );
    assert!(derive_prefix(&fixture.session, None, Some(999)).is_err());
}

#[test]
fn invalid_control_revision_and_unowned_narration_cannot_become_a_round_anchor() {
    let mut fixture = Fixture::new();
    let mut empty = derive(&fixture.session, None).unwrap();
    apply_record(
        &mut empty,
        1,
        Body::Narration {
            text: "调试正文".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: FinishReason::Stop,
            },
        },
    )
    .unwrap();
    assert!(empty.round.is_none());
    let id = begin(&mut fixture.session, DiceMode::Manual);
    let input = derive(&fixture.session, None)
        .unwrap()
        .round
        .unwrap()
        .input_seq;
    assert!(derive_path(&fixture.session, None, vec![], input, false).is_err());
    let skipped = skip(&mut fixture.session, &id);
    assert!(derive_path(&fixture.session, None, vec![], skipped, false).is_err());
    let mut recovered = derive(&fixture.session, None).unwrap();
    let history = Fact::HistoryFork {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        mode: crate::record::facts::ForkMode::Rewind,
        parent_control_seq: 0,
        target_seq: skipped,
        prefix_hash: crate::record::format::hash(b"prefix"),
        source_round_id: None,
        reused_dice_seq: None,
    };
    apply_record(
        &mut recovered,
        skipped + 1,
        history.body("控制由有效路径层解释").unwrap(),
    )
    .unwrap();
    assert_eq!(recovered.round.unwrap().skipped, Some(skipped));
}
