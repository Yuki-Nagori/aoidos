//! 手动运行的磁盘记录轨迹探索；使用合成提案，不调用模型供应商。

use aoidos_engine::{
    fault::Fault,
    game::{
        assets,
        catalog::{Node, Scene, SceneCatalog, SceneRuleView},
        presence::{PresencePolicy, SceneEntryPresence},
    },
    migration::{MigrationEvent, MigrationEvents},
    ports::Terminal,
    record::{
        facts::{SceneNode, ScenePosition},
        format::{Body, InputMode},
        session::{Appended, RecordEvents},
    },
    storage::Storage,
};
use aoidos_memory::model::{
    Candidate, Classification, Dimension, EntryState, PolicyOverrides, Proposal, ProposalContext,
    SubjectRef, new_id,
};
use aoidos_memory::repository::{BatchCompletion, CandidateOutcome};
use std::{
    collections::BTreeMap,
    env, io,
    path::Path,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

#[path = "support/disk_metrics.rs"]
mod disk_metrics;

use disk_metrics::{disk_sizes, disk_summary};

#[derive(Default)]
struct Events(AtomicU64);

impl MigrationEvents for Events {
    fn prepare(&self, _: &MigrationEvent) -> Result<u64, Fault> {
        Ok(self.0.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn deliver(&self, _: u64, _: MigrationEvent) -> Result<(), Fault> {
        Ok(())
    }

    fn retire(&self, _: &str) {}
}

impl RecordEvents for Events {
    fn prepare(&self, _: &Appended) -> Result<u64, Fault> {
        Ok(self.0.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn deliver(&self, _: u64, _: Appended) -> Result<(), Fault> {
        Ok(())
    }
}

#[test]
#[ignore = "磁盘记录轨迹探索；显式运行 bun run memory:stress"]
fn memory_runtime_long_record_trace_reports_disk_metrics() {
    let mut reports = Vec::new();
    for record_pairs in [50, 100, 200] {
        reports.push(run_trace(record_pairs));
    }
    println!("{}", serde_json::to_string_pretty(&reports).unwrap());
}

#[test]
#[ignore = "显式运行的 1,000 条活动记忆检索容量实验"]
fn memory_runtime_target_retrieval_scans_1000_active_entries() {
    println!(
        "{}",
        serde_json::to_string_pretty(&run_target_retrieval_capacity(1_000)).unwrap()
    );
}

#[test]
#[ignore = "显式运行的进程终止 / 重开记忆批次恢复实验"]
fn memory_runtime_process_termination_recovers_received_batch() {
    const CHILD_EXIT: i32 = 73;
    const CHILD_ROOT: &str = "AOIDOS_MEMORY_RESTART_ROOT";
    const CHILD_STAGE: &str = "AOIDOS_MEMORY_RESTART_STAGE";

    if let (Ok(root), Ok(stage)) = (env::var(CHILD_ROOT), env::var(CHILD_STAGE)) {
        prepare_and_terminate_batch(Path::new(&root), &stage, CHILD_EXIT);
        unreachable!("子进程必须在持久化阶段后立即退出");
    }

    let mut child_peak_rss_bytes = None;
    for stage in ["queued", "dispatched", "received"] {
        let temp_dir = TempDirectory::new();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "memory_runtime_process_termination_recovers_received_batch",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD_ROOT, temp_dir.path())
            .env(CHILD_STAGE, stage)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("启动记忆恢复子进程");
        assert_eq!(child.code(), Some(CHILD_EXIT), "阶段 {stage} 未按预期终止");
        if let Some(bytes) =
            std::fs::read_to_string(temp_dir.path().join(format!("peak-rss-{stage}")))
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
        {
            child_peak_rss_bytes =
                Some(child_peak_rss_bytes.map_or(bytes, |peak: u64| peak.max(bytes)));
        }

        let migration_events = Arc::new(Events::default());
        let record_events = Arc::new(Events::default());
        let reopened = Storage::new(temp_dir.path(), migration_events, record_events);
        let session_id = std::fs::read_to_string(temp_dir.path().join("session_id"))
            .expect("子进程应保存 session ID");
        reopened.records.open("mistbell", &session_id).unwrap();
        let run = reopened.memory_run_for_session(&session_id).unwrap();
        let status = reopened
            .active_memory_batch_status(&run.run_id)
            .unwrap()
            .expect("未完成批次应在进程重开后保持可见");

        match stage {
            "queued" => assert_eq!(status.state, aoidos_memory::repository::BatchState::Queued),
            "dispatched" => {
                assert_eq!(
                    status.state,
                    aoidos_memory::repository::BatchState::Dispatched
                );
            }
            "received" => {
                assert_eq!(
                    status.state,
                    aoidos_memory::repository::BatchState::Received
                );
                let catalog = stress_catalog();
                assert!(
                    aoidos_engine::record::memory::processing::complete_received_batch(
                        &reopened,
                        &session_id,
                        &catalog,
                        &status.batch_id,
                    )
                    .unwrap()
                );
                let first_completion = reopened
                    .memory_batch_status(&run.run_id, &status.batch_id)
                    .unwrap()
                    .completion
                    .expect("首次恢复应持久完成回执");
                let first_entries = reopened.memory_entries(&run.run_id, None, 32).unwrap();
                assert_eq!(first_entries.entries.len(), 1);
                assert_eq!(first_entries.entries[0].state, EntryState::Active);
                assert!(first_entries.next_cursor.is_none());
                assert!(
                    !aoidos_engine::record::memory::processing::complete_received_batch(
                        &reopened,
                        &session_id,
                        &catalog,
                        &status.batch_id,
                    )
                    .unwrap()
                );
                let completed = reopened
                    .memory_batch_status(&run.run_id, &status.batch_id)
                    .unwrap();
                assert_eq!(
                    completed.state,
                    aoidos_memory::repository::BatchState::Completed
                );
                let completion: BatchCompletion = completed.completion.expect("恢复后应有完成回执");
                assert_eq!(completion, first_completion);
                assert_eq!(completion.outcomes.len(), 1);
                assert_eq!(completion.outcomes[0].outcome, CandidateOutcome::Applied);
                let entries = reopened.memory_entries(&run.run_id, None, 32).unwrap();
                assert_eq!(entries.entries.len(), first_entries.entries.len());
                assert_eq!(
                    entries.entries[0].entry_id,
                    first_entries.entries[0].entry_id
                );
                assert_eq!(
                    entries.entries[0].current_version_id,
                    first_entries.entries[0].current_version_id
                );
                assert_eq!(
                    entries.entries[0].state_revision,
                    first_entries.entries[0].state_revision
                );
                assert_eq!(entries.entries[0].state, first_entries.entries[0].state);
                assert!(entries.next_cursor.is_none());
            }
            _ => unreachable!(),
        }
        reopened.close().unwrap();
    }
    println!(
        "{{\"experiment\":\"process-termination-recovery\",\"stages\":[\"queued\",\"dispatched\",\"received\"],\"childPeakRssBytes\":{}}}",
        child_peak_rss_bytes
            .map(|bytes| bytes.to_string())
            .unwrap_or_else(|| "null".into()),
    );
}

fn prepare_and_terminate_batch(root: &Path, stage: &str, exit_code: i32) {
    let migration_events = Arc::new(Events::default());
    let record_events = Arc::new(Events::default());
    let storage = Storage::new(root, migration_events, record_events);
    storage.ready().unwrap();

    let header = aoidos_engine::game::builtin::header("mistbell").unwrap();
    let session_id = header.session_id.clone();
    std::fs::write(root.join("session_id"), &session_id).unwrap();
    storage.records.create(header).unwrap();
    let catalog = stress_catalog();
    let run = storage
        .initialize_memory_run(&session_id, &catalog)
        .unwrap();
    storage
        .records
        .get(&session_id)
        .unwrap()
        .lock()
        .unwrap()
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: "进程终止恢复实验素材".into(),
            mode: Some(InputMode::OutOfCharacter),
            content_range: None,
        })
        .unwrap();
    let materials = storage.collect_memory_materials(&run.run_id, None).unwrap();
    assert_eq!(materials.eligible_count, 1);
    let snapshot = storage
        .prepare_collectible_memory_batch(&run.run_id)
        .unwrap();
    let batch_id = snapshot.batch_id.clone();
    storage.freeze_memory_batch(&catalog, snapshot).unwrap();

    if stage != "queued" {
        let turn_id = new_id();
        let request_id = new_id();
        storage
            .with_memory(&session_id, &catalog, |repository, connection, port| {
                assert!(repository.dispatch_batch(
                    connection,
                    port,
                    &batch_id,
                    &turn_id,
                    &request_id,
                )?);
                Ok(())
            })
            .unwrap();

        if stage == "received" {
            let (batch, _) = storage
                .with_memory(&session_id, &catalog, |repository, connection, _| {
                    repository.batch(connection, &batch_id)
                })
                .unwrap();
            let proposal = Proposal {
                version: 1,
                candidates: batch
                    .spans
                    .iter()
                    .map(|span| Candidate {
                        candidate_id: "restart-noop".into(),
                        classification: Classification::New,
                        source_ids: vec![span.source_id.clone()],
                        subject_ref: SubjectRef::PlayerPreference,
                        entity_ids: vec![],
                        evidence_ids: vec![],
                        target_ref: None,
                        dimension: Some(Dimension::PlayerPreference),
                        summary: Some(span.text.clone()),
                        attitude: None,
                        attitude_evidence_ids: vec![],
                        inferred: Some(false),
                        reason: "进程恢复实验的合成偏好提案".into(),
                    })
                    .collect(),
                cycle_marks: vec![],
            };
            let response = aoidos_json::canonical_string(&proposal).unwrap();
            storage
                .with_memory(&session_id, &catalog, |repository, connection, _| {
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
        }
    }

    if let Some(bytes) = disk_metrics::peak_rss_bytes() {
        std::fs::write(root.join(format!("peak-rss-{stage}")), bytes.to_string())
            .expect("保存恢复子进程 RSS 样本");
    }
    std::process::exit(exit_code);
}

fn run_trace(record_pairs: usize) -> serde_json::Value {
    let temp_dir = TempDirectory::new();
    let root = temp_dir.path();
    let migration_events = Arc::new(Events::default());
    let record_events = Arc::new(Events::default());
    let storage = Storage::new(root, migration_events.clone(), record_events.clone());
    storage.ready().unwrap();

    let header = aoidos_engine::game::builtin::header("mistbell").unwrap();
    let session_id = header.session_id.clone();
    storage.records.create(header).unwrap();
    let session = storage.records.get(&session_id).unwrap();
    let start = Instant::now();
    {
        let mut session = session.lock().unwrap();
        for pair in 0..record_pairs {
            let turn_id = new_id();
            session
                .append(Body::PlayerSpeech {
                    player_id: "player".into(),
                    text: format!("第 {pair} 组记录的合成输入。"),
                    // 合成素材使用场外输入，才会按协议授权玩家偏好主体。
                    mode: Some(InputMode::OutOfCharacter),
                    content_range: None,
                })
                .unwrap();
            session
                .append(Body::Narration {
                    text: format!("第 {pair} 组记录的合成叙事。"),
                    turn_id,
                    terminal: Terminal::Completed {
                        finish_reason: aoidos_llm::schedule::FinishReason::Stop,
                    },
                })
                .unwrap();
        }
    }
    let record_write_ms = start.elapsed().as_millis();

    let catalog = stress_catalog();
    let run = storage
        .initialize_memory_run(&session_id, &catalog)
        .unwrap();

    let start = Instant::now();
    let mut cursor = None;
    let mut eligible_materials = 0;
    loop {
        let page = storage
            .collect_memory_materials(&run.run_id, cursor.clone())
            .unwrap();
        eligible_materials += page.eligible_count;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(eligible_materials, record_pairs * 2);
    let material_scan_ms = start.elapsed().as_millis();

    let start = Instant::now();
    let expected_batches = eligible_materials.div_ceil(16);
    let mut batches = Vec::new();
    loop {
        if batches.len() == expected_batches {
            assert!(matches!(
                storage.prepare_collectible_memory_batch(&run.run_id),
                Err(error) if error.code == "app.not-found"
            ));
            break;
        }
        let snapshot = match storage.prepare_collectible_memory_batch(&run.run_id) {
            Ok(snapshot) => snapshot,
            Err(error) if error.code == "app.not-found" => panic!("素材处理提前结束"),
            Err(error) => panic!("构造合成批次失败：{}", error.code),
        };
        let batch_id = snapshot.batch_id.clone();
        let expected_candidates = snapshot.spans.len();
        storage.freeze_memory_batch(&catalog, snapshot).unwrap();
        submit_synthetic_not_worthy(&storage, &session_id, &catalog, &batch_id);
        batches.push((batch_id, expected_candidates));
    }
    assert_eq!(batches.len(), expected_batches);
    let batch_processing_ms = start.elapsed().as_millis();

    let files_while_open = disk_sizes(root);
    let start = Instant::now();
    storage.close().unwrap();
    drop(storage);
    let reopened = Storage::new(root, migration_events, record_events);
    reopened.records.open("mistbell", &session_id).unwrap();
    let recovered_run = reopened.memory_run_for_session(&session_id).unwrap();
    assert_eq!(recovered_run.run_id, run.run_id);
    assert!(
        reopened
            .active_memory_batch_status(&run.run_id)
            .unwrap()
            .is_none()
    );
    for (batch_id, expected_candidates) in &batches {
        let status = reopened.memory_batch_status(&run.run_id, batch_id).unwrap();
        let completion: BatchCompletion = status.completion.expect("批次应有持久完成回执");
        assert!(
            completion.outcomes.len() == *expected_candidates
                && completion
                    .outcomes
                    .iter()
                    .all(|result| result.outcome == CandidateOutcome::NoOp)
        );
    }
    let reopen_validation_ms = start.elapsed().as_millis();
    let files_after_reopen = disk_sizes(root);
    reopened.close().unwrap();
    drop(reopened);

    serde_json::json!({
        "syntheticRecordPairs": record_pairs,
        "completedGameRounds": 0,
        "restartMode": "clean-in-process-reopen",
        "recordBlocks": record_pairs * 2,
        "eligibleMaterials": eligible_materials,
        "completedSyntheticBatches": batches.len(),
        "providerRequests": 0,
        "recordWriteMs": record_write_ms,
        "materialScanMs": material_scan_ms,
        "batchProcessingMs": batch_processing_ms,
        "reopenValidationMs": reopen_validation_ms,
        "filesWhileOpen": files_while_open,
        "filesAfterReopen": files_after_reopen,
        "processPeakRssBytes": disk_metrics::peak_rss_bytes(),
        "crashInjection": false,
    })
}

fn run_target_retrieval_capacity(entry_count: usize) -> serde_json::Value {
    let temp_dir = TempDirectory::new();
    let root = temp_dir.path();
    let migration_events = Arc::new(Events::default());
    let record_events = Arc::new(Events::default());
    let storage = Storage::new(root, migration_events.clone(), record_events.clone());
    storage.ready().unwrap();

    let header = aoidos_engine::game::builtin::header("mistbell").unwrap();
    let session_id = header.session_id.clone();
    storage.records.create(header).unwrap();
    let catalog = stress_catalog();
    let config = storage.memory_script_config("mistbell").unwrap();
    storage
        .save_memory_script_config(
            "mistbell",
            config.config_version,
            config.app_config_version,
            PolicyOverrides {
                active_capacity: Some(entry_count as u64),
                ..PolicyOverrides::default()
            },
        )
        .unwrap();
    let session = storage.records.get(&session_id).unwrap();
    {
        let mut session = session.lock().unwrap();
        for index in 0..entry_count {
            session
                .append(Body::PlayerSpeech {
                    player_id: "player".into(),
                    text: format!("memorytoken{index:04}"),
                    mode: Some(InputMode::OutOfCharacter),
                    content_range: None,
                })
                .unwrap();
        }
    }
    let run = storage
        .initialize_memory_run(&session_id, &catalog)
        .unwrap();
    assert_eq!(run.policy.active_capacity, entry_count as u64);

    let mut cursor = None;
    let mut eligible_materials = 0;
    loop {
        let page = storage
            .collect_memory_materials(&run.run_id, cursor.clone())
            .unwrap();
        eligible_materials += page.eligible_count;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(eligible_materials, entry_count);

    let versions_created = Instant::now();
    let expected_batches = entry_count.div_ceil(16);
    let mut batches = Vec::with_capacity(expected_batches);
    for _ in 0..expected_batches {
        let snapshot = storage
            .prepare_collectible_memory_batch(&run.run_id)
            .unwrap_or_else(|error| panic!("准备合成记忆批次失败：{}", error.code));
        let batch_id = snapshot.batch_id.clone();
        storage.freeze_memory_batch(&catalog, snapshot).unwrap();
        submit_synthetic_new(&storage, &session_id, &catalog, &batch_id);
        batches.push(batch_id);
    }
    assert_eq!(batches.len(), expected_batches);
    assert_active_memory_entries(&storage, &run.run_id, entry_count);
    let version_creation_ms = versions_created.elapsed().as_millis();

    let lookup_seq = session
        .lock()
        .unwrap()
        .append(Body::PlayerSpeech {
            player_id: "player".into(),
            text: format!("memorytoken{:04} 检索确认", entry_count - 1),
            mode: Some(InputMode::OutOfCharacter),
            content_range: None,
        })
        .unwrap();
    let mut cursor = None;
    let mut lookup_materials = 0;
    loop {
        let page = storage
            .collect_memory_materials(&run.run_id, cursor.clone())
            .unwrap();
        lookup_materials += page.eligible_count;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(lookup_materials, 1);
    let lookup_batch = storage
        .prepare_collectible_memory_batch(&run.run_id)
        .unwrap();
    assert_eq!(lookup_batch.spans.len(), 1);
    let lookup_batch_id = lookup_batch.batch_id.clone();
    let freeze_started = Instant::now();
    storage.freeze_memory_batch(&catalog, lookup_batch).unwrap();
    let context = storage
        .with_memory(&session_id, &catalog, |repository, connection, _| {
            let (snapshot, _) = repository.batch(connection, &lookup_batch_id)?;
            let context: ProposalContext =
                aoidos_json::from_value(snapshot.input["proposalContext"].clone())
                    .map_err(|_| aoidos_memory::error::Error::Corrupt)?;
            Ok(context)
        })
        .unwrap();
    assert_eq!(lookup_seq, entry_count as u64 + 1);
    assert_eq!(context.targets.len(), 1);
    assert_eq!(
        context.targets.values().next().unwrap().summary,
        format!("memorytoken{:04}", entry_count - 1)
    );
    let target_search = context.target_search.clone().unwrap();
    assert_eq!(target_search.scanned_entries as usize, entry_count);
    assert_eq!(target_search.matched_entries as usize, 1);
    let freeze_ms = freeze_started.elapsed().as_millis();
    let files_while_open = disk_summary(root);

    let reopen_started = Instant::now();
    storage.close().unwrap();
    drop(storage);
    let reopened = Storage::new(root, migration_events, record_events);
    reopened.records.open("mistbell", &session_id).unwrap();
    let recovered_context = reopened
        .with_memory(&session_id, &catalog, |repository, connection, _| {
            let (snapshot, _) = repository.batch(connection, &lookup_batch_id)?;
            let context: ProposalContext =
                aoidos_json::from_value(snapshot.input["proposalContext"].clone())
                    .map_err(|_| aoidos_memory::error::Error::Corrupt)?;
            Ok(context)
        })
        .unwrap();
    assert_active_memory_entries(&reopened, &run.run_id, entry_count);
    assert_eq!(recovered_context.targets.len(), 1);
    assert_eq!(recovered_context.targets, context.targets);
    assert_eq!(recovered_context.target_search, context.target_search);
    let reopen_validation_ms = reopen_started.elapsed().as_millis();
    let files_after_reopen = disk_summary(root);
    reopened.close().unwrap();
    drop(reopened);

    serde_json::json!({
        "experiment": "bounded-target-retrieval",
        "activeMemoryEntries": entry_count,
        "matchingTargets": 1,
        "scannedEntries": entry_count,
        "versionCreationMs": version_creation_ms,
        "targetFreezeMs": freeze_ms,
        "reopenValidationMs": reopen_validation_ms,
        "filesWhileOpen": files_while_open,
        "filesAfterReopen": files_after_reopen,
        "providerRequests": 0,
        "completedGameRounds": 0,
        "processPeakRssBytes": disk_metrics::peak_rss_bytes(),
    })
}

fn stress_catalog() -> SceneCatalog {
    let scenario = assets::scenario("mistbell").unwrap();
    let entities = assets::entity_directory("mistbell").unwrap();
    let catalog_revision = "memory-stress-v1".to_owned();
    let script_revision = entities.script_revision().to_owned();
    SceneCatalog::new(
        catalog_revision.clone(),
        script_revision,
        BTreeMap::from([(
            "entry".into(),
            Node {
                kind: "scene".into(),
                title: scenario.title.clone(),
                parent: None,
            },
        )]),
        BTreeMap::from([(
            "entry".into(),
            Scene {
                position: ScenePosition {
                    scene_id: "entry".into(),
                    path: vec![SceneNode {
                        kind: "scene".into(),
                        id: "entry".into(),
                        title: scenario.title,
                    }],
                },
                rules: SceneRuleView {
                    scene_id: "entry".into(),
                    catalog_revision,
                    advance_rule_ids: vec![],
                    progress_rule_ids: vec![],
                    hint_rules: vec![],
                    exit_rule_ids: vec![],
                },
                advance: vec![],
                check_rule_ids: vec![],
                actor_ids: vec!["player".into()],
                forced_check: None,
                dice_disabled: false,
            },
        )]),
        BTreeMap::new(),
    )
    .unwrap()
    .with_entity_directory(entities)
    .unwrap()
    .with_presence_policy(
        PresencePolicy::new(
            "entry".into(),
            SceneEntryPresence {
                rule_id: "memory-stress-entry-v1".into(),
                rule_version: 1,
                character_ids: vec!["player".into()],
            },
            BTreeMap::from([(
                "entry".into(),
                SceneEntryPresence {
                    rule_id: "memory-stress-entry-v1".into(),
                    rule_version: 1,
                    character_ids: vec!["player".into()],
                },
            )]),
            vec![],
        )
        .unwrap(),
    )
    .unwrap()
}

fn submit_synthetic_not_worthy(
    storage: &Storage,
    session_id: &str,
    catalog: &aoidos_engine::game::catalog::SceneCatalog,
    batch_id: &str,
) {
    let (snapshot, context) = storage
        .with_memory(session_id, catalog, |repository, connection, _| {
            let (snapshot, _) = repository.batch(connection, batch_id)?;
            let context: ProposalContext =
                aoidos_json::from_value(snapshot.input["proposalContext"].clone())
                    .map_err(|_| aoidos_memory::error::Error::Corrupt)?;
            Ok((snapshot, context))
        })
        .unwrap();
    let proposal = Proposal {
        version: 1,
        candidates: snapshot
            .spans
            .iter()
            .enumerate()
            .map(|(index, span)| Candidate {
                candidate_id: format!("stress-{index}"),
                classification: Classification::NotWorthy,
                source_ids: vec![span.source_id.clone()],
                subject_ref: SubjectRef::PlayerPreference,
                entity_ids: vec![],
                evidence_ids: vec![],
                target_ref: None,
                dimension: None,
                summary: None,
                attitude: None,
                attitude_evidence_ids: vec![],
                inferred: None,
                reason: "长对局资源实验的合成响应，不代表语义标注".into(),
            })
            .collect(),
        cycle_marks: vec![],
    };
    proposal
        .validate(&context)
        .expect("合成提案应符合冻结白名单");
    let response = aoidos_json::canonical_string(&proposal).unwrap();
    let turn_id = new_id();
    let request_id = new_id();
    storage
        .with_memory(session_id, catalog, |repository, connection, port| {
            repository.dispatch_batch(connection, port, batch_id, &turn_id, &request_id)?;
            repository.receive_batch(
                connection,
                batch_id,
                &turn_id,
                &request_id,
                response.as_bytes(),
            )
        })
        .unwrap();
    assert!(
        aoidos_engine::record::memory::processing::complete_received_batch(
            storage, session_id, catalog, batch_id,
        )
        .unwrap()
    );
}

fn submit_synthetic_new(
    storage: &Storage,
    session_id: &str,
    catalog: &aoidos_engine::game::catalog::SceneCatalog,
    batch_id: &str,
) {
    let (snapshot, context) = storage
        .with_memory(session_id, catalog, |repository, connection, _| {
            let (snapshot, _) = repository.batch(connection, batch_id)?;
            let context: ProposalContext =
                aoidos_json::from_value(snapshot.input["proposalContext"].clone())
                    .map_err(|_| aoidos_memory::error::Error::Corrupt)?;
            Ok((snapshot, context))
        })
        .unwrap();
    let proposal = Proposal {
        version: 1,
        candidates: snapshot
            .spans
            .iter()
            .enumerate()
            .map(|(index, span)| Candidate {
                candidate_id: format!("capacity-{index}"),
                classification: Classification::New,
                source_ids: vec![span.source_id.clone()],
                subject_ref: SubjectRef::PlayerPreference,
                entity_ids: vec![],
                evidence_ids: vec![],
                target_ref: None,
                dimension: Some(Dimension::PlayerPreference),
                summary: Some(span.text.clone()),
                attitude: None,
                attitude_evidence_ids: vec![],
                inferred: Some(false),
                reason: "容量实验使用的合成提案".into(),
            })
            .collect(),
        cycle_marks: vec![],
    };
    proposal
        .validate(&context)
        .expect("合成提案应符合冻结白名单");
    let response = aoidos_json::canonical_string(&proposal).unwrap();
    let expected_candidates = proposal.candidates.len();
    let turn_id = new_id();
    let request_id = new_id();
    storage
        .with_memory(session_id, catalog, |repository, connection, port| {
            repository.dispatch_batch(connection, port, batch_id, &turn_id, &request_id)?;
            repository.receive_batch(
                connection,
                batch_id,
                &turn_id,
                &request_id,
                response.as_bytes(),
            )
        })
        .unwrap();
    assert!(
        aoidos_engine::record::memory::processing::complete_received_batch(
            storage, session_id, catalog, batch_id,
        )
        .unwrap()
    );
    let completion = storage
        .memory_batch_status(&snapshot.run_id, batch_id)
        .unwrap()
        .completion
        .expect("容量实验批次应有持久完成回执");
    assert_eq!(completion.outcomes.len(), expected_candidates);
    assert!(
        completion
            .outcomes
            .iter()
            .all(|result| result.outcome == CandidateOutcome::Applied)
    );
}

fn assert_active_memory_entries(storage: &Storage, run_id: &str, expected: usize) {
    let mut cursor = None;
    let mut entries = 0;
    loop {
        let page = storage.memory_entries(run_id, cursor.clone(), 32).unwrap();
        assert!(
            page.entries
                .iter()
                .all(|entry| entry.state == EntryState::Active),
            "容量实验不应意外归档或弱化合成条目"
        );
        entries += page.entries.len();
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(entries, expected);
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("aoidos-memory-stress-{}", new_id())))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "清理记忆压力实验临时目录失败：{}：{error}",
                self.0.display()
            );
        }
    }
}
