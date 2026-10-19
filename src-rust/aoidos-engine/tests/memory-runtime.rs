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
    Candidate, Classification, Proposal, ProposalContext, SubjectRef, new_id,
};
use aoidos_memory::repository::{BatchCompletion, CandidateOutcome};
use std::{
    collections::BTreeMap,
    io,
    path::Path,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

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

    let scenario = assets::scenario("mistbell").unwrap();
    let entities = assets::entity_directory("mistbell").unwrap();
    let catalog_revision = "memory-stress-v1".to_owned();
    let script_revision = entities.script_revision().to_owned();
    let catalog = SceneCatalog::new(
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
    .unwrap();
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
        "rssPeakBytes": null,
        "crashInjection": false,
    })
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

fn disk_sizes(root: &Path) -> serde_json::Value {
    let mut files = serde_json::Map::new();
    visit_files(root, root, &mut files).expect("读取压力实验文件大小");
    assert!(
        files.contains_key("storage.sqlite"),
        "未采集到 SQLite 主数据库"
    );
    assert!(
        files.keys().any(|path| path.contains("/transcript/")),
        "未采集到记录正文文件"
    );
    serde_json::Value::Object(files)
}

fn visit_files(
    root: &Path,
    path: &Path,
    files: &mut serde_json::Map<String, serde_json::Value>,
) -> io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_files(root, &path, files)?;
        } else {
            let metadata = entry.metadata()?;
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative, serde_json::json!(metadata.len()));
        }
    }
    Ok(())
}
