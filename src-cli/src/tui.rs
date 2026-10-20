//! 终端视图只把键盘 / 鼠标转换成操作；游戏状态仍由共享 app 与 engine 所有。

use crate::{CliApp, events::RuntimeEvent};
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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::{collections::VecDeque, io, time::Duration};
use unicode_width::UnicodeWidthChar;

const MAX_TRANSCRIPT_LINES: usize = 80;
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;
const MAX_STREAM_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Profile,
    Script,
    Input,
    Submit,
    Continue,
}

struct ViewState {
    profiles: Vec<LlmProfile>,
    scripts: Vec<ScriptInfo>,
    profile_index: usize,
    script_index: usize,
    session: Option<OpenedSession>,
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
    locale: &'static str,
}

impl ViewState {
    fn new(profiles: Vec<LlmProfile>, scripts: Vec<ScriptInfo>) -> Self {
        Self {
            profiles,
            scripts,
            profile_index: 0,
            script_index: 0,
            session: None,
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
            if state.session.is_none() {
                if handle_selection_key(app, state, key).await? {
                    return Ok(true);
                }
            } else if handle_game_key(app, state, key).await? {
                return Ok(true);
            }
        }
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if state.session.is_none() {
                    handle_selection_click(app, state, mouse.column, mouse.row).await?;
                } else {
                    handle_game_click(app, state, mouse.column, mouse.row).await?;
                }
            }
            MouseEventKind::ScrollUp if state.session.is_some() => {
                state.history_scroll = state.history_scroll.saturating_add(3);
            }
            MouseEventKind::ScrollDown if state.session.is_some() => {
                state.history_scroll = state.history_scroll.saturating_sub(3);
            }
            _ => {}
        },
        Event::Resize(_, _) => {}
        _ => {}
    }
    Ok(false)
}

async fn handle_selection_key(
    app: &mut CliApp,
    state: &mut ViewState,
    key: KeyEvent,
) -> Result<bool, Box<dyn std::error::Error>> {
    match key.code {
        KeyCode::Esc => return Ok(true),
        KeyCode::Char('q') if state.focus != Focus::Input => return Ok(true),
        KeyCode::Tab | KeyCode::BackTab => {
            state.focus = match state.focus {
                Focus::Profile => Focus::Script,
                Focus::Script | Focus::Submit | Focus::Continue => Focus::Profile,
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
    let Some(profile) = state.profiles.get(state.profile_index) else {
        state.status = "没有可用 profile。".into();
        return Ok(());
    };
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
            state.phase = app.services.runtime.get_phase(&session.session_id).ok();
            load_history(app, state, &session.session_id);
            state.session = Some(session);
            state.focus = Focus::Input;
            if state.draft.is_empty()
                && let Some(suggestion) = state.suggested_action().map(str::to_owned)
            {
                state.draft = suggestion;
            }
        }
        Err(error) => state.status = format!("{}：{}", error.code, error.message),
    }
    Ok(())
}

async fn handle_game_key(
    app: &mut CliApp,
    state: &mut ViewState,
    key: KeyEvent,
) -> Result<bool, Box<dyn std::error::Error>> {
    match key.code {
        KeyCode::Esc => return Ok(true),
        KeyCode::Char('q') if state.focus != Focus::Input => return Ok(true),
        KeyCode::PageUp => state.history_scroll = state.history_scroll.saturating_add(10),
        KeyCode::PageDown => state.history_scroll = state.history_scroll.saturating_sub(10),
        KeyCode::Char('x') | KeyCode::Char('X') if state.busy => {
            cancel_game(app, state).await;
        }
        KeyCode::Tab | KeyCode::BackTab => {
            state.focus = match state.focus {
                Focus::Input => Focus::Submit,
                Focus::Submit => Focus::Continue,
                Focus::Continue | Focus::Profile | Focus::Script => Focus::Input,
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
        .split(chunks[2]);
    if contains(controls[0], x, y) {
        state.focus = Focus::Input;
    } else if contains(controls[1], x, y) {
        submit_action(app, state).await;
    } else if contains(chunks[3], x, y) {
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
    let Some(session) = state.session.as_ref() else {
        return;
    };
    let action = std::mem::take(&mut state.draft);
    match app
        .services
        .runtime
        .submit_input(&session.session_id, action.clone())
        .await
    {
        Ok(_) => {
            state.add_history(format!("你：{action}"));
            state.busy = true;
            state.stream.clear();
            state.active_turn_id = None;
            state.stream_finalized = false;
            state.status = "已提交，正在等待模型响应…".into();
            refresh_phase(app, state);
        }
        Err(error) => {
            state.draft = action;
            state.status = format!("{}：{}", error.code, error.message);
        }
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
        Err(error) => state.status = format!("{}：{}", error.code, error.message),
    }
}

async fn continue_game(app: &mut CliApp, state: &mut ViewState) {
    let Some(session) = state.session.as_ref() else {
        return;
    };
    let id = session.session_id.clone();
    let phase = match app.services.runtime.get_phase(&id) {
        Ok(phase) => phase,
        Err(error) => {
            state.status = format!("{}：{}", error.code, error.message);
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
        Err(error) => state.status = format!("{}：{}", error.code, error.message),
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
                state.status = format!("{code}：{message}");
                refresh_phase(app, state);
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
                state.status = format!("{}：{}", error.code, error.message);
            }
        }
        RuntimeEvent::Record(event) => {
            if state
                .session
                .as_ref()
                .is_some_and(|session| session.session_id == event.session_id)
            {
                load_history(app, state, &event.session_id);
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
            state.status = format!("{}：{}", error.code, error.message);
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
    if state.session.is_none() {
        draw_selection(frame, state);
    } else {
        draw_game(frame, state);
    }
}

fn draw_selection(frame: &mut ratatui::Frame<'_>, state: &ViewState) {
    let chunks = selection_layout(frame.area());
    frame.render_widget(
        Paragraph::new("Aoidos · 终端游戏")
            .block(Block::default().borders(Borders::ALL).title("欢迎"))
            .style(Style::default().fg(Color::Cyan)),
        chunks[0],
    );
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);
    let profile_lines = state
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
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(profile_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("模型 profile（↑↓ 选择）"),
            )
            .wrap(Wrap { trim: true }),
        columns[0],
    );
    let script_lines = state
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
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(script_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("剧本（←→ 切换）"),
            )
            .wrap(Wrap { trim: true }),
        columns[1],
    );
    let actions = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            " Enter  打开 / 继续当前周目 ",
            button_style(),
        )]))
        .block(Block::default().borders(Borders::ALL)),
        actions[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            " N  新建周目 ",
            button_style(),
        )]))
        .block(Block::default().borders(Borders::ALL)),
        actions[1],
    );
    frame.render_widget(
        Paragraph::new(format!("{}    Tab / 鼠标可选择，Esc 退出", state.status))
            .block(Block::default().borders(Borders::ALL).title("提示")),
        chunks[3],
    );
}

fn draw_game(frame: &mut ratatui::Frame<'_>, state: &ViewState) {
    let session = state.session.as_ref().expect("游戏界面始终有活动会话");
    let chunks = game_layout(frame.area());
    let phase = state
        .phase
        .as_ref()
        .map(|phase| format!("{:?}", phase.state.phase))
        .unwrap_or_else(|| "读取中".into());
    frame.render_widget(
        Paragraph::new(format!(
            "{} · {} · {}",
            session.title, session.script_id, phase
        ))
        .block(Block::default().borders(Borders::ALL).title("Aoidos")),
        chunks[0],
    );
    let mut transcript = state.history.iter().cloned().collect::<Vec<_>>();
    if !state.stream.is_empty() {
        transcript.push(format!("AI：{}", state.stream));
    }
    let content_width = usize::from(chunks[1].width.saturating_sub(2)).max(1);
    let visible_count = usize::from(chunks[1].height.saturating_sub(2)).max(1);
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
    let lines = visual_lines[visual_start..visual_end]
        .iter()
        .map(|(text, is_stream)| {
            if *is_stream {
                Line::styled(text.clone(), Style::default().fg(Color::Cyan))
            } else {
                Line::from(text.clone())
            }
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        {
            let block = Block::default().borders(Borders::ALL).title("对话与记录");
            Paragraph::new(lines).block(block)
        },
        chunks[1],
    );
    let controls = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(78), Constraint::Percentage(22)])
        .split(chunks[2]);
    let input_style = if state.focus == Focus::Input {
        Style::default().fg(Color::Yellow)
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
                format!("{suggestion}\n\n（剧本建议已填入；可编辑，按 Enter 才会发送）")
            })
            .unwrap_or_else(|| "输入行动…".into())
    } else {
        state.draft.clone()
    };
    frame.render_widget(
        Paragraph::new(input)
            .style(input_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("行动（Shift+Enter 换行）"),
            )
            .wrap(Wrap { trim: true }),
        controls[0],
    );
    frame.render_widget(
        Paragraph::new(if state.busy {
            "等待响应…"
        } else {
            "提交行动"
        })
        .style(button_style())
        .block(Block::default().borders(Borders::ALL).border_style(
            if state.focus == Focus::Submit {
                input_style
            } else {
                Style::default()
            },
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
        Paragraph::new(format!(
            "{}  ·  {}  ·  PgUp/PgDn 或滚轮翻阅记录 · Esc 退出",
            state.status, continue_label
        ))
        .block(Block::default().borders(Borders::ALL).title("状态"))
        .style(if state.focus == Focus::Continue {
            input_style
        } else {
            Style::default()
        }),
        chunks[3],
    );
    frame.set_cursor_position(ratatui::layout::Position::new(
        controls[0].x.saturating_add(
            1 + state
                .draft
                .chars()
                .count()
                .min(controls[0].width.saturating_sub(2) as usize) as u16,
        ),
        controls[0].y.saturating_add(1),
    ));
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
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn button_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn selection_layout(area: Rect) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(area)
        .to_vec()
}

fn game_layout(area: Rect) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(5),
            Constraint::Length(3),
        ])
        .split(area)
        .to_vec()
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
    io::Error::other(format!("{}：{}", fault.code, fault.message))
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
mod tests {
    use super::{ViewState, append_bounded, apply_turn_snapshot, wrap_visual_lines};

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
}
