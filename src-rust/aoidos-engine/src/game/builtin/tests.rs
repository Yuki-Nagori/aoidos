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
        let events = Arc::new(crate::record::test_support::Events::default());
        let storage = Arc::new(Storage::new(
            &root,
            Arc::new(MigrationEvents),
            events.clone(),
        ));
        let mut header = header("mistbell").unwrap();
        edit(&mut header);
        let session = Arc::new(Mutex::new(
            Session::create(root.join("record.jsonl"), header, events).unwrap(),
        ));
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
    assert!(domain.registered("sessionEnded", 1));
    assert!(!domain.registered("sessionEnded", 2));
    assert!(!domain.registered("npcRevived", 1));
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
        domain.apply(&mutation, &hash).unwrap_err().code,
        "engine.invalid-phase"
    );
    assert!(!WorldPort::applied(domain.as_ref(), &id, &hash).unwrap());
    mutation.data.version = 1;
    domain.apply(&mutation, &hash).unwrap();
    domain.apply(&mutation, &hash).unwrap();
    assert!(WorldPort::applied(domain.as_ref(), &id, &hash).unwrap());
    assert!(HistoryPort::applied(domain.as_ref(), &id, &hash).unwrap());
    assert!(domain.apply(&mutation, &format::hash(b"conflict")).is_err());
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
    assert!(domain.apply(&mutation, &hash).is_err());
    assert!(domain.rebuild(&record, &[], &rewind_id, &hash).is_err());
    fixture.storage.close().unwrap();
    assert_eq!(domain.confirmed(&record).unwrap_err().code, "app.not-ready");
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
    {
        let record = fixture.session.lock().unwrap();
        let domain = context.domain.lock().unwrap();
        let view = domain.confirmed(&record).unwrap();
        assert!(
            domain
                .consequences(&record, &round, &view)
                .unwrap()
                .is_empty()
        );
        context.completed.completed(CompletedRound {
            session_id: record.header().session_id.clone(),
            round_id: round.identity.round_id,
            history_revision: 0,
            through_seq: 0,
        });
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
    };
    assert!(domain.baseline_scene().is_none());
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
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.code, "app.bad-request");
    assert_eq!(fixture.session.lock().unwrap().last_event_seq(), 0);
    assert!(fixture.open().await.is_ok());
}
