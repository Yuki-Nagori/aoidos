use super::*;

#[test]
fn new_sessions_freeze_original_and_prompt_hashes_separately() {
    let first = header("mistbell").unwrap();
    let second = header("mistbell").unwrap();
    assert_ne!(first.session_id, second.session_id);
    assert_eq!(first.script_revision, second.script_revision);
    assert_eq!(
        first.script_revision,
        super::super::assets::revision("mistbell").unwrap()
    );
    assert_eq!(
        first.static_prefix_hash,
        format::hash(first.static_prefix.as_bytes())
    );
    assert_ne!(first.static_prefix_hash, first.script_revision);
    assert!(
        first
            .static_prefix
            .starts_with("[AOIDOS:STATIC]\n# 雾钟地窖")
    );
    assert!(first.static_prefix.ends_with("\n[/AOIDOS:STATIC]\n"));
    assert_eq!(header("../mistbell").unwrap_err().code, "app.not-found");
}

struct MigrationEvents;
impl crate::migration::MigrationEvents for MigrationEvents {
    fn prepare(&self, _: &crate::migration::MigrationEvent) -> Result<u64, Fault> {
        Ok(1)
    }
    fn deliver(&self, _: u64, _: crate::migration::MigrationEvent) -> Result<(), Fault> {
        Ok(())
    }
    fn retire(&self, _: &str) {}
}
struct Fixture {
    root: std::path::PathBuf,
    storage: Arc<Storage>,
    session: Arc<Mutex<Session>>,
}
impl Fixture {
    fn new(edit: impl FnOnce(&mut Header)) -> Self {
        let root = std::env::temp_dir().join(format!("aoidos-builtin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Storage::new(
            &root,
            Arc::new(MigrationEvents),
            Arc::new(crate::record::test_support::Events::default()),
        ));
        let mut header = header("mistbell").unwrap();
        edit(&mut header);
        let session = storage.records.create(header).unwrap();
        Self {
            root,
            storage,
            session,
        }
    }
    async fn open(&self) -> Result<Arc<Context>, Fault> {
        open(
            self.storage.clone(),
            self.session.clone(),
            Coordinator::new(Arc::new(crate::test_support::Events::default()), 16).unwrap(),
            Arc::new(super::super::test_support::PhaseEventsFixture::default()),
        )
        .await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.storage.close().unwrap();
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[tokio::test]
async fn opening_a_run_establishes_one_initial_cycle_node_baseline() {
    let fixture = Fixture::new(|_| {});
    let session_id = fixture.session.lock().unwrap().header().session_id.clone();

    for _ in 0..2 {
        drop(fixture.open().await.unwrap());
        fixture
            .storage
            .with_database(|connection| {
                let run_id: String = connection.query_row(
                    "SELECT run_id FROM memory_run_sessions WHERE session_id=?1",
                    [&session_id],
                    |row| row.get(0),
                ).unwrap();
                let baseline_count: u64 = connection.query_row(
                    "SELECT count(*) FROM memory_cycle_scene_progress WHERE run_id=?1 AND scene_id='entry' AND entry_history_revision=0 AND entry_record_seq=0",
                    [&run_id],
                    |row| row.get(0),
                ).unwrap();
                let pending_count: u64 = connection.query_row(
                    "SELECT count(*) FROM memory_cycle_events WHERE run_id=?1",
                    [&run_id],
                    |row| row.get(0),
                ).unwrap();
                assert_eq!(baseline_count, 1);
                assert_eq!(pending_count, 0);
                Ok(())
            })
            .unwrap();
    }
}

#[tokio::test]
async fn reopening_applies_and_retires_a_prepared_initial_scene_event() {
    let fixture = Fixture::new(|_| {});
    let context = fixture.open().await.unwrap();
    let run_id = context.completed.run_id().unwrap();
    let (session_id, catalog_revision, world_revision) = {
        let session = fixture.session.lock().unwrap();
        let domain = context.domain.lock().unwrap();
        let world = domain.confirmed(&session).unwrap();
        (
            session.header().session_id.clone(),
            domain.catalog().revision().to_owned(),
            world.world_revision().to_owned(),
        )
    };
    let event = aoidos_memory::model::CycleLifecycleEvent {
        version: 1,
        event_id: aoidos_memory::model::CycleLifecycleEvent::event_id_for(
            &run_id,
            aoidos_memory::model::CycleEventKind::SceneEntered,
            0,
            0,
        ),
        run_id: run_id.clone(),
        session_id,
        kind: aoidos_memory::model::CycleEventKind::SceneEntered,
        source_record_seq: 0,
        history_revision: 0,
        scene_id: "entry".into(),
        entry_record_seq: 0,
        catalog_revision,
        world_revision,
        round_id: None,
        evidence: vec![],
    };
    context.completed.prepare_cycle_event(event).unwrap();
    drop(context);

    let _reopened = fixture.open().await.unwrap();
    fixture
        .storage
        .with_database(|connection| {
            let progress_count: u64 = connection.query_row(
                "SELECT count(*) FROM memory_cycle_scene_progress WHERE run_id=?1 AND scene_id='entry' AND entry_history_revision=0 AND entry_record_seq=0",
                [&run_id],
                |row| row.get(0),
            ).unwrap();
            let pending_count: u64 = connection.query_row(
                "SELECT count(*) FROM memory_cycle_events WHERE run_id=?1",
                [&run_id],
                |row| row.get(0),
            ).unwrap();
            assert_eq!(progress_count, 1);
            assert_eq!(pending_count, 0);
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn original_revision_and_framed_prefix_are_both_required_on_reopen() {
    for change in 0..3 {
        let fixture = Fixture::new(|header| match change {
            0 => header.script_revision = format::hash(b"old release"),
            1 => {
                header.static_prefix = header.static_prefix.replace("雾钟地窖", "旧版地窖");
                header.static_prefix_hash = format::hash(header.static_prefix.as_bytes());
            }
            _ => header.script_id = "unknown".into(),
        });
        let error = fixture.open().await.err().unwrap();
        assert_eq!(
            error.code,
            if change == 2 {
                "app.not-found"
            } else {
                "app.not-ready"
            }
        );
        assert_eq!(fixture.session.lock().unwrap().last_event_seq(), 0);
    }
}

#[tokio::test]
async fn legacy_session_reopens_with_its_original_entity_revision_for_memory_reads() {
    let legacy_revision = super::super::assets::legacy_revision("mistbell").unwrap();
    let fixture = Fixture::new(|header| header.script_revision = legacy_revision);
    let context = fixture.open().await.unwrap();
    let run_id = context.completed.run_id().unwrap();
    let (_, catalog) = fixture.storage.memory_catalog_for_run(&run_id).unwrap();
    assert_eq!(
        catalog.script_revision(),
        fixture.session.lock().unwrap().header().script_revision
    );
}
#[tokio::test]
async fn registered_domain_freezes_rules_and_commits_only_supported_idempotent_intents() {
    let fixture = Fixture::new(|_| {});
    let context = fixture.open().await.unwrap();
    let record = fixture.session.lock().unwrap();
    let mut domain = context.domain.lock().unwrap();
    assert_eq!(domain.baseline_scene().unwrap().scene_id, "entry");
    assert_eq!(
        domain
            .catalog()
            .scene("entry")
            .unwrap()
            .forced_check
            .as_deref(),
        None
    );
    let view = domain.confirmed(&record).unwrap();
    assert!(domain.context(&view).unwrap().contains("未登记"));
    let plan = domain
        .check_plan("pbta-explore-v1", "player", &view)
        .unwrap();
    assert_eq!(plan.expression, "2d6");
    assert!(plan.modifiers.is_empty());
    for (rule, actor) in [("invented-rule", "player"), ("pbta-explore-v1", "npc")] {
        assert_eq!(
            domain.check_plan(rule, actor, &view).unwrap_err().code,
            "engine.invalid-phase"
        );
    }
    let stale = ConfirmedWorldView::capture(&record, "old-release", BTreeMap::new()).unwrap();
    assert_eq!(
        domain
            .check_plan("pbta-explore-v1", "player", &stale)
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    assert!(domain.exit_reason("entry", &view).unwrap().is_none());
    let id = uuid::Uuid::new_v4().to_string();
    let hash = format::hash(b"end-session");
    let mut mutation = WorldMutation {
        mutation_id: id.clone(),
        data: MutationData {
            version: 2,
            kind: "sessionEnded".into(),
            payload: serde_json::json!({}),
        },
    };
    assert_eq!(
        WorldPort::validate(domain.as_ref(), &record, std::slice::from_ref(&mutation))
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    assert_eq!(
        WorldPort::apply(domain.as_mut(), &record, 1, &mutation, &hash)
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    assert!(!WorldPort::applied(domain.as_ref(), &id, &hash).unwrap());
    mutation.data.version = 1;
    WorldPort::validate(domain.as_ref(), &record, std::slice::from_ref(&mutation)).unwrap();
    WorldPort::apply(domain.as_mut(), &record, 1, &mutation, &hash).unwrap();
    WorldPort::apply(domain.as_mut(), &record, 1, &mutation, &hash).unwrap();
    assert!(WorldPort::applied(domain.as_ref(), &id, &hash).unwrap());
    assert!(HistoryPort::applied(domain.as_ref(), &id, &hash).unwrap());
    assert!(
        WorldPort::apply(
            domain.as_mut(),
            &record,
            1,
            &mutation,
            &format::hash(b"conflict")
        )
        .is_err()
    );
    let rewind_id = uuid::Uuid::new_v4().to_string();
    domain.rebuild(&record, &[], &rewind_id, &hash).unwrap();
    domain.rebuild(&record, &[], &rewind_id, &hash).unwrap();
    assert!(HistoryPort::applied(domain.as_ref(), &rewind_id, &hash).unwrap());
    assert!(domain.verify_target(&record, 99).is_err());
    assert!(domain.verify_replay_target(&record, 99, 100).is_err());
    fixture
        .storage
        .with_database(|connection| {
            connection
                .execute_batch("DROP TABLE store_applied")
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(WorldPort::applied(domain.as_ref(), &id, &hash).is_err());
    assert!(WorldPort::apply(domain.as_mut(), &record, 1, &mutation, &hash).is_err());
    assert!(domain.rebuild(&record, &[], &rewind_id, &hash).is_err());
    drop(domain);
    drop(record);
    fixture.storage.close().unwrap();
    let record = fixture.session.lock().unwrap();
    assert_eq!(
        context
            .domain
            .lock()
            .unwrap()
            .confirmed(&record)
            .unwrap_err()
            .code,
        "app.not-ready"
    );
}
#[tokio::test]
async fn read_only_completion_does_not_create_world_consequences() {
    let fixture = Fixture::new(|_| {});
    let context = fixture.open().await.unwrap();
    let synthetic = super::super::test_support::Fixture::new(false, "stay");
    let mut runner = synthetic.accept("探索", DiceMode::Auto).await;
    let round = super::super::recovery::derive(
        &synthetic.context.session.lock().unwrap(),
        synthetic.context.domain.lock().unwrap().baseline_scene(),
    )
    .unwrap()
    .round
    .unwrap();
    let completed = {
        let record = fixture.session.lock().unwrap();
        let domain = context.domain.lock().unwrap();
        let view = domain.confirmed(&record).unwrap();
        assert!(
            domain
                .consequences(&record, &round, &view)
                .unwrap()
                .is_empty()
        );
        CompletedRound {
            session_id: record.header().session_id.clone(),
            round_id: round.identity.round_id,
            history_revision: 0,
            through_seq: 0,
        }
    };
    context.completed.completed(completed);
    {
        let record = fixture.session.lock().unwrap();
        assert_eq!(record.last_event_seq(), 0);
    }
    runner
        .finish(crate::ports::Outcome::Cancelled, None)
        .await
        .unwrap();
    let domain = BuiltinDomain {
        catalog: super::super::test_support::domain(
            &format::hash(b"revision"),
            false,
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
        .catalog,
        storage: fixture.storage.clone(),
        revision: "revision".into(),
        entity_seed: super::super::assets::entity_directory("mistbell").unwrap(),
    };
    assert!(domain.baseline_scene().is_none());
}

#[tokio::test]
async fn builtin_domain_ports_fail_closed_and_recovery_delegates_supported_facts() {
    let fixture = Fixture::new(|_| {});
    let context = fixture.open().await.unwrap();
    let mut domain = context.domain.lock().unwrap();
    let session = fixture.session.lock().unwrap();

    let mutation = |kind: &str, version| WorldMutation {
        mutation_id: uuid::Uuid::new_v4().to_string(),
        data: MutationData {
            version,
            kind: kind.into(),
            payload: serde_json::json!({}),
        },
    };
    for invalid in [mutation("unknown", 1), mutation("sessionEnded", 2)] {
        assert_eq!(
            WorldPort::validate(domain.as_ref(), &session, std::slice::from_ref(&invalid))
                .unwrap_err()
                .code,
            "engine.invalid-phase"
        );
        assert_eq!(
            WorldPort::apply(
                domain.as_mut(),
                &session,
                1,
                &invalid,
                &format::hash(b"invalid builtin mutation"),
            )
            .unwrap_err()
            .code,
            "engine.invalid-phase"
        );
    }

    let invalid_entity = mutation("entityCreated", 1);
    assert!(
        WorldPort::validate(
            domain.as_ref(),
            &session,
            std::slice::from_ref(&invalid_entity)
        )
        .is_err()
    );

    let ended = mutation("sessionEnded", 1);
    WorldPort::validate_recovery(domain.as_ref(), &session, 1, std::slice::from_ref(&ended))
        .unwrap();
    let hash = format::hash(b"builtin session ended");
    WorldPort::apply(domain.as_mut(), &session, 1, &ended, &hash).unwrap();
    WorldPort::apply(domain.as_mut(), &session, 1, &ended, &hash).unwrap();
    assert!(WorldPort::applied(domain.as_ref(), &ended.mutation_id, &hash).unwrap());

    let invalid_advance = Fact::SceneAdvanced {
        identity: crate::record::facts::RoundIdentity {
            version: 1,
            round_id: uuid::Uuid::new_v4().to_string(),
            operation_id: uuid::Uuid::new_v4().to_string(),
        },
        previous_scene_id: "entry".into(),
        position: ScenePosition {
            scene_id: "unregistered".into(),
            path: vec![],
        },
        world_revision: "0".into(),
        mutation_id: uuid::Uuid::new_v4().to_string(),
    }
    .body("invalid advance")
    .unwrap();
    let crate::record::format::Body::System { code, data, .. } = invalid_advance else {
        panic!("scene advance encodes as a system fact");
    };
    let invalid_advance = WorldMutation {
        mutation_id: uuid::Uuid::new_v4().to_string(),
        data: MutationData {
            version: 1,
            kind: code,
            payload: data,
        },
    };
    assert!(
        WorldPort::apply(
            domain.as_mut(),
            &session,
            2,
            &invalid_advance,
            &format::hash(b"invalid scene advance"),
        )
        .is_err()
    );
}

#[tokio::test]
async fn builtin_presence_snapshot_and_validation_use_the_registered_catalog() {
    let fixture = Fixture::new(|_| {});
    let context = fixture.open().await.unwrap();
    let domain = context.domain.lock().unwrap();
    let session = fixture.session.lock().unwrap();

    let snapshot = Domain::scene_presence(domain.as_ref(), &session).unwrap();
    assert_eq!(snapshot.scene_id, "entry");
    assert!(
        snapshot
            .characters
            .iter()
            .any(|entity| entity.id == "player")
    );

    let invalid_change = WorldMutation {
        mutation_id: uuid::Uuid::new_v4().to_string(),
        data: MutationData {
            version: 1,
            kind: "scenePresenceChanged".into(),
            payload: serde_json::json!({}),
        },
    };
    assert!(
        WorldPort::validate(domain.as_ref(), &session, &[invalid_change]).is_err(),
        "registered presence policy rejects malformed change payloads"
    );
    let invalid_recovery_change = WorldMutation {
        mutation_id: uuid::Uuid::new_v4().to_string(),
        data: MutationData {
            version: 1,
            kind: "scenePresenceChanged".into(),
            payload: serde_json::json!({}),
        },
    };
    assert_eq!(
        WorldPort::validate_recovery(
            domain.as_ref(),
            &session,
            1,
            std::slice::from_ref(&invalid_recovery_change),
        )
        .unwrap_err()
        .code,
        "engine.invalid-phase",
        "recovery rejects presence intents that are not on the current history path"
    );
}

#[tokio::test]
async fn builtin_open_rejects_incomplete_entity_seed_before_context_creation() {
    let fixture = Fixture::new(|_| {});
    let seed = super::super::assets::entity_directory("mistbell").unwrap();
    let incomplete = EntityDirectory::new(
        seed.script_revision().into(),
        seed.revision(),
        false,
        vec![CanonicalEntity {
            id: "player".into(),
            kind: EntityKind::Character,
            scope: "global".into(),
            canonical_name: "玩家".into(),
        }],
    )
    .unwrap();
    let error = open_context(
        fixture.storage.clone(),
        fixture.session.clone(),
        Coordinator::new(Arc::new(crate::test_support::Events::default()), 16).unwrap(),
        Arc::new(super::super::test_support::PhaseEventsFixture::default()),
        "雾钟地窖".into(),
        seed.script_revision().into(),
        incomplete,
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.code, "app.bad-request");
    assert_eq!(fixture.session.lock().unwrap().last_event_seq(), 0);
}

#[test]
fn default_domain_presence_and_bounded_recovery_cache_reject_unsupported_state() {
    let fixture = crate::record::test_support::Fixture::new();
    let domain = super::super::test_support::domain(
        &fixture.session.header().script_revision,
        false,
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    );
    assert_eq!(
        Domain::scene_presence(&domain, &fixture.session)
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    assert_eq!(
        crate::game::recovery::derive_evidence_records(&BTreeMap::new(), vec![1], 2)
            .unwrap_err()
            .code,
        "store.corrupt"
    );
}

#[tokio::test]
async fn invalid_registered_catalog_metadata_refuses_context_before_record_changes() {
    let fixture = Fixture::new(|_| {});
    let error = open_context(
        fixture.storage.clone(),
        fixture.session.clone(),
        Coordinator::new(Arc::new(crate::test_support::Events::default()), 16).unwrap(),
        Arc::new(super::super::test_support::PhaseEventsFixture::default()),
        "中".repeat(86),
        super::super::assets::revision("mistbell").unwrap(),
        super::super::assets::entity_directory("mistbell").unwrap(),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.code, "app.bad-request");
    assert_eq!(fixture.session.lock().unwrap().last_event_seq(), 0);
    assert!(fixture.open().await.is_ok());
}

#[test]
fn builtin_domain_commits_registered_dynamic_identity_in_shared_storage() {
    let fixture = Fixture::new(|_| {});
    let seed = super::super::assets::entity_directory("mistbell").unwrap();
    let mut domain = BuiltinDomain {
        catalog: super::super::test_support::domain(
            seed.script_revision(),
            false,
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
        .catalog
        .with_entity_creation_rules(vec![
            EntityCreationRule::new(
                "generated".into(),
                EntityKind::Character,
                "global".into(),
                2,
            )
            .unwrap(),
        ])
        .unwrap(),
        storage: fixture.storage.clone(),
        revision: seed.script_revision().into(),
        entity_seed: seed.clone(),
    };
    let mut session = fixture.session.lock().unwrap();
    let mutation = domain
        .catalog
        .prepare_entity_creation_for_session(
            &session.header().session_id,
            "generated",
            "New guide",
            &seed,
        )
        .unwrap();
    let mut unsupported_mutation = mutation.clone();
    unsupported_mutation.data.kind = "unknown".into();
    assert!(WorldPort::validate(&domain, &session, &[unsupported_mutation]).is_err());
    session
        .append(crate::record::format::Body::Narration {
            text: "Confirmed introduction".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: crate::ports::Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    session.commit_world(crate::record::format::Body::System {
        code: "settlementPlanned".into(), message: "Create registered identity".into(), related_seq: None, turn_id: None,
        data: serde_json::json!({"version":1,"roundId":uuid::Uuid::new_v4().to_string(),"operationId":uuid::Uuid::new_v4().to_string(),"narrativeSeq":1,"worldRevision":"0","mutations":[mutation]}),
    }, &mut domain).unwrap();
    let snapshot = fixture
        .storage
        .with_database(|connection| EntityRegistry.snapshot(connection, &session, &seed))
        .unwrap();
    assert!(
        snapshot
            .resolve_exact(EntityKind::Character, "global", "New guide")
            .unwrap()
            .is_some()
    );
}
