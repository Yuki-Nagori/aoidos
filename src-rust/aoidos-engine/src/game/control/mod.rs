//! 产品回退只接受已核验的步骤边界；所有控制追加到原文件，保留旧物理历史。

use super::recovery::{self, Recovery};
use crate::{
    fault::Fault,
    record::{
        facts::{Fact, ForkMode},
        format::{self, Body},
        session::{Session, store_fault},
    },
};
use sha2::{Digest, Sha256};

/// 最近终态回合的叙事前位置；有判定时只能复用 check 引用的既有骰子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegenerationTarget {
    pub target_seq: u64,
    pub source_round_id: String,
    pub reused_dice_seq: Option<u64>,
}
/// # Errors
/// 非最近终态回合 / 尚无 check 或 checkSkipped 拒绝，不增加重掷路径。
pub fn regeneration_target(
    recovered: &Recovery,
    round_id: &str,
) -> Result<RegenerationTarget, Fault> {
    if !format::valid_uuid(round_id) {
        return Err(Fault::bad_request());
    }
    let round = recovered.round.as_ref().ok_or_else(super::invalid_phase)?;
    if recovered.needs_recovery || round.identity.round_id != round_id || round.ended.is_none() {
        return Err(super::invalid_phase());
    }
    let target_seq = round
        .check
        .or(round.skipped)
        .ok_or_else(super::invalid_phase)?;
    Ok(RegenerationTarget {
        target_seq,
        source_round_id: round.identity.round_id.clone(),
        reused_dice_seq: round.dice,
    })
}
/// 验证当前有效路径的合法边界；存储中的 seq 存在本身不是回退授权。
/// # Errors
/// pending、无效路径、骰子与分档之间或任意正文 / 自由行切片拒绝。
pub fn verify_boundary(session: &Session, target: u64) -> Result<(), Fault> {
    if target == 0 || target > format::MAX_SEQ {
        return Err(Fault::bad_request());
    }
    if session.needs_recovery() || session.is_read_only() || session.in_flight().is_some() {
        return Err(super::invalid_phase());
    }
    let mut path = session.path_at(session.history_revision())?;
    if !path.contains(&target) {
        return Err(Fault::bad_request());
    }
    path.retain(|seq| *seq <= target);
    verify_projected_boundary(
        recovery::derive_path(session, None, path, session.history_revision(), false)?,
        target,
    )
}

/// 恢复已落盘 historyFork 的结构性边界核验；调用者仍负责同事务 applied / 世界重建。
/// 该入口不准许新控制，也不确认 pending 事实，只检查记录载荷里的父因果路径。
/// # Errors
/// 非法父路径、未知记录或非确认步骤边界拒绝；当前 partial 不属于父前缀。
pub fn verify_replay_boundary(session: &Session, parent: u64, target: u64) -> Result<(), Fault> {
    if target == 0 || target > format::MAX_SEQ {
        return Err(Fault::bad_request());
    }
    let mut path = session.path_at(parent)?;
    if !path.contains(&target) {
        return Err(Fault::bad_request());
    }
    path.retain(|seq| *seq <= target);
    verify_projected_boundary(
        recovery::derive_path(session, None, path, parent, false)?,
        target,
    )
}
fn verify_projected_boundary(recovered: Recovery, target: u64) -> Result<(), Fault> {
    let round = recovered.round.ok_or_else(super::invalid_phase)?;
    let boundary = round
        .ended
        .as_ref()
        .is_some_and(|(seq, _, _)| *seq == target)
        || (round.plan.as_ref().is_some_and(|(seq, _)| *seq == target) && round.dice.is_none())
        || (round.check == Some(target) && round.narrative.is_none())
        || (round.skipped == Some(target) && round.narrative.is_none());
    if !boundary {
        return Err(super::invalid_phase());
    }
    Ok(())
}
/// 准备固定控制载荷；调用方继续交给 022 commit_history_fork，在 SQL 确认前不发布分支。
/// # Errors
/// 非法身份、边界、源回合 / 骰值复用关系或历史读取失败拒绝，尚不追加任何行。
pub fn prepare_fork(
    session: &Session,
    operation_id: &str,
    mode: ForkMode,
    target: u64,
    regeneration: Option<&RegenerationTarget>,
) -> Result<Body, Fault> {
    if !format::valid_uuid(operation_id) {
        return Err(Fault::bad_request());
    }
    verify_boundary(session, target)?;
    if (mode == ForkMode::Regenerate) != regeneration.is_some() {
        return Err(Fault::bad_request());
    }
    if let Some(expected) = regeneration {
        let current = recovery::derive(session, None)?;
        if regeneration_target(&current, &expected.source_round_id)? != *expected
            || expected.target_seq != target
        {
            return Err(super::invalid_phase());
        }
    }
    let path = session.path_at(session.history_revision())?;
    let mut hash = Sha256::new();
    for seq in path.into_iter().filter(|seq| *seq <= target) {
        hash.update(session.read(seq).map_err(store_fault)?.raw.as_bytes());
    }
    let fact = Fact::HistoryFork {
        version: 1,
        operation_id: operation_id.into(),
        mode,
        parent_control_seq: session.history_revision(),
        target_seq: target,
        prefix_hash: format!("sha256:{:x}", hash.finalize()),
        source_round_id: regeneration.map(|source| source.source_round_id.clone()),
        reused_dice_seq: regeneration.and_then(|source| source.reused_dice_seq),
    };
    fact.body("切换有效因果路径").map_err(store_fault)
}

#[cfg(test)]
mod tests;
