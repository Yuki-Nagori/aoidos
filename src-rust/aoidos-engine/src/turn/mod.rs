//! 串行回合所有者：提交中的正文不被取消丢弃，快照仅暴露确认边界。

use aoidos_llm::schedule::{FinishReason, PendingText, RunOutcome, run_generation_with_output};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::fault::Fault;
use crate::ports::{EventPort, Outcome, OutputWriter, PreparedEvent, Terminal, TurnEvent};
use crate::request::PreparedGeneration;

/// 单个公开或私有调用的正文硬上限，按 UTF-8 字节计。
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
/// 单条正文事件的 UTF-8 字节上限；切分保留字符边界。
pub const MAX_CHUNK_BYTES: usize = 8 * 1024;
const MAX_SEQ: u64 = (1 << 53) - 1;

/// 快照确认的各事件基线；尚在写入的预留序号不可见。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Sequences {
    pub chunk: u64,
    pub done: u64,
    pub failed: u64,
}

impl Sequences {
    fn merge(&mut self, reserved: &Self) {
        self.chunk = self.chunk.max(reserved.chunk);
        self.done = self.done.max(reserved.done);
        self.failed = self.failed.max(reserved.failed);
    }
    fn confirm(&mut self, event: &PreparedEvent) {
        let baseline = match event.event {
            TurnEvent::Chunk { .. } => &mut self.chunk,
            TurnEvent::Done { .. } => &mut self.done,
            TurnEvent::Failed { .. } => &mut self.failed,
        };
        *baseline = event.seq;
    }
}

/// IPC 一致副本；进行中省略终态字段，取消不保留先前 finishReason。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshot {
    pub turn_id: String,
    pub text: String,
    pub seq: Sequences,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Fault>,
}

impl TurnSnapshot {
    fn empty(turn_id: String) -> Self {
        Self {
            turn_id,
            text: String::new(),
            seq: Sequences::default(),
            outcome: None,
            finish_reason: None,
            error: None,
        }
    }
    fn finish(&mut self, terminal: Terminal) {
        self.outcome = Some(terminal.outcome());
        self.finish_reason = terminal.finish_reason();
        self.error = terminal.error().cloned();
    }
}

/// 取消等待一致边界后的结果；重复取消返回已经胜出的终态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelledTurn {
    pub turn_id: String,
    pub outcome: Outcome,
}

struct TurnState {
    snapshot: TurnSnapshot,
    reserved: Sequences,
    producer_exited: bool,
}
impl TurnState {
    fn finish(&mut self, terminal: Terminal) {
        self.snapshot.seq.merge(&self.reserved);
        self.snapshot.finish(terminal);
    }
}
struct Turn {
    state: Mutex<TurnState>,
    cancel: CancellationToken,
    changed: Notify,
}
struct Registry {
    active: bool,
    producers: usize,
    turns: VecDeque<Arc<Turn>>,
    closing: bool,
}
struct Inner {
    registry: Mutex<Registry>,
    changed: Notify,
    shutdown: CancellationToken,
    events: Arc<dyn EventPort>,
    capacity: usize,
}

// 所有短临界区仅维护原子状态；中毒时保留最近确认副本，绝不在锁内 await。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 全应用共享协调器；产品、调试和内部提议使用同一 lease。
#[derive(Clone)]
pub struct Coordinator(Arc<Inner>);

impl Coordinator {
    /// 有界缓存容量包括当前 public 回合；private 不进入公开缓存。
    ///
    /// # Errors
    /// 容量不在 1–128 时返回 `app.bad-request`。
    pub fn new(events: Arc<dyn EventPort>, capacity: usize) -> Result<Self, Fault> {
        if !(1..=128).contains(&capacity) {
            return Err(Fault::bad_request());
        }
        Ok(Self(Arc::new(Inner {
            registry: Mutex::new(Registry {
                active: false,
                producers: 0,
                turns: VecDeque::new(),
                closing: false,
            }),
            changed: Notify::new(),
            shutdown: CancellationToken::new(),
            events,
            capacity,
        })))
    }

    /// 游戏回合保留 lease，public / private 子调用借用，不重新竞争应用门禁。
    ///
    /// # Errors
    /// 已占用为 `app.busy`，关闭期间为 `app.not-ready`。
    pub fn acquire(&self) -> Result<Lease, Fault> {
        let mut registry = lock(&self.0.registry);
        if registry.closing {
            return Err(Fault::new("app.not-ready", "应用正在关闭"));
        }
        if registry.active {
            return Err(Fault::busy());
        }
        registry.active = true;
        Ok(Lease(Arc::new(LeaseState {
            owner: Arc::downgrade(&self.0),
            busy: AtomicBool::new(false),
            cancel: self.0.shutdown.child_token(),
        })))
    }

    /// 接纳前的只读优先级检查；最终仍须 acquire 原子竞争同一门禁。
    /// # Errors
    /// 已占用返回 busy，关闭返回 not-ready；此检查不预留 lease。
    pub fn check_available(&self) -> Result<(), Fault> {
        let registry = lock(&self.0.registry);
        if registry.closing {
            return Err(Fault::new("app.not-ready", "应用正在关闭"));
        }
        if registry.active {
            return Err(Fault::busy());
        }
        Ok(())
    }
    /// 平台登记游戏服务时核验共享门禁，拒绝把另一协调器伪装为同一应用。
    pub fn shares_gate(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// 已校验请求的独立 public 提交，空快照创建后才返回 UUID。
    ///
    /// # Errors
    /// 同 [`Self::acquire`]；写入方错误在接纳后的事件 / 快照中表示。
    pub fn submit(
        &self,
        request: PreparedGeneration,
        writer: Arc<dyn OutputWriter>,
    ) -> Result<String, Fault> {
        let lease = self.acquire()?;
        self.submit_borrowed(&lease, request, writer)
    }

    /// 在游戏回合 lease 下创建 public 子调用；同一 lease 只允许一个子调用在飞。
    ///
    /// # Errors
    /// 外部协调器的 lease、关闭或重复子调用显式拒绝，不占新的应用门禁。
    pub fn submit_borrowed(
        &self,
        lease: &Lease,
        request: PreparedGeneration,
        writer: Arc<dyn OutputWriter>,
    ) -> Result<String, Fault> {
        let call = self.prepare_borrowed(lease, request, writer)?;
        let id = call.id().to_owned();
        call.start();
        Ok(id)
    }

    /// 先登记可读的空公开快照，驱动方发布阶段身份后再调用 start。
    /// 放弃准备对象会关闭空快照并释放子调用资源，父游戏 lease 仍由调用方所有。
    /// # Errors
    /// 与 submit_borrowed 相同；不得等待尚未 start 的子调用退出。
    pub fn prepare_borrowed(
        &self,
        lease: &Lease,
        request: PreparedGeneration,
        writer: Arc<dyn OutputWriter>,
    ) -> Result<PublicCall, Fault> {
        let runtime = tokio::runtime::Handle::try_current().map_err(runtime_missing)?;
        let permit = self.begin(lease)?;
        let id = Uuid::new_v4().to_string();
        let turn = Arc::new(Turn {
            state: Mutex::new(TurnState {
                snapshot: TurnSnapshot::empty(id.clone()),
                reserved: Sequences::default(),
                producer_exited: false,
            }),
            cancel: lease.0.cancel.child_token(),
            changed: Notify::new(),
        });
        let producer = Producer {
            permit: Some(permit),
            turn: Some(turn.clone()),
        };
        let retired = {
            let mut retired = Vec::new();
            let mut registry = lock(&self.0.registry);
            while registry.turns.len() >= self.0.capacity {
                let Some(position) = registry.turns.iter().position(|old| {
                    let state = lock(&old.state);
                    state.producer_exited && state.snapshot.outcome.is_some()
                }) else {
                    return Err(Fault::busy());
                };
                let old = registry.turns.remove(position).expect("position exists");
                retired.push(lock(&old.state).snapshot.turn_id.clone());
            }
            registry.turns.push_back(turn.clone());
            retired
        };
        for old_id in retired {
            self.0.events.retire(&old_id);
        }
        Ok(PublicCall {
            id,
            runtime,
            producer,
            owner: self.0.clone(),
            turn,
            request,
            writer,
        })
    }

    /// private 收集器复用调度与 lease，不创建公开 ring / 事件 / 正文写入。
    ///
    /// # Errors
    /// 门禁或执行失败返回脱敏错误；提议解析、规则合法性由调用引擎负责。
    pub async fn run_private(
        &self,
        lease: &Lease,
        request: PreparedGeneration,
    ) -> Result<PrivateResult, Fault> {
        self.run_private_cancellable(lease, request, CancellationToken::new())
            .await
    }

    /// 独立子取消令牌只结束本次 private 调用，父 lease 可继续交给新回合。
    /// # Errors
    /// 同 run_private；取消仍等待提交 / 用量结算完成。
    pub async fn run_private_cancellable(
        &self,
        lease: &Lease,
        request: PreparedGeneration,
        child_cancel: CancellationToken,
    ) -> Result<PrivateResult, Fault> {
        let permit = self.begin(lease)?;
        let _producer = Producer {
            permit: Some(permit),
            turn: None,
        };
        let id = Uuid::new_v4().to_string();
        let limit = request.private_limit;
        if child_cancel.is_cancelled() {
            return Ok(PrivateResult {
                turn_id: id,
                text: String::new(),
                outcome: Outcome::Cancelled,
                finish_reason: None,
            });
        }
        let cancel = lease.0.cancel.child_token();
        let mut text = String::new();
        let (tx, mut rx) = mpsc::channel::<PendingText>(32);
        let (result, fault) = {
            let producer = schedule(request, cancel.clone(), tx, id.clone());
            let consumer = async {
                while let Some(pending) = next_pending(&mut rx, &cancel).await {
                    if text.len() + pending.text.len() > limit {
                        cancel.cancel();
                        return Some(Fault::limit());
                    }
                    text.push_str(&pending.text);
                    let _ = pending.accepted.send(());
                }
                None
            };
            let run = async { tokio::join!(producer, consumer) };
            tokio::pin!(run);
            tokio::select! {
                biased;
                result = &mut run => result,
                () = child_cancel.cancelled() => { cancel.cancel(); run.await }
            }
        };
        let terminal = terminal(result, fault, !text.is_empty());
        if let Some(error) = terminal.error() {
            return Err(error.clone());
        }
        Ok(PrivateResult {
            turn_id: id,
            text,
            outcome: terminal.outcome(),
            finish_reason: terminal.finish_reason(),
        })
    }

    /// 一次短锁取得 text、终态与全部确认基线。
    ///
    /// # Errors
    /// 未知 / 已驱逐 UUID 为 `app.not-found`。
    pub fn snapshot(&self, id: &str) -> Result<TurnSnapshot, Fault> {
        Ok(lock(&self.find(id)?.state).snapshot.clone())
    }

    /// 取消 HTTP / 等待；已开始的提交由所有者完成，响应不先于终态一致边界。
    ///
    /// # Errors
    /// 未知 / 已驱逐 UUID 为 `app.not-found`。
    pub async fn cancel(&self, id: &str) -> Result<CancelledTurn, Fault> {
        let turn = self.find(id)?;
        turn.cancel.cancel();
        let snapshot = wait_turn(&turn).await;
        Ok(CancelledTurn {
            turn_id: id.to_owned(),
            outcome: snapshot.outcome.expect("exited producer has terminal"),
        })
    }

    /// 引擎等待 public 子调用退出，不触发取消；终态、提交与门禁释放全部完成才返回。
    ///
    /// # Errors
    /// 未知 / 已清退 UUID 为 app.not-found；已取得的回合引用不受后续驱逐影响。
    pub async fn wait(&self, id: &str) -> Result<TurnSnapshot, Fault> {
        let turn = self.find(id)?;
        Ok(wait_turn(&turn).await)
    }

    /// 拒绝新提交，取消全部生产者并等待已开始的提交完成；不依赖窗口投递成功。
    pub async fn shutdown(&self) {
        lock(&self.0.registry).closing = true;
        self.0.shutdown.cancel();
        loop {
            let notified = self.0.changed.notified();
            if lock(&self.0.registry).producers == 0 {
                return;
            }
            notified.await;
        }
    }

    fn find(&self, id: &str) -> Result<Arc<Turn>, Fault> {
        lock(&self.0.registry)
            .turns
            .iter()
            .find(|turn| lock(&turn.state).snapshot.turn_id == id)
            .cloned()
            .ok_or_else(Fault::not_found)
    }
    fn begin(&self, lease: &Lease) -> Result<CallPermit, Fault> {
        if !Weak::ptr_eq(&lease.0.owner, &Arc::downgrade(&self.0)) {
            return Err(Fault::bad_request());
        }
        let mut registry = lock(&self.0.registry);
        if registry.closing || lease.0.cancel.is_cancelled() {
            return Err(Fault::new("app.not-ready", "回合已停止"));
        }
        lease
            .0
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(lease_busy)?;
        registry.producers += 1;
        Ok(CallPermit {
            lease: lease.clone(),
            owner: self.0.clone(),
        })
    }
}
/// 已准备的公开子调用；快照身份确认后才能开始网络 / 正文写入。
pub struct PublicCall {
    id: String,
    runtime: tokio::runtime::Handle,
    producer: Producer,
    owner: Arc<Inner>,
    turn: Arc<Turn>,
    request: PreparedGeneration,
    writer: Arc<dyn OutputWriter>,
}
impl PublicCall {
    pub fn id(&self) -> &str {
        &self.id
    }
    /// 串行驱动确认并发布 inFlight.turnId 后调用；消费 self，不能重复启动。
    pub fn start(self) {
        self.runtime.spawn(async move {
            let _producer = self.producer;
            run_public(self.owner, self.turn, self.request, self.writer).await;
        });
    }
}

fn lease_busy(_: bool) -> Fault {
    Fault::busy()
}
fn runtime_missing(_: tokio::runtime::TryCurrentError) -> Fault {
    Fault::new("app.not-ready", "回合运行时尚未初始化")
}

/// 游戏回合所有权；最后一个持有者退出后释放应用门禁。
#[derive(Clone)]
pub struct Lease(Arc<LeaseState>);
impl Lease {
    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.0.cancel.clone()
    }
    /// 取消整轮及当前子调用；等待一致边界由协调器或阶段机持有者负责。
    pub fn cancel(&self) {
        self.0.cancel.cancel();
    }
}
struct LeaseState {
    owner: Weak<Inner>,
    busy: AtomicBool,
    cancel: CancellationToken,
}
impl Drop for LeaseState {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let mut registry = lock(&owner.registry);
            registry.active = false;
            owner.changed.notify_waiters();
        }
    }
}
struct CallPermit {
    lease: Lease,
    owner: Arc<Inner>,
}
impl Drop for CallPermit {
    fn drop(&mut self) {
        self.lease.0.busy.store(false, Ordering::Release);
        lock(&self.owner.registry).producers -= 1;
        self.owner.changed.notify_waiters();
    }
}
struct Producer {
    permit: Option<CallPermit>,
    turn: Option<Arc<Turn>>,
}
impl Drop for Producer {
    fn drop(&mut self) {
        if let Some(turn) = &self.turn {
            let mut state = lock(&turn.state);
            if state.snapshot.outcome.is_none() {
                state.finish(Terminal::failed(Fault::event()));
            }
        }
        // 不同时持有回合锁和注册表锁；门禁释放后才发布退出标志，wait 不会提前返回。
        drop(self.permit.take());
        if let Some(turn) = &self.turn {
            lock(&turn.state).producer_exited = true;
            turn.changed.notify_waiters();
        }
    }
}

async fn wait_turn(turn: &Turn) -> TurnSnapshot {
    loop {
        let notified = turn.changed.notified();
        {
            let state = lock(&turn.state);
            if state.producer_exited {
                return state.snapshot.clone();
            }
        }
        notified.await;
    }
}

/// private 结果只交给引擎，不实现 IPC 序列化，避免误接公开事件。
pub struct PrivateResult {
    pub turn_id: String,
    pub text: String,
    pub outcome: Outcome,
    pub finish_reason: Option<FinishReason>,
}

// 取消丢弃尚未开始提交的排队尾文；已经由所有者开始的写入不经过此检查。
async fn next_pending(
    rx: &mut mpsc::Receiver<PendingText>,
    cancel: &CancellationToken,
) -> Option<PendingText> {
    let pending = rx.recv().await?;
    (!cancel.is_cancelled()).then_some(pending)
}

async fn schedule(
    request: PreparedGeneration,
    cancel: CancellationToken,
    tx: mpsc::Sender<PendingText>,
    id: String,
) -> Result<RunOutcome, aoidos_llm::error::RunError> {
    let result = run_generation_with_output(
        request.provider.as_ref(),
        request.request,
        cancel,
        &request.policy,
        request.budget.as_ref(),
        &tx,
        &id,
    )
    .await;
    drop(tx);
    result
}

fn terminal(
    result: Result<RunOutcome, aoidos_llm::error::RunError>,
    fault: Option<Fault>,
    delivered: bool,
) -> Terminal {
    if let Some(fault) = fault {
        return Terminal::failed(fault);
    }
    match result {
        Ok(RunOutcome::Completed { finish, .. }) => Terminal::Completed {
            finish_reason: finish,
        },
        Ok(RunOutcome::Cancelled { .. }) => Terminal::Cancelled,
        Err(error) => {
            let (error, finish_reason) = Fault::generation(&error, delivered);
            Terminal::Failed {
                finish_reason,
                error,
            }
        }
    }
}

async fn run_public(
    owner: Arc<Inner>,
    turn: Arc<Turn>,
    request: PreparedGeneration,
    writer: Arc<dyn OutputWriter>,
) {
    let id = lock(&turn.state).snapshot.turn_id.clone();
    if let Err(error) = writer.begin(&id).await {
        // begin 未确认时不尝试第二次创建 / 封口，不覆盖首次存储失败原因。
        seal(&owner, &turn, None, &id, Terminal::failed(error)).await;
        return;
    }
    let (tx, mut rx) = mpsc::channel::<PendingText>(32);
    let producer = schedule(request, turn.cancel.clone(), tx, id.clone());
    let consumer = async {
        while let Some(pending) = next_pending(&mut rx, &turn.cancel).await {
            if let Err(error) =
                commit_text(&owner, &turn, writer.as_ref(), &id, &pending.text).await
            {
                turn.cancel.cancel();
                return Some(error);
            }
            let _ = pending.accepted.send(());
        }
        None
    };
    let (result, fault) = tokio::join!(producer, consumer);
    let ending = terminal(result, fault, !lock(&turn.state).snapshot.text.is_empty());
    seal(&owner, &turn, Some(writer.as_ref()), &id, ending).await;
}

fn prepare(owner: &Inner, event: TurnEvent) -> Result<PreparedEvent, Fault> {
    let event = owner.events.prepare(event)?;
    if event.seq == 0 || event.seq > MAX_SEQ {
        return Err(Fault::event());
    }
    Ok(event)
}

async fn commit_text(
    owner: &Inner,
    turn: &Turn,
    writer: &dyn OutputWriter,
    id: &str,
    mut text: &str,
) -> Result<(), Fault> {
    if lock(&turn.state).snapshot.text.len() + text.len() > MAX_TEXT_BYTES {
        return Err(Fault::limit());
    }
    while !text.is_empty() {
        let mut end = text.len().min(MAX_CHUNK_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let delta = &text[..end];
        let event = prepare(
            owner,
            TurnEvent::Chunk {
                turn_id: id.to_owned(),
                delta: delta.to_owned(),
            },
        )?;
        lock(&turn.state).reserved.confirm(&event);
        writer.append(id, event.seq, delta).await?;
        {
            let mut state = lock(&turn.state);
            state.snapshot.text.push_str(delta);
            state.snapshot.seq.confirm(&event);
        }
        // 投递失败保留确认副本，不回滚、不重发；最后事件全丢须主动恢复。
        let _ = owner.events.deliver(event);
        text = &text[end..];
        if turn.cancel.is_cancelled() {
            break;
        }
    }
    Ok(())
}

fn terminal_event(id: &str, chunk_seq: u64, ending: &Terminal) -> TurnEvent {
    if let Some(error) = ending.error() {
        TurnEvent::Failed {
            turn_id: id.to_owned(),
            code: error.code.clone(),
            message: error.message.clone(),
            chunk_seq,
            finish_reason: ending.finish_reason(),
        }
    } else {
        TurnEvent::Done {
            turn_id: id.to_owned(),
            outcome: ending.outcome(),
            chunk_seq,
            finish_reason: ending.finish_reason(),
        }
    }
}

async fn seal(
    owner: &Inner,
    turn: &Turn,
    writer: Option<&dyn OutputWriter>,
    id: &str,
    mut ending: Terminal,
) {
    let chunk_seq = {
        let state = lock(&turn.state);
        state.snapshot.seq.chunk.max(state.reserved.chunk)
    };
    let mut prepared = match prepare(owner, terminal_event(id, chunk_seq, &ending)) {
        Ok(event) => Some(event),
        Err(error) => {
            ending = Terminal::failed(error);
            None
        }
    };
    if let Some(event) = &prepared {
        lock(&turn.state).reserved.confirm(event);
    }
    if let Some(writer) = writer
        && let Err(error) = writer.finish(id, &ending).await
    {
        ending = Terminal::failed(error);
        prepared = prepare(owner, terminal_event(id, chunk_seq, &ending)).ok();
    }
    {
        let mut state = lock(&turn.state);
        if let Some(event) = &prepared {
            state.reserved.confirm(event);
        }
        state.finish(ending);
    }
    if let Some(event) = prepared {
        let _ = owner.events.deliver(event);
    }
    turn.changed.notify_waiters();
}

#[cfg(test)]
mod tests;
