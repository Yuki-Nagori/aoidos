//! Provider 抽象：薄 trait、能力声明与规范化增量。
//!
//! 一次 [`Provider::start`] 恰好是一次 HTTP 尝试：无隐式重试、无自动重连（reqwest 已显式
//! `retry(never)`，SSE 解析器只消费既有字节流）。重试 / 阶梯 / 看门狗归 [`crate::schedule`]。
//! 凭据经 `SecretString` 内部持有，不进序列化载荷；Reasoning 仅识别协议，不累积进正文。

use futures::stream::Stream;
use secrecy::SecretString;
use std::future::Future;
use std::pin::Pin;

use crate::error::ProviderError;
use crate::sampling::Sampling;

/// 调用形态；prefix 属于 Chat 的能力分支，由 [`ProviderCapabilities::prefix`] 声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    Completion,
    Chat,
}

/// 模型 / 端点能力；未知一律当作不支持，不默认 true。形态由调用方在
/// [`Provider::capabilities`] 的入参中给出，不在能力表内重复。
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderCapabilities {
    pub model: String,
    /// 已核验的上下文窗口；未知时拒绝记录投影，不推测无限容量。
    pub context_limit: Option<u32>,
    pub completion: bool,
    pub chat: bool,
    /// 末条 assistant 消息带 `prefix` 标志的续写能力（Chat 形态）。
    pub prefix: bool,
    /// 该形态是否支持 thinking。
    pub thinking: bool,
    /// 服务端 stop 参数上限；0 表示端点不支持 stop。
    pub stop_limit: usize,
    /// 单次输出 token 上限（端点口径，不是通用模型口径）。
    pub max_output_tokens: u32,
    /// 该形态下 temperature 是否影响采样。
    pub temperature_effective: bool,
    /// temperature 合法范围（含端点）。
    pub temperature_min: f64,
    pub temperature_max: f64,
}

/// Completion 形态输入：prompt 非空，open tag 由记录投影生成，结果不含 prompt / echo。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionInput {
    pub prompt: String,
}

/// 单条 chat 消息；role 仅 system / user / assistant。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

/// Chat 形态输入；`assistant_prefix` 由 adapter 注入（普通 chat 不能忽略该字段）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatInput {
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_prefix: Option<String>,
}

/// 已选定调用形态的输入。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ProviderInput {
    Completion(CompletionInput),
    Chat(ChatInput),
}

impl ProviderInput {
    /// 输入联合类型的调用形态；配置、请求与 IPC 接线共用这一映射。
    #[must_use]
    pub fn mode(&self) -> RequestMode {
        match self {
            Self::Completion(_) => RequestMode::Completion,
            Self::Chat(_) => RequestMode::Chat,
        }
    }
}

/// 一次物理请求的全部输入：模式已选定，只带适用的服务端 stop 子集；
/// `GuardSpec` 匹配状态不进 adapter（护栏归调用方）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRequest {
    pub model: String,
    pub input: ProviderInput,
    pub sampling: Sampling,
    pub stops: Vec<String>,
}

impl ProviderRequest {
    /// 请求的调用形态，由已选定的 [`ProviderInput`] 决定。
    #[must_use]
    pub fn mode(&self) -> RequestMode {
        self.input.mode()
    }

    /// 传输重试 / 温度阶梯复用同一冻结输入，仅改 temperature。
    #[must_use]
    pub fn with_temperature(&self, temperature: f64) -> Self {
        Self {
            model: self.model.clone(),
            input: self.input.clone(),
            sampling: Sampling {
                temperature,
                max_tokens: self.sampling.max_tokens,
            },
            stops: self.stops.clone(),
        }
    }
}

/// 供应商侧 usage；重试正文被丢弃也不能抹去已发生的用量，由调度器累计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Provider-reported cached input subset; None means the provider omitted it.
    pub cached_prompt_tokens: Option<u64>,
    /// Reasoning is a subset of completion output and is never added to billable output.
    pub reasoning_tokens: Option<u64>,
}

impl Usage {
    pub fn merge(&mut self, other: Usage) {
        let empty = self.prompt_tokens == 0 && self.completion_tokens == 0;
        self.prompt_tokens = self.prompt_tokens.saturating_add(other.prompt_tokens);
        self.completion_tokens = self
            .completion_tokens
            .saturating_add(other.completion_tokens);
        self.cached_prompt_tokens =
            merge_optional_count(self.cached_prompt_tokens, other.cached_prompt_tokens, empty);
        self.reasoning_tokens =
            merge_optional_count(self.reasoning_tokens, other.reasoning_tokens, empty);
    }
}

fn merge_optional_count(current: Option<u64>, next: Option<u64>, empty: bool) -> Option<u64> {
    if empty {
        return next;
    }
    current
        .zip(next)
        .map(|(current, next)| current.saturating_add(next))
}

/// 服务端 finish 的合法取值；`content_filter` / 未知 finish / 工具调用按 bad-response 失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFinish {
    Stop,
    Length,
}

/// 规范化增量：adapter 把 completion 的 `choices[0].text` 与 chat 的
/// `choices[0].delta.content` 归并为同一种 Text；单个 Text 可为空串（不改变首交付状态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderDelta {
    Text(String),
    Reasoning(String),
    Usage(Usage),
    Finish(ProviderFinish),
}

/// Provider 流：Send 的 boxed stream，不借用栈上的 prompt。
pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderDelta, ProviderError>> + Send>>;

/// start 返回的 boxed future；锁定的 Rust 版本可对 async trait 动态分派时也不改变语义。
pub type StartFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderStream, ProviderError>> + Send + 'a>>;

/// 薄 Provider trait：适配层只做传输与协议，不自带重试。
pub trait Provider: Send + Sync {
    /// 能力声明，不发网络请求。
    fn capabilities(&self, model: &str, mode: RequestMode) -> ProviderCapabilities;

    /// 发起一次 HTTP 尝试；`cancellation` 触发时中止传输，不产生额外请求。
    fn start(
        &self,
        request: ProviderRequest,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> StartFuture<'_>;
}

/// 凭据引用：Rust 内部持有，Debug 不泄漏明文，不参与任何序列化。
pub type Credential = SecretString;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_temperature_only_changes_temperature() {
        let request = ProviderRequest {
            model: "m".into(),
            input: ProviderInput::Completion(CompletionInput { prompt: "p".into() }),
            sampling: Sampling {
                temperature: 1.0,
                max_tokens: 128,
            },
            stops: vec!["</narration>".into()],
        };
        let hotter = request.with_temperature(1.1);
        assert_eq!(hotter.sampling.temperature, 1.1);
        assert_eq!(hotter.model, request.model);
        assert_eq!(hotter.stops, request.stops);
        assert_eq!(hotter.sampling.max_tokens, 128);
    }

    #[test]
    fn usage_merge_accumulates() {
        let mut usage = Usage {
            prompt_tokens: 10,
            completion_tokens: 20,
            ..Usage::default()
        };
        usage.merge(Usage {
            prompt_tokens: 1,
            completion_tokens: 2,
            ..Usage::default()
        });
        assert_eq!(
            usage,
            Usage {
                prompt_tokens: 11,
                completion_tokens: 22,
                ..Usage::default()
            }
        );
    }
}
