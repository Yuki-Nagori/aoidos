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
        session_id: None,
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
    assert_eq!(completion.max_output_tokens, FIM_MAX_OUTPUT_TOKENS);
    assert_eq!(completion.context_limit, Some(1_048_576));
    let chat = deepseek.capabilities("deepseek-flash", RequestMode::Chat);
    assert!(chat.chat);
    assert!(chat.prefix);
    assert!(chat.thinking);
    assert_eq!(chat.max_output_tokens, 393_216);
    assert_eq!(chat.context_limit, Some(1_048_576));
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
    assert_eq!(caps.context_limit, None);
    assert!(!caps.temperature_effective);
}

#[tokio::test]
async fn completion_stream_yields_text_usage_finish() {
    let body = format!(
        "{}{}{}data: [DONE]\n\n",
        sse_completion_frame("风吹过。"),
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"prompt_cache_hit_tokens\":0}}\n\n",
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
                completion_tokens: 5,
                cached_prompt_tokens: Some(0),
                cache_creation_tokens: None,
                reasoning_tokens: None,
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
        session_id: None,
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
        session_id: None,
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
        session_id: None,
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
    let deepseek = DeepSeek::new(format!("http://{addr}"), SecretString::new("k".into())).unwrap();
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
        serde_json::json!({"prompt_tokens":0,"completion_tokens":0,"prompt_cache_hit_tokens":0}),
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
                usage.get("prompt_tokens").is_some() && usage.get("completion_tokens").is_some()
            )
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Ok(ProviderDelta::Finish(ProviderFinish::Stop))))
        );
        if usage.get("prompt_cache_hit_tokens").is_none()
            && usage.get("prompt_tokens").is_some()
            && usage.get("completion_tokens").is_some()
        {
            assert!(events.iter().any(|event| matches!(
                event,
                Ok(ProviderDelta::Usage(Usage {
                    cached_prompt_tokens: None,
                    ..
                }))
            )));
        }
    }
}

#[test]
fn balance_amounts_accept_only_bounded_canonical_unsigned_decimals() {
    for amount in [
        "0",
        "1",
        "123",
        "0.0",
        "123.123456789",
        "999999999999999999999999999999999999999999",
    ] {
        assert!(valid_balance_amount(amount), "{amount}");
    }
    for amount in [
        "",
        "00",
        "01",
        "-1",
        "+1",
        " 1",
        "1 ",
        "1e2",
        ".1",
        "1.",
        "1.1234567890",
        "1.2.3",
        "1.a",
        "１２",
        "9999999999999999999999999999999999999999999",
    ] {
        assert!(!valid_balance_amount(amount), "{amount}");
    }
    for (currency, total, granted, topped) in [
        ("EUR", "0", "0", "0"),
        ("CNY", "-1", "0", "0"),
        ("USD", "0", "bad", "0"),
        ("USD", "0", "0", "bad"),
    ] {
        assert!(
            balance_account(BalanceInfo {
                currency: currency.into(),
                total_balance: total.into(),
                granted_balance: granted.into(),
                topped_up_balance: topped.into()
            })
            .is_err()
        );
    }
}

#[tokio::test]
async fn balance_snapshot_validates_accounts_and_sanitizes_all_failures() {
    use crate::testserver::Segment;
    crate::install_ring_provider();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = SecretString::from("fixture".to_owned());
    for (available, currency) in [(true, "CNY"), (false, "USD")] {
        let body = serde_json::json!({"is_available":available,"balance_infos":[{"currency":currency,"total_balance":"12.30","granted_balance":"2.30","topped_up_balance":"10"}]}).to_string();
        let server = ScriptedServer::start(vec![
            Segment::Status(200),
            Segment::Header("Connection: close".into()),
            Segment::BodyChunk(body.into_bytes()),
        ])
        .await;
        let snapshot = balance_snapshot_at(
            &client,
            &key,
            "now".into(),
            &format!("http://{}/user/balance", server.addr),
        )
        .await;
        assert_eq!(snapshot.provider_id, "deepseek");
        assert_eq!(snapshot.queried_at, "now");
        assert_eq!(
            snapshot.status,
            if available {
                BalanceStatus::Available
            } else {
                BalanceStatus::Unavailable
            }
        );
        assert_eq!(
            snapshot.accounts,
            vec![BalanceAccount {
                currency: currency.into(),
                total: "12.30".into(),
                granted: "2.30".into(),
                topped_up: "10".into()
            }]
        );
        let recorded = server.requests();
        assert_eq!(recorded[0].method, "GET");
        assert_eq!(recorded[0].path, "/user/balance");
        assert_eq!(
            recorded[0].header("authorization").as_deref(),
            Some("Bearer fixture")
        );
        server.shutdown().await;
    }
    for script in [
        vec![Segment::Status(401), Segment::BodyChunk(b"private-error".to_vec())],
        vec![Segment::Status(200), Segment::Header("Content-Length: 16385".into()), Segment::BodyChunk(vec![])],
        vec![Segment::Status(200), Segment::BodyChunk(vec![b'x'; 16385])],
        vec![Segment::Status(200), Segment::BodyChunk(b"invalid-json".to_vec())],
        vec![Segment::Status(200), Segment::BodyChunk(br#"{"is_available":true,"balance_infos":[]}"#.to_vec())],
        vec![Segment::Status(200), Segment::BodyChunk(br#"{"is_available":true,"balance_infos":[{"currency":"EUR","total_balance":"1","granted_balance":"0","topped_up_balance":"1"}]}"#.to_vec())],
        vec![Segment::Status(200), Segment::Header("Content-Length: 100".into()), Segment::BodyChunk(b"partial".to_vec()), Segment::Abort],
    ] {
        let server = ScriptedServer::start(script).await;
        let snapshot = balance_snapshot_at(&client, &key, "checked".into(), &format!("http://{}/user/balance", server.addr)).await;
        assert_eq!(snapshot, unknown_balance("checked".into()));
        server.shutdown().await;
    }
    let snapshot = query_balance(
        &client,
        &SecretString::from("invalid\nkey".to_owned()),
        "checked".into(),
    )
    .await;
    assert_eq!(snapshot, unknown_balance("checked".into()));
}

#[tokio::test]
async fn balance_query_timeout_returns_unknown_without_partial_accounts() {
    crate::install_ring_provider();
    let server = ScriptedServer::start(vec![crate::testserver::Segment::Hold]).await;
    let endpoint = format!("http://{}/user/balance", server.addr);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = SecretString::from("fixture".to_owned());
    let task = tokio::spawn(async move {
        balance_snapshot_at(&client, &key, "timed-out".into(), &endpoint).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while server.request_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    assert_eq!(task.await.unwrap(), unknown_balance("timed-out".into()));
    server.shutdown().await;
}
