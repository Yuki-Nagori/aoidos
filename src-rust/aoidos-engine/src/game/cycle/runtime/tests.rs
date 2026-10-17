use super::*;
use crate::{
    game::{
        catalog::{Condition, HintRule, Node, Scene, SceneRuleView},
        domain::CompletedRounds,
        presence::{PresencePolicy, SceneEntryPresence},
    },
    migration::{MigrationEvent, MigrationEvents},
    record::{facts::SceneNode, format::Header},
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
    assert!(
        runtime
            .claim_scene_hint("entry", history_revision, &world_revision, &matches)
            .is_err()
    );
    storage
        .with_memory_repository(|repository, connection| {
            repository.delete_cycle_event(connection, &run.run_id, &orphan.event_id)
        })
        .unwrap();

    let hint = runtime
        .claim_scene_hint("entry", history_revision, &world_revision, &matches)
        .unwrap()
        .unwrap();
    assert_eq!(hint.hint_id, "entry-hint");
    assert_eq!(hint.text, "石门边缘留有一道浅痕。");
    assert_eq!(hint.target_scope, "scene");
    assert!(
        runtime
            .claim_scene_hint("entry", history_revision, &world_revision, &matches)
            .unwrap()
            .is_none()
    );

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
