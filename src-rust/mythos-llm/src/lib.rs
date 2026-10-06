//! mythos-llm：LLM 供应商适配、流式护栏与单层请求策略。
//!
//! 设计见 ai-docs/architecture/llm.md，预算与错误码口径见通信契约。本 crate 不依赖 tauri，
//! 不生成 turnId、不投递窗口事件、不写数据库；费用账本经 [`schedule::BudgetPort`] 注入（035 落地前默认放行）。
//! 传输层显式禁用隐式重试与重连：reqwest `retry(never)`，SSE 解析器只消费既有字节流。

pub mod decode;
pub mod error;
pub mod guard;
pub mod provider;
pub mod providers;
pub mod schedule;
pub mod sse;

#[cfg(test)]
pub(crate) mod testserver;

/// 安装 ring 作为进程级 rustls CryptoProvider（幂等）。
/// reqwest 走 `rustls-no-provider`：不引入 aws-lc-rs 的 CMake/NASM 构建依赖，
/// 由本函数在构造客户端 / 测试 TLS 夹具前确定 provider。
pub(crate) fn install_ring_provider() {
    // 已安装（重复构建 adapter）时忽略 Err。
    let _ = rustls::crypto::ring::default_provider().install_default();
}
