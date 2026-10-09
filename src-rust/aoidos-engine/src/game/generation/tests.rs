use super::*;
use aoidos_llm::{provider::*, sampling::Sampling, schedule::AllowAll};
struct Adapter {
    chat: bool,
    prefix: bool,
    completion: bool,
    window: Option<u32>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl Provider for Adapter {
    fn capabilities(&self, model: &str, _: RequestMode) -> ProviderCapabilities {
        ProviderCapabilities {
            model: model.into(),
            context_limit: self.window,
            completion: self.completion,
            chat: self.chat,
            prefix: self.prefix,
            thinking: false,
            stop_limit: 4,
            max_output_tokens: 4096,
            temperature_effective: true,
            temperature_min: 0.0,
            temperature_max: 2.0,
        }
    }
    fn start(&self, _: ProviderRequest, _: tokio_util::sync::CancellationToken) -> StartFuture<'_> {
        {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Err(aoidos_llm::error::ProviderError::Auth) })
        }
    }
}
fn profile() -> LlmProfile {
    LlmProfile {
        profile_id: "p".into(),
        provider_id: "deepseek".into(),
        model: "deepseek-v4-pro".into(),
        mode: ProfileMode::Completion,
        thinking: false,
        sampling: Sampling {
            temperature: 1.0,
            max_tokens: 64,
        },
        proxy: aoidos_llm::config::ProxyConfig::None,
    }
}
fn build(p: LlmProfile, a: Adapter) -> Result<FrozenRound, Fault> {
    ProfileGeneration::with_provider(
        p,
        Arc::new(a),
        crate::record::facts::DiceMode::Manual,
        Arc::new(AllowAll),
        Arc::new(Mutex::new(Calibration::default())),
    )
}
fn adapter() -> Adapter {
    Adapter {
        chat: true,
        prefix: true,
        completion: true,
        window: Some(1_000_000),
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }
}
#[tokio::test]
async fn freezing_shapes_and_preparing_share_the_exact_profile_without_starting_transport() {
    let p = profile();
    let a = adapter();
    let calls = a.calls.clone();
    let frozen = build(p.clone(), a).unwrap();
    assert_eq!(frozen.generation.shape(), Shape::Completion);
    assert_eq!(frozen.profile_id, "p");
    assert_eq!(
        frozen.profile_revision,
        format::hash(aoidos_json::canonical_string(&p).unwrap().as_bytes())
    );
    assert_eq!(frozen.generation.projection_budget().max_output_tokens, 64);
    let input = ProviderInput::Completion(CompletionInput {
        prompt: "正文".into(),
    });
    let request = frozen
        .generation
        .prepare(input, GuardSpec::default(), false)
        .unwrap();
    assert_eq!(request.request.provider_request.model, p.model);
    assert_eq!(request.request.provider_request.sampling.max_tokens, 64);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(
        request
            .provider
            .start(
                request.request.provider_request,
                tokio_util::sync::CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        frozen
            .generation
            .prepare(
                ProviderInput::Completion(CompletionInput { prompt: " ".into() }),
                GuardSpec::default(),
                false
            )
            .is_err()
    );
    assert!(
        frozen
            .generation
            .prepare(
                ProviderInput::Chat(ChatInput {
                    messages: vec![],
                    assistant_prefix: None
                }),
                GuardSpec::default(),
                true
            )
            .is_err()
    );
    let mut p = profile();
    p.mode = ProfileMode::Chat;
    assert_eq!(
        build(p.clone(), adapter()).unwrap().generation.shape(),
        Shape::ChatPrefix
    );
    let mut a = adapter();
    a.prefix = false;
    assert_eq!(build(p, a).unwrap().generation.shape(), Shape::Chat);
}
#[test]
fn invalid_capabilities_and_profiles_fail_before_acceptance() {
    for change in 0..5 {
        let mut p = profile();
        match change {
            0 => p.model.clear(),
            1 => p.thinking = true,
            2 => p.sampling.temperature = f64::NAN,
            3 => p.sampling.temperature = -1.0,
            _ => p.sampling.temperature = 3.0,
        };
        assert_eq!(build(p, adapter()).err().unwrap().code, "app.bad-request");
    }
    let mut a = adapter();
    a.completion = false;
    assert_eq!(build(profile(), a).err().unwrap().code, "app.bad-request");
    let mut a = adapter();
    a.window = None;
    assert_eq!(build(profile(), a).err().unwrap().code, "app.bad-request");
    let mut p = profile();
    p.sampling.max_tokens = 4097;
    assert!(build(p, adapter()).is_err());
    assert_eq!(
        profile_encoding(aoidos_json::Error::InvalidEncoding).code,
        "app.bad-request"
    );
    let frozen = FrozenProfile {
        profile: profile(),
        credential: None,
    };
    assert_eq!(
        ProfileGeneration::freeze(
            frozen,
            &SystemProxySnapshot::default(),
            None,
            crate::record::facts::DiceMode::Manual,
            Arc::new(AllowAll),
            Arc::new(Mutex::new(Calibration::default()))
        )
        .err()
        .unwrap()
        .code,
        "llm.missing-key"
    );
    let frozen = FrozenProfile {
        profile: profile(),
        credential: Some("fixture".to_string().into()),
    };
    assert!(
        ProfileGeneration::freeze(
            frozen,
            &SystemProxySnapshot::default(),
            None,
            crate::record::facts::DiceMode::Manual,
            Arc::new(AllowAll),
            Arc::new(Mutex::new(Calibration::default()))
        )
        .is_ok()
    );
}
