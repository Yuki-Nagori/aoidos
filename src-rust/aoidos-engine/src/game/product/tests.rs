use super::*;

#[test]
fn selection_preserves_script_identity_and_rejects_unsafe_metadata() {
    let root = std::env::temp_dir().join(format!("aoidos-selection-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(read_selection(&root).unwrap().is_none());
    let selected = Selection {
        version: 1,
        session_id: uuid::Uuid::new_v4().to_string(),
        profile_id: "default".into(),
        script_id: "mistbell".into(),
    };
    write_selection(&root, &selected).unwrap();
    let restored = read_selection(&root).unwrap().unwrap();
    assert_eq!(restored.session_id, selected.session_id);
    assert_eq!(restored.script_id, selected.script_id);
    assert_eq!(opened(&restored).unwrap().title, "雾钟地窖");
    let invalid = Selection {
        script_id: "../mistbell".into(),
        ..selected
    };
    write_selection(&root, &invalid).unwrap();
    assert_eq!(read_selection(&root).err().unwrap().code, "store.corrupt");
    std::fs::remove_dir_all(&root).unwrap();
}

use crate::{
    game::test_support::{PhaseEventsFixture, PrepareGate},
    migration::{MigrationEvent, MigrationEvents},
    ports::{EventPort, PreparedEvent, TurnEvent},
    record::{facts::DiceMode, test_support::Events as RecordEvents},
};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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
impl EventPort for Events {
    fn prepare(&self, event: TurnEvent) -> Result<PreparedEvent, Fault> {
        Ok(PreparedEvent {
            seq: self.0.fetch_add(1, Ordering::SeqCst) + 1,
            event,
            envelope: serde_json::Value::Null,
        })
    }
    fn deliver(&self, _: PreparedEvent) -> Result<(), Fault> {
        Ok(())
    }
    fn retire(&self, _: &str) {}
}
struct Source {
    calls: AtomicUsize,
    error: Mutex<Option<Fault>>,
    identity: Mutex<Option<String>>,
    gate: Mutex<Option<Arc<PrepareGate>>>,
}
impl ProfileSource for Source {
    fn freeze(&self, id: &str) -> Result<ResolvedProfile, Fault> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = self.gate.lock().unwrap().take() {
            gate.block(std::time::Duration::from_secs(5))?;
        }
        if let Some(error) = self.error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(ResolvedProfile {
            frozen: FrozenProfile {
                profile: aoidos_llm::config::LlmProfile {
                    profile_id: self
                        .identity
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or_else(|| id.into()),
                    provider_id: "deepseek".into(),
                    model: "deepseek-v4-pro".into(),
                    mode: aoidos_llm::config::ProfileMode::Completion,
                    thinking: false,
                    sampling: aoidos_llm::sampling::Sampling {
                        temperature: 1.0,
                        max_tokens: 64,
                    },
                    proxy: aoidos_llm::config::ProxyConfig::None,
                },
                credential: Some("fixture-never-sent".to_owned().into()),
            },
            proxy_auth: None,
        })
    }
}
struct Fixture {
    root: PathBuf,
    storage: Arc<Storage>,
    coordinator: Coordinator,
    source: Arc<Source>,
    factory: Arc<ProfileFactory>,
    product: Arc<Product>,
    service: Arc<Service>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("aoidos-product-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Storage::new(
            &root,
            Arc::new(Events::default()),
            Arc::new(RecordEvents::default()),
        ));
        storage.ready().unwrap();
        let coordinator = Coordinator::new(Arc::new(Events::default()), 8).unwrap();
        let source = Arc::new(Source {
            calls: AtomicUsize::new(0),
            error: Mutex::new(None),
            identity: Mutex::new(None),
            gate: Mutex::new(None),
        });
        let factory = ProfileFactory::new(
            source.clone(),
            storage.clone(),
            SystemProxySnapshot::default(),
            Arc::new(Mutex::new(Calibration::default())),
        );
        let product = Arc::new(Product::new(
            &root,
            storage.clone(),
            coordinator.clone(),
            Arc::new(PhaseEventsFixture::default()),
            factory.clone(),
        ));
        let service = Arc::new(Service::new(coordinator.clone(), factory.clone()));
        Self {
            root,
            storage,
            coordinator,
            source,
            factory,
            product,
            service,
        }
    }
    async fn open(&self, new: bool) -> Result<OpenedSession, Fault> {
        self.product
            .open(&self.service, "default".into(), "mistbell".into(), new)
            .await
    }
    async fn close(&self) {
        self.service.shutdown().await;
        self.coordinator.shutdown().await;
        self.storage.close().unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn opening_reloading_and_switching_profiles_do_not_start_requests_or_replace_history() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.factory.freeze().err().unwrap().code,
        "app.not-ready"
    );
    let first = fixture.open(false).await.unwrap();
    assert_eq!(
        fixture
            .service
            .get_phase(&first.session_id)
            .unwrap()
            .state
            .scene
            .unwrap()
            .scene_id,
        "entry"
    );
    let frozen = fixture.factory.freeze().unwrap();
    assert_eq!(frozen.profile_id, "default");
    assert_eq!(frozen.dice_mode, DiceMode::Manual);
    let calls = fixture.source.calls.load(Ordering::SeqCst);
    let lease = fixture.coordinator.acquire().unwrap();
    *fixture.source.error.lock().unwrap() = Some(Fault::new("llm.missing-key", "fixture"));
    assert_eq!(
        fixture.open(false).await.unwrap().session_id,
        first.session_id
    );
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), calls);
    assert_eq!(fixture.open(true).await.unwrap_err().code, "app.busy");
    drop(lease);
    assert_eq!(
        fixture.open(true).await.unwrap_err().code,
        "llm.missing-key"
    );
    assert!(fixture.service.get_phase(&first.session_id).is_ok());
    *fixture.source.error.lock().unwrap() = None;
    let switched = fixture
        .product
        .open(&fixture.service, "second".into(), "mistbell".into(), false)
        .await
        .unwrap();
    assert_eq!(switched.session_id, first.session_id);
    assert_eq!(fixture.factory.freeze().unwrap().profile_id, "second");
    let next = fixture.open(true).await.unwrap();
    assert_ne!(first.session_id, next.session_id);
    assert_eq!(
        fixture
            .service
            .get_phase(&first.session_id)
            .unwrap_err()
            .code,
        "app.not-found"
    );
    assert_eq!(
        fixture
            .storage
            .records
            .get(&first.session_id)
            .err()
            .unwrap()
            .code,
        "app.not-found"
    );
    let old_path = fixture
        .root
        .join("workspaces/mistbell/transcript")
        .join(format!("{}.jsonl", first.session_id));
    assert!(old_path.exists());
    fixture.close().await;
}

#[tokio::test]
async fn restarting_reopens_saved_identity_without_implicitly_advancing() {
    let fixture = Fixture::new();
    let first = fixture.open(false).await.unwrap();
    fixture
        .service
        .close_session(&first.session_id)
        .await
        .unwrap();
    fixture.storage.records.close(&first.session_id).unwrap();
    let bytes = std::fs::read(
        fixture
            .root
            .join("workspaces/mistbell/transcript")
            .join(format!("{}.jsonl", first.session_id)),
    )
    .unwrap();
    let restored = fixture.open(false).await.unwrap();
    assert_eq!(restored.session_id, first.session_id);
    assert_eq!(
        std::fs::read(
            fixture
                .root
                .join("workspaces/mistbell/transcript")
                .join(format!("{}.jsonl", first.session_id))
        )
        .unwrap(),
        bytes
    );
    fixture.close().await;
}

#[tokio::test]
async fn invalid_input_and_configuration_leave_selection_and_records_untouched() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .product
            .open(&fixture.service, "".into(), "mistbell".into(), false)
            .await
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    assert_eq!(
        fixture
            .product
            .open(&fixture.service, "default".into(), "missing".into(), false)
            .await
            .unwrap_err()
            .code,
        "app.not-found"
    );
    *fixture.source.identity.lock().unwrap() = Some("different".into());
    assert_eq!(
        fixture.open(false).await.unwrap_err().code,
        "app.bad-request"
    );
    assert!(read_selection(&fixture.root).unwrap().is_none());
    *fixture.source.identity.lock().unwrap() = None;
    std::fs::write(fixture.root.join("ui-preferences.json"), "broken").unwrap();
    assert!(
        fixture
            .open(false)
            .await
            .unwrap_err()
            .code
            .starts_with("store.")
    );
    std::fs::remove_file(fixture.root.join("ui-preferences.json")).unwrap();
    fixture.storage.close().unwrap();
    assert_eq!(
        fixture.factory.for_profile("default").err().unwrap().code,
        "app.not-ready"
    );
    assert_eq!(fixture.open(false).await.unwrap_err().code, "app.not-ready");
    fixture.service.shutdown().await;
}

#[tokio::test]
async fn concurrent_open_is_rejected_before_waiting_for_profile_io() {
    let fixture = Fixture::new();
    let gate = Arc::new(PrepareGate::default());
    *fixture.source.gate.lock().unwrap() = Some(gate.clone());
    let product = fixture.product.clone();
    let service = fixture.service.clone();
    let opening = tokio::spawn(async move {
        product
            .open(&service, "default".into(), "mistbell".into(), false)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.cancelled())
        .await
        .unwrap();
    assert_eq!(fixture.open(false).await.unwrap_err().code, "app.busy");
    gate.release();
    opening.await.unwrap().unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn failed_registration_and_metadata_commit_release_record_writers() {
    let fixture = Fixture::new();
    let other = Service::new(
        Coordinator::new(Arc::new(Events::default()), 8).unwrap(),
        fixture.factory.clone(),
    );
    assert_eq!(
        fixture
            .product
            .open(&other, "default".into(), "mistbell".into(), false)
            .await
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    assert!(read_selection(&fixture.root).unwrap().is_none());
    let files = std::fs::read_dir(fixture.root.join("workspaces/mistbell/transcript"))
        .unwrap()
        .map(|file| file.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1);
    let id = files[0].file_stem().unwrap().to_str().unwrap();
    assert_eq!(
        fixture.storage.records.get(id).err().unwrap().code,
        "app.not-found"
    );
    // 冻结阶段阻塞后制造提交冲突：读取已通过，正式登记后 metadata 原子替换失败。
    let gate = Arc::new(PrepareGate::default());
    *fixture.source.gate.lock().unwrap() = Some(gate.clone());
    let product = fixture.product.clone();
    let service = fixture.service.clone();
    let opening = tokio::spawn(async move {
        product
            .open(&service, "default".into(), "mistbell".into(), false)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.cancelled())
        .await
        .unwrap();
    std::fs::create_dir(fixture.root.join("active-session.json")).unwrap();
    gate.release();
    assert!(
        opening
            .await
            .unwrap()
            .unwrap_err()
            .code
            .starts_with("store.")
    );
    let files = std::fs::read_dir(fixture.root.join("workspaces/mistbell/transcript"))
        .unwrap()
        .map(|file| file.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 2);
    for file in files {
        let id = file.file_stem().unwrap().to_str().unwrap();
        assert_eq!(
            fixture.storage.records.get(id).err().unwrap().code,
            "app.not-found"
        );
        assert_eq!(
            fixture.service.get_phase(id).unwrap_err().code,
            "app.not-found"
        );
    }
    assert_eq!(
        fixture.factory.freeze().err().unwrap().code,
        "app.not-ready"
    );
    other.shutdown().await;
    fixture.close().await;
}

#[tokio::test]
async fn resource_revision_failure_preserves_selection_but_closes_the_loaded_writer() {
    let fixture = Fixture::new();
    let mut header = builtin::header("mistbell").unwrap();
    header.script_revision = crate::record::format::hash(b"old resource");
    let id = header.session_id.clone();
    fixture.storage.records.create(header).unwrap();
    fixture.storage.records.close(&id).unwrap();
    let selected = Selection {
        version: 1,
        session_id: id.clone(),
        profile_id: "default".into(),
        script_id: "mistbell".into(),
    };
    write_selection(&fixture.root, &selected).unwrap();
    assert_eq!(fixture.open(false).await.unwrap_err().code, "app.not-ready");
    assert_eq!(
        read_selection(&fixture.root).unwrap().unwrap().session_id,
        id
    );
    assert_eq!(
        fixture.storage.records.get(&id).err().unwrap().code,
        "app.not-found"
    );
    fixture.close().await;
}

#[test]
fn selection_rejects_malformed_versions_identities_and_bounded_input() {
    let fixture = Fixture::new();
    let selected = Selection {
        version: 1,
        session_id: uuid::Uuid::new_v4().to_string(),
        profile_id: "default".into(),
        script_id: "mistbell".into(),
    };
    for (version, session_id, profile_id, script_id) in [
        (2, selected.session_id.clone(), "default", "mistbell"),
        (1, "not-uuid".into(), "default", "mistbell"),
        (1, selected.session_id.clone(), "", "mistbell"),
        (1, selected.session_id.clone(), "default", ""),
    ] {
        write_selection(
            &fixture.root,
            &Selection {
                version,
                session_id,
                profile_id: profile_id.into(),
                script_id: script_id.into(),
            },
        )
        .unwrap();
        assert_eq!(
            read_selection(&fixture.root).err().unwrap().code,
            "store.corrupt"
        );
    }
    std::fs::write(fixture.root.join("active-session.json"), "not-json").unwrap();
    assert_eq!(
        read_selection(&fixture.root).err().unwrap().code,
        "store.corrupt"
    );
    std::fs::write(fixture.root.join("active-session.json"), "x".repeat(4097)).unwrap();
    assert!(
        read_selection(&fixture.root)
            .err()
            .unwrap()
            .code
            .starts_with("store.")
    );
    assert_eq!(
        opened(&Selection {
            script_id: "missing".into(),
            ..selected
        })
        .unwrap_err()
        .code,
        "app.not-found"
    );
    fixture.storage.close().unwrap();
}

#[tokio::test]
async fn a_closed_phase_service_refuses_reopen_and_switch_without_erasing_selection() {
    let fixture = Fixture::new();
    let first = fixture.open(false).await.unwrap();
    fixture.service.shutdown().await;
    assert_eq!(fixture.open(false).await.unwrap_err().code, "app.not-ready");
    assert_eq!(fixture.open(true).await.unwrap_err().code, "app.not-ready");
    assert_eq!(
        read_selection(&fixture.root).unwrap().unwrap().session_id,
        first.session_id
    );
    assert!(fixture.storage.records.get(&first.session_id).is_ok());
    fixture.storage.close().unwrap();
}

#[test]
fn factory_validates_identity_and_propagates_provider_build_failures() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.factory.for_profile("").err().unwrap().code,
        "app.bad-request"
    );
    let resolved = fixture.source.freeze("default").unwrap();
    assert!(production_provider(resolved.frozen, &SystemProxySnapshot::default(), None).is_ok());
    let mut resolved = fixture.source.freeze("default").unwrap();
    resolved.frozen.credential = None;
    assert_eq!(
        production_provider(resolved.frozen, &SystemProxySnapshot::default(), None)
            .err()
            .unwrap()
            .code,
        "llm.missing-key"
    );
    let builder: Arc<ProviderBuilder> =
        Arc::new(|_, _, _| Err(Fault::new("llm.proxy-error", "fixture")));
    let factory = ProfileFactory::with_builder(
        fixture.source.clone(),
        fixture.storage.clone(),
        SystemProxySnapshot::default(),
        Arc::new(Mutex::new(Calibration::default())),
        builder,
        Arc::new(AllowAll),
    );
    assert_eq!(
        factory.for_profile("default").err().unwrap().code,
        "llm.proxy-error"
    );
    fixture.storage.close().unwrap();
}
