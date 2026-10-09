//! 记忆存储与恢复：借用业务单写者连接，不建立第二份世界或模型调用链路。
//!
//! 领域写入须在调用方的记录锁及共享数据库锁内完成；来源端口在同一边界核验。
//! 本 crate 不拥有 SQLite 连接、实例锁或后台任务，不接入对局投影热路径。

pub mod error;
pub mod model;
pub mod repository;
pub mod schema;
