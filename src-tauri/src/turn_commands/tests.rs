use super::*;
use mythos_engine::ports::Outcome;
use mythos_llm::config::{FrozenProfile, LlmProfile, ProfileMode, ProxyConfig};
use mythos_llm::provider::{CompletionInput, ProviderInput};
use mythos_llm::sampling::Sampling;
use secrecy::SecretString;
use std::sync::Mutex;

fn frozen() -> FrozenProfile {
    FrozenProfile {
        profile: LlmProfile {
            profile_id: "test".into(),
            provider_id: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            mode: ProfileMode::Completion,
            thinking: false,
            sampling: Sampling {
                temperature: 1.0,
                max_tokens: 64,
            },
            proxy: ProxyConfig::None,
        },
        credential: Some(SecretString::from("fixture-key".to_owned())),
    }
}
fn input() -> ProviderInput {
    ProviderInput::Completion(CompletionInput {
        prompt: "local".into(),
    })
}

#[tokio::test]
async fn window_adapter_prepares_confirms_and_retires_independent_sequences() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let sink = observed.clone();
    let events = Arc::new(WindowEvents::new(move |event, payload| {
        sink.lock().unwrap().push((event.to_owned(), payload));
        Ok(())
    }));
    let service = TurnService::new(events.clone(), SystemProxySnapshot::default()).unwrap();
    let accepted = submit_frozen(&service, frozen(), None, input()).unwrap();
    let result = service.coordinator.cancel(&accepted.turn_id).await.unwrap();
    assert!(matches!(
        result.outcome,
        Outcome::Completed | Outcome::Cancelled
    ));
    let snapshot = service.coordinator.snapshot(&accepted.turn_id).unwrap();
    assert_eq!(snapshot.outcome, Some(result.outcome));
    let messages = observed.lock().unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|(name, _)| name == "llm:turn:done")
            .count(),
        1
    );
    assert!(
        messages
            .iter()
            .all(|(_, value)| value["data"]["turnId"] == accepted.turn_id)
    );
    drop(messages);
    let other = events
        .prepare(TurnEvent::Chunk {
            turn_id: "other-window-adapter".into(),
            delta: "B".into(),
        })
        .unwrap();
    assert_eq!(other.seq, 1);
    events.retire("other-window-adapter");
    assert_eq!(
        events
            .prepare(TurnEvent::Chunk {
                turn_id: "other-window-adapter".into(),
                delta: "C".into()
            })
            .unwrap()
            .seq,
        1
    );
    events.retire("other-window-adapter");
    let invalid = event_prepare_error(CmdError::new(
        "app.event-failed",
        "raw internal error",
        None,
    ));
    assert_eq!(invalid.code, "app.event-failed");
    assert!(!invalid.message.contains("raw"));
    let losing = WindowEvents::new(|_, _| Err(Fault::new("app.event-failed", "投递失败")));
    assert!(losing.deliver(other).is_err());
}

#[tokio::test]
async fn submit_validates_profile_input_and_key_before_occupying_gate() {
    let service = TurnService::new(
        Arc::new(WindowEvents::new(|_, _| Ok(()))),
        SystemProxySnapshot::default(),
    )
    .unwrap();
    let mut bad = frozen();
    bad.profile.provider_id = "unknown".into();
    assert_eq!(
        submit_frozen(&service, bad, None, input())
            .unwrap_err()
            .code,
        "app.not-found"
    );
    let mut bad = frozen();
    bad.credential = None;
    assert_eq!(
        submit_frozen(&service, bad, None, input())
            .unwrap_err()
            .code,
        "llm.missing-key"
    );
    let mut bad = frozen();
    bad.profile.thinking = true;
    assert_eq!(
        submit_frozen(&service, bad, None, input())
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    let mut bad = frozen();
    bad.profile.mode = ProfileMode::Chat;
    assert_eq!(
        submit_frozen(&service, bad, None, input())
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    let mut bad = frozen();
    bad.profile.model = "unknown".into();
    assert_eq!(
        submit_frozen(&service, bad, None, input())
            .unwrap_err()
            .code,
        "app.bad-request"
    );
    assert!(service.coordinator.acquire().is_ok());
    let mut bad = frozen();
    bad.profile.proxy = ProxyConfig::System;
    let failing_transport = TurnService::new(
        Arc::new(WindowEvents::new(|_, _| Ok(()))),
        SystemProxySnapshot {
            https: Some("::".into()),
        },
    )
    .unwrap();
    assert_eq!(
        submit_frozen(&failing_transport, bad, None, input())
            .unwrap_err()
            .code,
        "llm.network"
    );
    for assistant_prefix in [None, Some("prefix".into())] {
        let chat = ProviderInput::Chat(mythos_llm::provider::ChatInput {
            messages: vec![mythos_llm::provider::ChatMessage {
                role: mythos_llm::provider::ChatRole::User,
                content: "chat".into(),
            }],
            assistant_prefix,
        });
        let mut profile = frozen();
        profile.profile.mode = ProfileMode::Chat;
        let result = submit_frozen(&service, profile, None, chat).unwrap();
        service.coordinator.cancel(&result.turn_id).await.unwrap();
    }
}

#[tokio::test]
async fn fixture_runs_to_completion_without_calling_real_transport() {
    let done = Arc::new(tokio::sync::Notify::new());
    let notified = done.clone();
    let service = TurnService::new(
        Arc::new(WindowEvents::new(move |event, _| {
            if event == "llm:turn:done" {
                notified.notify_one();
            }
            Ok(())
        })),
        SystemProxySnapshot::default(),
    )
    .unwrap();
    let accepted = submit_frozen(&service, frozen(), None, input()).unwrap();
    done.notified().await;
    let snapshot = service.coordinator.snapshot(&accepted.turn_id).unwrap();
    assert_eq!(snapshot.text, "本地夹具正文。");
    assert_eq!(snapshot.outcome, Some(Outcome::Completed));
    service.coordinator.shutdown().await;
}

#[test]
fn malformed_input_uses_uniform_command_error_and_valid_wire_shape_is_preserved() {
    let valid = serde_json::json!({"kind":"completion","prompt":"P"});
    assert_eq!(
        serde_json::to_value(decode_input(valid.clone()).unwrap()).unwrap(),
        valid
    );
    for invalid in [
        serde_json::json!({"kind":"chat","messages":[{"role":"tool","content":"T"}]}),
        serde_json::json!({"kind":"completion","prompt":"P","endpoint":"forbidden"}),
        serde_json::json!("invalid"),
    ] {
        assert_eq!(decode_input(invalid).unwrap_err().code(), "app.bad-request");
    }
}

#[tokio::test]
async fn highest_legal_temperature_does_not_require_an_impossible_default_ladder() {
    let service = TurnService::new(
        Arc::new(WindowEvents::new(|_, _| Ok(()))),
        SystemProxySnapshot::default(),
    )
    .unwrap();
    let mut profile = frozen();
    profile.profile.sampling.temperature = 2.0;
    let accepted = submit_frozen(&service, profile, None, input()).unwrap();
    service.coordinator.cancel(&accepted.turn_id).await.unwrap();
}

#[test]
fn closing_platform_adapter_releases_captured_handle_and_rejects_delivery() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Handle(Arc<AtomicBool>);
    impl Drop for Handle {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let handle = Handle(dropped.clone());
    let events = WindowEvents::new(move |_, _| {
        let _ = &handle;
        Ok(())
    });
    assert!(!dropped.load(Ordering::Acquire));
    events.close();
    assert!(dropped.load(Ordering::Acquire));
    let prepared = events
        .prepare(TurnEvent::Chunk {
            turn_id: "closed-adapter".into(),
            delta: "data".into(),
        })
        .unwrap();
    assert_eq!(
        events.deliver(prepared).unwrap_err().code,
        "app.event-failed"
    );
    events.close();
    events.retire("closed-adapter");
}
