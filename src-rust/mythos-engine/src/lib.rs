//! 对局引擎基座：串行回合协调、持久记录与恢复；产品阶段决策由 023 接入。

pub mod blocking;
pub mod fault;
pub mod game;
pub mod ports;
pub mod record;
pub mod request;
pub mod turn;

#[cfg(debug_assertions)]
pub mod fixture;
pub mod migration;
pub mod preferences;
pub mod records;
pub mod storage;

#[cfg(test)]
mod test_support;
