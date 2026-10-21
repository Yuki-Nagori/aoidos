use super::*;

#[test]
fn only_an_uncertain_commit_can_be_resolved_by_reading_the_active_selection() {
    let conflict = Fault::new("app.conflict", "revision changed");
    let uncertain = Fault::new("store.commit-unknown", "commit outcome unknown");
    assert!(!activation_error_matches_selection(
        &conflict,
        "same-session",
        "same-session"
    ));
    assert!(activation_error_matches_selection(
        &uncertain,
        "same-session",
        "same-session"
    ));
    assert!(!activation_error_matches_selection(
        &uncertain,
        "previous-session",
        "same-session"
    ));
}

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
    let opened = opened(&restored, "memory-run").unwrap();
    assert_eq!(opened.title, "雾钟地窖");
    assert_eq!(opened.memory_run_id, "memory-run");
    let invalid = Selection {
        script_id: "../mistbell".into(),
        ..selected
    };
    write_selection(&root, &invalid).unwrap();
    assert_eq!(read_selection(&root).err().unwrap().code, "store.corrupt");
    std::fs::remove_dir_all(&root).unwrap();
}

#[tokio::test]
async fn sqlite_active_selection_wins_over_a_stale_compatibility_file() {
    let fixture = Fixture::new();
    let active = Selection {
        version: 1,
        session_id: uuid::Uuid::new_v4().to_string(),
        profile_id: "default".into(),
        script_id: "mistbell".into(),
    };
    let stale = Selection {
        session_id: uuid::Uuid::new_v4().to_string(),
        ..active.clone()
    };
    let profile = fixture.factory.public_profile("default").unwrap();
    fixture
        .storage
        .register_new_run(&crate::run_catalog::NewRun {
            session_id: active.session_id.clone(),
            script_id: active.script_id.clone(),
            created_at: "2026-10-21T00:00:00Z".into(),
            memory_run_id: uuid::Uuid::new_v4().to_string(),
            profile,
        })
        .unwrap();
    write_selection(&fixture.root, &stale).unwrap();

    let restored = read_authoritative_selection(&fixture.storage, &fixture.root)
        .unwrap()
        .unwrap();
    assert_eq!(restored.session_id, active.session_id);
    assert_eq!(restored.profile_id, active.profile_id);
    assert_eq!(read_selection(&fixture.root).unwrap(), Some(active));
    fixture.close().await;
}

#[test]
fn durable_factory_requires_run_identity_and_freezes_provider_price_route() {
    let root =
        std::env::temp_dir().join(format!("aoidos-durable-profile-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let storage = Arc::new(Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(RecordEvents::default()),
    ));
    storage.ready().unwrap();
    let source = Arc::new(Source {
        calls: AtomicUsize::new(0),
        error: Mutex::new(None),
        identity: Mutex::new(None),
        model: Mutex::new("deepseek-v4-pro".into()),
        provider_id: Mutex::new("deepseek".into()),
        gate: Mutex::new(None),
    });
    let factory = ProfileFactory::new(
        source,
        storage.clone(),
        SystemProxySnapshot::default(),
        Arc::new(Mutex::new(Calibration::default())),
    );
    factory.select("default".into());
    assert_eq!(
        factory.freeze().err().unwrap().code,
        "budget.run-limit-missing"
    );
    assert_eq!(
        factory
            .freeze_for_session("not-a-session")
            .err()
            .unwrap()
            .code,
        "app.not-found"
    );
    storage.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn durable_game_and_private_requests_use_the_canonical_session_budget() {
    use crate::billing::BillingLedger;
    use aoidos_billing::{
        money::{Currency, NanoMoney},
        providers::deepseek,
    };
    use aoidos_llm::{
        guard::GuardSpec,
        provider::{CompletionInput, ProviderInput, RequestMode},
        schedule::{AttemptIdentity, AttemptKind},
    };

    let fixture = Fixture::new();
    *fixture.source.model.lock().unwrap() = "deepseek-flash".into();
    let factory = ProfileFactory::new(
        fixture.source.clone(),
        fixture.storage.clone(),
        SystemProxySnapshot::default(),
        Arc::new(Mutex::new(Calibration::default())),
    );
    let product = Arc::new(Product::new(
        &fixture.root,
        fixture.storage.clone(),
        fixture.coordinator.clone(),
        Arc::new(PhaseEventsFixture::default()),
        factory.clone(),
    ));
    let service = Service::new(fixture.coordinator.clone(), factory.clone());
    let opened = product
        .open(&service, "default".into(), "mistbell".into(), true)
        .await
        .unwrap();
    let now = "2026-10-21T00:00:00Z";
    let price = deepseek::flash_prices()
        .into_iter()
        .find(|price| price.currency == Currency::Cny)
        .unwrap();
    fixture
        .storage
        .with_database(|connection| {
            BillingLedger::register_price(connection, &price, now)
                .map_err(|_| Fault::bad_request())?;
            BillingLedger::select_profile_price(
                connection,
                "default",
                "deepseek",
                "deepseek-flash",
                "direct",
                Currency::Cny,
                now,
            )
            .map_err(|_| Fault::bad_request())?;
            BillingLedger::set_settings(
                connection,
                &opened.session_id,
                NanoMoney::parse("5").unwrap(),
                NanoMoney::ZERO,
                None,
                now,
            )
            .map_err(|_| Fault::bad_request())?;
            Ok(())
        })
        .unwrap();

    let frozen = factory.freeze_for_session(&opened.session_id).unwrap();
    let input = ProviderInput::Completion(CompletionInput {
        prompt: "只验证预算预留，不发送网络请求。".into(),
    });
    let input_upper_bound = input.token_upper_bound();
    let prepared = frozen
        .generation
        .prepare(input, GuardSpec::default(), false)
        .unwrap();
    let attempt = AttemptIdentity {
        request_id: uuid::Uuid::new_v4().to_string(),
        turn_id: "budget-identity-test".into(),
        attempt: 1,
        kind: AttemptKind::Initial,
        model: "deepseek-flash".into(),
        mode: RequestMode::Completion,
        max_tokens: 2048,
        temperature: 1.0,
    };
    prepared
        .budget
        .reserve_with_input_upper_bound(&attempt, input_upper_bound)
        .unwrap();

    let private_input = ProviderInput::Completion(CompletionInput {
        prompt: "验证私有记忆请求使用 canonical 账务范围。".into(),
    });
    *fixture.source.model.lock().unwrap() = "deepseek-v4-pro".into();
    let private_prepared = factory
        .prepare_private_for_session("default", &opened.session_id, None, |_| {
            Ok((private_input.clone(), GuardSpec::default()))
        })
        .unwrap();
    let private_attempt = AttemptIdentity {
        request_id: uuid::Uuid::new_v4().to_string(),
        turn_id: "private-budget-identity-test".into(),
        attempt: 1,
        kind: AttemptKind::Initial,
        model: "deepseek-flash".into(),
        mode: RequestMode::Completion,
        max_tokens: 2048,
        temperature: 1.0,
    };
    private_prepared
        .budget
        .reserve_with_input_upper_bound(&private_attempt, private_input.token_upper_bound())
        .unwrap();

    let counts = fixture
        .storage
        .with_database(|connection| {
            let count_for = |run_id: &str| {
                connection.query_row(
                    "SELECT count(*) FROM billing_physical_requests WHERE run_id=?1",
                    [run_id],
                    |row| row.get::<_, i64>(0),
                )
            };
            Ok((
                count_for(&opened.memory_run_id).map_err(|_| Fault::bad_request())?,
                count_for(&opened.session_id).map_err(|_| Fault::bad_request())?,
                connection
                    .query_row(
                        "SELECT count(*) FROM billing_physical_requests WHERE run_id=?1 AND model_id<>'deepseek-flash'",
                        [&opened.session_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|_| Fault::bad_request())?,
            ))
        })
        .unwrap();
    assert_eq!(counts, (0, 2, 0));

    // 旧账务数据出现后，既有请求仍可幂等重放，但两个请求入口都拒绝新收费。
    fixture
        .storage
        .with_database(|connection| {
            BillingLedger::set_settings(
                connection,
                &opened.memory_run_id,
                NanoMoney::ZERO,
                NanoMoney::ZERO,
                None,
                now,
            )
            .map_err(|_| Fault::bad_request())?;
            Ok(())
        })
        .unwrap();
    prepared
        .budget
        .reserve_with_input_upper_bound(&attempt, input_upper_bound)
        .unwrap();
    let new_attempt = AttemptIdentity {
        request_id: uuid::Uuid::new_v4().to_string(),
        turn_id: "new-game-request".into(),
        ..attempt
    };
    assert_eq!(
        prepared
            .budget
            .reserve_with_input_upper_bound(&new_attempt, input_upper_bound)
            .unwrap_err()
            .reason,
        "budget.reconciliation-required"
    );

    let private_with_legacy_scope = factory
        .prepare_private_for_session(
            "default",
            &opened.session_id,
            Some(&opened.memory_run_id),
            |_| Ok((private_input.clone(), GuardSpec::default())),
        )
        .unwrap();
    private_with_legacy_scope
        .budget
        .reserve_with_input_upper_bound(&private_attempt, private_input.token_upper_bound())
        .unwrap();
    let new_private_attempt = AttemptIdentity {
        request_id: uuid::Uuid::new_v4().to_string(),
        turn_id: "new-private-request".into(),
        ..private_attempt
    };
    assert_eq!(
        private_with_legacy_scope
            .budget
            .reserve_with_input_upper_bound(
                &new_private_attempt,
                private_input.token_upper_bound(),
            )
            .unwrap_err()
            .reason,
        "budget.reconciliation-required"
    );
    service.shutdown().await;
    fixture.close().await;
}

#[test]
fn billing_route_policy_is_explicit_per_provider_family() {
    assert_eq!(billing_route_policy("openrouter"), "openrouter");
    assert_eq!(billing_route_policy("deepseek"), "direct");
}

use crate::{
    game::test_support::{PhaseEventsFixture, PrepareGate},
    migration::{MigrationEvent, MigrationEvents},
    ports::{EventPort, PreparedEvent, TurnEvent},
    record::{facts::DiceMode, test_support::Events as RecordEvents},
};
use aoidos_memory::{
    model::{
        Attitude, Candidate, CausalPort, ChangeKind, Classification, Dimension, EntryState,
        Expected, InputMode as MemoryInputMode, Plan, PolicyOverrides, Proposal, ProposalContext,
        Subject, SubjectRef, Target, Version, new_id, set_hash,
    },
    repository::{BatchState, MAX_PROPOSAL_TARGET_SEARCH_BYTES},
};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[path = "../../../tests/support/disk_metrics.rs"]
mod disk_metrics;

#[derive(Default)]
struct StressProvider {
    starts: AtomicUsize,
}
impl aoidos_llm::provider::Provider for StressProvider {
    fn capabilities(
        &self,
        model: &str,
        _mode: aoidos_llm::provider::RequestMode,
    ) -> aoidos_llm::provider::ProviderCapabilities {
        aoidos_llm::provider::ProviderCapabilities {
            model: model.into(),
            context_limit: Some(1_000_000),
            completion: true,
            chat: true,
            prefix: false,
            thinking: false,
            stop_limit: 16,
            max_output_tokens: 4096,
            temperature_effective: true,
            temperature_min: 0.0,
            temperature_max: 2.0,
        }
    }

    fn start<'a>(
        &'a self,
        request: aoidos_llm::provider::ProviderRequest,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> aoidos_llm::provider::StartFuture<'a> {
        use aoidos_llm::provider::{ProviderDelta, ProviderFinish, ProviderInput, ProviderStream};

        self.starts.fetch_add(1, Ordering::SeqCst);
        let text = match request.input {
            ProviderInput::Completion(input)
                if input.prompt.contains("[AOIDOS:CHECK-PROPOSAL]")
                    || input.prompt.contains("[AOIDOS:SCENE-PROPOSAL]") =>
            {
                "{\"kind\":\"noCheck\"}"
            }
            _ => "灯光照亮了房间。",
        };
        Box::pin(async move {
            let deltas = vec![
                Ok(ProviderDelta::Text(text.into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ];
            Ok(Box::pin(futures::stream::iter(deltas)) as ProviderStream)
        })
    }
}

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
    model: Mutex<String>,
    provider_id: Mutex<String>,
    gate: Mutex<Option<Arc<PrepareGate>>>,
}
impl ProfileSource for Source {
    fn public_profile(&self, id: &str) -> Result<aoidos_llm::config::LlmProfile, Fault> {
        if let Some(gate) = self.gate.lock().unwrap().take() {
            gate.block(std::time::Duration::from_secs(5))?;
        }
        Ok(aoidos_llm::config::LlmProfile {
            profile_id: self
                .identity
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| id.into()),
            provider_id: self.provider_id.lock().unwrap().clone(),
            model: self.model.lock().unwrap().clone(),
            mode: aoidos_llm::config::ProfileMode::Completion,
            thinking: false,
            sampling: aoidos_llm::sampling::Sampling {
                temperature: 1.0,
                max_tokens: 64,
            },
            proxy: aoidos_llm::config::ProxyConfig::None,
        })
    }

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
                    provider_id: self.provider_id.lock().unwrap().clone(),
                    model: self.model.lock().unwrap().clone(),
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

    fn freeze_snapshot(
        &self,
        profile: &aoidos_llm::config::LlmProfile,
    ) -> Result<ResolvedProfile, Fault> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(ResolvedProfile {
            frozen: FrozenProfile {
                profile: profile.clone(),
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
    stress_provider: Option<Arc<StressProvider>>,
}
impl Fixture {
    fn new() -> Self {
        Self::build(None)
    }
    fn with_stress_provider() -> Self {
        Self::build(Some(Arc::new(StressProvider::default())))
    }
    fn build(stress_provider: Option<Arc<StressProvider>>) -> Self {
        let root = std::env::temp_dir().join(format!("aoidos-product-{}", uuid::Uuid::new_v4()));
        Self::build_at(root, stress_provider)
    }
    fn build_at(root: PathBuf, stress_provider: Option<Arc<StressProvider>>) -> Self {
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
            model: Mutex::new("deepseek-v4-pro".into()),
            provider_id: Mutex::new("deepseek".into()),
            gate: Mutex::new(None),
        });
        let builder: Arc<ProviderBuilder> = if let Some(provider) = &stress_provider {
            let provider = provider.clone();
            Arc::new(move |_, _, _| Ok(provider.clone() as Arc<dyn aoidos_llm::provider::Provider>))
        } else {
            Arc::new(production_provider)
        };
        let factory = ProfileFactory::with_builder(
            source.clone(),
            storage.clone(),
            SystemProxySnapshot::default(),
            Arc::new(Mutex::new(Calibration::default())),
            builder,
            Arc::new(AllowAll),
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
            stress_provider,
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
    let first_run = fixture
        .storage
        .memory_run_for_session(&first.session_id)
        .unwrap();
    assert_eq!(first.memory_run_id, first_run.run_id);
    let catalog = fixture
        .storage
        .run_catalog_page(None, crate::run_catalog::DEFAULT_PAGE_SIZE)
        .unwrap();
    assert_eq!(catalog.entries.len(), 1);
    assert_eq!(catalog.entries[0].session_id, first.session_id);
    assert_eq!(
        catalog.entries[0].memory_run_id.as_deref(),
        Some(first.memory_run_id.as_str())
    );
    assert_eq!(
        catalog.entries[0].billing_run_id.as_deref(),
        Some(first.session_id.as_str())
    );
    assert!(catalog.entries[0].active);
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
    *fixture.source.model.lock().unwrap() = "deepseek-v4-flash".into();
    let reopened_with_frozen_profile = fixture.open(false).await.unwrap();
    assert_eq!(reopened_with_frozen_profile.session_id, first.session_id);
    *fixture.source.model.lock().unwrap() = "deepseek-v4-pro".into();
    let frozen = fixture.factory.freeze().unwrap();
    assert_eq!(frozen.profile_id, "default");
    assert_eq!(frozen.dice_mode, DiceMode::Manual);
    let calls = fixture.source.calls.load(Ordering::SeqCst);
    let mut next_parameters = fixture.storage.memory_app_config().unwrap().parameters;
    next_parameters.algorithm = aoidos_memory::model::Algorithm::None;
    fixture
        .storage
        .save_memory_app_config(1, next_parameters)
        .unwrap();
    let lease = fixture.coordinator.acquire().unwrap();
    *fixture.source.error.lock().unwrap() = Some(Fault::new("llm.missing-key", "fixture"));
    let reopened = fixture.open(false).await.unwrap();
    assert_eq!(reopened.session_id, first.session_id);
    assert_eq!(reopened.memory_run_id, first.memory_run_id);
    assert_eq!(
        fixture
            .storage
            .memory_run_for_session(&first.session_id)
            .unwrap()
            .policy
            .algorithm,
        first_run.policy.algorithm
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
    assert_eq!(
        fixture
            .product
            .open(&fixture.service, "second".into(), "mistbell".into(), false)
            .await
            .unwrap_err()
            .code,
        "app.conflict"
    );
    let next = fixture
        .product
        .open(&fixture.service, "second".into(), "mistbell".into(), true)
        .await
        .unwrap();
    assert_eq!(fixture.factory.freeze().unwrap().profile_id, "second");
    assert_ne!(first.session_id, next.session_id);
    assert_ne!(first.memory_run_id, next.memory_run_id);
    assert_eq!(
        fixture
            .storage
            .memory_run_for_session(&next.session_id)
            .unwrap()
            .policy
            .algorithm,
        aoidos_memory::model::Algorithm::None
    );
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
async fn explicitly_selecting_a_catalog_run_restores_its_frozen_identity() {
    let fixture = Fixture::new();
    let first = fixture.open(true).await.unwrap();
    *fixture.source.model.lock().unwrap() = "deepseek-v4-flash".into();
    let second = fixture
        .product
        .open(&fixture.service, "default".into(), "mistbell".into(), true)
        .await
        .unwrap();
    assert_ne!(first.session_id, second.session_id);

    let page = fixture
        .storage
        .run_catalog_page(None, crate::run_catalog::DEFAULT_PAGE_SIZE)
        .unwrap();
    let first_entry = page
        .entries
        .iter()
        .find(|entry| entry.session_id == first.session_id)
        .unwrap();
    let restored = fixture
        .product
        .open_selected(
            &fixture.service,
            first.session_id.clone(),
            first_entry.revision,
        )
        .await
        .unwrap();

    assert_eq!(restored.session_id, first.session_id);
    assert_eq!(restored.memory_run_id, first.memory_run_id);
    assert!(!restored.selection_mirror_pending);
    assert_eq!(
        fixture
            .storage
            .active_run_selection()
            .unwrap()
            .unwrap()
            .session_id,
        first.session_id
    );
    assert!(fixture.service.get_phase(&second.session_id).is_err());
    assert!(fixture.service.get_phase(&first.session_id).is_ok());
    fixture.close().await;
}

#[tokio::test]
async fn selected_run_reports_when_the_compatibility_mirror_needs_repair() {
    let fixture = Fixture::new();
    let first = fixture.open(true).await.unwrap();
    let second = fixture
        .product
        .open(&fixture.service, "default".into(), "mistbell".into(), true)
        .await
        .unwrap();
    let page = fixture
        .storage
        .run_catalog_page(None, crate::run_catalog::DEFAULT_PAGE_SIZE)
        .unwrap();
    let revision = page
        .entries
        .iter()
        .find(|entry| entry.session_id == first.session_id)
        .unwrap()
        .revision;

    std::fs::remove_file(fixture.root.join("active-session.json")).unwrap();
    std::fs::create_dir(fixture.root.join("active-session.json")).unwrap();
    let restored = fixture
        .product
        .open_selected(&fixture.service, first.session_id.clone(), revision)
        .await
        .unwrap();

    assert!(restored.selection_mirror_pending);
    assert_eq!(restored.session_id, first.session_id);
    assert_eq!(
        fixture
            .storage
            .active_run_selection()
            .unwrap()
            .unwrap()
            .session_id,
        first.session_id
    );
    assert!(fixture.service.get_phase(&second.session_id).is_err());
    fixture
        .service
        .close_session(&first.session_id)
        .await
        .unwrap();
    fixture.storage.records.close(&first.session_id).unwrap();
    let reopened = fixture
        .product
        .open(&fixture.service, "default".into(), "mistbell".into(), false)
        .await
        .unwrap();
    assert!(reopened.selection_mirror_pending);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "显式运行的同一产品会话 Engine / SQLite 长回合探索"]
async fn memory_runtime_product_rounds_collect_into_the_same_disk_sqlite_run() {
    for rounds in [50, 100, 200] {
        let fixture = Fixture::with_stress_provider();
        let opened = fixture.open(false).await.unwrap();
        let started = std::time::Instant::now();

        for index in 0..rounds {
            let text = format!("查看第 {index} 个线索");
            let accepted = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    match fixture
                        .service
                        .submit_input(&opened.session_id, text.clone())
                        .await
                    {
                        Ok(accepted) => break accepted,
                        Err(error) if error.code == "app.busy" => {
                            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                        }
                        Err(error) => panic!("合成产品回合无法接纳：{error}"),
                    }
                }
            })
            .await
            .expect("前一合成回合应及时释放运行门");
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let state = fixture.service.get_phase(&opened.session_id).unwrap().state;
                    if let Some(operation) = state.last_operation
                        && operation.operation_id == accepted.operation_id
                    {
                        match operation.outcome {
                            crate::game::state::OperationOutcome::Accepted => {}
                            crate::game::state::OperationOutcome::Completed
                                if state.phase == crate::game::state::Phase::Idle
                                    && state.in_flight.is_none() =>
                            {
                                break;
                            }
                            crate::game::state::OperationOutcome::Completed => {}
                            outcome => {
                                let record_count = fixture.storage.records.get(&opened.session_id)
                                    .ok()
                                    .and_then(|session| session.try_lock().ok().map(|session| {
                                        let count = session.next_sequence().saturating_sub(1);
                                        let facts = (1..=count).filter_map(|seq| {
                                            let parsed = session.read(seq).ok()?;
                                            let body = parsed.body?.body;
                                            let crate::record::format::Body::System { code, data, .. } = body else { return None; };
                                            let crate::record::facts::Fact::SceneAdvanced { position, .. } = crate::record::facts::Fact::decode(&code, &data).ok()? else { return None; };
                                            Some((seq, position.scene_id))
                                        }).collect::<Vec<_>>();
                                        (count, facts)
                                    }));
                                panic!(
                                "第 {index} 个合成回合以非成功终态结束：{outcome:?}，错误：{:?}，当前场景：{:?}，初始场景：{:?}，记录数与场景事实：{:?}",
                                operation.error,
                                state.scene.as_ref().map(|scene| &scene.scene_id),
                                fixture
                                    .storage
                                    .memory_catalog_for_run(&opened.memory_run_id)
                                    .unwrap()
                                    .1
                                    .presence_policy()
                                    .map(|policy| policy.initial_scene_id()),
                                    record_count
                                )
                            },
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("合成产品回合应在有界时间内完成");
        }
        let rounds_elapsed_ms = started.elapsed().as_millis();

        let mut cursor = None;
        let mut eligible_materials = 0;
        let scan_started = std::time::Instant::now();
        loop {
            let page = fixture
                .storage
                .collect_memory_materials(&opened.memory_run_id, cursor)
                .unwrap();
            eligible_materials += page.eligible_count;
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let scan_ms = scan_started.elapsed().as_millis();
        let provider_starts = fixture
            .stress_provider
            .as_ref()
            .expect("压力测试应使用合成 Provider")
            .starts
            .load(Ordering::SeqCst);
        let record_path = fixture
            .root
            .join("workspaces/mistbell/transcript")
            .join(format!("{}.jsonl", opened.session_id));
        let record_bytes = std::fs::metadata(record_path)
            .expect("产品会话记录应落到工作区磁盘")
            .len();
        let sqlite_bytes = std::fs::metadata(fixture.root.join("storage.sqlite"))
            .expect("产品会话的记忆应写入 SQLite")
            .len();
        let wal_bytes = std::fs::metadata(fixture.root.join("storage.sqlite-wal"))
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let files_after_rounds = disk_metrics::disk_summary(&fixture.root);

        assert_eq!(provider_starts, rounds * 2);
        assert_eq!(eligible_materials, rounds * 5);
        assert!(sqlite_bytes > 0);
        assert!(record_bytes > 0);
        let peak_rss = disk_metrics::peak_rss_bytes()
            .map(|bytes| bytes.to_string())
            .unwrap_or_else(|| "null".into());
        println!(
            "{{\"rounds\":{rounds},\"syntheticProviderStarts\":{provider_starts},\"eligibleMaterials\":{eligible_materials},\"roundsElapsedMs\":{rounds_elapsed_ms},\"materialScanMs\":{scan_ms},\"totalElapsedMs\":{},\"recordBytes\":{record_bytes},\"sqliteBytes\":{sqlite_bytes},\"walBytes\":{wal_bytes},\"processPeakRssBytes\":{peak_rss},\"filesAfterRounds\":{files_after_rounds}}}",
            started.elapsed().as_millis()
        );
        fixture.close().await;
    }
}

#[tokio::test]
async fn product_session_can_commit_player_memory_from_authorized_input() {
    let fixture = Fixture::with_stress_provider();
    let config = fixture.storage.memory_script_config("mistbell").unwrap();
    fixture
        .storage
        .save_memory_script_config(
            "mistbell",
            config.config_version,
            config.app_config_version,
            PolicyOverrides {
                active_capacity: Some(1_000),
                ..PolicyOverrides::default()
            },
        )
        .unwrap();
    let opened = fixture.open(false).await.unwrap();
    for index in 0..2 {
        complete_product_stress_round(
            &fixture,
            &opened.session_id,
            &format!("/ooc memorytoken{index:04} 玩家偏好实验记录"),
            index,
        )
        .await;
    }

    let page = fixture
        .storage
        .collect_memory_materials(&opened.memory_run_id, None)
        .unwrap();
    assert!(page.eligible_count >= 2);
    let snapshot = fixture
        .storage
        .prepare_collectible_memory_batch(&opened.memory_run_id)
        .unwrap();
    let batch_id = snapshot.batch_id.clone();
    let catalog = fixture
        .storage
        .memory_catalog_for_run(&opened.memory_run_id)
        .unwrap()
        .1;
    fixture
        .storage
        .freeze_memory_batch(&catalog, snapshot)
        .unwrap();

    let added = submit_product_stress_batch(
        &fixture.storage,
        &opened.session_id,
        &catalog,
        &batch_id,
        true,
    );
    assert_eq!(added, 2);
    assert_eq!(
        count_active_memory_entries(&fixture.storage, &opened.memory_run_id),
        2
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "显式运行的同一产品会话 1,000 条活动记忆目标检索探索"]
async fn memory_runtime_product_session_retrieves_target_after_1000_active_entries() {
    const ENTRY_COUNT: usize = 1_000;

    let fixture = Fixture::with_stress_provider();
    let config = fixture.storage.memory_script_config("mistbell").unwrap();
    fixture
        .storage
        .save_memory_script_config(
            "mistbell",
            config.config_version,
            config.app_config_version,
            PolicyOverrides {
                active_capacity: Some(ENTRY_COUNT as u64),
                ..PolicyOverrides::default()
            },
        )
        .unwrap();
    let opened = fixture.open(false).await.unwrap();
    let catalog = fixture
        .storage
        .memory_catalog_for_run(&opened.memory_run_id)
        .unwrap()
        .1;

    // 以合成的既有 OOC 记录注入容量负载，批次仍走真实 Storage 冻结和完成路径。
    {
        let session = fixture.storage.records.get(&opened.session_id).unwrap();
        let mut session = session.lock().unwrap();
        for index in 0..ENTRY_COUNT {
            session
                .append(crate::record::format::Body::PlayerSpeech {
                    player_id: "player".into(),
                    text: format!("memorytoken{index:04}"),
                    mode: Some(crate::record::format::InputMode::OutOfCharacter),
                    content_range: None,
                })
                .unwrap();
        }
    }

    let mut cursor = None;
    let mut collected_materials = 0;
    loop {
        let page = fixture
            .storage
            .collect_memory_materials(&opened.memory_run_id, cursor)
            .unwrap();
        collected_materials += page.eligible_count;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(collected_materials, ENTRY_COUNT);

    let create_started = std::time::Instant::now();
    let mut completed_batches = 0;
    for _ in 0..ENTRY_COUNT.div_ceil(16) {
        let snapshot = fixture
            .storage
            .prepare_collectible_memory_batch(&opened.memory_run_id)
            .unwrap();
        assert!(!snapshot.spans.is_empty(), "应仍有未处理的合成素材");
        let batch_id = snapshot.batch_id.clone();
        fixture
            .storage
            .freeze_memory_batch(&catalog, snapshot)
            .unwrap();
        let added = submit_product_stress_batch(
            &fixture.storage,
            &opened.session_id,
            &catalog,
            &batch_id,
            true,
        );
        assert!(added > 0, "容量批次应提交至少一条合成活动记忆");
        completed_batches += 1;
    }
    let create_ms = create_started.elapsed().as_millis();
    assert_eq!(completed_batches, ENTRY_COUNT.div_ceil(16));
    assert_eq!(
        count_active_memory_entries(&fixture.storage, &opened.memory_run_id),
        ENTRY_COUNT
    );

    let lookup_started = std::time::Instant::now();
    {
        let session = fixture.storage.records.get(&opened.session_id).unwrap();
        session
            .lock()
            .unwrap()
            .append(crate::record::format::Body::PlayerSpeech {
                player_id: "player".into(),
                text: "memorytoken0999 检索确认".into(),
                mode: Some(crate::record::format::InputMode::OutOfCharacter),
                content_range: None,
            })
            .unwrap();
    }
    let mut cursor = None;
    let mut lookup_materials = 0;
    loop {
        let page = fixture
            .storage
            .collect_memory_materials(&opened.memory_run_id, cursor)
            .unwrap();
        lookup_materials += page.eligible_count;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(lookup_materials, 1);
    let lookup = fixture
        .storage
        .prepare_collectible_memory_batch(&opened.memory_run_id)
        .unwrap();
    assert_eq!(lookup.spans.len(), 1);
    let lookup_batch_id = lookup.batch_id.clone();
    fixture
        .storage
        .freeze_memory_batch(&catalog, lookup)
        .unwrap();
    let context = read_frozen_proposal_context(
        &fixture.storage,
        &opened.session_id,
        &catalog,
        &lookup_batch_id,
    );
    let search = context.target_search.as_ref().unwrap();
    assert_eq!(search.scanned_entries, ENTRY_COUNT as u32);
    assert_eq!(search.matched_entries, 1);
    assert_eq!(context.targets.len(), 1);
    assert_eq!(
        context.targets.values().next().unwrap().summary,
        "memorytoken0999"
    );
    let lookup_preparation_ms = lookup_started.elapsed().as_millis();
    submit_product_stress_batch(
        &fixture.storage,
        &opened.session_id,
        &catalog,
        &lookup_batch_id,
        false,
    );

    let (snapshot_before_close, _) = fixture
        .storage
        .with_memory(&opened.session_id, &catalog, |repository, connection, _| {
            repository.batch(connection, &lookup_batch_id)
        })
        .unwrap();
    fixture.storage.records.close(&opened.session_id).unwrap();
    fixture.close().await;
    let reopen_started = std::time::Instant::now();
    let reopened = Fixture::build_at(fixture.root.clone(), fixture.stress_provider.clone());
    let reopened_session = reopened.open(false).await.unwrap();
    assert_eq!(reopened_session.session_id, opened.session_id);
    assert_eq!(reopened_session.memory_run_id, opened.memory_run_id);
    let snapshot_after_reopen = reopened
        .storage
        .with_memory(&opened.session_id, &catalog, |repository, connection, _| {
            repository.batch(connection, &lookup_batch_id)
        })
        .unwrap()
        .0;
    assert_eq!(snapshot_after_reopen, snapshot_before_close);
    assert_eq!(
        count_active_memory_entries(&reopened.storage, &opened.memory_run_id),
        ENTRY_COUNT
    );
    let reopen_validation_ms = reopen_started.elapsed().as_millis();
    let long_game_started = std::time::Instant::now();
    for index in 0..200 {
        complete_product_stress_round(
            &reopened,
            &opened.session_id,
            &format!("查看第 {index} 个线索"),
            index,
        )
        .await;
    }
    let long_game_ms = long_game_started.elapsed().as_millis();
    assert_eq!(
        count_active_memory_entries(&reopened.storage, &opened.memory_run_id),
        ENTRY_COUNT,
        "长对局不应改变已提交的活动记忆数"
    );
    let provider_starts = reopened
        .stress_provider
        .as_ref()
        .unwrap()
        .starts
        .load(Ordering::SeqCst);
    assert_eq!(provider_starts, 400, "200 个产品回合应各执行两次合成生成");
    let sqlite_path = reopened.root.join("storage.sqlite");
    let record_path = reopened
        .root
        .join("workspaces/mistbell/transcript")
        .join(format!("{}.jsonl", opened.session_id));
    let sqlite_bytes = std::fs::metadata(&sqlite_path).unwrap().len();
    let record_bytes = std::fs::metadata(&record_path).unwrap().len();
    let wal_bytes = std::fs::metadata(reopened.root.join("storage.sqlite-wal"))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let peak_rss = disk_metrics::peak_rss_bytes()
        .map(|bytes| bytes.to_string())
        .unwrap_or_else(|| "null".into());
    let files_after_reopen = disk_metrics::disk_summary(&reopened.root);
    println!(
        "{{\"activeEntries\":{ENTRY_COUNT},\"completedBatches\":{completed_batches},\"createMs\":{create_ms},\"lookupPreparationMs\":{lookup_preparation_ms},\"reopenValidationMs\":{reopen_validation_ms},\"longGameRounds\":200,\"longGameMs\":{long_game_ms},\"recordBytes\":{record_bytes},\"sqliteBytes\":{sqlite_bytes},\"walBytes\":{wal_bytes},\"processPeakRssBytes\":{peak_rss},\"filesAfterReopen\":{files_after_reopen},\"providerStarts\":{}}}",
        provider_starts,
    );
    reopened.close().await;
}

#[tokio::test]
#[ignore = "显式运行的同一产品会话 10,000 条活动记忆重开与预算拒绝探索"]
async fn memory_runtime_product_session_reopens_at_10000_active_entries() {
    const ENTRY_COUNT: usize = 10_000;
    const ROUNDS_AFTER_REOPEN: usize = 6;

    let fixture = Fixture::with_stress_provider();
    let config = fixture.storage.memory_script_config("mistbell").unwrap();
    fixture
        .storage
        .save_memory_script_config(
            "mistbell",
            config.config_version,
            config.app_config_version,
            PolicyOverrides {
                active_capacity: Some(ENTRY_COUNT as u64),
                ..PolicyOverrides::default()
            },
        )
        .unwrap();
    let opened = fixture.open(false).await.unwrap();
    let catalog = fixture
        .storage
        .memory_catalog_for_run(&opened.memory_run_id)
        .unwrap()
        .1;
    let source_seq = {
        let session = fixture.storage.records.get(&opened.session_id).unwrap();
        session
            .lock()
            .unwrap()
            .append(crate::record::format::Body::PlayerSpeech {
                player_id: "player".into(),
                text: "用于满负载记忆容量探索的合成来源".into(),
                mode: Some(crate::record::format::InputMode::OutOfCharacter),
                content_range: None,
            })
            .unwrap()
    };

    // 使用生产版本校验与 SQLite 操作写入完整有效条目，避免把 NULL 版本行误当成容量证据。
    let seed_started = std::time::Instant::now();
    fixture
        .storage
        .with_memory_context(
            &opened.session_id,
            &catalog,
            |repository, connection, port, _, _| {
                let run = repository.run(connection, &opened.memory_run_id)?;
                let source = port.reference(&opened.memory_run_id, source_seq)?;
                let source_set_hash = set_hash(std::slice::from_ref(&source))?;
                let evidence_set_hash = set_hash::<aoidos_memory::model::KnowledgeEvidence>(&[])?;
                let versions = (0..ENTRY_COUNT)
                    .map(|index| Version {
                        version: 1,
                        entry_id: new_id(),
                        version_id: new_id(),
                        parent_version_id: None,
                        restored_from_version_id: None,
                        subject: Subject::PlayerPreference,
                        dimension: Dimension::PlayerPreference,
                        summary: format!("合成长期容量记忆 {index:05}"),
                        attitude: Attitude::Believed,
                        inferred: false,
                        change: ChangeKind::New,
                        sources: vec![source.clone()],
                        evidence: vec![],
                        source_set_hash,
                        evidence_set_hash,
                        policy_hash: run.policy_hash,
                    })
                    .collect::<Vec<_>>();

                for versions in versions.chunks(16) {
                    let boundary = port.boundary()?;
                    let targets = versions
                        .iter()
                        .map(|version| Target {
                            entry_id: version.entry_id.clone(),
                            expected: Expected {
                                version_id: None,
                                state_revision: 0,
                            },
                            next: Some(version.clone()),
                            state: EntryState::Active,
                            effects: vec![],
                        })
                        .collect();
                    let plan = Plan {
                        expected_logical_clock: repository.clock(
                            connection,
                            port,
                            &opened.memory_run_id,
                        )?,
                        operation_id: new_id(),
                        run_id: opened.memory_run_id.clone(),
                        history_revision: boundary.history_revision,
                        entity_revision: boundary.entity_revision,
                        evidence_revision: boundary.evidence_revision,
                        policy_hash: run.policy_hash,
                        batch_id: None,
                        candidate_ids: vec![],
                        targets,
                        processed_materials: vec![],
                        reason: "10,000 条产品容量测试夹具".into(),
                    };
                    repository.prepare(connection, port, &plan)?;
                    repository.write_versions(connection, &plan.operation_id)?;
                    repository.apply(connection, port, &plan.operation_id)?;
                }
                let extra_version = Version {
                    version: 1,
                    entry_id: new_id(),
                    version_id: new_id(),
                    parent_version_id: None,
                    restored_from_version_id: None,
                    subject: Subject::PlayerPreference,
                    dimension: Dimension::PlayerPreference,
                    summary: "超出活动容量的合成记忆".into(),
                    attitude: Attitude::Believed,
                    inferred: false,
                    change: ChangeKind::New,
                    sources: vec![source.clone()],
                    evidence: vec![],
                    source_set_hash,
                    evidence_set_hash,
                    policy_hash: run.policy_hash,
                };
                let boundary = port.boundary()?;
                let overflow_plan = Plan {
                    expected_logical_clock: repository.clock(
                        connection,
                        port,
                        &opened.memory_run_id,
                    )?,
                    operation_id: new_id(),
                    run_id: opened.memory_run_id.clone(),
                    history_revision: boundary.history_revision,
                    entity_revision: boundary.entity_revision,
                    evidence_revision: boundary.evidence_revision,
                    policy_hash: run.policy_hash,
                    batch_id: None,
                    candidate_ids: vec![],
                    targets: vec![Target {
                        entry_id: extra_version.entry_id.clone(),
                        expected: Expected {
                            version_id: None,
                            state_revision: 0,
                        },
                        next: Some(extra_version),
                        state: EntryState::Active,
                        effects: vec![],
                    }],
                    processed_materials: vec![],
                    reason: "验证活动记忆容量上限".into(),
                };
                assert!(matches!(
                    repository.prepare(connection, port, &overflow_plan),
                    Err(aoidos_memory::error::Error::Rejected(
                        aoidos_memory::error::Reason::CapacityBlocked
                    ))
                ));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(
        count_active_memory_entries(&fixture.storage, &opened.memory_run_id),
        ENTRY_COUNT
    );
    let seed_ms = seed_started.elapsed().as_millis();

    fixture.storage.records.close(&opened.session_id).unwrap();
    fixture.close().await;
    let reopen_started = std::time::Instant::now();
    let reopened = Fixture::build_at(fixture.root.clone(), fixture.stress_provider.clone());
    let reopened_session = reopened.open(false).await.unwrap();
    assert_eq!(reopened_session.session_id, opened.session_id);
    assert_eq!(reopened_session.memory_run_id, opened.memory_run_id);
    assert_eq!(
        count_active_memory_entries(&reopened.storage, &opened.memory_run_id),
        ENTRY_COUNT
    );
    let reopen_validation_ms = reopen_started.elapsed().as_millis();

    let turns_started = std::time::Instant::now();
    for index in 0..ROUNDS_AFTER_REOPEN {
        complete_product_stress_round(
            &reopened,
            &opened.session_id,
            &format!("检查满负载后的第 {index} 条线索"),
            index,
        )
        .await;
    }
    let mut turns_ms = turns_started.elapsed().as_millis();

    {
        let session = reopened.storage.records.get(&opened.session_id).unwrap();
        session
            .lock()
            .unwrap()
            .append(crate::record::format::Body::PlayerSpeech {
                player_id: "player".into(),
                text: "合成长期容量记忆 00000 检索确认".into(),
                mode: Some(crate::record::format::InputMode::OutOfCharacter),
                content_range: None,
            })
            .unwrap();
    }
    let collected = reopened
        .storage
        .collect_memory_materials(&opened.memory_run_id, None)
        .unwrap();
    assert!(collected.eligible_count > 0, "查询输入应登记为可处理素材");
    let lookup_started = std::time::Instant::now();
    let target_search_catalog = reopened
        .storage
        .memory_catalog_for_run(&opened.memory_run_id)
        .unwrap()
        .1;
    let target_search_refused_for_body_budget = reopened
        .storage
        .with_memory_context(
            &opened.session_id,
            &target_search_catalog,
            |repository, connection, port, _, _| {
                let history_revision = port.boundary()?.history_revision;
                let (scanned_entries, distinct_subjects, body_bytes): (i64, i64, i64) = connection
                    .query_row(
                        "SELECT COUNT(*), COUNT(DISTINCT v.subject_json), SUM(v.body_length) \
                         FROM memory_entries e JOIN memory_versions v \
                           ON v.entry_id=e.entry_id AND v.version_id=e.current_version_id \
                         WHERE e.run_id=?1 AND e.status IN ('active','archived')",
                        [&opened.memory_run_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(|error| {
                        aoidos_memory::error::Error::from(aoidos_store::db::sqlite_error(error))
                    })?;
                assert_eq!(usize::try_from(scanned_entries).unwrap(), ENTRY_COUNT);
                assert_eq!(distinct_subjects, 1);
                assert!(
                    usize::try_from(body_bytes).unwrap() > MAX_PROPOSAL_TARGET_SEARCH_BYTES,
                    "同一主体的版本正文总量应超过目标检索预算"
                );
                Ok(matches!(
                    repository.entries_for_subjects(
                        connection,
                        port,
                        &opened.memory_run_id,
                        history_revision,
                        &[Subject::PlayerPreference],
                        aoidos_memory::model::MAX_PROPOSAL_TARGET_SCAN + 1,
                    ),
                    Err(aoidos_memory::error::Error::Rejected(
                        aoidos_memory::error::Reason::CapacityBlocked
                    ))
                ))
            },
        )
        .unwrap();
    assert!(
        target_search_refused_for_body_budget,
        "目标扫描应由版本正文总字节预算拒绝"
    );
    let lookup_error = reopened
        .storage
        .prepare_collectible_memory_batch(&opened.memory_run_id)
        .expect_err("超过目标正文扫描预算时应在准备阶段失败关闭");
    assert_eq!(lookup_error.code, "app.bad-request");
    let lookup_ms = lookup_started.elapsed().as_millis();
    assert!(
        reopened
            .storage
            .active_memory_batch_status(&opened.memory_run_id)
            .unwrap()
            .is_none(),
        "上下文超限不得冻结批次或占用素材"
    );

    let turns_after_refusal_started = std::time::Instant::now();
    for index in ROUNDS_AFTER_REOPEN..(ROUNDS_AFTER_REOPEN * 2) {
        complete_product_stress_round(
            &reopened,
            &opened.session_id,
            &format!("检查预算拒绝后的第 {index} 条线索"),
            index,
        )
        .await;
    }
    turns_ms += turns_after_refusal_started.elapsed().as_millis();
    let provider_starts = reopened
        .stress_provider
        .as_ref()
        .unwrap()
        .starts
        .load(Ordering::SeqCst);
    assert_eq!(provider_starts, ROUNDS_AFTER_REOPEN * 4);
    assert_eq!(
        count_active_memory_entries(&reopened.storage, &opened.memory_run_id),
        ENTRY_COUNT,
        "回合与失败关闭的记忆请求都不应改变已提交容量"
    );
    let sqlite_bytes = std::fs::metadata(reopened.root.join("storage.sqlite"))
        .unwrap()
        .len();
    let record_bytes = std::fs::metadata(
        reopened
            .root
            .join("workspaces/mistbell/transcript")
            .join(format!("{}.jsonl", opened.session_id)),
    )
    .unwrap()
    .len();
    let wal_bytes = match std::fs::metadata(reopened.root.join("storage.sqlite-wal")) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("读取 SQLite WAL 大小失败：{error}"),
    };
    let peak_rss = disk_metrics::peak_rss_bytes()
        .map(|bytes| bytes.to_string())
        .unwrap_or_else(|| "null".into());
    let files_after_reopen = disk_metrics::disk_summary(&reopened.root);
    println!(
        "{{\"activeEntries\":{ENTRY_COUNT},\"seedMs\":{seed_ms},\"reopenValidationMs\":{reopen_validation_ms},\"turnsAfterReopen\":{},\"turnsMs\":{turns_ms},\"targetBudgetRefused\":true,\"lookupMs\":{lookup_ms},\"providerStarts\":{provider_starts},\"recordBytes\":{record_bytes},\"sqliteBytes\":{sqlite_bytes},\"walBytes\":{wal_bytes},\"processPeakRssBytes\":{peak_rss},\"filesAfterReopen\":{files_after_reopen}}}",
        ROUNDS_AFTER_REOPEN * 2,
    );
    reopened.close().await;
}

async fn complete_product_stress_round(
    fixture: &Fixture,
    session_id: &str,
    text: &str,
    index: usize,
) {
    let accepted = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match fixture
                .service
                .submit_input(session_id, text.to_owned())
                .await
            {
                Ok(accepted) => break accepted,
                Err(error) if error.code == "app.busy" => {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
                Err(error) => panic!("第 {index} 个合成产品回合无法接纳：{error}"),
            }
        }
    })
    .await
    .expect("前一合成产品回合应及时释放运行门");
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let state = fixture.service.get_phase(session_id).unwrap().state;
            if let Some(operation) = state.last_operation
                && operation.operation_id == accepted.operation_id
            {
                match operation.outcome {
                    crate::game::state::OperationOutcome::Accepted => {}
                    crate::game::state::OperationOutcome::Completed
                        if state.phase == crate::game::state::Phase::Idle
                            && state.in_flight.is_none() =>
                    {
                        break;
                    }
                    crate::game::state::OperationOutcome::Completed => {}
                    outcome => panic!(
                        "第 {index} 个合成产品回合失败：{outcome:?}，{:?}",
                        operation.error
                    ),
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("合成产品回合应在有界时间内完成");
}

fn submit_product_stress_batch(
    storage: &Storage,
    session_id: &str,
    catalog: &crate::game::catalog::SceneCatalog,
    batch_id: &str,
    classify_ooc_as_new: bool,
) -> usize {
    let snapshot = storage
        .with_memory(session_id, catalog, |repository, connection, _| {
            repository.batch(connection, batch_id)
        })
        .unwrap()
        .0;
    let context = read_frozen_proposal_context(storage, session_id, catalog, batch_id);
    let mut added = 0;
    let proposal = Proposal {
        version: 1,
        candidates: snapshot
            .spans
            .iter()
            .enumerate()
            .map(|(index, span)| {
                let is_player_preference = classify_ooc_as_new
                    && span.source.mode == Some(MemoryInputMode::OutOfCharacter);
                if is_player_preference {
                    added += 1;
                }
                Candidate {
                    candidate_id: format!("product-stress-{index}"),
                    classification: if is_player_preference {
                        Classification::New
                    } else {
                        Classification::NotWorthy
                    },
                    source_ids: vec![span.source_id.clone()],
                    subject_ref: SubjectRef::PlayerPreference,
                    entity_ids: vec![],
                    evidence_ids: vec![],
                    target_ref: None,
                    dimension: is_player_preference.then_some(Dimension::PlayerPreference),
                    summary: is_player_preference.then(|| span.text.clone()),
                    attitude: None,
                    attitude_evidence_ids: vec![],
                    inferred: is_player_preference.then_some(false),
                    reason: "产品链路探索的确定性合成响应，不用于语义质量评估".into(),
                }
            })
            .collect(),
        cycle_marks: vec![],
    };
    proposal
        .validate(&context)
        .expect("合成产品提案应符合冻结上下文");
    let response = aoidos_json::canonical_string(&proposal).unwrap();
    let turn_id = new_id();
    let request_id = new_id();
    storage
        .with_memory(session_id, catalog, |repository, connection, port| {
            assert!(repository.dispatch_batch(
                connection,
                port,
                batch_id,
                &turn_id,
                &request_id,
            )?);
            repository.receive_batch(
                connection,
                batch_id,
                &turn_id,
                &request_id,
                response.as_bytes(),
            )?;
            Ok(())
        })
        .unwrap();
    assert!(
        crate::record::memory::processing::complete_received_batch(
            storage, session_id, catalog, batch_id,
        )
        .unwrap()
    );
    assert_eq!(
        storage
            .memory_batch_status(&snapshot.run_id, batch_id)
            .unwrap()
            .state,
        BatchState::Completed
    );
    added
}

fn read_frozen_proposal_context(
    storage: &Storage,
    session_id: &str,
    catalog: &crate::game::catalog::SceneCatalog,
    batch_id: &str,
) -> ProposalContext {
    storage
        .with_memory(session_id, catalog, |repository, connection, _| {
            let (snapshot, _) = repository.batch(connection, batch_id)?;
            aoidos_json::from_value(snapshot.input["proposalContext"].clone())
                .map_err(|_| aoidos_memory::error::Error::Corrupt)
        })
        .unwrap()
}

fn count_active_memory_entries(storage: &Storage, run_id: &str) -> usize {
    let mut cursor = None;
    let mut active = 0;
    loop {
        let page = storage.memory_entries(run_id, cursor, 32).unwrap();
        active += page
            .entries
            .iter()
            .filter(|entry| entry.state == aoidos_memory::model::EntryState::Active)
            .count();
        cursor = page.next_cursor;
        if cursor.is_none() {
            return active;
        }
    }
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
        fixture
            .factory
            .for_profile("default", None, None, None)
            .err()
            .unwrap()
            .code,
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
async fn failed_registration_releases_writers_and_mirror_failure_keeps_catalog_run_open() {
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
    // 冻结阶段阻塞后让兼容文件写入失败；SQLite 已登记的活动周目仍可正常使用。
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
    let opened = opening.await.unwrap().unwrap();
    assert!(opened.selection_mirror_pending);
    assert!(fixture.service.get_phase(&opened.session_id).is_ok());
    assert_eq!(
        fixture
            .storage
            .active_run_selection()
            .unwrap()
            .unwrap()
            .session_id,
        opened.session_id
    );
    let files = std::fs::read_dir(fixture.root.join("workspaces/mistbell/transcript"))
        .unwrap()
        .map(|file| file.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 2);
    for file in files {
        let id = file.file_stem().unwrap().to_str().unwrap();
        if id == opened.session_id {
            assert!(fixture.storage.records.get(id).is_ok());
            assert!(fixture.service.get_phase(id).is_ok());
        } else {
            assert_eq!(
                fixture.storage.records.get(id).err().unwrap().code,
                "app.not-found"
            );
            assert_eq!(
                fixture.service.get_phase(id).unwrap_err().code,
                "app.not-found"
            );
        }
    }
    assert!(fixture.factory.freeze().is_ok());
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
        opened(
            &Selection {
                script_id: "missing".into(),
                ..selected
            },
            "memory-run",
        )
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
        fixture
            .factory
            .for_profile("", None, None, None)
            .err()
            .unwrap()
            .code,
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
        factory
            .for_profile("default", None, None, None)
            .err()
            .unwrap()
            .code,
        "llm.proxy-error"
    );
    fixture.storage.close().unwrap();
}
