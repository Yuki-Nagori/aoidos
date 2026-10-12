use super::*;
use crate::ports::Terminal;
use crate::record::test_support::Fixture;
use aoidos_llm::schedule::FinishReason;

struct NoopWorld;
impl crate::record::world::WorldPort for NoopWorld {
    fn validate(
        &self,
        _: &crate::record::session::Session,
        _: &[crate::record::facts::WorldMutation],
    ) -> std::result::Result<(), crate::fault::Fault> {
        Ok(())
    }
    fn applied(&self, _: &str, _: &str) -> std::result::Result<bool, crate::fault::Fault> {
        Ok(true)
    }
    fn apply(
        &mut self,
        _: &crate::record::session::Session,
        _: u64,
        _: &crate::record::facts::WorldMutation,
        _: &str,
    ) -> std::result::Result<(), crate::fault::Fault> {
        Ok(())
    }
}

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
fn source_port_freezes_only_a_complete_directory_matching_the_script_revision() {
    let fixture = Fixture::new();
    let revision = fixture.session.header().script_revision.clone();
    let directory = crate::game::catalog::EntityDirectory::new(
        revision,
        7,
        true,
        vec![crate::game::catalog::CanonicalEntity {
            id: "npc".into(),
            kind: crate::game::catalog::EntityKind::Character,
            scope: "demo".into(),
            canonical_name: "NPC".into(),
        }],
    )
    .unwrap();
    let port = SourcePort::with_entity_directory(&fixture.session, &directory).unwrap();
    assert_eq!(port.boundary().unwrap().entity_revision, 7);

    let stale = crate::game::catalog::EntityDirectory::new(
        crate::record::format::hash(b"different script"),
        8,
        true,
        vec![crate::game::catalog::CanonicalEntity {
            id: "npc".into(),
            kind: crate::game::catalog::EntityKind::Character,
            scope: "demo".into(),
            canonical_name: "NPC".into(),
        }],
    )
    .unwrap();
    assert!(matches!(
        SourcePort::with_entity_directory(&fixture.session, &stale),
        Err(Error::Rejected(Reason::StaleVersion))
    ));

    let incomplete = crate::game::catalog::EntityDirectory::new(
        fixture.session.header().script_revision.clone(),
        9,
        false,
        vec![crate::game::catalog::CanonicalEntity {
            id: "npc".into(),
            kind: crate::game::catalog::EntityKind::Character,
            scope: "demo".into(),
            canonical_name: "NPC".into(),
        }],
    )
    .unwrap();
    assert!(matches!(
        SourcePort::with_entity_directory(&fixture.session, &incomplete),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
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

fn direct_character_dialogue(
    fixture: &mut Fixture,
    mode: RecordMode,
    target_speaker: &str,
    response_speaker: &str,
    include_check_skip: bool,
) -> u64 {
    use crate::record::facts::{DiceMode, GenerationTarget};

    let round_id = aoidos_memory::model::new_id();
    let identity = identity(&round_id);
    let input_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "你刚才说了什么？".into(),
            mode: Some(mode),
            content_range: None,
        })
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundAccepted {
                identity: identity.clone(),
                input_seq,
                mode,
                dice_mode: DiceMode::Auto,
                profile_id: "profile".into(),
                profile_revision: "a".repeat(64),
                target: GenerationTarget::CharacterSpeech {
                    speaker_id: target_speaker.into(),
                },
                scene_id: "mistbell".into(),
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("角色对话接纳")
            .unwrap(),
        )
        .unwrap();
    if include_check_skip {
        fixture
            .session
            .append(
                Fact::CheckSkipped {
                    identity: identity.clone(),
                    reason: crate::record::facts::SkipReason::NoCheck,
                    world_revision: "world-v1".into(),
                }
                .body("跳过检定")
                .unwrap(),
            )
            .unwrap();
    }
    let response_seq = fixture
        .session
        .append(Body::CharacterSpeech {
            speaker_id: response_speaker.into(),
            text: "我听见了。".into(),
            turn_id: aoidos_memory::model::new_id(),
            terminal: Terminal::Completed {
                finish_reason: FinishReason::Stop,
            },
        })
        .unwrap();
    let settlement_seq = fixture
        .session
        .commit_world(
            Fact::SettlementPlanned {
                identity: identity.clone(),
                narrative_seq: response_seq,
                world_revision: "world-v1".into(),
                mutations: vec![],
            }
            .body("计划结算")
            .unwrap(),
            &mut NoopWorld,
        )
        .unwrap();
    let _settled_seq = fixture
        .session
        .append(
            Fact::RoundSettled {
                identity: identity.clone(),
                narrative_seq: response_seq,
                settlement_seq,
                applied_through_mutation_id: None,
            }
            .body("角色对话结算")
            .unwrap(),
        )
        .unwrap();
    let advance_seq = fixture
        .session
        .append(
            Fact::SceneStayed {
                identity: identity.clone(),
                scene_id: "mistbell".into(),
                world_revision: "world-v1".into(),
            }
            .body("保持场景")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundEnded {
                identity,
                outcome: Outcome::Completed,
                through_seq: advance_seq,
                completed_steps: vec![],
                error: None,
            }
            .body("回合结束")
            .unwrap(),
        )
        .unwrap();
    input_seq
}

fn completed_recovered_character_dialogue(
    fixture: &mut Fixture,
    interrupted_outcome: Option<Outcome>,
) -> u64 {
    use crate::record::facts::{DiceMode, GenerationTarget};

    let input_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "你刚才说了什么？".into(),
            mode: Some(RecordMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let original_id = aoidos_memory::model::new_id();
    let original_identity = identity(&original_id);
    let target = GenerationTarget::CharacterSpeech {
        speaker_id: "npc".into(),
    };
    fixture
        .session
        .append(
            Fact::RoundAccepted {
                identity: original_identity.clone(),
                input_seq,
                mode: RecordMode::InCharacter,
                dice_mode: DiceMode::Auto,
                profile_id: "profile".into(),
                profile_revision: "a".repeat(64),
                target: target.clone(),
                scene_id: "mistbell".into(),
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("原回合接纳")
            .unwrap(),
        )
        .unwrap();
    let checkpoint_seq = fixture
        .session
        .append(
            Fact::CheckSkipped {
                identity: original_identity.clone(),
                reason: crate::record::facts::SkipReason::NoCheck,
                world_revision: "world-v1".into(),
            }
            .body("跳过检定")
            .unwrap(),
        )
        .unwrap();
    if let Some(outcome) = interrupted_outcome {
        fixture
            .session
            .append(
                Fact::RoundEnded {
                    identity: original_identity,
                    outcome,
                    through_seq: checkpoint_seq,
                    completed_steps: vec![],
                    error: (outcome == Outcome::Failed)
                        .then(|| crate::fault::Fault::new("llm.aborted", "中断")),
                }
                .body("原回合中断")
                .unwrap(),
            )
            .unwrap();
    }

    let recovered_id = aoidos_memory::model::new_id();
    let recovered_identity = identity(&recovered_id);
    fixture
        .session
        .append(
            Fact::RoundAccepted {
                identity: recovered_identity.clone(),
                input_seq,
                mode: RecordMode::InCharacter,
                dice_mode: DiceMode::Auto,
                profile_id: "profile".into(),
                profile_revision: "b".repeat(64),
                target,
                scene_id: "mistbell".into(),
                source_round_id: Some(original_id),
                checkpoint_seq: Some(checkpoint_seq),
            }
            .body("恢复回合接纳")
            .unwrap(),
        )
        .unwrap();
    let response_seq = fixture
        .session
        .append(Body::CharacterSpeech {
            speaker_id: "npc".into(),
            text: "我听见了。".into(),
            turn_id: aoidos_memory::model::new_id(),
            terminal: Terminal::Completed {
                finish_reason: FinishReason::Stop,
            },
        })
        .unwrap();
    let settlement_seq = fixture
        .session
        .commit_world(
            Fact::SettlementPlanned {
                identity: recovered_identity.clone(),
                narrative_seq: response_seq,
                world_revision: "world-v1".into(),
                mutations: vec![],
            }
            .body("计划结算")
            .unwrap(),
            &mut NoopWorld,
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundSettled {
                identity: recovered_identity.clone(),
                narrative_seq: response_seq,
                settlement_seq,
                applied_through_mutation_id: None,
            }
            .body("角色对话结算")
            .unwrap(),
        )
        .unwrap();
    let advance_seq = fixture
        .session
        .append(
            Fact::SceneStayed {
                identity: recovered_identity.clone(),
                scene_id: "mistbell".into(),
                world_revision: "world-v1".into(),
            }
            .body("保持场景")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundEnded {
                identity: recovered_identity,
                outcome: Outcome::Completed,
                through_seq: advance_seq,
                completed_steps: vec![],
                error: None,
            }
            .body("回合结束")
            .unwrap(),
        )
        .unwrap();
    input_seq
}

fn character_directory(fixture: &Fixture) -> crate::game::catalog::EntityDirectory {
    crate::game::catalog::EntityDirectory::new(
        fixture.session.header().script_revision.clone(),
        7,
        true,
        vec![crate::game::catalog::CanonicalEntity {
            id: "npc".into(),
            kind: crate::game::catalog::EntityKind::Character,
            scope: "demo".into(),
            canonical_name: "NPC".into(),
        }],
    )
    .unwrap()
}

fn told_evidence(source: SourceRef) -> KnowledgeEvidence {
    KnowledgeEvidence {
        evidence_id: aoidos_memory::model::new_id(),
        subject_id: "npc".into(),
        source,
        kind: aoidos_memory::model::KnowledgeKind::Told,
        informant_id: Some("player".into()),
        scope: DIRECT_DIALOGUE_SCOPE.into(),
        rule_id: DIRECT_DIALOGUE_RULE.into(),
    }
}

#[test]
fn told_evidence_requires_complete_directed_dialogue_and_exact_subject_and_informant() {
    let mut fixture = Fixture::new();
    let input_seq =
        direct_character_dialogue(&mut fixture, RecordMode::InCharacter, "npc", "npc", true);
    let run_id = aoidos_memory::model::new_id();
    let source = SourcePort::new(&fixture.session)
        .reference(&run_id, input_seq)
        .unwrap();
    let directory = character_directory(&fixture);
    let evidence = told_evidence(source.clone());
    {
        let port = SourcePort::with_entity_directory(&fixture.session, &directory).unwrap();
        assert!(port.valid_evidence(&evidence).unwrap());
    }

    for _ in 0..257 {
        fixture
            .session
            .append(Body::PlayerSpeech {
                player_id: "player".into(),
                text: "x".repeat(17 * 1024),
                mode: Some(RecordMode::OutOfCharacter),
                content_range: None,
            })
            .unwrap();
    }
    let port = SourcePort::with_entity_directory(&fixture.session, &directory).unwrap();
    assert!(port.valid_evidence(&evidence).unwrap());

    let mut invalid = evidence.clone();
    invalid.subject_id = "someone-else".into();
    assert!(!port.valid_evidence(&invalid).unwrap());
    let mut invalid = evidence.clone();
    invalid.informant_id = Some("another-player".into());
    assert!(!port.valid_evidence(&invalid).unwrap());
    let mut invalid = evidence.clone();
    invalid.scope = "scene".into();
    assert!(!port.valid_evidence(&invalid).unwrap());
    let mut invalid = evidence.clone();
    invalid.rule_id = "model-claimed".into();
    assert!(!port.valid_evidence(&invalid).unwrap());
    assert!(
        !SourcePort::new(&fixture.session)
            .valid_evidence(&evidence)
            .unwrap()
    );
}

#[test]
fn told_evidence_accepts_completed_checkpoint_recovery_with_inherited_target() {
    let mut fixture = Fixture::new();
    let input_seq = completed_recovered_character_dialogue(&mut fixture, None);
    let source = SourcePort::new(&fixture.session)
        .reference(&aoidos_memory::model::new_id(), input_seq)
        .unwrap();
    let directory = character_directory(&fixture);
    let port = SourcePort::with_entity_directory(&fixture.session, &directory).unwrap();

    assert!(port.valid_evidence(&told_evidence(source)).unwrap());
}

#[test]
fn told_evidence_accepts_successful_recovery_after_failed_or_cancelled_round() {
    for outcome in [Outcome::Failed, Outcome::Cancelled] {
        let mut fixture = Fixture::new();
        let input_seq = completed_recovered_character_dialogue(&mut fixture, Some(outcome));
        let source = SourcePort::new(&fixture.session)
            .reference(&aoidos_memory::model::new_id(), input_seq)
            .unwrap();
        let directory = character_directory(&fixture);
        let port = SourcePort::with_entity_directory(&fixture.session, &directory).unwrap();

        assert!(port.valid_evidence(&told_evidence(source)).unwrap());
    }
}

#[test]
fn told_evidence_rejects_offstage_and_mismatched_character_dialogue() {
    for (mode, target_speaker, response_speaker) in [
        (RecordMode::OutOfCharacter, "npc", "npc"),
        (RecordMode::InCharacter, "npc", "other-npc"),
        (RecordMode::InCharacter, "unregistered", "unregistered"),
    ] {
        let mut fixture = Fixture::new();
        let input_seq =
            direct_character_dialogue(&mut fixture, mode, target_speaker, response_speaker, true);
        let source = SourcePort::new(&fixture.session)
            .reference(&aoidos_memory::model::new_id(), input_seq)
            .unwrap();
        let evidence = told_evidence(source);
        let directory = character_directory(&fixture);
        let port = SourcePort::with_entity_directory(&fixture.session, &directory).unwrap();
        assert!(!port.valid_evidence(&evidence).unwrap());
    }
}

#[test]
fn told_evidence_rejects_incomplete_turn_recovery_and_withdrawn_path() {
    let mut incomplete = Fixture::new();
    let input_seq = direct_character_dialogue(
        &mut incomplete,
        RecordMode::InCharacter,
        "npc",
        "npc",
        false,
    );
    let source = SourcePort::new(&incomplete.session)
        .reference(&aoidos_memory::model::new_id(), input_seq)
        .unwrap();
    let evidence = told_evidence(source);
    let directory = character_directory(&incomplete);
    assert!(matches!(
        SourcePort::with_entity_directory(&incomplete.session, &directory)
            .unwrap()
            .valid_evidence(&evidence),
        Err(Error::Corrupt)
    ));

    let mut withdrawn = Fixture::new();
    let input_seq =
        direct_character_dialogue(&mut withdrawn, RecordMode::InCharacter, "npc", "npc", true);
    let source = SourcePort::new(&withdrawn.session)
        .reference(&aoidos_memory::model::new_id(), input_seq)
        .unwrap();
    let evidence = told_evidence(source);
    let directory = character_directory(&withdrawn);
    withdrawn.session.effective = Some([input_seq].into_iter().collect());
    withdrawn.session.history_revision = 1;
    assert!(
        !SourcePort::with_entity_directory(&withdrawn.session, &directory)
            .unwrap()
            .valid_evidence(&evidence)
            .unwrap()
    );
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
    assert!(matches!(
        memory_recovery_error(crate::fault::Fault::bad_request()),
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

fn dialogue_variant(change: impl FnMut(u64, &mut Body)) -> Fixture {
    dialogue_with_padding(change, 0, "")
}

fn dialogue_with_padding(
    mut change: impl FnMut(u64, &mut Body),
    padding_count: usize,
    padding_text: &str,
) -> Fixture {
    let mut original = Fixture::new();
    direct_character_dialogue(&mut original, RecordMode::InCharacter, "npc", "npc", true);
    let mut variant = Fixture::new();
    for &seq in original.session.index.keys() {
        let mut body = original.session.known(seq).unwrap().body;
        change(seq, &mut body);
        if matches!(&body, Body::System { code, .. } if code == "roundEnded") {
            for _ in 0..padding_count {
                variant
                    .session
                    .append(Body::PlayerSpeech {
                        player_id: "player".into(),
                        text: padding_text.into(),
                        mode: Some(RecordMode::OutOfCharacter),
                        content_range: None,
                    })
                    .unwrap();
            }
        }
        if matches!(&body, Body::System { code, .. } if code == "settlementPlanned") {
            variant.session.commit_world(body, &mut NoopWorld).unwrap();
        } else {
            variant.session.append(body).unwrap();
        }
    }
    variant
}

fn dialogue_is_evidence(fixture: &Fixture) -> Result<bool> {
    let source = SourcePort::new(&fixture.session).reference(&aoidos_memory::model::new_id(), 1)?;
    let directory = character_directory(fixture);
    SourcePort::with_entity_directory(&fixture.session, &directory)?
        .valid_evidence(&told_evidence(source))
}

#[test]
fn told_evidence_rejects_legacy_input_narrator_and_malformed_recovery_acceptance() {
    for case in 0..4 {
        let fixture = dialogue_variant(|_, body| match body {
            Body::PlayerSpeech { mode, .. } if case == 0 => *mode = None,
            Body::System { code, data, .. } if code == "roundAccepted" => match case {
                1 => data["target"] = serde_json::json!({"kind": "narration"}),
                2 => data["mode"] = serde_json::json!("outOfCharacter"),
                3 => {
                    data["sourceRoundId"] = serde_json::json!(aoidos_memory::model::new_id());
                    data["checkpointSeq"] = serde_json::json!(1);
                }
                _ => {}
            },
            _ => {}
        });
        assert!(!dialogue_is_evidence(&fixture).unwrap(), "case {case}");
    }
}

#[test]
fn told_evidence_rejects_unsuccessful_or_misdirected_terminal() {
    for case in 0..3 {
        let fixture = dialogue_variant(|_, body| {
            if let Body::System { code, data, .. } = body
                && code == "roundEnded"
            {
                match case {
                    0 => data["outcome"] = serde_json::json!("cancelled"),
                    1 => {
                        data["outcome"] = serde_json::json!("failed");
                        data["error"] =
                            serde_json::to_value(crate::fault::Fault::new("llm.aborted", "中断"))
                                .unwrap();
                    }
                    2 => data["throughSeq"] = serde_json::json!(5),
                    _ => unreachable!(),
                }
            }
        });
        assert!(!dialogue_is_evidence(&fixture).unwrap(), "case {case}");
    }
}

#[test]
fn told_evidence_requires_a_terminal_within_both_scan_limits() {
    for (count, text) in [
        (MAX_SCAN + 1, "旁白".to_owned()),
        (17, "x".repeat(crate::record::format::MAX_TEXT)),
    ] {
        let fixture = dialogue_with_padding(|_, _| {}, count, &text);
        assert!(!dialogue_is_evidence(&fixture).unwrap());
    }
}

#[test]
fn alias_context_and_canonical_lookup_use_all_trusted_entity_categories() {
    use crate::game::catalog::{CanonicalEntity, EntityDirectory, EntityKind};
    let fixture = Fixture::new();
    let kinds = [
        (EntityKind::Character, EntityCategory::Character),
        (EntityKind::Place, EntityCategory::Place),
        (EntityKind::Organization, EntityCategory::Organization),
        (EntityKind::Item, EntityCategory::Item),
        (EntityKind::Term, EntityCategory::Term),
    ];
    let entities: Vec<_> = kinds
        .iter()
        .enumerate()
        .map(|(index, (kind, _))| CanonicalEntity {
            id: format!("entity-{index}"),
            kind: *kind,
            scope: "demo".into(),
            canonical_name: format!("名称{index}"),
        })
        .collect();
    let directory = EntityDirectory::new(
        fixture.session.header().script_revision.clone(),
        7,
        true,
        entities.clone(),
    )
    .unwrap();
    let port = SourcePort::with_entity_directory(&fixture.session, &directory).unwrap();
    for (entity, (_, category)) in entities.iter().zip(kinds) {
        assert_eq!(
            port.entity_alias_context(&entity.id).unwrap(),
            Some(EntityAliasContext {
                entity_id: entity.id.clone(),
                category,
                scope: entity.scope.clone(),
                canonical_name: entity.canonical_name.clone(),
            })
        );
        assert_eq!(
            port.canonical_entity_id(category, "demo", &entity.canonical_name)
                .unwrap(),
            Some(entity.id.clone())
        );
        assert_eq!(
            port.canonical_entity_id(category, "other", &entity.canonical_name)
                .unwrap(),
            None
        );
    }
    assert_eq!(port.entity_alias_context("missing").unwrap(), None);
    assert_eq!(
        SourcePort::new(&fixture.session)
            .entity_alias_context("missing")
            .unwrap(),
        None
    );
    assert_eq!(
        SourcePort::new(&fixture.session)
            .canonical_entity_id(EntityCategory::Character, "demo", "名称0")
            .unwrap(),
        None
    );
    assert!(matches!(
        port.canonical_entity_id(EntityCategory::Character, "", "名称0"),
        Err(Error::Rejected(Reason::PolicyUnavailable))
    ));
}

#[test]
fn alias_context_does_not_trust_an_incomplete_directory() {
    let fixture = Fixture::new();
    let incomplete = crate::game::catalog::EntityDirectory::new(
        fixture.session.header().script_revision.clone(),
        1,
        false,
        vec![crate::game::catalog::CanonicalEntity {
            id: "npc".into(),
            kind: crate::game::catalog::EntityKind::Character,
            scope: "demo".into(),
            canonical_name: "角色".into(),
        }],
    )
    .unwrap();
    // 正式构造器会先拒绝；此处验证底层目录读取仍拒绝局部快照。
    let port = SourcePort {
        session: &fixture.session,
        entity_directory: Some(&incomplete),
    };
    assert!(matches!(
        port.entity_alias_context("missing"),
        Err(Error::Rejected(Reason::PolicyUnavailable))
    ));
}
