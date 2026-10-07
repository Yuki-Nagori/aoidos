//! 对局引擎基座：串行回合协调与输出边界，阶段决策 / 持久记录由后续任务接入。

pub mod fault;
pub mod ports;
pub mod request;
pub mod turn;

#[cfg(debug_assertions)]
pub mod fixture;
