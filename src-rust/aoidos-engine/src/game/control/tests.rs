use super::*;
use crate::{
    game::state::{CheckpointStage, Phase},
    ports::Outcome,
    record::{
        facts::{DiceMode, GenerationTarget, RoundIdentity, SkipReason},
        format::InputMode,
        history::HistoryPort,
        test_support::Fixture,
    },
};
fn fact(session: &mut Session, fact: Fact) -> u64 {
    session.append(fact.body("步骤").unwrap()).unwrap()
}
struct History;
impl HistoryPort for History {
    fn verify_target(&self, session: &Session, target: u64) -> Result<(), Fault> {
        verify_boundary(session, target)
    }
    fn applied(&self, _: &str, _: &str) -> Result<bool, Fault> {
        Ok(true)
    }
    fn rebuild(&mut self, _: &Session, _: &[u64], _: &str, _: &str) -> Result<(), Fault> {
        Ok(())
    }
}
#[test]
fn regenerate_forks_to_confirmed_prebody_boundary_and_keeps_physical_history() {
    let mut fixture = Fixture::new();
    let session = &mut fixture.session;
    let input = session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "搜索".into(),
            mode: Some(InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let id = RoundIdentity {
        version: 1,
        round_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let accepted = fact(
        session,
        Fact::RoundAccepted {
            identity: id.clone(),
            input_seq: input,
            mode: InputMode::InCharacter,
            dice_mode: DiceMode::Manual,
            profile_id: "default".into(),
            profile_revision: "1".into(),
            target: GenerationTarget::Narration,
            scene_id: "room".into(),
            source_round_id: None,
            checkpoint_seq: None,
        },
    );
    assert!(verify_boundary(session, input).is_err());
    assert!(verify_boundary(session, accepted).is_err());
    let skipped = fact(
        session,
        Fact::CheckSkipped {
            identity: id.clone(),
            reason: SkipReason::NoCheck,
            world_revision: "1".into(),
        },
    );
    verify_boundary(session, skipped).unwrap();
    assert!(regeneration_target(&recovery::derive(session, None).unwrap(), &id.round_id).is_err());
    let ended = fact(
        session,
        Fact::RoundEnded {
            identity: id.clone(),
            outcome: Outcome::Cancelled,
            through_seq: skipped,
            completed_steps: vec![],
            error: None,
        },
    );
    verify_boundary(session, ended).unwrap();
    let target =
        regeneration_target(&recovery::derive(session, None).unwrap(), &id.round_id).unwrap();
    assert_eq!(target.target_seq, skipped);
    assert!(target.reused_dice_seq.is_none());
    let operation = uuid::Uuid::new_v4().to_string();
    let body = prepare_fork(
        session,
        &operation,
        ForkMode::Regenerate,
        skipped,
        Some(&target),
    )
    .unwrap();
    let mut wrong_target = target.clone();
    wrong_target.target_seq = ended;
    assert!(
        prepare_fork(
            session,
            &operation,
            ForkMode::Regenerate,
            skipped,
            Some(&wrong_target)
        )
        .is_err()
    );
    verify_replay_boundary(session, 0, skipped).unwrap();
    for invalid in [0, format::MAX_SEQ + 1, ended + 1] {
        assert!(verify_replay_boundary(session, 0, invalid).is_err());
    }
    let control = session.commit_history_fork(body, &mut History).unwrap();
    assert!(verify_boundary(session, ended).is_err());
    assert!(!session.path_at(control).unwrap().contains(&ended));
    assert_eq!(session.read(ended).unwrap().seq, ended);
    let recovered = recovery::derive(session, None).unwrap();
    assert_eq!(recovered.phase, Phase::Idle);
    assert_eq!(
        recovered.last_operation.as_ref().unwrap().operation_id,
        operation
    );
    assert_eq!(
        recovered.last_operation.as_ref().unwrap().outcome,
        crate::game::state::OperationOutcome::Completed
    );
    assert!(
        recovered
            .last_operation
            .as_ref()
            .unwrap()
            .round_id
            .is_none()
    );
    let checkpoint = recovered.checkpoint.as_ref().unwrap();
    assert_eq!(checkpoint.stage, CheckpointStage::Narration);
    assert_eq!(checkpoint.through_seq, skipped);
    assert_eq!(checkpoint.source_round_id, id.round_id);
    assert!(prepare_fork(session, "invalid", ForkMode::Rewind, skipped, None).is_err());
    assert!(prepare_fork(session, &operation, ForkMode::Regenerate, skipped, None).is_err());
    assert!(
        prepare_fork(
            session,
            &operation,
            ForkMode::Rewind,
            skipped,
            Some(&target)
        )
        .is_err()
    );
    assert!(verify_boundary(session, 0).is_err());
    assert!(verify_boundary(session, format::MAX_SEQ + 1).is_err());
    assert!(regeneration_target(&recovered, "invalid").is_err());
}

#[tokio::test]
async fn a_committed_partial_output_blocks_new_history_controls() {
    use crate::ports::OutputWriter;
    use crate::record::session::{PersistentWriter, Target};
    use std::sync::{Arc, Mutex};
    let fixture = Fixture::new();
    let session = Arc::new(Mutex::new(
        Session::open(fixture.path.clone(), fixture.events.clone()).unwrap(),
    ));
    let writer = PersistentWriter {
        session: session.clone(),
        target: Target::Narration,
        high: false,
    };
    writer
        .begin(&uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    assert_eq!(
        verify_boundary(&session.lock().unwrap(), 1)
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
}
