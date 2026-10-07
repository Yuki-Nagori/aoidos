//! `DeepSeek` adapter：Completion（/beta/completions，FIM）与 Chat / Prefix（(/beta/)chat/completions）。
//!
//! 能力表按官方文档登记（核验 2026-10-05）：completion 端点输出上限 4K（不能拿通用模型上限替代）、
//! 服务端 stop 上限 16、chat 默认启用 thinking（叙事 adapter 显式关闭）。
//! 一次 [`Provider::start`] 恰好一次 HTTP；错误按 [`crate::error::ProviderError`] 结构化，
//! 响应正文不进错误。SSE 事件只取 `data`，`[DONE]` 为结束标记，未见到合法 finish 的流按
//! 中断分类（交付边界由调度器决定）。

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

/// 叙事候选与成本优先候选；其余模型按未知能力处理（全部不支持）。
const KNOWN_MODELS: [&str; 2] = ["deepseek-v4-pro", "deepseek-flash"];
const STOP_LIMIT: usize = 16;
/// FIM 指南给出的端点输出上限。
const COMPLETION_MAX_OUTPUT: u32 = 4096;
/// chat 端点输出上限（v1 初值，黄金样例联调后核验）。
const CHAT_MAX_OUTPUT: u32 = 8192;

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

impl Provider for DeepSeek {
    fn capabilities(&self, model: &str, mode: RequestMode) -> ProviderCapabilities {
        let known = KNOWN_MODELS.contains(&model);
        let (max_output, thinking, temperature_effective) = match mode {
            // FIM 端点为非 thinking；temperature 生效。
            RequestMode::Completion => (COMPLETION_MAX_OUTPUT, false, known),
            // chat 默认启用 thinking；thinking 下 temperature 无效，
            // 叙事 profile 显式关闭后生效，能力按形态声明、由协调器结合 profile 判定。
            RequestMode::Chat => (CHAT_MAX_OUTPUT, known, known),
        };
        ProviderCapabilities {
            model: model.to_owned(),
            completion: known && mode == RequestMode::Completion,
            chat: known && mode == RequestMode::Chat,
            prefix: known && mode == RequestMode::Chat,
            thinking,
            stop_limit: if known { STOP_LIMIT } else { 0 },
            max_output_tokens: if known { max_output } else { 0 },
            temperature_effective,
            temperature_min: 0.0,
            temperature_max: 2.0,
        }
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
            let response = self
                .client
                .post(url)
                .bearer_auth(self.key.expose_secret())
                .json(&body)
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
fn err_send(error: reqwest::Error) -> ProviderError {
    ProviderError::from_reqwest(&error, crate::error::RequestPhase::Send)
}

/// data 载荷不是合法 JSON：协议损坏按 bad-response，不重试。
fn err_chunk_json(_: serde_json::Error) -> ProviderError {
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
}

/// 传输适配流：字节 → 增量 UTF-8 → 增量 SSE → JSON → 规范化增量。
struct AdapterStream<B> {
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
    fn new(body: B) -> Self {
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
        let chunk: StreamChunk = serde_json::from_str(data).map_err(err_chunk_json)?;
        if let Some(StreamUsage {
            prompt_tokens: Some(prompt_tokens),
            completion_tokens: Some(completion_tokens),
        }) = chunk.usage
        {
            self.queue.push_back(ProviderDelta::Usage(Usage {
                prompt_tokens,
                completion_tokens,
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
    async fn step(&mut self) -> Option<Result<ProviderDelta, ProviderError>> {
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
mod tests {
    use super::*;
    use crate::provider::{ChatInput, ProviderStream};
    use crate::testserver::{
        ScriptedServer, Segment, http_error_script, sse_completion_frame, sse_finish_frame,
        sse_ok_script, sse_text_frame,
    };
    use futures::StreamExt;
    use secrecy::SecretString;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn with_client_uses_injected_transport() {
        // 装配层构造的客户端原样持有；base 独立于客户端注入。
        let adapter = DeepSeek::with_client(
            "https://api.deepseek.com".into(),
            SecretString::from(String::from("sk-x")),
            reqwest::Client::new(),
        );
        assert_eq!(adapter.base, "https://api.deepseek.com");
    }

    fn adapter(server: &ScriptedServer) -> DeepSeek {
        DeepSeek::new(
            format!("http://{}", server.addr),
            SecretString::new("test-key".into()),
        )
        .expect("client builds")
    }

    async fn collect(stream: ProviderStream) -> Vec<Result<ProviderDelta, ProviderError>> {
        let mut out = Vec::new();
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            out.push(item);
        }
        out
    }

    fn completion_request(stops: Vec<String>) -> ProviderRequest {
        ProviderRequest {
            model: "deepseek-v4-pro".into(),
            input: crate::provider::ProviderInput::Completion(crate::provider::CompletionInput {
                prompt: "<narration>".into(),
            }),
            sampling: crate::sampling::Sampling {
                temperature: 1.0,
                max_tokens: 2048,
            },
            stops,
        }
    }

    #[test]
    fn capabilities_known_models() {
        let deepseek = DeepSeek::new(
            "https://api.deepseek.com".into(),
            SecretString::new("k".into()),
        )
        .unwrap();
        let completion = deepseek.capabilities("deepseek-v4-pro", RequestMode::Completion);
        assert!(completion.completion);
        assert!(!completion.chat);
        assert!(!completion.thinking);
        assert!(completion.temperature_effective);
        assert_eq!(completion.stop_limit, 16);
        assert_eq!(completion.max_output_tokens, 4096);
        let chat = deepseek.capabilities("deepseek-flash", RequestMode::Chat);
        assert!(chat.chat);
        assert!(chat.prefix);
        assert!(chat.thinking);
        assert_eq!(chat.max_output_tokens, 8192);
    }

    #[test]
    fn capabilities_unknown_model_reports_nothing() {
        let deepseek = DeepSeek::new(
            "https://api.deepseek.com".into(),
            SecretString::new("k".into()),
        )
        .unwrap();
        let caps = deepseek.capabilities("gpt-x", RequestMode::Chat);
        assert!(!caps.chat);
        assert!(!caps.completion);
        assert!(!caps.prefix);
        assert_eq!(caps.stop_limit, 0);
        assert_eq!(caps.max_output_tokens, 0);
        assert!(!caps.temperature_effective);
    }

    #[tokio::test]
    async fn completion_stream_yields_text_usage_finish() {
        let body = format!(
            "{}{}{}data: [DONE]\n\n",
            sse_completion_frame("风吹过。"),
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
            sse_finish_frame("stop"),
        );
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let stream = deepseek
            .start(
                completion_request(vec!["</narration>".into()]),
                CancellationToken::new(),
            )
            .await
            .expect("stream starts");
        let deltas = collect(stream).await;
        assert_eq!(
            deltas,
            vec![
                Ok(ProviderDelta::Text("风吹过。".into())),
                Ok(ProviderDelta::Usage(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5
                })),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ]
        );
    }

    #[tokio::test]
    async fn request_carries_frozen_fields_and_auth() {
        let server = ScriptedServer::start(sse_ok_script(&format!(
            "{}data: [DONE]\n\n",
            sse_finish_frame("stop")
        )))
        .await;
        let deepseek = adapter(&server);
        let stream = deepseek
            .start(
                completion_request(vec!["</narration>".into()]),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        collect(stream).await;
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/beta/completions");
        assert_eq!(
            requests[0].header("authorization"),
            Some("Bearer test-key".to_owned())
        );
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["model"], "deepseek-v4-pro");
        assert_eq!(body["prompt"], "<narration>");
        assert_eq!(body["stream"], true);
        assert_eq!(body["temperature"], 1.0);
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["stop"], serde_json::json!(["</narration>"]));
    }

    #[tokio::test]
    async fn empty_stops_omit_the_parameter() {
        let server = ScriptedServer::start(sse_ok_script(&format!(
            "{}data: [DONE]\n\n",
            sse_finish_frame("stop")
        )))
        .await;
        let deepseek = adapter(&server);
        collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        let body: serde_json::Value = serde_json::from_slice(&server.requests()[0].body).unwrap();
        assert!(body.get("stop").is_none());
    }

    #[tokio::test]
    async fn chat_prefix_uses_beta_endpoint_and_flag() {
        let server = ScriptedServer::start(sse_ok_script(&format!(
            "{}data: [DONE]\n\n",
            sse_finish_frame("stop")
        )))
        .await;
        let deepseek = adapter(&server);
        let request = ProviderRequest {
            model: "deepseek-flash".into(),
            input: crate::provider::ProviderInput::Chat(ChatInput {
                messages: vec![crate::provider::ChatMessage {
                    role: ChatRole::User,
                    content: "继续".into(),
                }],
                assistant_prefix: Some("夜色".into()),
            }),
            sampling: crate::sampling::Sampling {
                temperature: 1.0,
                max_tokens: 512,
            },
            stops: vec![],
        };
        collect(
            deepseek
                .start(request, CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        let requests = server.requests();
        assert_eq!(requests[0].path, "/beta/chat/completions");
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["thinking"], serde_json::json!({"type": "disabled"}));
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["prefix"], true);
        assert_eq!(messages[1]["content"], "夜色");
    }

    #[tokio::test]
    async fn chat_serializes_system_and_assistant_roles() {
        let server = ScriptedServer::start(sse_ok_script(&format!(
            "{}data: [DONE]\n\n",
            sse_finish_frame("stop")
        )))
        .await;
        let deepseek = adapter(&server);
        let request = ProviderRequest {
            model: "deepseek-flash".into(),
            input: crate::provider::ProviderInput::Chat(ChatInput {
                messages: vec![
                    crate::provider::ChatMessage {
                        role: ChatRole::System,
                        content: "规则".into(),
                    },
                    crate::provider::ChatMessage {
                        role: ChatRole::User,
                        content: "问".into(),
                    },
                    crate::provider::ChatMessage {
                        role: ChatRole::Assistant,
                        content: "旧答".into(),
                    },
                ],
                assistant_prefix: None,
            }),
            sampling: crate::sampling::Sampling {
                temperature: 1.0,
                max_tokens: 64,
            },
            stops: vec![],
        };
        collect(
            deepseek
                .start(request, CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        let body: serde_json::Value = serde_json::from_slice(&server.requests()[0].body).unwrap();
        let roles: Vec<&str> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "user", "assistant"]);
    }

    #[tokio::test]
    async fn stream_is_terminal_after_finish() {
        // 终止一次性契约：Finish 之后再 poll 只得 None，不重复产出增量或错误。
        let body = format!("{}data: [DONE]\n\n", sse_finish_frame("stop"));
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let mut stream = deepseek
            .start(completion_request(vec![]), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            stream.next().await,
            Some(Ok(ProviderDelta::Finish(ProviderFinish::Stop)))
        );
        assert_eq!(stream.next().await, None);
        assert_eq!(stream.next().await, None);
    }

    #[tokio::test]
    async fn done_marker_without_trailing_newline_finishes_normally() {
        // 结束标记不带尾换行：EOF 冲刷补上 [DONE]，随后按 done 分支发出 Finish。
        let body = format!(
            "{}{}data: [DONE]",
            sse_text_frame("尾"),
            sse_finish_frame("stop")
        );
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(body.into_bytes()),
        ];
        let server = ScriptedServer::start(script).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas,
            vec![
                Ok(ProviderDelta::Text("尾".into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ]
        );
    }

    #[tokio::test]
    async fn invalid_json_in_eof_tail_is_bad_response() {
        // 尾部半行事件在干净 EOF 冲刷时派发，JSON 损坏同样归 bad-response。
        let body = format!("{}data: {{bad", sse_text_frame("前文"));
        let script = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(body.into_bytes()),
        ];
        let server = ScriptedServer::start(script).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas.last(),
            Some(&Err(ProviderError::BadResponse {
                reason: "json",
                status: None
            }))
        );
        // 错误项与正常项都能被格式化对比。
        assert!(format_deltas(&deltas).contains("BadResponse"));
    }

    #[tokio::test]
    async fn plain_chat_uses_standard_endpoint_without_prefix_flag() {
        let server = ScriptedServer::start(sse_ok_script(&format!(
            "{}data: [DONE]\n\n",
            sse_finish_frame("stop")
        )))
        .await;
        let deepseek = adapter(&server);
        let request = ProviderRequest {
            model: "deepseek-flash".into(),
            input: crate::provider::ProviderInput::Chat(ChatInput {
                messages: vec![crate::provider::ChatMessage {
                    role: ChatRole::User,
                    content: "你好".into(),
                }],
                assistant_prefix: None,
            }),
            sampling: crate::sampling::Sampling {
                temperature: 1.0,
                max_tokens: 64,
            },
            stops: vec![],
        };
        collect(
            deepseek
                .start(request, CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(server.requests()[0].path, "/chat/completions");
        let body: serde_json::Value = serde_json::from_slice(&server.requests()[0].body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(body["thinking"], serde_json::json!({"type": "disabled"}));
    }

    #[tokio::test]
    async fn only_index_zero_choices_are_consumed() {
        // 契约「只抽 index=0，不启用 n>1」：非 0 下标的候选即使排在前面也不进正文。
        let body = format!(
            "data: {{\"choices\":[{{\"index\":1,\"delta\":{{\"content\":\"X\"}}}}]}}\n\n{}{}data: [DONE]\n\n",
            sse_text_frame("正文"),
            sse_finish_frame("stop"),
        );
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas,
            vec![
                Ok(ProviderDelta::Text("正文".into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ]
        );
    }

    #[tokio::test]
    async fn reasoning_delta_is_not_text() {
        let body = format!(
            "{}{}data: [DONE]\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"思考中\"}}]}\n\n",
            sse_finish_frame("stop"),
        );
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas,
            vec![
                Ok(ProviderDelta::Reasoning("思考中".into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ]
        );
    }

    #[tokio::test]
    async fn length_finish_is_reported() {
        let body = format!("{}data: [DONE]\n\n", sse_finish_frame("length"));
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas,
            vec![Ok(ProviderDelta::Finish(ProviderFinish::Length))]
        );
    }

    #[tokio::test]
    async fn unknown_finish_reason_is_bad_response() {
        let body = format!("{}data: [DONE]\n\n", sse_finish_frame("tool_calls"));
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas.last(),
            Some(&Err(ProviderError::BadResponse {
                reason: "finish-reason",
                status: None
            }))
        );
    }

    #[tokio::test]
    async fn server_aborted_finish_maps_to_dedicated_error() {
        let body = format!("{}data: [DONE]\n\n", sse_finish_frame("aborted"));
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(deltas.last(), Some(&Err(ProviderError::ServerAborted)));
    }

    #[tokio::test]
    async fn done_without_finish_is_bad_response() {
        let body = format!("{}data: [DONE]\n\n", sse_text_frame("文本"));
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas.last(),
            Some(&Err(ProviderError::BadResponse {
                reason: "missing-finish",
                status: None
            }))
        );
    }

    #[tokio::test]
    async fn clean_eof_without_done_marker_is_interrupted() {
        let body = sse_text_frame("半途");
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(deltas.last(), Some(&Err(ProviderError::Interrupted)));
    }

    #[tokio::test]
    async fn invalid_json_payload_is_bad_response() {
        let body = "data: {not json}\n\n".to_owned();
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas.last(),
            Some(&Err(ProviderError::BadResponse {
                reason: "json",
                status: None
            }))
        );
    }

    #[tokio::test]
    async fn invalid_utf8_body_is_bad_response() {
        let segments = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(b"data: \xff\xfe\n\n".to_vec()),
        ];
        let server = ScriptedServer::start(segments).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas.last(),
            Some(&Err(ProviderError::BadResponse {
                reason: "utf8",
                status: None
            }))
        );
    }

    #[tokio::test]
    async fn truncated_multibyte_at_eof_is_bad_response() {
        // 正文以多字节字符的残缺前缀结尾：EOF 扣留字节报 bad-response，不合成 U+FFFD 交付。
        let mut body = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"".to_vec();
        body.extend_from_slice(&"风".as_bytes()[..2]);
        let segments = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(body),
        ];
        let server = ScriptedServer::start(segments).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas.last(),
            Some(&Err(ProviderError::BadResponse {
                reason: "utf8",
                status: None
            }))
        );
    }

    #[tokio::test]
    async fn oversized_sse_event_is_bad_response() {
        let big = "x".repeat(crate::sse::MAX_EVENT_BYTES + 2);
        let body = format!("data: {big}\n\n");
        let server = ScriptedServer::start(sse_ok_script(&body)).await;
        let deepseek = adapter(&server);
        let deltas = collect(
            deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            deltas.last(),
            Some(&Err(ProviderError::BadResponse {
                reason: "sse",
                status: None
            }))
        );
    }

    #[tokio::test]
    async fn http_error_statuses_map_to_categories() {
        for (status, expected) in [
            (401u16, ProviderError::Auth),
            (402, ProviderError::Quota),
            (429, ProviderError::RateLimited),
            (
                404,
                ProviderError::BadResponse {
                    reason: "http-status",
                    status: Some(404),
                },
            ),
        ] {
            let server = ScriptedServer::start(http_error_script(status)).await;
            let deepseek = adapter(&server);
            let result = deepseek
                .start(completion_request(vec![]), CancellationToken::new())
                .await;
            assert_eq!(
                result.err().expect("start fails"),
                expected,
                "status {status}"
            );
        }
    }

    #[tokio::test]
    async fn server_5xx_maps_to_retryable_network() {
        let server = ScriptedServer::start(http_error_script(500)).await;
        let deepseek = adapter(&server);
        let result = deepseek
            .start(completion_request(vec![]), CancellationToken::new())
            .await;
        assert_eq!(
            result.err().expect("start fails"),
            ProviderError::Network {
                source: crate::error::TransportClass::Connect
            }
        );
    }

    #[tokio::test]
    async fn connection_refused_is_network() {
        // 绑定后立即关闭，留下一个几乎必然拒绝连接的端口。
        let server = ScriptedServer::start(vec![]).await;
        let addr = server.addr;
        server.shutdown().await;
        let deepseek =
            DeepSeek::new(format!("http://{addr}"), SecretString::new("k".into())).unwrap();
        let result = deepseek
            .start(completion_request(vec![]), CancellationToken::new())
            .await;
        assert_eq!(
            result.err().expect("start fails"),
            ProviderError::Network {
                source: crate::error::TransportClass::Connect
            }
        );
    }

    /// 直驱 [`AdapterStream`] 收集全部增量：不建 socket，供字节切割重放测试使用
    /// （每个切割位一个连接会在 Windows 上耗尽临时端口，TIME_WAIT 累积导致
    /// connect 间歇性失败；本测试的被测对象是字节切割重放而非 HTTP 传输）。
    fn replay_stream<S>(body: S) -> crate::provider::ProviderStream
    where
        S: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin + Send + 'static,
    {
        // 与 start() 相同的 unfold 驱动：字节切割重放测试不经 HTTP 层
        // （每个切割位一个连接会在 Windows 上耗尽临时端口，TIME_WAIT 累积导致
        // connect 间歇性失败；本测试的被测对象是字节切割重放而非 HTTP 传输）。
        futures::stream::unfold(AdapterStream::new(body), |mut state| async move {
            state.step().await.map(|item| (item, state))
        })
        .boxed()
    }

    #[tokio::test]
    async fn byte_split_streaming_replays_identically() {
        let body = format!(
            "{}{}data: [DONE]\n\n",
            sse_text_frame("风渡🌉"),
            sse_finish_frame("stop"),
        );
        let chunks = |cut: usize| {
            vec![
                Ok(bytes::Bytes::from(body.as_bytes()[..cut].to_vec())),
                Ok(bytes::Bytes::from(body.as_bytes()[cut..].to_vec())),
            ]
        };
        let whole =
            format_deltas(&collect(replay_stream(futures::stream::iter(chunks(body.len())))).await);
        for cut in 1..body.len() {
            let deltas = collect(replay_stream(futures::stream::iter(chunks(cut)))).await;
            assert_eq!(format_deltas(&deltas), whole, "cut at {cut}");
        }
    }

    #[tokio::test]
    async fn cancellation_stops_the_stream() {
        let segments = vec![
            Segment::Status(200),
            Segment::Header("Content-Type: text/event-stream".into()),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(sse_text_frame("风").into_bytes()),
            Segment::Hold,
        ];
        let server = ScriptedServer::start(segments).await;
        let deepseek = adapter(&server);
        let token = CancellationToken::new();
        let mut stream = deepseek
            .start(completion_request(vec![]), token.clone())
            .await
            .unwrap();
        let first = stream.next().await;
        assert_eq!(first, Some(Ok(ProviderDelta::Text("风".into()))));
        token.cancel();
        let after = stream.next().await;
        assert!(after.is_none());
    }

    #[tokio::test]
    async fn self_signed_tls_is_classified_as_tls() {
        let server = ScriptedServer::start_tls(vec![
            Segment::Status(200),
            Segment::Header("Connection: close".into()),
        ])
        .await;
        let deepseek = DeepSeek::new(
            format!("https://localhost:{}", server.addr.port()),
            SecretString::new("k".into()),
        )
        .unwrap();
        let result = deepseek
            .start(completion_request(vec![]), CancellationToken::new())
            .await;
        assert_eq!(result.err().expect("start fails"), ProviderError::Tls);
    }

    fn format_deltas(deltas: &[Result<ProviderDelta, ProviderError>]) -> String {
        deltas
            .iter()
            .map(|item| match item {
                Ok(delta) => format!("{delta:?}"),
                Err(error) => format!("{error:?}"),
            })
            .collect::<Vec<_>>()
            .join("|")
    }
    #[tokio::test]
    async fn missing_usage_fields_are_not_synthesized_as_zero_tokens() {
        for usage in [
            serde_json::json!({}),
            serde_json::json!({"prompt_tokens":0}),
            serde_json::json!({"completion_tokens":0}),
            serde_json::json!({"prompt_tokens":0,"completion_tokens":0}),
        ] {
            let frame = serde_json::json!({"choices":[{"index":0,"text":"正文","finish_reason":"stop"}],"usage":usage});
            let body = format!("data: {frame}\n\ndata: [DONE]\n\n");
            let server = ScriptedServer::start(sse_ok_script(&body)).await;
            let provider = DeepSeek::new(
                format!("http://{}", server.addr),
                SecretString::from("fixture".to_owned()),
            )
            .unwrap();
            let events = collect(
                provider
                    .start(completion_request(vec![]), CancellationToken::new())
                    .await
                    .unwrap(),
            )
            .await;
            let reported = events
                .iter()
                .filter(|event| matches!(event, Ok(ProviderDelta::Usage(_))))
                .count();
            assert_eq!(
                reported,
                usize::from(
                    usage.get("prompt_tokens").is_some()
                        && usage.get("completion_tokens").is_some()
                )
            );
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, Ok(ProviderDelta::Finish(ProviderFinish::Stop))))
            );
        }
    }
}
