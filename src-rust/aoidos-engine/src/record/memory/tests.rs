use super::*;
use crate::ports::Terminal;
use crate::record::test_support::Fixture;
use aoidos_llm::schedule::FinishReason;

#[test]
fn source_references_preserve_original_text_and_validate_utf8_identity_and_mode() {
    let mut fixture = Fixture::new();
    let seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "李雷看见🐉，不是李磊。".into(),
            mode: Some(RecordMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let port = SourcePort::new(&fixture.session);
    let mut source = port
        .reference(&aoidos_memory::model::new_id(), seq)
        .unwrap();
    assert_eq!(
        port.source_text(&source).unwrap().as_deref(),
        Some("李雷看见🐉，不是李磊。")
    );
    assert!(port.valid_source(&source).unwrap());
    assert_eq!(source.mode, Some(InputMode::InCharacter));
    source.start_byte = 1;
    assert!(!port.valid_source(&source).unwrap());
    source.start_byte = 0;
    source.end_byte = 3;
    assert_eq!(port.source_text(&source).unwrap().as_deref(), Some("李"));
    source.body_hash = [0; 32];
    assert!(!port.valid_source(&source).unwrap());
    source.body_hash = hash("李雷看见🐉，不是李磊。".as_bytes());
    source.mode = Some(InputMode::OutOfCharacter);
    assert!(!port.valid_source(&source).unwrap());
    source.mode = Some(InputMode::InCharacter);
    source.script_id = "other".into();
    assert!(!port.valid_source(&source).unwrap());
    source.script_id = fixture.session.header().script_id.clone();
    source.session_id = aoidos_memory::model::new_id();
    assert!(!port.valid_source(&source).unwrap());
    assert!(
        port.reference(&aoidos_memory::model::new_id(), seq + 1)
            .is_err()
    );
}

#[test]
fn cancelled_and_failed_narration_are_not_material_and_offstage_keeps_its_mode() {
    let mut fixture = Fixture::new();
    let completed = fixture
        .session
        .append(Body::Narration {
            text: "已完成经历".into(),
            turn_id: aoidos_memory::model::new_id(),
            terminal: Terminal::Completed {
                finish_reason: FinishReason::Stop,
            },
        })
        .unwrap();
    let cancelled = fixture
        .session
        .append(Body::Narration {
            text: "取消前文".into(),
            turn_id: aoidos_memory::model::new_id(),
            terminal: Terminal::Cancelled,
        })
        .unwrap();
    let failed = fixture
        .session
        .append(Body::Narration {
            text: "失败前文".into(),
            turn_id: aoidos_memory::model::new_id(),
            terminal: Terminal::Failed {
                error: crate::fault::Fault::new("llm.aborted", "中断"),
                finish_reason: None,
            },
        })
        .unwrap();
    let offstage = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "不要恐怖描写".into(),
            mode: Some(RecordMode::OutOfCharacter),
            content_range: None,
        })
        .unwrap();
    let port = SourcePort::new(&fixture.session);
    let run = aoidos_memory::model::new_id();
    assert!(port.reference(&run, completed).is_ok());
    assert!(port.reference(&run, cancelled).is_err());
    assert!(port.reference(&run, failed).is_err());
    let source = port.reference(&run, offstage).unwrap();
    assert_eq!(source.mode, Some(InputMode::OutOfCharacter));
    assert!(
        !port
            .completed_round(&aoidos_memory::model::new_id(), &source)
            .unwrap()
    );
}

#[test]
fn withdrawn_branch_and_pending_world_or_history_are_not_writable_boundaries() {
    let mut fixture = Fixture::new();
    let seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "原始陈述".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    let run = aoidos_memory::model::new_id();
    let source = SourcePort::new(&fixture.session)
        .reference(&run, seq)
        .unwrap();
    fixture.session.history_revision = 99;
    fixture.session.effective = Some(std::collections::BTreeSet::new());
    assert!(
        !SourcePort::new(&fixture.session)
            .valid_source(&source)
            .unwrap()
    );
    fixture.session.history_revision = 0;
    fixture.session.effective = None;
    for field in 0..3 {
        fixture.session.pending_world = field == 0;
        fixture.session.pending_history = field == 1;
        fixture.session.read_only = field == 2;
        assert!(matches!(
            SourcePort::new(&fixture.session).boundary(),
            Err(Error::Rejected(Reason::RecoveryRequired))
        ));
    }
    fixture.session.read_only = false;
    assert_eq!(
        SourcePort::new(&fixture.session)
            .boundary()
            .unwrap()
            .history_revision,
        0
    );
}

fn identity(round_id: &str) -> crate::record::facts::RoundIdentity {
    crate::record::facts::RoundIdentity {
        version: 1,
        round_id: round_id.into(),
        operation_id: aoidos_memory::model::new_id(),
    }
}
fn accepted(fixture: &mut Fixture, round_id: &str, mode: RecordMode) -> u64 {
    use crate::record::facts::{DiceMode, GenerationTarget};
    let input_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "继续探索".into(),
            mode: Some(mode),
            content_range: None,
        })
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundAccepted {
                identity: identity(round_id),
                input_seq,
                mode,
                dice_mode: DiceMode::Auto,
                profile_id: "profile".into(),
                profile_revision: "a".repeat(64),
                target: GenerationTarget::Narration,
                scene_id: "mistbell".into(),
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("回合接纳")
            .unwrap(),
        )
        .unwrap()
}
fn ended(fixture: &mut Fixture, round_id: &str, through_seq: u64, outcome: Outcome) -> u64 {
    fixture
        .session
        .append(
            Fact::RoundEnded {
                identity: identity(round_id),
                outcome,
                through_seq,
                completed_steps: vec![],
                error: (outcome == Outcome::Failed)
                    .then(|| crate::fault::Fault::new("llm.aborted", "中断")),
            }
            .body("回合结束")
            .unwrap(),
        )
        .unwrap()
}
#[test]
fn completed_round_requires_matching_effective_in_character_acceptance() {
    let mut fixture = Fixture::new();
    let round = aoidos_memory::model::new_id();
    let acceptance = accepted(&mut fixture, &round, RecordMode::InCharacter);
    let seq = ended(&mut fixture, &round, acceptance, Outcome::Completed);
    let run = aoidos_memory::model::new_id();
    let source = SourcePort::new(&fixture.session)
        .reference(&run, seq)
        .unwrap();
    assert!(
        SourcePort::new(&fixture.session)
            .completed_round(&round, &source)
            .unwrap()
    );
    assert!(
        !SourcePort::new(&fixture.session)
            .completed_round(&aoidos_memory::model::new_id(), &source)
            .unwrap()
    );
    let mut invalid = source.clone();
    invalid.end_byte = 0;
    assert!(
        !SourcePort::new(&fixture.session)
            .completed_round(&round, &invalid)
            .unwrap()
    );
    let evidence = KnowledgeEvidence {
        evidence_id: aoidos_memory::model::new_id(),
        subject_id: "npc".into(),
        source: source.clone(),
        kind: aoidos_memory::model::KnowledgeKind::Observed,
        informant_id: None,
        scope: "scene".into(),
        rule_id: "observed".into(),
    };
    assert!(
        !SourcePort::new(&fixture.session)
            .valid_evidence(&evidence)
            .unwrap()
    );
    fixture.session.history_revision = 99;
    fixture.session.effective = Some([seq].into_iter().collect());
    assert!(
        !SourcePort::new(&fixture.session)
            .completed_round(&round, &source)
            .unwrap()
    );
}
#[test]
fn offstage_and_missing_acceptance_do_not_advance_memory_clock() {
    let mut fixture = Fixture::new();
    let offstage = aoidos_memory::model::new_id();
    let through = accepted(&mut fixture, &offstage, RecordMode::OutOfCharacter);
    let seq = ended(&mut fixture, &offstage, through, Outcome::Completed);
    let run = aoidos_memory::model::new_id();
    let source = SourcePort::new(&fixture.session)
        .reference(&run, seq)
        .unwrap();
    assert!(
        !SourcePort::new(&fixture.session)
            .completed_round(&offstage, &source)
            .unwrap()
    );
    let unmatched = aoidos_memory::model::new_id();
    let seq = ended(&mut fixture, &unmatched, through, Outcome::Completed);
    let source = SourcePort::new(&fixture.session)
        .reference(&run, seq)
        .unwrap();
    assert!(
        !SourcePort::new(&fixture.session)
            .completed_round(&unmatched, &source)
            .unwrap()
    );
    for outcome in [Outcome::Cancelled, Outcome::Failed] {
        let seq = ended(&mut fixture, &offstage, through, outcome);
        assert!(
            SourcePort::new(&fixture.session)
                .reference(&run, seq)
                .is_err()
        );
    }
}
#[test]
fn successful_character_text_and_settlement_facts_have_canonical_sources() {
    let mut fixture = Fixture::new();
    let round = aoidos_memory::model::new_id();
    let acceptance = accepted(&mut fixture, &round, RecordMode::InCharacter);
    let seq = fixture
        .session
        .append(Body::CharacterSpeech {
            speaker_id: "keeper".into(),
            text: "钟声不是幻觉。".into(),
            turn_id: aoidos_memory::model::new_id(),
            terminal: Terminal::Completed {
                finish_reason: FinishReason::Length,
            },
        })
        .unwrap();
    let run = aoidos_memory::model::new_id();
    let source = SourcePort::new(&fixture.session)
        .reference(&run, seq)
        .unwrap();
    assert_eq!(source.mode, None);
    assert_eq!(
        SourcePort::new(&fixture.session)
            .source_text(&source)
            .unwrap()
            .as_deref(),
        Some("钟声不是幻觉。")
    );
    for fact in [
        Fact::SceneStayed {
            identity: identity(&round),
            scene_id: "mistbell".into(),
            world_revision: "revision".into(),
        },
        Fact::RoundSettled {
            identity: identity(&round),
            narrative_seq: seq,
            settlement_seq: acceptance,
            applied_through_mutation_id: None,
        },
    ] {
        let body = fact.body("展示信息不作为事实来源").unwrap();
        let expected = match &body {
            Body::System { data, .. } => aoidos_json::canonical_string(data).unwrap(),
            _ => unreachable!(),
        };
        let seq = fixture.session.append(body).unwrap();
        let source = SourcePort::new(&fixture.session)
            .reference(&run, seq)
            .unwrap();
        assert_eq!(
            SourcePort::new(&fixture.session)
                .source_text(&source)
                .unwrap()
                .unwrap(),
            expected
        );
        assert!(
            !SourcePort::new(&fixture.session)
                .completed_round(&round, &source)
                .unwrap()
        );
    }
    let mut invalid = source.clone();
    invalid.end_byte = u64::MAX;
    assert!(
        !SourcePort::new(&fixture.session)
            .valid_source(&invalid)
            .unwrap()
    );
}
#[test]
fn empty_completed_text_is_not_a_source() {
    let mut fixture = Fixture::new();
    let run = aoidos_memory::model::new_id();
    let seq = fixture
        .session
        .append(Body::Narration {
            text: String::new(),
            turn_id: aoidos_memory::model::new_id(),
            terminal: Terminal::Completed {
                finish_reason: FinishReason::Stop,
            },
        })
        .unwrap();
    assert!(
        SourcePort::new(&fixture.session)
            .reference(&run, seq)
            .is_err()
    );
    assert!(matches!(
        json_error(aoidos_json::Error::InvalidJson),
        Error::Corrupt
    ));
}

#[test]
fn dice_and_check_sources_serialize_the_registered_body_not_display_text() {
    use crate::record::{
        facts::{CheckPlan, DiceMode, ResultPolicy},
        format::{CheckResult, RngTrace, Roll, Source},
    };
    let mut fixture = Fixture::new();
    let round = aoidos_memory::model::new_id();
    accepted(&mut fixture, &round, RecordMode::InCharacter);
    let planned = Fact::CheckPlanned {
        identity: identity(&round),
        dice_mode: DiceMode::Manual,
        plan: CheckPlan {
            plan_id: "plan".into(),
            rule_id: "rule".into(),
            rule_version: 1,
            actor_id: "player".into(),
            expression: "1d20".into(),
            modifiers: vec![],
            result_policy: ResultPolicy {
                kind: "dc".into(),
                dc: Some(10.0),
            },
            success_branch_id: "success".into(),
            costly_success_branch_id: "costly".into(),
            failure_branch_id: "failure".into(),
            world_revision: "0".into(),
            plan_hash: crate::record::format::hash(b"plan"),
        },
    };
    let planned_body = planned.body("计划不是已完成事实").unwrap();
    if let Body::System { code, data, .. } = &planned_body {
        Fact::decode(code, data).unwrap();
    }
    let plan_seq = fixture.session.append(planned_body).unwrap();
    let run = aoidos_memory::model::new_id();
    assert!(
        SourcePort::new(&fixture.session)
            .reference(&run, plan_seq)
            .is_err()
    );
    let dice = Body::Dice {
        expression: "1d20".into(),
        rolls: vec![Roll {
            sides: 20,
            value: 13,
        }],
        total: 13,
        source: Source {
            kind: "rule".into(),
            id: "rule".into(),
        },
        plan_id: "plan".into(),
        rng: RngTrace {
            algorithm: "chacha20-v1".into(),
            mapping_version: 1,
            seed: "1".repeat(64),
            start_counter: "0".into(),
            end_counter: "1".into(),
        },
        modifiers: vec![],
    };
    let expected = aoidos_json::canonical_string(&dice).unwrap();
    let dice_seq = fixture.session.append(dice).unwrap();
    let source = SourcePort::new(&fixture.session)
        .reference(&run, dice_seq)
        .unwrap();
    assert_eq!(
        SourcePort::new(&fixture.session)
            .source_text(&source)
            .unwrap()
            .unwrap(),
        expected
    );
    let check = Body::Check {
        dice_seq,
        dc: Some(10.0),
        result: CheckResult::Success,
        rule_id: "rule".into(),
        plan_id: "plan".into(),
    };
    let expected = aoidos_json::canonical_string(&check).unwrap();
    let seq = fixture.session.append(check).unwrap();
    let source = SourcePort::new(&fixture.session)
        .reference(&run, seq)
        .unwrap();
    assert_eq!(
        SourcePort::new(&fixture.session)
            .source_text(&source)
            .unwrap()
            .unwrap(),
        expected
    );
}
#[test]
fn round_acceptance_lookup_has_a_hard_record_limit() {
    let mut fixture = Fixture::new();
    let round = aoidos_memory::model::new_id();
    let through = accepted(&mut fixture, &round, RecordMode::InCharacter);
    for _ in 0..256 {
        fixture
            .session
            .append(Body::PlayerSpeech {
                player_id: "player".into(),
                text: "已提交补充".into(),
                mode: None,
                content_range: None,
            })
            .unwrap();
    }
    let seq = ended(&mut fixture, &round, through, Outcome::Completed);
    let source = SourcePort::new(&fixture.session)
        .reference(&aoidos_memory::model::new_id(), seq)
        .unwrap();
    assert!(
        !SourcePort::new(&fixture.session)
            .completed_round(&round, &source)
            .unwrap()
    );
}

#[test]
fn round_acceptance_lookup_has_a_hard_byte_limit() {
    let mut fixture = Fixture::new();
    let round = aoidos_memory::model::new_id();
    let through = accepted(&mut fixture, &round, RecordMode::InCharacter);
    for _ in 0..16 {
        fixture
            .session
            .append(Body::PlayerSpeech {
                player_id: "player".into(),
                text: "x".repeat(crate::record::format::MAX_TEXT),
                mode: None,
                content_range: None,
            })
            .unwrap();
    }
    let seq = ended(&mut fixture, &round, through, Outcome::Completed);
    let source = SourcePort::new(&fixture.session)
        .reference(&aoidos_memory::model::new_id(), seq)
        .unwrap();
    assert!(
        !SourcePort::new(&fixture.session)
            .completed_round(&round, &source)
            .unwrap()
    );
}
