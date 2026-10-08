//! 跨模块引擎单测复用的事件端口与本地 Provider；不发送网络请求。

use crate::{
    fault::Fault,
    ports::{EventPort, PreparedEvent, TurnEvent},
    request::PreparedGeneration,
};
use futures::{FutureExt, StreamExt, stream};
use mythos_llm::{
    error::ProviderError,
    guard::GuardSpec,
    provider::*,
    sampling::Sampling,
    schedule::{AllowAll, GenerationRequest, RunPolicy},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
pub(crate) struct Events {
    pub(crate) seqs: Mutex<HashMap<(String, String), u64>>,
    pub(crate) sent: Mutex<Vec<(u64, TurnEvent)>>,
    pub(crate) retired: Mutex<Vec<String>>,
    pub(crate) prepare_calls: AtomicUsize,
    pub(crate) fail_prepare: AtomicUsize,
    pub(crate) invalid_seq: AtomicBool,
    pub(crate) lose_delivery: AtomicBool,
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
            context_limit: Some(1_000_000),
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
pub(crate) fn generation(
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
