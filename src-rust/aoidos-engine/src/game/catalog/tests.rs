use super::*;
use crate::{
    game::presence::SceneEntryPresence,
    record::{facts::SceneNode, test_support::Fixture},
};
fn catalog() -> SceneCatalog {
    let nodes = BTreeMap::from([(
        "room".into(),
        Node {
            kind: "scene".into(),
            title: "房间".into(),
            parent: None,
        },
    )]);
    let scenes = BTreeMap::from([(
        "room".into(),
        Scene {
            position: ScenePosition {
                scene_id: "room".into(),
                path: vec![SceneNode {
                    kind: "scene".into(),
                    id: "room".into(),
                    title: "房间".into(),
                }],
            },
            rules: SceneRuleView {
                scene_id: "room".into(),
                catalog_revision: "1".into(),
                advance_rule_ids: vec![],
                progress_rule_ids: vec!["progress".into()],
                hint_rules: vec![],
                exit_rule_ids: vec![],
            },
            advance: vec![],
            check_rule_ids: vec![],
            actor_ids: vec!["player".into()],
            forced_check: None,
            dice_disabled: true,
        },
    )]);
    SceneCatalog::new(
        "1".into(),
        crate::record::test_support::header().script_revision,
        nodes,
        scenes,
        BTreeMap::from([(
            "progress".into(),
            Condition::AtLeast {
                key: "visited".into(),
                value: 1,
            },
        )]),
    )
    .unwrap()
}

fn cycle_run(session: &crate::record::session::Session) -> aoidos_memory::model::Run {
    use aoidos_memory::model::{Algorithm, Policy};
    aoidos_memory::model::Run {
        run_id: "00000000-0000-4000-8000-000000000001".into(),
        script_id: session.header().script_id.clone(),
        session_id: session.header().session_id.clone(),
        cycle_gen: 7,
        policy: Policy {
            version: 1,
            algorithm_version: 1,
            config_version: 1,
            app_config_version: 1,
            algorithm: Algorithm::None,
            base_strength: 100_000,
            reinforcement_gain: 0,
            reinforcement_cap: 0,
            strength_cap: 100_000,
            half_life_rounds: 10,
            repeat_window_rounds: 1,
            repeat_limit: 1,
            revival_cooldown_rounds: 1,
            active_capacity: 100,
            rate_table_version: 1,
            decay_rate: 1,
        },
        policy_hash: [0; 32],
    }
}

#[test]
fn cycle_mark_authorization_expands_only_registered_entities_matching_kind_and_scope() {
    let fixture = Fixture::new();
    let mut catalog = catalog();
    let script_revision = catalog.script_revision.clone();
    let directory = EntityDirectory::new(
        script_revision,
        3,
        true,
        vec![
            CanonicalEntity {
                id: "player".into(),
                kind: EntityKind::Character,
                scope: "harbor/players".into(),
                canonical_name: "玩家".into(),
            },
            CanonicalEntity {
                id: "fixed-guard".into(),
                kind: EntityKind::Character,
                scope: "harbor/guards".into(),
                canonical_name: "守门人".into(),
            },
            CanonicalEntity {
                id: "dynamic-guard".into(),
                kind: EntityKind::Character,
                scope: "harbor/guards".into(),
                canonical_name: "新来的守卫".into(),
            },
            CanonicalEntity {
                id: "harbor-statue".into(),
                kind: EntityKind::Item,
                scope: "harbor/guards".into(),
                canonical_name: "石像".into(),
            },
            CanonicalEntity {
                id: "city-guard".into(),
                kind: EntityKind::Character,
                scope: "city/guards".into(),
                canonical_name: "城门卫兵".into(),
            },
        ],
    )
    .unwrap();
    let unbound_directory = directory.clone();
    let directory = directory.bind_session_snapshot(&fixture.session).unwrap();
    catalog = catalog.with_entity_directory(directory.clone()).unwrap();
    catalog = catalog
        .with_cycle_mark_rules(vec![
            CycleMarkRule::new(
                "guard-recognition".into(),
                2,
                vec![
                    CycleSubjectScope::new(EntityKind::Character, "harbor/guards".into()).unwrap(),
                ],
                "rescue-memory".into(),
                CycleKind::Familiarity,
                DetailPolicy::Vague,
                20,
            )
            .unwrap(),
        ])
        .unwrap();

    let authorizations = catalog
        .cycle_mark_authorizations(&cycle_run(&fixture.session), &fixture.session, &directory)
        .unwrap();
    let authorization = &authorizations["guard-recognition"];
    assert_eq!(authorization.script_id, fixture.session.header().script_id);
    assert_eq!(authorization.run_id, "00000000-0000-4000-8000-000000000001");
    assert_eq!(authorization.cycle_gen, 7);
    assert_eq!(authorization.rule_version, 2);
    assert_eq!(authorization.imprint_key, "rescue-memory");
    assert_eq!(
        authorization.allowed_subject_ids,
        BTreeSet::from(["dynamic-guard".into(), "fixed-guard".into()])
    );
    assert!(
        catalog
            .cycle_mark_authorizations(
                &cycle_run(&fixture.session),
                &fixture.session,
                &unbound_directory
            )
            .is_err()
    );

    let other_fixture = Fixture::new();
    assert!(
        catalog
            .cycle_mark_authorizations(
                &cycle_run(&other_fixture.session),
                &other_fixture.session,
                &directory,
            )
            .is_err()
    );
}

#[test]
fn cycle_mark_rules_reject_invalid_scopes_and_stale_or_incomplete_directories() {
    assert!(CycleSubjectScope::new(EntityKind::Character, " ".into()).is_err());
    assert!(
        CycleMarkRule::new(
            "rule".into(),
            1,
            vec![
                CycleSubjectScope::new(EntityKind::Character, "harbor".into()).unwrap(),
                CycleSubjectScope::new(EntityKind::Character, "harbor".into()).unwrap(),
            ],
            "imprint".into(),
            CycleKind::Familiarity,
            DetailPolicy::Vague,
            0,
        )
        .is_err()
    );

    let fixture = Fixture::new();
    let catalog = catalog()
        .with_cycle_mark_rules(vec![
            CycleMarkRule::new(
                "rule".into(),
                1,
                vec![CycleSubjectScope::new(EntityKind::Character, "harbor".into()).unwrap()],
                "imprint".into(),
                CycleKind::Familiarity,
                DetailPolicy::Vague,
                0,
            )
            .unwrap(),
        ])
        .unwrap();
    let stale = EntityDirectory::new(
        format::hash(b"another script revision"),
        1,
        true,
        vec![CanonicalEntity {
            id: "guard".into(),
            kind: EntityKind::Character,
            scope: "harbor".into(),
            canonical_name: "守卫".into(),
        }],
    )
    .unwrap();
    assert!(
        catalog
            .cycle_mark_authorizations(&cycle_run(&fixture.session), &fixture.session, &stale)
            .is_err()
    );
    let incomplete = EntityDirectory::new(
        catalog.script_revision.clone(),
        1,
        false,
        vec![CanonicalEntity {
            id: "guard".into(),
            kind: EntityKind::Character,
            scope: "harbor".into(),
            canonical_name: "守卫".into(),
        }],
    )
    .unwrap();
    assert!(
        catalog
            .cycle_mark_authorizations(&cycle_run(&fixture.session), &fixture.session, &incomplete)
            .is_err()
    );
}

#[test]
fn cycle_mark_context_uses_only_current_batch_sources_and_matching_rule_scope() {
    use crate::{
        ports::Terminal,
        record::{
            format::Body,
            memory::{CycleMarkContextInput, SourcePort, build_cycle_mark_context},
        },
    };
    use aoidos_memory::{
        model::CausalPort,
        repository::{BatchSnapshot, Span},
    };

    let mut fixture = Fixture::new();
    let source_seq = fixture
        .session
        .append(Body::Narration {
            text: "守卫记得玩家救过他。".into(),
            turn_id: aoidos_memory::model::new_id(),
            terminal: Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    let run = cycle_run(&fixture.session);
    let catalog = catalog()
        .with_cycle_mark_rules(vec![
            CycleMarkRule::new(
                "guard-recognition".into(),
                2,
                vec![
                    CycleSubjectScope::new(EntityKind::Character, "harbor/guards".into()).unwrap(),
                ],
                "rescue-memory".into(),
                CycleKind::Familiarity,
                DetailPolicy::Vague,
                20,
            )
            .unwrap(),
        ])
        .unwrap();
    let directory = EntityDirectory::new(
        fixture.session.header().script_revision.clone(),
        3,
        true,
        vec![
            CanonicalEntity {
                id: "dynamic-guard".into(),
                kind: EntityKind::Character,
                scope: "harbor/guards".into(),
                canonical_name: "临时守卫".into(),
            },
            CanonicalEntity {
                id: "harbor-statue".into(),
                kind: EntityKind::Item,
                scope: "harbor/guards".into(),
                canonical_name: "石像".into(),
            },
        ],
    )
    .unwrap()
    .bind_session_snapshot(&fixture.session)
    .unwrap();
    let source_port = SourcePort::with_entity_directory(&fixture.session, &directory).unwrap();
    let source = source_port.reference(&run.run_id, source_seq).unwrap();
    let boundary = source_port.boundary().unwrap();
    let snapshot = BatchSnapshot {
        logical_clock: 1,
        version: 1,
        batch_id: aoidos_memory::model::new_id(),
        run_id: run.run_id.clone(),
        history_revision: boundary.history_revision,
        entity_revision: boundary.entity_revision,
        evidence_revision: boundary.evidence_revision,
        policy_hash: run.policy_hash,
        spans: vec![Span {
            source_id: "s1".into(),
            material_id: aoidos_memory::model::new_id(),
            source,
            text: "守卫记得玩家救过他。".into(),
            context: false,
            continuation: false,
        }],
        input: serde_json::Value::Null,
    };

    let mut context = aoidos_memory::model::ProposalContext::default();
    build_cycle_mark_context(CycleMarkContextInput {
        catalog: &catalog,
        run: &run,
        session: &fixture.session,
        directory: &directory,
        batch: &snapshot,
        source_port: &source_port,
    })
    .unwrap()
    .apply_to(&mut context)
    .unwrap();
    assert_eq!(context.source_ids, BTreeSet::from(["s1".into()]));
    assert_eq!(context.entity_ids, BTreeSet::from(["dynamic-guard".into()]));
    assert_eq!(context.cycle_rules.len(), 1);
    assert_eq!(context.cycle_sources["s1"].source.record_seq, source_seq);

    let mut inconsistent_context = aoidos_memory::model::ProposalContext::default();
    inconsistent_context
        .cycle_sources
        .insert("stale-source".into(), context.cycle_sources["s1"].clone());
    assert!(
        build_cycle_mark_context(CycleMarkContextInput {
            catalog: &catalog,
            run: &run,
            session: &fixture.session,
            directory: &directory,
            batch: &snapshot,
            source_port: &source_port,
        })
        .unwrap()
        .apply_to(&mut inconsistent_context)
        .is_err()
    );
    assert!(inconsistent_context.cycle_rules.is_empty());

    let newer_directory = EntityDirectory::new(
        fixture.session.header().script_revision.clone(),
        4,
        true,
        vec![
            CanonicalEntity {
                id: "dynamic-guard".into(),
                kind: EntityKind::Character,
                scope: "harbor/guards".into(),
                canonical_name: "临时守卫".into(),
            },
            CanonicalEntity {
                id: "late-guard".into(),
                kind: EntityKind::Character,
                scope: "harbor/guards".into(),
                canonical_name: "后来加入的守卫".into(),
            },
        ],
    )
    .unwrap()
    .bind_session_snapshot(&fixture.session)
    .unwrap();
    assert!(
        build_cycle_mark_context(CycleMarkContextInput {
            catalog: &catalog,
            run: &run,
            session: &fixture.session,
            directory: &newer_directory,
            batch: &snapshot,
            source_port: &source_port,
        })
        .is_err()
    );

    let mut stale = snapshot;
    stale.history_revision += 1;
    assert!(
        build_cycle_mark_context(CycleMarkContextInput {
            catalog: &catalog,
            run: &run,
            session: &fixture.session,
            directory: &directory,
            batch: &stale,
            source_port: &source_port,
        })
        .is_err()
    );
}

#[test]
fn rule_queries_are_read_only_and_unknown_or_missing_evidence_is_not_false() {
    let fixture = Fixture::new();
    let catalog = catalog();
    let value = Value::from(1);
    let world = ConfirmedWorldView::capture(
        &fixture.session,
        "0",
        BTreeMap::from([(
            "visited".into(),
            WorldFact {
                value: value.clone(),
                evidence_refs: vec![EvidenceRef::Script {
                    script_revision: fixture.session.header().script_revision.clone(),
                    rule_version: 1,
                    key: "visited".into(),
                    value_hash: value_hash_of(&value).unwrap(),
                }],
            },
        )]),
    )
    .unwrap();
    assert_eq!(world.session_id(), fixture.session.header().session_id);
    assert!(
        catalog
            .evaluate_scene_rule("room", "progress", &world, "0", 0)
            .unwrap()
            .matched
    );
    assert!(
        catalog
            .evaluate_scene_rule("room", "unknown", &world, "0", 0)
            .is_err()
    );
    assert!(
        catalog
            .evaluate_scene_rule("room", "progress", &world, "1", 0)
            .is_err()
    );
    assert!(catalog.get_scene_rule_view("room", "old").is_err());
    assert!(catalog.scene("unknown").is_err());
    assert_eq!(
        catalog
            .get_scene_rule_view("room", catalog.revision())
            .unwrap()
            .hint_rules
            .len(),
        0
    );
    assert!(catalog.candidates("room", &world).unwrap().0.is_empty());
    let empty = ConfirmedWorldView::capture(&fixture.session, "0", BTreeMap::new()).unwrap();
    assert!(
        catalog
            .evaluate_scene_rule("room", "progress", &empty, "0", 0)
            .is_err()
    );
    assert_eq!(fixture.session.last_event_seq(), 0);
}

fn world(fixture: &Fixture, value: Value) -> ConfirmedWorldView {
    let proof = EvidenceRef::Script {
        script_revision: fixture.session.header().script_revision.clone(),
        rule_version: 1,
        key: "visited".into(),
        value_hash: value_hash_of(&value).unwrap(),
    };
    ConfirmedWorldView::capture(
        &fixture.session,
        "0",
        BTreeMap::from([(
            "visited".into(),
            WorldFact {
                value,
                evidence_refs: vec![proof],
            },
        )]),
    )
    .unwrap()
}

#[test]
fn presence_policy_is_checked_against_an_attached_complete_entity_directory() {
    let fixture = Fixture::new();
    let directory = EntityDirectory::new(
        fixture.session.header().script_revision.clone(),
        1,
        true,
        vec![CanonicalEntity {
            id: "player".into(),
            kind: EntityKind::Character,
            scope: "global".into(),
            canonical_name: "玩家".into(),
        }],
    )
    .unwrap();
    let entry = SceneEntryPresence {
        rule_id: "entry-v1".into(),
        rule_version: 1,
        character_ids: vec!["player".into()],
    };
    let policy = PresencePolicy::new(
        "room".into(),
        entry.clone(),
        BTreeMap::from([("room".into(), entry)]),
        vec![],
    )
    .unwrap();

    let without_directory = catalog().with_presence_policy(policy.clone()).unwrap();
    assert!(without_directory.entity_directory().is_none());

    let configured = catalog()
        .with_entity_directory(directory.clone())
        .unwrap()
        .with_presence_policy(policy.clone())
        .unwrap();
    assert_eq!(configured.entity_directory().unwrap().revision(), 1);
    assert_eq!(
        configured.presence_policy().unwrap().initial_scene_id(),
        "room"
    );

    let incomplete = EntityDirectory::new(
        directory.script_revision().into(),
        1,
        false,
        directory.entries().cloned().collect(),
    )
    .unwrap();
    assert!(
        catalog()
            .with_entity_directory(incomplete)
            .unwrap()
            .with_presence_policy(policy)
            .is_err(),
        "presence policy must not bind to an incomplete entity directory"
    );
}
#[test]
fn typed_boolean_composition_checks_all_sources_and_rejects_type_errors() {
    let fixture = Fixture::new();
    let view = world(&fixture, Value::from(1));
    let mut catalog = catalog();
    let equality = Condition::Equals {
        key: "visited".into(),
        value: Value::from(1),
    };
    for (condition, expected) in [
        (Condition::Always {}, true),
        (equality.clone(), true),
        (
            Condition::Not {
                condition: Box::new(equality.clone()),
            },
            false,
        ),
        (
            Condition::All {
                conditions: vec![Condition::Always {}, equality.clone()],
            },
            true,
        ),
        (
            Condition::Any {
                conditions: vec![
                    Condition::Equals {
                        key: "visited".into(),
                        value: Value::from(2),
                    },
                    equality.clone(),
                ],
            },
            true,
        ),
    ] {
        catalog.conditions.insert("progress".into(), condition);
        let matched = catalog
            .evaluate_scene_rule("room", "progress", &view, "0", 0)
            .unwrap();
        assert_eq!(matched.matched, expected);
        assert!(matched.evidence_refs.len() <= 2);
    }
    catalog.conditions.insert(
        "progress".into(),
        Condition::Equals {
            key: "visited".into(),
            value: Value::String("1".into()),
        },
    );
    assert_eq!(
        catalog
            .evaluate_scene_rule("room", "progress", &view, "0", 0)
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    catalog.conditions.insert(
        "progress".into(),
        Condition::AtLeast {
            key: "visited".into(),
            value: 1,
        },
    );
    assert!(
        catalog
            .evaluate_scene_rule(
                "room",
                "progress",
                &world(&fixture, Value::Bool(true)),
                "0",
                0
            )
            .is_err()
    );
    for condition in [
        Condition::All {
            conditions: vec![
                Condition::Not {
                    condition: Box::new(Condition::Always {}),
                },
                Condition::Equals {
                    key: "missing".into(),
                    value: Value::Bool(true),
                },
            ],
        },
        Condition::Any {
            conditions: vec![
                Condition::Always {},
                Condition::Equals {
                    key: "missing".into(),
                    value: Value::Bool(true),
                },
            ],
        },
    ] {
        catalog.conditions.insert("progress".into(), condition);
        assert!(
            catalog
                .evaluate_scene_rule("room", "progress", &view, "0", 0)
                .is_err()
        );
    }
    for value in [
        Value::Null,
        Value::Bool(true),
        Value::String("已确认".into()),
    ] {
        catalog.conditions.insert(
            "progress".into(),
            Condition::Equals {
                key: "visited".into(),
                value: value.clone(),
            },
        );
        assert!(
            catalog
                .evaluate_scene_rule("room", "progress", &world(&fixture, value), "0", 0)
                .unwrap()
                .matched
        );
    }
    assert!(
        aoidos_json::from_value::<Condition>(
            serde_json::json!({"kind":"always","worldMutation":true})
        )
        .is_err()
    );
    assert_eq!(fixture.session.last_event_seq(), 0);
}
#[test]
fn registered_hierarchy_nodes_do_not_automatically_become_active_scenes() {
    let mut catalog = catalog();
    catalog.nodes.insert(
        "chapter".into(),
        Node {
            kind: "chapter".into(),
            title: "第一章".into(),
            parent: None,
        },
    );
    let position = ScenePosition {
        scene_id: "chapter".into(),
        path: vec![SceneNode {
            kind: "chapter".into(),
            id: "chapter".into(),
            title: "第一章".into(),
        }],
    };
    assert!(catalog.verify_position(&position).is_err());
    catalog
        .verify_position(&catalog.scene("room").unwrap().position)
        .unwrap();
}

fn register(candidate: SceneCatalog) -> Result<SceneCatalog, Fault> {
    SceneCatalog::new(
        candidate.revision,
        candidate.script_revision,
        candidate.nodes,
        candidate.scenes,
        candidate.conditions,
    )
}
#[test]
fn malformed_catalogs_paths_and_rule_budgets_are_rejected_before_registration() {
    let mut empty_revision = catalog();
    empty_revision.revision.clear();
    assert!(register(empty_revision).is_err());
    for index in 0..12 {
        let mut candidate = catalog();
        let scene = candidate.scenes.get_mut("room").unwrap();
        match index {
            0 => candidate.nodes.get_mut("room").unwrap().title.clear(),
            1 => candidate.nodes.get_mut("room").unwrap().parent = Some("room".into()),
            2 => candidate.nodes.get_mut("room").unwrap().parent = Some("missing".into()),
            3 => {
                candidate
                    .conditions
                    .insert("/".into(), Condition::Always {});
            }
            4 => scene.position.scene_id = "wrong".into(),
            5 => scene.actor_ids.clear(),
            6 => scene.forced_check = Some("missing".into()),
            7 => scene.rules.progress_rule_ids = vec!["missing".into()],
            8 => {
                scene.advance = vec![Advance {
                    rule_id: "progress".into(),
                    target_scene_id: "missing".into(),
                    priority: 0,
                }]
            }
            9 => {
                scene.advance = vec![
                    Advance {
                        rule_id: "progress".into(),
                        target_scene_id: "room".into(),
                        priority: 0
                    };
                    65
                ]
            }
            10 => scene.position.path.clear(),
            11 => scene.position.path[0].title = "foreign".into(),
            _ => unreachable!(),
        }
        assert!(register(candidate).is_err(), "case {index}");
    }
    for condition in [
        Condition::Equals {
            key: "/".into(),
            value: Value::Bool(true),
        },
        Condition::Equals {
            key: "visited".into(),
            value: serde_json::json!({}),
        },
        Condition::AtLeast {
            key: "/".into(),
            value: 0,
        },
        Condition::All { conditions: vec![] },
        Condition::Any {
            conditions: vec![Condition::Always {}; 128],
        },
    ] {
        let mut candidate = catalog();
        candidate.conditions.insert("progress".into(), condition);
        assert!(register(candidate).is_err());
    }
    let mut deep = Condition::Always {};
    for _ in 0..17 {
        deep = Condition::Not {
            condition: Box::new(deep),
        };
    }
    assert!(validate_condition(&deep, 0, &mut 0).is_err());
    validate_condition(
        &Condition::Equals {
            key: "visited".into(),
            value: Value::Bool(true),
        },
        0,
        &mut 0,
    )
    .unwrap();
    validate_condition(
        &Condition::All {
            conditions: vec![
                Condition::Always {},
                Condition::Not {
                    condition: Box::new(Condition::Always {}),
                },
            ],
        },
        0,
        &mut 0,
    )
    .unwrap();
    let good = HintRule {
        hint_id: "hint".into(),
        when_rule_id: "progress".into(),
        text: "灯光照亮出口".into(),
        priority: 1,
        target_scope: "player".into(),
    };
    for hints in [
        vec![good.clone(), good.clone()],
        vec![HintRule {
            text: String::new(),
            ..good.clone()
        }],
        vec![good.clone(); 33],
    ] {
        let mut candidate = catalog();
        candidate.scenes.get_mut("room").unwrap().rules.hint_rules = hints;
        assert!(register(candidate).is_err());
    }
    let mut candidate = catalog();
    candidate.scenes.get_mut("room").unwrap().rules.hint_rules = vec![good];
    register(candidate).unwrap();
}

#[test]
fn entity_directory_is_version_bound_and_complete_actor_sets_are_closed() {
    fn directory(script_revision: String, complete: bool) -> EntityDirectory {
        EntityDirectory::new(
            script_revision,
            1,
            complete,
            vec![CanonicalEntity {
                id: "player".into(),
                kind: EntityKind::Character,
                scope: "global".into(),
                canonical_name: "玩家".into(),
            }],
        )
        .unwrap()
    }

    let registered = catalog();
    let script_revision = registered.script_revision.clone();
    let with_directory = registered
        .with_entity_directory(directory(script_revision.clone(), true))
        .unwrap();
    let entities = with_directory.entity_directory().unwrap();
    assert!(entities.is_complete());
    assert_eq!(entities.revision(), 1);
    assert_eq!(
        entities
            .resolve_exact(EntityKind::Character, "global", "玩家")
            .unwrap()
            .unwrap()
            .id,
        "player"
    );

    assert!(
        catalog()
            .with_entity_directory(directory(
                crate::record::format::hash(b"another script"),
                true
            ))
            .is_err()
    );

    let mut unknown_actor = catalog();
    unknown_actor
        .scenes
        .get_mut("room")
        .unwrap()
        .actor_ids
        .push("unregistered".into());
    assert!(
        unknown_actor
            .with_entity_directory(directory(script_revision.clone(), true))
            .is_err()
    );

    let non_character = EntityDirectory::new(
        script_revision.clone(),
        1,
        true,
        vec![CanonicalEntity {
            id: "player".into(),
            kind: EntityKind::Place,
            scope: "global".into(),
            canonical_name: "玩家".into(),
        }],
    )
    .unwrap();
    assert!(catalog().with_entity_directory(non_character).is_err());

    let partial = catalog()
        .with_entity_directory(directory(script_revision, false))
        .unwrap();
    assert!(!partial.entity_directory().unwrap().is_complete());
    assert!(
        partial
            .entity_directory()
            .unwrap()
            .resolve_exact(EntityKind::Character, "global", "玩家")
            .is_err()
    );
}

#[test]
fn record_evidence_is_verified_against_confirmed_history_and_content_hash() {
    use crate::record::format::Body;
    let mut fixture = Fixture::new();
    let seq = fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "查看".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    let hash = format::hash(fixture.session.read(seq).unwrap().raw.as_bytes());
    let valid = EvidenceRef::Record {
        session_id: fixture.session.header().session_id.clone(),
        record_seq: seq,
        content_hash: hash,
    };
    for (proof, okay) in [
        (valid.clone(), true),
        (
            EvidenceRef::Record {
                session_id: "foreign".into(),
                record_seq: seq,
                content_hash: format::hash(b"bad"),
            },
            false,
        ),
        (
            EvidenceRef::Record {
                session_id: fixture.session.header().session_id.clone(),
                record_seq: seq + 1,
                content_hash: format::hash(b"bad"),
            },
            false,
        ),
        (
            EvidenceRef::Record {
                session_id: fixture.session.header().session_id.clone(),
                record_seq: seq,
                content_hash: format::hash(b"bad"),
            },
            false,
        ),
        (
            EvidenceRef::Script {
                script_revision: fixture.session.header().script_revision.clone(),
                rule_version: 0,
                key: "visited".into(),
                value_hash: format::hash(b"bad"),
            },
            false,
        ),
    ] {
        let result = ConfirmedWorldView::capture(
            &fixture.session,
            "0",
            BTreeMap::from([(
                "visited".into(),
                WorldFact {
                    value: Value::from(1),
                    evidence_refs: vec![proof],
                },
            )]),
        );
        assert_eq!(result.is_ok(), okay);
    }
    for value in [Value::String("x".repeat(4097)), serde_json::json!([])] {
        assert!(
            ConfirmedWorldView::capture(
                &fixture.session,
                "0",
                BTreeMap::from([(
                    "visited".into(),
                    WorldFact {
                        value,
                        evidence_refs: vec![valid.clone()]
                    }
                )])
            )
            .is_err()
        );
    }
    assert!(ConfirmedWorldView::capture(&fixture.session, "", BTreeMap::new()).is_err());
    assert!(
        ConfirmedWorldView::capture(
            &fixture.session,
            "0",
            BTreeMap::from([(
                "visited".into(),
                WorldFact {
                    value: Value::from(1),
                    evidence_refs: vec![]
                }
            )])
        )
        .is_err()
    );
    let view = ConfirmedWorldView::capture(
        &fixture.session,
        "0",
        BTreeMap::from([(
            "visited".into(),
            WorldFact {
                value: Value::from(1),
                evidence_refs: vec![valid],
            },
        )]),
    )
    .unwrap();
    let mut refs = vec![
        EvidenceRef::Script {
            script_revision: fixture.session.header().script_revision.clone(),
            rule_version: 1,
            key: "other".into(),
            value_hash: format::hash(b"other")
        };
        32
    ];
    assert!(
        evaluate(
            &Condition::AtLeast {
                key: "visited".into(),
                value: 0
            },
            &view,
            &mut refs
        )
        .is_err()
    );
}

#[test]
fn scene_candidates_have_stable_priority_deduplication_and_a_hard_bound() {
    let fixture = Fixture::new();
    let view = world(&fixture, Value::from(1));
    let mut candidate = catalog();
    candidate
        .conditions
        .insert("advance".into(), Condition::Always {});
    let template = candidate.scenes["room"].clone();
    for i in 0..34 {
        let id = format!("room-{i:02}");
        candidate.nodes.insert(
            id.clone(),
            Node {
                kind: "scene".into(),
                title: id.clone(),
                parent: None,
            },
        );
        let mut next = template.clone();
        next.position.scene_id = id.clone();
        next.position.path[0].id = id.clone();
        next.position.path[0].title = id.clone();
        next.rules.scene_id = id.clone();
        candidate.scenes.insert(id.clone(), next);
        let root = candidate.scenes.get_mut("room").unwrap();
        root.rules.advance_rule_ids = vec!["advance".into()];
        root.advance.push(Advance {
            rule_id: "advance".into(),
            target_scene_id: id,
            priority: i % 2,
        });
    }
    candidate
        .scenes
        .get_mut("room")
        .unwrap()
        .advance
        .push(Advance {
            rule_id: "advance".into(),
            target_scene_id: "room-01".into(),
            priority: 1,
        });
    candidate.conditions.insert(
        "blocked".into(),
        Condition::Not {
            condition: Box::new(Condition::Always {}),
        },
    );
    candidate
        .scenes
        .get_mut("room")
        .unwrap()
        .rules
        .advance_rule_ids
        .push("blocked".into());
    candidate
        .scenes
        .get_mut("room")
        .unwrap()
        .advance
        .push(Advance {
            rule_id: "blocked".into(),
            target_scene_id: "room".into(),
            priority: 999,
        });
    let candidate = register(candidate).unwrap();
    let (ids, truncated) = candidate.candidates("room", &view).unwrap();
    assert!(truncated);
    assert_eq!(ids.len(), 32);
    assert_eq!(ids[0], "room-01");
    assert_eq!(ids[17], "room-00");
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
    let mut candidate = candidate;
    candidate.conditions.insert(
        "advance".into(),
        Condition::AtLeast {
            key: "visited".into(),
            value: 1,
        },
    );
    let empty = ConfirmedWorldView::capture(&fixture.session, "0", BTreeMap::new()).unwrap();
    assert_eq!(
        candidate.candidates("room", &empty).unwrap_err().code,
        "engine.invalid-phase"
    );
}

#[test]
fn creation_rule_registration_rejects_empty_and_duplicate_rules() {
    assert!(catalog().with_entity_creation_rules(vec![]).is_err());
    let rule = EntityCreationRule::new(
        "generated".into(),
        EntityKind::Character,
        "global".into(),
        2,
    )
    .unwrap();
    assert!(
        catalog()
            .with_entity_creation_rules(vec![rule.clone(), rule])
            .is_err()
    );
}
