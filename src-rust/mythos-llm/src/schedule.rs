//! 单层请求调度器：传输重试、互斥温度阶梯、看门狗、可取消背压与预算端口。
//!
//! 契约口径（ai-docs/architecture/ipc-contract.md「工程纪律」）：头阶段 30s / 流式空闲 90s、
//! 1 次初始 + 2 次传输重试、500ms 指数退避 ±25% 抖动、每回合最多 3 次物理请求。
//! 两种重试互斥：传输失败走重试后不再进阶梯；温度阶梯请求传输预算为 0。
//! 首个安全字符被输出写入方接纳（或增量持久化，022 接入）即禁止一切重发；
//! 心跳 / reasoning / stop 片段不延寿。取消中止 HTTP、退避与队列等待。

use crate::error::{ProviderError, RunError, TransportClass};
use crate::guard::{Guard, GuardSpec};
use crate::provider::{
    Provider, ProviderDelta, ProviderFinish, ProviderRequest, RequestMode, Usage,
};
use futures::StreamExt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;

/// 契约 v1 预算值；字段公开给装配层与测试覆盖。
#[derive(Debug, Clone)]
pub struct RunPolicy {
    /// 请求开始至首个护栏安全字符被接纳（含连接、HTTP 头、SSE、护栏等待）。
    pub head_budget: Duration,
    /// 首交付后无新安全增量的空闲上限。
    pub idle_budget: Duration,
    /// 传输重试次数（不含初始请求）。
    pub transport_retries: u32,
    /// 退避基准，按 2^n 指数、±25% 抖动。
    pub backoff_base: Duration,
    /// 空输出温度阶梯；最多两步、严格递增、高于初始温度且在模型范围内。
    pub ladder: Vec<f64>,
    /// 每回合物理请求总数上限。
    pub max_requests: u32,
}

impl Default for RunPolicy {
    fn default() -> Self {
        Self {
            head_budget: Duration::from_secs(30),
            idle_budget: Duration::from_secs(90),
            transport_retries: 2,
            backoff_base: Duration::from_millis(500),
            ladder: vec![1.1, 1.2],
            max_requests: 3,
        }
    }
}

impl RunPolicy {
    /// 默认阶梯只保留高于初始温度且供应商支持的步长；显式策略仍严格校验。
    #[must_use]
    pub fn default_for(initial: f64, capabilities: &crate::provider::ProviderCapabilities) -> Self {
        let mut policy = Self::default();
        policy.ladder.retain(|step| {
            capabilities.temperature_effective
                && *step > initial
                && *step >= capabilities.temperature_min
                && *step <= capabilities.temperature_max
        });
        policy
    }
}

/// 对外收尾原因（契约 `llm:turn:*` 的 finishReason）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FinishReason {
    Stop,
    Guard,
    Length,
}

/// 调度结果。
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Completed {
        finish: FinishReason,
        requests: u32,
        usage: Usage,
    },
    Cancelled {
        requests: u32,
        usage: Usage,
    },
}

/// 一次物理尝试的身份；035 账本按此预留 / 结算（同一回合请求身份持久保存）。
#[derive(Debug, Clone, PartialEq)]
pub struct AttemptIdentity {
    /// 每次物理尝试唯一；035 不以重试序号代替请求身份。
    pub request_id: String,
    /// 协调器提供的回合身份；独立调度夹具为空，不用于生产计费。
    pub turn_id: String,
    /// 本回合第几次物理请求，从 1 起。
    pub attempt: u32,
    pub kind: AttemptKind,
    pub model: String,
    pub mode: RequestMode,
    pub max_tokens: u32,
    pub temperature: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptKind {
    Initial,
    TransportRetry,
    TemperatureStep,
}

/// 尝试结束时的用量生命周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// 获得完整 usage（或显式零）后结算。
    Settled {
        usage: Option<Usage>,
    },
    /// 失败 / 超时：消耗未确认，不按零退款。
    Failed,
    Cancelled,
}

/// 预算端口；035 落地前用 [`AllowAll`]，测试注入记账或拒绝实现。
pub trait BudgetPort: Send + Sync {
    /// 发请求前按估算预留；拒绝则本次物理请求不发出。
    ///
    /// # Errors
    /// 本地额度不足或预算状态异常时返回 [`BudgetDenial`]（code 属 budget 命名空间）。
    fn reserve(&self, attempt: &AttemptIdentity) -> Result<(), BudgetDenial>;
    /// 每次成功预留的尝试恰结算一次。
    fn settle(&self, attempt: &AttemptIdentity, outcome: &AttemptOutcome);
}

// 每次成功预留的尝试只收尾一次；Future 被丢弃也登记消耗未确认，不按零退款。
struct AttemptReservation<'a> {
    budget: &'a dyn BudgetPort,
    identity: &'a AttemptIdentity,
    settled: bool,
}
impl AttemptReservation<'_> {
    fn settle(&mut self, outcome: AttemptOutcome) {
        self.settled = true;
        self.budget.settle(self.identity, &outcome);
    }
}
impl Drop for AttemptReservation<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.budget
                .settle(self.identity, &AttemptOutcome::Cancelled);
        }
    }
}

/// 预留拒绝（code 属 budget 命名空间，由 035 细分）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetDenial {
    pub reason: String,
}

/// 默认放行端口：035 接入前不构成额度控制。
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAll;

impl BudgetPort for AllowAll {
    fn reserve(&self, _attempt: &AttemptIdentity) -> Result<(), BudgetDenial> {
        Ok(())
    }

    fn settle(&self, _attempt: &AttemptIdentity, _outcome: &AttemptOutcome) {}
}

/// 调度器入参：冻结的 provider 请求 + 护栏 spec。
#[derive(Debug, Clone)]
pub struct GenerationRequest {
    pub provider_request: ProviderRequest,
    pub guard: GuardSpec,
}

/// 退避延迟：基准 × 2^(retry-1) × (1 + jitter)，jitter ∈ [-0.25, 0.25]。
#[must_use]
pub fn backoff_delay(base: Duration, retry: u32, jitter: f64) -> Duration {
    let factor = 1u64 << (retry - 1).min(30);
    let scaled = jitter.clamp(-0.25, 0.25) + 1.0;
    let millis = base.as_millis() as f64 * factor as f64 * scaled;
    Duration::from_millis(millis.max(0.0) as u64)
}

/// 无 rand 依赖的时钟抖动：取亚微秒纳斯散布，只需统计性质不需密码学质量。
fn clock_jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    f64::from(nanos % 1000) / 1000.0 * 0.5 - 0.25
}

/// 安全增量提交端口；true 表示消费方已接纳，而不是仅排进待写队列。
pub trait OutputPort: Send + Sync {
    /// 提交确认端口在写入中超时后不得重发；所有者仍等待已开始的提交。
    fn requires_confirmation(&self) -> bool {
        false
    }
    fn send(&self, text: String) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
}

impl OutputPort for mpsc::Sender<String> {
    fn send(&self, text: String) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.send(text).await.is_ok() })
    }
}

/// 待消费增量；所有者完成提交后发送确认。消费方关闭 / 不确认按取消收尾。
pub struct PendingText {
    pub text: String,
    pub accepted: oneshot::Sender<()>,
}

impl OutputPort for mpsc::Sender<PendingText> {
    fn requires_confirmation(&self) -> bool {
        true
    }
    fn send(&self, text: String) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async move {
            let (accepted, receipt) = oneshot::channel();
            if self.send(PendingText { text, accepted }).await.is_err() {
                return false;
            }
            receipt.await.is_ok()
        })
    }
}

/// 单回合生成主流程。`output` 是调用方构造的有界通道（契约容量 32）：
/// 队列满时背压等待（可取消 / 可被看门狗打断）；接收端提前关闭视作消费方放弃，按取消收尾。
///
/// # Errors
/// 传输 / 协议失败（已按交付边界定码）、看门狗耗尽、空输出与本地策略 / 预算拒绝，见 [`RunError`]。
pub async fn run_generation(
    provider: &dyn Provider,
    request: GenerationRequest,
    cancel: CancellationToken,
    policy: &RunPolicy,
    budget: &dyn BudgetPort,
    output: mpsc::Sender<String>,
) -> Result<RunOutcome, RunError> {
    run_generation_with_output(provider, request, cancel, policy, budget, &output, "").await
}

/// 协调器使用的提交确认入口；重试和看门狗与通道夹具共用同一实现。
///
/// # Errors
/// 与 [`run_generation`] 相同；输出写入失败由所有者停止确认并映射域错误。
pub async fn run_generation_with_output(
    provider: &dyn Provider,
    request: GenerationRequest,
    cancel: CancellationToken,
    policy: &RunPolicy,
    budget: &dyn BudgetPort,
    output: &dyn OutputPort,
    turn_id: &str,
) -> Result<RunOutcome, RunError> {
    let mode = request.provider_request.mode();
    let caps = provider.capabilities(&request.provider_request.model, mode);
    validate_ladder(
        policy,
        request.provider_request.sampling.temperature,
        caps.temperature_min,
        caps.temperature_max,
    )?;

    let mut requests: u32 = 0;
    let mut transport_used: u32 = 0;
    let mut ladder_pos: usize = 0;
    let mut total_usage = Usage::default();
    let mut delivered = false;

    loop {
        if cancel.is_cancelled() {
            return Ok(RunOutcome::Cancelled {
                requests,
                usage: total_usage,
            });
        }
        let kind = match (transport_used, ladder_pos) {
            (0, 0) => AttemptKind::Initial,
            (0, _) => AttemptKind::TemperatureStep,
            _ => AttemptKind::TransportRetry,
        };
        let temperature = if kind == AttemptKind::TemperatureStep {
            policy.ladder[ladder_pos - 1]
        } else {
            request.provider_request.sampling.temperature
        };
        let provider_request = request.provider_request.with_temperature(temperature);
        let identity = AttemptIdentity {
            request_id: uuid::Uuid::new_v4().to_string(),
            turn_id: turn_id.to_owned(),
            attempt: requests + 1,
            kind,
            model: provider_request.model.clone(),
            mode,
            max_tokens: provider_request.sampling.max_tokens,
            temperature,
        };
        if let Err(denial) = budget.reserve(&identity) {
            return Err(RunError::Budget {
                reason: denial.reason,
            });
        }
        let mut reservation = AttemptReservation {
            budget,
            identity: &identity,
            settled: false,
        };
        requests += 1;
        match attempt(
            provider,
            provider_request,
            request.guard.clone(),
            &cancel,
            policy,
            output,
            &mut delivered,
        )
        .await
        {
            AttemptEnd::Completed { finish, usage } => {
                total_usage.merge(usage.unwrap_or_default());
                reservation.settle(AttemptOutcome::Settled { usage });
                if !delivered
                    && ladder_entry_allowed(
                        policy,
                        finish,
                        transport_used,
                        ladder_pos,
                        caps.temperature_effective,
                    )
                {
                    ladder_pos += 1;
                    continue;
                }
                if !delivered {
                    return Err(RunError::EmptyOutput { finish });
                }
                return Ok(RunOutcome::Completed {
                    finish,
                    requests,
                    usage: total_usage,
                });
            }
            AttemptEnd::Cancelled { usage } => {
                total_usage.merge(usage.unwrap_or_default());
                reservation.settle(AttemptOutcome::Cancelled);
                return Ok(RunOutcome::Cancelled {
                    requests,
                    usage: total_usage,
                });
            }
            AttemptEnd::Failed { error, usage } => {
                total_usage.merge(usage.unwrap_or_default());
                reservation.settle(AttemptOutcome::Failed);
                if transport_retry_allowed(
                    &error,
                    policy,
                    delivered,
                    ladder_pos,
                    transport_used,
                    requests,
                ) {
                    transport_used += 1;
                    if !wait_backoff(&cancel, policy, transport_used).await {
                        return Ok(RunOutcome::Cancelled {
                            requests,
                            usage: total_usage,
                        });
                    }
                    continue;
                }
                // 首字节后的中断 / 空闲超时按交付边界映射 aborted，保留前文不重试。
                return Err(RunError::Provider(error));
            }
            AttemptEnd::DeliveryStalled => {
                reservation.settle(AttemptOutcome::Failed);
                return Err(RunError::Stalled);
            }
            AttemptEnd::StalledHead => {
                reservation.settle(AttemptOutcome::Failed);
                if transport_retry_allowed(
                    &ProviderError::Interrupted,
                    policy,
                    delivered,
                    ladder_pos,
                    transport_used,
                    requests,
                ) {
                    transport_used += 1;
                    if !wait_backoff(&cancel, policy, transport_used).await {
                        return Ok(RunOutcome::Cancelled {
                            requests,
                            usage: total_usage,
                        });
                    }
                    continue;
                }
                return Err(RunError::Stalled);
            }
        }
    }
}

/// 传输重试进入条件：仅首交付前、未进阶梯、预算未耗尽且错误可重试。
fn transport_retry_allowed(
    error: &ProviderError,
    policy: &RunPolicy,
    delivered: bool,
    ladder_pos: usize,
    transport_used: u32,
    requests: u32,
) -> bool {
    !delivered
        && ladder_pos == 0
        && transport_used < policy.transport_retries
        && requests < policy.max_requests
        && error.transport_retryable()
}

/// 退避等待（500ms × 2^n ± 25% 抖动）；被取消打断时返回 false。
async fn wait_backoff(cancel: &CancellationToken, policy: &RunPolicy, retry: u32) -> bool {
    let delay = backoff_delay(policy.backoff_base, retry, clock_jitter());
    tokio::select! {
        biased;
        () = cancel.cancelled() => false,
        () = sleep_until(Instant::now() + delay) => true,
    }
}

/// 阶梯配置校验：最多两步、严格递增、高于初始温度且在模型合法范围内。
///
/// # Errors
/// 非法阶梯返回 `InvalidPolicy`；接纳前与调度器共用同一校验。
pub fn validate_ladder(
    policy: &RunPolicy,
    initial: f64,
    min: f64,
    max: f64,
) -> Result<(), RunError> {
    if policy.ladder.len() > 2 {
        return Err(RunError::InvalidPolicy("ladder exceeds two steps".into()));
    }
    let mut previous = initial;
    for step in &policy.ladder {
        if !step.is_finite() || *step <= previous {
            return Err(RunError::InvalidPolicy(format!(
                "ladder must strictly increase above {previous}"
            )));
        }
        if *step < min || *step > max {
            return Err(RunError::InvalidPolicy(format!(
                "ladder step {step} outside model range [{min}, {max}]"
            )));
        }
        previous = *step;
    }
    Ok(())
}

/// 空输出温度重试的进入条件（契约：仅首尝试、stop 正常收尾、未截断、无传输重试、
/// temperature 生效且阶梯还有余量）。
fn ladder_entry_allowed(
    policy: &RunPolicy,
    finish: FinishReason,
    transport_used: u32,
    ladder_pos: usize,
    temperature_effective: bool,
) -> bool {
    finish == FinishReason::Stop
        && transport_used == 0
        && ladder_pos < policy.ladder.len()
        && temperature_effective
}

enum AttemptEnd {
    Completed {
        finish: FinishReason,
        usage: Option<Usage>,
    },
    Cancelled {
        usage: Option<Usage>,
    },
    Failed {
        error: ProviderError,
        usage: Option<Usage>,
    },
    StalledHead,
    DeliveryStalled,
}

async fn attempt(
    provider: &dyn Provider,
    provider_request: ProviderRequest,
    guard_spec: GuardSpec,
    cancel: &CancellationToken,
    policy: &RunPolicy,
    output: &dyn OutputPort,
    delivered: &mut bool,
) -> AttemptEnd {
    let mut usage: Option<Usage> = None;
    // 发送失败的统一收尾：取消 / 关闭归取消，头 / 空闲超时归各自错误。
    macro_rules! try_emit {
        ($emit:expr) => {
            match $emit.await {
                EmitEnd::Sent => {}
                EmitEnd::Cancelled | EmitEnd::Closed => {
                    return AttemptEnd::Cancelled { usage };
                }
                EmitEnd::StalledHead => {
                    return if output.requires_confirmation() {
                        AttemptEnd::DeliveryStalled
                    } else {
                        AttemptEnd::StalledHead
                    }
                }
                EmitEnd::Idle => {
                    return AttemptEnd::Failed {
                        error: ProviderError::Network {
                            source: TransportClass::Timeout,
                        },
                        usage,
                    };
                }
            }
        };
    }

    let child = cancel.child_token();
    let head_deadline = Instant::now() + policy.head_budget;
    let mut stream = tokio::select! {
        biased;
        () = cancel.cancelled() => return AttemptEnd::Cancelled { usage: None },
        () = sleep_until(head_deadline) => return AttemptEnd::StalledHead,
        started = provider.start(provider_request, child.clone()) => match started {
            Ok(stream) => stream,
            Err(error) => return AttemptEnd::Failed { error, usage: None },
        },
    };
    let mut guard = Guard::new(guard_spec);
    // 空闲计时只在首交付后由 `if *delivered` 守卫生效；初值取远期时刻。
    let mut idle_deadline = far_future();
    loop {
        let item = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                guard.discard();
                return AttemptEnd::Cancelled { usage };
            }
            () = sleep_until(head_deadline), if !*delivered => return AttemptEnd::StalledHead,
            () = sleep_until(idle_deadline), if *delivered => {
                return AttemptEnd::Failed {
                    error: ProviderError::Network { source: TransportClass::Timeout },
                    usage,
                };
            }
            item = stream.next() => item,
        };
        match item {
            None => {
                // 干净流结束但没有 Finish 增量：取消路径已被上方分支接管，这里是提前 EOF。
                return AttemptEnd::Failed {
                    error: ProviderError::Interrupted,
                    usage,
                };
            }
            Some(Err(error)) => {
                return AttemptEnd::Failed { error, usage };
            }
            Some(Ok(delta)) => match delta {
                ProviderDelta::Text(text) => match guard.push(&text) {
                    crate::guard::GuardStep::Stopped { delivered: safe } => {
                        let marks_delivery = !safe.is_empty();
                        try_emit!(emit(
                            output,
                            cancel,
                            *delivered,
                            idle_deadline,
                            head_deadline,
                            safe
                        ));
                        *delivered = *delivered || marks_delivery;
                        // 完整命中只取消本次请求的 child，按 completed/guard 收尾。
                        child.cancel();
                        return AttemptEnd::Completed {
                            finish: FinishReason::Guard,
                            usage,
                        };
                    }
                    crate::guard::GuardStep::Delta(safe) => {
                        if safe.is_empty() {
                            continue;
                        }
                        try_emit!(emit(
                            output,
                            cancel,
                            *delivered,
                            idle_deadline,
                            head_deadline,
                            safe
                        ));
                        *delivered = true;
                        idle_deadline = Instant::now() + policy.idle_budget;
                    }
                },
                ProviderDelta::Reasoning(_) => {
                    // 心跳 / reasoning 不重置任何计时器。
                }
                ProviderDelta::Usage(chunk) => {
                    usage.get_or_insert_with(Usage::default).merge(chunk)
                }
                ProviderDelta::Finish(provider_finish) => {
                    let flushed = guard.finish();
                    if !flushed.tail.is_empty() {
                        try_emit!(emit(
                            output,
                            cancel,
                            *delivered,
                            idle_deadline,
                            head_deadline,
                            flushed.tail
                        ));
                        *delivered = true;
                    }
                    let finish = if flushed.truncated {
                        FinishReason::Guard
                    } else {
                        match provider_finish {
                            ProviderFinish::Stop => FinishReason::Stop,
                            ProviderFinish::Length => FinishReason::Length,
                        }
                    };
                    return AttemptEnd::Completed { finish, usage };
                }
            },
        }
    }
}

enum EmitEnd {
    Sent,
    Cancelled,
    Closed,
    StalledHead,
    Idle,
}

/// 背压发送：队列满时等待，可被取消与看门狗打断。
async fn emit(
    output: &dyn OutputPort,
    cancel: &CancellationToken,
    delivered: bool,
    idle_deadline: Instant,
    head_deadline: Instant,
    text: String,
) -> EmitEnd {
    tokio::select! {
        biased;
        () = cancel.cancelled() => EmitEnd::Cancelled,
        () = sleep_until(head_deadline), if !delivered => EmitEnd::StalledHead,
        () = sleep_until(idle_deadline), if delivered => EmitEnd::Idle,
        sent = output.send(text) => {
            if sent {
                EmitEnd::Sent
            } else {
                EmitEnd::Closed
            }
        }
    }
}

/// 永不触发的时刻；仅在未交付前充当空闲计时的占位（该分支由 `if delivered` 守卫禁用）。
fn far_future() -> Instant {
    Instant::now() + Duration::from_hours(876000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::{Anchor, GuardRule};
    use crate::provider::ChatInput;
    use crate::providers::DeepSeek;
    use crate::testserver::{
        ScriptedServer, Segment, http_error_script, sse_finish_frame, sse_ok_script,
        sse_reasoning_frame, sse_text_frame, sse_usage_frame,
    };
    use secrecy::SecretString;
    use std::sync::{Arc, Mutex};

    fn fixture_guard() -> crate::guard::GuardSpec {
        crate::guard::GuardSpec::compile(vec![
            GuardRule {
                id: "close".into(),
                pattern: "</narration>".into(),
                anchor: Anchor::Anywhere,
                priority: 10,
                server_eligible: true,
            },
            GuardRule {
                id: "player".into(),
                pattern: "[PLAYER]".into(),
                anchor: Anchor::LineStart,
                priority: 20,
                server_eligible: false,
            },
        ])
        .expect("fixture guard compiles")
    }

    fn generation_request() -> GenerationRequest {
        GenerationRequest {
            provider_request: ProviderRequest {
                model: "deepseek-v4-pro".into(),
                input: crate::provider::ProviderInput::Completion(
                    crate::provider::CompletionInput { prompt: "P".into() },
                ),
                sampling: crate::sampling::Sampling {
                    temperature: 1.0,
                    max_tokens: 64,
                },
                stops: vec![],
            },
            guard: fixture_guard(),
        }
    }

    /// 看门狗语义不依赖绝对时长：测试用缩短的真实预算（契约默认值另由单测断言）。
    /// 不用 paused 虚拟时钟：暂停时钟会在真实 IO 等待时空转推进，误触发计时器。
    /// 250ms 给慢速 CI 留足首包余量（过小会让 idle/head 在正文到达前误触发走错分支）。
    fn fast_policy() -> RunPolicy {
        RunPolicy {
            head_budget: Duration::from_millis(250),
            idle_budget: Duration::from_millis(250),
            transport_retries: 2,
            backoff_base: Duration::from_millis(5),
            ladder: vec![1.1, 1.2],
            max_requests: 3,
        }
    }

    async fn setup(
        script: Vec<Segment>,
    ) -> (
        ScriptedServer,
        Arc<DeepSeek>,
        mpsc::Sender<String>,
        mpsc::Receiver<String>,
    ) {
        let server = ScriptedServer::start(script).await;
        let provider = Arc::new(
            DeepSeek::new(
                format!("http://{}", server.addr),
                SecretString::new("test-key".into()),
            )
            .expect("client builds"),
        );
        let (tx, rx) = mpsc::channel(32);
        (server, provider, tx, rx)
    }

    fn drain(rx: &mut mpsc::Receiver<String>) -> String {
        let mut text = String::new();
        while let Ok(chunk) = rx.try_recv() {
            text.push_str(&chunk);
        }
        text
    }

    fn request_temperature(request: &crate::testserver::RecordedRequest) -> f64 {
        serde_json::from_slice::<serde_json::Value>(&request.body).expect("json body")["temperature"]
            .as_f64()
            .expect("temperature")
    }

    #[tokio::test]
    async fn happy_path_delivers_text_and_completes() {
        let body = format!(
            "{}{}{}data: [DONE]\n\n",
            sse_text_frame("风吹过。"),
            sse_usage_frame(10, 5),
            sse_finish_frame("stop")
        );
        let (server, provider, tx, mut rx) = setup(sse_ok_script(&body)).await;
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 1,
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5
                },
            })
        );
        assert_eq!(drain(&mut rx), "风吹过。");
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn guard_hit_completes_with_guard_and_drops_rest() {
        let body = format!(
            "{}{}{}{}data: [DONE]\n\n",
            sse_text_frame("尾声"),
            sse_text_frame("</narration>"),
            sse_text_frame("不再交付"),
            sse_finish_frame("stop")
        );
        let (server, provider, tx, mut rx) = setup(sse_ok_script(&body)).await;
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Guard,
                requests: 1,
                usage: Usage::default(),
            })
        );
        assert_eq!(drain(&mut rx), "尾声");
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn crlf_tail_is_flushed_at_normal_finish() {
        let body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("A\r"),
            sse_finish_frame("stop")
        );
        let (_server, provider, tx, mut rx) = setup(sse_ok_script(&body)).await;
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 1,
                usage: Usage::default(),
            })
        );
        assert_eq!(drain(&mut rx), "A\n");
    }

    #[tokio::test]
    async fn finish_flush_truncation_reports_guard_reason() {
        // 疑似闭合标签尾部在正常结束时保守丢弃，finishReason 记 guard。
        let body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("尾声</nar"),
            sse_finish_frame("stop")
        );
        let (_server, provider, tx, mut rx) = setup(sse_ok_script(&body)).await;
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Guard,
                requests: 1,
                usage: Usage::default(),
            })
        );
        assert_eq!(drain(&mut rx), "尾声");
    }

    #[tokio::test]
    async fn transport_failure_retries_then_succeeds_with_frozen_config() {
        let ok_body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("重试后"),
            sse_finish_frame("stop")
        );
        let server =
            ScriptedServer::start_sequence(vec![http_error_script(500), sse_ok_script(&ok_body)])
                .await;
        let provider = Arc::new(
            DeepSeek::new(
                format!("http://{}", server.addr),
                SecretString::new("k".into()),
            )
            .unwrap(),
        );
        let (tx, mut rx) = mpsc::channel(32);
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 2,
                usage: Usage::default(),
            })
        );
        assert_eq!(drain(&mut rx), "重试后");
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        // 重试沿用冻结的 prompt / stop / 模型与温度。
        assert_eq!(requests[0].body, requests[1].body);
    }

    #[tokio::test]
    async fn rate_limit_is_retried() {
        let ok_body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("限流后"),
            sse_finish_frame("stop")
        );
        let server =
            ScriptedServer::start_sequence(vec![http_error_script(429), sse_ok_script(&ok_body)])
                .await;
        let provider = Arc::new(
            DeepSeek::new(
                format!("http://{}", server.addr),
                SecretString::new("k".into()),
            )
            .unwrap(),
        );
        let (tx, _rx) = mpsc::channel(32);
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 2,
                usage: Usage::default(),
            })
        );
    }

    #[tokio::test]
    async fn transport_retry_exhaustion_fails_with_network() {
        let (server, provider, tx, _rx) = setup(http_error_script(500)).await;
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            RunError::Provider(ProviderError::Network {
                source: crate::error::TransportClass::Connect
            })
        );
        assert_eq!(server.request_count(), 3);
    }

    #[tokio::test]
    async fn tls_failure_is_not_retried() {
        let server = ScriptedServer::start_tls(vec![
            Segment::Status(200),
            Segment::Header("Connection: close".into()),
        ])
        .await;
        let provider = Arc::new(
            DeepSeek::new(
                // 直连回环（证书不含该主机名，校验失败同样归 Tls）。
                format!("https://127.0.0.1:{}", server.addr.port()),
                SecretString::new("k".into()),
            )
            .unwrap(),
        );
        let (tx, _rx) = mpsc::channel(32);
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(error, RunError::Provider(ProviderError::Tls));
    }

    #[tokio::test]
    async fn mid_stream_abort_after_delivery_maps_to_aborted_without_retry() {
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(sse_text_frame("前文").into_bytes()),
            // 留出读取窗口，避免 RST 清掉客户端尚未读走的缓冲；150ms 兼顾慢速 CI。
            Segment::Delay(Duration::from_millis(150)),
            Segment::Abort,
        ];
        let (server, provider, tx, mut rx) = setup(script).await;
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        let source = provider_error(&error).expect("provider failure");
        assert_eq!(source.code_at_boundary(true), "aborted");
        assert_eq!(drain(&mut rx), "前文");
        // 首字节后 5xx / 断线都不再重试。
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn early_clean_eof_is_retried_as_network() {
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::Header(String::new()),
        ];
        let (server, provider, tx, _rx) = setup(script).await;
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(error, RunError::Provider(ProviderError::Interrupted));
        assert_eq!(server.request_count(), 3);
    }

    #[tokio::test]
    async fn empty_stop_enters_ladder_and_recovers() {
        let empty_body = format!("{}data: [DONE]\n\n", sse_finish_frame("stop"));
        let ok_body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("阶梯后"),
            sse_finish_frame("stop")
        );
        let server = ScriptedServer::start_sequence(vec![
            sse_ok_script(&empty_body),
            sse_ok_script(&ok_body),
        ])
        .await;
        let provider = Arc::new(
            DeepSeek::new(
                format!("http://{}", server.addr),
                SecretString::new("k".into()),
            )
            .unwrap(),
        );
        let (tx, mut rx) = mpsc::channel(32);
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 2,
                usage: Usage::default(),
            })
        );
        assert_eq!(drain(&mut rx), "阶梯后");
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(request_temperature(&requests[0]), 1.0);
        assert_eq!(request_temperature(&requests[1]), 1.1);
    }

    #[tokio::test]
    async fn usage_accumulates_across_ladder_attempts() {
        // 空输出进入阶梯：两次物理尝试的 usage 都计入总量（丢弃正文不抹去已发生用量）。
        let empty_with_usage = format!(
            "{}{}data: [DONE]\n\n",
            sse_usage_frame(10, 4),
            sse_finish_frame("stop")
        );
        let ok_with_usage = format!(
            "{}{}{}data: [DONE]\n\n",
            sse_text_frame("续"),
            sse_usage_frame(7, 2),
            sse_finish_frame("stop")
        );
        let server = ScriptedServer::start_sequence(vec![
            sse_ok_script(&empty_with_usage),
            sse_ok_script(&ok_with_usage),
        ])
        .await;
        let provider = Arc::new(
            DeepSeek::new(
                format!("http://{}", server.addr),
                SecretString::new("k".into()),
            )
            .unwrap(),
        );
        let (tx, mut rx) = mpsc::channel(32);
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 2,
                usage: Usage {
                    prompt_tokens: 17,
                    completion_tokens: 6,
                },
            })
        );
        assert_eq!(drain(&mut rx), "续");
    }

    #[tokio::test]
    async fn ladder_exhaustion_reports_empty_output_stop() {
        let empty_body = format!("{}data: [DONE]\n\n", sse_finish_frame("stop"));
        let (server, provider, tx, _rx) = setup(sse_ok_script(&empty_body)).await;
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            RunError::EmptyOutput {
                finish: FinishReason::Stop
            }
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        let temps: Vec<f64> = requests.iter().map(request_temperature).collect();
        assert_eq!(temps, [1.0, 1.1, 1.2]);
    }

    #[tokio::test]
    async fn guard_empty_output_never_enters_ladder() {
        let body = format!("{}data: [DONE]\n\n", sse_text_frame("</narration>"));
        let (server, provider, tx, _rx) = setup(sse_ok_script(&body)).await;
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            RunError::EmptyOutput {
                finish: FinishReason::Guard
            }
        );
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn length_empty_output_never_enters_ladder() {
        let body = format!("{}data: [DONE]\n\n", sse_finish_frame("length"));
        let (server, provider, tx, _rx) = setup(sse_ok_script(&body)).await;
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            RunError::EmptyOutput {
                finish: FinishReason::Length
            }
        );
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn transport_retry_used_then_empty_does_not_enter_ladder() {
        let empty_body = format!("{}data: [DONE]\n\n", sse_finish_frame("stop"));
        let server = ScriptedServer::start_sequence(vec![
            http_error_script(500),
            sse_ok_script(&empty_body),
        ])
        .await;
        let provider = Arc::new(
            DeepSeek::new(
                format!("http://{}", server.addr),
                SecretString::new("k".into()),
            )
            .unwrap(),
        );
        let (tx, _rx) = mpsc::channel(32);
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            RunError::EmptyOutput {
                finish: FinishReason::Stop
            }
        );
        assert_eq!(server.request_count(), 2);
    }

    #[tokio::test]
    async fn ladder_request_failure_ends_immediately() {
        let empty_body = format!("{}data: [DONE]\n\n", sse_finish_frame("stop"));
        let server = ScriptedServer::start_sequence(vec![
            sse_ok_script(&empty_body),
            http_error_script(500),
        ])
        .await;
        let provider = Arc::new(
            DeepSeek::new(
                format!("http://{}", server.addr),
                SecretString::new("k".into()),
            )
            .unwrap(),
        );
        let (tx, _rx) = mpsc::channel(32);
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            RunError::Provider(ProviderError::Network {
                source: crate::error::TransportClass::Connect
            })
        );
        assert_eq!(server.request_count(), 2);
    }

    #[tokio::test]
    async fn ladder_disabled_when_temperature_is_not_effective() {
        let body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("无温"),
            sse_finish_frame("stop")
        );
        let (server, provider, tx, mut rx) = setup(sse_ok_script(&body)).await;
        let mut request = generation_request();
        request.provider_request.model = "mystery-model".into();
        let outcome = run_generation(
            provider.as_ref(),
            request,
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 1,
                usage: Usage::default(),
            })
        );
        assert_eq!(drain(&mut rx), "无温");
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn head_stall_with_heartbeat_and_reasoning_is_retried_then_fails() {
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(b": keep-alive\n\n".to_vec()),
            Segment::BodyChunk(sse_reasoning_frame("思考").into_bytes()),
            Segment::Hold,
        ];
        let (server, provider, tx, _rx) = setup(script).await;
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(error, RunError::Stalled);
        assert_eq!(server.request_count(), 3);
    }

    #[tokio::test]
    async fn no_response_at_all_stalls_before_headers() {
        let (server, provider, tx, _rx) = setup(vec![Segment::Hold]).await;
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(error, RunError::Stalled);
        // 请求已被服务端读走并记录，只是永远不给响应。
        assert_eq!(server.request_count(), 3);
        // 关停挂起中的 Hold 连接，并等待各连接的取消分支完成返回。
        server.shutdown().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn idle_stall_after_delivery_maps_to_aborted() {
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(sse_text_frame("首段").into_bytes()),
            Segment::Delay(Duration::from_millis(30)),
            Segment::BodyChunk(sse_reasoning_frame("不再重置计时").into_bytes()),
            Segment::Hold,
        ];
        let (server, provider, tx, mut rx) = setup(script).await;
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        let source = provider_error(&error).expect("provider failure");
        assert_eq!(source.code_at_boundary(true), "aborted");
        assert_eq!(drain(&mut rx), "首段");
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn cancel_mid_stream_keeps_delivered_prefix() {
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(sse_text_frame("[PLA").into_bytes()),
            Segment::Hold,
        ];
        let (server, provider, tx, mut rx) = setup(script).await;
        let cancel = CancellationToken::new();
        let policy = Arc::new(fast_policy());
        let request = generation_request();
        let provider_ref = provider.clone();
        let cancel_for_task = cancel.clone();
        let task = tokio::spawn(async move {
            run_generation(
                provider_ref.as_ref(),
                request,
                cancel_for_task,
                &policy,
                &AllowAll,
                tx,
            )
            .await
        });
        // 扣留中的疑似标记不泄漏：取消前没有任何可交付正文。
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        let outcome = task.await.expect("task joins").expect("cancelled is Ok");
        assert_eq!(
            outcome,
            RunOutcome::Cancelled {
                requests: 1,
                usage: Usage::default()
            }
        );
        assert_eq!(drain(&mut rx), "");
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn cancel_during_backoff_returns_cancelled() {
        let (_server, provider, tx, _rx) = setup(http_error_script(500)).await;
        let mut policy = fast_policy();
        policy.backoff_base = Duration::from_millis(4000);
        let cancel = CancellationToken::new();
        let policy = Arc::new(policy);
        let ledger = Arc::new(Recorder::default());
        let request = generation_request();
        let provider_ref = provider.clone();
        let cancel_for_task = cancel.clone();
        let ledger_for_task = ledger.clone();
        let task = tokio::spawn(async move {
            run_generation(
                provider_ref.as_ref(),
                request,
                cancel_for_task,
                &policy,
                ledger_for_task.as_ref(),
                tx,
            )
            .await
        });
        // 观察到「首个尝试已结算失败」即确定进入退避等待，再取消。
        while !ledger
            .log
            .lock()
            .expect("log")
            .iter()
            .any(|entry| entry.starts_with("S1:Failed"))
        {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        cancel.cancel();
        let outcome = task.await.expect("task joins").expect("cancelled is Ok");
        assert_eq!(
            outcome,
            RunOutcome::Cancelled {
                requests: 1,
                usage: Usage::default()
            }
        );
    }

    #[tokio::test]
    async fn cancel_during_stalled_backoff_returns_cancelled() {
        let (_server, provider, tx, _rx) = setup(vec![Segment::Hold]).await;
        let mut policy = fast_policy();
        policy.backoff_base = Duration::from_millis(4000);
        let cancel = CancellationToken::new();
        let policy = Arc::new(policy);
        let ledger = Arc::new(Recorder::default());
        let request = generation_request();
        let provider_ref = provider.clone();
        let cancel_for_task = cancel.clone();
        let ledger_for_task = ledger.clone();
        let task = tokio::spawn(async move {
            run_generation(
                provider_ref.as_ref(),
                request,
                cancel_for_task,
                &policy,
                ledger_for_task.as_ref(),
                tx,
            )
            .await
        });
        // 头阶段看门狗触发并结算失败后，取消落在退避等待里。
        while !ledger
            .log
            .lock()
            .expect("log")
            .iter()
            .any(|entry| entry.starts_with("S1:Failed"))
        {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        cancel.cancel();
        let outcome = task.await.expect("task joins").expect("cancelled is Ok");
        assert_eq!(
            outcome,
            RunOutcome::Cancelled {
                requests: 1,
                usage: Usage::default()
            }
        );
    }

    #[tokio::test]
    async fn pre_cancelled_run_makes_no_request() {
        let (server, provider, tx, _rx) = setup(sse_ok_script("data: [DONE]\n\n")).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            cancel,
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Cancelled {
                requests: 0,
                usage: Usage::default()
            })
        );
        assert_eq!(server.request_count(), 0);
    }

    #[tokio::test]
    async fn backpressure_queue_bounds_delivery() {
        let body = format!(
            "{}{}{}{}data: [DONE]\n\n",
            sse_text_frame("A"),
            sse_text_frame("B"),
            sse_text_frame("C"),
            sse_finish_frame("stop")
        );
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(body.as_bytes().to_vec()),
        ];
        let (_server, provider, _default_tx, _default_rx) = setup(script).await;
        let (tx, mut rx) = mpsc::channel::<String>(1);
        // 容量 1 的队列需要并发消费者，否则第二条增量就被背压卡住。
        let received = Arc::new(Mutex::new(String::new()));
        let sink = {
            let received = received.clone();
            tokio::spawn(async move {
                while let Some(chunk) = rx.recv().await {
                    received.lock().expect("sink lock").push_str(&chunk);
                }
            })
        };
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 1,
                usage: Usage::default(),
            })
        );
        sink.await.expect("sink joins");
        assert_eq!(received.lock().expect("sink lock").clone(), "ABC");
    }

    #[tokio::test]
    async fn full_queue_cancel_interrupts_send() {
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(sse_text_frame("A").into_bytes()),
            Segment::BodyChunk(sse_text_frame("B").into_bytes()),
            Segment::Hold,
        ];
        let (_server, provider, _default_tx, _default_rx) = setup(script).await;
        let (tx, mut rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let policy = Arc::new(fast_policy());
        let request = generation_request();
        let provider_ref = provider.clone();
        let cancel_for_task = cancel.clone();
        let task = tokio::spawn(async move {
            run_generation(
                provider_ref.as_ref(),
                request,
                cancel_for_task,
                &policy,
                &AllowAll,
                tx,
            )
            .await
        });
        assert_eq!(rx.recv().await, Some("A".to_owned()));
        cancel.cancel();
        let outcome = task.await.expect("task joins").expect("cancelled is Ok");
        assert_eq!(
            outcome,
            RunOutcome::Cancelled {
                requests: 1,
                usage: Usage::default()
            }
        );
    }

    #[tokio::test]
    async fn head_stall_during_zero_capacity_backpressure() {
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(sse_text_frame("A").into_bytes()),
            Segment::Hold,
        ];
        let (server, provider, _default_tx, _default_rx) = setup(script).await;
        // 预占容量 1 的唯一槽位：首个安全增量的发送立即背压，首字节前头看门狗触发。
        let (tx, mut rx) = mpsc::channel(1);
        tx.send("占位".to_owned())
            .await
            .expect("prefill the only slot");
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(error, RunError::Stalled);
        assert_eq!(server.request_count(), 3);
        assert_eq!(drain(&mut rx), "占位");
    }

    #[tokio::test]
    async fn idle_stall_during_full_queue_backpressure() {
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(sse_text_frame("A").into_bytes()),
            Segment::BodyChunk(sse_text_frame("B").into_bytes()),
            Segment::Hold,
        ];
        let (_server, provider, _default_tx, _default_rx) = setup(script).await;
        let (tx, mut rx) = mpsc::channel(1);
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        let source = provider_error(&error).expect("provider failure");
        assert_eq!(source.code_at_boundary(true), "aborted");
        assert_eq!(drain(&mut rx), "A");
    }

    #[tokio::test]
    async fn closed_consumer_ends_as_cancelled() {
        let body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("X"),
            sse_finish_frame("stop")
        );
        let (server, provider, tx, rx) = setup(sse_ok_script(&body)).await;
        drop(rx);
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Cancelled {
                requests: 1,
                usage: Usage::default()
            })
        );
        assert_eq!(server.request_count(), 1);
    }

    /// 可切换拒绝的预算口：先拒绝验证「不发请求」，放开后验证预留-结算闭环。
    struct Toggle {
        deny: std::sync::atomic::AtomicBool,
        settled: Mutex<Vec<String>>,
    }

    impl BudgetPort for Toggle {
        fn reserve(&self, _attempt: &AttemptIdentity) -> Result<(), BudgetDenial> {
            if self.deny.load(std::sync::atomic::Ordering::SeqCst) {
                Err(BudgetDenial {
                    reason: "余额不足".into(),
                })
            } else {
                Ok(())
            }
        }

        fn settle(&self, _attempt: &AttemptIdentity, _outcome: &AttemptOutcome) {
            self.settled.lock().expect("settled").push("settled".into());
        }
    }

    #[tokio::test]
    async fn budget_denial_prevents_any_http_request() {
        let ok_body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("T"),
            sse_finish_frame("stop")
        );
        let (server, provider, tx, _rx) = setup(sse_ok_script(&ok_body)).await;
        let port = Toggle {
            deny: std::sync::atomic::AtomicBool::new(true),
            settled: Mutex::new(Vec::new()),
        };
        let error = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &port,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            RunError::Budget {
                reason: "余额不足".into()
            }
        );
        assert_eq!(server.request_count(), 0);
        assert!(port.settled.lock().expect("settled").is_empty());

        // 放开后同一端口正常走预留-结算闭环。
        port.deny.store(false, std::sync::atomic::Ordering::SeqCst);
        let (tx, _rx) = mpsc::channel(32);
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &port,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 1,
                usage: Usage::default(),
            })
        );
        assert_eq!(port.settled.lock().expect("settled").len(), 1);
    }

    #[derive(Default)]
    struct Recorder {
        log: Mutex<Vec<String>>,
    }

    impl BudgetPort for Recorder {
        fn reserve(&self, attempt: &AttemptIdentity) -> Result<(), BudgetDenial> {
            self.log
                .lock()
                .expect("log")
                .push(format!("R{}:{:?}", attempt.attempt, attempt.kind));
            Ok(())
        }

        fn settle(&self, attempt: &AttemptIdentity, outcome: &AttemptOutcome) {
            self.log
                .lock()
                .expect("log")
                .push(format!("S{}:{:?}", attempt.attempt, outcome));
        }
    }

    /// 供断言使用的错误下钻；None 分支由非 provider 失败的用例覆盖。
    fn provider_error(error: &RunError) -> Option<&ProviderError> {
        match error {
            RunError::Provider(source) => Some(source),
            RunError::Stalled
            | RunError::EmptyOutput { .. }
            | RunError::Budget { .. }
            | RunError::InvalidPolicy(_) => None,
        }
    }

    #[test]
    fn provider_error_helper_separates_domains() {
        assert!(provider_error(&RunError::Stalled).is_none());
        assert!(provider_error(&RunError::Provider(ProviderError::Auth)).is_some());
    }

    fn reserves(log: &[String]) -> Vec<String> {
        log.iter().filter(|e| e.starts_with('R')).cloned().collect()
    }

    #[tokio::test]
    async fn ledger_records_attempt_lifecycle() {
        let ok_body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("L"),
            sse_finish_frame("stop")
        );
        let empty_body = format!("{}data: [DONE]\n\n", sse_finish_frame("stop"));
        // 传输重试路径：500 失败后重试成功。
        let server =
            ScriptedServer::start_sequence(vec![http_error_script(500), sse_ok_script(&ok_body)])
                .await;
        let provider = Arc::new(
            DeepSeek::new(
                format!("http://{}", server.addr),
                SecretString::new("k".into()),
            )
            .unwrap(),
        );
        let ledger = Arc::new(Recorder::default());
        let (tx, _rx) = mpsc::channel(32);
        let outcome = run_generation(
            provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            ledger.as_ref(),
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 2,
                usage: Usage::default(),
            })
        );
        let log = ledger.log.lock().expect("log").clone();
        assert_eq!(reserves(&log), ["R1:Initial", "R2:TransportRetry"]);
        assert!(log.iter().any(|e| e.starts_with("S1:Failed")));
        assert!(log.iter().any(|e| e.starts_with("S2:Settled")));

        // 温度阶梯路径：空输出后按 TemperatureStep 重试成功（两种重试互斥，分开验证）。
        let ladder_server = ScriptedServer::start_sequence(vec![
            sse_ok_script(&empty_body),
            sse_ok_script(&ok_body),
        ])
        .await;
        let ladder_provider = Arc::new(
            DeepSeek::new(
                format!("http://{}", ladder_server.addr),
                SecretString::new("k".into()),
            )
            .unwrap(),
        );
        let ladder_ledger = Arc::new(Recorder::default());
        let (tx, _rx) = mpsc::channel(32);
        let outcome = run_generation(
            ladder_provider.as_ref(),
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            ladder_ledger.as_ref(),
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 2,
                usage: Usage::default(),
            })
        );
        let log = ladder_ledger.log.lock().expect("log").clone();
        assert_eq!(reserves(&log), ["R1:Initial", "R2:TemperatureStep"]);
        assert!(log.iter().any(|e| e.starts_with("S1:Settled")));
        assert!(log.iter().any(|e| e.starts_with("S2:Settled")));
    }

    #[tokio::test]
    async fn invalid_ladder_policies_are_rejected() {
        for ladder in [
            vec![1.05, 1.1, 1.15],
            vec![1.2, 1.1],
            vec![1.0, 1.1],
            vec![2.5],
        ] {
            let (server, provider, tx, _rx) = setup(sse_ok_script("data: [DONE]\n\n")).await;
            let mut policy = fast_policy();
            policy.ladder = ladder.clone();
            let error = run_generation(
                provider.as_ref(),
                generation_request(),
                CancellationToken::new(),
                &policy,
                &AllowAll,
                tx,
            )
            .await
            .unwrap_err();
            assert!(
                matches!(error, RunError::InvalidPolicy(_)),
                "ladder {ladder:?}"
            );
            assert_eq!(server.request_count(), 0);
        }
    }

    /// 可编排假 Provider：不经网络直接给出增量序列，覆盖调度器对异常流的防御分支。
    struct FakeProvider {
        deltas: Vec<Result<ProviderDelta, crate::error::ProviderError>>,
    }

    impl Provider for FakeProvider {
        fn capabilities(
            &self,
            model: &str,
            _mode: RequestMode,
        ) -> crate::provider::ProviderCapabilities {
            crate::provider::ProviderCapabilities {
                model: model.to_owned(),
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
            _request: ProviderRequest,
            _cancellation: CancellationToken,
        ) -> crate::provider::StartFuture<'a> {
            let deltas = self.deltas.clone();
            Box::pin(async move { Ok(futures::stream::iter(deltas).boxed()) })
        }
    }

    #[tokio::test]
    async fn stream_ending_without_finish_is_interrupted() {
        let provider = FakeProvider {
            deltas: vec![Ok(ProviderDelta::Usage(Usage {
                prompt_tokens: 3,
                completion_tokens: 0,
            }))],
        };
        let (tx, _rx) = mpsc::channel(32);
        let error = run_generation(
            &provider,
            generation_request(),
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            RunError::Provider(crate::error::ProviderError::Interrupted)
        );
    }

    #[tokio::test]
    async fn chat_mode_generation_runs_the_same_pipeline() {
        let provider = FakeProvider {
            deltas: vec![
                Ok(ProviderDelta::Text("嗨".into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ],
        };
        let mut request = generation_request();
        request.provider_request.input = crate::provider::ProviderInput::Chat(ChatInput {
            messages: vec![crate::provider::ChatMessage {
                role: crate::provider::ChatRole::User,
                content: "问".into(),
            }],
            assistant_prefix: None,
        });
        let (tx, mut rx) = mpsc::channel(32);
        let outcome = run_generation(
            &provider,
            request,
            CancellationToken::new(),
            &fast_policy(),
            &AllowAll,
            tx,
        )
        .await;
        assert_eq!(
            outcome,
            Ok(RunOutcome::Completed {
                finish: FinishReason::Stop,
                requests: 1,
                usage: Usage::default(),
            })
        );
        assert_eq!(drain(&mut rx), "嗨");
    }

    #[test]
    fn policy_defaults_match_contract() {
        let policy = RunPolicy::default();
        assert_eq!(policy.head_budget, Duration::from_secs(30));
        assert_eq!(policy.idle_budget, Duration::from_secs(90));
        assert_eq!(policy.transport_retries, 2);
        assert_eq!(policy.backoff_base, Duration::from_millis(500));
        assert_eq!(policy.ladder, vec![1.1, 1.2]);
        assert_eq!(policy.max_requests, 3);
    }

    #[test]
    fn backoff_delay_is_exponential_with_bounded_jitter() {
        let base = Duration::from_millis(500);
        assert_eq!(backoff_delay(base, 1, 0.0), Duration::from_millis(500));
        assert_eq!(backoff_delay(base, 2, 0.0), Duration::from_millis(1000));
        assert_eq!(backoff_delay(base, 3, 0.0), Duration::from_millis(2000));
        assert_eq!(backoff_delay(base, 1, 0.25), Duration::from_millis(625));
        assert_eq!(backoff_delay(base, 1, -0.25), Duration::from_millis(375));
        // 超出 ±25% 的抖动被钳制。
        assert_eq!(backoff_delay(base, 1, 0.9), Duration::from_millis(625));
        // 指数上限 2^30，避免移位溢出。
        assert_eq!(
            backoff_delay(base, 40, 0.0),
            Duration::from_millis(500 * (1u64 << 30))
        );
    }

    #[test]
    fn clock_jitter_stays_within_bounds() {
        for _ in 0..100 {
            let jitter = clock_jitter();
            assert!((-0.25..=0.25).contains(&jitter));
        }
    }
    #[tokio::test]
    async fn confirmed_output_needs_receipt_and_closed_queue_is_not_delivery() {
        let (tx, rx) = mpsc::channel::<PendingText>(1);
        drop(rx);
        assert!(!OutputPort::send(&tx, "discarded".into()).await);
        let (tx, mut rx) = mpsc::channel::<PendingText>(1);
        let pending =
            tokio::spawn(async move { OutputPort::send(&tx, "unconfirmed".into()).await });
        drop(rx.recv().await.unwrap());
        assert!(!pending.await.unwrap());
    }
    #[derive(Default)]
    struct Outcomes(Mutex<Vec<(AttemptIdentity, AttemptOutcome)>>);
    impl BudgetPort for Outcomes {
        fn reserve(&self, _: &AttemptIdentity) -> Result<(), BudgetDenial> {
            Ok(())
        }
        fn settle(&self, identity: &AttemptIdentity, outcome: &AttemptOutcome) {
            self.0.lock().unwrap().push((identity.clone(), *outcome));
        }
    }

    #[tokio::test]
    async fn absent_and_explicit_zero_usage_remain_distinct_in_budget_lifecycle() {
        for usage in [None, Some(Usage::default())] {
            let mut deltas = vec![Ok(ProviderDelta::Text("正文".into()))];
            if let Some(usage) = usage {
                deltas.push(Ok(ProviderDelta::Usage(usage)));
            }
            deltas.push(Ok(ProviderDelta::Finish(ProviderFinish::Stop)));
            let provider = FakeProvider { deltas };
            let port = Outcomes::default();
            let (tx, _rx) = mpsc::channel(32);
            run_generation(
                &provider,
                generation_request(),
                CancellationToken::new(),
                &RunPolicy::default(),
                &port,
                tx,
            )
            .await
            .unwrap();
            let entries = port.0.lock().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].1, AttemptOutcome::Settled { usage });
        }
    }

    struct WaitingProvider {
        entered: Arc<tokio::sync::Notify>,
        released: Arc<tokio::sync::Notify>,
    }
    impl Provider for WaitingProvider {
        fn capabilities(
            &self,
            model: &str,
            mode: RequestMode,
        ) -> crate::provider::ProviderCapabilities {
            FakeProvider { deltas: vec![] }.capabilities(model, mode)
        }
        fn start(
            &self,
            _: ProviderRequest,
            _: CancellationToken,
        ) -> crate::provider::StartFuture<'_> {
            Box::pin(async {
                self.entered.notify_one();
                self.released.notified().await;
                Ok(futures::stream::empty().boxed())
            })
        }
    }
    #[tokio::test]
    async fn dropping_generation_settles_reserved_attempt_once_as_unconfirmed() {
        for abort in [true, false] {
            let entered = Arc::new(tokio::sync::Notify::new());
            let released = Arc::new(tokio::sync::Notify::new());
            let provider = Arc::new(WaitingProvider {
                entered: entered.clone(),
                released: released.clone(),
            });
            let port = Arc::new(Outcomes::default());
            let observed = port.clone();
            let task = tokio::spawn(async move {
                let (tx, _rx) = mpsc::channel(32);
                let policy = RunPolicy {
                    transport_retries: 0,
                    ..RunPolicy::default()
                };
                run_generation(
                    provider.as_ref(),
                    generation_request(),
                    CancellationToken::new(),
                    &policy,
                    port.as_ref(),
                    tx,
                )
                .await
            });
            entered.notified().await;
            if abort {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                released.notify_one();
                assert!(task.await.unwrap().is_err());
            }
            let entries = observed.0.lock().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0].1,
                if abort {
                    AttemptOutcome::Cancelled
                } else {
                    AttemptOutcome::Failed
                }
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn confirmed_delivery_timeout_is_single_attempt_and_settles_unknown_usage() {
        let provider = FakeProvider {
            deltas: vec![
                Ok(ProviderDelta::Text("text".into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ],
        };
        let port = Arc::new(Outcomes::default());
        let observed = port.clone();
        let (tx, mut rx) = mpsc::channel::<PendingText>(32);
        let task = tokio::spawn(async move {
            run_generation_with_output(
                &provider,
                generation_request(),
                CancellationToken::new(),
                &RunPolicy::default(),
                port.as_ref(),
                &tx,
                "confirmed-timeout",
            )
            .await
        });
        let pending = rx.recv().await.unwrap();
        assert_eq!(task.await.unwrap().unwrap_err(), RunError::Stalled);
        drop(pending);
        let settled = observed.0.lock().unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].1, AttemptOutcome::Failed);
    }

    #[test]
    fn default_ladder_respects_initial_temperature_and_model_capability() {
        let caps = FakeProvider { deltas: vec![] }.capabilities("m", RequestMode::Completion);
        assert_eq!(RunPolicy::default_for(1.0, &caps).ladder, vec![1.1, 1.2]);
        assert!(RunPolicy::default_for(1.2, &caps).ladder.is_empty());
        let mut ineffective = caps.clone();
        ineffective.temperature_effective = false;
        assert!(RunPolicy::default_for(1.0, &ineffective).ladder.is_empty());
        let mut bounded = caps;
        bounded.temperature_max = 1.1;
        assert_eq!(RunPolicy::default_for(1.0, &bounded).ladder, vec![1.1]);
    }
}
