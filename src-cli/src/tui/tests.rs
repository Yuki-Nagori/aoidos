use super::{
    SettingsSection, ViewState, append_bounded, apply_turn_snapshot, billing_currency_shortcut,
    billing_lines, billing_title, cancel_game, confirm_action, continue_game, draft_cursor_column,
    draft_cursor_position, draft_cursor_row, friendly_error, game_layout, handle_game_key,
    handle_runtime_event, handle_selection_click, handle_selection_key, load_request_preflight,
    model_intro_lines, next_settings_section, open_selected_run, open_session, refresh_billing,
    refresh_runs, scroll_billing, settings_layout, settings_profile_index_at_row, submit_action,
    switch_settings_section, wrap_visual_lines,
};
use crate::{CliApp, EVENT_CAPACITY, EventBridge};
use aoidos_app::{GameServices, TurnRuntime, billing::RunBillingSummary, profiles::Profiles};
use aoidos_engine::{
    billing::{BillingLedger, BudgetWarning, LedgerBudgetPort, ModelUsageSummary},
    fault::Fault,
    game::product::{ProfileFactory, ProfileSource, ResolvedProfile},
    storage::Storage,
};
use aoidos_llm::{
    config::{FrozenProfile, LlmProfile, ProfileMode, ProxyConfig},
    error::ProviderError,
    provider::{
        Provider, ProviderCapabilities, ProviderDelta, ProviderFinish, ProviderRequest,
        ProviderStream, RequestMode, StartFuture, Usage,
    },
    proxy::SystemProxySnapshot,
    sampling::Sampling,
    schedule::{AllowAll, AttemptIdentity, AttemptOutcome, BudgetDenial, BudgetPort},
};
use futures::stream;
use secrecy::SecretString;
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
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
fn draft_cursor_uses_terminal_width_for_unicode_and_wrapped_lines() {
    let input = ratatui::layout::Rect::new(0, 0, 10, 5);

    assert_eq!(draft_cursor_column("甲乙", input), 4);
    assert_eq!(draft_cursor_row("甲乙", input), 0);
    assert_eq!(draft_cursor_column("甲乙丙", input), 0);
    assert_eq!(draft_cursor_row("甲乙丙", input), 1);
    assert_eq!(draft_cursor_column("abc\n中文", input), 4);
    assert_eq!(draft_cursor_row("abc\n中文", input), 1);
    assert_eq!(draft_cursor_position("甲乙丙丁", 5), (4, 1));
    assert_eq!(draft_cursor_position("abc\n", 3), (0, 1));
    assert_eq!(draft_cursor_position(&"x".repeat(100), 5).1, 20);
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
    let rendered = lines.join("\n");

    assert!(rendered.contains("deepseek/deepseek-flash"));
    assert!(rendered.contains("CNY 结算0.01"));
    assert!(rendered.contains("tok:240"));
    assert!(rendered.contains("未知:1"));
    assert!(rendered.contains("预算提醒：USD 已达到 80%"));
    assert!(billing_title(&state).contains("PgUp/PgDn"));
}

#[test]
fn billing_details_wrap_without_dropping_model_or_price_metadata() {
    let lines = super::wrap_display_lines(
        "价格：CNY · deepseek/deepseek-v4.1-flash · 版本 官方 2026-10",
        24,
    );
    let rendered = lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let flattened = rendered.replace('\n', "");

    assert!(flattened.contains("deepseek/deepseek-v4.1-flash"));
    assert!(flattened.contains("官方 2026-10"));
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

    assert_eq!(chunks.billing.height, 7);
    assert_eq!(chunks.status.height, 3);
    assert_eq!(
        [
            chunks.header,
            chunks.transcript,
            chunks.controls,
            chunks.billing,
            chunks.status
        ]
        .iter()
        .map(|chunk| chunk.height)
        .sum::<u16>(),
        24
    );
}

#[test]
fn game_layout_places_billing_beside_the_transcript_on_wide_terminals() {
    let chunks = game_layout(ratatui::layout::Rect::new(0, 0, 120, 30));

    assert_eq!(chunks.transcript.y, chunks.billing.y);
    assert_eq!(chunks.transcript.height, chunks.billing.height);
    assert_eq!(
        chunks.transcript.x + chunks.transcript.width,
        chunks.billing.x
    );
    assert_eq!(chunks.transcript.width + chunks.billing.width, 120);
    assert!(chunks.transcript.width > chunks.billing.width);
}

#[test]
fn settings_tabs_cycle_in_order_and_modal_stays_inside_terminal() {
    assert_eq!(
        next_settings_section(SettingsSection::Model, true),
        SettingsSection::Script
    );
    assert_eq!(
        next_settings_section(SettingsSection::Script, true),
        SettingsSection::Billing
    );
    assert_eq!(
        next_settings_section(SettingsSection::Billing, true),
        SettingsSection::Model
    );

    let area = ratatui::layout::Rect::new(0, 0, 100, 30);
    let layout = settings_layout(area);
    assert!(layout.panel.width <= area.width);
    assert!(layout.panel.height <= area.height);
    assert!(layout.panel.x + layout.panel.width <= area.width);
    assert!(layout.panel.y + layout.panel.height <= area.height);
    assert!(layout.model_tab.x < layout.script_tab.x);
    assert!(layout.script_tab.x < layout.billing_tab.x);
    assert!(layout.billing_tab.x < layout.close.x);
}

#[test]
fn model_profile_mouse_rows_match_the_rendered_wrapped_introduction() {
    let mut first = fake_profile();
    first.profile_id = "p".repeat(180);
    let second = fake_profile();
    for width in [20, 28, 40, 60, 80, 120] {
        let intro_rows = model_intro_lines(width, &[]).len();
        let first_height =
            wrap_visual_lines(&super::model_profile_text(&first, false, true), width).len();
        let second_height =
            wrap_visual_lines(&super::model_profile_text(&second, false, false), width).len();
        assert!(first_height > 1, "fixture must wrap at width {width}");
        let heights = [first_height, second_height];
        assert_eq!(
            settings_profile_index_at_row(intro_rows, intro_rows, &heights),
            Some(0)
        );
        assert_eq!(
            settings_profile_index_at_row(intro_rows + first_height - 1, intro_rows, &heights),
            Some(0)
        );
        assert_eq!(
            settings_profile_index_at_row(intro_rows + first_height, intro_rows, &heights),
            Some(1)
        );
        assert_eq!(
            settings_profile_index_at_row(intro_rows - 1, intro_rows, &heights),
            None
        );
        assert_eq!(
            settings_profile_index_at_row(
                intro_rows + first_height + second_height,
                intro_rows,
                &heights
            ),
            None
        );
    }
}

#[test]
fn leaving_billing_settings_cancels_run_bound_budget_draft() {
    let mut state = ViewState::new(Vec::new(), Vec::new());
    state.settings_section = SettingsSection::Billing;
    state.budget_editing = true;
    state.budget_edit_run_id = Some("run-a".into());
    state.budget_edit_revision = Some(4);
    state.budget_cny_draft = "1.25".into();
    state.budget_usd_draft = "2.50".into();

    switch_settings_section(&mut state, SettingsSection::Model);

    assert!(!state.budget_editing);
    assert_eq!(state.budget_edit_run_id, None);
    assert_eq!(state.budget_edit_revision, None);
    assert_eq!(state.settings_section, SettingsSection::Model);
    assert_eq!(state.budget_cny_draft, "1.25");
}

#[test]
fn deepseek_currency_shortcuts_match_the_account_options() {
    use crossterm::event::KeyCode;

    assert_eq!(
        billing_currency_shortcut(KeyCode::Char('c')),
        Some(aoidos_app::billing::Currency::Cny)
    );
    assert_eq!(
        billing_currency_shortcut(KeyCode::Char('U')),
        Some(aoidos_app::billing::Currency::Usd)
    );
    assert_eq!(billing_currency_shortcut(KeyCode::Char('x')), None);
}

#[test]
fn billing_groups_can_be_scrolled_with_keyboard_focus() {
    let mut state = ViewState::new(Vec::new(), Vec::new());
    state.billing = Some(RunBillingSummary {
        models: (0..12)
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
    assert!(billing_title(&state).contains("PgUp/PgDn"));
    scroll_billing(&mut state, true);
    assert_eq!(state.billing_scroll, 3);
    scroll_billing(&mut state, false);
    assert_eq!(state.billing_scroll, 2);
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
    let ledger_budget = Arc::new(RunLedgerBudget::default());
    let mut app = fake_app_with_options(
        &root,
        calls.clone(),
        false,
        ledger_budget.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let profiles = vec![fake_profile()];
    let scripts = aoidos_engine::game::assets::list().unwrap();
    let mut state = ViewState::new(profiles, scripts);

    open_session(&mut app, &mut state, false).await.unwrap();
    let first_session = state.session.as_ref().unwrap().session_id.clone();
    configure_tui_test_billing(&mut app, &mut state, &ledger_budget, "100");
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
    assert_eq!(calls.load(Ordering::SeqCst), 0, "请求预览不能调用 Provider");
    assert!(state.pending_action.is_some());
    let preview = billing_lines(&state, 5, 100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(preview.contains("模型：deepseek/deepseek-v4-pro"));
    assert!(preview.contains("价格：CNY"));
    assert!(preview.contains("本局上限：CNY 100.00 / USD 0.00"));
    handle_game_key(
        &mut app,
        &mut state,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ),
    )
    .await
    .unwrap();
    assert!(state.pending_action.is_none());
    assert_eq!(state.draft, action, "取消发送时保留草稿");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "取消预览不能调用 Provider");
    submit_action(&mut app, &mut state).await;
    confirm_action(&mut app, &mut state).await;
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
async fn tui_missing_run_budget_stops_before_confirmation() {
    let root = test_root("preflight-missing");
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut app = fake_app(&root, calls.clone());
    let mut state = new_state();
    open_session(&mut app, &mut state, false).await.unwrap();
    let draft = state.draft.clone();

    submit_action(&mut app, &mut state).await;

    assert!(state.status.contains("未设置"));
    assert!(state.pending_action.is_none());
    assert_eq!(state.draft, draft, "预算缺失时保留行动草稿");
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn tui_missing_selected_price_stops_before_confirmation() {
    let root = test_root("preflight-no-price");
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let ledger_budget = Arc::new(RunLedgerBudget::default());
    let mut app = fake_app_with_options(
        &root,
        calls.clone(),
        false,
        ledger_budget,
        Arc::new(AtomicBool::new(false)),
    );
    let mut state = new_state();
    open_session(&mut app, &mut state, false).await.unwrap();
    let run_id = state.session.as_ref().unwrap().session_id.clone();
    configure_test_budget(&app.storage, &run_id, "100");
    let draft = state.draft.clone();

    submit_action(&mut app, &mut state).await;

    assert!(state.status.contains("没有有效的登记价格"));
    assert!(state.pending_action.is_none());
    assert_eq!(state.draft, draft, "价格缺失时保留行动草稿");
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn tui_rechecks_run_limits_before_confirmed_dispatch() {
    let root = test_root("preflight-changed");
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let ledger_budget = Arc::new(RunLedgerBudget::default());
    let mut app = fake_app_with_options(
        &root,
        calls.clone(),
        false,
        ledger_budget.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let mut state = new_state();
    open_session(&mut app, &mut state, false).await.unwrap();
    configure_tui_test_billing(&mut app, &mut state, &ledger_budget, "100");
    let draft = state.draft.clone();

    submit_action(&mut app, &mut state).await;
    assert!(state.pending_action.is_some());
    update_test_limit(
        &app.storage,
        &state.session.as_ref().unwrap().session_id,
        "99",
        1,
    );
    confirm_action(&mut app, &mut state).await;

    assert!(state.pending_action.is_none());
    assert_eq!(state.draft, draft, "预览失效时保留行动草稿");
    assert!(state.status.contains("刚有变化"));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "旧上限下的确认不得派发");

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn tui_uses_the_frozen_profile_after_saved_profile_changes_or_deletion() {
    let root = test_root("preflight-profile-change");
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let ledger_budget = Arc::new(RunLedgerBudget::default());
    let mut app = fake_app_with_options(
        &root,
        calls.clone(),
        false,
        ledger_budget.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let mut state = new_state();
    open_session(&mut app, &mut state, false).await.unwrap();
    configure_tui_test_billing(&mut app, &mut state, &ledger_budget, "100");
    let frozen_profile = state.session_profile.clone().unwrap();

    let mut changed_profile = fake_profile();
    changed_profile.sampling.max_tokens += 1;
    app.profiles.save(changed_profile).unwrap();
    open_session(&mut app, &mut state, false).await.unwrap();
    assert_eq!(
        state.session_profile.as_ref(),
        Some(&frozen_profile),
        "普通重开也恢复该周目的冻结快照"
    );
    submit_action(&mut app, &mut state).await;
    assert!(state.pending_action.is_some());
    confirm_action(&mut app, &mut state).await;
    wait_for_turn(&mut app, &mut state).await;

    assert!(state.pending_action.is_none());
    let calls_after_profile_edit = calls.load(Ordering::SeqCst);
    assert!(calls_after_profile_edit > 0, "本局继续使用冻结配置");
    assert_eq!(state.session_profile.as_ref(), Some(&frozen_profile));

    app.profiles.delete("fake").unwrap();
    state.draft = "继续探索".into();
    submit_action(&mut app, &mut state).await;
    confirm_action(&mut app, &mut state).await;
    wait_for_turn(&mut app, &mut state).await;

    assert!(
        calls.load(Ordering::SeqCst) > calls_after_profile_edit,
        "删除新局配置不影响已开周目"
    );
    assert_eq!(state.session_profile.as_ref(), Some(&frozen_profile));

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn selecting_a_profile_for_the_next_run_keeps_active_run_profile_pinned() {
    let root = test_root("settings-next-profile");
    std::fs::create_dir_all(&root).unwrap();
    let ledger_budget = Arc::new(RunLedgerBudget::default());
    let mut app = fake_app_with_options(
        &root,
        Arc::new(AtomicUsize::new(0)),
        false,
        ledger_budget.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let mut state = new_state();
    open_session(&mut app, &mut state, false).await.unwrap();
    configure_tui_test_billing(&mut app, &mut state, &ledger_budget, "100");

    let mut next_profile = fake_profile();
    next_profile.profile_id = "next-run".into();
    next_profile.model = "deepseek-v4-flash".into();
    app.profiles.save(next_profile).unwrap();
    state.profiles = app.profiles.list().unwrap();
    state.profile_index = 1;

    let (_, active_profile) = load_request_preflight(&app, &mut state).unwrap();

    assert_eq!(active_profile.profile_id, "fake");
    assert_eq!(state.profile_index, 1, "保留新周目的模型选择");

    open_session(&mut app, &mut state, true).await.unwrap();
    assert_eq!(
        state.session_profile.as_ref().unwrap().profile_id,
        "next-run",
        "新周目才应用设置中的模型选择"
    );

    app.shutdown().await;
    drop(state);
    drop(app);
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
    let ledger_budget = Arc::new(RunLedgerBudget::default());
    let mut app = fake_app_with_options(
        &root,
        calls.clone(),
        true,
        ledger_budget.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let mut state = ViewState::new(
        vec![fake_profile()],
        aoidos_engine::game::assets::list().unwrap(),
    );
    open_session(&mut app, &mut state, false).await.unwrap();
    configure_tui_test_billing(&mut app, &mut state, &ledger_budget, "100");
    let action = state.draft.clone();
    submit_action(&mut app, &mut state).await;
    confirm_action(&mut app, &mut state).await;
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
    assert!(state.pending_action.is_some(), "继续骰判前必须预览并确认");
    assert_eq!(calls.load(Ordering::SeqCst), calls_before_resume);
    confirm_action(&mut app, &mut state).await;
    assert!(
        state
            .phase
            .as_ref()
            .is_some_and(|phase| phase.state.check.is_some())
    );
    continue_game(&mut app, &mut state).await;
    assert!(state.pending_action.is_some(), "恢复前必须再次确认");
    confirm_action(&mut app, &mut state).await;
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

#[tokio::test]
async fn tui_provider_auth_failure_is_visible_and_marks_the_operation_failed() {
    let root = test_root("provider-failure");
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let ledger_budget = Arc::new(RunLedgerBudget::default());
    let mut app = fake_app_with_options(
        &root,
        calls.clone(),
        false,
        ledger_budget.clone(),
        Arc::new(AtomicBool::new(true)),
    );
    let mut state = new_state();
    state.locale = "zh-Hans";
    open_session(&mut app, &mut state, false).await.unwrap();
    configure_tui_test_billing(&mut app, &mut state, &ledger_budget, "100");

    submit_action(&mut app, &mut state).await;
    confirm_action(&mut app, &mut state).await;
    wait_for_turn(&mut app, &mut state).await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(state.status.contains("认证") || state.status.contains("密钥"));
    assert!(!state.status.contains("llm.auth"));
    assert_eq!(
        state
            .phase
            .as_ref()
            .and_then(|phase| phase.state.last_operation.as_ref())
            .map(|operation| operation.outcome),
        Some(aoidos_engine::game::state::OperationOutcome::Failed)
    );

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn tui_billing_panel_refreshes_after_fake_provider_reservation_and_settlement() {
    let root = test_root("billing-lifecycle");
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let ledger_budget = Arc::new(RunLedgerBudget::default());
    let mut app = fake_app_with_options(
        &root,
        calls.clone(),
        false,
        ledger_budget.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let mut state = new_state();
    open_session(&mut app, &mut state, false).await.unwrap();
    let run_id = state.session.as_ref().unwrap().session_id.clone();
    configure_tui_test_billing(&mut app, &mut state, &ledger_budget, "100");

    submit_action(&mut app, &mut state).await;
    confirm_action(&mut app, &mut state).await;
    wait_for_turn(&mut app, &mut state).await;

    assert_eq!(
        state
            .phase
            .as_ref()
            .and_then(|phase| phase.state.last_operation.as_ref())
            .map(|operation| operation.outcome),
        Some(aoidos_engine::game::state::OperationOutcome::Completed),
        "计费成功路径应完成整个回合操作"
    );

    let summary = app.services.billing.run_summary(&run_id).unwrap();
    assert!(
        calls.load(Ordering::SeqCst) > 0,
        "回合应实际调用 fake Provider；状态={}；计费={:?}",
        state.status,
        state.billing
    );
    assert_eq!(summary.models.len(), 1);
    let model = &summary.models[0];
    assert_eq!(model.provider_id, "deepseek");
    assert_eq!(model.currency.to_string(), "CNY");
    assert_eq!(model.model_id, "deepseek-v4-pro");
    assert!(
        model
            .settled_amount
            .is_some_and(|amount| amount.nanos() > 0)
    );
    assert_eq!(model.reserved_amount, None, "成功完成后不应遗留预留金额");
    assert_eq!(model.known_tokens, Some(model.request_count * 120));
    assert_eq!(model.incomplete_requests, 0);
    assert_eq!(model.request_count, calls.load(Ordering::SeqCst) as u64);
    assert_eq!(
        state
            .billing
            .as_ref()
            .and_then(|billing| billing.models.first())
            .and_then(|model| model.settled_amount),
        model.settled_amount,
        "回合终态应刷新 TUI 费用面板"
    );

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn tui_budget_rejection_does_not_dispatch_fake_provider_request() {
    let root = test_root("budget-rejection");
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let ledger_budget = Arc::new(RunLedgerBudget::default());
    let mut app = fake_app_with_options(
        &root,
        calls.clone(),
        false,
        ledger_budget.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let mut state = new_state();
    state.locale = "zh-Hans";
    open_session(&mut app, &mut state, false).await.unwrap();
    let run_id = state.session.as_ref().unwrap().session_id.clone();
    configure_tui_test_billing(&mut app, &mut state, &ledger_budget, "0");

    submit_action(&mut app, &mut state).await;
    confirm_action(&mut app, &mut state).await;
    wait_for_turn(&mut app, &mut state).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "预算门禁拒绝后不能调用 Provider"
    );
    assert!(state.status.contains("预算不足"));
    assert!(
        app.services
            .billing
            .run_summary(&run_id)
            .unwrap()
            .models
            .is_empty()
    );

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

fn test_root(suffix: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("aoidos-cli-{suffix}-{unique}"))
}

fn new_state() -> ViewState {
    ViewState::new(
        vec![fake_profile()],
        aoidos_engine::game::assets::list().unwrap(),
    )
}

async fn wait_for_turn(app: &mut CliApp, state: &mut ViewState) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.busy {
            let event = app.events.recv().await.expect("事件桥仍然打开");
            handle_runtime_event(app, state, event);
        }
    })
    .await
    .expect("假 Provider 回合应在限定时间内结束");
}

fn fake_app(root: &Path, calls: Arc<AtomicUsize>) -> CliApp {
    fake_app_with_gate(root, calls, false)
}

fn fake_app_with_gate(root: &Path, calls: Arc<AtomicUsize>, request_check_once: bool) -> CliApp {
    fake_app_with_options(
        root,
        calls,
        request_check_once,
        Arc::new(AllowAll),
        Arc::new(AtomicBool::new(false)),
    )
}

fn fake_app_with_options(
    root: &Path,
    calls: Arc<AtomicUsize>,
    request_check_once: bool,
    budget: Arc<dyn BudgetPort>,
    fail_next_request: Arc<AtomicBool>,
) -> CliApp {
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
        fail_next_request,
    });
    let factory = ProfileFactory::with_builder(
        source,
        storage.clone(),
        proxy_snapshot,
        turns.diagnostics.clone(),
        Arc::new(move |_, _, _| Ok(provider.clone())),
        budget,
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
    let profiles = Arc::new(Profiles::new(&root.join("llm")));
    profiles.save(fake_profile()).unwrap();
    CliApp {
        _instance_lock: aoidos_store::lock::acquire(root).unwrap(),
        storage: storage.clone(),
        profiles,
        services: GameServices {
            turns,
            product,
            runtime,
            factory: factory.clone(),
            billing: aoidos_app::billing::BillingService::new(storage.clone()),
            runs: aoidos_app::runs::RunLifecycleService::new(storage, factory),
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

#[tokio::test]
async fn catalog_selection_restores_the_frozen_profile_without_sending_a_request() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("aoidos-cli-run-picker-{nonce}"));
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut app = fake_app(&root, calls.clone());
    let mut state = ViewState::new(
        vec![fake_profile()],
        aoidos_engine::game::assets::list().unwrap(),
    );

    open_session(&mut app, &mut state, true).await.unwrap();
    let session_id = state.session.as_ref().unwrap().session_id.clone();
    state.session = None;
    state.phase = None;
    state.existing_runs_open = true;
    refresh_runs(&app, &mut state, None);

    assert_eq!(state.existing_runs.len(), 1);
    assert_eq!(state.existing_runs[0].session_id, session_id);
    open_selected_run(&mut app, &mut state).await.unwrap();

    assert_eq!(state.session.as_ref().unwrap().session_id, session_id);
    assert_eq!(state.session_profile.as_ref().unwrap(), &fake_profile());
    assert!(!state.existing_runs_open);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn catalog_deletion_requires_confirmation_and_removes_only_the_selected_run() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("aoidos-cli-run-delete-{nonce}"));
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut app = fake_app(&root, calls.clone());
    let mut state = ViewState::new(
        vec![fake_profile()],
        aoidos_engine::game::assets::list().unwrap(),
    );

    open_session(&mut app, &mut state, true).await.unwrap();
    let session_id = state.session.as_ref().unwrap().session_id.clone();
    state.session = None;
    state.phase = None;
    state.existing_runs_open = true;
    refresh_runs(&app, &mut state, None);
    assert_eq!(state.existing_runs[0].session_id, session_id);

    handle_selection_key(
        &mut app,
        &mut state,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('d'),
            crossterm::event::KeyModifiers::NONE,
        ),
    )
    .await
    .unwrap();
    assert!(state.delete_confirmation);
    assert_eq!(state.existing_runs.len(), 1);

    handle_selection_key(
        &mut app,
        &mut state,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('y'),
            crossterm::event::KeyModifiers::NONE,
        ),
    )
    .await
    .unwrap();

    assert!(!state.delete_confirmation);
    assert!(state.existing_runs.is_empty(), "{}", state.status);
    assert!(state.deletion_in_progress.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn catalog_deletion_confirmation_keeps_its_target_when_another_row_is_clicked() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("aoidos-cli-run-delete-click-{nonce}"));
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut app = fake_app(&root, calls.clone());
    let mut state = ViewState::new(
        vec![fake_profile()],
        aoidos_engine::game::assets::list().unwrap(),
    );

    open_session(&mut app, &mut state, true).await.unwrap();
    let first_created_id = state.session.as_ref().unwrap().session_id.clone();
    state.session = None;
    state.phase = None;
    open_session(&mut app, &mut state, true).await.unwrap();
    let second_created_id = state.session.as_ref().unwrap().session_id.clone();
    assert_ne!(first_created_id, second_created_id);
    state.session = None;
    state.phase = None;
    state.existing_runs_open = true;
    refresh_runs(&app, &mut state, None);
    assert_eq!(state.existing_runs.len(), 2);
    let first_id = state.existing_runs[0].session_id.clone();
    let second_id = state.existing_runs[1].session_id.clone();

    state.existing_run_index = 0;
    handle_selection_key(
        &mut app,
        &mut state,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('d'),
            crossterm::event::KeyModifiers::NONE,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        state
            .delete_confirmation_target
            .as_ref()
            .unwrap()
            .session_id,
        first_id
    );

    let area = super::app_screen_area();
    let list = super::selection_layout(area)[1];
    handle_selection_click(&mut app, &mut state, list.x + 1, list.y + 2)
        .await
        .unwrap();
    assert_eq!(state.existing_run_index, 0);

    handle_selection_key(
        &mut app,
        &mut state,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('y'),
            crossterm::event::KeyModifiers::NONE,
        ),
    )
    .await
    .unwrap();
    assert!(
        !state
            .existing_runs
            .iter()
            .any(|run| run.session_id == first_id)
    );
    assert!(
        state
            .existing_runs
            .iter()
            .any(|run| run.session_id == second_id)
    );

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn startup_recovery_finishes_a_pending_deletion_before_exposing_the_catalog() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("aoidos-cli-run-recovery-{nonce}"));
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut app = fake_app(&root, calls.clone());
    let mut state = ViewState::new(
        vec![fake_profile()],
        aoidos_engine::game::assets::list().unwrap(),
    );

    open_session(&mut app, &mut state, true).await.unwrap();
    let session_id = state.session.as_ref().unwrap().session_id.clone();
    state.session = None;
    state.phase = None;
    state.existing_runs_open = true;
    refresh_runs(&app, &mut state, None);
    let run = state.existing_runs[0].clone();
    let operation_id = uuid::Uuid::new_v4().to_string();
    app.storage
        .request_run_deletion(
            &operation_id,
            &session_id,
            run.revision,
            "2026-10-21T00:00:00Z",
        )
        .unwrap();

    assert_eq!(
        app.services
            .runs
            .recover_pending_deletions(&app.services.product, &app.services.runtime)
            .await
            .unwrap(),
        aoidos_app::runs::DeletionRecoveryReport {
            completed: 1,
            failed: Vec::new(),
        }
    );
    assert!(app.storage.pending_run_deletions(128).unwrap().is_empty());
    assert!(
        app.services
            .runs
            .page(None, aoidos_engine::run_catalog::DEFAULT_PAGE_SIZE)
            .unwrap()
            .entries
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    app.shutdown().await;
    drop(state);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
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
    fail_next_request: Arc<AtomicBool>,
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
        if self.fail_next_request.swap(false, Ordering::SeqCst) {
            return Box::pin(async { Err(ProviderError::Auth) });
        }
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
                Ok(ProviderDelta::Usage(Usage {
                    prompt_tokens: 100,
                    completion_tokens: 20,
                    cached_prompt_tokens: Some(0),
                    cache_creation_tokens: Some(0),
                    reasoning_tokens: None,
                })),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ];
            Ok(Box::pin(stream::iter(deltas)) as ProviderStream)
        })
    }
}

#[derive(Default)]
struct RunLedgerBudget {
    storage: Mutex<Option<Arc<Storage>>>,
    run_id: Mutex<Option<String>>,
}

impl RunLedgerBudget {
    fn configure(&self, storage: Arc<Storage>, run_id: String) {
        *self.storage.lock().unwrap() = Some(storage);
        *self.run_id.lock().unwrap() = Some(run_id);
    }

    fn for_attempt(&self, attempt: &AttemptIdentity) -> Result<LedgerBudgetPort, BudgetDenial> {
        let storage = self
            .storage
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| BudgetDenial {
                reason: "test storage is not configured".into(),
            })?;
        let run_id = self
            .run_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| BudgetDenial {
                reason: "test run is not configured".into(),
            })?;
        Ok(LedgerBudgetPort::new(
            storage,
            run_id,
            "fake".into(),
            "deepseek".into(),
            attempt.model.clone(),
            "direct".into(),
            1_000_000,
        ))
    }
}

impl BudgetPort for RunLedgerBudget {
    fn reserve(&self, attempt: &AttemptIdentity) -> Result<(), BudgetDenial> {
        self.for_attempt(attempt)?.reserve(attempt)
    }

    fn reserve_with_input_upper_bound(
        &self,
        attempt: &AttemptIdentity,
        input_tokens: u64,
    ) -> Result<(), BudgetDenial> {
        self.for_attempt(attempt)?
            .reserve_with_input_upper_bound(attempt, input_tokens)
    }

    fn settle(&self, attempt: &AttemptIdentity, outcome: &AttemptOutcome) {
        if let Ok(port) = self.for_attempt(attempt) {
            port.settle(attempt, outcome);
        }
    }
}

fn configure_test_ledger(storage: &Storage, run_id: &str, cny_limit: &str) {
    let price = serde_json::from_value(serde_json::json!({
        "versionId": "cli-test-price-v1",
        "providerId": "deepseek",
        "modelId": "deepseek-v4-pro",
        "routePolicy": "direct",
        "currency": "CNY",
        "unitTokens": 1000,
        "inputUncached": "0.000001",
        "inputCached": "0.000001",
        "inputCachedWrite": "0.000001",
        "output": "0.000001",
        "schedule": null,
        "usageMappingVersion": "test-v1",
        "sourceKind": "user",
        "sourceUrl": null,
        "checkedAt": "2020-01-01T00:00:00Z",
        "effectiveFrom": "2020-01-01T00:00:00Z",
        "validUntil": null
    }))
    .unwrap();
    configure_test_budget(storage, run_id, cny_limit);
    storage
        .with_database(|connection| {
            BillingLedger::register_price(connection, &price, "2020-01-01T00:00:00Z").unwrap();
            BillingLedger::select_profile_price(
                connection,
                "fake",
                "deepseek",
                "deepseek-v4-pro",
                "direct",
                serde_json::from_value(serde_json::json!("CNY")).unwrap(),
                "2020-01-01T00:00:00Z",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
}

fn configure_test_budget(storage: &Storage, run_id: &str, cny_limit: &str) {
    storage
        .with_database(|connection| {
            BillingLedger::set_settings(
                connection,
                run_id,
                serde_json::from_value(serde_json::json!(cny_limit)).unwrap(),
                serde_json::from_value(serde_json::json!("0")).unwrap(),
                None,
                "2020-01-01T00:00:00Z",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
}

fn configure_tui_test_billing(
    app: &mut CliApp,
    state: &mut ViewState,
    budget: &RunLedgerBudget,
    cny_limit: &str,
) {
    let run_id = state.session.as_ref().unwrap().session_id.clone();
    configure_test_ledger(&app.storage, &run_id, cny_limit);
    budget.configure(app.storage.clone(), run_id.clone());
    refresh_billing(app, state, &run_id);
}

fn update_test_limit(storage: &Storage, run_id: &str, cny_limit: &str, revision: u64) {
    storage
        .with_database(|connection| {
            BillingLedger::set_settings(
                connection,
                run_id,
                serde_json::from_value(serde_json::json!(cny_limit)).unwrap(),
                serde_json::from_value(serde_json::json!("0")).unwrap(),
                Some(revision),
                "2020-01-02T00:00:00Z",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
}
