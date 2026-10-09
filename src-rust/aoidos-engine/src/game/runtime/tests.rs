use super::*;
use crate::{game::test_support::Fixture, record::facts::DiceMode};
use std::time::Duration;

struct Factory {
    generation: Arc<crate::game::test_support::TestGeneration>,
    mode: DiceMode,
}
impl RoundFactory for Factory {
    fn freeze(&self) -> Result<FrozenRound, Fault> {
        Ok(FrozenRound {
            profile_id: uuid::Uuid::new_v4().to_string(),
            profile_revision: "1".into(),
            dice_mode: self.mode,
            generation: self.generation.clone(),
        })
    }
}
fn service(fixture: &Fixture, mode: DiceMode) -> Service {
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(Factory {
            generation: fixture.generation.clone(),
            mode,
        }),
    );
    service.register(fixture.context.clone()).unwrap();
    service
}
async fn wait(service: &Service, id: &str, predicate: impl Fn(&PhaseState) -> bool) -> PhaseState {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = service.get_phase(id).unwrap();
            if predicate(&snapshot.state) {
                return snapshot.state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actor deadline")
}

// 终态快照先于阻塞提交回执发布；后续接纳须另等实际 lease 释放。
async fn wait_available(fixture: &Fixture) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::task::yield_now().await;
            if fixture.context.coordinator.check_available().is_ok() {
                return;
            }
        }
    })
    .await
    .expect("application gate deadline");
}

#[tokio::test]
async fn actor_manual_check_duplicate_and_shutdown_use_one_application_gate() {
    let fixture = Fixture::new(true, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "搜索".into()).await.unwrap();
    let state = wait(&service, &id, |state| state.check.is_some()).await;
    let plan = state.check.unwrap().plan_id;
    assert!(plan.starts_with("check-"));
    assert_eq!(
        fixture
            .context
            .coordinator
            .check_available()
            .unwrap_err()
            .code,
        "app.busy"
    );
    assert_eq!(
        service
            .submit_input(&id, "第二条".into())
            .await
            .unwrap_err()
            .code,
        "app.busy"
    );
    assert!(
        service
            .submit_check(&id, &accepted.round_id, &plan)
            .await
            .unwrap()
            .accepted
    );
    assert!(
        service
            .submit_check(&id, &accepted.round_id, &plan)
            .await
            .unwrap()
            .accepted
    );
    wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|operation| operation.outcome == OperationOutcome::Completed)
    })
    .await;
    assert_eq!(
        service
            .cancel_round(&id, &accepted.round_id)
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    wait_available(&fixture).await;
    let second = service.submit_input(&id, "再搜索".into()).await.unwrap();
    wait(&service, &id, |state| state.check.is_some()).await;
    assert_ne!(accepted.round_id, second.round_id);
    service.shutdown().await;
    assert!(fixture.context.coordinator.acquire().is_ok());
    assert_eq!(service.get_phase(&id).unwrap_err().code, "app.not-ready");
}

#[tokio::test]
async fn cancel_waiting_resume_and_regenerate_latest_resumed_round_preserve_original_dice() {
    let fixture = Fixture::new(true, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let first = service.submit_input(&id, "搜索".into()).await.unwrap();
    wait(&service, &id, |state| state.check.is_some()).await;
    assert_eq!(
        service
            .cancel_round(&id, &first.round_id)
            .await
            .unwrap()
            .outcome,
        Outcome::Cancelled
    );
    let resumed = service.resume(&id).await.unwrap();
    let plan = wait(&service, &id, |state| state.check.is_some())
        .await
        .check
        .unwrap()
        .plan_id;
    service
        .submit_check(&id, &resumed.round_id, &plan)
        .await
        .unwrap();
    wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|operation| operation.outcome == OperationOutcome::Completed)
    })
    .await;
    wait_available(&fixture).await;
    let regeneration = service.regenerate(&id, &resumed.round_id).await.unwrap();
    let state = wait(&service, &id, |state| {
        state.last_operation.as_ref().is_some_and(|operation| {
            operation.operation_id == regeneration.operation_id
                && operation.outcome == OperationOutcome::Completed
        })
    })
    .await;
    assert!(state.history_revision > 0);
    {
        let session = lock(&fixture.context.session);
        let path = session.path_at(session.history_revision()).unwrap();
        let count = path
            .into_iter()
            .filter(|seq| {
                matches!(
                    session.read(*seq).unwrap().body.unwrap().body,
                    format::Body::Dice { .. }
                )
            })
            .count();
        assert_eq!(count, 1);
    }
    service.shutdown().await;
}

#[tokio::test]
async fn malformed_unknown_and_wrong_coordinator_registration_are_rejected() {
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    let service = service(&fixture, DiceMode::Auto);
    let id = fixture.context.publisher.snapshot().state.session_id;
    assert!(service.register(fixture.context.clone()).is_err());
    assert_eq!(
        service.get_phase("bad").unwrap_err().code,
        "app.bad-request"
    );
    assert_eq!(
        service
            .submit_input(&id, " ".into())
            .await
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    assert_eq!(
        service
            .get_phase(&uuid::Uuid::new_v4().to_string())
            .unwrap_err()
            .code,
        "app.not-found"
    );
    assert!(service.resume(&id).await.is_err());
    let other = Service::new(
        crate::turn::Coordinator::new(Arc::new(crate::test_support::Events::default()), 16)
            .unwrap(),
        Arc::new(Factory {
            generation: fixture.generation.clone(),
            mode: DiceMode::Auto,
        }),
    );
    assert!(other.register(fixture.context.clone()).is_err());
    service.shutdown().await;
}

#[tokio::test]
async fn interrupt_waits_for_committed_prefix_keeps_parent_lease_and_accepts_a_new_manual_round() {
    let fixture = Fixture::new(true, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let first = service.submit_input(&id, "搜索".into()).await.unwrap();
    let plan = wait(&service, &id, |state| state.check.is_some())
        .await
        .check
        .unwrap()
        .plan_id;
    fixture.generation.hold.store(true, Ordering::SeqCst);
    service
        .submit_check(&id, &first.round_id, &plan)
        .await
        .unwrap();
    let active = wait(&service, &id, |state| {
        state
            .in_flight
            .as_ref()
            .and_then(|active| active.turn_id.as_ref())
            .is_some_and(|turn| {
                fixture
                    .context
                    .coordinator
                    .snapshot(turn)
                    .is_ok_and(|snapshot| !snapshot.text.is_empty())
            })
    })
    .await;
    let old_turn = active.in_flight.unwrap().turn_id.unwrap();
    let accepted = service
        .interrupt(&id, &first.round_id, "先退后一步".into())
        .await
        .unwrap();
    let next = wait(&service, &id, |state| state.check.is_some()).await;
    assert_eq!(
        next.in_flight.as_ref().unwrap().operation_id,
        accepted.operation_id
    );
    assert_ne!(
        next.in_flight.as_ref().unwrap().round_id.as_ref().unwrap(),
        &first.round_id
    );
    assert_eq!(
        fixture
            .context
            .coordinator
            .check_available()
            .unwrap_err()
            .code,
        "app.busy"
    );
    let old = fixture.context.coordinator.snapshot(&old_turn).unwrap();
    assert_eq!(old.outcome, Some(Outcome::Cancelled));
    assert!(!old.text.is_empty());
    assert_eq!(
        service
            .cancel_round(&id, &first.round_id)
            .await
            .unwrap()
            .outcome,
        Outcome::Cancelled
    );
    {
        let session = lock(&fixture.context.session);
        let path = session.path_at(session.history_revision()).unwrap();
        assert!(path.into_iter().any(|seq| matches!(session.read(seq).unwrap().body.unwrap().body,
            format::Body::Narration { text, terminal: crate::ports::Terminal::Cancelled, .. } if text == old.text)));
    }
    service.shutdown().await;
    assert!(fixture.context.coordinator.acquire().is_ok());
}

#[tokio::test]
async fn direct_public_cancel_also_seals_the_game_round_and_releases_the_gate() {
    let fixture = Fixture::new(true, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let first = service.submit_input(&id, "搜索".into()).await.unwrap();
    let plan = wait(&service, &id, |state| state.check.is_some())
        .await
        .check
        .unwrap()
        .plan_id;
    fixture.generation.hold.store(true, Ordering::SeqCst);
    service
        .submit_check(&id, &first.round_id, &plan)
        .await
        .unwrap();
    let active = wait(&service, &id, |state| {
        state
            .in_flight
            .as_ref()
            .and_then(|active| active.turn_id.as_ref())
            .is_some()
    })
    .await;
    fixture
        .context
        .coordinator
        .cancel(active.in_flight.unwrap().turn_id.as_ref().unwrap())
        .await
        .unwrap();
    wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|operation| operation.outcome == OperationOutcome::Cancelled)
    })
    .await;
    assert_eq!(
        service
            .cancel_round(&id, &first.round_id)
            .await
            .unwrap()
            .outcome,
        Outcome::Cancelled
    );
    service.shutdown().await;
    assert!(fixture.context.coordinator.acquire().is_ok());
}

struct FailureGeneration {
    fixture: Arc<crate::game::test_support::TestGeneration>,
    fail: CancellationToken,
}
struct FailureProvider {
    metadata: Arc<dyn aoidos_llm::provider::Provider>,
    fail: CancellationToken,
}
impl aoidos_llm::provider::Provider for FailureProvider {
    fn capabilities(
        &self,
        model: &str,
        mode: aoidos_llm::provider::RequestMode,
    ) -> aoidos_llm::provider::ProviderCapabilities {
        self.metadata.capabilities(model, mode)
    }
    fn start(
        &self,
        _: aoidos_llm::provider::ProviderRequest,
        _: CancellationToken,
    ) -> aoidos_llm::provider::StartFuture<'_> {
        use futures::{FutureExt, StreamExt};
        let fail = self.fail.clone();
        async move {
            Ok(futures::stream::once(async move {
                fail.cancelled().await;
                Err(aoidos_llm::error::ProviderError::Auth)
            })
            .boxed())
        }
        .boxed()
    }
}
impl crate::game::domain::Generation for FailureGeneration {
    fn shape(&self) -> crate::record::projection::Shape {
        self.fixture.shape()
    }
    fn projection_budget(&self) -> crate::record::projection::Budget {
        self.fixture.projection_budget()
    }
    fn prepare(
        &self,
        input: aoidos_llm::provider::ProviderInput,
        guard: aoidos_llm::guard::GuardSpec,
        automatic: bool,
    ) -> Result<crate::request::PreparedGeneration, Fault> {
        let mut request = self.fixture.prepare(input, guard, automatic)?;
        request.provider = Arc::new(FailureProvider {
            metadata: request.provider.clone(),
            fail: self.fail.clone(),
        });
        Ok(request)
    }
}
struct BlockingFactory {
    generation: Arc<dyn crate::game::domain::Generation>,
    entered: CancellationToken,
    release: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    calls: std::sync::atomic::AtomicUsize,
}
impl RoundFactory for BlockingFactory {
    fn freeze(&self) -> Result<FrozenRound, Fault> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
            self.entered.cancel();
            let (mutex, changed) = &*self.release;
            let mut released = mutex.lock().unwrap();
            while !*released {
                let (next, timeout) = changed
                    .wait_timeout(released, Duration::from_secs(5))
                    .unwrap();
                assert!(!timeout.timed_out(), "freeze barrier deadline");
                released = next;
            }
        }
        Ok(FrozenRound {
            profile_id: uuid::Uuid::new_v4().to_string(),
            profile_revision: "1".into(),
            dice_mode: DiceMode::Auto,
            generation: self.generation.clone(),
        })
    }
}
#[tokio::test]
async fn interrupt_losing_to_natural_failure_preserves_failed_round_and_accepts_no_new_input() {
    use crate::game::execution::test_lock as lock;
    let fixture = Fixture::new(false, "{}");
    let generation = Arc::new(FailureGeneration {
        fixture: fixture.generation.clone(),
        fail: CancellationToken::new(),
    });
    let entered = CancellationToken::new();
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(BlockingFactory {
            generation: generation.clone(),
            entered: entered.clone(),
            release: release.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    service.register(fixture.context.clone()).unwrap();
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "/ooc 解释".into()).await.unwrap();
    let active = wait(&service, &id, |state| {
        state
            .in_flight
            .as_ref()
            .is_some_and(|flight| flight.turn_id.is_some())
    })
    .await;
    let turn = active.in_flight.unwrap().turn_id.unwrap();
    let interrupt = service.interrupt(&id, &accepted.round_id, "新输入".into());
    let control = async {
        entered.cancelled().await;
        generation.fail.cancel();
        assert_eq!(
            fixture
                .context
                .coordinator
                .wait(&turn)
                .await
                .unwrap()
                .outcome,
            Some(Outcome::Failed)
        );
        let (mutex, changed) = &*release;
        *mutex.lock().unwrap() = true;
        changed.notify_all();
    };
    let (result, ()) = tokio::join!(interrupt, control);
    assert_eq!(result.unwrap_err().code, "engine.invalid-phase");
    let terminal = wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|operation| operation.outcome == OperationOutcome::Failed)
    })
    .await;
    assert_eq!(
        terminal.last_operation.unwrap().operation_id,
        accepted.operation_id
    );
    {
        let session = lock(&fixture.context.session);
        assert_eq!(
            session
                .path_at(session.history_revision())
                .unwrap()
                .into_iter()
                .filter(|seq| matches!(
                    session.read(*seq).unwrap().body.unwrap().body,
                    crate::record::format::Body::PlayerSpeech { .. }
                ))
                .count(),
            1
        );
    }
    service.shutdown().await;
}

#[tokio::test]
async fn terminal_delivery_failure_keeps_waiting_checkpoint_and_duplicate_cancel_result() {
    use std::sync::atomic::Ordering;
    let fixture = Fixture::new(true, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let round = service.submit_input(&id, "搜索".into()).await.unwrap();
    let waiting = wait(&service, &id, |state| state.check.is_some()).await;
    fixture.events.fail.store(true, Ordering::SeqCst);
    let first = service.cancel_round(&id, &round.round_id).await.unwrap();
    assert_eq!(first.outcome, Outcome::Cancelled);
    let next = service.get_phase(&id).unwrap().state;
    assert_eq!(next.phase, Phase::AwaitingCheck);
    assert_eq!(next.check, waiting.check);
    assert!(next.resume_required);
    assert!(!next.needs_recovery);
    assert_eq!(
        service
            .cancel_round(&id, &round.round_id)
            .await
            .unwrap()
            .outcome,
        first.outcome
    );
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
    service.shutdown().await;
}

#[tokio::test]
async fn rolling_delivery_failure_seals_without_rng_and_releases_the_application_gate() {
    use std::sync::atomic::Ordering;
    let fixture = Fixture::new(true, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "搜索".into()).await.unwrap();
    let plan = wait(&service, &id, |state| state.check.is_some())
        .await
        .check
        .unwrap()
        .plan_id;
    fixture.events.fail_rolling.store(true, Ordering::SeqCst);
    assert!(
        service
            .submit_check(&id, &accepted.round_id, &plan)
            .await
            .unwrap()
            .accepted
    );
    let terminal = wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|operation| operation.outcome == OperationOutcome::Failed)
    })
    .await;
    assert_eq!(terminal.phase, Phase::AwaitingCheck);
    assert_eq!(
        terminal.check.unwrap().status,
        crate::game::state::CheckStatus::Waiting
    );
    assert!(terminal.resume_required);
    assert!(!terminal.needs_recovery);
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
    wait_available(&fixture).await;
    fixture.events.fail_rolling.store(false, Ordering::SeqCst);
    service.resume(&id).await.unwrap();
    service.shutdown().await;
}

#[test]
fn registration_without_runtime_is_rejected_without_creating_an_actor() {
    let fixture = Fixture::new(false, "{}");
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(Factory {
            generation: fixture.generation.clone(),
            mode: DiceMode::Manual,
        }),
    );
    assert_eq!(
        service.register(fixture.context.clone()).unwrap_err().code,
        "app.not-ready"
    );
}

#[tokio::test]
async fn registry_capacity_duplicate_close_and_reopen_have_no_hidden_owners() {
    let fixtures = (0..17)
        .map(|_| Fixture::new(true, "{}"))
        .collect::<Vec<_>>();
    let service = Service::new(
        fixtures[0].context.coordinator.clone(),
        Arc::new(Factory {
            generation: fixtures[0].generation.clone(),
            mode: DiceMode::Manual,
        }),
    );
    for fixture in fixtures.iter().take(16) {
        let context = Arc::new(Context {
            coordinator: fixtures[0].context.coordinator.clone(),
            ..clone_context(&fixture.context)
        });
        service.register(context).unwrap();
    }
    assert_eq!(
        service
            .register(fixtures[0].context.clone())
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    let last = Arc::new(Context {
        coordinator: fixtures[0].context.coordinator.clone(),
        ..clone_context(&fixtures[16].context)
    });
    assert_eq!(service.register(last).unwrap_err().code, "app.busy");
    let id = fixtures[0].context.publisher.snapshot().state.session_id;
    service.close_session(&id).await.unwrap();
    assert_eq!(service.get_phase(&id).unwrap_err().code, "app.not-found");
    let fresh = fixtures[0].reopen().await;
    service.register(fresh).unwrap();
    service.shutdown().await;
    assert_eq!(service.get_phase(&id).unwrap_err().code, "app.not-ready");
}
fn clone_context(context: &Arc<Context>) -> Context {
    Context {
        player_id: context.player_id.clone(),
        actor_id: context.actor_id.clone(),
        session: context.session.clone(),
        domain: context.domain.clone(),
        publisher: context.publisher.clone(),
        coordinator: context.coordinator.clone(),
        completed: context.completed.clone(),
    }
}

#[tokio::test]
async fn overloaded_and_disconnected_command_mailboxes_reject_without_appending_input() {
    let fixture = Fixture::new(false, "{}");
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(Factory {
            generation: fixture.generation.clone(),
            mode: DiceMode::Manual,
        }),
    );
    let id = fixture.context.publisher.snapshot().state.session_id;
    let (commands, receiver) = mpsc::channel(1);
    let (tx, _rx) = oneshot::channel();
    assert!(commands.try_send(Command::Close(tx)).is_ok());
    lock(&service.sessions).insert(
        id.clone(),
        Arc::new(Handle {
            context: fixture.context.clone(),
            commands,
            exited: CancellationToken::new(),
            closing: Arc::new(AtomicBool::new(false)),
        }),
    );
    assert_eq!(
        service
            .submit_input(&id, "输入".into())
            .await
            .unwrap_err()
            .code,
        "app.busy"
    );
    drop(receiver);
    assert_eq!(
        service
            .submit_input(&id, "输入".into())
            .await
            .unwrap_err()
            .code,
        "app.not-ready"
    );
    let (tx, rx) = oneshot::channel::<Result<(), Fault>>();
    drop(tx);
    assert_eq!(receive(rx).await.unwrap_err().code, "app.not-ready");
    assert_eq!(
        crate::game::execution::test_lock(&fixture.context.session).next_sequence(),
        1
    );
    lock(&service.sessions).clear();
}

#[tokio::test]
async fn all_controls_reject_unknown_rounds_and_invalid_boundaries_without_writes() {
    let fixture = Fixture::new(true, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let unknown = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        service
            .interrupt(&id, &unknown, "输入".into())
            .await
            .unwrap_err()
            .code,
        "app.not-found"
    );
    assert_eq!(
        service.cancel_round(&id, &unknown).await.unwrap_err().code,
        "app.not-found"
    );
    assert_eq!(
        service
            .submit_check(&id, &unknown, "plan")
            .await
            .unwrap_err()
            .code,
        "app.not-found"
    );
    assert_eq!(
        service.regenerate(&id, &unknown).await.unwrap_err().code,
        "app.not-found"
    );
    assert_eq!(
        service
            .submit_check(&id, &unknown, "/")
            .await
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    for target in [0, crate::record::format::MAX_SEQ + 1] {
        assert_eq!(
            service.rewind(&id, target).await.unwrap_err().code,
            "app.bad-request"
        );
    }
    assert_eq!(
        service.resume(&id).await.unwrap_err().code,
        "engine.invalid-phase"
    );
    assert_eq!(
        service.rewind(&id, 1).await.unwrap_err().code,
        "app.bad-request"
    );
    let accepted = service.submit_input(&id, "搜索".into()).await.unwrap();
    wait(&service, &id, |state| state.check.is_some()).await;
    assert_eq!(service.resume(&id).await.unwrap_err().code, "app.busy");
    assert_eq!(service.rewind(&id, 1).await.unwrap_err().code, "app.busy");
    assert_eq!(
        service
            .regenerate(&id, &accepted.round_id)
            .await
            .unwrap_err()
            .code,
        "app.busy"
    );
    assert_eq!(
        service
            .interrupt(&id, &accepted.round_id, "新输入".into())
            .await
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    assert_eq!(
        service
            .submit_check(&id, &accepted.round_id, "old-plan")
            .await
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    service.shutdown().await;
}

#[tokio::test]
async fn registration_retains_confirmed_terminal_cancel_results_from_previous_owner() {
    for outcome in [Outcome::Completed, Outcome::Cancelled, Outcome::Failed] {
        let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
        let mut run = fixture.accept("查看", DiceMode::Auto).await;
        let round = run.round_id().to_owned();
        if outcome == Outcome::Completed {
            fixture.complete(&mut run).await;
        } else {
            run.finish(outcome, (outcome == Outcome::Failed).then(Fault::event))
                .await
                .unwrap();
        }
        drop(run);
        let service = service(&fixture, DiceMode::Manual);
        let id = fixture.context.publisher.snapshot().state.session_id;
        assert_eq!(
            service.cancel_round(&id, &round).await.unwrap().outcome,
            outcome
        );
        service.shutdown().await;
    }
}

#[tokio::test]
async fn rejected_close_and_disconnected_actor_commands_release_their_response_channels() {
    let (tx, rx) = oneshot::channel();
    Command::Close(tx).reject(Fault::event());
    rx.await.unwrap();
    let fixture = Fixture::new(false, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    lock(&service.sessions)
        .get(&id)
        .unwrap()
        .closing
        .store(true, Ordering::SeqCst);
    assert_eq!(
        service.close_session(&id).await.unwrap_err().code,
        "app.not-ready"
    );
    assert_eq!(service.get_phase(&id).unwrap_err().code, "app.not-ready");
    service.shutdown().await;
    assert_eq!(
        service.register(fixture.context.clone()).unwrap_err().code,
        "app.not-ready"
    );
}

#[tokio::test]
async fn shutdown_queued_during_interrupt_freeze_prevents_the_replacement_input() {
    use crate::game::execution::test_lock as lock;
    let fixture = Fixture::new(false, "{}");
    let generation = Arc::new(FailureGeneration {
        fixture: fixture.generation.clone(),
        fail: CancellationToken::new(),
    });
    let entered = CancellationToken::new();
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(BlockingFactory {
            generation,
            entered: entered.clone(),
            release: release.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    service.register(fixture.context.clone()).unwrap();
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "/ooc 解释".into()).await.unwrap();
    wait(&service, &id, |state| {
        state
            .in_flight
            .as_ref()
            .is_some_and(|flight| flight.turn_id.is_some())
    })
    .await;
    let handle = lock(&service.sessions).get(&id).unwrap().clone();
    let interrupt = service.interrupt(&id, &accepted.round_id, "替换输入".into());
    let closing = async {
        entered.cancelled().await;
        let release_freeze = async {
            while handle.commands.capacity() == 32 {
                tokio::task::yield_now().await;
            }
            let (mutex, changed) = &*release;
            *mutex.lock().unwrap() = true;
            changed.notify_all();
        };
        tokio::join!(service.shutdown(), release_freeze);
    };
    let (result, ()) = tokio::join!(interrupt, closing);
    assert_eq!(result.unwrap_err().code, "app.not-ready");
    let session = lock(&fixture.context.session);
    assert_eq!(
        session
            .path_at(session.history_revision())
            .unwrap()
            .iter()
            .filter(|seq| matches!(
                session.read(**seq).unwrap().body.unwrap().body,
                crate::record::format::Body::PlayerSpeech { .. }
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn active_controls_reject_foreign_round_and_shutdown_waits_for_the_public_child() {
    use std::sync::atomic::Ordering;
    let fixture = Fixture::new(false, "{}");
    fixture.generation.hold.store(true, Ordering::SeqCst);
    let service = service(&fixture, DiceMode::Auto);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "/ooc 解释".into()).await.unwrap();
    wait(&service, &id, |state| {
        state
            .in_flight
            .as_ref()
            .is_some_and(|flight| flight.turn_id.is_some())
    })
    .await;
    assert_eq!(
        service
            .cancel_round(&id, &uuid::Uuid::new_v4().to_string())
            .await
            .unwrap_err()
            .code,
        "app.not-found"
    );
    assert_eq!(
        service
            .submit_check(&id, &accepted.round_id, "plan")
            .await
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    assert_eq!(
        service
            .regenerate(&id, &accepted.round_id)
            .await
            .unwrap_err()
            .code,
        "app.busy"
    );
    service.shutdown().await;
    assert!(fixture.context.coordinator.acquire().is_ok());
    assert_eq!(
        fixture
            .context
            .publisher
            .snapshot()
            .state
            .last_operation
            .unwrap()
            .outcome,
        OperationOutcome::Cancelled
    );
}

#[tokio::test]
async fn shutdown_during_submission_or_regeneration_preflight_appends_no_new_facts() {
    use crate::game::execution::test_lock as lock;
    for regeneration in [false, true] {
        let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
        let old_round = if regeneration {
            let mut run = fixture.accept("查看", DiceMode::Auto).await;
            fixture.complete(&mut run).await;
            Some(run.round_id().to_owned())
        } else {
            None
        };
        let entered = CancellationToken::new();
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let generation = Arc::new(FailureGeneration {
            fixture: fixture.generation.clone(),
            fail: CancellationToken::new(),
        });
        let service = Service::new(
            fixture.context.coordinator.clone(),
            Arc::new(BlockingFactory {
                generation,
                entered: entered.clone(),
                release: release.clone(),
                calls: std::sync::atomic::AtomicUsize::new(1),
            }),
        );
        service.register(fixture.context.clone()).unwrap();
        let id = fixture.context.publisher.snapshot().state.session_id;
        let handle = lock(&service.sessions).get(&id).unwrap().clone();
        let before = lock(&fixture.context.session).next_sequence();
        let operation = async {
            if let Some(round) = old_round {
                service.regenerate(&id, &round).await
            } else {
                service.submit_input(&id, "新输入".into()).await
            }
        };
        let closing = async {
            entered.cancelled().await;
            let release_freeze = async {
                while handle.commands.capacity() == 32 {
                    tokio::task::yield_now().await;
                }
                let (mutex, changed) = &*release;
                *mutex.lock().unwrap() = true;
                changed.notify_all();
            };
            tokio::join!(service.shutdown(), release_freeze);
        };
        let (result, ()) = tokio::join!(operation, closing);
        assert_eq!(result.unwrap_err().code, "app.not-ready");
        assert_eq!(lock(&fixture.context.session).next_sequence(), before);
    }
}

#[tokio::test]
async fn rewinding_to_a_manual_plan_publishes_the_same_checkpoint_as_reopening() {
    let fixture = Fixture::new(true, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "搜索".into()).await.unwrap();
    wait(&service, &id, |state| state.check.is_some()).await;
    service.cancel_round(&id, &accepted.round_id).await.unwrap();
    let target = service
        .get_phase(&id)
        .unwrap()
        .state
        .checkpoint
        .unwrap()
        .through_seq;
    service.rewind(&id, target).await.unwrap();
    let live = service.get_phase(&id).unwrap().state;
    assert_eq!(live.phase, Phase::AwaitingCheck);
    assert!(live.resume_required);
    service.close_session(&id).await.unwrap();
    let opened = fixture.reopen().await;
    assert_eq!(live.phase, opened.publisher.snapshot().state.phase);
    assert_eq!(live.check, opened.publisher.snapshot().state.check);
    service.shutdown().await;
}

struct RejectingFactory {
    generation: Arc<crate::game::test_support::TestGeneration>,
    calls: std::sync::atomic::AtomicUsize,
    reject_first: bool,
}
impl RoundFactory for RejectingFactory {
    fn freeze(&self) -> Result<FrozenRound, Fault> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.reject_first || call > 0 {
            return Err(Fault::new("app.not-ready", "夹具凭据不可用"));
        }
        Factory {
            generation: self.generation.clone(),
            mode: DiceMode::Auto,
        }
        .freeze()
    }
}
#[tokio::test]
async fn request_preflight_and_shared_gate_failures_never_append_input() {
    let fixture = Fixture::new(false, "{}");
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(RejectingFactory {
            generation: fixture.generation.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
            reject_first: true,
        }),
    );
    service.register(fixture.context.clone()).unwrap();
    let id = fixture.context.publisher.snapshot().state.session_id;
    let lease = fixture.context.coordinator.acquire().unwrap();
    assert_eq!(
        service
            .submit_input(&id, "查看".into())
            .await
            .unwrap_err()
            .code,
        "app.busy"
    );
    drop(lease);
    assert_eq!(
        service
            .submit_input(&id, "查看".into())
            .await
            .unwrap_err()
            .code,
        "app.not-ready"
    );
    assert_eq!(
        crate::game::execution::test_lock(&fixture.context.session).next_sequence(),
        1
    );
    service.shutdown().await;
}
#[tokio::test]
async fn interrupt_preflight_failure_keeps_the_existing_private_child_until_explicit_cancel() {
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    fixture.generation.hold.store(true, Ordering::SeqCst);
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(RejectingFactory {
            generation: fixture.generation.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
            reject_first: false,
        }),
    );
    service.register(fixture.context.clone()).unwrap();
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "查看".into()).await.unwrap();
    while fixture.generation.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        service
            .interrupt(&id, &accepted.round_id, "插话".into())
            .await
            .unwrap_err()
            .code,
        "app.not-ready"
    );
    assert_eq!(
        fixture
            .context
            .coordinator
            .check_available()
            .unwrap_err()
            .code,
        "app.busy"
    );
    assert_eq!(
        service
            .cancel_round(&id, &accepted.round_id)
            .await
            .unwrap()
            .outcome,
        Outcome::Cancelled
    );
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 1);
    service.shutdown().await;
}
#[tokio::test]
async fn no_scene_or_quarantined_sessions_refuse_generation_before_profile_lookup() {
    for needs_recovery in [false, true] {
        let fixture = Fixture::new(false, "{}");
        let publisher = &fixture.context.publisher;
        if needs_recovery {
            publisher.quarantine(Fault::event());
        } else {
            let mut state = publisher.snapshot().state;
            state.scene = None;
            publisher
                .confirm(publisher.prepare(state, None).unwrap())
                .unwrap();
        }
        let service = service(&fixture, DiceMode::Auto);
        let id = publisher.snapshot().state.session_id;
        assert_eq!(
            service
                .submit_input(&id, "查看".into())
                .await
                .unwrap_err()
                .code,
            if needs_recovery {
                "engine.invalid-phase"
            } else {
                "engine.no-scene"
            }
        );
        assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
        service.shutdown().await;
    }
}

#[tokio::test]
async fn waiting_cancellation_prepare_failure_quarantines_and_releases_the_owner() {
    let fixture = Fixture::new(true, "{}");
    let mut run = fixture.accept("搜索", DiceMode::Manual).await;
    // 快照发布不代表提交回执已到达；等待真实 step 完成后验证等待态命令分支。
    run.step(CancellationToken::new()).await.unwrap();
    assert_eq!(run.stage(), Stage::Waiting);
    let round = run.round_id().to_owned();
    let before = lock(&fixture.context.session).next_sequence();
    let (_tx, rx) = mpsc::channel(1);
    let mut actor = actor_for_test(&fixture, Some(run), rx);
    fixture.events.fail_prepare.store(true, Ordering::SeqCst);
    let (reply, result) = oneshot::channel();
    assert!(!actor.command(Command::Cancel(round.clone(), reply)).await);
    assert_eq!(result.await.unwrap().unwrap_err().code, "app.event-failed");
    assert!(actor.runner.is_none());
    assert!(actor.terminal.is_empty());
    assert!(fixture.context.coordinator.check_available().is_ok());
    assert_eq!(lock(&fixture.context.session).next_sequence(), before);
    let snapshot = fixture.context.publisher.snapshot().state;
    assert!(snapshot.needs_recovery);
    assert!(!snapshot.resume_required);
    assert!(snapshot.checkpoint.is_none());
    let (reply, result) = oneshot::channel();
    assert!(!actor.command(Command::Cancel(round, reply)).await);
    assert_eq!(
        result.await.unwrap().unwrap_err().code,
        "engine.invalid-phase"
    );
    assert_eq!(lock(&fixture.context.session).next_sequence(), before);
}

#[tokio::test]
async fn completed_terminal_delivery_failure_does_not_append_a_second_round_end() {
    use crate::game::execution::test_lock as lock;
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    fixture.events.fail_done.store(true, Ordering::SeqCst);
    let service = service(&fixture, DiceMode::Auto);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "查看".into()).await.unwrap();
    wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|op| op.outcome == OperationOutcome::Completed)
    })
    .await;
    assert_eq!(
        service
            .cancel_round(&id, &accepted.round_id)
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    {
        let session = lock(&fixture.context.session);
        assert_eq!(session.path_at(session.history_revision()).unwrap().iter().filter(|seq|matches!(session.read(**seq).unwrap().body.unwrap().body,crate::record::format::Body::System {code,..} if code=="roundEnded")).count(),1);
    }
    assert_eq!(lock(&fixture.completed.0).len(), 1);
    service.shutdown().await;
}

fn actor_for_test(
    fixture: &Fixture,
    runner: Option<Runner>,
    receiver: mpsc::Receiver<Command>,
) -> Actor {
    Actor {
        context: fixture.context.clone(),
        factory: Arc::new(Factory {
            generation: fixture.generation.clone(),
            mode: DiceMode::Manual,
        }),
        receiver,
        runner,
        checked: VecDeque::new(),
        terminal: VecDeque::new(),
        exited: CancellationToken::new(),
        closing: Arc::new(AtomicBool::new(false)),
        app_closing: Arc::new(AtomicBool::new(false)),
    }
}
#[tokio::test]
async fn owner_exit_drains_queued_responses_and_waiting_lease_even_if_senders_disconnect() {
    let fixture = Fixture::new(true, "{}");
    let mut run = fixture.accept("搜索", DiceMode::Manual).await;
    run.step(CancellationToken::new()).await.unwrap();
    let (tx, rx) = mpsc::channel(4);
    let (close_tx, close_rx) = oneshot::channel();
    tx.send(Command::Close(close_tx)).await.ok().unwrap();
    let (submit_tx, submit_rx) = oneshot::channel();
    tx.send(Command::Submit("排队输入".into(), submit_tx))
        .await
        .ok()
        .unwrap();
    actor_for_test(&fixture, Some(run), rx).run().await;
    close_rx.await.unwrap();
    assert_eq!(submit_rx.await.unwrap().unwrap_err().code, "app.not-ready");
    assert!(fixture.context.coordinator.acquire().is_ok());
    drop(tx);
    let (tx, rx) = mpsc::channel(1);
    drop(tx);
    actor_for_test(&fixture, None, rx).run().await;
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let exited = CancellationToken::new();
    exited.cancel();
    let handle = Handle {
        context: fixture.context.clone(),
        commands: tx,
        exited,
        closing: Arc::new(AtomicBool::new(false)),
    };
    stop(&handle).await;
}
#[tokio::test]
async fn wrong_session_registration_and_unfinished_last_operation_are_not_cached_as_terminal() {
    let fixture = Fixture::new(false, "{}");
    let other = Fixture::new(false, "{}");
    let first_service = service(&fixture, DiceMode::Auto);
    let context = Arc::new(Context {
        session: other.context.session.clone(),
        ..clone_context(&fixture.context)
    });
    assert_eq!(
        first_service.register(context).unwrap_err().code,
        "app.bad-request"
    );
    first_service.shutdown().await;
    let fixture = Fixture::new(false, "{}");
    let run = fixture.accept("查看", DiceMode::Auto).await;
    let round = run.round_id().to_owned();
    drop(run);
    let service = service(&fixture, DiceMode::Auto);
    let id = fixture.context.publisher.snapshot().state.session_id;
    assert_eq!(
        service.cancel_round(&id, &round).await.unwrap_err().code,
        "engine.invalid-phase"
    );
    service.shutdown().await;
}
#[tokio::test]
async fn old_known_round_controls_cannot_mutate_the_new_waiting_round() {
    let fixture = Fixture::new(true, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let first = service.submit_input(&id, "搜索一".into()).await.unwrap();
    let old = wait(&service, &id, |state| state.check.is_some())
        .await
        .check
        .unwrap()
        .plan_id;
    service.cancel_round(&id, &first.round_id).await.unwrap();
    let second = service.submit_input(&id, "搜索二".into()).await.unwrap();
    wait(&service, &id, |state| state.check.is_some()).await;
    assert_eq!(
        service
            .interrupt(&id, &first.round_id, "过期插话".into())
            .await
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    assert_eq!(
        service
            .submit_check(&id, &first.round_id, &old)
            .await
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    assert_eq!(
        service
            .get_phase(&id)
            .unwrap()
            .state
            .in_flight
            .unwrap()
            .round_id,
        Some(second.round_id)
    );
    service.shutdown().await;
}

#[tokio::test]
async fn repeated_handoff_controls_are_busy_while_the_started_seal_finishes() {
    use crate::game::{execution::test_lock as lock, test_support::PrepareGate};
    let fixture = Fixture::new(true, "{}");
    fixture.generation.hold.store(true, Ordering::SeqCst);
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "/ooc 解释".into()).await.unwrap();
    while lock(&fixture.context.session).in_flight().is_none() {
        tokio::task::yield_now().await;
    }
    let gate = Arc::new(PrepareGate::default());
    *lock(&fixture.events.prepare_gate) = Some(gate.clone());
    let interrupt = service.interrupt(&id, &accepted.round_id, "新输入".into());
    let competing = async {
        gate.entered.cancelled().await;
        assert_eq!(
            service
                .cancel_round(&id, &accepted.round_id)
                .await
                .unwrap_err()
                .code,
            "app.busy"
        );
        assert_eq!(
            service
                .interrupt(&id, &accepted.round_id, "再次插话".into())
                .await
                .unwrap_err()
                .code,
            "app.busy"
        );
        gate.release();
    };
    let (accepted, ()) = tokio::join!(interrupt, competing);
    let accepted = accepted.unwrap();
    wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|operation| operation.operation_id == accepted.operation_id)
            && state.check.is_some()
    })
    .await;
    service.shutdown().await;
}

#[test]
fn concurrent_close_claims_have_exactly_one_owner() {
    let fixture = Fixture::new(false, "{}");
    let (commands, _rx) = mpsc::channel(1);
    let handle = Arc::new(Handle {
        context: fixture.context.clone(),
        commands,
        exited: CancellationToken::new(),
        closing: Arc::new(AtomicBool::new(false)),
    });
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let one = scope.spawn(|| {
            barrier.wait();
            handle.claim_close()
        });
        let two = scope.spawn(|| {
            barrier.wait();
            handle.claim_close()
        });
        [one.join().unwrap(), two.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .find_map(|result| result.as_ref().err())
            .unwrap()
            .code,
        "app.not-ready"
    );
}
#[tokio::test]
async fn malformed_round_id_rejection_precedes_all_control_dispatch() {
    let fixture = Fixture::new(false, "{}");
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    assert_eq!(
        service.cancel_round(&id, "bad").await.unwrap_err().code,
        "app.bad-request"
    );
    assert_eq!(
        service
            .interrupt(&id, "bad", "输入".into())
            .await
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    assert_eq!(
        service
            .submit_check(&id, "bad", "plan")
            .await
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    assert_eq!(
        service.regenerate(&id, "bad").await.unwrap_err().code,
        "app.bad-request"
    );
    service.shutdown().await;
}
#[tokio::test]
async fn lifecycle_closure_rejects_queued_admission_before_factory_lookup() {
    let fixture = Fixture::new(false, "{}");
    let (tx, rx) = mpsc::channel(2);
    let (reply, result) = oneshot::channel();
    tx.send(Command::Submit("排队输入".into(), reply))
        .await
        .ok()
        .unwrap();
    let (close, _closed) = oneshot::channel();
    tx.send(Command::Close(close)).await.ok().unwrap();
    let actor = actor_for_test(&fixture, None, rx);
    actor.closing.store(true, Ordering::SeqCst);
    actor.run().await;
    assert_eq!(result.await.unwrap().unwrap_err().code, "app.not-ready");
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn regenerating_an_older_terminal_is_rejected_without_more_requests() {
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    let service = service(&fixture, DiceMode::Auto);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let first = service.submit_input(&id, "查看一".into()).await.unwrap();
    wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|op| op.outcome == OperationOutcome::Completed)
    })
    .await;
    wait_available(&fixture).await;
    let second = service.submit_input(&id, "查看二".into()).await.unwrap();
    wait(&service, &id, |state| {
        state.last_operation.as_ref().is_some_and(|op| {
            op.operation_id == second.operation_id && op.outcome == OperationOutcome::Completed
        })
    })
    .await;
    wait_available(&fixture).await;
    let calls = fixture.generation.calls.load(Ordering::SeqCst);
    assert_eq!(
        service
            .regenerate(&id, &first.round_id)
            .await
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), calls);
    service.shutdown().await;
}

#[tokio::test]
async fn confirmation_after_waiting_publication_waits_for_the_commit_receipt() {
    use crate::game::{execution::test_lock as lock, test_support::PrepareGate};
    let fixture = Fixture::new(true, "{}");
    let gate = Arc::new(PrepareGate::default());
    *lock(&fixture.events.waiting_delivery_gate) = Some(gate.clone());
    let service = service(&fixture, DiceMode::Manual);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "搜索".into()).await.unwrap();
    gate.entered.cancelled().await;
    let plan = service.get_phase(&id).unwrap().state.check.unwrap().plan_id;
    let handle = lock(&service.sessions).get(&id).unwrap().clone();
    let (reply, mut result) = oneshot::channel();
    handle
        .commands
        .send(Command::Check(
            accepted.round_id.clone(),
            plan.clone(),
            reply,
        ))
        .await
        .ok()
        .unwrap();
    // FIFO 中的后续拒绝是屏障：证明确认已被 actor 收到，但固定提交尚未返回。
    assert_eq!(
        service
            .submit_input(&id, "重复输入".into())
            .await
            .unwrap_err()
            .code,
        "app.busy"
    );
    assert!(matches!(
        result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    gate.release();
    let confirmed = result.await.unwrap().unwrap();
    assert!(confirmed.accepted);
    assert_eq!(confirmed.plan_id, plan);
    wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|operation| operation.outcome == OperationOutcome::Completed)
    })
    .await;
    {
        let session = lock(&fixture.context.session);
        assert_eq!(
            session
                .path_at(session.history_revision())
                .unwrap()
                .iter()
                .filter(|seq| matches!(
                    session.read(**seq).unwrap().body.unwrap().body,
                    crate::record::format::Body::Dice { .. }
                ))
                .count(),
            1
        );
    }
    service.shutdown().await;
}

#[tokio::test]
async fn deferred_check_queue_is_bounded_and_respects_cancellation_and_commit_failure() {
    use crate::game::{execution::test_lock as lock, test_support::PrepareGate};
    for mode in [0, 1, 2, 3] {
        let fixture = Fixture::new(true, "{}");
        let gate = Arc::new(PrepareGate::default());
        *lock(&fixture.events.waiting_delivery_gate) = Some(gate.clone());
        let service = service(
            &fixture,
            if mode == 3 {
                DiceMode::Auto
            } else {
                DiceMode::Manual
            },
        );
        let id = fixture.context.publisher.snapshot().state.session_id;
        let accepted = service.submit_input(&id, "搜索".into()).await.unwrap();
        gate.entered.cancelled().await;
        let plan = service.get_phase(&id).unwrap().state.check.unwrap().plan_id;
        let handle = lock(&service.sessions).get(&id).unwrap().clone();
        let mut replies = Vec::new();
        for i in 0..33 {
            let (tx, rx) = oneshot::channel();
            handle
                .commands
                .send(Command::Check(
                    accepted.round_id.clone(),
                    if i == 0 {
                        "wrong-plan".into()
                    } else {
                        plan.clone()
                    },
                    tx,
                ))
                .await
                .ok()
                .unwrap();
            replies.push(rx);
        }
        assert_eq!(
            replies.pop().unwrap().await.unwrap().unwrap_err().code,
            "app.busy"
        );
        let cancel = if mode == 1 {
            let (tx, rx) = oneshot::channel();
            handle
                .commands
                .send(Command::Cancel(accepted.round_id.clone(), tx))
                .await
                .ok()
                .unwrap();
            Some(rx)
        } else {
            None
        };
        assert_eq!(
            service
                .submit_input(&id, "屏障输入".into())
                .await
                .unwrap_err()
                .code,
            "app.busy"
        );
        if mode == 2 {
            fixture.events.fail.store(true, Ordering::SeqCst);
        }
        gate.release();
        let bad = replies.remove(0).await.unwrap().unwrap_err();
        assert_eq!(
            bad.code,
            match mode {
                1 => "app.busy",
                2 => "app.event-failed",
                _ => "app.bad-request",
            }
        );
        for reply in replies {
            let result = reply.await.unwrap();
            if matches!(mode, 0 | 3) {
                assert!(result.unwrap().accepted);
            } else {
                assert!(result.is_err());
            }
        }
        if let Some(cancel) = cancel {
            assert_eq!(cancel.await.unwrap().unwrap().outcome, Outcome::Cancelled);
        }
        if mode == 2 {
            wait(&service, &id, |state| {
                state
                    .last_operation
                    .as_ref()
                    .is_some_and(|op| op.outcome == OperationOutcome::Failed)
            })
            .await;
        } else if mode != 1 {
            wait(&service, &id, |state| {
                state
                    .last_operation
                    .as_ref()
                    .is_some_and(|op| op.outcome == OperationOutcome::Completed)
            })
            .await;
        }
        service.shutdown().await;
    }
}

#[tokio::test]
async fn interrupt_losing_to_natural_completion_keeps_the_original_round() {
    use crate::game::{execution::test_lock as lock, test_support::PrepareGate};
    let fixture = Fixture::new(true, "{}");
    let gate = Arc::new(PrepareGate::default());
    *lock(&fixture.events.seal_prepare_gate) = Some(gate.clone());
    let entered = CancellationToken::new();
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(BlockingFactory {
            generation: fixture.generation.clone(),
            entered: entered.clone(),
            release: release.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    service.register(fixture.context.clone()).unwrap();
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "/ooc 解释".into()).await.unwrap();
    gate.entered.cancelled().await;
    let turn = service
        .get_phase(&id)
        .unwrap()
        .state
        .in_flight
        .unwrap()
        .turn_id
        .unwrap();
    let interrupt = service.interrupt(&id, &accepted.round_id, "竞争输入".into());
    let control = async {
        entered.cancelled().await;
        gate.release();
        assert_eq!(
            fixture
                .context
                .coordinator
                .wait(&turn)
                .await
                .unwrap()
                .outcome,
            Some(Outcome::Completed)
        );
        let (mutex, changed) = &*release;
        *mutex.lock().unwrap() = true;
        changed.notify_all();
    };
    let (result, ()) = tokio::join!(interrupt, control);
    assert_eq!(result.unwrap_err().code, "engine.invalid-phase");
    wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|operation| operation.outcome == OperationOutcome::Completed)
    })
    .await;
    {
        let session = lock(&fixture.context.session);
        assert_eq!(
            session
                .path_at(session.history_revision())
                .unwrap()
                .iter()
                .filter(|seq| matches!(
                    session.read(**seq).unwrap().body.unwrap().body,
                    crate::record::format::Body::PlayerSpeech { .. }
                ))
                .count(),
            1
        );
    }
    service.shutdown().await;
}

#[tokio::test]
async fn regeneration_prepare_failure_preserves_the_confirmed_history_and_starts_no_child() {
    use crate::game::execution::test_lock as lock;
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    let service = service(&fixture, DiceMode::Auto);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "查看".into()).await.unwrap();
    wait(&service, &id, |state| {
        state
            .last_operation
            .as_ref()
            .is_some_and(|op| op.outcome == OperationOutcome::Completed)
    })
    .await;
    wait_available(&fixture).await;
    let before = lock(&fixture.context.session).next_sequence();
    let calls = fixture.generation.calls.load(Ordering::SeqCst);
    fixture.events.fail_prepare.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .regenerate(&id, &accepted.round_id)
            .await
            .unwrap_err()
            .code,
        "app.event-failed"
    );
    assert_eq!(lock(&fixture.context.session).next_sequence(), before);
    assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), calls);
    fixture.events.fail_prepare.store(false, Ordering::SeqCst);
    service.shutdown().await;
}

#[tokio::test]
async fn controls_queued_during_handoff_freeze_keep_identity_and_busy_priority() {
    use crate::game::execution::test_lock as lock;
    let fixture = Fixture::new(true, "{}");
    let mut previous = fixture.accept("/ooc 原回合", DiceMode::Manual).await;
    fixture.complete(&mut previous).await;
    let previous_round = previous.round_id().to_owned();
    drop(previous);
    fixture.generation.hold.store(true, Ordering::SeqCst);
    let entered = CancellationToken::new();
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(BlockingFactory {
            generation: fixture.generation.clone(),
            entered: entered.clone(),
            release: release.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    service.register(fixture.context.clone()).unwrap();
    let id = fixture.context.publisher.snapshot().state.session_id;
    let active = service
        .submit_input(&id, "/ooc 当前回合".into())
        .await
        .unwrap();
    while lock(&fixture.context.session).in_flight().is_none() {
        tokio::task::yield_now().await;
    }
    let handle = lock(&service.sessions).get(&id).unwrap().clone();
    let interrupt = service.interrupt(&id, &active.round_id, "新的行动".into());
    let control = async {
        entered.cancelled().await;
        let (old_tx, old_rx) = oneshot::channel();
        handle
            .commands
            .send(Command::Cancel(previous_round, old_tx))
            .await
            .ok()
            .unwrap();
        let (unknown_tx, unknown_rx) = oneshot::channel();
        handle
            .commands
            .send(Command::Cancel(
                uuid::Uuid::new_v4().to_string(),
                unknown_tx,
            ))
            .await
            .ok()
            .unwrap();
        let (check_tx, check_rx) = oneshot::channel();
        handle
            .commands
            .send(Command::Check(
                active.round_id.clone(),
                "plan".into(),
                check_tx,
            ))
            .await
            .ok()
            .unwrap();
        let (submit_tx, submit_rx) = oneshot::channel();
        handle
            .commands
            .send(Command::Submit("重复行动".into(), submit_tx))
            .await
            .ok()
            .unwrap();
        let (mutex, changed) = &*release;
        *mutex.lock().unwrap() = true;
        changed.notify_all();
        assert_eq!(old_rx.await.unwrap().unwrap().outcome, Outcome::Completed);
        assert_eq!(unknown_rx.await.unwrap().unwrap_err().code, "app.not-found");
        assert_eq!(
            check_rx.await.unwrap().unwrap_err().code,
            "engine.invalid-phase"
        );
        assert_eq!(submit_rx.await.unwrap().unwrap_err().code, "app.busy");
    };
    let (result, ()) = tokio::join!(interrupt, control);
    assert!(result.is_ok());
    service.shutdown().await;
}

#[tokio::test]
async fn active_seal_failure_or_replacement_prepare_failure_never_accepts_a_new_input() {
    use crate::game::execution::test_lock as lock;
    for mode in [0, 1, 2] {
        let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
        fixture.generation.hold.store(true, Ordering::SeqCst);
        let service = service(&fixture, DiceMode::Manual);
        let id = fixture.context.publisher.snapshot().state.session_id;
        let accepted = service
            .submit_input(&id, if mode == 0 { "检查" } else { "/ooc 解释" }.into())
            .await
            .unwrap();
        if mode == 0 {
            while fixture.generation.calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        } else {
            while lock(&fixture.context.session).in_flight().is_none() {
                tokio::task::yield_now().await;
            }
        }
        if mode == 2 {
            fixture
                .events
                .fail_after_cancelled
                .store(true, Ordering::SeqCst);
        } else {
            fixture.events.fail_prepare.store(true, Ordering::SeqCst);
        }
        let error = if mode == 0 {
            service
                .cancel_round(&id, &accepted.round_id)
                .await
                .unwrap_err()
        } else {
            service
                .interrupt(&id, &accepted.round_id, "替换".into())
                .await
                .unwrap_err()
        };
        assert_eq!(error.code, "app.event-failed");
        let state = service.get_phase(&id).unwrap().state;
        assert_eq!(state.needs_recovery, mode != 2);
        {
            let session = lock(&fixture.context.session);
            assert_eq!(
                session
                    .path_at(session.history_revision())
                    .unwrap()
                    .iter()
                    .filter(|seq| matches!(
                        session.read(**seq).unwrap().body.unwrap().body,
                        crate::record::format::Body::PlayerSpeech { .. }
                    ))
                    .count(),
                1
            );
        }
        assert_eq!(fixture.generation.calls.load(Ordering::SeqCst), 1);
        fixture.events.fail_prepare.store(false, Ordering::SeqCst);
        fixture
            .events
            .fail_after_cancelled
            .store(false, Ordering::SeqCst);
        service.shutdown().await;
    }
}

#[tokio::test]
async fn cancellation_during_the_started_final_commit_returns_completed_once() {
    use crate::game::{execution::test_lock as lock, test_support::PrepareGate};
    let fixture = Fixture::new(false, "{\"kind\":\"noCheck\"}");
    let gate = Arc::new(PrepareGate::default());
    *lock(&fixture.events.ending_prepare_gate) = Some(gate.clone());
    let service = service(&fixture, DiceMode::Auto);
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "查看".into()).await.unwrap();
    gate.entered.cancelled().await;
    let handle = lock(&service.sessions).get(&id).unwrap().clone();
    let (reply, result) = oneshot::channel();
    handle
        .commands
        .send(Command::Cancel(accepted.round_id.clone(), reply))
        .await
        .ok()
        .unwrap();
    assert_eq!(
        service
            .submit_input(&id, "屏障输入".into())
            .await
            .unwrap_err()
            .code,
        "app.busy"
    );
    gate.release();
    assert_eq!(result.await.unwrap().unwrap().outcome, Outcome::Completed);
    assert_eq!(
        service
            .cancel_round(&id, &accepted.round_id)
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    assert_eq!(lock(&fixture.completed.0).len(), 1);
    service.shutdown().await;
}

#[tokio::test]
async fn shutdown_with_a_full_handoff_queue_cannot_admit_the_replacement() {
    use crate::game::execution::test_lock as lock;
    let fixture = Fixture::new(false, "{}");
    fixture.generation.hold.store(true, Ordering::SeqCst);
    let entered = CancellationToken::new();
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(BlockingFactory {
            generation: fixture.generation.clone(),
            entered: entered.clone(),
            release: release.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    service.register(fixture.context.clone()).unwrap();
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "/ooc 解释".into()).await.unwrap();
    while lock(&fixture.context.session).in_flight().is_none() {
        tokio::task::yield_now().await;
    }
    let handle = lock(&service.sessions).get(&id).unwrap().clone();
    let interrupt = service.interrupt(&id, &accepted.round_id, "替换输入".into());
    let control = async {
        entered.cancelled().await;
        for _ in 0..32 {
            let (reply, _rx) = oneshot::channel();
            handle
                .commands
                .try_send(Command::Submit("排队输入".into(), reply))
                .ok()
                .unwrap();
        }
        let release_freeze = async {
            while !service.closing.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
            let (mutex, changed) = &*release;
            *mutex.lock().unwrap() = true;
            changed.notify_all();
        };
        tokio::join!(service.shutdown(), release_freeze);
    };
    let (result, ()) = tokio::join!(interrupt, control);
    assert_eq!(result.unwrap_err().code, "app.not-ready");
    let session = lock(&fixture.context.session);
    assert_eq!(
        session
            .path_at(session.history_revision())
            .unwrap()
            .iter()
            .filter(|seq| matches!(
                session.read(**seq).unwrap().body.unwrap().body,
                crate::record::format::Body::PlayerSpeech { .. }
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn closing_claim_before_the_close_message_still_prevents_handoff() {
    use crate::game::execution::test_lock as lock;
    let fixture = Fixture::new(false, "{}");
    fixture.generation.hold.store(true, Ordering::SeqCst);
    let entered = CancellationToken::new();
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let service = Service::new(
        fixture.context.coordinator.clone(),
        Arc::new(BlockingFactory {
            generation: fixture.generation.clone(),
            entered: entered.clone(),
            release: release.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    service.register(fixture.context.clone()).unwrap();
    let id = fixture.context.publisher.snapshot().state.session_id;
    let accepted = service.submit_input(&id, "/ooc 解释".into()).await.unwrap();
    while lock(&fixture.context.session).in_flight().is_none() {
        tokio::task::yield_now().await;
    }
    let handle = lock(&service.sessions).get(&id).unwrap().clone();
    let interrupt = service.interrupt(&id, &accepted.round_id, "替换输入".into());
    let control = async {
        entered.cancelled().await;
        handle.claim_close().unwrap();
        let (mutex, changed) = &*release;
        *mutex.lock().unwrap() = true;
        changed.notify_all();
    };
    let (result, ()) = tokio::join!(interrupt, control);
    assert_eq!(result.unwrap_err().code, "app.not-ready");
    service.shutdown().await;
}

#[test]
fn bounded_control_caches_evict_oldest_and_keep_checked_rounds_known() {
    let fixture = Fixture::new(false, "{}");
    let (_, rx) = mpsc::channel(1);
    let mut actor = actor_for_test(&fixture, None, rx);
    for index in 0..17 {
        let round = format!("round-{index}");
        push_bounded(&mut actor.checked, (round.clone(), format!("plan-{index}")));
        push_bounded(&mut actor.terminal, (round, Outcome::Completed));
    }
    assert_eq!(actor.checked.len(), 16);
    assert_eq!(actor.terminal.len(), 16);
    assert_eq!(actor.checked.front().unwrap().0, "round-1");
    assert_eq!(actor.terminal.front().unwrap().0, "round-1");
    assert!(!actor.known("round-0", None));
    assert!(actor.known("round-16", None));
    actor.terminal.clear();
    assert!(actor.known("round-16", None));
    assert!(!actor.known("unknown", None));
    assert!(actor.known("current", Some("current")));
}
