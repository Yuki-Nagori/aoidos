//! 终端视图只把键盘 / 鼠标转换成操作；游戏状态仍由共享 app 与 engine 所有。

use crate::{CliApp, events::RuntimeEvent};
use aoidos_app::billing::{RequestBillingPreflight, RunBillingSummary};
use aoidos_engine::{
    game::{assets::ScriptInfo, product::OpenedSession, state::PhaseSnapshot},
    ports::TurnEvent,
    record::view::View,
};
use aoidos_llm::config::LlmProfile;
use crossterm::{
    event::{
        Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap},
};
use std::{collections::VecDeque, io, time::Duration};
use unicode_width::UnicodeWidthChar;

const MAX_TRANSCRIPT_LINES: usize = 80;
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;
const MAX_STREAM_BYTES: usize = 32 * 1024;

const COLOR_BACKGROUND: Color = Color::Rgb(16, 23, 36);
const COLOR_PANEL: Color = Color::Rgb(22, 31, 47);
const COLOR_PANEL_SELECTED: Color = Color::Rgb(31, 45, 62);
const COLOR_TEXT: Color = Color::Rgb(226, 232, 240);
const COLOR_MUTED: Color = Color::Rgb(143, 157, 176);
const COLOR_BORDER: Color = Color::Rgb(58, 73, 94);
const COLOR_ACCENT: Color = Color::Rgb(104, 190, 187);
const COLOR_SECONDARY: Color = Color::Rgb(176, 159, 233);
const COLOR_WARNING: Color = Color::Rgb(237, 188, 119);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Profile,
    Script,
    Input,
    Submit,
    Continue,
    Billing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    Model,
    Script,
    Billing,
}

struct ViewState {
    profiles: Vec<LlmProfile>,
    scripts: Vec<ScriptInfo>,
    profile_index: usize,
    script_index: usize,
    session: Option<OpenedSession>,
    session_profile: Option<LlmProfile>,
    phase: Option<PhaseSnapshot>,
    history: VecDeque<String>,
    draft: String,
    stream: String,
    active_turn_id: Option<String>,
    stream_finalized: bool,
    history_scroll: usize,
    focus: Focus,
    busy: bool,
    status: String,
    billing: Option<RunBillingSummary>,
    billing_preflight: Option<RequestBillingPreflight>,
    billing_error: Option<String>,
    billing_scroll: usize,
    settings_open: bool,
    settings_section: SettingsSection,
    pending_action: Option<PendingAction>,
    locale: &'static str,
}

#[derive(Clone)]
struct PendingAction {
    kind: PendingActionKind,
    preflight: RequestBillingPreflight,
    profile: LlmProfile,
}

#[derive(Clone)]
enum PendingActionKind {
    SubmitInput(String),
    Continue,
}

impl ViewState {
    fn new(profiles: Vec<LlmProfile>, scripts: Vec<ScriptInfo>) -> Self {
        Self {
            profiles,
            scripts,
            profile_index: 0,
            script_index: 0,
            session: None,
            session_profile: None,
            phase: None,
            history: VecDeque::new(),
            draft: String::new(),
            stream: String::new(),
            active_turn_id: None,
            stream_finalized: false,
            history_scroll: 0,
            focus: Focus::Profile,
            busy: false,
            status: "选择 profile 与剧本，然后按 Enter 开始。".into(),
            billing: None,
            billing_preflight: None,
            billing_error: None,
            billing_scroll: 0,
            settings_open: false,
            settings_section: SettingsSection::Model,
            pending_action: None,
            locale: terminal_locale(),
        }
    }

    fn add_history(&mut self, line: String) {
        self.history.push_back(line);
        while self.history.len() > MAX_TRANSCRIPT_LINES
            || self.history.iter().map(String::len).sum::<usize>() > MAX_TRANSCRIPT_BYTES
        {
            self.history.pop_front();
        }
    }

    fn suggested_action(&self) -> Option<&str> {
        let script = self.scripts.get(self.script_index)?;
        script
            .suggested_actions
            .get(self.locale)
            .map(String::as_str)
    }
}

pub async fn run(app: &mut CliApp) -> Result<(), Box<dyn std::error::Error>> {
    let profiles = app.profiles.list().map_err(display_fault)?;
    let scripts = aoidos_engine::game::assets::list().map_err(display_fault)?;
    if profiles.is_empty() {
        return Err("尚无模型配置。请先在 Aoidos 设置中添加 profile，再运行 aoidos。".into());
    }
    let mut state = ViewState::new(profiles, scripts);
    let terminal_session = TerminalSession::start()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let result = event_loop(app, &mut state, &mut terminal).await;
    terminal.show_cursor()?;
    drop(terminal_session);
    result
}

async fn event_loop(
    app: &mut CliApp,
    state: &mut ViewState,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = EventStream::new();
    let mut billing_refresh = tokio::time::interval(Duration::from_secs(1));
    billing_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    billing_refresh.tick().await;
    loop {
        terminal.draw(|frame| draw(frame, state))?;
        tokio::select! {
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(event)) => if handle_terminal_event(app, state, event).await? { return Ok(()); },
                    Some(Err(error)) => return Err(Box::new(error)),
                    None => return Ok(()),
                }
            }
            event = app.events.recv() => {
                if let Some(event) = event {
                    handle_runtime_event(app, state, event);
                }
            }
            _ = billing_refresh.tick(), if state.busy => {
                refresh_billing_for_active_session(app, state);
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if app.event_bridge.take_overflowed() {
                    if let Some(session_id) = state
                        .session
                        .as_ref()
                        .map(|session| session.session_id.clone())
                    {
                        load_history(app, state, &session_id);
                    }
                    refresh_phase(app, state);
                    refresh_billing_for_active_session(app, state);
                    state.status = "事件通知有丢失，已从当前状态与记录重新同步。".into();
                }
                if state.busy {
                    reconcile_runtime(app, state);
                }
            }
        }
    }
}

async fn handle_terminal_event(
    app: &mut CliApp,
    state: &mut ViewState,
    event: Event,
) -> Result<bool, Box<dyn std::error::Error>> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(true);
            }
            if state.settings_open {
                handle_settings_key(app, state, key).await?;
            } else if state.session.is_none() {
                if handle_selection_key(app, state, key).await? {
                    return Ok(true);
                }
            } else if handle_game_key(app, state, key).await? {
                return Ok(true);
            }
        }
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if state.settings_open {
                    handle_settings_click(app, state, mouse.column, mouse.row).await?;
                } else if state.session.is_none() {
                    handle_selection_click(app, state, mouse.column, mouse.row).await?;
                } else {
                    handle_game_click(app, state, mouse.column, mouse.row).await?;
                }
            }
            MouseEventKind::ScrollUp if state.session.is_some() => {
                if state.settings_open {
                    if state.settings_section == SettingsSection::Billing {
                        scroll_billing(state, false);
                    }
                } else if contains(
                    game_layout(app_screen_area()).billing,
                    mouse.column,
                    mouse.row,
                ) {
                    state.focus = Focus::Billing;
                    scroll_billing(state, false);
                } else {
                    state.history_scroll = state.history_scroll.saturating_add(3);
                }
            }
            MouseEventKind::ScrollDown if state.session.is_some() => {
                if state.settings_open {
                    if state.settings_section == SettingsSection::Billing {
                        scroll_billing(state, true);
                    }
                } else if contains(
                    game_layout(app_screen_area()).billing,
                    mouse.column,
                    mouse.row,
                ) {
                    state.focus = Focus::Billing;
                    scroll_billing(state, true);
                } else {
                    state.history_scroll = state.history_scroll.saturating_sub(3);
                }
            }
            _ => {}
        },
        Event::Resize(_, _) => {}
        _ => {}
    }
    Ok(false)
}

async fn handle_settings_key(
    app: &mut CliApp,
    state: &mut ViewState,
    key: KeyEvent,
) -> Result<(), Box<dyn std::error::Error>> {
    match key.code {
        KeyCode::F(2) | KeyCode::Esc | KeyCode::Char('s' | 'S') => {
            state.settings_open = false;
            if state.session.is_some() {
                state.focus = Focus::Input;
            }
        }
        KeyCode::Tab | KeyCode::Right => {
            state.settings_section = next_settings_section(state.settings_section, true);
            state.billing_scroll = 0;
        }
        KeyCode::BackTab | KeyCode::Left => {
            state.settings_section = next_settings_section(state.settings_section, false);
            state.billing_scroll = 0;
        }
        KeyCode::Char('1') => state.settings_section = SettingsSection::Model,
        KeyCode::Char('2') => state.settings_section = SettingsSection::Script,
        KeyCode::Char('3') => state.settings_section = SettingsSection::Billing,
        KeyCode::Up | KeyCode::Down => match state.settings_section {
            SettingsSection::Model => {
                state.profile_index =
                    move_index(state.profile_index, state.profiles.len(), key.code);
            }
            SettingsSection::Script => {
                state.script_index = move_index(state.script_index, state.scripts.len(), key.code);
            }
            SettingsSection::Billing => scroll_billing(state, key.code == KeyCode::Down),
        },
        KeyCode::PageUp if state.settings_section == SettingsSection::Billing => {
            for _ in 0..5 {
                scroll_billing(state, false);
            }
        }
        KeyCode::PageDown if state.settings_section == SettingsSection::Billing => {
            for _ in 0..5 {
                scroll_billing(state, true);
            }
        }
        KeyCode::Enter if state.settings_section != SettingsSection::Billing => {
            state.status = "选择已保存到本次 TUI；新周目开始时生效。".into();
        }
        KeyCode::Char('n' | 'N') => {
            if state.busy || state.pending_action.is_some() {
                state.status = "当前回合尚未结束，暂不能开始新周目。".into();
            } else {
                state.settings_open = false;
                open_session(app, state, true).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

async fn handle_settings_click(
    app: &mut CliApp,
    state: &mut ViewState,
    x: u16,
    y: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let layout = settings_layout(app_screen_area());
    if contains(layout.close, x, y) {
        state.settings_open = false;
        if state.session.is_some() {
            state.focus = Focus::Input;
        }
        return Ok(());
    }
    for (section, area) in [
        (SettingsSection::Model, layout.model_tab),
        (SettingsSection::Script, layout.script_tab),
        (SettingsSection::Billing, layout.billing_tab),
    ] {
        if contains(area, x, y) {
            if state.settings_section != section {
                state.billing_scroll = 0;
            }
            state.settings_section = section;
            return Ok(());
        }
    }
    if contains(layout.body, x, y) {
        match state.settings_section {
            SettingsSection::Model if !state.profiles.is_empty() => {
                let intro_rows = wrap_visual_lines(
                    "所选模型只用于新周目；当前周目继续使用开局时冻结的配置。",
                    usize::from(layout.body.width).max(1),
                )
                .len();
                let row = usize::from(y.saturating_sub(layout.body.y));
                if row >= intro_rows {
                    state.profile_index =
                        (row - intro_rows).min(state.profiles.len().saturating_sub(1));
                }
            }
            SettingsSection::Script if !state.scripts.is_empty() => {
                let intro_rows = wrap_visual_lines(
                    "所选剧本只用于新周目；当前周目不会切换剧本。",
                    usize::from(layout.body.width).max(1),
                )
                .len();
                let row = usize::from(y.saturating_sub(layout.body.y));
                if row >= intro_rows {
                    state.script_index =
                        (row - intro_rows).min(state.scripts.len().saturating_sub(1));
                }
            }
            SettingsSection::Billing => {}
            _ => {}
        }
    } else if contains(layout.new_run, x, y) {
        if state.busy || state.pending_action.is_some() {
            state.status = "当前回合尚未结束，暂不能开始新周目。".into();
        } else {
            state.settings_open = false;
            open_session(app, state, true).await?;
        }
    }
    Ok(())
}

async fn handle_selection_key(
    app: &mut CliApp,
    state: &mut ViewState,
    key: KeyEvent,
) -> Result<bool, Box<dyn std::error::Error>> {
    match key.code {
        KeyCode::F(2) => {
            state.settings_open = true;
            state.settings_section = SettingsSection::Model;
        }
        KeyCode::Esc => return Ok(true),
        KeyCode::Char('s' | 'S') if state.focus != Focus::Input => {
            state.settings_open = true;
            state.settings_section = SettingsSection::Model;
        }
        KeyCode::Char('q') if state.focus != Focus::Input => return Ok(true),
        KeyCode::Tab | KeyCode::BackTab => {
            state.focus = match state.focus {
                Focus::Profile => Focus::Script,
                Focus::Script | Focus::Submit | Focus::Continue | Focus::Billing => Focus::Profile,
                Focus::Input => Focus::Profile,
            };
        }
        KeyCode::Up | KeyCode::Down if state.focus == Focus::Profile => {
            state.profile_index = move_index(state.profile_index, state.profiles.len(), key.code);
        }
        KeyCode::Up | KeyCode::Down if state.focus == Focus::Script => {
            state.script_index = move_index(state.script_index, state.scripts.len(), key.code);
        }
        KeyCode::Left | KeyCode::Right => {
            state.focus = if state.focus == Focus::Profile {
                Focus::Script
            } else {
                Focus::Profile
            };
        }
        KeyCode::Enter => open_session(app, state, false).await?,
        KeyCode::Char('n') | KeyCode::Char('N') => open_session(app, state, true).await?,
        _ => {}
    }
    Ok(false)
}

async fn handle_selection_click(
    app: &mut CliApp,
    state: &mut ViewState,
    x: u16,
    y: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let area = app_screen_area();
    let chunks = selection_layout(area);
    let lists = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);
    if contains(lists[0], x, y) {
        state.profile_index = row_index(lists[0], y, state.profiles.len());
        state.focus = Focus::Profile;
    } else if contains(lists[1], x, y) {
        state.script_index = row_index(lists[1], y, state.scripts.len());
        state.focus = Focus::Script;
    } else if contains(chunks[2], x, y) {
        let thirds = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[2]);
        if contains(thirds[0], x, y) {
            open_session(app, state, false).await?;
        } else if contains(thirds[1], x, y) {
            open_session(app, state, true).await?;
        }
    }
    Ok(())
}

async fn open_session(
    app: &mut CliApp,
    state: &mut ViewState,
    start_new: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(profile_id) = state
        .profiles
        .get(state.profile_index)
        .map(|profile| profile.profile_id.clone())
    else {
        state.status = "没有可用 profile。".into();
        return Ok(());
    };
    let profiles = match app.profiles.list() {
        Ok(profiles) => profiles,
        Err(_) => {
            state.status = "重新读取模型配置失败；无法打开周目。".into();
            return Ok(());
        }
    };
    let Some(profile_index) = profiles
        .iter()
        .position(|profile| profile.profile_id == profile_id)
    else {
        state.status = "所选模型配置已不存在；无法打开周目。".into();
        return Ok(());
    };
    state.profiles = profiles;
    state.profile_index = profile_index;
    let profile = state.profiles[profile_index].clone();
    let Some(script) = state.scripts.get(state.script_index) else {
        state.status = "没有可用剧本。".into();
        return Ok(());
    };
    match app
        .services
        .product
        .open(
            &app.services.runtime,
            profile.profile_id.clone(),
            script.script_id.clone(),
            start_new,
        )
        .await
    {
        Ok(session) => {
            state.status = if start_new {
                "已创建新周目。"
            } else {
                "已打开当前周目。"
            }
            .into();
            state.history.clear();
            state.stream.clear();
            state.active_turn_id = None;
            state.stream_finalized = false;
            state.history_scroll = 0;
            state.pending_action = None;
            state.billing = None;
            state.billing_preflight = None;
            state.billing_error = None;
            state.billing_scroll = 0;
            state.phase = app.services.runtime.get_phase(&session.session_id).ok();
            load_history(app, state, &session.session_id);
            let run_id = session.memory_run_id.clone();
            state.session = Some(session);
            state.session_profile = Some(profile);
            refresh_billing(app, state, &run_id);
            state.focus = Focus::Input;
            if state.draft.is_empty()
                && let Some(suggestion) = state.suggested_action().map(str::to_owned)
            {
                state.draft = suggestion;
            }
        }
        Err(error) => state.status = friendly_error(&error.code, &error.message, state.locale),
    }
    Ok(())
}

async fn handle_game_key(
    app: &mut CliApp,
    state: &mut ViewState,
    key: KeyEvent,
) -> Result<bool, Box<dyn std::error::Error>> {
    if state.pending_action.is_some() {
        match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => confirm_action(app, state).await,
            KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                let pending = state.pending_action.take().expect("待确认操作存在");
                let is_submit = matches!(&pending.kind, PendingActionKind::SubmitInput(_));
                restore_pending_draft(state, pending.kind);
                state.status = if is_submit {
                    "已取消发送；行动草稿已保留。".into()
                } else {
                    "已取消继续请求。".into()
                };
            }
            _ => state.status = "请按 Y 确认，或按 Esc 取消。".into(),
        }
        return Ok(false);
    }
    match key.code {
        KeyCode::Esc => return Ok(true),
        KeyCode::F(2) => {
            state.settings_open = true;
            state.settings_section = SettingsSection::Model;
        }
        KeyCode::Char('s' | 'S') if state.focus != Focus::Input => {
            state.settings_open = true;
            state.settings_section = SettingsSection::Model;
        }
        KeyCode::Char('q') if state.focus != Focus::Input => return Ok(true),
        KeyCode::PageUp if state.focus == Focus::Billing => scroll_billing(state, false),
        KeyCode::PageDown if state.focus == Focus::Billing => scroll_billing(state, true),
        KeyCode::PageUp => state.history_scroll = state.history_scroll.saturating_add(10),
        KeyCode::PageDown => state.history_scroll = state.history_scroll.saturating_sub(10),
        KeyCode::Char('x') | KeyCode::Char('X') if state.busy => {
            cancel_game(app, state).await;
        }
        KeyCode::Tab | KeyCode::BackTab => {
            state.focus = match state.focus {
                Focus::Input => Focus::Submit,
                Focus::Submit => Focus::Continue,
                Focus::Continue => Focus::Billing,
                Focus::Billing | Focus::Profile | Focus::Script => Focus::Input,
            };
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
            push_draft(state, '\n');
        }
        KeyCode::Enter if state.focus == Focus::Continue => continue_game(app, state).await,
        KeyCode::Enter if state.focus == Focus::Submit || state.focus == Focus::Input => {
            submit_action(app, state).await;
        }
        KeyCode::Char(ch)
            if state.focus == Focus::Input && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            push_draft(state, ch);
        }
        KeyCode::Backspace if state.focus == Focus::Input => {
            state.draft.pop();
        }
        KeyCode::Char('r') | KeyCode::Char('R')
            if state
                .phase
                .as_ref()
                .is_some_and(|p| p.state.check.is_some() || p.state.resume_required) =>
        {
            continue_game(app, state).await;
        }
        _ => {}
    }
    Ok(false)
}

fn push_draft(state: &mut ViewState, character: char) {
    if state.draft.len().saturating_add(character.len_utf8()) <= MAX_STREAM_BYTES {
        state.draft.push(character);
    } else {
        state.status = "输入已达到 32 KiB 上限。".into();
    }
}

fn scroll_billing(state: &mut ViewState, down: bool) {
    let area = if state.settings_open {
        settings_layout(app_screen_area()).body
    } else {
        game_layout(app_screen_area()).billing
    };
    let visible_rows = if state.settings_open {
        let info_rows =
            settings_billing_info_lines(usize::from(area.width.saturating_sub(1)).max(1)).len();
        usize::from(area.height).saturating_sub(info_rows) as u16
    } else {
        area.height.saturating_sub(2)
    };
    let visible = usize::from(visible_rows).max(1);
    let width = if state.settings_open {
        usize::from(area.width.saturating_sub(1)).max(1)
    } else {
        usize::from(area.width.saturating_sub(4)).max(1)
    };
    let last_start = billing_content_lines(state, width)
        .len()
        .saturating_sub(visible);
    state.billing_scroll = if down {
        state.billing_scroll.saturating_add(1).min(last_start)
    } else {
        state.billing_scroll.saturating_sub(1)
    };
}

async fn handle_game_click(
    app: &mut CliApp,
    state: &mut ViewState,
    x: u16,
    y: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let chunks = game_layout(app_screen_area());
    let controls = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(78), Constraint::Percentage(22)])
        .split(chunks.controls);
    if contains(chunks.settings_button, x, y) {
        state.settings_open = true;
        state.settings_section = SettingsSection::Model;
        return Ok(());
    }
    if state.pending_action.is_some() {
        if contains(controls[1], x, y) {
            confirm_action(app, state).await;
        }
        return Ok(());
    }
    if contains(controls[0], x, y) {
        state.focus = Focus::Input;
    } else if contains(controls[1], x, y) {
        if state.busy {
            cancel_game(app, state).await;
        } else {
            submit_action(app, state).await;
        }
    } else if contains(chunks.billing, x, y) {
        state.focus = Focus::Billing;
    } else if contains(chunks.status, x, y) {
        if state.busy {
            cancel_game(app, state).await;
        } else {
            continue_game(app, state).await;
        }
    }
    Ok(())
}

async fn submit_action(app: &mut CliApp, state: &mut ViewState) {
    if state.busy || state.draft.trim().is_empty() {
        return;
    }
    if state.pending_action.is_some() {
        return;
    }
    let Some((preflight, profile)) = load_request_preflight(app, state) else {
        return;
    };
    if preflight.budget.is_none() {
        state.status = "本局未设置 CNY / USD 上限；请先在设置中配置预算，再发送。".into();
        return;
    }
    let Some(price) = preflight.selected_price.as_ref() else {
        state.status = "当前 profile 没有有效的登记价格；请先在价格登记中确认费率。".into();
        return;
    };
    let currency = price.currency.clone();
    state.billing_preflight = Some(preflight.clone());
    state.pending_action = Some(PendingAction {
        kind: PendingActionKind::SubmitInput(state.draft.clone()),
        preflight,
        profile: profile.clone(),
    });
    state.status = format!(
        "请求预览：{} · {}/{} · {} 价格 · 输出 ≤ {} Token；确认详情见本局费用栏。按 Y 发送，Esc 取消。",
        profile.profile_id,
        profile.provider_id,
        profile.model,
        currency,
        profile.sampling.max_tokens,
    );
}

async fn confirm_action(app: &mut CliApp, state: &mut ViewState) {
    let Some(pending) = state.pending_action.take() else {
        return;
    };
    let Some((latest, latest_profile)) = load_request_preflight(app, state) else {
        restore_pending_draft(state, pending.kind);
        return;
    };
    if latest != pending.preflight || latest_profile != pending.profile {
        state.billing_preflight = Some(latest);
        refresh_profiles_preserving_selection(app, state);
        restore_pending_draft(state, pending.kind);
        state.status = "价格或本局上限刚有变化；请重新核对费用栏后再提交。".into();
        return;
    }
    match pending.kind {
        PendingActionKind::SubmitInput(input) => send_action(app, state, input).await,
        PendingActionKind::Continue => continue_game_unchecked(app, state).await,
    }
}

fn restore_pending_draft(state: &mut ViewState, kind: PendingActionKind) {
    if let PendingActionKind::SubmitInput(input) = kind {
        state.draft = input;
    }
}

async fn send_action(app: &mut CliApp, state: &mut ViewState, action: String) {
    let Some(session) = state.session.as_ref() else {
        state.draft = action;
        return;
    };
    match app
        .services
        .runtime
        .submit_input(&session.session_id, action.clone())
        .await
    {
        Ok(_) => {
            state.add_history(format!("你：{action}"));
            state.draft.clear();
            state.busy = true;
            state.stream.clear();
            state.active_turn_id = None;
            state.stream_finalized = false;
            state.status = "已提交，正在等待模型响应…".into();
            refresh_phase(app, state);
        }
        Err(error) => {
            state.draft = action;
            state.status = friendly_error(&error.code, &error.message, state.locale);
        }
    }
}

fn load_request_preflight(
    app: &CliApp,
    state: &mut ViewState,
) -> Option<(RequestBillingPreflight, aoidos_llm::config::LlmProfile)> {
    let session = state.session.as_ref()?;
    let frozen_profile = state.session_profile.as_ref()?.clone();
    let selected_profile_id = state
        .profiles
        .get(state.profile_index)
        .map(|profile| profile.profile_id.clone());
    let profiles = match app.profiles.list() {
        Ok(profiles) => profiles,
        Err(_) => {
            state.status = "重新读取模型配置失败；未发送请求。".into();
            return None;
        }
    };
    let Some(profile) = profiles
        .iter()
        .find(|profile| profile.profile_id == frozen_profile.profile_id)
        .cloned()
    else {
        state.status = "当前周目使用的模型配置已不存在；未发送请求。".into();
        return None;
    };
    state.profiles = profiles;
    if let Some(index) = selected_profile_id.and_then(|id| {
        state
            .profiles
            .iter()
            .position(|profile| profile.profile_id == id)
    }) {
        state.profile_index = index;
    }
    if frozen_profile != profile {
        state.status = "当前周目的模型配置已变化；请关闭并重新打开周目后再发送。".into();
        return None;
    }
    let preflight = app.services.billing.request_preflight(
        &session.memory_run_id,
        &profile.profile_id,
        &profile.provider_id,
        &profile.model,
        aoidos_engine::game::product::billing_route_policy(&profile.provider_id),
    );
    match preflight {
        Ok(preflight) => Some((preflight, profile)),
        Err(_) => {
            state.status = "读取本局价格或额度失败；未发送请求。".into();
            None
        }
    }
}

fn refresh_profiles_preserving_selection(app: &CliApp, state: &mut ViewState) {
    let selected_profile_id = state
        .profiles
        .get(state.profile_index)
        .map(|profile| profile.profile_id.clone());
    if let Ok(profiles) = app.profiles.list() {
        if let Some(index) = selected_profile_id
            .and_then(|id| profiles.iter().position(|profile| profile.profile_id == id))
        {
            state.profile_index = index;
        } else {
            state.profile_index = state.profile_index.min(profiles.len().saturating_sub(1));
        }
        state.profiles = profiles;
    }
}

async fn cancel_game(app: &mut CliApp, state: &mut ViewState) {
    let Some((session, round)) = state.session.as_ref().and_then(|session| {
        state
            .phase
            .as_ref()?
            .state
            .in_flight
            .as_ref()?
            .round_id
            .as_ref()
            .map(|round| (session.session_id.clone(), round.clone()))
    }) else {
        state.status = "当前没有可取消的回合。".into();
        return;
    };

    match app.services.runtime.cancel_round(&session, &round).await {
        Ok(_) => {
            state.busy = false;
            state.status = "已请求取消；已提交的记录仍会保留。".into();
            refresh_phase(app, state);
        }
        Err(error) => state.status = friendly_error(&error.code, &error.message, state.locale),
    }
}

async fn continue_game(app: &mut CliApp, state: &mut ViewState) {
    if state.busy || state.pending_action.is_some() {
        return;
    }
    let Some((preflight, profile)) = load_request_preflight(app, state) else {
        return;
    };
    if preflight.budget.is_none() {
        state.status = "本局未设置 CNY / USD 上限；请先在设置中配置预算，再继续。".into();
        return;
    }
    let Some(price) = preflight.selected_price.as_ref() else {
        state.status = "当前 profile 没有有效的登记价格；请先在价格登记中确认费率。".into();
        return;
    };
    let currency = price.currency.clone();
    state.billing_preflight = Some(preflight.clone());
    state.pending_action = Some(PendingAction {
        kind: PendingActionKind::Continue,
        preflight,
        profile: profile.clone(),
    });
    state.status = format!(
        "继续请求预览：{} · {}/{} · {} 价格 · 输出 ≤ {} Token；确认详情见本局费用栏。按 Y 继续，Esc 取消。",
        profile.profile_id,
        profile.provider_id,
        profile.model,
        currency,
        profile.sampling.max_tokens,
    );
}

async fn continue_game_unchecked(app: &mut CliApp, state: &mut ViewState) {
    let Some(session) = state.session.as_ref() else {
        return;
    };
    let id = session.session_id.clone();
    let phase = match app.services.runtime.get_phase(&id) {
        Ok(phase) => phase,
        Err(error) => {
            state.status = friendly_error(&error.code, &error.message, state.locale);
            return;
        }
    };
    let result = if let (Some(check), Some(round)) = (&phase.state.check, &phase.state.in_flight) {
        if let Some(round_id) = &round.round_id {
            app.services
                .runtime
                .submit_check(&id, round_id, &check.plan_id)
                .await
                .map(|_| ())
        } else {
            state.status = "骰判等待有效回合身份；请重新读取状态。".into();
            return;
        }
    } else if phase.state.resume_required || phase.state.needs_recovery {
        app.services.runtime.resume(&id).await.map(|_| ())
    } else {
        state.status = "目前没有待繼續的檢查或恢复操作。".into();
        return;
    };
    match result {
        Ok(()) => {
            state.busy = true;
            state.status = "继续处理中…".into();
            refresh_phase(app, state);
        }
        Err(error) => state.status = friendly_error(&error.code, &error.message, state.locale),
    }
}

fn handle_runtime_event(app: &CliApp, state: &mut ViewState, event: RuntimeEvent) {
    match event {
        RuntimeEvent::Turn(TurnEvent::Chunk { turn_id, delta: _ }) => {
            if state.active_turn_id.as_ref() != Some(&turn_id) && !is_active_turn(state, &turn_id) {
                return;
            }
            refresh_turn_snapshot_for(app, state, &turn_id);
        }
        RuntimeEvent::Turn(TurnEvent::Done {
            turn_id, outcome, ..
        }) => {
            if is_active_turn(state, &turn_id) {
                refresh_turn_snapshot_for(app, state, &turn_id);
                state.active_turn_id = Some(turn_id);
                finalize_stream(state, false);
                state.status = format!("操作结束：{outcome:?}");
                refresh_phase(app, state);
                refresh_billing_for_active_session(app, state);
            }
        }
        RuntimeEvent::Turn(TurnEvent::Failed {
            turn_id,
            code,
            message,
            ..
        }) => {
            if is_active_turn(state, &turn_id) {
                refresh_turn_snapshot_for(app, state, &turn_id);
                state.active_turn_id = Some(turn_id);
                finalize_stream(state, true);
                state.status = friendly_error(&code, &message, state.locale);
                refresh_phase(app, state);
                refresh_billing_for_active_session(app, state);
            }
        }
        RuntimeEvent::Phase(event) => {
            let event_state = event.state();
            let should_apply = state
                .phase
                .as_ref()
                .is_some_and(|phase| phase_state_is_newer(event_state, &phase.state));
            if should_apply && let Some(phase) = state.phase.as_mut() {
                phase.state = event.state().clone();
                sync_busy(state);
            }
            if let Some(error) = state
                .phase
                .as_ref()
                .and_then(|phase| phase.state.last_operation.as_ref())
                .and_then(|operation| operation.error.as_ref())
            {
                state.status = friendly_error(&error.code, &error.message, state.locale);
            }
        }
        RuntimeEvent::Record(event) => {
            let run_id = state
                .session
                .as_ref()
                .filter(|session| session.session_id == event.session_id)
                .map(|session| session.memory_run_id.clone());
            if let Some(run_id) = run_id {
                load_history(app, state, &event.session_id);
                refresh_billing(app, state, &run_id);
            }
        }
        RuntimeEvent::Migration(event) => {
            state.status = match event {
                aoidos_engine::migration::MigrationEvent::Done { .. } => "存储迁移已完成。".into(),
                aoidos_engine::migration::MigrationEvent::Failed { code, message, .. } => {
                    format!("存储迁移失败：{code}：{message}")
                }
                aoidos_engine::migration::MigrationEvent::Progress { .. } => {
                    "正在执行存储迁移…".into()
                }
            };
        }
    }
}

fn append_bounded(target: &mut String, value: &str, limit: usize) {
    let mut available = limit.saturating_sub(target.len());
    for character in value.chars() {
        let bytes = character.len_utf8();
        if bytes > available {
            break;
        }
        target.push(character);
        available -= bytes;
    }
}

fn is_active_turn(state: &ViewState, turn_id: &str) -> bool {
    let expected = state
        .phase
        .as_ref()
        .and_then(|phase| phase.state.in_flight.as_ref())
        .and_then(|flight| flight.turn_id.as_ref())
        .or(state.active_turn_id.as_ref());
    expected.is_none_or(|expected| expected == turn_id)
}

fn refresh_phase(app: &CliApp, state: &mut ViewState) {
    if let Some(session) = &state.session
        && let Ok(phase) = app.services.runtime.get_phase(&session.session_id)
    {
        let is_newer = state
            .phase
            .as_ref()
            .is_none_or(|current| phase_state_is_newer(&phase.state, &current.state));
        if is_newer {
            state.phase = Some(phase);
            sync_busy(state);
        }
        let phase_state = &state.phase.as_ref().expect("刚写入阶段快照").state;
        if let Some(error) = phase_state
            .last_operation
            .as_ref()
            .filter(|operation| {
                operation.outcome == aoidos_engine::game::state::OperationOutcome::Failed
            })
            .and_then(|operation| operation.error.as_ref())
        {
            state.status = friendly_error(&error.code, &error.message, state.locale);
        }
        refresh_turn_snapshot(app, state);
    }
}

fn phase_state_is_newer(
    incoming: &aoidos_engine::game::state::PhaseState,
    current: &aoidos_engine::game::state::PhaseState,
) -> bool {
    incoming.state_epoch != current.state_epoch || incoming.phase_revision > current.phase_revision
}

fn reconcile_runtime(app: &CliApp, state: &mut ViewState) {
    refresh_phase(app, state);
}

fn sync_busy(state: &mut ViewState) {
    let Some(phase) = state.phase.as_ref() else {
        return;
    };
    let waiting_for_check = phase
        .state
        .check
        .as_ref()
        .is_some_and(|check| check.status == aoidos_engine::game::state::CheckStatus::Waiting);
    state.busy = phase.state.in_flight.is_some() && !waiting_for_check;
}

fn refresh_turn_snapshot(app: &CliApp, state: &mut ViewState) {
    if !state.busy {
        return;
    }
    let turn_id = state
        .phase
        .as_ref()
        .and_then(|phase| phase.state.in_flight.as_ref())
        .and_then(|in_flight| in_flight.turn_id.clone())
        .or_else(|| state.active_turn_id.clone());
    let Some(turn_id) = turn_id else { return };
    let Ok(snapshot) = app.services.turns.coordinator.snapshot(&turn_id) else {
        return;
    };
    apply_turn_snapshot(state, snapshot);
}

fn refresh_turn_snapshot_for(app: &CliApp, state: &mut ViewState, turn_id: &str) {
    let Ok(snapshot) = app.services.turns.coordinator.snapshot(turn_id) else {
        return;
    };
    apply_turn_snapshot(state, snapshot);
}

fn apply_turn_snapshot(state: &mut ViewState, snapshot: aoidos_engine::turn::TurnSnapshot) {
    let turn_id = snapshot.turn_id;
    if state.active_turn_id.as_ref() != Some(&turn_id) {
        state.stream_finalized = false;
        state.history_scroll = 0;
    }
    state.active_turn_id = Some(turn_id);
    if state.stream_finalized {
        return;
    }
    state.stream.clear();
    append_bounded(&mut state.stream, &snapshot.text, MAX_STREAM_BYTES);
    if snapshot.outcome.is_some() {
        finalize_stream(state, snapshot.error.is_some());
    }
}

fn finalize_stream(state: &mut ViewState, failed: bool) {
    if state.stream_finalized {
        return;
    }
    if !state.stream.is_empty() {
        let response = std::mem::take(&mut state.stream);
        state.add_history(if failed {
            format!("AI（未完成）：{response}")
        } else {
            format!("AI：{response}")
        });
    }
    state.stream_finalized = true;
}

fn load_history(app: &CliApp, state: &mut ViewState, session_id: &str) {
    let Ok(session) = app.storage.records.get(session_id) else {
        return;
    };
    let session = session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Ok(view) = session.view(Some(40)) else {
        state.status = "读取记录失败；可重新打开周目恢复。".into();
        return;
    };
    display_view(&session, state, view);
}

fn refresh_billing_for_active_session(app: &CliApp, state: &mut ViewState) {
    if let Some(run_id) = state
        .session
        .as_ref()
        .map(|session| session.memory_run_id.clone())
    {
        refresh_billing(app, state, &run_id);
    }
}

fn refresh_billing(app: &CliApp, state: &mut ViewState, run_id: &str) {
    match app.services.billing.run_summary(run_id) {
        Ok(summary) => {
            state.billing_scroll = state
                .billing_scroll
                .min(summary.models.len().saturating_sub(1));
            state.billing = Some(summary);
            state.billing_error = None;
            let preflight = load_request_preflight(app, state).map(|(preflight, _)| preflight);
            state.billing_preflight = preflight;
        }
        Err(_) => {
            state.billing = None;
            state.billing_preflight = None;
            state.billing_error = Some("费用账本暂时不可用".into());
        }
    }
}

fn display_view(
    session: &aoidos_engine::record::session::Session,
    state: &mut ViewState,
    view: View,
) {
    let mut history = VecDeque::new();
    let mut total = 0;
    for item in view.page.items.into_iter().rev() {
        let body = match (&item.body_ref, item.body) {
            (Some(reference), _) => session.body(reference, None).ok().map(|page| page.text),
            (None, Some(body)) => record_text(&body),
            _ => None,
        };
        let Some(body) = body else { continue };
        if body.is_empty() {
            continue;
        }
        let speaker = if item.kind == "playerSpeech" {
            "你"
        } else {
            "AI"
        };
        let line = format!("{speaker}：{}", body.replace('\n', " "));
        total += line.len();
        history.push_back(line);
        while history.len() > MAX_TRANSCRIPT_LINES || total > MAX_TRANSCRIPT_BYTES {
            if let Some(removed) = history.pop_front() {
                total = total.saturating_sub(removed.len());
            }
        }
    }
    state.history = history;
    if view.needs_recovery {
        state.status = "该周目需要恢复；按 R 尝试继续，不会重复发送已提交请求。".into();
    }
}

fn record_text(body: &serde_json::Value) -> Option<String> {
    body.get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            body.get("input")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            body.get("action")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &ViewState) {
    frame.render_widget(
        Block::default().style(Style::default().bg(COLOR_BACKGROUND)),
        frame.area(),
    );
    if state.session.is_none() {
        draw_selection(frame, state);
    } else {
        draw_game(frame, state);
    }
    if state.settings_open {
        draw_settings(frame, state);
    }
}

fn draw_selection(frame: &mut ratatui::Frame<'_>, state: &ViewState) {
    let chunks = selection_layout(frame.area());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " AOIDOS ",
                    Style::default()
                        .fg(COLOR_BACKGROUND)
                        .bg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "  故事从你的选择开始",
                    Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(
                "选择模型与剧本；打开周目不会自动发送请求。  S 设置",
                Style::default().fg(COLOR_MUTED),
            )),
        ])
        .block(Block::default().style(Style::default().bg(COLOR_BACKGROUND))),
        chunks[0],
    );
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);
    let profile_lines = if state.profiles.is_empty() {
        vec![Line::styled(
            "尚无模型配置，请先运行 `aoidos profiles`。",
            Style::default().fg(COLOR_WARNING),
        )]
    } else {
        state
            .profiles
            .iter()
            .enumerate()
            .map(|(index, profile)| {
                let marker = if index == state.profile_index {
                    "› "
                } else {
                    "  "
                };
                let text = format!(
                    "{marker}{} · {} / {}",
                    profile.profile_id, profile.provider_id, profile.model
                );
                Line::styled(
                    text,
                    row_style(index == state.profile_index, state.focus == Focus::Profile),
                )
            })
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        Paragraph::new(profile_lines)
            .block(panel_block(
                &format!(
                    "模型  {}/{}",
                    state
                        .profile_index
                        .saturating_add(1)
                        .min(state.profiles.len()),
                    state.profiles.len()
                ),
                state.focus == Focus::Profile,
            ))
            .wrap(Wrap { trim: true }),
        columns[0],
    );
    let script_lines = if state.scripts.is_empty() {
        vec![Line::styled(
            "当前没有可用剧本。",
            Style::default().fg(COLOR_WARNING),
        )]
    } else {
        state
            .scripts
            .iter()
            .enumerate()
            .map(|(index, script)| {
                let marker = if index == state.script_index {
                    "› "
                } else {
                    "  "
                };
                let title = script
                    .display_names
                    .get(state.locale)
                    .unwrap_or(&script.title);
                let text = format!("{marker}{title} · {}", script.script_id);
                Line::styled(
                    text,
                    row_style(index == state.script_index, state.focus == Focus::Script),
                )
            })
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        Paragraph::new(script_lines)
            .block(panel_block(
                &format!(
                    "剧本  {}/{}",
                    state
                        .script_index
                        .saturating_add(1)
                        .min(state.scripts.len()),
                    state.scripts.len()
                ),
                state.focus == Focus::Script,
            ))
            .wrap(Wrap { trim: true }),
        columns[1],
    );
    let actions = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "  Enter  打开 / 继续当前周目  ",
            button_style(),
        )]))
        .block(panel_block("开始", false)),
        actions[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "  N  新建周目  ",
            button_style(),
        )]))
        .block(panel_block("新游戏", false)),
        actions[1],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                state.status.clone(),
                Style::default().fg(COLOR_TEXT),
            )),
            Line::from(Span::styled(
                "↑↓ 选择 · Tab 切换 · Enter 打开 · N 新建 · Esc 退出",
                Style::default().fg(COLOR_MUTED),
            )),
        ])
        .block(panel_block("操作指南", false)),
        chunks[3],
    );
}

fn draw_game(frame: &mut ratatui::Frame<'_>, state: &ViewState) {
    let session = state.session.as_ref().expect("游戏界面始终有活动会话");
    let chunks = game_layout(frame.area());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ◉ ",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                session.title.clone(),
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ·  {}", session.script_id),
                Style::default().fg(COLOR_MUTED),
            ),
        ]))
        .style(Style::default().bg(COLOR_PANEL)),
        chunks.header,
    );
    frame.render_widget(
        Paragraph::new("S 设置")
            .alignment(Alignment::Center)
            .style(Style::default().fg(COLOR_ACCENT).bg(COLOR_PANEL))
            .block(Block::default().style(Style::default().bg(COLOR_PANEL)))
            .style(Style::default().fg(COLOR_ACCENT).bg(COLOR_PANEL)),
        chunks.settings_button,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            phase_label(state),
            Style::default()
                .fg(phase_color(state))
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center)
        .style(Style::default().bg(COLOR_PANEL)),
        chunks.phase,
    );
    let mut transcript = state.history.iter().cloned().collect::<Vec<_>>();
    if !state.stream.is_empty() {
        transcript.push(format!("AI：{}", state.stream));
    }
    let content_width = usize::from(chunks.transcript.width.saturating_sub(4)).max(1);
    let visible_count = usize::from(chunks.transcript.height.saturating_sub(2)).max(1);
    let mut visual_lines = Vec::new();
    for (index, text) in transcript.iter().enumerate() {
        let is_stream = !state.stream.is_empty() && index + 1 == transcript.len();
        visual_lines.extend(
            wrap_visual_lines(text, content_width)
                .into_iter()
                .map(|line| (line, is_stream)),
        );
    }
    let offset = state
        .history_scroll
        .min(visual_lines.len().saturating_sub(visible_count));
    let visual_end = visual_lines.len().saturating_sub(offset);
    let visual_start = visual_end.saturating_sub(visible_count);
    let lines = if visual_lines.is_empty() {
        vec![Line::from(vec![
            Span::styled(
                "暂时还没有对话。",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  在下方写下行动，提交前会先确认预算。",
                Style::default().fg(COLOR_MUTED),
            ),
        ])]
    } else {
        visual_lines[visual_start..visual_end]
            .iter()
            .map(|(text, is_stream)| {
                if *is_stream {
                    Line::styled(text.clone(), Style::default().fg(COLOR_SECONDARY))
                } else if let Some((speaker, body)) = text.split_once('：') {
                    let color = if speaker == "你" {
                        COLOR_ACCENT
                    } else {
                        COLOR_TEXT
                    };
                    Line::from(vec![
                        Span::styled(
                            format!("{speaker}："),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(body.to_owned(), Style::default().fg(COLOR_TEXT)),
                    ])
                } else {
                    Line::styled(text.clone(), Style::default().fg(COLOR_MUTED))
                }
            })
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        {
            let block = panel_block("故事记录", false);
            Paragraph::new(lines).block(block)
        },
        chunks.transcript,
    );
    let billing_lines = billing_lines(
        state,
        usize::from(chunks.billing.height.saturating_sub(2)),
        usize::from(chunks.billing.width.saturating_sub(4)),
    );
    frame.render_widget(
        Paragraph::new(billing_lines).block(panel_block(
            &billing_title(state),
            state.focus == Focus::Billing,
        )),
        chunks.billing,
    );
    let controls = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(78), Constraint::Percentage(22)])
        .split(chunks.controls);
    let input_style = if state.focus == Focus::Input {
        Style::default().fg(COLOR_TEXT)
    } else {
        Style::default()
    };
    let input = if state.draft.is_empty() {
        state
            .scripts
            .iter()
            .find(|script| script.script_id == session.script_id)
            .and_then(|script| script.suggested_actions.get(state.locale))
            .map(|suggestion| {
                format!("{suggestion}\n\n剧本建议 · 可编辑；Enter 提交前会显示费用预览")
            })
            .unwrap_or_else(|| "描述你接下来要做的事…".into())
    } else {
        state.draft.clone()
    };
    let input_width = usize::from(controls[0].width.saturating_sub(4)).max(1);
    let input_lines = wrap_visual_lines(&input, input_width)
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    let (cursor_column, cursor_row) = draft_cursor_position(&state.draft, input_width);
    let input_height = controls[0].height.saturating_sub(2).max(1);
    let cursor_row = cursor_row.min(u16::MAX as usize) as u16;
    let input_scroll = cursor_row.saturating_sub(input_height.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(input_lines)
            .style(input_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(if state.focus == Focus::Input {
                        COLOR_ACCENT
                    } else {
                        COLOR_BORDER
                    })
                    .style(Style::default().fg(COLOR_TEXT).bg(COLOR_PANEL))
                    .title(Span::styled(
                        " 行动  ·  Shift+Enter 换行 ",
                        Style::default()
                            .fg(COLOR_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .padding(Padding::horizontal(1)),
            )
            .scroll((input_scroll, 0)),
        controls[0],
    );
    frame.render_widget(
        Paragraph::new(if state.busy {
            vec![
                Line::from(Span::styled(
                    "正在生成",
                    Style::default()
                        .fg(COLOR_WARNING)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled("X 取消", Style::default().fg(COLOR_MUTED))),
            ]
        } else if let Some(pending) = &state.pending_action {
            let label = match pending.kind {
                PendingActionKind::SubmitInput(_) => "确认发送",
                PendingActionKind::Continue => "确认继续",
            };
            vec![
                Line::from(Span::styled(label, button_style())),
                Line::from(Span::styled("Enter / Y", Style::default().fg(COLOR_MUTED))),
            ]
        } else {
            vec![
                Line::from(Span::styled("提交行动", button_style())),
                Line::from(Span::styled(
                    "Enter · 预览费用",
                    Style::default().fg(COLOR_MUTED),
                )),
            ]
        })
        .alignment(Alignment::Center)
        .block(panel_block(
            if state.busy { "处理中" } else { "行动" },
            state.focus == Focus::Submit,
        )),
        controls[1],
    );
    let continue_label = if state.busy {
        "X：取消当前回合"
    } else if state
        .phase
        .as_ref()
        .is_some_and(|phase| phase.state.check.is_some())
    {
        "R / 点击：完成骰判"
    } else if state
        .phase
        .as_ref()
        .is_some_and(|phase| phase.state.resume_required || phase.state.needs_recovery)
    {
        "R / 点击：恢复继续"
    } else {
        "没有待恢复操作"
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "● ",
                Style::default().fg(if state.busy {
                    COLOR_WARNING
                } else {
                    COLOR_ACCENT
                }),
            ),
            Span::styled(state.status.clone(), Style::default().fg(COLOR_TEXT)),
            Span::styled(
                format!("  ·  {continue_label}"),
                Style::default().fg(if state.focus == Focus::Continue {
                    COLOR_ACCENT
                } else {
                    COLOR_MUTED
                }),
            ),
            Span::styled(
                "  ·  PgUp/PgDn 翻阅 · Esc 退出",
                Style::default().fg(COLOR_MUTED),
            ),
        ]))
        .style(Style::default().bg(COLOR_PANEL)),
        chunks.status,
    );
    if !state.settings_open && controls[0].width >= 5 && controls[0].height >= 3 {
        frame.set_cursor_position(ratatui::layout::Position::new(
            controls[0]
                .x
                .saturating_add(2 + cursor_column.min(u16::MAX as usize) as u16),
            controls[0]
                .y
                .saturating_add(1 + cursor_row.saturating_sub(input_scroll)),
        ));
    }
}

fn draw_settings(frame: &mut ratatui::Frame<'_>, state: &ViewState) {
    let layout = settings_layout(frame.area());
    frame.render_widget(Clear, layout.panel);
    frame.render_widget(
        Block::default()
            .title(Span::styled(
                " 设置 ",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(COLOR_ACCENT)
            .style(Style::default().fg(COLOR_TEXT).bg(COLOR_PANEL)),
        layout.panel,
    );
    for (section, area, label) in [
        (SettingsSection::Model, layout.model_tab, "1 模型"),
        (SettingsSection::Script, layout.script_tab, "2 剧本"),
        (SettingsSection::Billing, layout.billing_tab, "3 费用"),
    ] {
        frame.render_widget(
            Paragraph::new(label).alignment(Alignment::Center).style(
                if state.settings_section == section {
                    Style::default()
                        .fg(COLOR_BACKGROUND)
                        .bg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(COLOR_MUTED).bg(COLOR_PANEL_SELECTED)
                },
            ),
            area,
        );
    }
    frame.render_widget(
        Paragraph::new("×")
            .alignment(Alignment::Center)
            .style(Style::default().fg(COLOR_MUTED).bg(COLOR_PANEL)),
        layout.close,
    );

    let body = match state.settings_section {
        SettingsSection::Model => {
            let current_id = state
                .session_profile
                .as_ref()
                .map(|profile| profile.profile_id.as_str());
            let mut lines = vec![Line::styled(
                "所选模型只用于新周目；当前周目继续使用开局时冻结的配置。",
                Style::default().fg(COLOR_MUTED),
            )];
            if state.profiles.is_empty() {
                lines.push(Line::styled(
                    "没有可用模型配置。",
                    Style::default().fg(COLOR_WARNING),
                ));
            } else {
                lines.extend(state.profiles.iter().enumerate().map(|(index, profile)| {
                    let selected = index == state.profile_index;
                    let current = current_id == Some(profile.profile_id.as_str());
                    let marker = if selected { "› " } else { "  " };
                    let suffix = if current { "  · 当前周目" } else { "" };
                    Line::styled(
                        format!(
                            "{marker}{} · {}/{} · max {}{}",
                            profile.profile_id,
                            profile.provider_id,
                            profile.model,
                            profile.sampling.max_tokens,
                            suffix,
                        ),
                        row_style(selected, true),
                    )
                }));
            }
            lines
        }
        SettingsSection::Script => {
            let current_script = state
                .session
                .as_ref()
                .map(|session| session.script_id.as_str());
            let mut lines = vec![Line::styled(
                "所选剧本只用于新周目；当前周目不会切换剧本。",
                Style::default().fg(COLOR_MUTED),
            )];
            if state.scripts.is_empty() {
                lines.push(Line::styled(
                    "没有可用剧本。",
                    Style::default().fg(COLOR_WARNING),
                ));
            } else {
                lines.extend(state.scripts.iter().enumerate().map(|(index, script)| {
                    let selected = index == state.script_index;
                    let current = current_script == Some(script.script_id.as_str());
                    let title = script
                        .display_names
                        .get(state.locale)
                        .unwrap_or(&script.title);
                    Line::styled(
                        format!(
                            "{}{} · {}{}",
                            if selected { "› " } else { "  " },
                            title,
                            script.script_id,
                            if current { "  · 当前周目" } else { "" },
                        ),
                        row_style(selected, true),
                    )
                }));
            }
            lines
        }
        SettingsSection::Billing => {
            let width = usize::from(layout.body.width.saturating_sub(1)).max(1);
            let mut lines = settings_billing_info_lines(width);
            let available = usize::from(layout.body.height).saturating_sub(lines.len());
            lines.extend(billing_lines(state, available, width));
            lines
        }
    };
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::default().fg(COLOR_TEXT).bg(COLOR_PANEL))
            .wrap(Wrap { trim: true }),
        layout.body,
    );

    let footer = if state.busy || state.pending_action.is_some() {
        Line::from(vec![
            Span::styled("新周目暂不可用", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "  ·  当前操作结束后再试  ·  F2 / Esc 返回",
                Style::default().fg(COLOR_WARNING),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("N  新建周目", button_style()),
            Span::styled(
                "  ·  保存选择并开始  ·  Tab 切页  ·  F2 / Esc 返回",
                Style::default().fg(COLOR_MUTED),
            ),
        ])
    };
    frame.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .block(panel_block("", false)),
        layout.new_run,
    );
}

fn draft_cursor_position(draft: &str, width: usize) -> (usize, usize) {
    let width = width.max(1);
    let mut column = 0usize;
    let mut row = 0usize;
    for (line_index, line) in draft.split('\n').enumerate() {
        if line_index > 0 {
            row = row.saturating_add(1);
            column = 0;
        }
        for character in line.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if column > 0 && column.saturating_add(character_width) > width {
                row = row.saturating_add(1);
                column = 0;
            }
            column = column.saturating_add(character_width);
        }
    }
    if column >= width {
        row = row.saturating_add(1);
        column = 0;
    }
    (column, row)
}

#[cfg(test)]
fn draft_cursor_column(draft: &str, area: Rect) -> usize {
    draft_cursor_position(draft, usize::from(area.width.saturating_sub(4)).max(1)).0
}

#[cfg(test)]
fn draft_cursor_row(draft: &str, area: Rect) -> usize {
    draft_cursor_position(draft, usize::from(area.width.saturating_sub(4)).max(1)).1
}

fn wrap_visual_lines(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for source_line in text.split('\n') {
        let mut line = String::new();
        let mut line_width = 0;
        for character in source_line.chars() {
            let char_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if line_width > 0 && line_width + char_width > width {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
            line.push(character);
            line_width += char_width;
        }
        if !line.is_empty() || source_line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

fn row_style(selected: bool, focused: bool) -> Style {
    if selected && focused {
        Style::default()
            .fg(COLOR_BACKGROUND)
            .bg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default()
            .fg(COLOR_SECONDARY)
            .bg(COLOR_PANEL_SELECTED)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_TEXT)
    }
}

fn button_style() -> Style {
    Style::default()
        .fg(COLOR_BACKGROUND)
        .bg(COLOR_ACCENT)
        .add_modifier(Modifier::BOLD)
}

fn panel_block(title: &str, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused { COLOR_ACCENT } else { COLOR_BORDER })
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_PANEL))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(if focused { COLOR_ACCENT } else { COLOR_MUTED })
                .add_modifier(Modifier::BOLD),
        ))
}

fn phase_label(state: &ViewState) -> &'static str {
    if state.busy {
        "生成中"
    } else if state
        .phase
        .as_ref()
        .is_some_and(|phase| phase.state.check.is_some())
    {
        "等待判定"
    } else if state
        .phase
        .as_ref()
        .is_some_and(|phase| phase.state.resume_required || phase.state.needs_recovery)
    {
        "需要恢复"
    } else if state.phase.is_some() {
        "等待行动"
    } else {
        "读取中"
    }
}

fn phase_color(state: &ViewState) -> Color {
    if state.busy {
        COLOR_WARNING
    } else if state.phase.as_ref().is_some_and(|phase| {
        phase.state.check.is_some() || phase.state.resume_required || phase.state.needs_recovery
    }) {
        COLOR_SECONDARY
    } else {
        COLOR_ACCENT
    }
}

fn selection_layout(area: Rect) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(area)
        .to_vec()
}

#[derive(Clone, Copy)]
struct GameLayout {
    header: Rect,
    settings_button: Rect,
    phase: Rect,
    transcript: Rect,
    controls: Rect,
    billing: Rect,
    status: Rect,
}

#[derive(Clone, Copy)]
struct SettingsLayout {
    panel: Rect,
    model_tab: Rect,
    script_tab: Rect,
    billing_tab: Rect,
    close: Rect,
    body: Rect,
    new_run: Rect,
}

fn settings_layout(area: Rect) -> SettingsLayout {
    let width = area.width.saturating_sub(4).min(92).max(area.width.min(24));
    let height = area
        .height
        .saturating_sub(2)
        .min(24)
        .max(area.height.min(8));
    let panel = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let inner = Block::default().borders(Borders::ALL).inner(panel);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(inner);
    let tabs = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(10),
        ])
        .split(sections[0]);
    SettingsLayout {
        panel,
        model_tab: tabs[0],
        script_tab: tabs[1],
        billing_tab: tabs[2],
        close: tabs[3],
        body: sections[1],
        new_run: sections[2],
    }
}

fn next_settings_section(section: SettingsSection, forward: bool) -> SettingsSection {
    match (section, forward) {
        (SettingsSection::Model, true) | (SettingsSection::Billing, false) => {
            SettingsSection::Script
        }
        (SettingsSection::Script, true) => SettingsSection::Billing,
        (SettingsSection::Script, false) => SettingsSection::Model,
        (SettingsSection::Billing, true) => SettingsSection::Model,
        (SettingsSection::Model, false) => SettingsSection::Billing,
    }
}

fn game_layout(area: Rect) -> GameLayout {
    let wide = area.width >= 120;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if wide {
            [
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(5),
                Constraint::Length(3),
                Constraint::Length(0),
            ]
        } else {
            [
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(5),
                Constraint::Length(7),
                Constraint::Length(3),
            ]
        })
        .split(area);
    let header_columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(24)])
        .split(chunks[0]);
    let header_actions = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(10), Constraint::Length(14)])
        .split(header_columns[1]);
    let (transcript, billing) = if wide {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(chunks[1]);
        (columns[0], columns[1])
    } else {
        (chunks[1], chunks[3])
    };
    GameLayout {
        header: header_columns[0],
        settings_button: header_actions[0],
        phase: header_actions[1],
        transcript,
        controls: chunks[2],
        billing,
        status: if wide { chunks[3] } else { chunks[4] },
    }
}

fn billing_content_lines(state: &ViewState, width: usize) -> Vec<Line<'static>> {
    if let Some(error) = &state.billing_error {
        return wrap_display_lines(error, width);
    }
    let Some(summary) = &state.billing else {
        return wrap_display_lines("打开周目后显示模型用量与费用。", width);
    };
    let mut lines = Vec::new();
    if let (Some(profile), Some(preflight)) = (
        state
            .session_profile
            .as_ref()
            .or_else(|| state.profiles.get(state.profile_index)),
        state.billing_preflight.as_ref(),
    ) {
        lines.extend(wrap_display_lines(
            &format!(
                "模型：{}/{} · 输出 ≤ {} Token",
                profile.provider_id, profile.model, profile.sampling.max_tokens
            ),
            width,
        ));
        if let Some(price) = &preflight.selected_price {
            lines.extend(wrap_display_lines(
                &format!(
                    "价格：{} · {}/{} · 版本 {}",
                    price.currency, price.provider_id, price.model_id, price.version_id
                ),
                width,
            ));
        } else {
            lines.extend(wrap_display_lines("价格：未登记，不可发送", width));
        }
        if let Some(budget) = &preflight.budget {
            lines.extend(wrap_display_lines(
                &format!(
                    "本局上限：CNY {} / USD {}",
                    format_money(budget.cny_limit.nanos()),
                    format_money(budget.usd_limit.nanos())
                ),
                width,
            ));
        } else {
            lines.extend(wrap_display_lines("本局上限：未设置，不可发送", width));
        }
    }
    for warning in summary.warnings.iter().filter(|warning| !warning.read) {
        lines.extend(wrap_display_lines(
            &format!(
                "预算提醒：{} 已达到 {}%",
                warning.currency, warning.threshold_percent
            ),
            width,
        ));
    }
    for item in &summary.models {
        let settled = item
            .settled_amount
            .map(|amount| format_money(amount.nanos()))
            .unwrap_or_else(|| "—".into());
        let reserved = item
            .reserved_amount
            .map(|amount| format_money(amount.nanos()))
            .unwrap_or_else(|| "—".into());
        let tokens = item
            .known_tokens
            .map(|tokens| tokens.to_string())
            .unwrap_or_else(|| "—".into());
        let text = format!(
            "{}/{} {} 结算{} 预留{} tok:{} 请求:{}{}",
            item.provider_id,
            item.model_id,
            item.currency,
            settled,
            reserved,
            tokens,
            item.request_count,
            if item.incomplete_requests > 0 {
                format!(" 未知:{}", item.incomplete_requests)
            } else {
                String::new()
            }
        );
        lines.extend(wrap_display_lines(&text, width));
    }
    if lines.is_empty() {
        lines.push(Line::from("本局尚无模型请求。"));
    }
    lines
}

fn billing_lines(state: &ViewState, max_lines: usize, width: usize) -> Vec<Line<'static>> {
    let lines = billing_content_lines(state, width);
    let start = state
        .billing_scroll
        .min(lines.len().saturating_sub(max_lines));
    lines.into_iter().skip(start).take(max_lines).collect()
}

fn billing_title(state: &ViewState) -> String {
    let _ = state;
    "本局费用 · PgUp/PgDn 滚动".into()
}

fn settings_billing_info_lines(width: usize) -> Vec<Line<'static>> {
    wrap_display_lines("账单只读；编辑预算和价格请使用桌面端设置。", width)
}

fn wrap_display_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    wrap_visual_lines(text, width)
        .into_iter()
        .map(Line::from)
        .collect()
}

fn format_money(nanos: i64) -> String {
    let cents = nanos.saturating_add(5_000_000) / 10_000_000;
    format!("{}.{:02}", cents / 100, cents % 100)
}

fn app_screen_area() -> Rect {
    ratatui::layout::Rect::new(
        0,
        0,
        crossterm::terminal::size().map_or(80, |(width, _)| width),
        crossterm::terminal::size().map_or(24, |(_, height)| height),
    )
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && x < area.x.saturating_add(area.width)
        && y >= area.y
        && y < area.y.saturating_add(area.height)
}

fn row_index(area: Rect, y: u16, len: usize) -> usize {
    usize::from(y.saturating_sub(area.y).saturating_sub(1)).min(len.saturating_sub(1))
}

fn move_index(index: usize, len: usize, key: KeyCode) -> usize {
    if len == 0 {
        return 0;
    }
    match key {
        KeyCode::Up => index.saturating_sub(1),
        KeyCode::Down => (index + 1).min(len - 1),
        _ => index,
    }
}

fn terminal_locale() -> &'static str {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(std::env::var_os)
        .and_then(|value| value.into_string().ok())
        .unwrap_or_default();
    if locale.to_ascii_lowercase().starts_with("zh") {
        "zh-Hans"
    } else {
        "en"
    }
}

fn display_fault(fault: aoidos_engine::fault::Fault) -> io::Error {
    io::Error::other(friendly_error(
        &fault.code,
        &fault.message,
        terminal_locale(),
    ))
}

fn friendly_error(code: &str, message: &str, locale: &str) -> String {
    let english = locale == "en";
    match code {
        "budget.exceeded" if english => {
            "The run budget is insufficient. Check the limit for this currency in Billing settings.".into()
        }
        "budget.exceeded" => "本局预算不足，请在费用设置中检查对应币种的上限。".into(),
        "budget.price-missing" if english => {
            "No price is registered for this model. Register its price before sending a request.".into()
        }
        "budget.price-missing" => "尚未登记所选模型的价格，请先完成价格登记。".into(),
        "budget.run-limit-missing" if english => {
            "The run limit is not set. Set a budget before starting a request.".into()
        }
        "budget.run-limit-missing" => "本局费用上限尚未设置，请先完成预算设置。".into(),
        "budget.settlement-backlog" if english => {
            "The pending settlement queue is full. Restore pending items in Billing before retrying.".into()
        }
        "budget.settlement-backlog" => {
            "待结算队列已满，请先在费用页面恢复待结算项目，再重试。".into()
        }
        "llm.missing-key" if english => {
            "The selected model profile has no API key. Add one in Model settings.".into()
        }
        "llm.missing-key" => "所选模型配置缺少 API 密钥，请在模型设置中补充密钥。".into(),
        _ => message.to_owned(),
    }
}

struct TerminalSession;

impl TerminalSession {
    fn start() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(
            io::stdout(),
            EnterAlternateScreen,
            crossterm::event::EnableMouseCapture
        ) {
            let _ = disable_raw_mode();
            let _ = execute!(
                io::stdout(),
                crossterm::event::DisableMouseCapture,
                LeaveAlternateScreen
            );
            return Err(error);
        }
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture
        );
    }
}

#[cfg(test)]
mod tests;
