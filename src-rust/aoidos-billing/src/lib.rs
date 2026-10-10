//! LLM 费用账本与精确金额类型。
//!
//! 领域规则不依赖 Tauri；共享 SQLite 连接和迁移顺序由引擎提供，界面通过壳命令访问。

pub mod money;
pub mod price;
pub mod schema;
pub mod usage;
