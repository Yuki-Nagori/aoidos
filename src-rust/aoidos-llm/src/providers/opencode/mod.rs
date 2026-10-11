//! OpenCode Zen / Go 的多协议文本生成适配器。
//!
//! 模型目录按已核验的 endpoint 与协议版本控制；公开 models endpoint 中缺少协议或价格信息的
//! preview / compatibility ID 不会因此自动开放。

use super::models::{GO_MODELS, ModelSpec, Protocol, ZEN_MODELS};
use crate::decode::IncrementalUtf8;
use crate::error::ProviderError;
use crate::provider::{
    ChatInput, ChatRole, Credential, Provider, ProviderCapabilities, ProviderDelta, ProviderFinish,
    ProviderInput, ProviderRequest, RequestMode, StartFuture, Usage,
};
use crate::sse::{IncrementalSse, SseError};
use futures::StreamExt;
use secrecy::ExposeSecret;
use serde_json::Value;
use std::collections::VecDeque;

const CHAT_STOP_LIMIT: usize = 4;
const MESSAGES_STOP_LIMIT: usize = 4;

/// OpenCode 按独立账户与费用身份接入的产品。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeEdition {
    Zen,
    Go,
}

impl OpenCodeEdition {
    fn base(self) -> &'static str {
        match self {
            Self::Zen => "https://opencode.ai/zen/v1",
            Self::Go => "https://opencode.ai/zen/go/v1",
        }
    }

    fn protocol(self, model: &str) -> Option<Protocol> {
        self.model(model).map(|spec| spec.protocol)
    }

    fn model(self, model: &str) -> Option<&'static ModelSpec> {
        let catalog = match self {
            Self::Zen => ZEN_MODELS,
            Self::Go => GO_MODELS,
        };
        catalog.iter().find(|spec| spec.id == model)
    }
}

impl Protocol {
    fn endpoint(self, model: &str) -> String {
        match self {
            Self::ChatCompletions => "/chat/completions".into(),
            Self::Responses => "/responses".into(),
            Self::Messages => "/messages".into(),
            Self::Gemini => format!("/models/{model}:streamGenerateContent?alt=sse"),
        }
    }
}

/// OpenCode Zen 与 Go 的文本生成 API。
pub struct OpenCode {
    edition: OpenCodeEdition,
    base: String,
    key: Credential,
    client: reqwest::Client,
}

impl OpenCode {
    /// 使用装配层配置好的 HTTP 客户端构建适配器。
    pub fn new(edition: OpenCodeEdition, key: Credential, client: reqwest::Client) -> Self {
        Self {
            edition,
            base: edition.base().to_owned(),
            key,
            client,
        }
    }
}

impl Provider for OpenCode {
    fn capabilities(&self, model: &str, mode: RequestMode) -> ProviderCapabilities {
        let spec = self.edition.model(model);
        let protocol = spec.map(|spec| spec.protocol);
        let known_spec = spec.filter(|_| mode == RequestMode::Chat);
        let known = known_spec.is_some();
        let stop_limit = match protocol {
            Some(Protocol::ChatCompletions) => CHAT_STOP_LIMIT,
            Some(Protocol::Messages | Protocol::Gemini) => MESSAGES_STOP_LIMIT,
            Some(Protocol::Responses) | None => 0,
        };
        let temperature_max = if protocol == Some(Protocol::Messages) {
            1.0
        } else {
            2.0
        };
        ProviderCapabilities {
            model: model.to_owned(),
            // 使用 edition 专属目录中的上下文窗口和最大输出上限。
            context_limit: known_spec.map(|spec| spec.context_window),
            completion: false,
            chat: known,
            prefix: false,
            thinking: false,
            stop_limit: if known { stop_limit } else { 0 },
            max_output_tokens: known_spec.map_or(0, |spec| spec.max_output_tokens),
            temperature_effective: known,
            temperature_min: 0.0,
            temperature_max,
        }
    }

    fn start(
        &self,
        request: ProviderRequest,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> StartFuture<'_> {
        Box::pin(async move {
            let ProviderInput::Chat(ref input) = request.input else {
                return Err(unsupported("unsupported-mode"));
            };
            let Some(protocol) = self.edition.protocol(&request.model) else {
                return Err(unsupported("unsupported-model"));
            };
            if input.assistant_prefix.is_some() {
                return Err(unsupported("unsupported-capability"));
            }
            let body = request_body(protocol, &request, input)?;
            let mut builder = self
                .client
                .post(format!(
                    "{}{}",
                    self.base,
                    protocol.endpoint(&request.model)
                ))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::USER_AGENT, "Aoidos/0.1")
                .body(body);
            builder = if protocol == Protocol::Gemini {
                builder.header("x-goog-api-key", self.key.expose_secret())
            } else {
                builder.bearer_auth(self.key.expose_secret())
            };
            if let Some(session_id) = request.session_id.as_deref() {
                let value = reqwest::header::HeaderValue::from_str(session_id)
                    .map_err(|_| unsupported("invalid-session"))?;
                builder = builder.header("x-opencode-session", value);
            }
            if protocol == Protocol::Messages {
                builder = builder.header("anthropic-version", "2023-06-01");
            }
            let response = builder.send().await.map_err(super::deepseek::err_send)?;
            let status = response.status().as_u16();
            if status != 200 {
                return Err(ProviderError::from_status(status));
            }
            let stream = OpenCodeStream::new(response.bytes_stream(), protocol);
            let deltas = futures::stream::unfold(stream, |mut state| async move {
                state.step().await.map(|item| (item, state))
            });
            Ok(deltas.take_until(cancellation.cancelled_owned()).boxed())
        })
    }
}

fn request_body(
    protocol: Protocol,
    request: &ProviderRequest,
    input: &ChatInput,
) -> Result<Vec<u8>, ProviderError> {
    let messages: Vec<_> = input
        .messages
        .iter()
        .map(|message| {
            serde_json::json!({
                "role": role_name(message.role),
                "content": message.content,
            })
        })
        .collect();
    let mut body = match protocol {
        Protocol::ChatCompletions => serde_json::json!({
            "model": request.model,
            "stream": true,
            "stream_options": { "include_usage": true },
            "max_tokens": request.sampling.max_tokens,
            "temperature": request.sampling.temperature,
            "messages": messages,
        }),
        Protocol::Responses => serde_json::json!({
            "model": request.model,
            "stream": true,
            "max_output_tokens": request.sampling.max_tokens,
            "temperature": request.sampling.temperature,
            "input": messages,
        }),
        Protocol::Messages => {
            let system = input
                .messages
                .iter()
                .filter(|message| message.role == ChatRole::System)
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>();
            let messages: Vec<_> = input
                .messages
                .iter()
                .filter(|message| message.role != ChatRole::System)
                .map(|message| {
                    serde_json::json!({
                        "role": if message.role == ChatRole::Assistant { "assistant" } else { "user" },
                        "content": message.content,
                    })
                })
                .collect();
            let mut body = serde_json::json!({
                "model": request.model,
                "stream": true,
                "max_tokens": request.sampling.max_tokens,
                "temperature": request.sampling.temperature,
                "messages": messages,
            });
            if !system.is_empty() {
                body["system"] = serde_json::json!(system.join("\n\n"));
            }
            body
        }
        Protocol::Gemini => {
            let system = input
                .messages
                .iter()
                .filter(|message| message.role == ChatRole::System)
                .map(|message| serde_json::json!({ "text": message.content }))
                .collect::<Vec<_>>();
            let contents: Vec<_> = input
                .messages
                .iter()
                .filter(|message| message.role != ChatRole::System)
                .map(|message| {
                    serde_json::json!({
                        "role": if message.role == ChatRole::Assistant { "model" } else { "user" },
                        "parts": [{ "text": message.content }],
                    })
                })
                .collect();
            let mut body = serde_json::json!({
                "contents": contents,
                "generationConfig": {
                    "maxOutputTokens": request.sampling.max_tokens,
                    "temperature": request.sampling.temperature,
                },
            });
            if !system.is_empty() {
                body["systemInstruction"] = serde_json::json!({ "parts": system });
            }
            body
        }
    };
    if !request.stops.is_empty() {
        match protocol {
            Protocol::ChatCompletions => body["stop"] = serde_json::json!(request.stops),
            Protocol::Messages => body["stop_sequences"] = serde_json::json!(request.stops),
            Protocol::Gemini => {
                body["generationConfig"]["stopSequences"] = serde_json::json!(request.stops);
            }
            Protocol::Responses => {
                return Err(unsupported("stop-not-supported"));
            }
        }
    }
    aoidos_json::to_vec(&body).map_err(super::deepseek::err_chunk_json)
}

fn role_name(role: ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
    }
}

fn unsupported(reason: &'static str) -> ProviderError {
    ProviderError::BadResponse {
        reason,
        status: None,
    }
}

struct OpenCodeStream<B> {
    body: B,
    decode: IncrementalUtf8,
    sse: IncrementalSse,
    protocol: Protocol,
    queue: VecDeque<ProviderDelta>,
    message_usage: Option<Usage>,
    finish: Option<ProviderFinish>,
    done_marker: bool,
    finished: bool,
}

impl<B> OpenCodeStream<B>
where
    B: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin + Send + 'static,
{
    fn new(body: B, protocol: Protocol) -> Self {
        Self {
            body,
            decode: IncrementalUtf8::new(),
            sse: IncrementalSse::new(),
            protocol,
            queue: VecDeque::new(),
            message_usage: None,
            finish: None,
            done_marker: false,
            finished: false,
        }
    }

    fn ingest(&mut self, data: &str) -> Result<(), ProviderError> {
        if data == "[DONE]" {
            self.done_marker = true;
            return Ok(());
        }
        let value: Value = aoidos_json::decode(data, crate::sse::MAX_EVENT_BYTES)
            .map_err(super::deepseek::err_chunk_json)?;
        match self.protocol {
            Protocol::ChatCompletions => self.ingest_chat(&value),
            Protocol::Responses => self.ingest_responses(&value),
            Protocol::Messages => self.ingest_messages(&value),
            Protocol::Gemini => self.ingest_gemini(&value),
        }
    }

    fn ingest_chat(&mut self, value: &Value) -> Result<(), ProviderError> {
        if let Some(usage) = value.get("usage")
            && let (Some(prompt_tokens), Some(completion_tokens)) = (
                usage.get("prompt_tokens").and_then(Value::as_u64),
                usage.get("completion_tokens").and_then(Value::as_u64),
            )
        {
            let cached = usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64);
            self.queue.push_back(ProviderDelta::Usage(Usage {
                prompt_tokens,
                completion_tokens,
                cached_prompt_tokens: cached,
                cache_creation_tokens: None,
                reasoning_tokens: usage
                    .get("completion_tokens_details")
                    .and_then(|details| details.get("reasoning_tokens"))
                    .and_then(Value::as_u64),
            }));
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| {
                choices
                    .iter()
                    .find(|choice| choice["index"].as_u64().unwrap_or(0) == 0)
            })
        else {
            return Ok(());
        };
        if let Some(text) = choice["delta"]["content"]
            .as_str()
            .filter(|text| !text.is_empty())
        {
            self.queue.push_back(ProviderDelta::Text(text.to_owned()));
        }
        if let Some(reasoning) = choice["delta"]["reasoning_content"]
            .as_str()
            .filter(|text| !text.is_empty())
        {
            self.queue
                .push_back(ProviderDelta::Reasoning(reasoning.to_owned()));
        }
        self.finish = match choice["finish_reason"].as_str() {
            None => self.finish,
            Some("stop" | "stop_sequence") => Some(ProviderFinish::Stop),
            Some("length" | "max_tokens") => Some(ProviderFinish::Length),
            Some(_) => return Err(unsupported("finish-reason")),
        };
        Ok(())
    }

    fn ingest_responses(&mut self, value: &Value) -> Result<(), ProviderError> {
        match value["type"].as_str() {
            Some("response.output_text.delta") => {
                if let Some(text) = value["delta"].as_str().filter(|text| !text.is_empty()) {
                    self.queue.push_back(ProviderDelta::Text(text.to_owned()));
                }
            }
            Some("response.completed") => {
                let response = &value["response"];
                self.push_responses_usage(&response["usage"]);
                self.finish = Some(ProviderFinish::Stop);
                self.done_marker = true;
            }
            Some("response.incomplete") => {
                let response = &value["response"];
                self.push_responses_usage(&response["usage"]);
                if response["incomplete_details"]["reason"].as_str() != Some("max_output_tokens") {
                    return Err(unsupported("incomplete-response"));
                }
                self.finish = Some(ProviderFinish::Length);
                self.done_marker = true;
            }
            Some("response.failed" | "error") => return Err(unsupported("response-failed")),
            _ => {}
        }
        Ok(())
    }

    fn push_responses_usage(&mut self, usage: &Value) {
        if let (Some(prompt_tokens), Some(completion_tokens)) = (
            usage["input_tokens"].as_u64(),
            usage["output_tokens"].as_u64(),
        ) {
            let cached = usage["input_tokens_details"]["cached_tokens"].as_u64();
            self.queue.push_back(ProviderDelta::Usage(Usage {
                prompt_tokens,
                completion_tokens,
                cached_prompt_tokens: cached,
                cache_creation_tokens: None,
                reasoning_tokens: usage["output_tokens_details"]["reasoning_tokens"].as_u64(),
            }));
        }
    }

    fn ingest_messages(&mut self, value: &Value) -> Result<(), ProviderError> {
        match value["type"].as_str() {
            Some("message_start") => {
                let usage = &value["message"]["usage"];
                if let Some(input_tokens) = usage["input_tokens"].as_u64() {
                    let cached_read = usage["cache_read_input_tokens"].as_u64();
                    let cached_write = usage["cache_creation_input_tokens"].as_u64();
                    let Some(prompt_tokens) = input_tokens
                        .checked_add(cached_read.unwrap_or(0))
                        .and_then(|total| total.checked_add(cached_write.unwrap_or(0)))
                    else {
                        return Err(unsupported("invalid-usage"));
                    };
                    self.message_usage = Some(Usage {
                        prompt_tokens,
                        cached_prompt_tokens: cached_read,
                        cache_creation_tokens: cached_write,
                        ..Usage::default()
                    });
                }
            }
            Some("content_block_delta") => {
                if value["delta"]["type"].as_str() == Some("text_delta")
                    && let Some(text) = value["delta"]["text"]
                        .as_str()
                        .filter(|text| !text.is_empty())
                {
                    self.queue.push_back(ProviderDelta::Text(text.to_owned()));
                }
            }
            Some("message_delta") => {
                let usage = &value["usage"];
                if let Some(completion_tokens) = usage["output_tokens"].as_u64() {
                    self.message_usage
                        .get_or_insert_with(Usage::default)
                        .completion_tokens = completion_tokens;
                }
                self.finish = match value["delta"]["stop_reason"].as_str() {
                    Some("end_turn" | "stop_sequence") => Some(ProviderFinish::Stop),
                    Some("max_tokens") => Some(ProviderFinish::Length),
                    Some(_) => return Err(unsupported("finish-reason")),
                    None => self.finish,
                };
            }
            Some("message_stop") => {
                if let Some(usage) = self.message_usage.take() {
                    self.queue.push_back(ProviderDelta::Usage(usage));
                }
                self.done_marker = true;
            }
            Some("error") => return Err(unsupported("message-failed")),
            _ => {}
        }
        Ok(())
    }

    fn ingest_gemini(&mut self, value: &Value) -> Result<(), ProviderError> {
        if let Some(candidates) = value["candidates"].as_array() {
            let Some(candidate) = candidates.first() else {
                return Ok(());
            };
            if let Some(parts) = candidate["content"]["parts"].as_array() {
                for part in parts {
                    if let Some(text) = part["text"].as_str().filter(|text| !text.is_empty()) {
                        self.queue.push_back(ProviderDelta::Text(text.to_owned()));
                    }
                }
            }
            self.finish = match candidate["finishReason"].as_str() {
                None => self.finish,
                Some("STOP") => Some(ProviderFinish::Stop),
                Some("MAX_TOKENS") => Some(ProviderFinish::Length),
                Some(_) => return Err(unsupported("finish-reason")),
            };
            if self.finish.is_some() {
                self.done_marker = true;
            }
        }
        if let Some(usage) = gemini_usage(&value["usageMetadata"])? {
            self.queue.push_back(ProviderDelta::Usage(usage));
        }
        Ok(())
    }

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
                return Some(match self.finish {
                    Some(finish) => Ok(ProviderDelta::Finish(finish)),
                    None => Err(unsupported("missing-finish")),
                });
            }
            match self.body.next().await {
                Some(Ok(bytes)) => {
                    let text = match self.decode.feed(&bytes) {
                        Ok(text) => text,
                        Err(_) => return self.fail("utf8"),
                    };
                    let events = match self.sse.feed(&text) {
                        Ok(events) => events,
                        Err(error) => return self.fail_sse(error),
                    };
                    for event in events {
                        if let Err(error) = self.ingest(&event.data) {
                            self.finished = true;
                            return Some(Err(error));
                        }
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
                    if self.decode.finish().is_err() {
                        return self.fail("utf8");
                    }
                    let events = self.sse.finish();
                    for event in events {
                        if let Err(error) = self.ingest(&event.data) {
                            self.finished = true;
                            return Some(Err(error));
                        }
                    }
                    if self.done_marker {
                        continue;
                    }
                    self.finished = true;
                    return Some(Err(ProviderError::Interrupted));
                }
            }
        }
    }

    fn fail(&mut self, reason: &'static str) -> Option<Result<ProviderDelta, ProviderError>> {
        self.finished = true;
        Some(Err(unsupported(reason)))
    }

    fn fail_sse(&mut self, _: SseError) -> Option<Result<ProviderDelta, ProviderError>> {
        self.fail("sse")
    }
}

fn gemini_usage(usage: &Value) -> Result<Option<Usage>, ProviderError> {
    let (Some(prompt_tokens), Some(candidates_tokens)) = (
        usage["promptTokenCount"].as_u64(),
        usage["candidatesTokenCount"].as_u64(),
    ) else {
        return Ok(None);
    };
    let reasoning_tokens = usage["thoughtsTokenCount"].as_u64();
    let Some(completion_tokens) = candidates_tokens.checked_add(reasoning_tokens.unwrap_or(0))
    else {
        return Err(unsupported("invalid-usage"));
    };
    Ok(Some(Usage {
        prompt_tokens,
        completion_tokens,
        cached_prompt_tokens: usage["cachedContentTokenCount"].as_u64(),
        reasoning_tokens,
        ..Usage::default()
    }))
}

#[cfg(test)]
mod tests;
