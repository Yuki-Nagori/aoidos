//! crate 级共享词汇：采样参数。
//!
//! 被 `provider`（请求载荷）、`config`（profile 持久化）与 `schedule`
//! （温度阶梯校验）消费；独立成模块使持久化与调度不反向依赖传输层。

/// 采样参数；thinking 与 temperature 冲突时由调用方禁用阶梯并省略无效参数。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sampling {
    pub temperature: f64,
    pub max_tokens: u32,
}
