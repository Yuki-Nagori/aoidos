use super::*;
use crate::ports::MemoryWriter;
use futures::{FutureExt, StreamExt, future::BoxFuture, stream};
use mythos_llm::guard::{Anchor, GuardRule, GuardSpec};
use mythos_llm::provider::*;
use mythos_llm::schedule::{
    AllowAll, AttemptIdentity, AttemptOutcome, BudgetDenial, BudgetPort, GenerationRequest,
    RunPolicy,
};
use mythos_llm::{error::ProviderError, sampling::Sampling};
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

#[derive(Default)]
struct Events {
    seqs: Mutex<HashMap<(String, String), u64>>,
    sent: Mutex<Vec<(u64, TurnEvent)>>,
    retired: Mutex<Vec<String>>,
    prepare_calls: AtomicUsize,
    fail_prepare: AtomicUsize,
    invalid_seq: AtomicBool,
    lose_delivery: AtomicBool,
}
impl EventPort for Events {
    fn prepare(&self, event: TurnEvent) -> Result<PreparedEvent, Fault> {
        let call = self.prepare_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_prepare.load(Ordering::SeqCst) == call {
            return Err(Fault::event());
        }
        let mut seqs = lock(&self.seqs);
        let seq = seqs
            .entry((event.name().into(), event.turn_id().into()))
            .or_default();
        *seq += 1;
        let seq = if self.invalid_seq.load(Ordering::SeqCst) {
            0
        } else {
            *seq
        };
        let envelope = serde_json::json!({"seq": seq, "data": &event});
        Ok(PreparedEvent {
            seq,
            event,
            envelope,
        })
    }
    fn deliver(&self, event: PreparedEvent) -> Result<(), Fault> {
        if self.lose_delivery.load(Ordering::SeqCst) {
            return Err(Fault::event());
        }
        lock(&self.sent).push((event.seq, event.event));
        Ok(())
    }
    fn retire(&self, id: &str) {
        lock(&self.seqs).retain(|(_, turn), _| turn != id);
        lock(&self.retired).push(id.into());
    }
}

struct Fake {
    deltas: Vec<Result<ProviderDelta, ProviderError>>,
    hold: bool,
    calls: Arc<AtomicUsize>,
}
impl Provider for Fake {
    fn capabilities(&self, model: &str, _: RequestMode) -> ProviderCapabilities {
        ProviderCapabilities {
            model: model.into(),
            completion: model == "m",
            chat: model == "m",
            prefix: model == "m",
            thinking: false,
            stop_limit: 2,
            max_output_tokens: 4096,
            temperature_effective: true,
            temperature_min: 0.0,
            temperature_max: 2.0,
        }
    }
    fn start(&self, _: ProviderRequest, _: CancellationToken) -> StartFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let stream = stream::iter(self.deltas.clone());
        let stream = if self.hold {
            stream.chain(stream::pending()).boxed()
        } else {
            stream.boxed()
        };
        async move { Ok(stream) }.boxed()
    }
}
fn generation(
    deltas: Vec<Result<ProviderDelta, ProviderError>>,
    hold: bool,
) -> (PreparedGeneration, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(Fake {
        deltas,
        hold,
        calls: calls.clone(),
    });
    let request = GenerationRequest {
        provider_request: ProviderRequest {
            model: "m".into(),
            input: ProviderInput::Completion(CompletionInput { prompt: "P".into() }),
            sampling: Sampling {
                temperature: 1.0,
                max_tokens: 64,
            },
            stops: vec![],
        },
        guard: GuardSpec::default(),
    };
    let mut policy = RunPolicy::default();
    policy.ladder.clear();
    policy.transport_retries = 0;
    (
        PreparedGeneration::new(provider, request, policy, Arc::new(AllowAll)).unwrap(),
        calls,
    )
}
fn completed(text: &str) -> PreparedGeneration {
    generation(
        vec![
            Ok(ProviderDelta::Text(text.into())),
            Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
        ],
        false,
    )
    .0
}
fn coordinator(capacity: usize) -> (Coordinator, Arc<Events>) {
    let events = Arc::new(Events::default());
    (Coordinator::new(events.clone(), capacity).unwrap(), events)
}
async fn exited(coordinator: &Coordinator, id: &str) -> TurnSnapshot {
    coordinator.wait(id).await.unwrap()
}

#[tokio::test]
async fn accepted_snapshot_completed_event_and_bounded_retirement() {
    let (c, events) = coordinator(1);
    let id = c.submit(completed("中文"), Arc::new(MemoryWriter)).unwrap();
    assert!(Uuid::parse_str(&id).is_ok());
    assert_eq!(
        serde_json::to_value(c.snapshot(&id).unwrap()).unwrap(),
        serde_json::json!({"turnId":id,"text":"","seq":{"chunk":0,"done":0,"failed":0}})
    );
    assert_eq!(
        c.submit(completed("busy"), Arc::new(MemoryWriter))
            .unwrap_err()
            .code,
        "app.busy"
    );
    let result = exited(&c, &id).await;
    assert_eq!(result.text, "中文");
    assert_eq!(result.outcome, Some(Outcome::Completed));
    assert_eq!(result.finish_reason, Some(FinishReason::Stop));
    assert_eq!(
        result.seq,
        Sequences {
            chunk: 1,
            done: 1,
            failed: 0
        }
    );
    assert_eq!(c.cancel(&id).await.unwrap().outcome, Outcome::Completed);
    assert_eq!(lock(&events.sent).len(), 2);
    let done = serde_json::to_value(&lock(&events.sent)[1].1).unwrap();
    assert_eq!(done["chunkSeq"], 1);
    assert_eq!(done["finishReason"], "stop");
    let next = c.submit(completed("next"), Arc::new(MemoryWriter)).unwrap();
    assert_ne!(id, next);
    exited(&c, &next).await;
    assert_eq!(c.snapshot(&id).unwrap_err().code, "app.not-found");
    assert_eq!(c.cancel(&id).await.unwrap_err().code, "app.not-found");
    assert_eq!(*lock(&events.retired), vec![id.clone()]);
    assert!(lock(&events.seqs).keys().all(|(_, turn)| turn != &id));
}

struct Writer {
    text: Mutex<String>,
    fail_append: bool,
    fail_finish: bool,
    slow: bool,
    started: Notify,
    released: Notify,
}
impl Writer {
    fn new(fail_append: bool, fail_finish: bool, slow: bool) -> Self {
        Self {
            text: Mutex::new(String::new()),
            fail_append,
            fail_finish,
            slow,
            started: Notify::new(),
            released: Notify::new(),
        }
    }
}
impl OutputWriter for Writer {
    fn append<'a>(&'a self, _: &'a str, text: &'a str) -> BoxFuture<'a, Result<(), Fault>> {
        async move {
            self.started.notify_one();
            if self.slow {
                self.released.notified().await;
            }
            if self.fail_append {
                return Err(Fault::new("store.permission", "没有操作权限"));
            }
            lock(&self.text).push_str(text);
            Ok(())
        }
        .boxed()
    }
    fn finish<'a>(&'a self, _: &'a str, _: &'a Terminal) -> BoxFuture<'a, Result<(), Fault>> {
        async move {
            if self.fail_finish {
                Err(Fault::new("store.io", "存储读写失败"))
            } else {
                Ok(())
            }
        }
        .boxed()
    }
}

#[tokio::test]
async fn cancellation_waits_for_started_commit_and_does_not_expose_reservation() {
    let (c, events) = coordinator(16);
    let writer = Arc::new(Writer::new(false, false, true));
    let request = generation(vec![Ok(ProviderDelta::Text("前文".into()))], true).0;
    let id = c.submit(request, writer.clone()).unwrap();
    writer.started.notified().await;
    let snapshot = c.snapshot(&id).unwrap();
    assert!(snapshot.text.is_empty());
    assert_eq!(snapshot.seq.chunk, 0);
    assert!(lock(&events.sent).is_empty());
    let cancelling = {
        let c = c.clone();
        let id = id.clone();
        tokio::spawn(async move { c.cancel(&id).await })
    };
    tokio::task::yield_now().await;
    assert!(!cancelling.is_finished());
    writer.released.notify_one();
    assert_eq!(
        cancelling.await.unwrap().unwrap().outcome,
        Outcome::Cancelled
    );
    let snapshot = exited(&c, &id).await;
    assert_eq!(snapshot.text, "前文");
    assert_eq!(*lock(&writer.text), snapshot.text);
    assert!(snapshot.finish_reason.is_none());
    assert!(snapshot.error.is_none());
    assert_eq!(
        serde_json::to_value(&lock(&events.sent)[1].1).unwrap(),
        serde_json::json!({"turnId":id,"outcome":"cancelled","chunkSeq":1})
    );
    assert!(c.acquire().is_ok());
}

#[tokio::test]
async fn output_and_terminal_failures_confirm_discarded_sequences_without_false_success() {
    for (fail_append, fail_finish) in [(true, false), (false, true), (true, true)] {
        let (c, events) = coordinator(16);
        let writer = Arc::new(Writer::new(fail_append, fail_finish, false));
        let id = c.submit(completed("text"), writer.clone()).unwrap();
        let s = exited(&c, &id).await;
        assert_eq!(s.outcome, Some(Outcome::Failed));
        assert!(s.finish_reason.is_none());
        assert_eq!(s.text, *lock(&writer.text));
        assert_eq!(s.seq.chunk, 1);
        let sent = lock(&events.sent);
        assert!(
            sent.iter()
                .all(|(_, e)| !matches!(e, TurnEvent::Done { .. }))
        );
        let failed = serde_json::to_value(&sent.last().unwrap().1).unwrap();
        assert_eq!(failed["chunkSeq"], 1);
        assert_eq!(failed["code"], s.error.unwrap().code);
        assert!(c.acquire().is_ok());
    }
}

#[tokio::test]
async fn prepare_failure_never_writes_delta_and_terminal_prepare_failure_releases_gate() {
    for failing_call in [1, 2] {
        let (c, events) = coordinator(16);
        events.fail_prepare.store(failing_call, Ordering::SeqCst);
        let writer = Arc::new(Writer::new(false, false, false));
        let id = c.submit(completed("text"), writer.clone()).unwrap();
        let snapshot = exited(&c, &id).await;
        assert_eq!(snapshot.outcome, Some(Outcome::Failed));
        assert_eq!(snapshot.error.unwrap().code, "app.event-failed");
        assert_eq!(snapshot.text, *lock(&writer.text));
        if failing_call == 1 {
            assert!(snapshot.text.is_empty());
            assert_eq!(snapshot.seq.chunk, 0);
        }
        assert!(c.acquire().is_ok());
    }
    let (c, events) = coordinator(1);
    events.invalid_seq.store(true, Ordering::SeqCst);
    let id = c.submit(completed("text"), Arc::new(MemoryWriter)).unwrap();
    assert_eq!(
        exited(&c, &id).await.error.unwrap().code,
        "app.event-failed"
    );
}

#[tokio::test]
async fn lost_last_events_keep_final_snapshot_for_explicit_recovery() {
    let (c, events) = coordinator(16);
    events.lose_delivery.store(true, Ordering::SeqCst);
    let id = c.submit(completed("保留"), Arc::new(MemoryWriter)).unwrap();
    let snapshot = exited(&c, &id).await;
    assert!(lock(&events.sent).is_empty());
    assert_eq!(snapshot.text, "保留");
    assert_eq!(snapshot.outcome, Some(Outcome::Completed));
    assert_eq!(snapshot.seq.done, 1);
    assert_eq!(snapshot.seq.chunk, 1);
}

#[tokio::test]
async fn utf8_chunks_and_total_limits_bound_memory() {
    let (c, events) = coordinator(16);
    let text = "中文字".repeat(3000);
    let id = c.submit(completed(&text), Arc::new(MemoryWriter)).unwrap();
    assert_eq!(exited(&c, &id).await.text, text);
    let chunks: Vec<String> = lock(&events.sent)
        .iter()
        .filter_map(|(_, event)| match event {
            TurnEvent::Chunk { delta, .. } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert!(chunks.iter().all(|chunk| chunk.len() <= MAX_CHUNK_BYTES));
    assert_eq!(chunks.concat(), text);
    let too_large = "x".repeat(MAX_TEXT_BYTES + 1);
    let id = c
        .submit(completed(&too_large), Arc::new(MemoryWriter))
        .unwrap();
    let snapshot = exited(&c, &id).await;
    assert!(snapshot.text.is_empty());
    assert_eq!(snapshot.error.unwrap().code, "llm.bad-response");
}

#[tokio::test]
async fn empty_finish_reasons_match_failed_events_and_cancel_has_none() {
    for finish in [
        FinishReason::Stop,
        FinishReason::Guard,
        FinishReason::Length,
    ] {
        let (c, events) = coordinator(16);
        let (mut request, _) = generation(
            vec![Ok(ProviderDelta::Finish(
                if finish == FinishReason::Length {
                    ProviderFinish::Length
                } else {
                    ProviderFinish::Stop
                },
            ))],
            false,
        );
        if finish == FinishReason::Guard {
            request.request.guard = GuardSpec::compile(vec![GuardRule {
                id: "close".into(),
                pattern: "</end>".into(),
                anchor: Anchor::Anywhere,
                priority: 0,
                server_eligible: true,
            }])
            .unwrap();
            request = generation(vec![Ok(ProviderDelta::Text("</end>".into()))], false).0;
            request.request.guard = GuardSpec::compile(vec![GuardRule {
                id: "close".into(),
                pattern: "</end>".into(),
                anchor: Anchor::Anywhere,
                priority: 0,
                server_eligible: true,
            }])
            .unwrap();
        }
        let id = c.submit(request, Arc::new(MemoryWriter)).unwrap();
        let s = exited(&c, &id).await;
        assert_eq!(s.finish_reason, Some(finish));
        assert_eq!(s.error.unwrap().code, "llm.empty-output");
        let event = serde_json::to_value(&lock(&events.sent)[0].1).unwrap();
        assert_eq!(event["finishReason"], serde_json::to_value(finish).unwrap());
    }
}

#[tokio::test]
async fn borrowed_private_does_not_publish_or_release_parent_lease() {
    let (c, events) = coordinator(16);
    let lease = c.acquire().unwrap();
    let result = c
        .run_private(&lease, completed("{\"private\":true}"))
        .await
        .unwrap();
    assert!(Uuid::parse_str(&result.turn_id).is_ok());
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.finish_reason, Some(FinishReason::Stop));
    assert_eq!(result.text, "{\"private\":true}");
    assert_eq!(
        c.snapshot(&result.turn_id).unwrap_err().code,
        "app.not-found"
    );
    assert!(lock(&events.sent).is_empty());
    assert!(lock(&events.seqs).is_empty());
    assert_eq!(c.acquire().err().unwrap().code, "app.busy");
    let (other, _) = coordinator(16);
    assert_eq!(
        other
            .submit_borrowed(&lease, completed("x"), Arc::new(MemoryWriter))
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    let public = c
        .submit_borrowed(&lease, completed("public"), Arc::new(MemoryWriter))
        .unwrap();
    assert_eq!(
        c.submit_borrowed(&lease, completed("busy"), Arc::new(MemoryWriter))
            .unwrap_err()
            .code,
        "app.busy"
    );
    exited(&c, &public).await;
    drop(lease);
    assert!(c.acquire().is_ok());
}

#[tokio::test]
async fn private_errors_cancel_and_shutdown_do_not_leak_or_hang() {
    let (c, events) = coordinator(1);
    let lease = c.acquire().unwrap();
    assert_eq!(
        c.run_private(&lease, completed(&"x".repeat(MAX_TEXT_BYTES + 1)))
            .await
            .err()
            .unwrap()
            .code,
        "llm.bad-response"
    );
    assert_eq!(
        c.run_private(&lease, completed(""))
            .await
            .err()
            .unwrap()
            .code,
        "llm.empty-output"
    );
    let (request, calls) = generation(vec![Ok(ProviderDelta::Text("P".into()))], true);
    let private = {
        let c = c.clone();
        let lease = lease.clone();
        tokio::spawn(async move { c.run_private(&lease, request).await })
    };
    while calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    c.shutdown().await;
    let result = private.await.unwrap().unwrap();
    assert_eq!(result.outcome, Outcome::Cancelled);
    assert!(result.finish_reason.is_none());
    assert_eq!(c.acquire().err().unwrap().code, "app.not-ready");
    assert_eq!(
        c.submit_borrowed(&lease, completed("x"), Arc::new(MemoryWriter))
            .unwrap_err()
            .code,
        "app.not-ready"
    );
    assert!(lock(&events.sent).is_empty());
}

#[tokio::test(start_paused = true)]
async fn head_deadline_during_commit_waits_boundary_without_reissuing_http() {
    let (c, _) = coordinator(1);
    let writer = Arc::new(Writer::new(false, false, true));
    let (mut request, calls) = generation(vec![Ok(ProviderDelta::Text("committed".into()))], true);
    request.policy.head_budget = Duration::from_secs(30);
    let id = c.submit(request, writer.clone()).unwrap();
    writer.started.notified().await;
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(c.snapshot(&id).unwrap().outcome.is_none());
    writer.released.notify_one();
    let s = exited(&c, &id).await;
    assert_eq!(s.text, "committed");
    assert_eq!(s.error.unwrap().code, "llm.aborted");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn error_mapping_and_budget_identity_do_not_echo_raw_inputs() {
    use mythos_llm::error::{RunError, TransportClass};
    for error in [
        ProviderError::Auth,
        ProviderError::Quota,
        ProviderError::RateLimited,
        ProviderError::Tls,
        ProviderError::Interrupted,
        ProviderError::Network {
            source: TransportClass::Connect,
        },
        ProviderError::BadResponse {
            reason: "secret prompt",
            status: Some(400),
        },
    ] {
        let (c, _) = coordinator(1);
        let request = generation(vec![Err(error)], false).0;
        let id = c.submit(request, Arc::new(MemoryWriter)).unwrap();
        let s = exited(&c, &id).await;
        assert_eq!(s.outcome, Some(Outcome::Failed));
        assert!(!serde_json::to_string(&s).unwrap().contains("secret prompt"));
    }
    for error in [
        RunError::Stalled,
        RunError::Budget {
            reason: "secret".into(),
        },
        RunError::InvalidPolicy("secret".into()),
    ] {
        let (fault, _) = Fault::generation(&error, false);
        assert!(!fault.to_string().contains("secret"));
    }
    struct Ledger(Mutex<Vec<AttemptIdentity>>);
    impl BudgetPort for Ledger {
        fn reserve(&self, id: &AttemptIdentity) -> Result<(), BudgetDenial> {
            lock(&self.0).push(id.clone());
            Ok(())
        }
        fn settle(&self, _: &AttemptIdentity, _: &AttemptOutcome) {}
    }
    let (c, _) = coordinator(1);
    let ledger = Arc::new(Ledger(Mutex::new(Vec::new())));
    let mut request = completed("x");
    request.budget = ledger.clone();
    let id = c.submit(request, Arc::new(MemoryWriter)).unwrap();
    exited(&c, &id).await;
    assert_eq!(lock(&ledger.0)[0].turn_id, id);
    assert!(Uuid::parse_str(&lock(&ledger.0)[0].request_id).is_ok());
    assert!(Coordinator::new(Arc::new(Events::default()), 0).is_err());
    assert!(Coordinator::new(Arc::new(Events::default()), 129).is_err());
}

#[test]
fn input_and_policy_validation_precede_gate_and_preserve_wire_shape() {
    let template = completed("valid");
    let invalid = |request: GenerationRequest, policy: RunPolicy| {
        assert_eq!(
            PreparedGeneration::new(
                template.provider.clone(),
                request,
                policy,
                template.budget.clone()
            )
            .err()
            .unwrap()
            .code,
            "app.bad-request"
        );
    };
    for model in ["", "bad\nmodel", &"x".repeat(257), "unknown"] {
        let mut request = template.request.clone();
        request.provider_request.model = model.into();
        invalid(request, template.policy.clone());
    }
    for temperature in [f64::NAN, -1.0, 3.0] {
        let mut request = template.request.clone();
        request.provider_request.sampling.temperature = temperature;
        invalid(request, template.policy.clone());
    }
    for max_tokens in [0, 4097] {
        let mut request = template.request.clone();
        request.provider_request.sampling.max_tokens = max_tokens;
        invalid(request, template.policy.clone());
    }
    for policy in [
        RunPolicy {
            max_requests: 0,
            ..template.policy.clone()
        },
        RunPolicy {
            max_requests: 4,
            ..template.policy.clone()
        },
        RunPolicy {
            transport_retries: 3,
            ..template.policy.clone()
        },
        RunPolicy {
            head_budget: Duration::ZERO,
            ..template.policy.clone()
        },
        RunPolicy {
            idle_budget: Duration::ZERO,
            ..template.policy.clone()
        },
        RunPolicy {
            ladder: vec![1.0],
            ..template.policy.clone()
        },
        RunPolicy {
            ladder: vec![f64::NAN],
            ..template.policy.clone()
        },
    ] {
        invalid(template.request.clone(), policy);
    }
    for prompt in [
        " ".to_owned(),
        "x".repeat(crate::request::MAX_INPUT_BYTES + 1),
    ] {
        let mut request = template.request.clone();
        request.provider_request.input = ProviderInput::Completion(CompletionInput { prompt });
        invalid(request, template.policy.clone());
    }
    let message = ChatMessage {
        role: ChatRole::User,
        content: "chat".into(),
    };
    for chat in [
        ChatInput {
            messages: vec![],
            assistant_prefix: None,
        },
        ChatInput {
            messages: vec![message.clone(); 129],
            assistant_prefix: None,
        },
        ChatInput {
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: " ".into(),
            }],
            assistant_prefix: None,
        },
        ChatInput {
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: "x".repeat(crate::request::MAX_INPUT_BYTES + 1),
            }],
            assistant_prefix: None,
        },
        ChatInput {
            messages: vec![message.clone()],
            assistant_prefix: Some(" ".into()),
        },
        ChatInput {
            messages: vec![message.clone()],
            assistant_prefix: Some("x".repeat(crate::request::MAX_INPUT_BYTES)),
        },
    ] {
        let mut request = template.request.clone();
        request.provider_request.input = ProviderInput::Chat(chat);
        invalid(request, template.policy.clone());
    }
    for assistant_prefix in [None, Some("prefix".into())] {
        let mut request = template.request.clone();
        request.provider_request.input = ProviderInput::Chat(ChatInput {
            messages: vec![message.clone()],
            assistant_prefix,
        });
        assert!(
            PreparedGeneration::new(
                template.provider.clone(),
                request,
                template.policy.clone(),
                template.budget.clone()
            )
            .is_ok()
        );
    }
    for input in [
        serde_json::json!({"kind":"completion","prompt":"P"}),
        serde_json::json!({"kind":"chat","messages":[{"role":"system","content":"S"},{"role":"user","content":"U"},{"role":"assistant","content":"A"}],"assistantPrefix":"pre"}),
    ] {
        let decoded: ProviderInput = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), input);
    }
    assert!(
        serde_json::from_value::<ProviderInput>(
            serde_json::json!({"kind":"chat","messages":[{"role":"tool","content":"T"}]})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<ProviderInput>(
            serde_json::json!({"kind":"completion","prompt":"P","endpoint":"forbidden"})
        )
        .is_err()
    );
    let (c, _) = coordinator(1);
    assert_eq!(
        c.submit(completed("x"), Arc::new(MemoryWriter))
            .unwrap_err()
            .code,
        "app.not-ready"
    );
}

#[tokio::test]
async fn queued_tail_cancel_and_abnormal_owner_exit_preserve_invariants() {
    let (tx, mut rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let (accepted, receipt) = tokio::sync::oneshot::channel();
    tx.send(PendingText {
        text: "uncommitted".into(),
        accepted,
    })
    .await
    .unwrap();
    cancel.cancel();
    assert!(next_pending(&mut rx, &cancel).await.is_none());
    assert!(receipt.await.is_err());
    struct PanicWriter;
    impl OutputWriter for PanicWriter {
        fn append<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<(), Fault>> {
            async { panic!("injected output owner panic") }.boxed()
        }
        fn finish<'a>(&'a self, _: &'a str, _: &'a Terminal) -> BoxFuture<'a, Result<(), Fault>> {
            async { Ok(()) }.boxed()
        }
    }
    let (c, _) = coordinator(1);
    let id = c.submit(completed("text"), Arc::new(PanicWriter)).unwrap();
    let s = exited(&c, &id).await;
    assert_eq!(s.outcome, Some(Outcome::Failed));
    assert_eq!(s.seq.chunk, 1);
    assert!(s.text.is_empty());
    assert!(c.acquire().is_ok());
    let (c, _) = coordinator(1);
    let lease = c.acquire().unwrap();
    let old = Arc::new(Turn {
        state: Mutex::new(TurnState {
            snapshot: TurnSnapshot::empty("draining".into()),
            reserved: Sequences::default(),
            producer_exited: false,
        }),
        cancel: CancellationToken::new(),
        changed: Notify::new(),
    });
    lock(&c.0.registry).turns.push_back(old.clone());
    assert_eq!(
        c.submit_borrowed(&lease, completed("new"), Arc::new(MemoryWriter))
            .unwrap_err()
            .code,
        "app.busy"
    );
    {
        let mut state = lock(&old.state);
        state.finish(Terminal::failed(Fault::event()));
        state.producer_exited = true;
    }
    let id = c
        .submit_borrowed(&lease, completed("new"), Arc::new(MemoryWriter))
        .unwrap();
    exited(&c, &id).await;
    drop(lease);
    assert!(c.acquire().is_ok());
    let lease = c.acquire().unwrap();
    drop(c);
    drop(lease);
}

#[tokio::test]
async fn lease_cancel_stops_private_and_wait_releases_child_before_next_call() {
    let (c, events) = coordinator(1);
    let lease = c.acquire().unwrap();
    let first = c
        .submit_borrowed(&lease, completed("first"), Arc::new(MemoryWriter))
        .unwrap();
    c.wait(&first).await.unwrap();
    let second = c
        .submit_borrowed(&lease, completed("second"), Arc::new(MemoryWriter))
        .unwrap();
    c.wait(&second).await.unwrap();
    assert_eq!(c.wait(&first).await.unwrap_err().code, "app.not-found");
    let (request, calls) = generation(vec![Ok(ProviderDelta::Text("private".into()))], true);
    let private = {
        let c = c.clone();
        let lease = lease.clone();
        tokio::spawn(async move { c.run_private(&lease, request).await })
    };
    while calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    lease.cancel();
    assert_eq!(private.await.unwrap().unwrap().outcome, Outcome::Cancelled);
    assert_eq!(lock(&events.sent).len(), 4);
    drop(lease);
    assert!(c.acquire().is_ok());
}

#[tokio::test]
async fn sequence_retirement_runs_without_holding_coordinator_lock() {
    struct Reentrant {
        events: Events,
        owner: Mutex<Weak<Inner>>,
    }
    impl EventPort for Reentrant {
        fn prepare(&self, event: TurnEvent) -> Result<PreparedEvent, Fault> {
            self.events.prepare(event)
        }
        fn deliver(&self, event: PreparedEvent) -> Result<(), Fault> {
            self.events.deliver(event)
        }
        fn retire(&self, id: &str) {
            let owner = lock(&self.owner).upgrade().unwrap();
            assert!(
                owner.registry.try_lock().is_ok(),
                "external port called inside registry lock"
            );
            self.events.retire(id);
        }
    }
    let events = Arc::new(Reentrant {
        events: Events::default(),
        owner: Mutex::new(Weak::new()),
    });
    let c = Coordinator::new(events.clone(), 1).unwrap();
    *lock(&events.owner) = Arc::downgrade(&c.0);
    let first = c.submit(completed("one"), Arc::new(MemoryWriter)).unwrap();
    c.wait(&first).await.unwrap();
    let second = c.submit(completed("two"), Arc::new(MemoryWriter)).unwrap();
    c.wait(&second).await.unwrap();
    assert_eq!(*lock(&events.events.retired), vec![first]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_cancel_and_finish_keep_one_terminal_and_release_borrowed_child() {
    let (c, events) = coordinator(1);
    let lease = c.acquire().unwrap();
    for _ in 0..64 {
        let id = c
            .submit_borrowed(&lease, completed("race"), Arc::new(MemoryWriter))
            .unwrap();
        let (cancelled, snapshot) = tokio::join!(c.cancel(&id), c.wait(&id));
        let snapshot = snapshot.unwrap();
        assert_eq!(Some(cancelled.unwrap().outcome), snapshot.outcome);
        match snapshot.outcome.unwrap() {
            Outcome::Completed => {
                assert_eq!(snapshot.text, "race");
                assert_eq!(snapshot.finish_reason, Some(FinishReason::Stop));
            }
            Outcome::Cancelled => assert_eq!(snapshot.finish_reason, None),
            Outcome::Failed => panic!("fixture must complete or cancel"),
        }
        assert_eq!(
            lock(&events.sent)
                .iter()
                .filter(|(_, event)| event.turn_id() == id && event.name() != "llm:turn:chunk")
                .count(),
            1
        );
        assert_eq!(Some(c.cancel(&id).await.unwrap().outcome), snapshot.outcome);
    }
    assert_eq!(lock(&events.retired).len(), 63);
}
