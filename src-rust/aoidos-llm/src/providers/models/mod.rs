//! 各供应商型号的协议及上游上下文、输出上限目录。
//!
//! OpenCode 限额静态同步自 Models.dev 的 opencode / opencode-go 记录（2026-10-11，SHA-256：
//! `8b78bf51d0a4aaa6a7ddad1ca5e96c92c9fcc9f2ec015dc770cfca8b45ada503`）；
//! Qwen3.7 Max / Plus 使用 Alibaba Cloud Model Studio 官方型号页。
//! OpenCode 路由的运行期限制仍可能低于目录声明。DeepSeek 型号上限取自
//! [官方模型列表](https://api-docs.deepseek.com/api/list-models/)；
//! FIM 输出上限属于端点能力，不与型号的 Chat 输出上限混用。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Protocol {
    ChatCompletions,
    Responses,
    Messages,
    Gemini,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModelSpec {
    pub(super) id: &'static str,
    pub(super) protocol: Protocol,
    /// 上下文窗口总量；投影会为最大输出和协议余量预留空间。
    pub(super) context_window: u32,
    pub(super) max_output_tokens: u32,
}

impl ModelSpec {
    const fn new(
        id: &'static str,
        protocol: Protocol,
        context_window: u32,
        max_output_tokens: u32,
    ) -> Self {
        Self {
            id,
            protocol,
            context_window,
            max_output_tokens,
        }
    }
}

pub(super) const ZEN_MODELS: &[ModelSpec] = &[
    ModelSpec::new("qwen3.8-max", Protocol::ChatCompletions, 262144, 131072),
    ModelSpec::new(
        "deepseek-v4.1-flash",
        Protocol::ChatCompletions,
        1000000,
        384000,
    ),
    ModelSpec::new(
        "deepseek-v4-pro",
        Protocol::ChatCompletions,
        1000000,
        384000,
    ),
    ModelSpec::new(
        "deepseek-v4-flash",
        Protocol::ChatCompletions,
        1000000,
        384000,
    ),
    ModelSpec::new(
        "deepseek-v4-flash-vision-exp",
        Protocol::ChatCompletions,
        1000000,
        384000,
    ),
    ModelSpec::new("minimax-m3", Protocol::ChatCompletions, 512000, 128000),
    ModelSpec::new("minimax-m2.7", Protocol::ChatCompletions, 204800, 131072),
    ModelSpec::new("minimax-m2.5", Protocol::ChatCompletions, 204800, 131072),
    ModelSpec::new("glm-5.3-flash", Protocol::ChatCompletions, 1000000, 131072),
    ModelSpec::new("glm-5.3", Protocol::ChatCompletions, 1000000, 131072),
    ModelSpec::new("glm-5.2", Protocol::ChatCompletions, 1000000, 131072),
    ModelSpec::new("glm-5.1", Protocol::ChatCompletions, 204800, 131072),
    ModelSpec::new("glm-5", Protocol::ChatCompletions, 204800, 131072),
    ModelSpec::new("kimi-k2.5", Protocol::ChatCompletions, 262144, 65536),
    ModelSpec::new("kimi-k2.6", Protocol::ChatCompletions, 262144, 65536),
    ModelSpec::new("kimi-k2.7-code", Protocol::ChatCompletions, 262144, 262144),
    ModelSpec::new("kimi-k3", Protocol::ChatCompletions, 1048576, 131072),
    ModelSpec::new("mistral-large-4", Protocol::ChatCompletions, 524288, 262144),
    ModelSpec::new("big-pickle", Protocol::ChatCompletions, 200000, 32000),
    ModelSpec::new(
        "space-bunny-free",
        Protocol::ChatCompletions,
        1048576,
        524288,
    ),
    ModelSpec::new(
        "longcat-2.5-preview-free",
        Protocol::ChatCompletions,
        1000000,
        131072,
    ),
    ModelSpec::new(
        "step-5-preview-free",
        Protocol::ChatCompletions,
        1000000,
        65536,
    ),
    ModelSpec::new("exo-free", Protocol::ChatCompletions, 1048576, 131072),
    ModelSpec::new(
        "mimo-v2.6-flash-free",
        Protocol::ChatCompletions,
        200000,
        32000,
    ),
    ModelSpec::new("mimo-v2.5-free", Protocol::ChatCompletions, 200000, 32000),
    ModelSpec::new(
        "ling-3.1-flash-free",
        Protocol::ChatCompletions,
        262144,
        32768,
    ),
    ModelSpec::new(
        "ling-3.0-flash-fin-free",
        Protocol::ChatCompletions,
        262144,
        32768,
    ),
    ModelSpec::new(
        "nemotron-3-ultra-free",
        Protocol::ChatCompletions,
        1000000,
        128000,
    ),
    ModelSpec::new(
        "nemotron-3.5-lightning-free",
        Protocol::ChatCompletions,
        262144,
        262144,
    ),
    ModelSpec::new("gpt-6-astra", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-6-sol", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-6.1-sol", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-6-luna", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-5.6-sol", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-5.6-terra", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-5.6-luna", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-5.5", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-5.5-pro", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-5.4", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-5.4-pro", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-5.4-mini", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5.4-nano", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5.3-codex", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5.3-codex-spark", Protocol::Responses, 128000, 128000),
    ModelSpec::new("gpt-5.2", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5.2-codex", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5.1", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5.1-codex", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5.1-codex-max", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5.1-codex-mini", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5-codex", Protocol::Responses, 400000, 128000),
    ModelSpec::new("gpt-5-nano", Protocol::Responses, 400000, 128000),
    ModelSpec::new("grok-4.7", Protocol::Responses, 500000, 500000),
    ModelSpec::new("grok-4.6", Protocol::Responses, 500000, 500000),
    ModelSpec::new("grok-4.5", Protocol::Responses, 500000, 500000),
    ModelSpec::new("grok-build-0.1", Protocol::Responses, 256000, 256000),
    ModelSpec::new("muse-spark-1.3", Protocol::Responses, 1048576, 131072),
    ModelSpec::new("muse-spark-1.2", Protocol::Responses, 1048576, 131072),
    ModelSpec::new(
        "muse-spark-1.3-contributor-free",
        Protocol::Responses,
        1048576,
        131072,
    ),
    ModelSpec::new("claude-fable-5-1", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-fable-5", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-opus-5-5", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-opus-5", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-opus-4-8", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-opus-4-7", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-opus-4-6", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-opus-4-5", Protocol::Messages, 200000, 64000),
    ModelSpec::new("claude-sonnet-5-5", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-sonnet-5", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-sonnet-4-6", Protocol::Messages, 1000000, 64000),
    ModelSpec::new("claude-sonnet-4-5", Protocol::Messages, 1000000, 64000),
    ModelSpec::new("claude-haiku-5-5", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("claude-haiku-4-5", Protocol::Messages, 200000, 64000),
    ModelSpec::new("qwen3.8-flash", Protocol::Messages, 1000000, 131072),
    ModelSpec::new("qwen3.7-max", Protocol::Messages, 1000000, 131072),
    ModelSpec::new("qwen3.7-plus", Protocol::Messages, 1000000, 131072),
    ModelSpec::new("qwen3.6-plus", Protocol::Messages, 262144, 65536),
    ModelSpec::new("qwen3.5-plus", Protocol::Messages, 262144, 65536),
    ModelSpec::new("gemini-3.8-flash", Protocol::Gemini, 1048576, 65536),
    ModelSpec::new("gemini-3.7-flash", Protocol::Gemini, 1048576, 65536),
    ModelSpec::new("gemini-3.6-flash", Protocol::Gemini, 1048576, 65536),
    ModelSpec::new("gemini-3.5-flash", Protocol::Gemini, 1048576, 65536),
    ModelSpec::new("gemini-3.5-flash-lite", Protocol::Gemini, 1048576, 65536),
    ModelSpec::new("gemini-3.1-pro", Protocol::Gemini, 1048576, 65536),
    ModelSpec::new("gemini-3-flash", Protocol::Gemini, 1048576, 65536),
];

pub(super) const GO_MODELS: &[ModelSpec] = &[
    ModelSpec::new("glm-5.3-flash", Protocol::ChatCompletions, 1000000, 131072),
    ModelSpec::new("glm-5.3", Protocol::ChatCompletions, 1000000, 131072),
    ModelSpec::new("glm-5.2", Protocol::ChatCompletions, 1000000, 131072),
    ModelSpec::new("kimi-k3", Protocol::ChatCompletions, 1048576, 131072),
    ModelSpec::new("kimi-k2.7-code", Protocol::ChatCompletions, 262144, 262144),
    ModelSpec::new("kimi-k2.6", Protocol::ChatCompletions, 262144, 65536),
    ModelSpec::new("longcat-2.0", Protocol::ChatCompletions, 1000000, 131072),
    ModelSpec::new(
        "deepseek-v4.1-flash",
        Protocol::ChatCompletions,
        1000000,
        384000,
    ),
    ModelSpec::new(
        "deepseek-v4-pro",
        Protocol::ChatCompletions,
        1000000,
        384000,
    ),
    ModelSpec::new(
        "deepseek-v4-flash",
        Protocol::ChatCompletions,
        1000000,
        384000,
    ),
    ModelSpec::new(
        "deepseek-v4-flash-vision-exp",
        Protocol::ChatCompletions,
        1000000,
        384000,
    ),
    ModelSpec::new(
        "mimo-v2.6-flash",
        Protocol::ChatCompletions,
        1048576,
        131072,
    ),
    ModelSpec::new("mimo-v2.6-pro", Protocol::ChatCompletions, 1048576, 131072),
    ModelSpec::new("mimo-v2.5", Protocol::ChatCompletions, 1000000, 128000),
    ModelSpec::new("mimo-v2.5-pro", Protocol::ChatCompletions, 1048576, 128000),
    ModelSpec::new("hy4-preview", Protocol::ChatCompletions, 1024000, 64000),
    ModelSpec::new("hy3", Protocol::ChatCompletions, 256000, 128000),
    ModelSpec::new("space-bunny", Protocol::ChatCompletions, 1048576, 524288),
    ModelSpec::new(
        "longcat-2.5-preview-free",
        Protocol::ChatCompletions,
        1000000,
        131072,
    ),
    ModelSpec::new(
        "step-5-preview-free",
        Protocol::ChatCompletions,
        1000000,
        65536,
    ),
    ModelSpec::new("grok-4.7", Protocol::Responses, 500000, 500000),
    ModelSpec::new("grok-4.6", Protocol::Responses, 500000, 500000),
    ModelSpec::new("gpt-6-luna", Protocol::Responses, 1050000, 128000),
    ModelSpec::new("gpt-5.6-luna", Protocol::Responses, 1050000, 128000),
    ModelSpec::new(
        "muse-spark-1.3-contributor",
        Protocol::Responses,
        1048576,
        131072,
    ),
    ModelSpec::new(
        "muse-spark-1.2-contributor",
        Protocol::Responses,
        1048576,
        131072,
    ),
    ModelSpec::new("claude-haiku-5-5", Protocol::Messages, 1000000, 128000),
    ModelSpec::new("minimax-m3", Protocol::Messages, 1000000, 131072),
    ModelSpec::new("minimax-m2.7", Protocol::Messages, 204800, 131072),
    ModelSpec::new("qwen3.8-max", Protocol::Messages, 1000000, 131072),
    ModelSpec::new("qwen3.8-flash", Protocol::Messages, 1000000, 131072),
    ModelSpec::new("qwen3.7-plus", Protocol::Messages, 1000000, 65536),
];

/// DeepSeek 直连接口的叙事候选；FIM 输出上限单独由端点能力限制。
pub(super) const DEEPSEEK_MODELS: &[ModelSpec] = &[
    ModelSpec::new(
        "deepseek-v4-pro",
        Protocol::ChatCompletions,
        1_048_576,
        393_216,
    ),
    ModelSpec::new(
        "deepseek-flash",
        Protocol::ChatCompletions,
        1_048_576,
        393_216,
    ),
];

#[cfg(test)]
mod tests;
