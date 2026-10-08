//! mythos-store：存储与文件基建（原子写、路径消毒、SQLite 迁移、多开锁）。
//!
//! 设计见 ai-docs/architecture/storage.md；本 crate 不依赖 tauri，
//! 数据根路径由装配层注入，普通文件覆写经 [`atomic`]；SQLite 由事务和 backup API 保证一致性。

pub mod applied;
pub mod atomic;
pub mod db;
pub mod error;
pub mod journal;
pub mod lock;
pub mod paths;
pub mod read;
