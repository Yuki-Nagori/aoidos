//! 基于 serde / serde_json 的 JSON 编解码、重复键校验与规范化。
//!
//! 公共 API 明确以 [`serde_json::Value`] 为交换类型；这是选定依赖，不承诺解析库可替换。
//! 消费方直接依赖 serde_json 并导入其 Value / Map / Number / json!，本 crate 不转导出。
//! 字节预算、稳定脱敏错误和重复键校验在此共用；领域 schema、JSONL 行规则、
//! hash 协议和 I/O 留在消费模块。普通编码保持 serde 字段顺序，规范化须显式调用。

mod decode;
mod encode;
mod error;
pub use decode::{decode, decode_bytes, parse};
pub use encode::{canonical_string, decode_value, from_value, to_string, to_value, to_vec};
pub use error::Error;
