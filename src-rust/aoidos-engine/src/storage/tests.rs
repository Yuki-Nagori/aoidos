use super::*;
use crate::{migration::MigrationEvent, record::test_support};
use aoidos_memory::{
    error::{Error, Reason},
    model::{Algorithm, CausalPort, CycleKind, DetailPolicy, Policy, ProposalContext},
    repository::{BatchCompletion, BatchSnapshot, Span},
};
use std::sync::atomic::{AtomicU64, Ordering};

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
fn policy() -> Policy {
    Policy {
        version: 1,
        algorithm_version: 1,
        config_version: 1,
        app_config_version: 1,
        algorithm: Algorithm::None,
        base_strength: 100_000,
        reinforcement_gain: 10_000,
        reinforcement_cap: 100_000,
        strength_cap: 500_000,
        half_life_rounds: 10,
        repeat_window_rounds: 1,
        repeat_limit: 1,
        revival_cooldown_rounds: 1,
        active_capacity: 100,
        rate_table_version: 1,
        decay_rate: 1,
    }
}
fn scene_catalog() -> crate::game::catalog::SceneCatalog {
    let scenario = crate::game::assets::scenario("mistbell").unwrap();
    crate::game::builtin::memory_scene_catalog(
        scenario.title,
        crate::game::assets::entity_directory("mistbell").unwrap(),
    )
    .unwrap()
}

#[test]
fn memory_batch_diagnostics_expose_only_registered_codes() {
    assert_eq!(
        memory_batch_diagnostic("policy.capacity_blocked"),
        "policy.capacity_blocked"
    );
    assert_eq!(memory_batch_diagnostic("source.invalid"), "source.invalid");
    assert_eq!(
        memory_batch_diagnostic("provider.internal-secret"),
        "memory.unknown"
    );
}

#[test]
fn memory_borrows_registered_session_and_shared_migrated_connection() {
    let root = std::env::temp_dir().join(format!("aoidos-memory-storage-{}", uuid::Uuid::new_v4()));
    let storage = Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(test_support::Events::default()),
    );
    let header = crate::game::builtin::header("mistbell").unwrap();
    let session_id = header.session_id.clone();
    storage.records.create(header).unwrap();
    let catalog = scene_catalog();
    let run = storage
        .with_memory(&session_id, &catalog, |repository, conn, port| {
            assert_eq!(port.boundary()?.session_id, session_id);
            assert_eq!(port.boundary()?.entity_revision, 1);
            assert_eq!(port.boundary()?.evidence_revision, 0);
            repository.create_run(conn, port, policy())
        })
        .unwrap();
    assert_eq!(run.session_id, session_id);
    assert_eq!(
        storage
            .memory_entries(
                &run.run_id,
                Some(MemoryEntryCursor {
                    run_id: uuid::Uuid::new_v4().to_string(),
                    history_revision: 0,
                    snapshot_max_sequence: 0,
                    after_sequence: 0,
                }),
                8,
            )
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    storage
        .with_database(|conn| {
            let count: i64 = conn
                .query_row("SELECT count(*) FROM memory_runs", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        storage
            .with_memory(
                &uuid::Uuid::new_v4().to_string(),
                &catalog,
                |_, _, _| Ok(())
            )
            .unwrap_err()
            .code(),
        "app.not-found"
    );
    assert_eq!(
        storage
            .with_memory(&session_id, &catalog, |_, _, _| Err::<(), _>(
                Error::Corrupt
            ))
            .unwrap_err()
            .code(),
        "store.corrupt"
    );
    for reason in [
        Reason::StaleVersion,
        Reason::CapacityBlocked,
        Reason::PolicyUnavailable,
    ] {
        let error = storage
            .with_memory(&session_id, &catalog, |_, _, _| {
                Err::<(), _>(Error::Rejected(reason))
            })
            .unwrap_err();
        assert!(matches!(error, Error::Rejected(actual) if actual == reason));
    }
    storage.close().unwrap();
    assert!(matches!(
        storage.with_memory(&session_id, &catalog, |_, _, _| Ok(())),
        Err(Error::Rejected(Reason::RecoveryRequired))
    ));
    drop(storage);
    let reopened = Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(test_support::Events::default()),
    );
    reopened.records.open(&run.script_id, &session_id).unwrap();
    let recovered = reopened
        .with_memory(&session_id, &catalog, |repository, conn, port| {
            let mut latest = policy();
            latest.config_version = 2;
            latest.base_strength = 150_000;
            repository.create_run(conn, port, latest)
        })
        .unwrap();
    assert_eq!(recovered.run_id, run.run_id);
    assert_eq!(recovered.policy, run.policy);
    reopened
        .with_database(|connection| {
            connection
                .execute(
                    "INSERT INTO memory_config_versions(scope_kind,scope_id,config_version,algorithm_version,payload_hash,payload_json) VALUES('application','default',1,1,zeroblob(32),x'696e76616c6964')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO memory_config_heads(scope_kind,scope_id,config_version) VALUES('application','default',1)",
                    [],
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        reopened
            .initialize_memory_run(&session_id, &catalog)
            .unwrap()
            .run_id,
        run.run_id,
        "重开已有周目不读取当前配置，也不替换冻结策略"
    );
    reopened
        .with_database(|connection| {
            connection
                .execute(
                    "UPDATE memory_runs SET script_id='other-script' WHERE run_id=?1",
                    [&run.run_id],
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        reopened
            .memory_run_for_session(&session_id)
            .unwrap_err()
            .code,
        "store.corrupt",
        "损坏的周目剧本身份必须与已登记会话核对"
    );
    reopened.close().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn memory_collection_registers_stable_utf8_units_without_exposing_source_text() {
    let root = std::env::temp_dir().join(format!("aoidos-memory-collect-{}", uuid::Uuid::new_v4()));
    let storage = Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(test_support::Events::default()),
    );
    let header = crate::game::builtin::header("mistbell").unwrap();
    let session_id = header.session_id.clone();
    storage.records.create(header).unwrap();
    let catalog = scene_catalog();
    let run = storage
        .initialize_memory_run(&session_id, &catalog)
        .unwrap();
    let session = storage.records.get(&session_id).unwrap();
    session
        .lock()
        .unwrap()
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "🦊".repeat(600),
            mode: Some(crate::record::format::InputMode::OutOfCharacter),
            content_range: None,
        })
        .unwrap();

    let page = storage.collect_memory_materials(&run.run_id, None).unwrap();
    assert_eq!(page.eligible_count, 2);
    assert_eq!(page.run_id, run.run_id);
    assert_eq!(page.next_cursor, None);
    let repeated = storage
        .collect_memory_materials(&page.run_id, None)
        .unwrap();
    assert_eq!(repeated.eligible_count, 2);
    storage.close().unwrap();
}

#[test]
fn received_cycle_mark_is_materialized_from_current_authority_and_completed_atomically() {
    let root = std::env::temp_dir().join(format!(
        "aoidos-cycle-mark-completion-{}",
        uuid::Uuid::new_v4()
    ));
    let storage = Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(test_support::Events::default()),
    );
    let header = crate::game::builtin::header("mistbell").unwrap();
    let session_id = header.session_id.clone();
    storage.records.create(header).unwrap();
    let base_catalog = scene_catalog();
    let catalog = base_catalog
        .with_cycle_mark_rules(vec![
            crate::game::catalog::CycleMarkRule::new(
                "remember-rescue".into(),
                1,
                vec![
                    crate::game::catalog::CycleSubjectScope::new(
                        crate::game::catalog::EntityKind::Character,
                        "mistbell".into(),
                    )
                    .unwrap(),
                ],
                "rescued-player".into(),
                CycleKind::Familiarity,
                DetailPolicy::Vague,
                10,
            )
            .unwrap(),
        ])
        .unwrap();
    let run = storage
        .initialize_memory_run(&session_id, &catalog)
        .unwrap();
    let session = storage.records.get(&session_id).unwrap();
    let source_seq = session
        .lock()
        .unwrap()
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "我曾帮助洛苇逃出钟室。".into(),
            mode: Some(crate::record::format::InputMode::InCharacter),
            content_range: None,
        })
        .unwrap();
    let batch_id = aoidos_memory::model::new_id();
    let turn_id = aoidos_memory::model::new_id();
    let request_id = aoidos_memory::model::new_id();
    let response = r#"{"version":1,"candidates":[],"cycleMarks":[{"candidateId":"mark-1","ruleId":"remember-rescue","sourceIds":["s1"],"subjectIds":["luo-wei"],"summary":"洛苇记得玩家曾帮助她逃离钟室。"}]}"#.as_bytes();

    storage
        .with_memory(&session_id, &catalog, |repository, connection, port| {
            let source = port.reference(&run.run_id, source_seq)?;
            let material_id = repository.register_material(connection, port, &source, 0)?;
            let boundary = port.boundary()?;
            let snapshot = BatchSnapshot {
                logical_clock: repository.clock(connection, port, &run.run_id)?,
                version: 1,
                batch_id: batch_id.clone(),
                run_id: run.run_id.clone(),
                history_revision: boundary.history_revision,
                entity_revision: boundary.entity_revision,
                evidence_revision: boundary.evidence_revision,
                policy_hash: run.policy_hash,
                spans: vec![Span {
                    source_id: "s1".into(),
                    material_id,
                    source,
                    text: "我曾帮助洛苇逃出钟室。".into(),
                    context: false,
                    continuation: false,
                }],
                input: serde_json::json!({
                    "version": 1,
                    "proposalContext": ProposalContext {
                        source_ids: ["s1".into()].into(),
                        ..ProposalContext::default()
                    }
                }),
            };
            repository.freeze_batch(connection, port, &snapshot)?;
            Ok(())
        })
        .unwrap();

    let mut context = ProposalContext::default();
    crate::record::memory::processing::prepare_cycle_mark_context(
        &storage,
        &session_id,
        &catalog,
        &batch_id,
        &mut context,
    )
    .unwrap();
    storage
        .with_memory(&session_id, &catalog, |repository, connection, port| {
            repository.dispatch_batch(connection, port, &batch_id, &turn_id, &request_id)?;
            repository.receive_batch(connection, &batch_id, &turn_id, &request_id, response)?;
            Ok(())
        })
        .unwrap();
    let completion = BatchCompletion {
        version: 1,
        outcomes: vec![],
        cycle_marks: vec![],
    };
    assert!(
        crate::record::memory::processing::complete_received_batch(
            &storage,
            &session_id,
            &catalog,
            &batch_id,
            &context,
            &completion,
        )
        .unwrap()
    );
    assert!(
        !crate::record::memory::processing::complete_received_batch(
            &storage,
            &session_id,
            &catalog,
            &batch_id,
            &context,
            &completion,
        )
        .unwrap()
    );
    let (snapshot, state) = storage
        .with_memory(&session_id, &catalog, |repository, connection, _| {
            let result = repository.batch(connection, &batch_id)?;
            let receipt = repository.batch_receipt(connection, &batch_id)?;
            assert_eq!(
                receipt.state,
                aoidos_memory::repository::BatchState::Completed
            );
            let marks = repository.cycle_marks(connection, &run.script_id, false)?;
            assert_eq!(marks.len(), 1);
            assert_eq!(marks[0].mark.subject_ids, ["luo-wei"]);
            Ok(result)
        })
        .unwrap();
    assert_eq!(state, aoidos_memory::repository::BatchState::Completed);
    assert_eq!(snapshot.batch_id, batch_id);
    storage.close().unwrap();
    drop(storage);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn alias_schema_migration_only_advances_an_existing_database() {
    let root =
        std::env::temp_dir().join(format!("aoidos-alias-migration-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("storage.sqlite");
    let legacy = rusqlite::Connection::open(&path).unwrap();
    for migration in &STORAGE_MIGRATIONS[..STORAGE_MIGRATIONS.len() - 1] {
        legacy.execute_batch(migration).unwrap();
    }
    legacy
        .pragma_update(None, "user_version", STORAGE_MIGRATIONS.len() - 1)
        .unwrap();
    drop(legacy);

    let storage = Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(test_support::Events::default()),
    );
    storage
        .with_database(|connection| {
            let version: u32 = connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(version as usize, STORAGE_MIGRATIONS.len());
            let table_exists: i64 = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='memory_aliases')",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(table_exists, 1);
            Ok(())
        })
        .unwrap();
    storage.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn registration_faults_map_to_typed_memory_context_errors() {
    assert!(matches!(
        memory_context_error(Fault::new("app.not-found", "unknown")),
        Error::NotFound
    ));
    assert!(matches!(
        memory_context_error(Fault::bad_request()),
        Error::Rejected(Reason::InvalidSchema)
    ));
    assert!(matches!(
        memory_context_error(Fault::new("app.not-ready", "closed")),
        Error::Rejected(Reason::RecoveryRequired)
    ));
}

#[test]
fn memory_requires_an_open_database_even_for_registered_records() {
    let root = std::env::temp_dir().join(format!("aoidos-memory-closed-{}", uuid::Uuid::new_v4()));
    let storage = Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(test_support::Events::default()),
    );
    let header = crate::game::builtin::header("mistbell").unwrap();
    let session_id = header.session_id.clone();
    storage.records.create(header).unwrap();
    let catalog = scene_catalog();
    storage.database.lock().unwrap().take();
    assert!(matches!(
        storage.with_memory(&session_id, &catalog, |_, _, _| Ok(())),
        Err(Error::Rejected(Reason::RecoveryRequired))
    ));
    storage.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn memory_rejects_corrupt_registry_snapshots_before_invoking_the_job() {
    let root = std::env::temp_dir().join(format!("aoidos-memory-corrupt-{}", uuid::Uuid::new_v4()));
    let storage = Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(test_support::Events::default()),
    );
    let header = crate::game::builtin::header("mistbell").unwrap();
    let session_id = header.session_id.clone();
    storage.records.create(header).unwrap();
    let catalog = scene_catalog();
    storage
        .with_memory(&session_id, &catalog, |_, _, _| Ok(()))
        .unwrap();
    storage
        .with_database(|connection| {
            connection
                .execute("UPDATE entity_registry_runs SET script_id='other'", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(
        storage
            .with_memory::<()>(&session_id, &catalog, |_, _, _| panic!(
                "corrupt identity must not reach memory job"
            ))
            .is_err()
    );
    storage.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
