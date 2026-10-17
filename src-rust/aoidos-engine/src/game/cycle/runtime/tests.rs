use super::*;
use crate::{
    game::{
        catalog::{Condition, HintRule, Node, Scene, SceneRuleView},
        domain::CompletedRounds,
        presence::{PresencePolicy, SceneEntryPresence},
    },
    migration::{MigrationEvent, MigrationEvents},
    record::{
        facts::{
            DiceMode, Fact, ForkMode, GenerationTarget, RoundIdentity, SceneNode, ScenePosition,
            WorldMutation,
        },
        format::{Header, InputMode},
        history::HistoryPort,
        world::WorldPort,
    },
};
use aoidos_memory::model::{CycleEventKind, CycleLifecycleEvent};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Default)]
struct Events(AtomicU64);

impl MigrationEvents for Events {
    fn prepare(&self, _: &MigrationEvent) -> Result<u64, Fault> {
        Ok(self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }

    fn deliver(&self, _: u64, _: MigrationEvent) -> Result<(), Fault> {
        Ok(())
    }

    fn retire(&self, _: &str) {}
}

struct SceneMutationPort(Arc<crate::storage::Storage>);

impl WorldPort for SceneMutationPort {
    fn validate(
        &self,
        _: &crate::record::session::Session,
        mutations: &[WorldMutation],
    ) -> Result<(), Fault> {
        if mutations
            .iter()
            .all(|mutation| mutation.data.version == 1 && mutation.data.kind == "sceneAdvanced")
        {
            Ok(())
        } else {
            Err(Fault::bad_request())
        }
    }

    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        self.0.with_database(|connection| {
            aoidos_store::applied::contains(connection, id, hash).map_err(Fault::from)
        })
    }

    fn apply(
        &mut self,
        _: &crate::record::session::Session,
        _: u64,
        mutation: &WorldMutation,
        hash: &str,
    ) -> Result<(), Fault> {
        self.0.with_database(|connection| {
            aoidos_store::applied::apply_once(connection, &mutation.mutation_id, hash, &mut |_| {
                Ok(())
            })
            .map(|_| ())
            .map_err(Fault::from)
        })
    }
}

struct HistoryRebuildPort(Arc<crate::storage::Storage>);

impl HistoryPort for HistoryRebuildPort {
    fn verify_target(
        &self,
        session: &crate::record::session::Session,
        target: u64,
    ) -> Result<(), Fault> {
        session
            .read(target)
            .map(drop)
            .map_err(crate::record::session::store_fault)
    }

    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        self.0.with_database(|connection| {
            aoidos_store::applied::contains(connection, id, hash).map_err(Fault::from)
        })
    }

    fn rebuild(
        &mut self,
        _: &crate::record::session::Session,
        _: &[u64],
        id: &str,
        hash: &str,
    ) -> Result<(), Fault> {
        self.0.with_database(|connection| {
            aoidos_store::applied::apply_once(connection, id, hash, &mut |_| Ok(()))
                .map(|_| ())
                .map_err(Fault::from)
        })
    }
}

struct RuntimeFixture {
    root: std::path::PathBuf,
    storage: Arc<crate::storage::Storage>,
    session_id: String,
    session: Arc<std::sync::Mutex<crate::record::session::Session>>,
    catalog: Arc<crate::game::catalog::SceneCatalog>,
    run: aoidos_memory::model::Run,
}

impl RuntimeFixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("aoidos-cycle-runtime-{}", uuid::Uuid::new_v4()));
        let storage = Arc::new(crate::storage::Storage::new(
            &root,
            Arc::new(Events::default()),
            Arc::new(crate::record::test_support::Events::default()),
        ));
        assert!(
            storage.migration.snapshot().error.is_none(),
            "{:?}",
            storage.migration.snapshot()
        );
        let header = crate::game::builtin::header("mistbell").unwrap();
        let session_id = header.session_id.clone();
        let session = storage.records.create(header.clone()).unwrap();
        let catalog = Arc::new(catalog(&header));
        let run = storage
            .initialize_memory_run(&session_id, &catalog)
            .unwrap();
        let runtime = CycleRuntime::new(storage.clone(), catalog.clone(), session_id.clone());
        runtime.set_run_id(run.run_id.clone()).unwrap();
        runtime.ensure_current_node("entry", 0, &[]).unwrap();
        Self {
            root,
            storage,
            session_id,
            session,
            catalog,
            run,
        }
    }

    fn runtime(&self) -> CycleRuntime {
        let runtime = CycleRuntime::new(
            self.storage.clone(),
            self.catalog.clone(),
            self.session_id.clone(),
        );
        runtime.set_run_id(self.run.run_id.clone()).unwrap();
        runtime
    }

    fn reopen_storage(&mut self) {
        self.storage.close().unwrap();
        let storage = Arc::new(crate::storage::Storage::new(
            &self.root,
            Arc::new(Events::default()),
            Arc::new(crate::record::test_support::Events::default()),
        ));
        let session = storage
            .records
            .open(&self.run.script_id, &self.session_id)
            .unwrap();
        session
            .lock()
            .unwrap()
            .recover_world(&mut SceneMutationPort(storage.clone()))
            .unwrap();
        self.storage = storage;
        self.session = session;
    }
}

impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        self.storage.close().unwrap();
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn prepare_completed_round(
    fixture: &RuntimeFixture,
    runtime: &CycleRuntime,
    input_seq: u64,
) -> u64 {
    let history_revision = fixture.session.lock().unwrap().history_revision();
    let round_id = uuid::Uuid::new_v4().to_string();
    let identity = RoundIdentity {
        version: 1,
        round_id: round_id.clone(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let accepted_seq = fixture
        .session
        .lock()
        .unwrap()
        .append(
            Fact::RoundAccepted {
                identity: identity.clone(),
                input_seq,
                mode: InputMode::InCharacter,
                dice_mode: DiceMode::Manual,
                profile_id: "profile".into(),
                profile_revision: "profile-v1".into(),
                target: GenerationTarget::Narration,
                scene_id: "entry".into(),
                presence_revision: None,
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("接纳回合")
            .unwrap(),
        )
        .unwrap();
    let source_record_seq = fixture.session.lock().unwrap().next_sequence();
    runtime
        .prepare_cycle_event(CycleLifecycleEvent {
            version: 1,
            event_id: CycleLifecycleEvent::event_id_for(
                &fixture.run.run_id,
                CycleEventKind::RoundCompleted,
                history_revision,
                source_record_seq,
            ),
            run_id: fixture.run.run_id.clone(),
            session_id: fixture.session_id.clone(),
            kind: CycleEventKind::RoundCompleted,
            source_record_seq,
            history_revision,
            scene_id: "entry".into(),
            entry_record_seq: 0,
            catalog_revision: fixture.catalog.revision().into(),
            world_revision: "world-v1".into(),
            round_id: Some(round_id),
            evidence: vec![],
        })
        .unwrap();
    fixture
        .session
        .lock()
        .unwrap()
        .append(
            Fact::RoundEnded {
                identity,
                outcome: crate::ports::Outcome::Completed,
                through_seq: accepted_seq,
                completed_steps: vec![],
                error: None,
            }
            .body("回合结束")
            .unwrap(),
        )
        .unwrap();
    source_record_seq
}

fn catalog(header: &Header) -> crate::game::catalog::SceneCatalog {
    let revision = "catalog-v1";
    let entry = SceneEntryPresence {
        rule_id: "entry-characters".into(),
        rule_version: 1,
        character_ids: vec!["player".into()],
    };
    crate::game::catalog::SceneCatalog::new(
        revision.into(),
        header.script_revision.clone(),
        BTreeMap::from([(
            "entry".into(),
            Node {
                kind: "scene".into(),
                title: "入口".into(),
                parent: None,
            },
        )]),
        BTreeMap::from([(
            "entry".into(),
            Scene {
                position: crate::record::facts::ScenePosition {
                    scene_id: "entry".into(),
                    path: vec![SceneNode {
                        kind: "scene".into(),
                        id: "entry".into(),
                        title: "入口".into(),
                    }],
                },
                rules: SceneRuleView {
                    scene_id: "entry".into(),
                    catalog_revision: revision.into(),
                    advance_rule_ids: vec![],
                    progress_rule_ids: vec![],
                    hint_rules: vec![HintRule {
                        hint_id: "entry-hint".into(),
                        when_rule_id: "always".into(),
                        text: "石门边缘留有一道浅痕。".into(),
                        priority: 1,
                        target_scope: "scene".into(),
                    }],
                    exit_rule_ids: vec![],
                },
                advance: vec![],
                check_rule_ids: vec![],
                actor_ids: vec!["player".into()],
                forced_check: None,
                dice_disabled: true,
            },
        )]),
        BTreeMap::from([("always".into(), Condition::Always {})]),
    )
    .unwrap()
    .with_entity_directory(crate::game::assets::entity_directory("mistbell").unwrap())
    .unwrap()
    .with_presence_policy(
        PresencePolicy::new(
            "entry".into(),
            entry.clone(),
            BTreeMap::from([("entry".into(), entry)]),
            vec![],
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn scene_hint_is_returned_only_after_the_current_node_claim_is_persisted() {
    let root = std::env::temp_dir().join(format!("aoidos-cycle-hint-{}", uuid::Uuid::new_v4()));
    let storage = Arc::new(crate::storage::Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(crate::record::test_support::Events::default()),
    ));
    let header = crate::game::builtin::header("mistbell").unwrap();
    let session_id = header.session_id.clone();
    storage.records.create(header.clone()).unwrap();
    let catalog = Arc::new(catalog(&header));
    let run = storage
        .initialize_memory_run(&session_id, &catalog)
        .unwrap();
    let runtime = CycleRuntime::new(storage.clone(), catalog.clone(), session_id.clone());
    runtime.set_run_id(run.run_id.clone()).unwrap();
    runtime.ensure_current_node("entry", 0, &[]).unwrap();

    // 进度计数由仓储测试覆盖；这里直接构造已停滞节点，专测规则到领取的引擎接线。
    storage
        .with_database(|connection| {
            connection
                .execute(
                    "UPDATE memory_cycle_scene_progress SET revision=?2, completed_rounds=?2, stagnation_rounds=?2, last_round_id=?3, last_evidence_hash=?4 WHERE run_id=?1 AND scene_id='entry' AND entry_history_revision=0 AND entry_record_seq=0",
                    rusqlite::params![
                        &run.run_id,
                        aoidos_memory::model::NODE_HINT_AFTER_STAGNANT_ROUNDS,
                        uuid::Uuid::new_v4().to_string(),
                        vec![0_u8; 32],
                    ],
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    storage
        .records
        .get(&session_id)
        .unwrap()
        .lock()
        .unwrap()
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "我沿着石门查看。".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    let orphan = CycleLifecycleEvent {
        version: 1,
        event_id: CycleLifecycleEvent::event_id_for(
            &run.run_id,
            CycleEventKind::RoundCompleted,
            0,
            1,
        ),
        run_id: run.run_id.clone(),
        session_id: session_id.clone(),
        kind: CycleEventKind::RoundCompleted,
        source_record_seq: 1,
        history_revision: 0,
        scene_id: "entry".into(),
        entry_record_seq: 0,
        catalog_revision: catalog.revision().into(),
        world_revision: "world-v1".into(),
        round_id: Some(uuid::Uuid::new_v4().to_string()),
        evidence: vec![],
    };
    runtime.prepare_cycle_event(orphan.clone()).unwrap();
    let (history_revision, world_revision, matches) = {
        let session = storage.records.get(&session_id).unwrap();
        let session = session.lock().unwrap();
        let world = crate::game::catalog::ConfirmedWorldView::capture(
            &session,
            "world-v1",
            BTreeMap::new(),
        )
        .unwrap();
        let matched = catalog
            .evaluate_scene_rule(
                "entry",
                "always",
                &world,
                world.world_revision(),
                world.history_revision(),
            )
            .unwrap();
        (
            world.history_revision(),
            world.world_revision().to_owned(),
            vec![matched],
        )
    };

    // 尚未能核验的已完成事件必须阻止读取旧停滞进度和展示提示。
    assert!(runtime.drain_cycle_events().is_err());
    let session_handle = storage.records.get(&session_id).unwrap();
    let session = session_handle.lock().unwrap();
    assert!(
        runtime
            .claim_scene_hint(
                &session,
                "entry",
                history_revision,
                &world_revision,
                &matches
            )
            .is_err()
    );
    storage
        .with_memory_repository(|repository, connection| {
            repository.delete_cycle_event(connection, &run.run_id, &orphan.event_id)
        })
        .unwrap();

    let hint = runtime
        .claim_scene_hint(
            &session,
            "entry",
            history_revision,
            &world_revision,
            &matches,
        )
        .unwrap()
        .unwrap();
    assert_eq!(hint.hint_id, "entry-hint");
    assert_eq!(hint.text, "石门边缘留有一道浅痕。");
    assert_eq!(hint.target_scope, "scene");
    assert!(
        runtime
            .claim_scene_hint(
                &session,
                "entry",
                history_revision,
                &world_revision,
                &matches
            )
            .unwrap()
            .is_none()
    );
    drop(session);

    storage
        .with_memory(&session_id, &catalog, |repository, connection, port| {
            let state = repository
                .scene_progress(
                    connection,
                    port,
                    &run.run_id,
                    SceneNodeKey {
                        scene_id: "entry",
                        history_revision: 0,
                        entry_record_seq: 0,
                    },
                )?
                .unwrap();
            assert_eq!(state.hints_shown, 1);
            Ok(())
        })
        .unwrap();

    storage.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn committed_scene_advance_recovers_its_node_after_storage_reopens() {
    let mut fixture = RuntimeFixture::new();
    let runtime = fixture.runtime();

    let source_record_seq = fixture.session.lock().unwrap().next_sequence();
    runtime
        .prepare_cycle_event(CycleLifecycleEvent {
            version: 1,
            event_id: CycleLifecycleEvent::event_id_for(
                &fixture.run.run_id,
                CycleEventKind::SceneEntered,
                0,
                source_record_seq,
            ),
            run_id: fixture.run.run_id.clone(),
            session_id: fixture.session_id.clone(),
            kind: CycleEventKind::SceneEntered,
            source_record_seq,
            history_revision: 0,
            scene_id: "entry".into(),
            entry_record_seq: source_record_seq,
            catalog_revision: fixture.catalog.revision().into(),
            world_revision: "world-v1".into(),
            round_id: None,
            evidence: vec![],
        })
        .unwrap();
    let mut mutation_port = SceneMutationPort(fixture.storage.clone());
    fixture
        .session
        .lock()
        .unwrap()
        .commit_world(
            Fact::SceneAdvanced {
                identity: RoundIdentity {
                    version: 1,
                    round_id: uuid::Uuid::new_v4().to_string(),
                    operation_id: uuid::Uuid::new_v4().to_string(),
                },
                previous_scene_id: "entry".into(),
                position: ScenePosition {
                    scene_id: "entry".into(),
                    path: vec![SceneNode {
                        kind: "scene".into(),
                        id: "entry".into(),
                        title: "入口".into(),
                    }],
                },
                world_revision: "world-v1".into(),
                mutation_id: uuid::Uuid::new_v4().to_string(),
            }
            .body("场景切换")
            .unwrap(),
            &mut mutation_port,
        )
        .unwrap();

    drop(runtime);
    fixture.reopen_storage();
    let reopened = fixture.runtime();
    reopened.drain_cycle_events().unwrap();
    let state = fixture
        .storage
        .with_memory(
            &fixture.session_id,
            &fixture.catalog,
            |repository, connection, port| {
                let state = repository
                    .scene_progress(
                        connection,
                        port,
                        &fixture.run.run_id,
                        SceneNodeKey {
                            scene_id: "entry",
                            history_revision: 0,
                            entry_record_seq: source_record_seq,
                        },
                    )?
                    .unwrap();
                assert!(
                    repository
                        .pending_cycle_events(
                            connection,
                            &fixture.run.run_id,
                            MAX_CYCLE_EVENT_PAGE
                        )?
                        .is_empty()
                );
                Ok(state)
            },
        )
        .unwrap();
    assert_eq!(state.entry_record_seq, source_record_seq);
    assert_eq!(state.completed_rounds, 0);
}

#[test]
fn completed_round_recovery_replays_after_event_retirement_failure() {
    let fixture = RuntimeFixture::new();
    let runtime = fixture.runtime();
    let input_seq = fixture
        .session
        .lock()
        .unwrap()
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "我检查石门。".into(),
            mode: Some(InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    prepare_completed_round(&fixture, &runtime, input_seq);

    fixture
        .storage
        .with_database(|connection| {
            connection.execute_batch(
                "CREATE TRIGGER reject_cycle_event_delete BEFORE DELETE ON memory_cycle_events BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            )
            .map_err(aoidos_store::db::sqlite_error)?;
            Ok(())
        })
        .unwrap();
    assert!(runtime.drain_cycle_events().is_err());
    let applied_before_retry = fixture
        .storage
        .with_memory(
            &fixture.session_id,
            &fixture.catalog,
            |repository, connection, port| {
                let state = repository
                    .scene_progress(
                        connection,
                        port,
                        &fixture.run.run_id,
                        SceneNodeKey {
                            scene_id: "entry",
                            history_revision: 0,
                            entry_record_seq: 0,
                        },
                    )?
                    .unwrap();
                assert_eq!(
                    repository
                        .pending_cycle_events(
                            connection,
                            &fixture.run.run_id,
                            MAX_CYCLE_EVENT_PAGE
                        )?
                        .len(),
                    1
                );
                let logical_clock = repository.clock(connection, port, &fixture.run.run_id)?;
                Ok((state, logical_clock))
            },
        )
        .unwrap();
    assert_eq!(applied_before_retry.0.completed_rounds, 1);
    assert_eq!(applied_before_retry.0.stagnation_rounds, 1);
    assert_eq!(applied_before_retry.1, 1);

    fixture
        .storage
        .with_database(|connection| {
            connection
                .execute_batch("DROP TRIGGER reject_cycle_event_delete")
                .map_err(aoidos_store::db::sqlite_error)?;
            Ok(())
        })
        .unwrap();
    runtime.drain_cycle_events().unwrap();
    let applied_after_retry = fixture
        .storage
        .with_memory(
            &fixture.session_id,
            &fixture.catalog,
            |repository, connection, port| {
                let state = repository
                    .scene_progress(
                        connection,
                        port,
                        &fixture.run.run_id,
                        SceneNodeKey {
                            scene_id: "entry",
                            history_revision: 0,
                            entry_record_seq: 0,
                        },
                    )?
                    .unwrap();
                assert!(
                    repository
                        .pending_cycle_events(
                            connection,
                            &fixture.run.run_id,
                            MAX_CYCLE_EVENT_PAGE
                        )?
                        .is_empty()
                );
                let logical_clock = repository.clock(connection, port, &fixture.run.run_id)?;
                Ok((state, logical_clock))
            },
        )
        .unwrap();
    assert_eq!(applied_after_retry.0.completed_rounds, 1);
    assert_eq!(applied_after_retry.0.stagnation_rounds, 1);
    assert_eq!(applied_after_retry.1, 1);
}

#[test]
fn completed_round_clock_is_reconciled_when_history_is_forked() {
    let fixture = RuntimeFixture::new();
    let runtime = fixture.runtime();
    let input_seq = fixture
        .session
        .lock()
        .unwrap()
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "我重新观察石门。".into(),
            mode: Some(InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    prepare_completed_round(&fixture, &runtime, input_seq);
    runtime.drain_cycle_events().unwrap();
    let clock_before_fork = fixture
        .storage
        .with_memory(
            &fixture.session_id,
            &fixture.catalog,
            |repository, connection, port| repository.clock(connection, port, &fixture.run.run_id),
        )
        .unwrap();
    assert_eq!(clock_before_fork, 1);

    let mut session = fixture.session.lock().unwrap();
    let path = session
        .path_at(session.history_revision())
        .unwrap()
        .into_iter()
        .filter(|seq| *seq <= input_seq)
        .collect::<Vec<_>>();
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    for seq in &path {
        digest.update(session.read(*seq).unwrap().raw.as_bytes());
    }
    let fork = Fact::HistoryFork {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        mode: ForkMode::Rewind,
        parent_control_seq: session.history_revision(),
        target_seq: input_seq,
        prefix_hash: format!("sha256:{:x}", digest.finalize()),
        source_round_id: None,
        reused_dice_seq: None,
    }
    .body("回退到回合输入")
    .unwrap();
    session
        .commit_history_fork(fork, &mut HistoryRebuildPort(fixture.storage.clone()))
        .unwrap();
    drop(session);

    fixture.runtime().drain_cycle_events().unwrap();
    let clock_after_fork = fixture
        .storage
        .with_memory(
            &fixture.session_id,
            &fixture.catalog,
            |repository, connection, port| repository.clock(connection, port, &fixture.run.run_id),
        )
        .unwrap();
    assert_eq!(clock_after_fork, 0);

    let fork_revision = fixture.session.lock().unwrap().history_revision();
    fixture
        .runtime()
        .ensure_current_node("entry", fork_revision, &[])
        .unwrap();
    prepare_completed_round(&fixture, &fixture.runtime(), input_seq);
    fixture.runtime().drain_cycle_events().unwrap();
    let next_clock = fixture
        .storage
        .with_memory(
            &fixture.session_id,
            &fixture.catalog,
            |repository, connection, port| repository.clock(connection, port, &fixture.run.run_id),
        )
        .unwrap();
    assert_eq!(next_clock, 1);
}

#[test]
fn pending_completed_round_survives_a_fork_that_keeps_its_source() {
    let fixture = RuntimeFixture::new();
    let runtime = fixture.runtime();
    let input_seq = fixture
        .session
        .lock()
        .unwrap()
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "我已完成检查。".into(),
            mode: Some(InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let completed_seq = prepare_completed_round(&fixture, &runtime, input_seq);

    let mut session = fixture.session.lock().unwrap();
    let path = session
        .path_at(session.history_revision())
        .unwrap()
        .into_iter()
        .filter(|seq| *seq <= completed_seq)
        .collect::<Vec<_>>();
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    for seq in &path {
        digest.update(session.read(*seq).unwrap().raw.as_bytes());
    }
    let fork = Fact::HistoryFork {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        mode: ForkMode::Rewind,
        parent_control_seq: session.history_revision(),
        target_seq: completed_seq,
        prefix_hash: format!("sha256:{:x}", digest.finalize()),
        source_round_id: None,
        reused_dice_seq: None,
    }
    .body("保留已完成回合")
    .unwrap();
    session
        .commit_history_fork(fork, &mut HistoryRebuildPort(fixture.storage.clone()))
        .unwrap();
    drop(session);

    fixture.runtime().drain_cycle_events().unwrap();
    let (clock, pending) = fixture
        .storage
        .with_memory(
            &fixture.session_id,
            &fixture.catalog,
            |repository, connection, port| {
                Ok((
                    repository.clock(connection, port, &fixture.run.run_id)?,
                    repository.pending_cycle_events(
                        connection,
                        &fixture.run.run_id,
                        MAX_CYCLE_EVENT_PAGE,
                    )?,
                ))
            },
        )
        .unwrap();
    assert_eq!(clock, 1);
    assert!(pending.is_empty());
}

#[test]
fn recovery_retires_a_prewrite_event_when_its_source_was_never_committed() {
    let fixture = RuntimeFixture::new();
    let runtime = fixture.runtime();
    let source_record_seq = fixture.session.lock().unwrap().next_sequence();
    let event_id = CycleLifecycleEvent::event_id_for(
        &fixture.run.run_id,
        CycleEventKind::SceneEntered,
        0,
        source_record_seq,
    );
    runtime
        .prepare_cycle_event(CycleLifecycleEvent {
            version: 1,
            event_id: event_id.clone(),
            run_id: fixture.run.run_id.clone(),
            session_id: fixture.session_id.clone(),
            kind: CycleEventKind::SceneEntered,
            source_record_seq,
            history_revision: 0,
            scene_id: "entry".into(),
            entry_record_seq: source_record_seq,
            catalog_revision: fixture.catalog.revision().into(),
            world_revision: "world-v1".into(),
            round_id: None,
            evidence: vec![],
        })
        .unwrap();

    let reopened = fixture.runtime();
    reopened.drain_cycle_events().unwrap();
    fixture
        .storage
        .with_memory(
            &fixture.session_id,
            &fixture.catalog,
            |repository, connection, _| {
                assert!(
                    repository
                        .pending_cycle_events(
                            connection,
                            &fixture.run.run_id,
                            MAX_CYCLE_EVENT_PAGE
                        )?
                        .is_empty()
                );
                Ok(())
            },
        )
        .unwrap();
    fixture
        .storage
        .with_database(|connection| {
            let count: i64 = connection.query_row(
                "SELECT count(*) FROM memory_cycle_scene_progress WHERE run_id=?1 AND scene_id='entry' AND entry_history_revision=0 AND entry_record_seq=?2",
                rusqlite::params![&fixture.run.run_id, source_record_seq],
                |row| row.get(0),
            ).map_err(aoidos_store::db::sqlite_error)?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn recovery_discards_pending_events_from_a_rewound_history_branch() {
    let fixture = RuntimeFixture::new();
    let runtime = fixture.runtime();
    let mut session = fixture.session.lock().unwrap();
    let input_seq = session
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "根分支记录。".into(),
            mode: Some(InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let round_id = uuid::Uuid::new_v4().to_string();
    let identity = RoundIdentity {
        version: 1,
        round_id: round_id.clone(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let accepted_seq = session
        .append(
            Fact::RoundAccepted {
                identity: identity.clone(),
                input_seq,
                mode: InputMode::InCharacter,
                dice_mode: DiceMode::Manual,
                profile_id: "profile".into(),
                profile_revision: "profile-v1".into(),
                target: GenerationTarget::Narration,
                scene_id: "entry".into(),
                presence_revision: None,
                source_round_id: None,
                checkpoint_seq: None,
            }
            .body("接纳回合")
            .unwrap(),
        )
        .unwrap();
    let discarded_source_seq = session.next_sequence();
    let event_id = CycleLifecycleEvent::event_id_for(
        &fixture.run.run_id,
        CycleEventKind::RoundCompleted,
        session.history_revision(),
        discarded_source_seq,
    );
    runtime
        .prepare_cycle_event(CycleLifecycleEvent {
            version: 1,
            event_id,
            run_id: fixture.run.run_id.clone(),
            session_id: fixture.session_id.clone(),
            kind: CycleEventKind::RoundCompleted,
            source_record_seq: discarded_source_seq,
            history_revision: session.history_revision(),
            scene_id: "entry".into(),
            entry_record_seq: 0,
            catalog_revision: fixture.catalog.revision().into(),
            world_revision: "world-v1".into(),
            round_id: Some(round_id),
            evidence: vec![],
        })
        .unwrap();
    session
        .append(
            Fact::RoundEnded {
                identity,
                outcome: crate::ports::Outcome::Completed,
                through_seq: accepted_seq,
                completed_steps: vec![],
                error: None,
            }
            .body("回合结束")
            .unwrap(),
        )
        .unwrap();

    let path = session
        .path_at(session.history_revision())
        .unwrap()
        .into_iter()
        .filter(|seq| *seq <= 1)
        .collect::<Vec<_>>();
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    for seq in &path {
        digest.update(session.read(*seq).unwrap().raw.as_bytes());
    }
    let prefix_hash = format!("sha256:{:x}", digest.finalize());
    let fork = Fact::HistoryFork {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        mode: ForkMode::Rewind,
        parent_control_seq: session.history_revision(),
        target_seq: 1,
        prefix_hash,
        source_round_id: None,
        reused_dice_seq: None,
    }
    .body("回退到根分支记录")
    .unwrap();
    session
        .commit_history_fork(fork, &mut HistoryRebuildPort(fixture.storage.clone()))
        .unwrap();
    drop(session);

    fixture.runtime().drain_cycle_events().unwrap();
    fixture
        .storage
        .with_memory(
            &fixture.session_id,
            &fixture.catalog,
            |repository, connection, _| {
                assert!(
                    repository
                        .pending_cycle_events(
                            connection,
                            &fixture.run.run_id,
                            MAX_CYCLE_EVENT_PAGE
                        )?
                        .is_empty()
                );
                Ok(())
            },
        )
        .unwrap();
}
