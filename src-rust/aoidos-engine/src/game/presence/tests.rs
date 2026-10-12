use super::*;
use crate::{
    game::catalog::{Node, Scene, SceneRuleView},
    ports::Terminal,
    record::{
        facts::{
            DiceMode, Fact, GenerationTarget, RoundIdentity, SceneNode, ScenePosition, SkipReason,
            Step,
        },
        format::Body,
        history::HistoryPort,
        test_support::Fixture,
        world::WorldPort,
    },
};
use rusqlite::Connection;

struct Port {
    connection: Connection,
    catalog: SceneCatalog,
    directory: EntityDirectory,
    fail_apply: bool,
    fail_apply_on: Option<usize>,
    applied_count: usize,
}

impl WorldPort for Port {
    fn validate(&self, session: &Session, mutations: &[WorldMutation]) -> Result<(), Fault> {
        ScenePresence::validate_mutations(
            &self.connection,
            session,
            &self.catalog,
            &self.directory,
            mutations,
        )
    }

    fn validate_recovery(
        &self,
        session: &Session,
        record_seq: u64,
        mutations: &[WorldMutation],
    ) -> Result<(), Fault> {
        ScenePresence::validate_recovery_mutations(
            &self.connection,
            session,
            &self.catalog,
            &self.directory,
            record_seq,
            mutations,
        )
    }

    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        aoidos_store::applied::contains(&self.connection, id, hash).map_err(Fault::from)
    }

    fn apply(
        &mut self,
        _: &Session,
        _: u64,
        mutation: &WorldMutation,
        hash: &str,
    ) -> Result<(), Fault> {
        if self.fail_apply || self.fail_apply_on == Some(self.applied_count) {
            return Err(Fault::new("store.unavailable", "synthetic apply failure"));
        }
        aoidos_store::applied::apply_once(
            &mut self.connection,
            &mutation.mutation_id,
            hash,
            &mut |_| Ok(()),
        )
        .map(|_| ())
        .map_err(Fault::from)?;
        self.applied_count += 1;
        Ok(())
    }
}

impl HistoryPort for Port {
    fn verify_target(&self, session: &Session, target: u64) -> Result<(), Fault> {
        crate::game::control::verify_boundary(session, target)
    }

    fn verify_replay_target(
        &self,
        session: &Session,
        parent: u64,
        target: u64,
    ) -> Result<(), Fault> {
        crate::game::control::verify_replay_boundary(session, parent, target)
    }

    fn applied(&self, operation: &str, hash: &str) -> Result<bool, Fault> {
        aoidos_store::applied::contains(&self.connection, operation, hash).map_err(Fault::from)
    }

    fn rebuild(
        &mut self,
        _: &Session,
        _: &[u64],
        operation: &str,
        hash: &str,
    ) -> Result<(), Fault> {
        aoidos_store::applied::apply_once(&mut self.connection, operation, hash, &mut |_| Ok(()))
            .map(|_| ())
            .map_err(Fault::from)
    }
}

fn begin_round(session: &mut Session) -> RoundIdentity {
    begin_round_for_target(session, GenerationTarget::Narration)
}

fn begin_round_for_target(session: &mut Session, target: GenerationTarget) -> RoundIdentity {
    let input_seq = session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "查看周围".into(),
            mode: Some(crate::record::format::InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let identity = RoundIdentity {
        version: 1,
        round_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    session
        .append(
            Fact::RoundAccepted {
                identity: identity.clone(),
                input_seq,
                mode: crate::record::format::InputMode::InCharacter,
                dice_mode: DiceMode::Manual,
                profile_id: "default".into(),
                profile_revision: "1".into(),
                target,
                scene_id: "room".into(),
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("接纳回合")
            .unwrap(),
        )
        .unwrap();
    session
        .append(
            Fact::CheckSkipped {
                identity: identity.clone(),
                reason: SkipReason::NoCheck,
                world_revision: "world-v1".into(),
            }
            .body("无需检定")
            .unwrap(),
        )
        .unwrap();
    identity
}

fn finish_round(
    session: &mut Session,
    port: &mut Port,
    identity: RoundIdentity,
    narrative_seq: u64,
    mutations: Vec<WorldMutation>,
) -> u64 {
    let applied_through_mutation_id = mutations
        .last()
        .map(|mutation| mutation.mutation_id.clone());
    let settlement_seq = session
        .commit_world(
            Fact::SettlementPlanned {
                identity: identity.clone(),
                narrative_seq,
                world_revision: "world-v1".into(),
                mutations,
            }
            .body("登记结算")
            .unwrap(),
            port,
        )
        .unwrap();
    session
        .append(
            Fact::RoundSettled {
                identity: identity.clone(),
                narrative_seq,
                settlement_seq,
                applied_through_mutation_id,
            }
            .body("结算完成")
            .unwrap(),
        )
        .unwrap();
    let through_seq = session
        .append(
            Fact::SceneStayed {
                identity: identity.clone(),
                scene_id: "room".into(),
                world_revision: "world-v1".into(),
            }
            .body("留在房间")
            .unwrap(),
        )
        .unwrap();
    session
        .append(
            Fact::RoundEnded {
                identity,
                outcome: crate::ports::Outcome::Completed,
                through_seq,
                completed_steps: vec![
                    Step::Check,
                    Step::Narration,
                    Step::Settlement,
                    Step::Advance,
                ],
                error: None,
            }
            .body("回合完成")
            .unwrap(),
        )
        .unwrap()
}

fn catalog(script_revision: &str) -> (SceneCatalog, EntityDirectory) {
    catalog_version(script_revision, "catalog-v1")
}

fn catalog_version(
    script_revision: &str,
    catalog_revision: &str,
) -> (SceneCatalog, EntityDirectory) {
    let directory = EntityDirectory::new(
        script_revision.into(),
        1,
        true,
        [
            ("player", "玩家"),
            ("keeper", "守门人"),
            ("messenger", "信使"),
        ]
        .into_iter()
        .map(|(id, name)| CanonicalEntity {
            id: id.into(),
            kind: EntityKind::Character,
            scope: "global".into(),
            canonical_name: name.into(),
        })
        .collect(),
    )
    .unwrap();
    let position = ScenePosition {
        scene_id: "room".into(),
        path: vec![SceneNode {
            kind: "scene".into(),
            id: "room".into(),
            title: "房间".into(),
        }],
    };
    let policy = PresencePolicy::new(
        "room".into(),
        SceneEntryPresence {
            rule_id: "room-entry-v1".into(),
            rule_version: 1,
            character_ids: vec!["player".into()],
        },
        BTreeMap::from([
            (
                "room".into(),
                SceneEntryPresence {
                    rule_id: "room-entry-v1".into(),
                    rule_version: 1,
                    character_ids: vec!["player".into()],
                },
            ),
            (
                "hall".into(),
                SceneEntryPresence {
                    rule_id: "hall-entry-v1".into(),
                    rule_version: 1,
                    character_ids: vec!["player".into()],
                },
            ),
        ]),
        vec![PresenceChangeRule {
            rule_id: "keeper-presence-v1".into(),
            rule_version: 1,
            scene_id: "room".into(),
            entity_scope: "global".into(),
            allow_enter: true,
            allow_leave: true,
        }],
    )
    .unwrap()
    .with_knowledge_rules(
        vec![
            ObservationRule {
                rule_id: "room-observation-v1".into(),
                rule_version: 1,
                scene_id: "room".into(),
                scope_id: "room-visible-v1".into(),
                source_kind: ObservationSourceKind::SceneNarration,
            },
            ObservationRule {
                rule_id: "hall-observation-v1".into(),
                rule_version: 1,
                scene_id: "hall".into(),
                scope_id: "hall-visible-v1".into(),
                source_kind: ObservationSourceKind::SceneNarration,
            },
        ],
        vec![RumorRule {
            rule_id: "room-rumor-v1".into(),
            rule_version: 1,
            scene_id: "room".into(),
            scope_id: "room-rumor-v1".into(),
            require_informant_present: true,
        }],
    )
    .unwrap();
    let catalog = SceneCatalog::new(
        catalog_revision.into(),
        script_revision.into(),
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
        BTreeMap::from([
            (
                "room".into(),
                Scene {
                    position,
                    rules: SceneRuleView {
                        scene_id: "room".into(),
                        catalog_revision: catalog_revision.into(),
                        advance_rule_ids: vec![],
                        progress_rule_ids: vec![],
                        hint_rules: vec![],
                        exit_rule_ids: vec![],
                    },
                    advance: vec![],
                    check_rule_ids: vec![],
                    actor_ids: vec!["player".into(), "keeper".into(), "messenger".into()],
                    forced_check: None,
                    dice_disabled: true,
                },
            ),
            (
                "hall".into(),
                Scene {
                    position: ScenePosition {
                        scene_id: "hall".into(),
                        path: vec![SceneNode {
                            kind: "scene".into(),
                            id: "hall".into(),
                            title: "走廊".into(),
                        }],
                    },
                    rules: SceneRuleView {
                        scene_id: "hall".into(),
                        catalog_revision: catalog_revision.into(),
                        advance_rule_ids: vec![],
                        progress_rule_ids: vec![],
                        hint_rules: vec![],
                        exit_rule_ids: vec![],
                    },
                    advance: vec![],
                    check_rule_ids: vec![],
                    actor_ids: vec!["player".into(), "keeper".into(), "messenger".into()],
                    forced_check: None,
                    dice_disabled: true,
                },
            ),
        ]),
        BTreeMap::new(),
    )
    .unwrap()
    .with_entity_directory(directory.clone())
    .unwrap()
    .with_presence_policy(policy)
    .unwrap();
    (catalog, directory)
}

fn port(session: &Session) -> Port {
    let (catalog, directory) = catalog(&session.header().script_revision);
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(aoidos_store::applied::SCHEMA)
        .unwrap();
    Port {
        connection,
        catalog,
        directory,
        fail_apply: false,
        fail_apply_on: None,
        applied_count: 0,
    }
}

fn commit_presence_change(fixture: &mut Fixture, port: &mut Port, mutation: WorldMutation) {
    let narrative_seq = fixture
        .session
        .append(Body::Narration {
            text: "门外传来脚步声。".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    fixture
        .session
        .commit_world(
            Fact::SettlementPlanned {
                identity: RoundIdentity {
                    version: 1,
                    round_id: uuid::Uuid::new_v4().to_string(),
                    operation_id: uuid::Uuid::new_v4().to_string(),
                },
                narrative_seq,
                world_revision: "world-v1".into(),
                mutations: vec![mutation],
            }
            .body("可信在场变更")
            .unwrap(),
            port,
        )
        .unwrap();
}

fn append_completed_character_line(session: &mut Session, speaker_id: &str, text: &str) -> u64 {
    session
        .append(Body::CharacterSpeech {
            speaker_id: speaker_id.into(),
            text: text.into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap()
}

fn append_completed_narration(session: &mut Session, text: &str) -> u64 {
    session
        .append(Body::Narration {
            text: text.into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap()
}

fn append_failed_source_round(session: &mut Session, text: &str) -> u64 {
    let input_seq = session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "我想起一段后来失败的行动".into(),
            mode: Some(crate::record::format::InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let identity = RoundIdentity {
        version: 1,
        round_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    session
        .append(
            Fact::RoundAccepted {
                identity: identity.clone(),
                input_seq,
                mode: crate::record::format::InputMode::InCharacter,
                dice_mode: DiceMode::Manual,
                profile_id: "default".into(),
                profile_revision: "1".into(),
                target: GenerationTarget::Narration,
                scene_id: "room".into(),
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("接纳失败回合")
            .unwrap(),
        )
        .unwrap();
    let source_seq = append_completed_narration(session, text);
    session
        .append(
            Fact::RoundEnded {
                identity,
                outcome: crate::ports::Outcome::Failed,
                through_seq: source_seq,
                completed_steps: vec![Step::Check, Step::Narration],
                error: Some(crate::fault::Fault::new(
                    "engine.invalid-phase",
                    "行动未完成",
                )),
            }
            .body("失败回合")
            .unwrap(),
        )
        .unwrap();
    source_seq
}

fn evidence_source(seq: u64, text: &str) -> EvidenceSource {
    EvidenceSource {
        record_seq: seq,
        body_hash: format::hash(text.as_bytes()),
        start_byte: 0,
        end_byte: text.len() as u64,
    }
}

#[test]
fn snapshots_include_only_explicitly_present_catalog_characters() {
    let fixture = Fixture::new();
    let port = port(&fixture.session);
    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();

    assert_eq!(snapshot.scene_id, "room");
    assert_eq!(snapshot.characters.len(), 1);
    assert_eq!(snapshot.characters[0].id, "player");
    assert!(snapshot.presence_revision.starts_with("sha256:"));
}

#[test]
fn presence_snapshot_rejects_catalog_or_entity_directory_from_another_script() {
    let fixture = Fixture::new();
    let port = port(&fixture.session);
    let (mismatched_catalog, _) = catalog(&crate::record::format::hash(b"other-script"));
    assert!(
        ScenePresence::snapshot(
            &port.connection,
            &fixture.session,
            &mismatched_catalog,
            &port.directory,
        )
        .is_err()
    );

    let mismatched_directory = EntityDirectory::new(
        crate::record::format::hash(b"other-script"),
        1,
        true,
        vec![
            CanonicalEntity {
                id: "player".into(),
                kind: EntityKind::Character,
                scope: "global".into(),
                canonical_name: "玩家".into(),
            },
            CanonicalEntity {
                id: "keeper".into(),
                kind: EntityKind::Character,
                scope: "global".into(),
                canonical_name: "守门人".into(),
            },
        ],
    )
    .unwrap();
    assert!(
        ScenePresence::snapshot(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &mismatched_directory,
        )
        .is_err()
    );
}

#[test]
fn applied_presence_changes_update_snapshot_and_stale_revisions_are_rejected() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let stale = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, stale.clone());

    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    assert_eq!(snapshot.characters.len(), 2);
    assert_ne!(
        snapshot.presence_revision,
        serde_json::from_value::<PresenceChange>(stale.data.payload.clone())
            .unwrap()
            .expected_presence_revision
    );

    let narrative_seq = fixture
        .session
        .append(Body::Narration {
            text: "玩家又尝试让守门人进场。".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    let error = fixture
        .session
        .commit_world(
            Fact::SettlementPlanned {
                identity: RoundIdentity {
                    version: 1,
                    round_id: uuid::Uuid::new_v4().to_string(),
                    operation_id: uuid::Uuid::new_v4().to_string(),
                },
                narrative_seq,
                world_revision: "world-v1".into(),
                mutations: vec![stale],
            }
            .body("过期在场意图")
            .unwrap(),
            &mut port,
        )
        .unwrap_err();
    assert_eq!(error.code, "engine.invalid-phase");
}

#[test]
fn committed_presence_change_requires_its_original_catalog_revision() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let change = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, change);
    let (replacement_catalog, _) =
        catalog_version(&fixture.session.header().script_revision, "catalog-v2");
    assert_eq!(
        ScenePresence::snapshot(
            &port.connection,
            &fixture.session,
            &replacement_catalog,
            &port.directory,
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase"
    );
    assert_eq!(
        ScenePresence::snapshot(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
        )
        .unwrap()
        .characters
        .len(),
        2
    );
}

#[test]
fn policy_requires_complete_character_directory_and_registered_scenes() {
    let script_revision = crate::record::format::hash(b"presence-policy");
    let (catalog, directory) = catalog(&script_revision);
    let policy = catalog.presence_policy().unwrap();
    assert_eq!(policy.initial_scene_id(), "room");
    assert_eq!(policy.initial().character_ids.first().unwrap(), "player");
    assert_eq!(policy.entry("room").unwrap().rule_id, "room-entry-v1");
    assert!(policy.entry("missing").is_none());
    assert!(policy.change_rule("keeper-presence-v1").is_some());
    assert!(policy.change_rule("missing").is_none());
    assert!(policy.observation_rule("room-observation-v1").is_some());
    assert!(policy.observation_rule("missing").is_none());
    assert!(policy.rumor_rule("room-rumor-v1").is_some());
    assert!(policy.rumor_rule("missing").is_none());
    let incomplete = EntityDirectory::new(
        script_revision.clone(),
        1,
        false,
        vec![CanonicalEntity {
            id: "player".into(),
            kind: EntityKind::Character,
            scope: "global".into(),
            canonical_name: "玩家".into(),
        }],
    )
    .unwrap();
    assert!(
        catalog
            .presence_policy()
            .unwrap()
            .validate_directory(&incomplete)
            .is_err()
    );
    assert!(
        catalog
            .with_presence_policy(
                PresencePolicy::new(
                    "missing".into(),
                    SceneEntryPresence {
                        rule_id: "missing-entry-v1".into(),
                        rule_version: 1,
                        character_ids: vec!["player".into()],
                    },
                    BTreeMap::from([(
                        "missing".into(),
                        SceneEntryPresence {
                            rule_id: "missing-entry-v1".into(),
                            rule_version: 1,
                            character_ids: vec!["player".into()],
                        },
                    )]),
                    vec![],
                )
                .unwrap(),
            )
            .is_err()
    );
    assert!(directory.is_complete());
}

#[test]
fn presence_policy_enforces_encoded_byte_budget() {
    let character_ids = (0..MAX_CHARACTERS)
        .map(|index| format!("{index:064}"))
        .collect::<Vec<_>>();
    let entry = SceneEntryPresence {
        rule_id: "r".repeat(64),
        rule_version: 1,
        character_ids,
    };
    let entries = (0..1024)
        .map(|index| (format!("scene-{index}"), entry.clone()))
        .collect();
    assert!(
        PresencePolicy::new("room".into(), entry.clone(), entries, vec![]).is_err(),
        "a valid-count policy can still exceed its serialized byte budget"
    );
}

#[test]
fn presence_snapshot_checks_bytes_even_when_character_count_is_valid() {
    let fixture = Fixture::new();
    let port = port(&fixture.session);
    for name_bytes in [8, 256] {
        let mut entities = ["player", "keeper", "messenger"]
            .into_iter()
            .map(|id| port.directory.entity(id).unwrap().unwrap().clone())
            .collect::<Vec<_>>();
        entities.extend(
            (0..MAX_CHARACTERS - entities.len()).map(|index| CanonicalEntity {
                id: format!("character-{index:04}"),
                kind: EntityKind::Character,
                scope: "\\".repeat(128),
                canonical_name: format!("{index:0name_bytes$}"),
            }),
        );
        let entry = SceneEntryPresence {
            rule_id: "room-entry-v1".into(),
            rule_version: 1,
            character_ids: entities.iter().map(|entity| entity.id.clone()).collect(),
        };
        entities.push(CanonicalEntity {
            id: "outside-character".into(),
            kind: EntityKind::Character,
            scope: "global".into(),
            canonical_name: "场外角色".into(),
        });
        let directory = EntityDirectory::new(
            fixture.session.header().script_revision.clone(),
            1,
            true,
            entities,
        )
        .unwrap();
        let policy = PresencePolicy::new(
            "room".into(),
            entry.clone(),
            BTreeMap::from([("room".into(), entry.clone()), ("hall".into(), entry)]),
            vec![PresenceChangeRule {
                rule_id: "capacity-v1".into(),
                rule_version: 1,
                scene_id: "room".into(),
                entity_scope: "global".into(),
                allow_enter: true,
                allow_leave: true,
            }],
        )
        .unwrap();
        let (base_catalog, _) = catalog(&fixture.session.header().script_revision);
        let catalog = base_catalog
            .with_entity_directory(directory.clone())
            .unwrap()
            .with_presence_policy(policy)
            .unwrap();
        let snapshot =
            ScenePresence::snapshot(&port.connection, &fixture.session, &catalog, &directory);
        if name_bytes == 8 {
            assert_eq!(snapshot.unwrap().characters.len(), MAX_CHARACTERS);
            let overflow = ScenePresence::prepare_change(
                &port.connection,
                &fixture.session,
                &catalog,
                &directory,
                "capacity-v1",
                "outside-character",
                PresenceAction::Enter,
            )
            .unwrap();
            assert!(
                ScenePresence::validate_mutations(
                    &port.connection,
                    &fixture.session,
                    &catalog,
                    &directory,
                    &[overflow],
                )
                .is_err(),
                "the 513th character is rejected before publishing a snapshot"
            );
            assert_eq!(
                ScenePresence::snapshot(&port.connection, &fixture.session, &catalog, &directory,)
                    .unwrap()
                    .characters
                    .len(),
                MAX_CHARACTERS
            );
        } else {
            let error = snapshot.unwrap_err();
            assert_eq!(error.code, "app.bad-request");
            assert_eq!(error.detail.unwrap()["reason"], "presenceSnapshotLimit");
        }
    }
}

#[test]
fn presence_policy_rejects_invalid_entries_rules_and_capacity_overflow() {
    let initial = SceneEntryPresence {
        rule_id: "room-entry-v1".into(),
        rule_version: 1,
        character_ids: vec!["player".into()],
    };
    let entries = BTreeMap::from([("room".into(), initial.clone())]);
    let change = |rule_id: String| PresenceChangeRule {
        rule_id,
        rule_version: 1,
        scene_id: "room".into(),
        entity_scope: "global".into(),
        allow_enter: true,
        allow_leave: true,
    };

    assert!(
        PresencePolicy::new("bad scene".into(), initial.clone(), entries.clone(), vec![]).is_err()
    );
    assert!(PresencePolicy::new("room".into(), initial.clone(), BTreeMap::new(), vec![]).is_err());
    assert!(
        PresencePolicy::new(
            "room".into(),
            initial.clone(),
            BTreeMap::from([("bad scene".into(), initial.clone())]),
            vec![]
        )
        .is_err()
    );

    let mut invalid_entry = initial.clone();
    invalid_entry.rule_version = 0;
    assert!(PresencePolicy::new("room".into(), invalid_entry, entries.clone(), vec![]).is_err());
    let mut invalid_character_id = initial.clone();
    invalid_character_id.character_ids = vec!["bad character".into()];
    assert!(
        PresencePolicy::new("room".into(), invalid_character_id, entries.clone(), vec![]).is_err()
    );
    let mut duplicate_entry = initial.clone();
    duplicate_entry.character_ids.push("player".into());
    assert!(PresencePolicy::new("room".into(), duplicate_entry, entries.clone(), vec![]).is_err());
    let oversized_entry = SceneEntryPresence {
        character_ids: (0..=MAX_CHARACTERS)
            .map(|index| format!("character-{index}"))
            .collect(),
        ..initial.clone()
    };
    assert!(PresencePolicy::new("room".into(), oversized_entry, entries.clone(), vec![]).is_err());

    let mut invalid_change = change("bad rule".into());
    invalid_change.rule_id = "bad rule".into();
    assert!(
        PresencePolicy::new(
            "room".into(),
            initial.clone(),
            entries.clone(),
            vec![invalid_change]
        )
        .is_err()
    );
    let mut invalid_change = change("change-v1".into());
    invalid_change.scene_id = "bad scene".into();
    assert!(
        PresencePolicy::new(
            "room".into(),
            initial.clone(),
            entries.clone(),
            vec![invalid_change]
        )
        .is_err()
    );
    let mut invalid_change = change("change-v1".into());
    invalid_change.entity_scope = "\n".into();
    assert!(
        PresencePolicy::new(
            "room".into(),
            initial.clone(),
            entries.clone(),
            vec![invalid_change]
        )
        .is_err()
    );
    let mut invalid_change = change("change-v1".into());
    invalid_change.rule_version = 0;
    assert!(
        PresencePolicy::new(
            "room".into(),
            initial.clone(),
            entries.clone(),
            vec![invalid_change]
        )
        .is_err()
    );
    let mut invalid_change = change("change-v1".into());
    invalid_change.allow_enter = false;
    invalid_change.allow_leave = false;
    assert!(
        PresencePolicy::new(
            "room".into(),
            initial.clone(),
            entries.clone(),
            vec![invalid_change]
        )
        .is_err()
    );
    assert!(
        PresencePolicy::new(
            "room".into(),
            initial.clone(),
            entries.clone(),
            vec![change("same-v1".into()), change("same-v1".into())]
        )
        .is_err()
    );
    let too_many_changes = (0..=MAX_RULES)
        .map(|index| change(format!("change-{index}")))
        .collect();
    assert!(
        PresencePolicy::new(
            "room".into(),
            initial.clone(),
            entries.clone(),
            too_many_changes
        )
        .is_err()
    );

    let too_many_characters = SceneEntryPresence {
        character_ids: (0..=MAX_CHARACTERS)
            .map(|index| format!("character-{index}"))
            .collect(),
        ..initial.clone()
    };
    let too_many_scenes = (0..=1024)
        .map(|index| (format!("scene-{index}"), initial.clone()))
        .collect();
    assert!(PresencePolicy::new("room".into(), initial, too_many_scenes, vec![]).is_err());
    assert!(PresencePolicy::new("room".into(), too_many_characters, entries, vec![]).is_err());
}

#[test]
fn knowledge_rules_reject_invalid_identity_duplicates_and_total_rule_overflow() {
    let initial = SceneEntryPresence {
        rule_id: "room-entry-v1".into(),
        rule_version: 1,
        character_ids: vec!["player".into()],
    };
    let policy = PresencePolicy::new(
        "room".into(),
        initial.clone(),
        BTreeMap::from([("room".into(), initial)]),
        vec![PresenceChangeRule {
            rule_id: "keeper-presence-v1".into(),
            rule_version: 1,
            scene_id: "room".into(),
            entity_scope: "global".into(),
            allow_enter: true,
            allow_leave: true,
        }],
    )
    .unwrap();
    let observation = |rule_id: &str| ObservationRule {
        rule_id: rule_id.into(),
        rule_version: 1,
        scene_id: "room".into(),
        scope_id: "visible-v1".into(),
        source_kind: ObservationSourceKind::SceneNarration,
    };
    let rumor = |rule_id: &str| RumorRule {
        rule_id: rule_id.into(),
        rule_version: 1,
        scene_id: "room".into(),
        scope_id: "rumor-v1".into(),
        require_informant_present: true,
    };

    assert!(
        policy
            .clone()
            .with_knowledge_rules(vec![observation("bad rule")], vec![])
            .is_err()
    );
    let invalid_version = ObservationRule {
        rule_version: 0,
        ..observation("invalid-version")
    };
    let invalid_scene = ObservationRule {
        scene_id: "bad scene".into(),
        ..observation("invalid-scene")
    };
    let invalid_scope = ObservationRule {
        scope_id: "\n".into(),
        ..observation("invalid-scope")
    };
    for invalid in [invalid_version, invalid_scene, invalid_scope] {
        assert!(
            policy
                .clone()
                .with_knowledge_rules(vec![invalid], vec![])
                .is_err()
        );
    }
    assert!(
        policy
            .clone()
            .with_knowledge_rules(vec![observation("keeper-presence-v1")], vec![])
            .is_err()
    );
    assert!(
        policy
            .clone()
            .with_knowledge_rules(
                vec![observation("shared-id"), observation("other-id")],
                vec![rumor("shared-id")]
            )
            .is_err()
    );
    assert!(
        policy
            .clone()
            .with_knowledge_rules(
                vec![observation("duplicate-v1"), observation("duplicate-v1")],
                vec![]
            )
            .is_err()
    );
    assert!(
        policy
            .clone()
            .with_knowledge_rules(vec![], vec![rumor("duplicate-v1"), rumor("duplicate-v1")])
            .is_err()
    );
    let too_many_observations = (0..=MAX_RULES)
        .map(|index| observation(&format!("observation-{index}")))
        .collect();
    assert!(
        policy
            .with_knowledge_rules(too_many_observations, vec![])
            .is_err()
    );

    let initial = SceneEntryPresence {
        rule_id: "entry-v1".into(),
        rule_version: 1,
        character_ids: vec!["player".into()],
    };
    let byte_limited_policy = PresencePolicy::new(
        "room".into(),
        initial.clone(),
        BTreeMap::from([("room".into(), initial)]),
        vec![],
    )
    .unwrap();
    let oversized_rules = (0..MAX_RULES)
        .map(|index| ObservationRule {
            rule_id: format!("r{index:063}"),
            rule_version: 1,
            scene_id: "s".repeat(64),
            scope_id: "x".repeat(128),
            source_kind: ObservationSourceKind::SceneNarration,
        })
        .collect();
    assert!(
        byte_limited_policy
            .with_knowledge_rules(oversized_rules, vec![])
            .is_err()
    );
}

#[test]
fn presence_change_preparation_enforces_rule_scene_and_membership() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let enter = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, enter);

    assert!(
        ScenePresence::prepare_change(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            "keeper-presence-v1",
            "keeper",
            PresenceAction::Enter,
        )
        .is_err()
    );
    assert!(
        ScenePresence::prepare_change(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            "unknown-rule",
            "keeper",
            PresenceAction::Leave,
        )
        .is_err()
    );

    let leave = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Leave,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, leave);
    assert!(
        ScenePresence::prepare_change(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            "keeper-presence-v1",
            "keeper",
            PresenceAction::Leave,
        )
        .is_err()
    );
    assert!(
        ScenePresence::prepare_change(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            "keeper-presence-v1",
            "missing-character",
            PresenceAction::Enter,
        )
        .is_err()
    );
}

#[test]
fn scene_advance_requires_a_registered_scene_entry() {
    let (catalog, _) = catalog(&crate::record::format::hash(b"scene-advance"));
    let position = ScenePosition {
        scene_id: "room".into(),
        path: vec![SceneNode {
            kind: "scene".into(),
            id: "room".into(),
            title: "房间".into(),
        }],
    };
    assert!(ScenePresence::validate_scene_advance(&catalog, &position).is_ok());

    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    fixture
        .session
        .commit_world(
            Fact::SceneAdvanced {
                identity: RoundIdentity {
                    version: 1,
                    round_id: uuid::Uuid::new_v4().to_string(),
                    operation_id: uuid::Uuid::new_v4().to_string(),
                },
                previous_scene_id: "room".into(),
                position: position.clone(),
                world_revision: "world-v1".into(),
                mutation_id: uuid::Uuid::new_v4().to_string(),
            }
            .body("切换到已登记场景")
            .unwrap(),
            &mut port,
        )
        .unwrap();
    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    assert_eq!(snapshot.scene_id, "room");

    let missing_entry = ScenePosition {
        scene_id: "unregistered".into(),
        path: vec![],
    };
    assert!(ScenePresence::validate_scene_advance(&catalog, &missing_entry).is_err());
}

#[test]
fn ended_session_does_not_produce_a_live_presence_snapshot() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    fixture
        .session
        .commit_world(
            Fact::SessionEnded {
                identity: RoundIdentity {
                    version: 1,
                    round_id: uuid::Uuid::new_v4().to_string(),
                    operation_id: uuid::Uuid::new_v4().to_string(),
                },
                previous_scene_id: "room".into(),
                reason_id: "campaign-ended".into(),
                world_revision: "world-v1".into(),
                mutation_id: uuid::Uuid::new_v4().to_string(),
            }
            .body("轮回结束")
            .unwrap(),
            &mut port,
        )
        .unwrap();
    assert!(
        ScenePresence::snapshot(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
        )
        .is_err()
    );
}

#[test]
fn pending_world_intent_blocks_presence_reads_and_new_mutations() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    port.fail_apply = true;
    let position = ScenePosition {
        scene_id: "room".into(),
        path: vec![SceneNode {
            kind: "scene".into(),
            id: "room".into(),
            title: "房间".into(),
        }],
    };
    assert!(
        fixture
            .session
            .commit_world(
                Fact::SceneAdvanced {
                    identity: RoundIdentity {
                        version: 1,
                        round_id: uuid::Uuid::new_v4().to_string(),
                        operation_id: uuid::Uuid::new_v4().to_string(),
                    },
                    previous_scene_id: "room".into(),
                    position,
                    world_revision: "world-v1".into(),
                    mutation_id: uuid::Uuid::new_v4().to_string(),
                }
                .body("注入应用失败")
                .unwrap(),
                &mut port,
            )
            .is_err()
    );

    assert!(
        ScenePresence::snapshot(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
        )
        .is_err()
    );
    assert!(
        fixture
            .session
            .bounded_current_records(MAX_EVIDENCE_RECORDS, MAX_EVIDENCE_BYTES)
            .is_ok()
    );
    assert_eq!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase"
    );
    assert!(
        ScenePresence::prepare_change(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            "keeper-presence-v1",
            "keeper",
            PresenceAction::Enter,
        )
        .is_err()
    );
}

#[test]
fn recovery_validation_rebuilds_the_root_branch_baseline_before_an_intent() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let narrative_seq = fixture
        .session
        .append(Body::Narration {
            text: "守门人走近。".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    let mutation = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    let identity = RoundIdentity {
        version: 1,
        round_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let record_seq = fixture
        .session
        .commit_world(
            Fact::SettlementPlanned {
                identity,
                narrative_seq,
                world_revision: "world-v1".into(),
                mutations: vec![mutation.clone()],
            }
            .body("在场变更意图")
            .unwrap(),
            &mut port,
        )
        .unwrap();

    assert!(
        ScenePresence::validate_recovery_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            record_seq,
            &[mutation],
        )
        .is_ok()
    );
    assert!(
        ScenePresence::validate_recovery_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            record_seq + 1,
            &[],
        )
        .is_err()
    );
}

#[test]
fn recovery_after_rewind_preserves_applied_presence_from_the_root_branch() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let identity = begin_round(&mut fixture.session);
    let source_seq = append_completed_narration(&mut fixture.session, "守门人进入房间。");
    let change = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    let target = finish_round(
        &mut fixture.session,
        &mut port,
        identity,
        source_seq,
        vec![change],
    );
    let discarded = append_completed_narration(&mut fixture.session, "将被回退的后续内容。");
    let fork = crate::game::control::prepare_fork(
        &fixture.session,
        &uuid::Uuid::new_v4().to_string(),
        crate::record::facts::ForkMode::Rewind,
        target,
        None,
    )
    .unwrap();
    fixture
        .session
        .commit_history_fork(fork, &mut port)
        .unwrap();
    let branch = fixture.session.history_revision();
    assert!(branch > 0);
    assert!(
        !fixture
            .session
            .path_at(branch)
            .unwrap()
            .contains(&discarded)
    );
    fixture.session = Session::open(fixture.path.clone(), fixture.events.clone()).unwrap();
    fixture.session.recover_history(&mut port).unwrap();
    let applied_before = port.applied_count;
    fixture.session.recover_world(&mut port).unwrap();
    fixture.session.recover_world(&mut port).unwrap();
    assert_eq!(port.applied_count, applied_before);
    assert!(!fixture.session.needs_recovery());
    let records = fixture
        .session
        .bounded_current_records(MAX_EVIDENCE_RECORDS, MAX_EVIDENCE_BYTES)
        .unwrap();
    assert_eq!(records.len(), 8);
    assert!(!records.contains_key(&branch));
    assert!(!records.contains_key(&discarded));
    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    assert_eq!(snapshot.scene_id, "room");
    assert_eq!(
        snapshot
            .characters
            .iter()
            .map(|entity| entity.id.as_str())
            .collect::<Vec<_>>(),
        ["keeper", "player"]
    );
}

#[test]
fn observed_evidence_requires_the_subject_to_be_present_and_a_registered_rule() {
    let mut fixture = Fixture::new();
    let port = port(&fixture.session);
    let text = "玩家察觉到石门上有新鲜划痕。";
    let source_seq = append_completed_narration(&mut fixture.session, text);
    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .observation_rule("room-observation-v1")
        .unwrap();
    let valid = ObservedEvent::mutation(
        &fixture.session,
        &port.catalog,
        ObservedEvidenceInput {
            subject_id: "player".into(),
            source: evidence_source(source_seq, text),
            scene_id: snapshot.scene_id,
            scope_id: rule.scope_id.clone(),
            presence_revision: snapshot.presence_revision.clone(),
        },
        rule,
    )
    .unwrap();
    ScenePresence::validate_mutations(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        std::slice::from_ref(&valid),
    )
    .unwrap();

    let absent_subject = ObservedEvent::mutation(
        &fixture.session,
        &port.catalog,
        ObservedEvidenceInput {
            subject_id: "keeper".into(),
            source: evidence_source(source_seq, text),
            scene_id: "room".into(),
            scope_id: rule.scope_id.clone(),
            presence_revision: ScenePresence::snapshot(
                &port.connection,
                &fixture.session,
                &port.catalog,
                &port.directory,
            )
            .unwrap()
            .presence_revision,
        },
        rule,
    )
    .unwrap();
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[absent_subject],
        )
        .is_err()
    );
}

#[test]
fn observed_evidence_rejects_malformed_payloads_and_untrusted_source_ranges() {
    let mut fixture = Fixture::new();
    let port = port(&fixture.session);
    let text = "玩家看到门上刻着旧字。";
    let source_seq = append_completed_narration(&mut fixture.session, text);
    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .observation_rule("room-observation-v1")
        .unwrap();
    let valid = ObservedEvent::mutation(
        &fixture.session,
        &port.catalog,
        ObservedEvidenceInput {
            subject_id: "player".into(),
            source: evidence_source(source_seq, text),
            scene_id: "room".into(),
            scope_id: rule.scope_id.clone(),
            presence_revision: snapshot.presence_revision,
        },
        rule,
    )
    .unwrap();

    let mut unsupported_version = valid.clone();
    unsupported_version.data.version = 2;
    let mut malformed_shape = valid.clone();
    malformed_shape.data.payload = serde_json::json!({ "evidenceId": valid.mutation_id });
    for malformed in [unsupported_version, malformed_shape] {
        assert!(
            ScenePresence::validate_mutations(
                &port.connection,
                &fixture.session,
                &port.catalog,
                &port.directory,
                &[malformed],
            )
            .is_err()
        );
    }

    let invalid_ranges = [
        (0, 0),
        (1, text.len() as u64),
        (0, text.len() as u64 + 1),
        (0, 32 * 1024 + 1),
    ];
    for (start_byte, end_byte) in invalid_ranges {
        let mut malformed = valid.clone();
        malformed.data.payload["source"]["startByte"] = serde_json::json!(start_byte);
        malformed.data.payload["source"]["endByte"] = serde_json::json!(end_byte);
        assert!(
            ScenePresence::validate_mutations(
                &port.connection,
                &fixture.session,
                &port.catalog,
                &port.directory,
                &[malformed],
            )
            .is_err(),
            "invalid byte range {start_byte}..{end_byte} must not be trusted"
        );
    }

    let mut mismatched_hash = valid.clone();
    mismatched_hash.data.payload["source"]["bodyHash"] =
        serde_json::json!(format::hash(b"different committed text"));
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[mismatched_hash],
        )
        .is_err()
    );

    let mut missing_source = valid;
    missing_source.data.payload["source"]["recordSeq"] = serde_json::json!(u64::MAX);
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[missing_source],
        )
        .is_err()
    );
}

#[test]
fn duplicate_observed_evidence_identity_is_rejected_within_one_settlement() {
    let mut fixture = Fixture::new();
    let port = port(&fixture.session);
    let text = "玩家辨认出桌上的徽记。";
    let source_seq = append_completed_narration(&mut fixture.session, text);
    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .observation_rule("room-observation-v1")
        .unwrap();
    let event = ObservedEvent::mutation(
        &fixture.session,
        &port.catalog,
        ObservedEvidenceInput {
            subject_id: "player".into(),
            source: evidence_source(source_seq, text),
            scene_id: "room".into(),
            scope_id: rule.scope_id.clone(),
            presence_revision: snapshot.presence_revision,
        },
        rule,
    )
    .unwrap();

    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[event.clone(), event],
        )
        .is_err()
    );
}

#[test]
fn observed_evidence_cannot_use_narration_from_another_scene() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let text = "玩家看見房間牆上刻著舊字。";
    let source_seq = append_completed_narration(&mut fixture.session, text);
    fixture
        .session
        .commit_world(
            Fact::SceneAdvanced {
                identity: RoundIdentity {
                    version: 1,
                    round_id: uuid::Uuid::new_v4().to_string(),
                    operation_id: uuid::Uuid::new_v4().to_string(),
                },
                previous_scene_id: "room".into(),
                position: ScenePosition {
                    scene_id: "hall".into(),
                    path: vec![SceneNode {
                        kind: "scene".into(),
                        id: "hall".into(),
                        title: "走廊".into(),
                    }],
                },
                world_revision: "world-v1".into(),
                mutation_id: uuid::Uuid::new_v4().to_string(),
            }
            .body("切换到走廊")
            .unwrap(),
            &mut port,
        )
        .unwrap();

    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .observation_rule("hall-observation-v1")
        .unwrap();
    let event = ObservedEvent::mutation(
        &fixture.session,
        &port.catalog,
        ObservedEvidenceInput {
            subject_id: "player".into(),
            source: evidence_source(source_seq, text),
            scene_id: "hall".into(),
            scope_id: rule.scope_id.clone(),
            presence_revision: snapshot.presence_revision,
        },
        rule,
    )
    .unwrap();

    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[event],
        )
        .is_err()
    );
}

#[test]
fn observed_evidence_rejects_player_input_as_a_narration_source() {
    let mut fixture = Fixture::new();
    let port = port(&fixture.session);
    let text = "我看到桌上刻着一串数字。";
    let source_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: text.into(),
            mode: Some(crate::record::format::InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .observation_rule("room-observation-v1")
        .unwrap();
    let event = ObservedEvent::mutation(
        &fixture.session,
        &port.catalog,
        ObservedEvidenceInput {
            subject_id: "player".into(),
            source: evidence_source(source_seq, text),
            scene_id: "room".into(),
            scope_id: rule.scope_id.clone(),
            presence_revision: snapshot.presence_revision,
        },
        rule,
    )
    .unwrap();

    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[event],
        )
        .is_err()
    );
}

#[test]
fn rumor_evidence_requires_present_receiver_and_matching_delivery_author() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let add_keeper = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, add_keeper);
    let add_messenger = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "messenger",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, add_messenger);

    let text = "守门人向玩家低声透露，旧井下方有回声。";
    let source_seq = append_completed_character_line(&mut fixture.session, "keeper", text);
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .rumor_rule("room-rumor-v1")
        .unwrap();
    let unknown_recipient = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "unknown-character".into(),
        "keeper".into(),
        evidence_source(source_seq, text),
        evidence_source(source_seq, text),
        None,
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[unknown_recipient],
        )
        .is_err()
    );
    let valid = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "player".into(),
        "keeper".into(),
        evidence_source(source_seq, text),
        evidence_source(source_seq, text),
        None,
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[valid.clone(), valid.clone()],
        )
        .is_err(),
        "a settlement cannot publish the same rumor identity twice"
    );
    ScenePresence::validate_mutations(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        std::slice::from_ref(&valid),
    )
    .unwrap();

    let relay_text = "玩家想把消息还给守门人。";
    let relay_seq = append_completed_character_line(&mut fixture.session, "player", relay_text);
    let cycle = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "keeper".into(),
        "player".into(),
        evidence_source(relay_seq, relay_text),
        evidence_source(source_seq, text),
        Some(valid.mutation_id.clone()),
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[valid.clone(), cycle],
        )
        .is_err(),
        "a rumor chain cannot return to its root informant"
    );

    let mismatched_author = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "player".into(),
        "player".into(),
        evidence_source(source_seq, text),
        evidence_source(source_seq, text),
        None,
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[mismatched_author],
        )
        .is_err()
    );

    let messenger_text = "信使也听到了井下的回声。";
    let messenger_seq =
        append_completed_character_line(&mut fixture.session, "messenger", messenger_text);
    let wrong_root = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "player".into(),
        "messenger".into(),
        evidence_source(messenger_seq, messenger_text),
        evidence_source(source_seq, text),
        None,
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[wrong_root],
        )
        .is_err()
    );
}

#[test]
fn rumor_sources_accept_in_character_player_speech_but_reject_other_bodies() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let add_keeper = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, add_keeper);

    let in_character_text = "我把井下有回声的消息告诉守门人。";
    let in_character_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: in_character_text.into(),
            mode: Some(crate::record::format::InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let out_of_character_text = "玩家想讨论规则，不是角色发言。";
    let out_of_character_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: out_of_character_text.into(),
            mode: Some(crate::record::format::InputMode::OutOfCharacter),
            content_range: None,
        })
        .unwrap();
    let narration_text = "旁白提到井下有回声。";
    let narration_seq = append_completed_narration(&mut fixture.session, narration_text);
    let non_text_seq = fixture
        .session
        .append(
            Fact::CheckSkipped {
                identity: RoundIdentity {
                    version: 1,
                    round_id: uuid::Uuid::new_v4().to_string(),
                    operation_id: uuid::Uuid::new_v4().to_string(),
                },
                reason: SkipReason::NoCheck,
                world_revision: "world-v1".into(),
            }
            .body("无检定记录")
            .unwrap(),
        )
        .unwrap();
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .rumor_rule("room-rumor-v1")
        .unwrap();
    let event = |source: EvidenceSource, informant: &str| {
        RumorDelivery::mutation(
            &fixture.session,
            &port.catalog,
            "keeper".into(),
            informant.into(),
            source.clone(),
            source,
            None,
            "room".into(),
            rule.scope_id.clone(),
            RumorUncertainty::Unverified,
            rule,
        )
        .unwrap()
    };
    let validate = |event: &WorldMutation| {
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            std::slice::from_ref(event),
        )
    };

    let valid_player_source = event(
        evidence_source(in_character_seq, in_character_text),
        "player",
    );
    validate(&valid_player_source).unwrap();
    for invalid in [
        event(
            evidence_source(out_of_character_seq, out_of_character_text),
            "player",
        ),
        event(evidence_source(narration_seq, narration_text), "player"),
        event(evidence_source(non_text_seq, "no text"), "keeper"),
    ] {
        assert!(validate(&invalid).is_err());
    }
}

#[test]
fn rumor_delivery_chain_accepts_eight_hops_and_rejects_a_ninth() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let relay_ids = (1..=7)
        .map(|index| format!("relay{index:02}"))
        .collect::<Vec<_>>();
    let mut entities = vec![
        ("player", "玩家"),
        ("keeper", "守门人"),
        ("messenger", "信使"),
    ]
    .into_iter()
    .map(|(id, name)| CanonicalEntity {
        id: id.into(),
        kind: EntityKind::Character,
        scope: "global".into(),
        canonical_name: name.into(),
    })
    .collect::<Vec<_>>();
    entities.extend(
        relay_ids
            .iter()
            .enumerate()
            .map(|(index, id)| CanonicalEntity {
                id: id.clone(),
                kind: EntityKind::Character,
                scope: "global".into(),
                canonical_name: format!("传递者{}", index + 1),
            }),
    );
    port.directory = EntityDirectory::new(
        fixture.session.header().script_revision.clone(),
        1,
        true,
        entities,
    )
    .unwrap();

    for id in std::iter::once("keeper".to_owned())
        .chain(std::iter::once("messenger".to_owned()))
        .chain(relay_ids.iter().cloned())
    {
        let mutation = ScenePresence::prepare_change(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            "keeper-presence-v1",
            &id,
            PresenceAction::Enter,
        )
        .unwrap();
        commit_presence_change(&mut fixture, &mut port, mutation);
    }

    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .rumor_rule("room-rumor-v1")
        .unwrap();
    let root_text = "守门人告诉玩家，古井深处有回声。";
    let root_seq = append_completed_character_line(&mut fixture.session, "keeper", root_text);
    let root_source = evidence_source(root_seq, root_text);
    let root = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "player".into(),
        "keeper".into(),
        root_source.clone(),
        root_source.clone(),
        None,
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    let mut mutations = vec![root.clone()];
    let recipients = std::iter::once("messenger".to_owned())
        .chain(relay_ids.iter().take(6).cloned())
        .collect::<Vec<_>>();
    let informants = std::iter::once("player".to_owned())
        .chain(std::iter::once("messenger".to_owned()))
        .chain(relay_ids.iter().take(5).cloned())
        .collect::<Vec<_>>();
    let mut previous = root;
    for (index, (recipient, informant)) in recipients.iter().zip(informants).enumerate() {
        let text = format!("{informant}把这条消息继续传给{recipient}。");
        let source_seq = append_completed_character_line(&mut fixture.session, &informant, &text);
        let source = evidence_source(source_seq, &text);
        let next = RumorDelivery::mutation(
            &fixture.session,
            &port.catalog,
            recipient.clone(),
            informant,
            source,
            root_source.clone(),
            Some(previous.mutation_id.clone()),
            "room".into(),
            rule.scope_id.clone(),
            RumorUncertainty::Unverified,
            rule,
        )
        .unwrap();
        previous = next.clone();
        mutations.push(next);
        assert_eq!(index + 2, mutations.len());
    }

    ScenePresence::validate_mutations(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        &mutations,
    )
    .unwrap();

    let ninth_text = "第九位传递者试图继续传播。";
    let ninth_seq = append_completed_character_line(&mut fixture.session, "relay06", ninth_text);
    let ninth_source = evidence_source(ninth_seq, ninth_text);
    let ninth = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "relay07".into(),
        "relay06".into(),
        ninth_source,
        root_source,
        Some(previous.mutation_id),
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    mutations.push(ninth);
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &mutations,
        )
        .is_err(),
        "rumor deliveries beyond the configured maximum hop count are rejected"
    );
}

#[test]
fn recovery_validates_and_finishes_a_partially_applied_rumor_group() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let add_keeper = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, add_keeper);
    let add_messenger = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "messenger",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, add_messenger);

    let narrative_seq = append_completed_narration(&mut fixture.session, "两人交换了秘密。 ");
    let root_text = "守门人告诉玩家，旧井下方有回声。";
    let root_seq = append_completed_character_line(&mut fixture.session, "keeper", root_text);
    let reply_text = "玩家低声复述了这则消息。";
    let reply_seq = append_completed_character_line(&mut fixture.session, "player", reply_text);
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .rumor_rule("room-rumor-v1")
        .unwrap();
    let root = evidence_source(root_seq, root_text);
    let first = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "player".into(),
        "keeper".into(),
        root.clone(),
        root.clone(),
        None,
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    let second = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "messenger".into(),
        "player".into(),
        evidence_source(reply_seq, reply_text),
        root,
        Some(first.mutation_id.clone()),
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    let mutations = vec![first.clone(), second.clone()];
    let intent = Fact::SettlementPlanned {
        identity: RoundIdentity {
            version: 1,
            round_id: uuid::Uuid::new_v4().to_string(),
            operation_id: uuid::Uuid::new_v4().to_string(),
        },
        narrative_seq,
        world_revision: "world-v1".into(),
        mutations,
    }
    .body("两跳传闻交付")
    .unwrap();
    port.fail_apply_on = Some(port.applied_count + 1);
    assert!(fixture.session.commit_world(intent, &mut port).is_err());
    let first_hash = crate::record::world::mutation_hash(&first).unwrap();
    let second_hash = crate::record::world::mutation_hash(&second).unwrap();
    assert!(
        aoidos_store::applied::contains(&port.connection, &first.mutation_id, &first_hash).unwrap()
    );
    assert!(
        !aoidos_store::applied::contains(&port.connection, &second.mutation_id, &second_hash)
            .unwrap()
    );

    port.fail_apply_on = None;
    fixture.session.recover_world(&mut port).unwrap();
    assert!(
        aoidos_store::applied::contains(&port.connection, &second.mutation_id, &second_hash)
            .unwrap()
    );
}

#[test]
fn completed_rumor_round_returns_only_the_verified_delivery_metadata() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    assert_eq!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            "not-a-uuid",
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase"
    );
    let add_keeper = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, add_keeper);
    let add_messenger = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "messenger",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, add_messenger);

    let input_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "问守门人井下有什么。".into(),
            mode: Some(crate::record::format::InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let identity = RoundIdentity {
        version: 1,
        round_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    fixture
        .session
        .append(
            Fact::RoundAccepted {
                identity: identity.clone(),
                input_seq,
                mode: crate::record::format::InputMode::InCharacter,
                dice_mode: DiceMode::Manual,
                profile_id: "default".into(),
                profile_revision: "1".into(),
                target: GenerationTarget::CharacterSpeech {
                    speaker_id: "keeper".into(),
                },
                scene_id: "room".into(),
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("接纳询问回合")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::CheckSkipped {
                identity: identity.clone(),
                reason: SkipReason::NoCheck,
                world_revision: "world-v1".into(),
            }
            .body("无需检定")
            .unwrap(),
        )
        .unwrap();

    let text = "守门人低声说，旧井下方有回声。";
    let source_seq = append_completed_character_line(&mut fixture.session, "keeper", text);
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .rumor_rule("room-rumor-v1")
        .unwrap()
        .clone();
    let event = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "player".into(),
        "keeper".into(),
        evidence_source(source_seq, text),
        evidence_source(source_seq, text),
        None,
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        &rule,
    )
    .unwrap();
    let settlement_seq = fixture
        .session
        .commit_world(
            Fact::SettlementPlanned {
                identity: identity.clone(),
                narrative_seq: source_seq,
                world_revision: "world-v1".into(),
                mutations: vec![event.clone()],
            }
            .body("登记定向传闻")
            .unwrap(),
            &mut port,
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundSettled {
                identity: identity.clone(),
                narrative_seq: source_seq,
                settlement_seq,
                applied_through_mutation_id: Some(event.mutation_id.clone()),
            }
            .body("传闻结算完成")
            .unwrap(),
        )
        .unwrap();
    let advance_seq = fixture
        .session
        .append(
            Fact::SceneStayed {
                identity: identity.clone(),
                scene_id: "room".into(),
                world_revision: "world-v1".into(),
            }
            .body("留在当前场景")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundEnded {
                identity,
                outcome: crate::ports::Outcome::Completed,
                through_seq: advance_seq,
                completed_steps: vec![
                    Step::Check,
                    Step::Narration,
                    Step::Settlement,
                    Step::Advance,
                ],
                error: None,
            }
            .body("回合完成")
            .unwrap(),
        )
        .unwrap();

    let trusted = SceneEvidence::get(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        &event.mutation_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(trusted.kind, EvidenceKind::Rumor);
    assert_eq!(trusted.subject_id, "player");
    assert_eq!(trusted.informant_id.as_deref(), Some("keeper"));
    assert_eq!(trusted.source.record_seq, source_seq);
    assert_eq!(trusted.uncertainty, Some(RumorUncertainty::Unverified));

    let (foreign_catalog, _) = catalog(&format::hash(b"foreign-script"));
    assert_eq!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &foreign_catalog,
            &port.directory,
            &event.mutation_id,
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase"
    );

    let (replacement_catalog, _) =
        catalog_version(&fixture.session.header().script_revision, "catalog-v2");
    assert_eq!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &replacement_catalog,
            &port.directory,
            &event.mutation_id,
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase"
    );

    let next_input_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "把井下的回声告诉信使。".into(),
            mode: Some(crate::record::format::InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let next_identity = RoundIdentity {
        version: 1,
        round_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    fixture
        .session
        .append(
            Fact::RoundAccepted {
                identity: next_identity.clone(),
                input_seq: next_input_seq,
                mode: crate::record::format::InputMode::InCharacter,
                dice_mode: DiceMode::Manual,
                profile_id: "default".into(),
                profile_revision: "1".into(),
                target: GenerationTarget::CharacterSpeech {
                    speaker_id: "player".into(),
                },
                scene_id: "room".into(),
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("接纳转述回合")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::CheckSkipped {
                identity: next_identity.clone(),
                reason: SkipReason::NoCheck,
                world_revision: "world-v1".into(),
            }
            .body("无需检定")
            .unwrap(),
        )
        .unwrap();
    let reply = "信使，请记住旧井下方有回声。";
    let reply_seq = append_completed_character_line(&mut fixture.session, "player", reply);
    let second_event = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "messenger".into(),
        "player".into(),
        evidence_source(reply_seq, reply),
        evidence_source(source_seq, text),
        Some(event.mutation_id.clone()),
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Disputed,
        &rule,
    )
    .unwrap();
    let mut missing_parent = second_event.clone();
    missing_parent.data.payload["previousEvidenceId"] =
        serde_json::json!(uuid::Uuid::new_v4().to_string());
    let mut mismatched_root = second_event.clone();
    mismatched_root.data.payload["rootSource"] =
        serde_json::to_value(evidence_source(reply_seq, reply)).unwrap();
    let mut returns_to_root = second_event.clone();
    returns_to_root.data.payload["recipientId"] = serde_json::json!("keeper");
    let mut malformed_rumor = second_event.clone();
    malformed_rumor.data.payload = serde_json::json!({ "evidenceId": second_event.mutation_id });
    for rejected in [
        missing_parent,
        mismatched_root,
        returns_to_root,
        malformed_rumor,
    ] {
        assert!(
            ScenePresence::validate_mutations(
                &port.connection,
                &fixture.session,
                &port.catalog,
                &port.directory,
                &[rejected],
            )
            .is_err(),
            "a cross-round relay must preserve its trusted parent and root without cycles"
        );
    }
    assert!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &second_event.mutation_id,
        )
        .unwrap()
        .is_none(),
        "rejected proposals do not publish evidence"
    );
    let second_settlement_seq = fixture
        .session
        .commit_world(
            Fact::SettlementPlanned {
                identity: next_identity.clone(),
                narrative_seq: reply_seq,
                world_revision: "world-v1".into(),
                mutations: vec![second_event.clone()],
            }
            .body("继续转述传闻")
            .unwrap(),
            &mut port,
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundSettled {
                identity: next_identity.clone(),
                narrative_seq: reply_seq,
                settlement_seq: second_settlement_seq,
                applied_through_mutation_id: Some(second_event.mutation_id.clone()),
            }
            .body("转述结算完成")
            .unwrap(),
        )
        .unwrap();
    let second_advance_seq = fixture
        .session
        .append(
            Fact::SceneStayed {
                identity: next_identity.clone(),
                scene_id: "room".into(),
                world_revision: "world-v1".into(),
            }
            .body("留在当前场景")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundEnded {
                identity: next_identity,
                outcome: crate::ports::Outcome::Completed,
                through_seq: second_advance_seq,
                completed_steps: vec![
                    Step::Check,
                    Step::Narration,
                    Step::Settlement,
                    Step::Advance,
                ],
                error: None,
            }
            .body("转述回合完成")
            .unwrap(),
        )
        .unwrap();
    let trusted_second = SceneEvidence::get(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        &second_event.mutation_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(trusted_second.kind, EvidenceKind::Rumor);
    assert_eq!(trusted_second.subject_id, "messenger");
    assert_eq!(trusted_second.informant_id.as_deref(), Some("player"));
    assert_eq!(trusted_second.source.record_seq, reply_seq);
    assert_eq!(trusted_second.uncertainty, Some(RumorUncertainty::Disputed));

    let replay_input_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "再次确认井下的消息。".into(),
            mode: Some(crate::record::format::InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let replay_identity = RoundIdentity {
        version: 1,
        round_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    fixture
        .session
        .append(
            Fact::RoundAccepted {
                identity: replay_identity.clone(),
                input_seq: replay_input_seq,
                mode: crate::record::format::InputMode::InCharacter,
                dice_mode: DiceMode::Manual,
                profile_id: "default".into(),
                profile_revision: "1".into(),
                target: GenerationTarget::CharacterSpeech {
                    speaker_id: "keeper".into(),
                },
                scene_id: "room".into(),
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("接纳重放回合")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::CheckSkipped {
                identity: replay_identity.clone(),
                reason: SkipReason::NoCheck,
                world_revision: "world-v1".into(),
            }
            .body("无需检定")
            .unwrap(),
        )
        .unwrap();
    let replay_output_seq =
        append_completed_character_line(&mut fixture.session, "keeper", "井下仍有回声。 ");
    let replay_settlement_seq = fixture
        .session
        .commit_world(
            Fact::SettlementPlanned {
                identity: replay_identity.clone(),
                narrative_seq: replay_output_seq,
                world_revision: "world-v1".into(),
                mutations: vec![event.clone()],
            }
            .body("重复记录既有获知事件")
            .unwrap(),
            &mut port,
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundSettled {
                identity: replay_identity.clone(),
                narrative_seq: replay_output_seq,
                settlement_seq: replay_settlement_seq,
                applied_through_mutation_id: Some(event.mutation_id.clone()),
            }
            .body("重放结算完成")
            .unwrap(),
        )
        .unwrap();
    let replay_advance_seq = fixture
        .session
        .append(
            Fact::SceneStayed {
                identity: replay_identity.clone(),
                scene_id: "room".into(),
                world_revision: "world-v1".into(),
            }
            .body("留在当前场景")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundEnded {
                identity: replay_identity,
                outcome: crate::ports::Outcome::Completed,
                through_seq: replay_advance_seq,
                completed_steps: vec![
                    Step::Check,
                    Step::Narration,
                    Step::Settlement,
                    Step::Advance,
                ],
                error: None,
            }
            .body("重放回合完成")
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &event.mutation_id,
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase"
    );
}

#[test]
fn evidence_lookup_stops_before_reading_more_than_its_total_byte_budget() {
    let mut fixture = Fixture::new();
    let port = port(&fixture.session);
    let large_text = "x".repeat(crate::record::format::MAX_TEXT);
    for _ in 0..MAX_EVIDENCE_BYTES / large_text.len() + 1 {
        append_completed_narration(&mut fixture.session, &large_text);
    }
    let records = fixture
        .session
        .bounded_current_records(MAX_EVIDENCE_RECORDS, MAX_EVIDENCE_BYTES)
        .unwrap();
    assert!(
        records
            .values()
            .map(|record| record.raw.len())
            .sum::<usize>()
            <= MAX_EVIDENCE_BYTES
    );
    assert!(!records.contains_key(&1));

    assert!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn evidence_query_uses_a_shared_recent_window_in_long_sessions() {
    for (prefix_records, text_bytes) in [
        (0, 1),
        (300, 1),
        (
            MAX_EVIDENCE_BYTES / crate::record::format::MAX_TEXT + 1,
            crate::record::format::MAX_TEXT,
        ),
    ] {
        let mut fixture = Fixture::new();
        let mut port = port(&fixture.session);
        let prefix_text = "x".repeat(text_bytes);
        for _ in 0..prefix_records {
            append_completed_narration(&mut fixture.session, &prefix_text);
        }
        let identity = begin_round(&mut fixture.session);
        let text = "玩家看到门上的刻痕。";
        let source_seq = append_completed_narration(&mut fixture.session, text);
        let snapshot = ScenePresence::snapshot(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
        )
        .unwrap();
        let rule = port
            .catalog
            .presence_policy()
            .unwrap()
            .observation_rule("room-observation-v1")
            .unwrap();
        let event = ObservedEvent::mutation(
            &fixture.session,
            &port.catalog,
            ObservedEvidenceInput {
                subject_id: "player".into(),
                source: evidence_source(source_seq, text),
                scene_id: snapshot.scene_id,
                scope_id: rule.scope_id.clone(),
                presence_revision: snapshot.presence_revision,
            },
            rule,
        )
        .unwrap();
        finish_round(
            &mut fixture.session,
            &mut port,
            identity,
            source_seq,
            vec![event.clone()],
        );
        let evidence = SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &event.mutation_id,
        )
        .unwrap();
        let evidence = evidence.unwrap();
        assert_eq!(evidence.source.record_seq, source_seq);
        assert_eq!(evidence.subject_id, "player");
        let recent = fixture
            .session
            .bounded_current_records(8, MAX_EVIDENCE_BYTES)
            .unwrap();
        assert_eq!(recent.len(), 8);
        assert!(recent.contains_key(&source_seq));
        assert_eq!(recent.keys().next().copied(), Some(source_seq - 3));
        let without_source = fixture
            .session
            .bounded_current_records(4, MAX_EVIDENCE_BYTES)
            .unwrap();
        assert_eq!(without_source.len(), 4);
        assert!(!without_source.contains_key(&source_seq));
    }
}

#[test]
fn evidence_query_rejects_a_new_event_whose_source_is_outside_the_byte_window() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let identity = begin_round(&mut fixture.session);
    let old_text = "玩家发现旧门上有刻痕。";
    let old_source = append_completed_narration(&mut fixture.session, old_text);
    finish_round(
        &mut fixture.session,
        &mut port,
        identity,
        old_source,
        vec![],
    );
    let large_text = "x".repeat(crate::record::format::MAX_TEXT);
    for _ in 0..MAX_EVIDENCE_BYTES / large_text.len() + 1 {
        append_completed_narration(&mut fixture.session, &large_text);
    }
    let identity = begin_round(&mut fixture.session);
    let narrative_seq = append_completed_narration(&mut fixture.session, "玩家回忆起旧门的刻痕。");
    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .observation_rule("room-observation-v1")
        .unwrap();
    let event = ObservedEvent::mutation(
        &fixture.session,
        &port.catalog,
        ObservedEvidenceInput {
            subject_id: "player".into(),
            source: evidence_source(old_source, old_text),
            scene_id: snapshot.scene_id,
            scope_id: rule.scope_id.clone(),
            presence_revision: snapshot.presence_revision,
        },
        rule,
    )
    .unwrap();
    finish_round(
        &mut fixture.session,
        &mut port,
        identity,
        narrative_seq,
        vec![event.clone()],
    );
    let records = fixture
        .session
        .bounded_current_records(MAX_EVIDENCE_RECORDS, MAX_EVIDENCE_BYTES)
        .unwrap();
    assert!(records.len() <= MAX_EVIDENCE_RECORDS);
    assert!(
        records
            .values()
            .map(|record| record.raw.len())
            .sum::<usize>()
            <= MAX_EVIDENCE_BYTES
    );
    assert!(records.contains_key(&narrative_seq));
    assert!(!records.contains_key(&old_source));
    assert_eq!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &event.mutation_id,
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase"
    );
}

#[test]
fn rumor_relay_cannot_use_a_parent_outside_the_shared_query_window() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    for entity_id in ["keeper", "messenger"] {
        let change = ScenePresence::prepare_change(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            "keeper-presence-v1",
            entity_id,
            PresenceAction::Enter,
        )
        .unwrap();
        commit_presence_change(&mut fixture, &mut port, change);
    }
    let identity = begin_round_for_target(
        &mut fixture.session,
        GenerationTarget::CharacterSpeech {
            speaker_id: "keeper".into(),
        },
    );
    let root_text = "守门人告诉玩家，井下有回声。";
    let root_seq = append_completed_character_line(&mut fixture.session, "keeper", root_text);
    let root_source = evidence_source(root_seq, root_text);
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .rumor_rule("room-rumor-v1")
        .unwrap()
        .clone();
    let root = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "player".into(),
        "keeper".into(),
        root_source.clone(),
        root_source.clone(),
        None,
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        &rule,
    )
    .unwrap();
    finish_round(
        &mut fixture.session,
        &mut port,
        identity,
        root_seq,
        vec![root.clone()],
    );
    assert!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &root.mutation_id,
        )
        .unwrap()
        .is_some()
    );
    let large_text = "x".repeat(crate::record::format::MAX_TEXT);
    for _ in 0..MAX_EVIDENCE_BYTES / large_text.len() + 1 {
        append_completed_narration(&mut fixture.session, &large_text);
    }
    assert!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &root.mutation_id,
        )
        .unwrap()
        .is_none()
    );
    let relay_identity = begin_round_for_target(
        &mut fixture.session,
        GenerationTarget::CharacterSpeech {
            speaker_id: "player".into(),
        },
    );
    let reply = "玩家把井下的回声告诉信使。";
    let reply_seq = append_completed_character_line(&mut fixture.session, "player", reply);
    let relay = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "messenger".into(),
        "player".into(),
        evidence_source(reply_seq, reply),
        root_source,
        Some(root.mutation_id),
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        &rule,
    )
    .unwrap();
    let last_event_seq = fixture.session.last_event_seq();
    assert!(
        fixture
            .session
            .commit_world(
                Fact::SettlementPlanned {
                    identity: relay_identity,
                    narrative_seq: reply_seq,
                    world_revision: "world-v1".into(),
                    mutations: vec![relay],
                }
                .body("不能引用窗口外父传闻")
                .unwrap(),
                &mut port
            )
            .is_err()
    );
    assert_eq!(fixture.session.last_event_seq(), last_event_seq);
    assert!(!fixture.session.needs_recovery());
}

#[test]
fn recovery_anchors_rumor_parent_lookup_before_each_intent() {
    for later_history in [false, true] {
        let mut fixture = Fixture::new();
        let mut port = port(&fixture.session);
        for entity_id in ["keeper", "messenger"] {
            let change = ScenePresence::prepare_change(
                &port.connection,
                &fixture.session,
                &port.catalog,
                &port.directory,
                "keeper-presence-v1",
                entity_id,
                PresenceAction::Enter,
            )
            .unwrap();
            commit_presence_change(&mut fixture, &mut port, change);
        }
        let identity = begin_round_for_target(
            &mut fixture.session,
            GenerationTarget::CharacterSpeech {
                speaker_id: "keeper".into(),
            },
        );
        let root_text = "守门人告诉玩家，井下有回声。";
        let root_seq = append_completed_character_line(&mut fixture.session, "keeper", root_text);
        let root_source = evidence_source(root_seq, root_text);
        let rule = port
            .catalog
            .presence_policy()
            .unwrap()
            .rumor_rule("room-rumor-v1")
            .unwrap()
            .clone();
        let root = RumorDelivery::mutation(
            &fixture.session,
            &port.catalog,
            "player".into(),
            "keeper".into(),
            root_source.clone(),
            root_source.clone(),
            None,
            "room".into(),
            rule.scope_id.clone(),
            RumorUncertainty::Unverified,
            &rule,
        )
        .unwrap();
        finish_round(
            &mut fixture.session,
            &mut port,
            identity,
            root_seq,
            vec![root.clone()],
        );
        let identity = begin_round_for_target(
            &mut fixture.session,
            GenerationTarget::CharacterSpeech {
                speaker_id: "player".into(),
            },
        );
        let reply = "玩家把井下的消息转告信使。";
        let reply_seq = append_completed_character_line(&mut fixture.session, "player", reply);
        let child = RumorDelivery::mutation(
            &fixture.session,
            &port.catalog,
            "messenger".into(),
            "player".into(),
            evidence_source(reply_seq, reply),
            root_source,
            Some(root.mutation_id.clone()),
            "room".into(),
            rule.scope_id.clone(),
            RumorUncertainty::Unverified,
            &rule,
        )
        .unwrap();
        if later_history {
            finish_round(
                &mut fixture.session,
                &mut port,
                identity,
                reply_seq,
                vec![child.clone()],
            );
            assert!(
                SceneEvidence::get(
                    &port.connection,
                    &fixture.session,
                    &port.catalog,
                    &port.directory,
                    &child.mutation_id,
                )
                .unwrap()
                .is_some()
            );
            let large_text = "x".repeat(crate::record::format::MAX_TEXT);
            for _ in 0..MAX_EVIDENCE_BYTES / large_text.len() + 1 {
                append_completed_narration(&mut fixture.session, &large_text);
            }
            assert!(
                SceneEvidence::get(
                    &port.connection,
                    &fixture.session,
                    &port.catalog,
                    &port.directory,
                    &root.mutation_id,
                )
                .unwrap()
                .is_none()
            );
        } else {
            port.fail_apply = true;
            assert!(
                fixture
                    .session
                    .commit_world(
                        Fact::SettlementPlanned {
                            identity,
                            narrative_seq: reply_seq,
                            world_revision: "world-v1".into(),
                            mutations: vec![child.clone()],
                        }
                        .body("持久化待恢复转述")
                        .unwrap(),
                        &mut port
                    )
                    .is_err()
            );
            assert!(fixture.session.needs_recovery());
            assert_eq!(
                SceneEvidence::get(
                    &port.connection,
                    &fixture.session,
                    &port.catalog,
                    &port.directory,
                    &root.mutation_id,
                )
                .unwrap_err()
                .code,
                "engine.invalid-phase"
            );
            port.fail_apply = false;
        }
        fixture.session = Session::open(fixture.path.clone(), fixture.events.clone()).unwrap();
        fixture.session.recover_history(&mut port).unwrap();
        let applied_before = port.applied_count;
        fixture.session.recover_world(&mut port).unwrap();
        fixture.session.recover_world(&mut port).unwrap();
        assert_eq!(
            port.applied_count,
            applied_before + usize::from(!later_history)
        );
        assert!(
            aoidos_store::applied::contains(
                &port.connection,
                &child.mutation_id,
                &crate::record::world::mutation_hash(&child).unwrap(),
            )
            .unwrap()
        );
        assert!(!fixture.session.needs_recovery());
        assert!(
            SceneEvidence::get(
                &port.connection,
                &fixture.session,
                &port.catalog,
                &port.directory,
                &child.mutation_id,
            )
            .unwrap()
            .is_none(),
            "incomplete rounds and events outside the current window are not published"
        );
    }
}

#[test]
fn read_only_evidence_requires_an_applied_event_and_a_complete_successful_round() {
    let mut fixture = Fixture::new();
    let mut port = port(&fixture.session);
    let failed_text = "失败行动前已生成、但不能成为记忆证据的正文。";
    let failed_source_seq = append_failed_source_round(&mut fixture.session, failed_text);
    let input_seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "查看门上的痕迹".into(),
            mode: Some(crate::record::format::InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let identity = RoundIdentity {
        version: 1,
        round_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    fixture
        .session
        .append(
            Fact::RoundAccepted {
                identity: identity.clone(),
                input_seq,
                mode: crate::record::format::InputMode::InCharacter,
                dice_mode: DiceMode::Manual,
                profile_id: "default".into(),
                profile_revision: "1".into(),
                target: GenerationTarget::Narration,
                scene_id: "room".into(),
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("接纳回合")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::CheckSkipped {
                identity: identity.clone(),
                reason: SkipReason::NoCheck,
                world_revision: "world-v1".into(),
            }
            .body("无需检定")
            .unwrap(),
        )
        .unwrap();
    let text = "玩家发现门上的刻痕仍然湿润。";
    let narrative_seq = append_completed_narration(&mut fixture.session, text);
    let snapshot = ScenePresence::snapshot(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
    )
    .unwrap();
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .observation_rule("room-observation-v1")
        .unwrap();
    let event = ObservedEvent::mutation(
        &fixture.session,
        &port.catalog,
        ObservedEvidenceInput {
            subject_id: "player".into(),
            source: evidence_source(failed_source_seq, failed_text),
            scene_id: "room".into(),
            scope_id: rule.scope_id.clone(),
            presence_revision: snapshot.presence_revision.clone(),
        },
        rule,
    )
    .unwrap();
    let successful_source_event = ObservedEvent::mutation(
        &fixture.session,
        &port.catalog,
        ObservedEvidenceInput {
            subject_id: "player".into(),
            source: evidence_source(narrative_seq, text),
            scene_id: "room".into(),
            scope_id: rule.scope_id.clone(),
            presence_revision: snapshot.presence_revision.clone(),
        },
        rule,
    )
    .unwrap();
    let settlement_seq = fixture
        .session
        .commit_world(
            Fact::SettlementPlanned {
                identity: identity.clone(),
                narrative_seq,
                world_revision: "world-v1".into(),
                mutations: vec![successful_source_event.clone(), event.clone()],
            }
            .body("登记观察证据")
            .unwrap(),
            &mut port,
        )
        .unwrap();
    assert!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &event.mutation_id,
        )
        .unwrap()
        .is_none()
    );
    fixture
        .session
        .append(
            Fact::RoundSettled {
                identity: identity.clone(),
                narrative_seq,
                settlement_seq,
                applied_through_mutation_id: Some(event.mutation_id.clone()),
            }
            .body("结算完成")
            .unwrap(),
        )
        .unwrap();
    let advance_seq = fixture
        .session
        .append(
            Fact::SceneStayed {
                identity: identity.clone(),
                scene_id: "room".into(),
                world_revision: "world-v1".into(),
            }
            .body("留在当前场景")
            .unwrap(),
        )
        .unwrap();
    fixture
        .session
        .append(
            Fact::RoundEnded {
                identity,
                outcome: crate::ports::Outcome::Completed,
                through_seq: advance_seq,
                completed_steps: vec![
                    Step::Check,
                    Step::Narration,
                    Step::Settlement,
                    Step::Advance,
                ],
                error: None,
            }
            .body("回合完成")
            .unwrap(),
        )
        .unwrap();

    assert_eq!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &event.mutation_id,
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase"
    );
    let trusted = SceneEvidence::get(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        &successful_source_event.mutation_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(trusted.kind, EvidenceKind::Observed);
    assert_eq!(trusted.subject_id, "player");
    assert_eq!(trusted.source.record_seq, narrative_seq);

    let (replacement_catalog, _) =
        catalog_version(&fixture.session.header().script_revision, "catalog-v2");
    assert_eq!(
        SceneEvidence::get(
            &port.connection,
            &fixture.session,
            &replacement_catalog,
            &port.directory,
            &successful_source_event.mutation_id,
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase"
    );

    let add_keeper = ScenePresence::prepare_change(
        &port.connection,
        &fixture.session,
        &port.catalog,
        &port.directory,
        "keeper-presence-v1",
        "keeper",
        PresenceAction::Enter,
    )
    .unwrap();
    commit_presence_change(&mut fixture, &mut port, add_keeper);
    let reply = "我把门上的刻痕告诉守门人。";
    let reply_seq = append_completed_character_line(&mut fixture.session, "player", reply);
    let rule = port
        .catalog
        .presence_policy()
        .unwrap()
        .rumor_rule("room-rumor-v1")
        .unwrap();
    let rumor = RumorDelivery::mutation(
        &fixture.session,
        &port.catalog,
        "keeper".into(),
        "player".into(),
        evidence_source(reply_seq, reply),
        evidence_source(reply_seq, reply),
        Some(successful_source_event.mutation_id),
        "room".into(),
        rule.scope_id.clone(),
        RumorUncertainty::Unverified,
        rule,
    )
    .unwrap();
    assert!(
        ScenePresence::validate_mutations(
            &port.connection,
            &fixture.session,
            &port.catalog,
            &port.directory,
            &[rumor],
        )
        .is_err(),
        "an observed event cannot serve as the previous delivery of a rumor chain"
    );
}
