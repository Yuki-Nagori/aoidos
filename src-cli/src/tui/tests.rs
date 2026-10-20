use super::{
    ViewState, append_bounded, apply_turn_snapshot, billing_lines, billing_title, cancel_game,
    continue_game, friendly_error, game_layout, handle_runtime_event, open_session, scroll_billing,
    submit_action, truncate_display_width, wrap_visual_lines,
};
use crate::{CliApp, EVENT_CAPACITY, EventBridge};
use aoidos_app::{GameServices, TurnRuntime, billing::RunBillingSummary, profiles::Profiles};
use aoidos_engine::{
    billing::{BudgetWarning, ModelUsageSummary},
    fault::Fault,
    game::product::{ProfileFactory, ProfileSource, ResolvedProfile},
    storage::Storage,
};
use aoidos_llm::{
    config::{FrozenProfile, LlmProfile, ProfileMode, ProxyConfig},
    provider::{
        Provider, ProviderCapabilities, ProviderDelta, ProviderFinish, ProviderRequest,
        ProviderStream, RequestMode, StartFuture,
    },
    proxy::SystemProxySnapshot,
    sampling::Sampling,
    schedule::AllowAll,
};
use futures::stream;
use secrecy::SecretString;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

#[test]
fn transcript_stream_limit_counts_utf8_bytes_without_splitting_characters() {
    let mut output = String::from("a");

    append_bounded(&mut output, "中文", 7);
    append_bounded(&mut output, "再", 7);

    assert_eq!(output, "a中文");
    assert_eq!(output.len(), 7);
}

#[test]
fn transcript_wraps_long_narration_into_scrollable_display_rows() {
    assert_eq!(wrap_visual_lines("abcdef", 3), ["abc", "def"]);
    assert_eq!(wrap_visual_lines("甲乙丙", 4), ["甲乙", "丙"]);
    assert_eq!(wrap_visual_lines("a\nb", 3), ["a", "b"]);
}

#[test]
fn repeated_turn_snapshots_replace_stream_without_duplicating_text() {
    let mut state = ViewState::new(Vec::new(), Vec::new());
    let snapshot = || aoidos_engine::turn::TurnSnapshot {
        turn_id: "turn-1".into(),
        text: "完整正文".into(),
        seq: aoidos_engine::turn::Sequences::default(),
        outcome: None,
        finish_reason: None,
        error: None,
    };

    apply_turn_snapshot(&mut state, snapshot());
    apply_turn_snapshot(&mut state, snapshot());

    assert_eq!(state.stream, "完整正文");
}

#[test]
fn billing_panel_shows_original_currency_tokens_and_missing_usage() {
    let mut state = ViewState::new(Vec::new(), Vec::new());
    state.billing = Some(RunBillingSummary {
        models: vec![ModelUsageSummary {
            provider_id: "deepseek".into(),
            currency: "CNY".parse().unwrap(),
            model_id: "deepseek-flash".into(),
            settled_amount: Some(serde_json::from_str("\"0.014\"").unwrap()),
            reserved_amount: Some(serde_json::from_str("\"0.02\"").unwrap()),
            known_tokens: Some(240),
            incomplete_requests: 1,
            request_count: 2,
        }],
        warnings: vec![BudgetWarning {
            currency: "USD".parse().unwrap(),
            threshold_percent: 80,
            reached_at: "2026-10-20T00:00:00Z".into(),
            read: false,
        }],
    });

    let lines = billing_lines(&state, 2, 100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(lines[0].contains("deepseek/deepseek-flash"));
    assert!(lines[0].contains("CNY 结算0.01"));
    assert!(lines[0].contains("tok:240"));
    assert!(lines[0].contains("未知:1"));
    assert!(billing_title(&state).contains("USD 80%"));
}

#[test]
fn billing_rows_are_truncated_by_terminal_display_width() {
    assert_eq!(truncate_display_width("模型名称很长", 5), "模型…");
    assert_eq!(truncate_display_width("abc", 2), "ab");
}

#[test]
fn billing_failures_show_guidance_without_exposing_internal_codes() {
    let message = friendly_error("budget.exceeded", "内部预算错误", "zh-Hans");
    let settlement = friendly_error("budget.settlement-backlog", "内部队列错误", "zh-Hans");
    let english = friendly_error("budget.exceeded", "backend text", "en");

    assert!(message.contains("本局预算不足"));
    assert!(!message.contains("budget.exceeded"));
    assert!(!message.contains("内部预算错误"));
    assert!(settlement.contains("待结算队列已满"));
    assert!(!settlement.contains("不会重新发送"));
    assert_eq!(
        friendly_error("budget.exceeded", "backend text", "en"),
        "The run budget is insufficient. Check the limit for this currency in Billing settings."
    );
    assert!(!english.contains("budget.exceeded"));
    assert_eq!(
        friendly_error("app.not-ready", "存档版本尚不兼容", "zh-Hans"),
        "存档版本尚不兼容"
    );
}

#[test]
fn game_layout_allocates_dedicated_billing_and_status_panels() {
    let chunks = game_layout(ratatui::layout::Rect::new(0, 0, 100, 24));

    assert_eq!(chunks.len(), 5);
    assert_eq!(chunks[3].height, 4);
    assert_eq!(chunks[4].height, 3);
    assert_eq!(chunks.iter().map(|chunk| chunk.height).sum::<u16>(), 24);
}

#[test]
fn billing_groups_can_be_scrolled_with_keyboard_focus() {
    let mut state = ViewState::new(Vec::new(), Vec::new());
    state.billing = Some(RunBillingSummary {
        models: (0..3)
            .map(|index| ModelUsageSummary {
                provider_id: "provider".into(),
                currency: "USD".parse().unwrap(),
                model_id: format!("model-{index}"),
                settled_amount: None,
                reserved_amount: None,
                known_tokens: None,
                incomplete_requests: 0,
                request_count: 1,
            })
            .collect(),
        warnings: Vec::new(),
    });

    scroll_billing(&mut state, true);
    scroll_billing(&mut state, true);
    let lines = billing_lines(&state, 1, 80);

    assert!(lines[0].to_string().contains("model-2"));
    assert!(billing_title(&state).contains("3-3/3"));
    scroll_billing(&mut state, true);
    assert_eq!(state.billing_scroll, 2);
    scroll_billing(&mut state, false);
    assert_eq!(state.billing_scroll, 1);
}

#[tokio::test]
async fn fake_provider_opens_reopens_without_sending_then_runs_suggested_action() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("aoidos-cli-lifecycle-{unique}"));
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut app = fake_app(&root, calls.clone());
    let profiles = vec![fake_profile()];
    let scripts = aoidos_engine::game::assets::list().unwrap();
    let mut state = ViewState::new(profiles, scripts);

    open_session(&mut app, &mut state, false).await.unwrap();
    let first_session = state.session.as_ref().unwrap().session_id.clone();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!state.draft.is_empty(), "开局后应提供可编辑的剧本建议");

    state.session = None;
    state.phase = None;
    state.draft.clear();
    open_session(&mut app, &mut state, false).await.unwrap();
    assert_eq!(state.session.as_ref().unwrap().session_id, first_session);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "重开不能隐式重发请求");

    let action = state.draft.clone();
    submit_action(&mut app, &mut state).await;
    assert!(state.busy);
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.busy {
            let event = app.events.recv().await.expect("事件桥仍然打开");
            handle_runtime_event(&app, &mut state, event);
        }
    })
    .await
    .expect("假供应商回合应在限定时间内结束");

    assert!(calls.load(Ordering::SeqCst) > 0);
    assert!(
        state
            .history
            .iter()
            .any(|line| line == &format!("你：{action}"))
    );
    assert!(
        state
            .history
            .iter()
            .any(|line| line.contains("本地测试叙事"))
    );
    assert_eq!(
        state
            .phase
            .as_ref()
            .and_then(|phase| phase.state.last_operation.as_ref())
            .map(|operation| operation.outcome),
        Some(aoidos_engine::game::state::OperationOutcome::Completed)
    );
    let calls_after_turn = calls.load(Ordering::SeqCst);

    app.shutdown().await;
    drop(state);
    drop(app);

    let mut reopened_app = fake_app(&root, calls.clone());
    let mut reopened_state = ViewState::new(
        vec![fake_profile()],
        aoidos_engine::game::assets::list().unwrap(),
    );
    open_session(&mut reopened_app, &mut reopened_state, false)
        .await
        .unwrap();
    assert_eq!(
        reopened_state.session.as_ref().unwrap().session_id,
        first_session
    );
    assert_eq!(calls.load(Ordering::SeqCst), calls_after_turn);
    assert!(
        reopened_state
            .history
            .iter()
            .any(|line| line == &format!("你：{action}"))
    );
    assert!(
        reopened_state
            .history
            .iter()
            .any(|line| line.contains("本地测试叙事"))
    );

    reopened_app.shutdown().await;
    drop(reopened_state);
    drop(reopened_app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn tui_cancel_then_explicit_resume_does_not_duplicate_player_action() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("aoidos-cli-resume-{unique}"));
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut app = fake_app_with_gate(&root, calls.clone(), true);
    let mut state = ViewState::new(
        vec![fake_profile()],
        aoidos_engine::game::assets::list().unwrap(),
    );
    open_session(&mut app, &mut state, false).await.unwrap();
    let action = state.draft.clone();
    submit_action(&mut app, &mut state).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !state
            .phase
            .as_ref()
            .is_some_and(|phase| phase.state.check.is_some())
        {
            let event = app.events.recv().await.expect("事件桥仍然打开");
            handle_runtime_event(&app, &mut state, event);
        }
    })
    .await
    .expect("假供应商应返回待确认的判定");

    cancel_game(&mut app, &mut state).await;
    assert!(!state.busy);
    assert!(
        state
            .phase
            .as_ref()
            .is_some_and(|phase| phase.state.resume_required)
    );
    let calls_before_resume = calls.load(Ordering::SeqCst);

    continue_game(&mut app, &mut state).await;
    assert!(
        state
            .phase
            .as_ref()
            .is_some_and(|phase| phase.state.check.is_some())
    );
    continue_game(&mut app, &mut state).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.busy {
            let event = app.events.recv().await.expect("事件桥仍然打开");
            handle_runtime_event(&app, &mut state, event);
        }
    })
    .await
    .expect("明确恢复后的假供应商回合应结束");

    assert!(calls.load(Ordering::SeqCst) > calls_before_resume);
    assert_eq!(
        state
            .history
            .iter()
            .filter(|line| *line == &format!("你：{action}"))
            .count(),
        1,
        "恢复不能重复追加已提交的玩家行动"
    );
    assert_eq!(
        state
            .phase
            .as_ref()
            .and_then(|phase| phase.state.last_operation.as_ref())
            .map(|operation| operation.outcome),
        Some(aoidos_engine::game::state::OperationOutcome::Completed)
    );

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

fn fake_app(root: &Path, calls: Arc<AtomicUsize>) -> CliApp {
    fake_app_with_gate(root, calls, false)
}

fn fake_app_with_gate(root: &Path, calls: Arc<AtomicUsize>, request_check_once: bool) -> CliApp {
    let (sender, events) = tokio::sync::mpsc::channel(EVENT_CAPACITY);
    let bridge = Arc::new(EventBridge::new(sender));
    let storage = Arc::new(Storage::new(root, bridge.clone(), bridge.clone()));
    storage.ready().unwrap();
    let proxy_snapshot = SystemProxySnapshot::default();
    let turns = TurnRuntime::new(bridge.clone(), proxy_snapshot.clone()).unwrap();
    let source = Arc::new(FakeProfileSource);
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider {
        calls,
        request_check_once: AtomicBool::new(request_check_once),
    });
    let factory = ProfileFactory::with_builder(
        source,
        storage.clone(),
        proxy_snapshot,
        turns.diagnostics.clone(),
        Arc::new(move |_, _, _| Ok(provider.clone())),
        Arc::new(AllowAll),
    );
    let product = aoidos_engine::game::product::Product::new(
        root,
        storage.clone(),
        turns.coordinator.clone(),
        bridge.clone(),
        factory.clone(),
    );
    let runtime =
        aoidos_engine::game::runtime::Service::new(turns.coordinator.clone(), factory.clone());
    CliApp {
        _instance_lock: aoidos_store::lock::acquire(root).unwrap(),
        storage: storage.clone(),
        profiles: Arc::new(Profiles::new(&root.join("llm"))),
        services: GameServices {
            turns,
            product,
            runtime,
            factory,
            billing: aoidos_app::billing::BillingService::new(storage),
        },
        events,
        event_bridge: bridge,
    }
}

fn fake_profile() -> LlmProfile {
    LlmProfile {
        profile_id: "fake".into(),
        provider_id: "deepseek".into(),
        model: "deepseek-v4-pro".into(),
        mode: ProfileMode::Completion,
        thinking: false,
        sampling: Sampling {
            temperature: 1.0,
            max_tokens: 256,
        },
        proxy: ProxyConfig::None,
    }
}

struct FakeProfileSource;

impl ProfileSource for FakeProfileSource {
    fn freeze(&self, id: &str) -> Result<ResolvedProfile, Fault> {
        let mut profile = fake_profile();
        profile.profile_id = id.to_owned();
        Ok(ResolvedProfile {
            frozen: FrozenProfile {
                profile,
                credential: Some(SecretString::from("test-only-credential")),
            },
            proxy_auth: None,
        })
    }
}

struct FakeProvider {
    calls: Arc<AtomicUsize>,
    request_check_once: AtomicBool,
}

impl Provider for FakeProvider {
    fn capabilities(&self, model: &str, _mode: RequestMode) -> ProviderCapabilities {
        ProviderCapabilities {
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
        request: ProviderRequest,
        _cancellation: CancellationToken,
    ) -> StartFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = match &request.input {
            aoidos_llm::provider::ProviderInput::Completion(input)
                if input.prompt.contains("[AOIDOS:CHECK-PROPOSAL]") =>
            {
                if self.request_check_once.swap(false, Ordering::SeqCst) {
                    r#"{"kind":"check","ruleId":"pbta-explore-v1","actorId":"player","reason":"是否能找到出口需要判定"}"#
                } else {
                    "{\"kind\":\"noCheck\"}"
                }
            }
            aoidos_llm::provider::ProviderInput::Completion(input)
                if input.prompt.contains("[AOIDOS:SCENE-PROPOSAL]") =>
            {
                "{\"kind\":\"stay\"}"
            }
            _ => "本地测试叙事：我看见雾中亮起一盏灯。",
        };
        Box::pin(async move {
            let deltas = vec![
                Ok(ProviderDelta::Text(text.into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ];
            Ok(Box::pin(stream::iter(deltas)) as ProviderStream)
        })
    }
}
