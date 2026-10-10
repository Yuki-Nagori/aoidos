//! 有界 session actor；控制消息只取消网络子调用，不丢弃已经开始的存储工作。

use super::{
    domain::{FrozenRound, RoundFactory},
    execution::{Context, Runner},
    publication::Finished,
    recovery,
    reducer::Stage,
    state::*,
};
use crate::{
    fault::Fault,
    ports::Outcome,
    record::{
        facts::{ForkMode, RoundIdentity},
        format,
    },
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

type Reply<T> = oneshot::Sender<Result<T, Fault>>;
enum Command {
    Submit(String, Reply<AcceptedRound>),
    Resume(Reply<AcceptedRound>),
    Interrupt(String, String, Reply<AcceptedOperation>),
    Cancel(String, Reply<CancelledRound>),
    Check(String, String, Reply<AcceptedCheck>),
    Rewind(u64, Reply<AcceptedOperation>),
    Regenerate(String, Reply<AcceptedRound>),
    Close(oneshot::Sender<()>),
}
impl Command {
    fn round_id(&self) -> Option<&str> {
        match self {
            Self::Interrupt(round, _, _)
            | Self::Cancel(round, _)
            | Self::Check(round, _, _)
            | Self::Regenerate(round, _) => Some(round),
            _ => None,
        }
    }
    fn reject(self, fault: Fault) {
        match self {
            Self::Submit(_, reply) | Self::Resume(reply) | Self::Regenerate(_, reply) => {
                let _ = reply.send(Err(fault));
            }
            Self::Interrupt(_, _, reply) | Self::Rewind(_, reply) => {
                let _ = reply.send(Err(fault));
            }
            Self::Cancel(_, reply) => {
                let _ = reply.send(Err(fault));
            }
            Self::Check(_, _, reply) => {
                let _ = reply.send(Err(fault));
            }
            Self::Close(reply) => {
                let _ = reply.send(());
            }
        }
    }
}
struct Handle {
    context: Arc<Context>,
    commands: mpsc::Sender<Command>,
    exited: CancellationToken,
    closing: Arc<AtomicBool>,
}

impl Handle {
    fn claim_close(&self) -> Result<(), Fault> {
        if self.closing.swap(true, Ordering::AcqRel) {
            Err(closed())
        } else {
            Ok(())
        }
    }
}

/// 平台装配共享 Coordinator，可信 Rust 初始化登记 Context；Webview 不可注册解释器。
pub struct Service {
    coordinator: crate::turn::Coordinator,
    factory: Arc<dyn RoundFactory>,
    sessions: Mutex<BTreeMap<String, Arc<Handle>>>,
    closing: Arc<AtomicBool>,
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn closed() -> Fault {
    Fault::new("app.not-ready", "阶段服务已关闭")
}
fn reply_closed(_: oneshot::error::RecvError) -> Fault {
    closed()
}
fn runtime_closed(_: tokio::runtime::TryCurrentError) -> Fault {
    closed()
}
fn command_rejected(error: mpsc::error::TrySendError<Command>) -> Fault {
    match error {
        mpsc::error::TrySendError::Full(_) => Fault::busy(),
        mpsc::error::TrySendError::Closed(_) => closed(),
    }
}
async fn receive<T>(reply: oneshot::Receiver<Result<T, Fault>>) -> Result<T, Fault> {
    reply.await.map_err(reply_closed)?
}
impl Service {
    pub fn new(coordinator: crate::turn::Coordinator, factory: Arc<dyn RoundFactory>) -> Self {
        Self {
            coordinator,
            factory,
            sessions: Mutex::new(BTreeMap::new()),
            closing: Arc::new(AtomicBool::new(false)),
        }
    }
    /// 每个登记会话只有一个 actor，旧身份清退完成之前不允许重新登记。
    /// # Errors
    /// 容量、重复身份、关闭或没有 Tokio runtime 拒绝，不发送生成请求。
    pub fn register(&self, context: Arc<Context>) -> Result<(), Fault> {
        let runtime = tokio::runtime::Handle::try_current().map_err(runtime_closed)?;
        let id = context.publisher.snapshot().state.session_id;
        if !self.coordinator.shares_gate(&context.coordinator) {
            return Err(Fault::bad_request());
        }
        if !format::valid_uuid(&id) || lock(&context.session).header().session_id != id {
            return Err(Fault::bad_request());
        }
        let mut sessions = lock(&self.sessions);
        if self.closing.load(Ordering::Acquire) {
            return Err(closed());
        }
        if sessions.contains_key(&id) {
            return Err(Fault::bad_request());
        }
        if sessions.len() == 16 {
            return Err(Fault::busy());
        }
        let (commands, receiver) = mpsc::channel(32);
        let exited = CancellationToken::new();
        let terminal = context
            .publisher
            .snapshot()
            .state
            .last_operation
            .and_then(|operation| {
                let outcome = match operation.outcome {
                    OperationOutcome::Accepted => return None,
                    OperationOutcome::Completed => Outcome::Completed,
                    OperationOutcome::Cancelled => Outcome::Cancelled,
                    OperationOutcome::Failed => Outcome::Failed,
                };
                operation.round_id.map(|round| (round, outcome))
            })
            .into_iter()
            .collect();
        let closing = Arc::new(AtomicBool::new(false));
        let actor = Actor {
            closing: closing.clone(),
            app_closing: self.closing.clone(),
            context: context.clone(),
            factory: self.factory.clone(),
            receiver,
            runner: None,
            checked: VecDeque::new(),
            terminal,
            exited: exited.clone(),
        };
        runtime.spawn(actor.run());
        sessions.insert(
            id,
            Arc::new(Handle {
                context,
                commands,
                exited,
                closing,
            }),
        );
        Ok(())
    }
    fn get(&self, id: &str) -> Result<Arc<Handle>, Fault> {
        if !format::valid_uuid(id) {
            return Err(Fault::bad_request());
        }
        if self.closing.load(Ordering::Acquire) {
            return Err(closed());
        }
        let handle = lock(&self.sessions)
            .get(id)
            .cloned()
            .ok_or_else(|| Fault::new("app.not-found", "会话阶段未登记"))?;
        if handle.closing.load(Ordering::Acquire) {
            return Err(closed());
        }
        Ok(handle)
    }
    fn send(&self, id: &str, command: Command) -> Result<(), Fault> {
        self.get(id)?
            .commands
            .try_send(command)
            .map_err(command_rejected)
    }
    /// # Errors
    /// 未登记会话 / 非法身份拒绝；查询只读取原子内存副本。
    pub fn get_phase(&self, id: &str) -> Result<PhaseSnapshot, Fault> {
        Ok(self.get(id)?.context.publisher.snapshot())
    }
    /// # Errors
    /// 参数、共享门禁、配置和当前阶段错误不追加输入。
    pub async fn submit_input(&self, id: &str, text: String) -> Result<AcceptedRound, Fault> {
        super::input::parse_input(&text)?;
        let (tx, rx) = oneshot::channel();
        self.send(id, Command::Submit(text, tx))?;
        receive(rx).await
    }
    /// # Errors
    /// 无检查点、pending、共享门禁或冻结设置失败拒绝。
    pub async fn resume(&self, id: &str) -> Result<AcceptedRound, Fault> {
        let (tx, rx) = oneshot::channel();
        self.send(id, Command::Resume(tx))?;
        receive(rx).await
    }
    /// # Errors
    /// 只允许当前回合的提议 / 叙事插话；第二个交接返回 busy。
    pub async fn interrupt(
        &self,
        id: &str,
        round: &str,
        text: String,
    ) -> Result<AcceptedOperation, Fault> {
        super::input::parse_input(&text)?;
        valid_round(round)?;
        let (tx, rx) = oneshot::channel();
        self.send(id, Command::Interrupt(round.into(), text, tx))?;
        receive(rx).await
    }
    /// # Errors
    /// 未知身份拒绝；同回合重试返回缓存终态，不重新追加。
    pub async fn cancel_round(&self, id: &str, round: &str) -> Result<CancelledRound, Fault> {
        valid_round(round)?;
        let (tx, rx) = oneshot::channel();
        self.send(id, Command::Cancel(round.into(), tx))?;
        receive(rx).await
    }
    /// # Errors
    /// 过期计划拒绝；确认过的同计划在有界缓存内幂等返回。
    pub async fn submit_check(
        &self,
        id: &str,
        round: &str,
        plan: &str,
    ) -> Result<AcceptedCheck, Fault> {
        valid_round(round)?;
        if !format::valid_id(plan) {
            return Err(Fault::bad_request());
        }
        let (tx, rx) = oneshot::channel();
        self.send(id, Command::Check(round.into(), plan.into(), tx))?;
        receive(rx).await
    }
    /// # Errors
    /// 只接受合法确认边界，世界重建失败保留 pending。
    pub async fn rewind(&self, id: &str, target: u64) -> Result<AcceptedOperation, Fault> {
        if target == 0 || target > format::MAX_SEQ {
            return Err(Fault::bad_request());
        }
        let (tx, rx) = oneshot::channel();
        self.send(id, Command::Rewind(target, tx))?;
        receive(rx).await
    }
    /// # Errors
    /// 只接受最近终态，沿用叙事前的 check / skip，不重掷。
    pub async fn regenerate(&self, id: &str, round: &str) -> Result<AcceptedRound, Fault> {
        valid_round(round)?;
        let (tx, rx) = oneshot::channel();
        self.send(id, Command::Regenerate(round.into(), tx))?;
        receive(rx).await
    }
    /// 先清退旧 owner 和事件身份，再释放登记槽；期间禁止同 session 重新打开。
    /// # Errors
    /// 未登记身份或已开始关闭拒绝；正在执行的存储工作会先结束。
    pub async fn close_session(&self, id: &str) -> Result<(), Fault> {
        let handle = self.get(id)?;
        handle.claim_close()?;
        stop(&handle).await;
        lock(&self.sessions).remove(id);
        Ok(())
    }
    /// 先停止所有游戏 owner，包括没有 HTTP 生产者的 manual 等待，再关闭协调器。
    pub async fn shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        let handles = lock(&self.sessions).values().cloned().collect::<Vec<_>>();
        for handle in &handles {
            handle.closing.store(true, Ordering::Release);
            stop(handle).await;
        }
        lock(&self.sessions).clear();
    }
}
async fn stop(handle: &Handle) {
    let (tx, rx) = oneshot::channel();
    if handle.commands.send(Command::Close(tx)).await.is_ok() {
        let _ = rx.await;
    }
    handle.exited.cancelled().await;
    handle.context.publisher.retire();
}
fn valid_round(round: &str) -> Result<(), Fault> {
    if format::valid_uuid(round) {
        Ok(())
    } else {
        Err(Fault::bad_request())
    }
}
fn push_bounded<T>(cache: &mut VecDeque<T>, value: T) {
    if cache.len() == 16 {
        cache.pop_front();
    }
    cache.push_back(value);
}
struct Actor {
    closing: Arc<AtomicBool>,
    app_closing: Arc<AtomicBool>,
    context: Arc<Context>,
    factory: Arc<dyn RoundFactory>,
    receiver: mpsc::Receiver<Command>,
    runner: Option<Runner>,
    checked: VecDeque<(String, String)>,
    terminal: VecDeque<(String, Outcome)>,
    exited: CancellationToken,
}
impl Actor {
    fn known(&self, round: &str, current: Option<&str>) -> bool {
        current == Some(round)
            || self.terminal.iter().any(|(id, _)| id == round)
            || self.checked.iter().any(|(id, _)| id == round)
            || self
                .context
                .publisher
                .snapshot()
                .state
                .last_operation
                .as_ref()
                .and_then(|operation| operation.round_id.as_deref())
                == Some(round)
    }
    fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire) || self.app_closing.load(Ordering::Acquire)
    }
    async fn freeze(&self) -> Result<FrozenRound, Fault> {
        let factory = self.factory.clone();
        let run_id = self.context.publisher.snapshot().state.session_id;
        crate::blocking::run(move || factory.freeze_for_run(&run_id)).await
    }
    async fn run(mut self) {
        loop {
            let active = self.runner.as_ref().is_some_and(|run| {
                run.has_delivery_error() || !matches!(run.stage(), Stage::Idle | Stage::Waiting)
            });
            if active {
                if self.drive().await {
                    break;
                }
            } else {
                let Some(command) = self.receiver.recv().await else {
                    break;
                };
                if self.command(command).await {
                    break;
                }
            }
        }
        if let Some(mut run) = self.runner.take() {
            let _ = run.finish(Outcome::Cancelled, None).await;
        }
        self.receiver.close();
        while let Ok(command) = self.receiver.try_recv() {
            command.reject(closed());
        }
        self.exited.cancel();
    }
    fn cached(&self, command: Command) -> Result<Command, ()> {
        match command {
            Command::Cancel(round, reply) if self.terminal.iter().any(|(id, _)| id == &round) => {
                let outcome = self
                    .terminal
                    .iter()
                    .find(|(id, _)| id == &round)
                    .expect("cached identity")
                    .1;
                let _ = reply.send(Ok(CancelledRound {
                    round_id: round,
                    outcome,
                }));
                Err(())
            }
            Command::Check(round, plan, reply)
                if self.checked.contains(&(round.clone(), plan.clone())) =>
            {
                let _ = reply.send(Ok(AcceptedCheck {
                    round_id: round,
                    plan_id: plan,
                    accepted: true,
                }));
                Err(())
            }
            command => Ok(command),
        }
    }
    async fn command(&mut self, command: Command) -> bool {
        if command
            .round_id()
            .is_some_and(|round| !self.known(round, self.runner.as_ref().map(Runner::round_id)))
        {
            command.reject(Fault::not_found());
            return false;
        }
        let Ok(command) = self.cached(command) else {
            return false;
        };
        if let Command::Close(reply) = command {
            let _ = reply.send(());
            return true;
        }
        if self.is_closing() {
            command.reject(closed());
            return false;
        }
        if let Some(run) = &mut self.runner {
            match command {
                Command::Check(round, plan, reply) if round == run.round_id() => {
                    let result = run.submit_check(&plan).map(|()| AcceptedCheck {
                        round_id: round.clone(),
                        plan_id: plan.clone(),
                        accepted: true,
                    });
                    if result.is_ok() {
                        push_bounded(&mut self.checked, (round, plan));
                    }
                    let _ = reply.send(result);
                }
                Command::Cancel(round, reply) if round == run.round_id() => {
                    let result =
                        run.finish(Outcome::Cancelled, None)
                            .await
                            .map(|()| CancelledRound {
                                round_id: round.clone(),
                                outcome: Outcome::Cancelled,
                            });
                    if result.is_ok() {
                        push_bounded(&mut self.terminal, (round, Outcome::Cancelled));
                    }
                    if let Err(error) = &result {
                        self.context.publisher.quarantine(error.clone());
                    }
                    self.runner = None;
                    let _ = reply.send(result);
                }
                Command::Cancel(_, _) | Command::Check(_, _, _) | Command::Interrupt(_, _, _) => {
                    command.reject(super::invalid_phase())
                }
                _ => command.reject(Fault::busy()),
            }
            return false;
        }
        let availability = self.context.coordinator.check_available();
        if let Err(error) = availability {
            command.reject(error);
            return false;
        }
        if self.context.publisher.snapshot().state.needs_recovery {
            command.reject(super::invalid_phase());
            return false;
        }
        match command {
            Command::Submit(text, reply) => {
                let result = self.start(Some(&text)).await;
                let _ = reply.send(result);
            }
            Command::Resume(reply) => {
                let result = self.start(None).await;
                let _ = reply.send(result);
            }
            Command::Rewind(target, reply) => {
                let result = async {
                    let lease = self.context.coordinator.acquire()?;
                    let operation = uuid::Uuid::new_v4().to_string();
                    fork(self.context.clone(), lease, operation.clone(), target, None).await?;
                    Ok(AcceptedOperation {
                        operation_id: operation,
                    })
                }
                .await;
                let _ = reply.send(result);
            }
            Command::Regenerate(round, reply) => {
                let result = async {
                    let frozen = self.freeze().await?;
                    if self.is_closing() {
                        return Err(closed());
                    }
                    let lease = self.context.coordinator.acquire()?;
                    let identity = RoundIdentity {
                        version: 1,
                        round_id: uuid::Uuid::new_v4().to_string(),
                        operation_id: uuid::Uuid::new_v4().to_string(),
                    };
                    let target = {
                        let session = lock(&self.context.session);
                        let domain = lock(&self.context.domain);
                        super::control::regeneration_target(
                            &recovery::derive(&session, domain.baseline_scene())?,
                            &round,
                        )?
                    };
                    fork(
                        self.context.clone(),
                        lease.clone(),
                        identity.operation_id.clone(),
                        target.target_seq,
                        Some(target),
                    )
                    .await?;
                    let run =
                        Runner::resume_as(self.context.clone(), lease, frozen, identity).await?;
                    let result = AcceptedRound {
                        operation_id: run.identity.operation_id.clone(),
                        round_id: run.identity.round_id.clone(),
                    };
                    self.runner = Some(run);
                    Ok(result)
                }
                .await;
                let _ = reply.send(result);
            }
            _ => command.reject(super::invalid_phase()),
        }
        false
    }
    async fn start(&mut self, input: Option<&str>) -> Result<AcceptedRound, Fault> {
        let phase = self.context.publisher.snapshot().state;
        if phase.needs_recovery || (input.is_none() && !phase.resume_required) {
            return Err(super::invalid_phase());
        }
        if input.is_some() && phase.scene.is_none() {
            return Err(Fault::new("engine.no-scene", "当前会话没有可推进场景"));
        }
        let frozen = self.freeze().await?;
        if self.is_closing() {
            return Err(closed());
        }
        let lease = self.context.coordinator.acquire()?;
        let run = if let Some(text) = input {
            Runner::accept(
                self.context.clone(),
                lease,
                frozen,
                super::input::parse_input(text)?,
            )
            .await?
        } else {
            Runner::resume(self.context.clone(), lease, frozen).await?
        };
        let result = AcceptedRound {
            operation_id: run.identity.operation_id.clone(),
            round_id: run.identity.round_id.clone(),
        };
        self.runner = Some(run);
        Ok(result)
    }
    async fn drive(&mut self) -> bool {
        let mut run = self.runner.take().expect("active runner");
        let stage = run.stage();
        let round = run.round_id().to_owned();
        let cancel = CancellationToken::new();
        let mut cancels = Vec::new();
        let mut checks = Vec::new();
        let mut interrupt = None;
        let mut close = None;
        let result = {
            let step = run.step(cancel.clone());
            tokio::pin!(step);
            loop {
                tokio::select! {
                    biased;
                    result = &mut step => break result,
                    command = self.receiver.recv() => {
                        let Some(command) = command else { cancel.cancel(); break step.await; };
                        if command.round_id().is_some_and(|id| !self.known(id, Some(&round))) {
                            command.reject(Fault::not_found()); continue;
                        }
                        let Ok(command) = self.cached(command) else { continue; };
                        match command {
                            Command::Close(reply) => { cancel.cancel(); close = Some(reply); }
                            Command::Cancel(id, reply) if id == round && interrupt.is_some() => {
                                let _ = reply.send(Err(Fault::busy()));
                            }
                            Command::Cancel(id, reply) if id == round && cancels.len() < 32 => {
                                if matches!(stage, Stage::CheckProposal | Stage::Narration | Stage::SceneProposal) { cancel.cancel(); }
                                cancels.push(reply);
                            }
                            Command::Interrupt(id, text, reply) if id == round && matches!(stage, Stage::CheckProposal | Stage::Narration) => {
                                if interrupt.is_some() || !cancels.is_empty() || close.is_some() { let _ = reply.send(Err(Fault::busy())); }
                                else { match self.freeze().await {
                                    Ok(frozen) => {
                                        // 配置冻结期间控制可能已经排队；关闭必须先于新输入落盘。
                                        for _ in 0..32 {
                                            let Ok(queued) = self.receiver.try_recv() else { break; };
                                            if queued.round_id().is_some_and(|id| !self.known(id, Some(&round))) {
                                                queued.reject(Fault::not_found()); continue;
                                            }
                                            let Ok(queued) = self.cached(queued) else { continue; };
                                            match queued {
                                                Command::Close(reply) => close = Some(reply),
                                                Command::Check(_, _, _) => queued.reject(super::invalid_phase()),
                                                _ => queued.reject(Fault::busy()),
                                            }
                                        }
                                        interrupt = Some((text, frozen, reply)); cancel.cancel();
                                    }
                                    Err(error) => { let _ = reply.send(Err(error)); }
                                } }
                            }
                            Command::Check(id, plan, reply) if id == round && stage == Stage::PlanCommit && self.context.publisher.snapshot().state.check.is_some() => {
                                // 待骰已经发布，actor 的阻塞提交回执可能尚未到达；先有界保留确认。
                                if checks.len() == 32 {let _ = reply.send(Err(Fault::busy()));} else {checks.push((plan, reply));}
                            }
                            Command::Check(_, _, _) | Command::Interrupt(_, _, _) => command.reject(super::invalid_phase()),
                            _ => command.reject(Fault::busy()),
                        }
                    }
                }
            }
        };
        if result.is_ok()
            && run.stage() == Stage::Rolling
            && let Some(plan) = run.plan_id()
        {
            let key = (round.clone(), plan.to_owned());
            // 自动 rolling 同样确认计划；固定 effect 回执只会触发一次。
            push_bounded(&mut self.checked, key);
        }
        for (plan, reply) in checks {
            let result = if !cancels.is_empty() || close.is_some() || self.is_closing() {
                Err(Fault::busy())
            } else if let Err(error) = &result {
                Err(error.clone())
            } else {
                let key = (round.clone(), plan.clone());
                if self.checked.contains(&key) {
                    Ok(())
                } else {
                    let confirmed = run.submit_check(&plan);
                    if confirmed.is_ok() {
                        push_bounded(&mut self.checked, key);
                    }
                    confirmed
                }
            };
            let _ = reply.send(result.map(|()| AcceptedCheck {
                round_id: round.clone(),
                plan_id: plan,
                accepted: true,
            }));
        }
        if run.stage() == Stage::Idle
            && (result.is_ok()
                || result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.code == "app.event-failed"))
        {
            push_bounded(&mut self.terminal, (round.clone(), Outcome::Completed));
            for reply in cancels {
                let _ = reply.send(Ok(CancelledRound {
                    round_id: round.clone(),
                    outcome: Outcome::Completed,
                }));
            }
        } else if result.is_err() || !cancels.is_empty() || close.is_some() {
            let cancelled = (cancel.is_cancelled()
                && !matches!(
                    run.child_outcome(),
                    Some(Outcome::Completed | Outcome::Failed)
                ))
                || run.turn_id().is_some_and(|id| {
                    self.context
                        .coordinator
                        .snapshot(id)
                        .is_ok_and(|snapshot| snapshot.outcome == Some(Outcome::Cancelled))
                });
            let commit_failed = result.as_ref().err().is_some_and(|error| {
                error.code.starts_with("store.") || error.code == "app.event-failed"
            });
            let outcome = if !commit_failed
                && (cancelled || (result.is_ok() && (!cancels.is_empty() || close.is_some())))
            {
                Outcome::Cancelled
            } else {
                Outcome::Failed
            };
            let error = if outcome == Outcome::Failed {
                result.err()
            } else {
                None
            };
            let finish = run.finish(outcome, error.clone()).await;
            if let Err(finish_error) = &finish {
                self.context
                    .publisher
                    .quarantine(error.unwrap_or_else(|| finish_error.clone()));
            }
            if finish.is_ok() {
                push_bounded(&mut self.terminal, (round.clone(), outcome));
            }
            for reply in cancels {
                let _ = reply.send(finish.clone().map(|()| CancelledRound {
                    round_id: round.clone(),
                    outcome,
                }));
            }
            if let Some((text, frozen, reply)) = interrupt {
                if close.is_some() || self.is_closing() {
                    let _ = reply.send(Err(closed()));
                    if let Some(reply) = close.take() {
                        let _ = reply.send(());
                    }
                    return true;
                }
                let lease = run.lease();
                drop(run);
                let result = if let Err(error) = finish {
                    Err(error)
                } else if outcome != Outcome::Cancelled {
                    Err(super::invalid_phase())
                } else {
                    match Runner::accept(
                        self.context.clone(),
                        lease,
                        frozen,
                        super::input::parse_input(&text).expect("validated command"),
                    )
                    .await
                    {
                        Ok(next) => {
                            let accepted = AcceptedOperation {
                                operation_id: next.identity.operation_id.clone(),
                            };
                            self.runner = Some(next);
                            Ok(accepted)
                        }
                        Err(error) => Err(error),
                    }
                };
                let _ = reply.send(result);
            }
        } else {
            if let Some((_, _, reply)) = interrupt {
                let _ = reply.send(Err(super::invalid_phase()));
            }
            self.runner = Some(run);
        }
        if let Some(reply) = close {
            let _ = reply.send(());
            true
        } else {
            false
        }
    }
}

async fn fork(
    context: Arc<Context>,
    lease: crate::turn::Lease,
    operation: String,
    target: u64,
    regeneration: Option<super::control::RegenerationTarget>,
) -> Result<(), Fault> {
    crate::blocking::run(move || {
        let _lease = lease;
        let mut session = lock(&context.session);
        let mut domain = lock(&context.domain);
        let mode = if regeneration.is_some() {
            ForkMode::Regenerate
        } else {
            ForkMode::Rewind
        };
        let body = super::control::prepare_fork(
            &session,
            &operation,
            mode.clone(),
            target,
            regeneration.as_ref(),
        )?;
        let predicted_revision = session.next_sequence();
        let mut recovered =
            recovery::derive_prefix(&session, domain.baseline_scene(), Some(target))?;
        recovery::project(&mut recovered, predicted_revision);
        let mut next = context.publisher.snapshot().state;
        next.history_revision = predicted_revision;
        next.phase = recovered.phase;
        next.check = super::execution::paused_check(&recovered);
        next.scene = recovered.scene;
        next.in_flight = None;
        next.resume_required = recovered.checkpoint.is_some();
        next.checkpoint = recovered.checkpoint;
        next.last_operation = Some(Operation {
            operation_id: operation.clone(),
            round_id: None,
            outcome: OperationOutcome::Accepted,
            error: None,
        });
        let finished = if mode == ForkMode::Rewind {
            Some(Finished {
                operation_id: operation,
                outcome: Outcome::Completed,
                error: None,
            })
        } else {
            None
        };
        let prepared = context.publisher.prepare(next, finished)?;
        session.commit_history_fork(body, domain.as_mut())?;
        context.publisher.confirm(prepared)
    })
    .await
}

#[cfg(test)]
mod tests;
