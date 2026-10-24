//! `DeepSeek` adapter：Completion（/beta/completions，FIM）与 Chat / Prefix（(/beta/)chat/completions）。
//!
//! 能力表按官方文档登记（核验 2026-10-05）：completion 端点输出上限 4K（不能拿通用模型上限替代）、
//! 服务端 stop 上限 16、chat 默认启用 thinking（叙事 adapter 显式关闭）。
//! 一次 [`Provider::start`] 恰好一次 HTTP；错误按 [`crate::error::ProviderError`] 结构化，
//! 响应正文不进错误。SSE 事件只取 `data`，`[DONE]` 为结束标记，未见到合法 finish 的流按
//! 中断分类（交付边界由调度器决定）。

use super::models::DEEPSEEK_MODELS;
use crate::decode::IncrementalUtf8;
use crate::error::ProviderError;
use crate::provider::{
    ChatRole, Credential, Provider, ProviderCapabilities, ProviderDelta, ProviderFinish,
    ProviderRequest, RequestMode, StartFuture, Usage,
};
use crate::sse::{IncrementalSse, SseError};
use futures::StreamExt;
use secrecy::ExposeSecret;
use serde::Deserialize;
use std::collections::VecDeque;

/// DeepSeek 当前账户的余额快照。金额保持服务商返回的十进制词法，不经过浮点数。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceSnapshot {
    pub provider_id: String,
    pub queried_at: String,
    pub stale: bool,
    pub status: BalanceStatus,
    pub accounts: Vec<BalanceAccount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BalanceStatus {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceAccount {
    pub currency: String,
    pub total: String,
    pub granted: String,
    pub topped_up: String,
}

#[derive(Deserialize)]
struct BalanceResponse {
    is_available: bool,
    balance_infos: Vec<BalanceInfo>,
}

#[derive(Deserialize)]
struct BalanceInfo {
    currency: String,
    total_balance: String,
    granted_balance: String,
    topped_up_balance: String,
}

/// 查询用户明确请求的 DeepSeek 账户余额；任何传输 / 响应异常都只返回未知状态。
///
/// # Errors
/// 本函数不返回供应商原文或可泄漏凭据的错误；异常被折叠为 `unknown` 快照。
pub async fn query_balance(
    client: &reqwest::Client,
    key: &Credential,
    queried_at: String,
) -> BalanceSnapshot {
    balance_snapshot_at(
        client,
        key,
        queried_at,
        "https://api.deepseek.com/user/balance",
    )
    .await
}

async fn balance_snapshot_at(
    client: &reqwest::Client,
    key: &Credential,
    queried_at: String,
    endpoint: &str,
) -> BalanceSnapshot {
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        query_balance_at(client, key, endpoint),
    )
    .await
    .unwrap_or(Err(()));
    match response {
        Ok((status, accounts)) => BalanceSnapshot {
            provider_id: "deepseek".into(),
            queried_at,
            stale: false,
            status,
            accounts,
        },
        Err(()) => unknown_balance(queried_at),
    }
}

fn unknown_balance(queried_at: String) -> BalanceSnapshot {
    BalanceSnapshot {
        provider_id: "deepseek".into(),
        queried_at,
        stale: false,
        status: BalanceStatus::Unknown,
        accounts: Vec::new(),
    }
}

async fn query_balance_at(
    client: &reqwest::Client,
    key: &Credential,
    endpoint: &str,
) -> Result<(BalanceStatus, Vec<BalanceAccount>), ()> {
    const MAX_RESPONSE_BYTES: usize = 16 * 1024;
    let response = client
        .get(endpoint)
        .bearer_auth(key.expose_secret())
        .send()
        .await
        .map_err(|_| ())?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(());
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(());
        }
        body.extend_from_slice(&chunk);
    }
    let response: BalanceResponse =
        aoidos_json::decode_bytes(&body, MAX_RESPONSE_BYTES).map_err(|_| ())?;
    let accounts = response
        .balance_infos
        .into_iter()
        .map(balance_account)
        .collect::<Result<Vec<_>, _>>()?;
    if accounts.is_empty() {
        return Err(());
    }
    Ok((
        if response.is_available {
            BalanceStatus::Available
        } else {
            BalanceStatus::Unavailable
        },
        accounts,
    ))
}

fn balance_account(info: BalanceInfo) -> Result<BalanceAccount, ()> {
    if !matches!(info.currency.as_str(), "CNY" | "USD")
        || !valid_balance_amount(&info.total_balance)
        || !valid_balance_amount(&info.granted_balance)
        || !valid_balance_amount(&info.topped_up_balance)
    {
        return Err(());
    }
    Ok(BalanceAccount {
        currency: info.currency,
        total: info.total_balance,
        granted: info.granted_balance,
        topped_up: info.topped_up_balance,
    })
}

fn valid_balance_amount(value: &str) -> bool {
    if value.len() > 42 {
        return false;
    }
    let Some((whole, fraction)) = value.split_once('.') else {
        return canonical_unsigned_integer(value);
    };
    canonical_unsigned_integer(whole)
        && !fraction.is_empty()
        && fraction.len() <= 9
        && fraction.bytes().all(|byte| byte.is_ascii_digit())
}

fn canonical_unsigned_integer(value: &str) -> bool {
    value == "0"
        || (!value.is_empty()
            && !value.starts_with('0')
            && value.bytes().all(|byte| byte.is_ascii_digit()))
}

const STOP_LIMIT: usize = 16;
/// DeepSeek Beta FIM 接口文档规定的端点级输出上限。
const FIM_MAX_OUTPUT_TOKENS: u32 = 4_096;

/// `DeepSeek` 供应商适配器。`base` 形如 `https://api.deepseek.com`，测试注入回环地址。
pub struct DeepSeek {
    base: String,
    key: Credential,
    client: reqwest::Client,
}

impl DeepSeek {
    /// 构建 adapter：传输客户端由 [`crate::proxy::build_client`] 统一定型
    /// （`retry(never)` + 禁自动重定向；默认空快照 = 直连，代理配置由装配层
    /// 从冻结 profile 解析后经 [`Self::with_client`] 注入）。
    ///
    /// # Errors
    /// HTTP 客户端构建失败（TLS provider / backend 初始化异常）时返回 reqwest 错误。
    pub fn new(base: String, key: Credential) -> reqwest::Result<Self> {
        let client = crate::proxy::default_transport()?;
        Ok(Self::with_client(base, key, client))
    }

    /// 使用装配层构造好的客户端（代理模式 / 信任根已定）。
    #[must_use]
    pub fn with_client(base: String, key: Credential, client: reqwest::Client) -> Self {
        Self { base, key, client }
    }

    fn endpoint(&self, prefix_mode: bool, mode: RequestMode) -> String {
        match mode {
            RequestMode::Completion => format!("{}/beta/completions", self.base),
            RequestMode::Chat if prefix_mode => format!("{}/beta/chat/completions", self.base),
            RequestMode::Chat => format!("{}/chat/completions", self.base),
        }
    }
}

pub(super) fn deepseek_capabilities(model: &str, mode: RequestMode) -> ProviderCapabilities {
    let spec = DEEPSEEK_MODELS.iter().find(|spec| spec.id == model);
    let known = spec.is_some();
    let (max_output, thinking, temperature_effective) = match mode {
        // FIM 使用端点级限制，不使用型号的 Chat 输出上限。
        RequestMode::Completion => (if known { FIM_MAX_OUTPUT_TOKENS } else { 0 }, false, known),
        // chat 默认启用 thinking；thinking 下 temperature 无效，
        // 叙事 profile 显式关闭后生效，能力按形态声明、由协调器结合 profile 判定。
        RequestMode::Chat => (spec.map_or(0, |spec| spec.max_output_tokens), known, known),
    };
    ProviderCapabilities {
        model: model.to_owned(),
        context_limit: spec.map(|spec| spec.context_window),
        completion: known && mode == RequestMode::Completion,
        chat: known && mode == RequestMode::Chat,
        prefix: known && mode == RequestMode::Chat,
        thinking,
        stop_limit: if known { STOP_LIMIT } else { 0 },
        max_output_tokens: max_output,
        temperature_effective,
        temperature_min: 0.0,
        temperature_max: 2.0,
    }
}

impl Provider for DeepSeek {
    fn capabilities(&self, model: &str, mode: RequestMode) -> ProviderCapabilities {
        deepseek_capabilities(model, mode)
    }
    fn start(
        &self,
        request: ProviderRequest,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> StartFuture<'_> {
        Box::pin(async move {
            let prefix_mode = matches!(&request.input, crate::provider::ProviderInput::Chat(chat) if chat.assistant_prefix.is_some());
            let url = self.endpoint(prefix_mode, request.mode());
            let body = build_body(&request);
            let body = aoidos_json::to_vec(&body).map_err(err_chunk_json)?;
            let response = self
                .client
                .post(url)
                .bearer_auth(self.key.expose_secret())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(err_send)?;
            let status = response.status().as_u16();
            if status != 200 {
                return Err(ProviderError::from_status(status));
            }
            let stream = AdapterStream::new(response.bytes_stream());
            // unfold 把逐步推进变成增量流；take_until_owned 让取消立即停止产出，
            // 传输句柄由调度器丢弃时连接随之关闭。
            let deltas = futures::stream::unfold(stream, |mut state| async move {
                state.step().await.map(|item| (item, state))
            });
            Ok(deltas.take_until(cancellation.cancelled_owned()).boxed())
        })
    }
}

/// send 阶段传输错误 → 结构化分类（对 error source 链下钻识别 TLS）。
pub(super) fn err_send(error: reqwest::Error) -> ProviderError {
    ProviderError::from_reqwest(&error, crate::error::RequestPhase::Send)
}

/// 请求编码或 data 解码不合法：按 bad-response 拒绝，不重试。
pub(super) fn err_chunk_json(_: aoidos_json::Error) -> ProviderError {
    ProviderError::BadResponse {
        reason: "json",
        status: None,
    }
}

fn build_body(request: &ProviderRequest) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": request.model,
        "stream": true,
        "max_tokens": request.sampling.max_tokens,
        "temperature": request.sampling.temperature,
    });
    if !request.stops.is_empty() {
        body["stop"] = serde_json::json!(request.stops);
    }
    match &request.input {
        crate::provider::ProviderInput::Completion(input) => {
            body["prompt"] = serde_json::json!(input.prompt);
        }
        crate::provider::ProviderInput::Chat(input) => {
            let mut messages: Vec<serde_json::Value> = input
                .messages
                .iter()
                .map(|message| {
                    serde_json::json!({ "role": role_name(message.role), "content": message.content })
                })
                .collect();
            if let Some(prefix) = &input.assistant_prefix {
                // Chat Prefix：末条 assistant 消息带 prefix=true，走 /beta 端点。
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": prefix,
                    "prefix": true,
                }));
            }
            // 叙事 adapter 显式关闭 thinking，不依赖服务端默认值。
            body["thinking"] = serde_json::json!({ "type": "disabled" });
            body["messages"] = serde_json::json!(messages);
        }
    }
    body
}

fn role_name(role: ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
    }
}

/// SSE data 载荷的可解析形状；缺失字段按空处理。
#[derive(Deserialize, Default)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<StreamUsage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    /// choices 的 index；缺失按 0 处理（DeepSeek 恒发该字段）。
    #[serde(default)]
    index: i64,
    #[serde(default)]
    delta: Option<StreamDelta>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[derive(Deserialize)]
struct StreamUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    prompt_cache_hit_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<StreamPromptUsage>,
    #[serde(default)]
    completion_tokens_details: Option<StreamCompletionUsage>,
}

#[derive(Deserialize)]
struct StreamPromptUsage {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct StreamCompletionUsage {
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}

/// 传输适配流：字节 → 增量 UTF-8 → 增量 SSE → JSON → 规范化增量。
pub(super) struct AdapterStream<B> {
    body: B,
    decode: IncrementalUtf8,
    sse: IncrementalSse,
    queue: VecDeque<ProviderDelta>,
    finish: Option<ProviderFinish>,
    done_marker: bool,
    /// 终止后不再产出：错误与 Finish 都是一次性结尾，之后的 poll 一律 None。
    finished: bool,
}

impl<B> AdapterStream<B>
where
    B: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin + Send + 'static,
{
    pub(super) fn new(body: B) -> Self {
        Self {
            body,
            decode: IncrementalUtf8::new(),
            sse: IncrementalSse::new(),
            queue: VecDeque::new(),
            finish: None,
            done_marker: false,
            finished: false,
        }
    }

    /// 处理一段 data 载荷：只抽 index 0，usage-only 与空 choices 合法。
    fn ingest_data(&mut self, data: &str) -> Result<(), ProviderError> {
        if data == "[DONE]" {
            self.done_marker = true;
            return Ok(());
        }
        let chunk: StreamChunk =
            aoidos_json::decode(data, crate::sse::MAX_EVENT_BYTES).map_err(err_chunk_json)?;
        if let Some(usage) = chunk.usage
            && let (Some(prompt_tokens), Some(completion_tokens)) =
                (usage.prompt_tokens, usage.completion_tokens)
        {
            let cached_prompt_tokens = usage.prompt_cache_hit_tokens.or_else(|| {
                usage
                    .prompt_tokens_details
                    .and_then(|details| details.cached_tokens)
            });
            self.queue.push_back(ProviderDelta::Usage(Usage {
                prompt_tokens,
                completion_tokens,
                cached_prompt_tokens,
                // DeepSeek 用输入总量与缓存命中量表示命中 / 未命中；没有独立缓存写入计数。
                // 未命中部分按普通输入计价，显式标记不存在独立写入量，避免账本误留待结算。
                cache_creation_tokens: Some(0),
                reasoning_tokens: usage
                    .completion_tokens_details
                    .and_then(|details| details.reasoning_tokens),
            }));
        }
        // 只抽 index=0（契约：不启用 n>1）；其余下标一律忽略。
        let Some(choice) = chunk.choices.into_iter().find(|choice| choice.index == 0) else {
            // 合法空 choices / usage-only chunk / 无 0 号候选。
            return Ok(());
        };
        match choice.finish_reason.as_deref() {
            None => {}
            Some("stop") => self.finish = Some(ProviderFinish::Stop),
            Some("length") => self.finish = Some(ProviderFinish::Length),
            Some("aborted" | "insufficient_system_resource") => {
                return Err(ProviderError::ServerAborted);
            }
            // content_filter、未启用的 tool_calls 与未知 finish 不伪装成功。
            Some(_) => {
                return Err(ProviderError::BadResponse {
                    reason: "finish-reason",
                    status: None,
                });
            }
        }
        if let Some(delta) = choice.delta {
            if let Some(reasoning) = delta.reasoning_content
                && !reasoning.is_empty()
            {
                self.queue.push_back(ProviderDelta::Reasoning(reasoning));
            }
            if let Some(text) = delta.content
                && !text.is_empty()
            {
                self.queue.push_back(ProviderDelta::Text(text));
            }
        }
        if let Some(text) = choice.text
            && !text.is_empty()
        {
            self.queue.push_back(ProviderDelta::Text(text));
        }
        Ok(())
    }

    fn ingest_text(&mut self, text: &str) -> Result<(), ProviderError> {
        for event in feed_sse(&mut self.sse, text)? {
            self.ingest_data(&event.data)?;
        }
        Ok(())
    }

    async fn advance(&mut self) -> Option<reqwest::Result<bytes::Bytes>> {
        self.body.next().await
    }

    /// 推进到产出一条增量或流结束；错误与 Finish 之后流进入终止态。
    pub(super) async fn step(&mut self) -> Option<Result<ProviderDelta, ProviderError>> {
        loop {
            if self.finished {
                return None;
            }
            if let Some(delta) = self.queue.pop_front() {
                return Some(Ok(delta));
            }
            if self.done_marker {
                self.finished = true;
                return match self.finish {
                    Some(finish) => Some(Ok(ProviderDelta::Finish(finish))),
                    // [DONE] 前必须见过合法 finish。
                    None => Some(Err(ProviderError::BadResponse {
                        reason: "missing-finish",
                        status: None,
                    })),
                };
            }
            match self.advance().await {
                Some(Ok(bytes)) => {
                    let Ok(text) = self.decode.feed(&bytes) else {
                        self.finished = true;
                        return Some(Err(ProviderError::BadResponse {
                            reason: "utf8",
                            status: None,
                        }));
                    };
                    if !text.is_empty()
                        && let Err(error) = self.ingest_text(&text)
                    {
                        self.finished = true;
                        return Some(Err(error));
                    }
                }
                Some(Err(error)) => {
                    self.finished = true;
                    return Some(Err(ProviderError::from_reqwest(
                        &error,
                        crate::error::RequestPhase::Body,
                    )));
                }
                None => {
                    // 干净 EOF：冲刷扣留字节与半行事件，仍需 [DONE] 才算合法结束。
                    let utf8_broken = self.decode.finish().is_err();
                    let mut tail_error = None;
                    if !utf8_broken {
                        for event in self.sse.finish() {
                            if let Err(error) = self.ingest_data(&event.data) {
                                tail_error = Some(error);
                                break;
                            }
                        }
                    }
                    let terminal = if utf8_broken {
                        Some(Err(ProviderError::BadResponse {
                            reason: "utf8",
                            status: None,
                        }))
                    } else if let Some(error) = tail_error {
                        Some(Err(error))
                    } else if !self.done_marker {
                        // 未带合法 finish / 结束标记的提前 EOF 按网络中断分类。
                        Some(Err(ProviderError::Interrupted))
                    } else {
                        // 尾部冲刷恰好补上 [DONE]：回到循环顶部按 done 分支发 Finish。
                        None
                    };
                    if let Some(item) = terminal {
                        self.finished = true;
                        return Some(item);
                    }
                }
            }
        }
    }
}

fn sse_error(_: SseError) -> ProviderError {
    // SSE 上限 / 损坏只进错误码，细节不外传。
    ProviderError::BadResponse {
        reason: "sse",
        status: None,
    }
}

fn feed_sse(
    sse: &mut IncrementalSse,
    text: &str,
) -> Result<Vec<crate::sse::SseEvent>, ProviderError> {
    sse.feed(text).map_err(sse_error)
}

#[cfg(test)]
mod tests;
