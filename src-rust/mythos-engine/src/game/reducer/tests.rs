use super::*;
fn initial(mode: InputMode, dice_mode: DiceMode) -> State {
    State::new("epoch".into(), "round".into(), mode, dice_mode, false, None)
}
fn advance(state: State, receipt: Receipt) -> State {
    let identity = state.current.as_ref().unwrap().identity.clone();
    state
        .step(Event::Completed { identity, receipt })
        .unwrap()
        .state
}
#[test]
fn manual_and_no_check_rounds_follow_confirmed_effects_and_ignore_old_receipts() {
    let state = initial(InputMode::InCharacter, DiceMode::Manual);
    assert!(state.step(Event::SubmitCheck).is_err());
    let state = state.step(Event::Begin).unwrap().state;
    assert_eq!(state.current.as_ref().unwrap().action, Action::CommitInput);
    let old = state.current.as_ref().unwrap().identity.clone();
    let state = advance(state, Receipt::Input(1));
    assert_eq!(state.stage, Stage::CheckProposal);
    assert!(
        state
            .step(Event::Completed {
                identity: old,
                receipt: Receipt::Input(1)
            })
            .unwrap()
            .ignored
    );
    let state = advance(state, Receipt::NoCheck);
    let state = advance(state, Receipt::Skip(2));
    assert_eq!(state.stage.phase(state.checked), Phase::Generating);
    let state = advance(state, Receipt::Narrative(3));
    assert_eq!(state.stage.phase(state.checked), Phase::Settling);
    let state = advance(state, Receipt::Settlement(4));
    assert_eq!(state.stage.phase(state.checked), Phase::Advancing);
    let state = advance(state, Receipt::SceneSelected);
    let state = advance(state, Receipt::Advance(5));
    let state = advance(state, Receipt::Ended);
    assert_eq!(state.stage.phase(state.checked), Phase::Idle);
    assert!(state.step(Event::Begin).is_err());
}

fn plan() -> CheckPlan {
    crate::game::dice::freeze_pbta(crate::game::dice::PlanSpec {
        rule_id: "search",
        actor_id: "player",
        modifiers: vec![],
        branches: ["s".into(), "c".into(), "f".into()],
        world_revision: "1",
    })
    .unwrap()
}
#[test]
fn manual_auto_forced_disabled_and_ooc_share_the_same_exactly_once_roll_transition() {
    for dice_mode in [DiceMode::Manual, DiceMode::Auto] {
        for forced in [false, true] {
            let mut state = initial(InputMode::InCharacter, dice_mode);
            if forced {
                state.plan = Some(plan());
            }
            let state = advance(state.step(Event::Begin).unwrap().state, Receipt::Input(1));
            let state = if forced {
                assert_eq!(state.stage, Stage::PlanCommit);
                state
            } else {
                advance(state, Receipt::Check(Box::new(plan())))
            };
            let mut state = advance(state, Receipt::Plan(2));
            assert_eq!(state.plan_seq, Some(2));
            if dice_mode == DiceMode::Manual {
                assert_eq!(state.stage.phase(false), Phase::AwaitingCheck);
                assert!(state.current.is_none());
                state = state.step(Event::SubmitCheck).unwrap().state;
            }
            assert_eq!(state.current.as_ref().unwrap().action, Action::RollDice);
            assert!(state.step(Event::SubmitCheck).unwrap().ignored);
            let state = advance(state, Receipt::Dice(3));
            assert_eq!(state.stage.phase(false), Phase::Settling);
            let state = advance(state, Receipt::CheckResult(4));
            assert!(state.checked);
            assert_eq!(state.stage.phase(true), Phase::Settling);
            let state = advance(state, Receipt::Narrative(5));
            let state = advance(state, Receipt::Settlement(6));
            let state = advance(state, Receipt::SceneSelected);
            let state = advance(state, Receipt::Advance(7));
            let state = advance(state, Receipt::Ended);
            assert_eq!(state.stage, Stage::Idle);
        }
    }
    for (mode, disabled) in [
        (InputMode::OutOfCharacter, false),
        (InputMode::InCharacter, true),
    ] {
        let mut state = initial(mode, DiceMode::Auto);
        state.dice_disabled = disabled;
        state.plan = Some(plan());
        let state = advance(state.step(Event::Begin).unwrap().state, Receipt::Input(1));
        assert_eq!(state.stage, Stage::SkipCommit);
        let state = advance(state, Receipt::Skip(2));
        assert_eq!(state.skip_seq, Some(2));
        assert_eq!(state.current.unwrap().action, Action::StartNarration);
    }
}
#[test]
fn malformed_current_receipts_and_effect_capacity_fail_without_mutating_the_original() {
    let mut state = initial(InputMode::InCharacter, DiceMode::Manual)
        .step(Event::Begin)
        .unwrap()
        .state;
    let current = state.current.as_ref().unwrap().identity.clone();
    assert!(
        state
            .step(Event::Completed {
                identity: current,
                receipt: Receipt::Ended
            })
            .is_err()
    );
    assert_eq!(state.stage, Stage::Accepting);
    state.next_effect = crate::record::format::MAX_SEQ + 1;
    let current = state.current.as_ref().unwrap().identity.clone();
    assert!(
        state
            .step(Event::Completed {
                identity: current,
                receipt: Receipt::Input(1)
            })
            .is_err()
    );
    assert_eq!(state.input_seq, None);
}
