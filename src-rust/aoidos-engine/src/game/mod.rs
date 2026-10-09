//! 产品游戏回合：规则与阶段属于引擎，LLM 子调用复用 turn 协调器。

pub mod catalog;
pub mod control;
pub mod dice;
pub mod domain;
pub mod execution;
pub mod input;
pub mod proposal;
pub mod publication;
pub mod recovery;
pub mod reducer;
pub mod runtime;
pub mod state;
#[cfg(test)]
mod test_support;

pub(crate) fn invalid_phase() -> crate::fault::Fault {
    crate::fault::Fault::new("engine.invalid-phase", "当前阶段或检查点不允许此操作")
}

pub mod generation;

pub mod builtin;

pub mod product;

pub mod assets;
