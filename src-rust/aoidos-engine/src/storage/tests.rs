use super::*;
use crate::{migration::MigrationEvent, record::test_support};
use aoidos_memory::{
    error::{Error, Reason},
    model::{
        Algorithm, Attitude, CausalPort, ChangeKind, CycleKind, DetailPolicy, Dimension,
        EntryState, Expected, Plan, Policy, ProposalContext, Subject, Target, Version,
        new_id as new_memory_id, set_hash,
    },
    repository::{BatchSnapshot, Span},
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
        memory_batch_diagnostic("memory.candidate.review"),
        "memory.candidate.review"
    );
    assert_eq!(
        memory_batch_diagnostic("memory.candidate.shared_target"),
        "memory.candidate.shared_target"
    );
    assert_eq!(
        memory_batch_diagnostic("memory.candidate.duplicate_anchor"),
        "memory.candidate.duplicate_anchor"
    );
    assert_eq!(
        memory_batch_diagnostic("memory.operation.rejected"),
        "memory.operation.rejected"
    );
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
    let snapshot = storage
        .prepare_collectible_memory_batch(&run.run_id)
        .unwrap();
    assert!(!snapshot.spans.is_empty());
    assert!(snapshot.spans.len() <= 16);
    let batch_id = snapshot.batch_id.clone();
    storage.freeze_memory_batch(&catalog, snapshot).unwrap();
    assert_eq!(
        storage
            .memory_batch_status(&run.run_id, &batch_id)
            .unwrap()
            .state,
        aoidos_memory::repository::BatchState::Queued,
        "准备只冻结批次；发送仍须由显式命令完成"
    );
    assert_eq!(
        storage
            .prepare_collectible_memory_batch(&run.run_id)
            .unwrap_err()
            .code,
        "app.bad-request",
        "未终结批次必须先恢复，不能再冻结第二批"
    );
    let recovered = storage
        .active_memory_batch_status(&run.run_id)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.batch_id, batch_id);
    assert_eq!(
        recovered.state,
        aoidos_memory::repository::BatchState::Queued
    );
    storage.close().unwrap();
}

#[test]
fn proposal_targets_are_frozen_and_completion_survives_target_changes() {
    let root = std::env::temp_dir().join(format!("aoidos-memory-targets-{}", uuid::Uuid::new_v4()));
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
    let prior_seq = session
        .lock()
        .unwrap()
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "我喜欢海风。".into(),
            mode: Some(crate::record::format::InputMode::OutOfCharacter),
            content_range: None,
        })
        .unwrap();
    let (entry_id, version_id) = storage
        .with_memory_context(
            &session_id,
            &catalog,
            |repository, connection, port, _, _| {
                let source = port.reference(&run.run_id, prior_seq)?;
                let sources = vec![source];
                let version = Version {
                    version: 1,
                    entry_id: new_memory_id(),
                    version_id: new_memory_id(),
                    parent_version_id: None,
                    restored_from_version_id: None,
                    subject: Subject::PlayerPreference,
                    dimension: Dimension::PlayerPreference,
                    summary: "用户喜欢海风".into(),
                    attitude: Attitude::Believed,
                    inferred: false,
                    change: ChangeKind::New,
                    source_set_hash: set_hash(&sources)?,
                    evidence_set_hash: set_hash::<aoidos_memory::model::KnowledgeEvidence>(&[])?,
                    sources,
                    evidence: vec![],
                    policy_hash: run.policy_hash,
                };
                let boundary = port.boundary()?;
                let plan = Plan {
                    expected_logical_clock: repository.clock(connection, port, &run.run_id)?,
                    operation_id: new_memory_id(),
                    run_id: run.run_id.clone(),
                    history_revision: boundary.history_revision,
                    entity_revision: boundary.entity_revision,
                    evidence_revision: boundary.evidence_revision,
                    policy_hash: run.policy_hash,
                    batch_id: None,
                    candidate_ids: vec![],
                    targets: vec![Target {
                        entry_id: version.entry_id.clone(),
                        expected: Expected {
                            version_id: None,
                            state_revision: 0,
                        },
                        next: Some(version.clone()),
                        state: EntryState::Active,
                        effects: vec![],
                    }],
                    processed_materials: vec![],
                    reason: "test target snapshot".into(),
                };
                repository.prepare(connection, port, &plan)?;
                repository.write_versions(connection, &plan.operation_id)?;
                repository.apply(connection, port, &plan.operation_id)?;
                Ok((version.entry_id, version.version_id))
            },
        )
        .unwrap();
    let current_seq = session
        .lock()
        .unwrap()
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "我仍然喜欢海风。".into(),
            mode: Some(crate::record::format::InputMode::OutOfCharacter),
            content_range: None,
        })
        .unwrap();
    let snapshot = storage
        .with_memory_context(
            &session_id,
            &catalog,
            |repository, connection, port, _, _| {
                let source = port.reference(&run.run_id, current_seq)?;
                let material_id = repository.register_material(connection, port, &source, 0)?;
                let boundary = port.boundary()?;
                Ok(BatchSnapshot {
                    logical_clock: repository.clock(connection, port, &run.run_id)?,
                    version: 1,
                    batch_id: new_memory_id(),
                    run_id: run.run_id.clone(),
                    history_revision: boundary.history_revision,
                    entity_revision: boundary.entity_revision,
                    evidence_revision: boundary.evidence_revision,
                    policy_hash: run.policy_hash,
                    spans: vec![Span {
                        source_id: "s1".into(),
                        material_id,
                        source,
                        text: "我仍然喜欢海风。".into(),
                        context: false,
                        continuation: false,
                    }],
                    input: serde_json::json!({}),
                })
            },
        )
        .unwrap();

    storage
        .freeze_memory_batch(&catalog, snapshot.clone())
        .unwrap();
    let frozen = storage
        .with_memory(&session_id, &catalog, |repository, connection, _| {
            repository
                .batch(connection, &snapshot.batch_id)
                .map(|(batch, _)| batch)
        })
        .unwrap();
    let context: ProposalContext =
        aoidos_json::from_value(frozen.input["proposalContext"].clone()).unwrap();
    let target = &context.targets[&entry_id];
    assert_eq!(target.version_id, version_id);
    assert_eq!(target.summary, "用户喜欢海风");
    assert_eq!(target.state_revision, 1);

    let candidate = aoidos_memory::model::Candidate {
        candidate_id: "update-1".into(),
        classification: aoidos_memory::model::Classification::Supplement,
        source_ids: vec!["s1".into()],
        subject_ref: aoidos_memory::model::SubjectRef::PlayerPreference,
        entity_ids: vec![],
        evidence_ids: vec![],
        target_ref: Some(aoidos_memory::model::TargetRef {
            entry_id: entry_id.clone(),
            expected_version_id: version_id,
            expected_state_revision: 1,
        }),
        dimension: Some(Dimension::PlayerPreference),
        summary: Some("用户在新回合再次表达对海风的偏好".into()),
        attitude: None,
        attitude_evidence_ids: vec![],
        inferred: Some(false),
        reason: "与既有偏好一致".into(),
    };
    let proposal = aoidos_memory::model::Proposal {
        version: 1,
        candidates: vec![candidate],
        cycle_marks: vec![],
    };
    proposal.validate(&context).unwrap();
    let response = aoidos_json::canonical_string(&proposal).unwrap();

    let turn_id = new_memory_id();
    let request_id = new_memory_id();
    let batch_id = snapshot.batch_id.clone();
    storage
        .with_memory(&session_id, &catalog, |repository, connection, port| {
            repository.dispatch_batch(connection, port, &batch_id, &turn_id, &request_id)?;
            repository.receive_batch(
                connection,
                &batch_id,
                &turn_id,
                &request_id,
                response.as_bytes(),
            )?;

            let entry = repository.entry(connection, &run.run_id, &entry_id)?;
            let version = repository.current(connection, port, &run.run_id, &entry_id)?;
            let boundary = port.boundary()?;
            let plan = Plan {
                expected_logical_clock: repository.clock(connection, port, &run.run_id)?,
                operation_id: new_memory_id(),
                run_id: run.run_id.clone(),
                history_revision: boundary.history_revision,
                entity_revision: boundary.entity_revision,
                evidence_revision: boundary.evidence_revision,
                policy_hash: run.policy_hash,
                batch_id: None,
                candidate_ids: vec![],
                targets: vec![Target {
                    entry_id: entry_id.clone(),
                    expected: Expected {
                        version_id: Some(version.version_id),
                        state_revision: entry.state_revision,
                    },
                    next: None,
                    state: EntryState::Archived,
                    effects: vec![],
                }],
                processed_materials: vec![],
                reason: "test target changed after response".into(),
            };
            repository.prepare(connection, port, &plan)?;
            repository.write_versions(connection, &plan.operation_id)?;
            repository.apply(connection, port, &plan.operation_id)?;
            Ok(())
        })
        .unwrap();

    assert!(
        crate::record::memory::processing::complete_received_batch(
            &storage,
            &session_id,
            &catalog,
            &batch_id,
        )
        .unwrap()
    );
    assert!(
        !crate::record::memory::processing::complete_received_batch(
            &storage,
            &session_id,
            &catalog,
            &batch_id,
        )
        .unwrap()
    );
    storage
        .with_memory(&session_id, &catalog, |repository, connection, _| {
            let receipt = repository.batch_receipt(connection, &batch_id)?;
            let completion: aoidos_memory::repository::BatchCompletion = aoidos_json::decode_bytes(
                receipt
                    .completion
                    .as_deref()
                    .ok_or(aoidos_memory::error::Error::Corrupt)?,
                aoidos_memory::model::MAX_PLAN_BYTES,
            )
            .map_err(|_| aoidos_memory::error::Error::Corrupt)?;
            assert_eq!(completion.outcomes.len(), 1);
            assert_eq!(completion.outcomes[0].candidate_id, "update-1");
            assert_eq!(
                completion.outcomes[0].outcome,
                aoidos_memory::repository::CandidateOutcome::Rejected
            );
            Ok(())
        })
        .unwrap();

    storage.close().unwrap();
}

#[test]
fn received_memory_candidate_is_applied_once_from_the_frozen_proposal() {
    let root = std::env::temp_dir().join(format!(
        "aoidos-memory-candidate-apply-{}",
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
    let catalog = scene_catalog();
    let run = storage
        .initialize_memory_run(&session_id, &catalog)
        .unwrap();
    let session = storage.records.get(&session_id).unwrap();
    let record_seq = session
        .lock()
        .unwrap()
        .append(crate::record::format::Body::PlayerSpeech {
            player_id: "player".into(),
            text: "我喜欢钟声。".into(),
            mode: Some(crate::record::format::InputMode::OutOfCharacter),
            content_range: None,
        })
        .unwrap();
    let batch_id = new_memory_id();
    let snapshot = storage
        .with_memory_context(
            &session_id,
            &catalog,
            |repository, connection, port, _, _| {
                let source = port.reference(&run.run_id, record_seq)?;
                let material_id = repository.register_material(connection, port, &source, 0)?;
                let boundary = port.boundary()?;
                Ok(BatchSnapshot {
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
                        text: "我喜欢钟声。".into(),
                        context: false,
                        continuation: false,
                    }],
                    input: serde_json::json!({}),
                })
            },
        )
        .unwrap();
    storage.freeze_memory_batch(&catalog, snapshot).unwrap();
    let frozen = storage
        .with_memory(&session_id, &catalog, |repository, connection, _| {
            repository
                .batch(connection, &batch_id)
                .map(|(snapshot, _)| snapshot)
        })
        .unwrap();
    let context: ProposalContext =
        aoidos_json::from_value(frozen.input["proposalContext"].clone()).unwrap();
    let candidate = aoidos_memory::model::Candidate {
        candidate_id: "new-pref".into(),
        classification: aoidos_memory::model::Classification::New,
        source_ids: vec!["s1".into()],
        subject_ref: aoidos_memory::model::SubjectRef::PlayerPreference,
        entity_ids: vec![],
        evidence_ids: vec![],
        target_ref: None,
        dimension: Some(Dimension::PlayerPreference),
        summary: Some("玩家明确表示喜欢钟声".into()),
        attitude: None,
        attitude_evidence_ids: vec![],
        inferred: Some(false),
        reason: "玩家明确表达".into(),
    };
    let not_worthy = aoidos_memory::model::Candidate {
        candidate_id: "skip-1".into(),
        classification: aoidos_memory::model::Classification::NotWorthy,
        source_ids: vec!["s1".into()],
        dimension: None,
        summary: None,
        inferred: None,
        ..candidate.clone()
    };
    let proposal = aoidos_memory::model::Proposal {
        version: 1,
        candidates: vec![candidate, not_worthy],
        cycle_marks: vec![],
    };
    proposal.validate(&context).unwrap();
    let response = aoidos_json::canonical_string(&proposal).unwrap();
    let turn_id = new_memory_id();
    let request_id = new_memory_id();
    storage
        .with_memory(&session_id, &catalog, |repository, connection, port| {
            repository.dispatch_batch(connection, port, &batch_id, &turn_id, &request_id)?;
            repository.receive_batch(
                connection,
                &batch_id,
                &turn_id,
                &request_id,
                response.as_bytes(),
            )?;
            Ok(())
        })
        .unwrap();

    assert!(
        crate::record::memory::processing::complete_received_batch(
            &storage,
            &session_id,
            &catalog,
            &batch_id,
        )
        .unwrap()
    );
    assert!(
        !crate::record::memory::processing::complete_received_batch(
            &storage,
            &session_id,
            &catalog,
            &batch_id,
        )
        .unwrap()
    );
    assert!(
        storage
            .active_memory_batch_status(&run.run_id)
            .unwrap()
            .is_none()
    );

    let status = storage.memory_batch_status(&run.run_id, &batch_id).unwrap();
    let outcomes = status.completion.unwrap().outcomes;
    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0].candidate_id, "new-pref");
    assert_eq!(
        outcomes[0].outcome,
        aoidos_memory::repository::CandidateOutcome::Applied
    );
    assert_eq!(outcomes[1].candidate_id, "skip-1");
    assert_eq!(
        outcomes[1].outcome,
        aoidos_memory::repository::CandidateOutcome::NoOp
    );
    let entries = storage.memory_entries(&run.run_id, None, 10).unwrap();
    assert_eq!(entries.entries.len(), 1);

    storage.close().unwrap();
    drop(storage);
    std::fs::remove_dir_all(root).unwrap();
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

    let snapshot = storage
        .with_memory_context(
            &session_id,
            &catalog,
            |repository, connection, port, _, _| {
                let source = port.reference(&run.run_id, source_seq)?;
                let material_id = repository.register_material(connection, port, &source, 0)?;
                let boundary = port.boundary()?;
                Ok(BatchSnapshot {
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
                        "untrustedPromptContext": ["伪造记忆不得进入模型输入"],
                    }),
                })
            },
        )
        .unwrap();
    storage.freeze_memory_batch(&catalog, snapshot).unwrap();
    storage
        .with_memory(&session_id, &catalog, |repository, connection, _| {
            let (frozen, _) = repository.batch(connection, &batch_id)?;
            assert!(frozen.input.get("untrustedPromptContext").is_none());
            let context: ProposalContext =
                aoidos_json::from_value(frozen.input["proposalContext"].clone())
                    .map_err(|_| aoidos_memory::error::Error::Corrupt)?;
            assert!(context.cycle_rules.contains_key("remember-rescue"));
            assert_eq!(context.source_ids, ["s1".into()].into());
            Ok(())
        })
        .unwrap();

    let mut context = ProposalContext {
        entity_ids: ["untrusted-entity".into()].into(),
        ..ProposalContext::default()
    };
    crate::record::memory::processing::prepare_memory_proposal_context(
        &storage,
        &session_id,
        &catalog,
        &batch_id,
        &mut context,
    )
    .unwrap();
    assert!(!context.entity_ids.contains("untrusted-entity"));
    assert!(context.cycle_rules.contains_key("remember-rescue"));
    assert_eq!(context.source_ids, ["s1".into()].into());
    let changed_catalog = catalog
        .clone()
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
            crate::game::catalog::CycleMarkRule::new(
                "remember-second".into(),
                1,
                vec![
                    crate::game::catalog::CycleSubjectScope::new(
                        crate::game::catalog::EntityKind::Character,
                        "mistbell".into(),
                    )
                    .unwrap(),
                ],
                "rescued-player-second".into(),
                CycleKind::Familiarity,
                DetailPolicy::Vague,
                10,
            )
            .unwrap(),
        ])
        .unwrap();
    assert_eq!(
        crate::record::memory::processing::prepare_memory_proposal_context(
            &storage,
            &session_id,
            &changed_catalog,
            &batch_id,
            &mut ProposalContext::default(),
        )
        .unwrap_err()
        .code,
        "app.bad-request"
    );
    storage
        .with_memory(&session_id, &catalog, |repository, connection, port| {
            repository.dispatch_batch(connection, port, &batch_id, &turn_id, &request_id)?;
            repository.receive_batch(connection, &batch_id, &turn_id, &request_id, response)?;
            Ok(())
        })
        .unwrap();
    assert!(
        crate::record::memory::processing::complete_received_batch(
            &storage,
            &session_id,
            &catalog,
            &batch_id,
        )
        .unwrap()
    );
    assert!(
        !crate::record::memory::processing::complete_received_batch(
            &storage,
            &session_id,
            &catalog,
            &batch_id,
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
fn oversized_escaped_memory_prompt_is_rejected_before_materials_are_reserved() {
    let root = std::env::temp_dir().join(format!(
        "aoidos-memory-prompt-capacity-{}",
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
    let catalog = scene_catalog();
    let run = storage
        .initialize_memory_run(&session_id, &catalog)
        .unwrap();
    let session = storage.records.get(&session_id).unwrap();
    let sequences = (0..16)
        .map(|_| {
            session
                .lock()
                .unwrap()
                .append(crate::record::format::Body::PlayerSpeech {
                    player_id: "player".into(),
                    text: "[".repeat(2048),
                    mode: None,
                    content_range: None,
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    let snapshot = storage
        .with_memory_context(
            &session_id,
            &catalog,
            |repository, connection, port, _, _| {
                let boundary = port.boundary()?;
                let spans = sequences
                    .iter()
                    .enumerate()
                    .map(|(index, &record_seq)| {
                        let source = port.reference(&run.run_id, record_seq)?;
                        let material_id =
                            repository.register_material(connection, port, &source, 0)?;
                        Ok(Span {
                            source_id: format!("s{}", index + 1),
                            material_id,
                            text: "[".repeat(2048),
                            source,
                            context: false,
                            continuation: false,
                        })
                    })
                    .collect::<aoidos_memory::error::Result<Vec<_>>>()?;
                Ok(BatchSnapshot {
                    logical_clock: repository.clock(connection, port, &run.run_id)?,
                    version: 1,
                    batch_id: aoidos_memory::model::new_id(),
                    run_id: run.run_id.clone(),
                    history_revision: boundary.history_revision,
                    entity_revision: boundary.entity_revision,
                    evidence_revision: boundary.evidence_revision,
                    policy_hash: run.policy_hash,
                    spans,
                    input: serde_json::json!({}),
                })
            },
        )
        .unwrap();

    assert_eq!(
        storage
            .freeze_memory_batch(&catalog, snapshot)
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    assert_eq!(
        storage
            .collect_memory_materials(&run.run_id, None)
            .unwrap()
            .eligible_count,
        16
    );
    storage.close().unwrap();
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
