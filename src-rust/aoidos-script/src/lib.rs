//! 原文剧本的纯解析边界；文件加载、来源许可和可执行规则由消费端登记。

mod format;
pub use format::{Error, MAX_BYTES, Scenario, decode, valid_id};
