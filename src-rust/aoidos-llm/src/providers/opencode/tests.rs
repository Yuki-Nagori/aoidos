use super::*;
use crate::provider::{ChatMessage, ProviderFinish};
use crate::testserver::{ScriptedServer, sse_ok_script};
use futures::StreamExt;
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

fn provider(edition: OpenCodeEdition, server: &ScriptedServer) -> OpenCode {
    crate::install_ring_provider();
    let mut provider = OpenCode::new(
        edition,
        SecretString::from("test-key".to_owned()),
        reqwest::Client::new(),
    );
    provider.base = format!(
        "http://{}{}",
        server.addr,
        match edition {
            OpenCodeEdition::Zen => "/zen/v1",
            OpenCodeEdition::Go => "/zen/go/v1",
        }
    );
    provider
}

fn request(model: &str, session_id: Option<&str>) -> ProviderRequest {
    ProviderRequest {
        model: model.into(),
        session_id: session_id.map(str::to_owned),
        input: ProviderInput::Chat(ChatInput {
            messages: vec![
                ChatMessage {
                    role: ChatRole::System,
                    content: "rules".into(),
                },
                ChatMessage {
                    role: ChatRole::User,
                    content: "hello".into(),
                },
            ],
            assistant_prefix: None,
        }),
        sampling: crate::sampling::Sampling {
            temperature: 0.7,
            max_tokens: 256,
        },
        stops: vec![],
    }
}

#[test]
fn documented_catalog_maps_each_model_to_exactly_one_protocol() {
    assert_eq!(GO_MODELS.len(), 32);
    assert_eq!(ZEN_MODELS.len(), 86);
    for (edition, catalog) in [
        (OpenCodeEdition::Zen, ZEN_MODELS),
        (OpenCodeEdition::Go, GO_MODELS),
    ] {
        for (index, spec) in catalog.iter().enumerate() {
            assert!(spec.context_window > 0, "missing context for {}", spec.id);
            assert!(spec.max_output_tokens > 0, "missing output for {}", spec.id);
            assert_eq!(
                catalog[..index]
                    .iter()
                    .filter(|other| other.id == spec.id)
                    .count(),
                0,
                "duplicate model {} in {edition:?}",
                spec.id
            );
            assert_eq!(
                edition.protocol(spec.id),
                Some(spec.protocol),
                "protocol mismatch for {}",
                spec.id
            );
        }
    }
    assert_eq!(
        OpenCodeEdition::Go.protocol("deepseek-v4.1-flash"),
        Some(Protocol::ChatCompletions)
    );
    assert_eq!(
        OpenCodeEdition::Go.protocol("gpt-6-luna"),
        Some(Protocol::Responses)
    );
    assert_eq!(
        OpenCodeEdition::Go.protocol("claude-haiku-5-5"),
        Some(Protocol::Messages)
    );
    assert_eq!(OpenCodeEdition::Go.protocol("experimental-unlisted"), None);
    assert_eq!(
        OpenCodeEdition::Zen.protocol("deepseek-v4.1-flash"),
        Some(Protocol::ChatCompletions)
    );
    assert_eq!(
        OpenCodeEdition::Zen.protocol("gpt-6-luna"),
        Some(Protocol::Responses)
    );
    assert_eq!(
        OpenCodeEdition::Zen.protocol("gemini-3.8-flash"),
        Some(Protocol::Gemini)
    );
    assert_eq!(
        OpenCodeEdition::Zen.protocol("jev-1.13"),
        None,
        "structured SystemOne is not a text-generation model"
    );
}

#[tokio::test]
async fn chat_protocol_sends_stable_session_and_parses_usage() {
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":8,\"prompt_tokens_details\":{\"cached_tokens\":30}}}\n\n",
        "data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let server = ScriptedServer::start(sse_ok_script(body)).await;
    let adapter = provider(OpenCodeEdition::Go, &server);
    let mut stream = adapter
        .start(
            request("deepseek-v4.1-flash", Some("run-abc")),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut text = String::new();
    let mut saw_usage = false;
    let mut finished = false;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ProviderDelta::Text(part) => text.push_str(&part),
            ProviderDelta::Usage(usage) => {
                saw_usage = usage.prompt_tokens == 120 && usage.cached_prompt_tokens == Some(30);
            }
            ProviderDelta::Finish(ProviderFinish::Stop) => finished = true,
            _ => {}
        }
    }
    assert_eq!(text, "Hello");
    assert!(saw_usage && finished);
    let recorded = server.requests();
    assert_eq!(recorded[0].path, "/zen/go/v1/chat/completions");
    assert_eq!(
        recorded[0].header("x-opencode-session").as_deref(),
        Some("run-abc")
    );
    assert_eq!(
        recorded[0].header("user-agent").as_deref(),
        Some("Aoidos/0.1")
    );
    assert_eq!(
        recorded[0].header("authorization").as_deref(),
        Some("Bearer test-key")
    );
    let body: Value =
        aoidos_json::decode(std::str::from_utf8(&recorded[0].body).unwrap(), 4096).unwrap();
    assert_eq!(body["model"], "deepseek-v4.1-flash");
    server.shutdown().await;
}

#[tokio::test]
async fn responses_api_maps_text_usage_and_incomplete_reason() {
    let body = concat!(
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}\n\n",
        "event: response.incomplete\ndata: {\"type\":\"response.incomplete\",\"response\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":4},\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n"
    );
    let server = ScriptedServer::start(sse_ok_script(body)).await;
    let adapter = provider(OpenCodeEdition::Go, &server);
    let mut stream = adapter
        .start(request("gpt-6-luna", None), CancellationToken::new())
        .await
        .unwrap();
    let mut text = String::new();
    let mut finish = None;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ProviderDelta::Text(part) => text.push_str(&part),
            ProviderDelta::Finish(reason) => finish = Some(reason),
            _ => {}
        }
    }
    assert_eq!(text, "Hi");
    assert_eq!(finish, Some(ProviderFinish::Length));
    assert_eq!(server.requests()[0].path, "/zen/go/v1/responses");
    server.shutdown().await;
}

#[tokio::test]
async fn messages_api_translates_system_and_cached_usage() {
    let body = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":5,\"cache_creation_input_tokens\":2}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":8}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    );
    let server = ScriptedServer::start(sse_ok_script(body)).await;
    let adapter = provider(OpenCodeEdition::Go, &server);
    let mut stream = adapter
        .start(request("claude-haiku-5-5", None), CancellationToken::new())
        .await
        .unwrap();
    let mut usage = Usage::default();
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ProviderDelta::Text(part) => text.push_str(&part),
            ProviderDelta::Usage(part) => usage.merge(part),
            _ => {}
        }
    }
    assert_eq!(text, "Hello");
    assert_eq!(usage.prompt_tokens, 17);
    assert_eq!(usage.completion_tokens, 8);
    assert_eq!(usage.cached_prompt_tokens, Some(5));
    assert_eq!(usage.cache_creation_tokens, Some(2));
    assert_eq!(server.requests()[0].path, "/zen/go/v1/messages");
    assert_eq!(
        server.requests()[0].header("anthropic-version").as_deref(),
        Some("2023-06-01")
    );
    let body: Value = aoidos_json::decode(
        std::str::from_utf8(&server.requests()[0].body).unwrap(),
        4096,
    )
    .unwrap();
    assert_eq!(body["system"], "rules");
    assert_eq!(body["messages"][0]["role"], "user");
    server.shutdown().await;
}

#[tokio::test]
async fn missing_finish_at_eof_is_interrupted() {
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n";
    let server = ScriptedServer::start(sse_ok_script(body)).await;
    let adapter = provider(OpenCodeEdition::Go, &server);
    let deltas = adapter
        .start(
            request("deepseek-v4.1-flash", None),
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        deltas.first(),
        Some(&Ok(ProviderDelta::Text("partial".into())))
    );
    assert_eq!(deltas.last(), Some(&Err(ProviderError::Interrupted)));
    server.shutdown().await;
}

#[tokio::test]
async fn oversized_sse_payload_is_rejected() {
    let payload = "x".repeat(crate::sse::MAX_EVENT_BYTES + 1);
    let body = format!("data: {payload}\n\n");
    let server = ScriptedServer::start(sse_ok_script(&body)).await;
    let adapter = provider(OpenCodeEdition::Go, &server);
    let deltas = adapter
        .start(
            request("deepseek-v4.1-flash", None),
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        deltas.last(),
        Some(&Err(ProviderError::BadResponse {
            reason: "sse",
            status: None,
        }))
    );
    server.shutdown().await;
}

#[tokio::test]
async fn cancellation_stops_a_stream_while_the_server_is_holding_it_open() {
    let server = ScriptedServer::start(vec![
        crate::testserver::Segment::Status(200),
        crate::testserver::Segment::Header("Content-Type: text/event-stream".into()),
        crate::testserver::Segment::Header("Connection: close".into()),
        crate::testserver::Segment::BodyChunk(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n".to_vec(),
        ),
        crate::testserver::Segment::Hold,
    ])
    .await;
    let adapter = provider(OpenCodeEdition::Go, &server);
    let cancellation = CancellationToken::new();
    let mut stream = adapter
        .start(request("deepseek-v4.1-flash", None), cancellation.clone())
        .await
        .unwrap();
    assert_eq!(
        stream.next().await,
        Some(Ok(ProviderDelta::Text("hello".into())))
    );
    cancellation.cancel();
    assert!(stream.next().await.is_none());
    server.shutdown().await;
}

#[test]
fn messages_usage_keeps_omitted_cache_counts_unknown() {
    let mut stream = OpenCodeStream::new(
        futures::stream::empty::<reqwest::Result<bytes::Bytes>>(),
        Protocol::Messages,
    );
    stream
        .ingest_messages(&serde_json::json!({
            "type": "message_start",
            "message": { "usage": { "input_tokens": 10 } },
        }))
        .unwrap();
    let usage = stream.message_usage.unwrap();
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.cached_prompt_tokens, None);
    assert_eq!(usage.cache_creation_tokens, None);
}

#[tokio::test]
async fn zen_gemini_api_uses_google_stream_shape_and_usage() {
    let body = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Gemini\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":12,\"candidatesTokenCount\":4,\"thoughtsTokenCount\":20,\"totalTokenCount\":36,\"cachedContentTokenCount\":3}}\n\n";
    let server = ScriptedServer::start(sse_ok_script(body)).await;
    let adapter = provider(OpenCodeEdition::Zen, &server);
    let mut stream = adapter
        .start(request("gemini-3.8-flash", None), CancellationToken::new())
        .await
        .unwrap();
    let mut text = String::new();
    let mut saw_usage = false;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ProviderDelta::Text(part) => text.push_str(&part),
            ProviderDelta::Usage(usage) => {
                saw_usage = usage.prompt_tokens == 12
                    && usage.completion_tokens == 24
                    && usage.cached_prompt_tokens == Some(3)
                    && usage.reasoning_tokens == Some(20);
            }
            _ => {}
        }
    }
    assert_eq!(text, "Gemini");
    assert!(saw_usage);
    let recorded = server.requests();
    assert_eq!(
        recorded[0].path,
        "/zen/v1/models/gemini-3.8-flash:streamGenerateContent?alt=sse"
    );
    assert_eq!(
        recorded[0].header("x-goog-api-key").as_deref(),
        Some("test-key")
    );
    let body: Value =
        aoidos_json::decode(std::str::from_utf8(&recorded[0].body).unwrap(), 4096).unwrap();
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "rules");
    server.shutdown().await;
}

#[test]
fn gemini_reasoning_usage_rejects_output_token_overflow() {
    assert!(matches!(
        gemini_usage(&serde_json::json!({
            "promptTokenCount": 1,
            "candidatesTokenCount": u64::MAX,
            "thoughtsTokenCount": 1,
        })),
        Err(ProviderError::BadResponse {
            reason: "invalid-usage",
            ..
        })
    ));
}

#[test]
fn model_capabilities_are_bounded_and_route_specific() {
    crate::install_ring_provider();
    let provider = OpenCode::new(
        OpenCodeEdition::Go,
        SecretString::from("key".to_owned()),
        reqwest::Client::new(),
    );
    let chat = provider.capabilities("deepseek-v4.1-flash", RequestMode::Chat);
    let responses = provider.capabilities("gpt-6-luna", RequestMode::Chat);
    let messages = provider.capabilities("claude-haiku-5-5", RequestMode::Chat);
    assert!(chat.chat && responses.chat && messages.chat);
    assert_eq!(chat.context_limit, Some(1_000_000));
    assert_eq!(chat.max_output_tokens, 384_000);
    assert_eq!(responses.context_limit, Some(1_050_000));
    assert_eq!(responses.max_output_tokens, 128_000);
    assert_eq!(messages.context_limit, Some(1_000_000));
    assert_eq!(messages.max_output_tokens, 128_000);
    assert_eq!(responses.stop_limit, 0);
    assert_eq!(messages.temperature_max, 1.0);
    assert!(
        !provider
            .capabilities("experimental-unlisted", RequestMode::Chat)
            .chat
    );
}

#[test]
fn request_bodies_preserve_assistant_roles_and_protocol_stop_fields() {
    for protocol in [
        Protocol::ChatCompletions,
        Protocol::Responses,
        Protocol::Messages,
        Protocol::Gemini,
    ] {
        for with_system in [false, true] {
            let mut req = request("fixture", None);
            let ProviderInput::Chat(ref mut input) = req.input else {
                unreachable!()
            };
            if !with_system {
                input.messages.clear();
            }
            input.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: "previous".into(),
            });
            let ProviderInput::Chat(ref input) = req.input else {
                unreachable!()
            };
            let body: Value =
                aoidos_json::decode_bytes(&request_body(protocol, &req, input).unwrap(), 4096)
                    .unwrap();
            match protocol {
                Protocol::Messages => {
                    assert_eq!(body.get("system").is_some(), with_system);
                    assert_eq!(
                        body["messages"].as_array().unwrap().last().unwrap()["role"],
                        "assistant"
                    );
                }
                Protocol::Gemini => {
                    assert_eq!(body.get("systemInstruction").is_some(), with_system);
                    assert_eq!(
                        body["contents"].as_array().unwrap().last().unwrap()["role"],
                        "model"
                    );
                }
                _ => {}
            }
            req.stops = vec!["END".into()];
            let ProviderInput::Chat(ref input) = req.input else {
                unreachable!()
            };
            let result = request_body(protocol, &req, input);
            if protocol == Protocol::Responses {
                assert_eq!(result, Err(unsupported("stop-not-supported")));
            } else {
                let body: Value = aoidos_json::decode_bytes(&result.unwrap(), 4096).unwrap();
                let stops = match protocol {
                    Protocol::ChatCompletions => &body["stop"],
                    Protocol::Messages => &body["stop_sequences"],
                    _ => &body["generationConfig"]["stopSequences"],
                };
                assert_eq!(stops, &serde_json::json!(["END"]));
            }
        }
    }
}

#[tokio::test]
async fn start_rejects_unsupported_requests_invalid_session_and_http_failure() {
    use crate::testserver::Segment;
    let server =
        ScriptedServer::start(vec![Segment::Status(401), Segment::BodyChunk(vec![])]).await;
    let adapter = provider(OpenCodeEdition::Go, &server);
    let mut req = request("gpt-6-luna", None);
    req.input = ProviderInput::Completion(crate::provider::CompletionInput {
        prompt: "prefix".into(),
    });
    assert_eq!(
        adapter.start(req, CancellationToken::new()).await.err(),
        Some(unsupported("unsupported-mode"))
    );
    assert_eq!(
        adapter
            .start(request("unknown", None), CancellationToken::new())
            .await
            .err(),
        Some(unsupported("unsupported-model"))
    );
    let mut req = request("gpt-6-luna", None);
    let ProviderInput::Chat(ref mut input) = req.input else {
        unreachable!()
    };
    input.assistant_prefix = Some("prefix".into());
    assert_eq!(
        adapter.start(req, CancellationToken::new()).await.err(),
        Some(unsupported("unsupported-capability"))
    );
    assert_eq!(
        adapter
            .start(
                request("gpt-6-luna", Some("invalid\nsession")),
                CancellationToken::new()
            )
            .await
            .err(),
        Some(unsupported("invalid-session"))
    );
    assert_eq!(server.request_count(), 0);
    assert_eq!(
        adapter
            .start(request("gpt-6-luna", None), CancellationToken::new())
            .await
            .err(),
        Some(ProviderError::from_status(401))
    );
    server.shutdown().await;
}

#[tokio::test]
async fn stream_protocol_boundaries_and_transport_failures_are_explicit() {
    // 全部夹具共用同一具体流类型，覆盖 LLVM 按函数实例统计的成功及失败分支。
    fn state(
        protocol: Protocol,
        chunks: Vec<reqwest::Result<bytes::Bytes>>,
    ) -> OpenCodeStream<futures::stream::Iter<std::vec::IntoIter<reqwest::Result<bytes::Bytes>>>>
    {
        OpenCodeStream::new(futures::stream::iter(chunks), protocol)
    }
    let mut chat = state(Protocol::ChatCompletions, vec![]);
    chat.ingest(r#"{"choices":[{"index":1,"delta":{"content":"ignore"}},{"index":0,"delta":{"content":"text","reasoning_content":"reason"},"finish_reason":"length"}],"usage":{"prompt_tokens":4,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":1},"completion_tokens_details":{"reasoning_tokens":1}}}"#).unwrap();
    assert_eq!(
        chat.queue.pop_front(),
        Some(ProviderDelta::Usage(Usage {
            prompt_tokens: 4,
            completion_tokens: 2,
            cached_prompt_tokens: Some(1),
            cache_creation_tokens: None,
            reasoning_tokens: Some(1)
        }))
    );
    assert_eq!(
        chat.queue.pop_front(),
        Some(ProviderDelta::Text("text".into()))
    );
    assert_eq!(
        chat.queue.pop_front(),
        Some(ProviderDelta::Reasoning("reason".into()))
    );
    assert_eq!(chat.finish, Some(ProviderFinish::Length));
    for reason in ["stop", "stop_sequence", "max_tokens"] {
        chat.ingest(&serde_json::json!({"choices":[{"finish_reason":reason}]}).to_string())
            .unwrap();
    }
    assert_eq!(
        chat.ingest(r#"{"choices":[{"finish_reason":"tool_calls"}]}"#),
        Err(unsupported("finish-reason"))
    );
    chat.ingest(r#"{"choices":[],"usage":{}}"#).unwrap();

    let mut responses = state(Protocol::Responses, vec![]);
    responses
        .ingest(r#"{"type":"response.output_text.delta","delta":"response"}"#)
        .unwrap();
    assert_eq!(
        responses.queue.pop_front(),
        Some(ProviderDelta::Text("response".into()))
    );
    responses.ingest(r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#).unwrap();
    assert_eq!(responses.finish, Some(ProviderFinish::Length));

    for frame in [
        serde_json::json!({}),
        serde_json::json!({"type":"response.output_text.delta","delta":""}),
        serde_json::json!({"type":"response.completed","response":{"usage":{"input_tokens":2,"output_tokens":1,"input_tokens_details":{"cached_tokens":1},"output_tokens_details":{"reasoning_tokens":1}}}}),
    ] {
        responses.ingest(&frame.to_string()).unwrap();
    }
    assert_eq!(responses.finish, Some(ProviderFinish::Stop));
    assert!(matches!(
        responses.queue.pop_front(),
        Some(ProviderDelta::Usage(Usage {
            cached_prompt_tokens: Some(1),
            reasoning_tokens: Some(1),
            ..
        }))
    ));
    responses.push_responses_usage(&serde_json::json!({}));
    for (frame, reason) in [
        (
            serde_json::json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"content_filter"}}}),
            "incomplete-response",
        ),
        (
            serde_json::json!({"type":"response.failed"}),
            "response-failed",
        ),
        (serde_json::json!({"type":"error"}), "response-failed"),
    ] {
        assert_eq!(
            responses.ingest(&frame.to_string()),
            Err(unsupported(reason))
        );
    }

    let mut messages = state(Protocol::Messages, vec![]);
    messages.ingest(r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":5,"cache_creation_input_tokens":2}}}"#).unwrap();
    assert_eq!(messages.message_usage.as_ref().unwrap().prompt_tokens, 17);
    messages
        .ingest(r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"message"}}"#)
        .unwrap();
    assert_eq!(
        messages.queue.pop_front(),
        Some(ProviderDelta::Text("message".into()))
    );
    messages
        .ingest(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#)
        .unwrap();
    messages.message_usage = None;

    for (read, write) in [(1, 0), (0, 1)] {
        assert_eq!(messages.ingest(&serde_json::json!({"type":"message_start","message":{"usage":{"input_tokens":u64::MAX,"cache_read_input_tokens":read,"cache_creation_input_tokens":write}}}).to_string()), Err(unsupported("invalid-usage")));
    }
    for frame in [
        serde_json::json!({}),
        serde_json::json!({"type":"message_start","message":{"usage":{}}}),
        serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":""}}),
        serde_json::json!({"type":"message_delta","usage":{"output_tokens":5},"delta":{}}),
        serde_json::json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}}),
        serde_json::json!({"type":"message_delta","delta":{"stop_reason":"stop_sequence"}}),
        serde_json::json!({"type":"message_stop"}),
        serde_json::json!({"type":"message_stop"}),
    ] {
        messages.ingest(&frame.to_string()).unwrap();
    }
    assert_eq!(messages.finish, Some(ProviderFinish::Stop));
    assert_eq!(
        messages.queue.pop_front(),
        Some(ProviderDelta::Usage(Usage {
            completion_tokens: 5,
            ..Usage::default()
        }))
    );
    assert_eq!(
        messages.ingest(r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#),
        Err(unsupported("finish-reason"))
    );
    assert_eq!(
        messages.ingest(r#"{"type":"error"}"#),
        Err(unsupported("message-failed"))
    );

    let mut gemini = state(Protocol::Gemini, vec![]);
    gemini
        .ingest(r#"{"candidates":[{"finishReason":"STOP"}]}"#)
        .unwrap();
    assert_eq!(gemini.finish, Some(ProviderFinish::Stop));
    gemini.finish = None;

    for frame in [
        serde_json::json!({}),
        serde_json::json!({"candidates":[]}),
        serde_json::json!({"candidates":[{"content":{"parts":[{"text":""},{"text":"hello"},{}]}}]}),
        serde_json::json!({"candidates":[{"finishReason":"MAX_TOKENS"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":1}}),
    ] {
        gemini.ingest(&frame.to_string()).unwrap();
    }
    assert_eq!(
        gemini.queue.pop_front(),
        Some(ProviderDelta::Text("hello".into()))
    );
    assert!(matches!(
        gemini.queue.pop_front(),
        Some(ProviderDelta::Usage(Usage {
            prompt_tokens: 2,
            completion_tokens: 1,
            ..
        }))
    ));
    assert_eq!(gemini.finish, Some(ProviderFinish::Length));
    assert_eq!(
        gemini.ingest(r#"{"candidates":[{"finishReason":"SAFETY"}]}"#),
        Err(unsupported("finish-reason"))
    );

    for (bytes, reason) in [
        (vec![0xff], "utf8"),
        (vec![0xe4], "utf8"),
        (b"data: invalid\n\n".to_vec(), "json"),
        (b"data: invalid".to_vec(), "json"),
        (b"data: [DONE]".to_vec(), "missing-finish"),
    ] {
        let mut stream = state(Protocol::ChatCompletions, vec![Ok(bytes.into())]);
        let result = stream.step().await.unwrap();
        assert!(
            matches!(result, Err(ProviderError::BadResponse { reason: actual, .. }) if actual == reason),
            "{result:?}"
        );
        assert!(stream.step().await.is_none());
    }
    let mut stream = state(
        Protocol::ChatCompletions,
        vec![Ok(bytes::Bytes::from(format!(
            "data: {}\n\n",
            "x".repeat(crate::sse::MAX_EVENT_BYTES + 1)
        )))],
    );
    assert_eq!(stream.step().await, Some(Err(unsupported("sse"))));
    let error = reqwest::Client::new()
        .get("invalid-url")
        .build()
        .unwrap_err();
    let expected = ProviderError::from_reqwest(&error, crate::error::RequestPhase::Body);
    let mut stream = state(Protocol::ChatCompletions, vec![Err(error)]);
    assert_eq!(stream.step().await, Some(Err(expected)));
    assert!(stream.step().await.is_none());
    let mut stream = state(
        Protocol::ChatCompletions,
        vec![Ok(bytes::Bytes::from_static(
            b"data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]",
        ))],
    );
    assert_eq!(
        stream.step().await,
        Some(Ok(ProviderDelta::Finish(ProviderFinish::Stop)))
    );
    assert!(stream.step().await.is_none());
    let mut stream = state(
        Protocol::ChatCompletions,
        vec![Ok(bytes::Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        ))],
    );
    assert_eq!(
        stream.step().await,
        Some(Ok(ProviderDelta::Text("partial".into())))
    );
    assert_eq!(stream.step().await, Some(Err(ProviderError::Interrupted)));
    assert!(stream.step().await.is_none());
}
