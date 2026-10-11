//! 纯记忆算法：输入冻结策略、有效增益与逻辑时钟，不访问仓储或外部状态。

use crate::{
    error::{Error, Reason, Result},
    model::{Algorithm, MAX_INTEGER, Policy, reject},
};

const FIXED_SCALE: u64 = 1_000_000;
pub const MAX_ARCHIVE_CANDIDATES: usize = 10_000;

/// 已由调用方验证来源与可见权限的活动记忆候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveCandidate {
    pub entry_id: String,
    /// 仅来自可信任务规则的保留优先级，不接受模型评分。
    pub required: bool,
    /// 已由调用方按冻结策略计算的定点强度。
    pub strength: u64,
    /// 最近有效素材的逻辑位置，不使用墙钟或文件时间。
    pub last_logical_position: u64,
}

/// 按确定顺序选出应归档的 ID；输入候选须已过滤失效 / 损坏 / 无权条目。
///
/// 必需候选优先，其余按强度、最近有效素材位置降序，最后按 ID 字节序升序。
/// 本函数只规划，不写入仓储，也不改变认知内容。
///
/// # Errors
/// 超过硬上限、非法容量 / 身份 / 整数或必需候选多于容量时拒绝规划。
pub fn select_archive(candidates: &[ArchiveCandidate], capacity: usize) -> Result<Vec<String>> {
    if candidates.len() > MAX_ARCHIVE_CANDIDATES
        || capacity == 0
        || capacity > MAX_ARCHIVE_CANDIDATES
    {
        return Err(reject());
    }

    let mut ids = std::collections::BTreeSet::new();
    for candidate in candidates {
        if !crate::model::valid_id(&candidate.entry_id)
            || candidate.strength > 1_000_000
            || candidate.last_logical_position > MAX_INTEGER
            || !ids.insert(candidate.entry_id.as_str())
        {
            return Err(reject());
        }
    }

    if candidates
        .iter()
        .filter(|candidate| candidate.required)
        .count()
        > capacity
    {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }

    let mut ordered = candidates.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        right
            .required
            .cmp(&left.required)
            .then_with(|| right.strength.cmp(&left.strength))
            .then_with(|| right.last_logical_position.cmp(&left.last_logical_position))
            .then_with(|| left.entry_id.as_bytes().cmp(right.entry_id.as_bytes()))
    });

    Ok(ordered
        .into_iter()
        .skip(capacity)
        .map(|candidate| candidate.entry_id.clone())
        .collect())
}

/// 计算用于候选排序的定点强度；不会改写记忆、记录效果或选择归档目标。
///
/// 强化增益先按周目策略封顶。`anchor` 是创建或最近一次有效复活的逻辑回合；
/// 失效来源过滤和锚点选择由调用方负责，不能通过分数函数恢复无效记忆。
///
/// # Errors
/// 策略版本 / 参数无效时返回策略错误；逻辑时间倒退返回 staleVersion；输入超安全整数范围拒绝。
pub fn strength_score(
    policy: &Policy,
    current_clock: u64,
    anchor: u64,
    effective_gain: u64,
) -> Result<u64> {
    policy.validate()?;
    if current_clock > MAX_INTEGER || anchor > MAX_INTEGER || effective_gain > MAX_INTEGER {
        return Err(reject());
    }
    if current_clock < anchor {
        return Err(Error::Rejected(Reason::StaleVersion));
    }

    let gain = effective_gain.min(policy.reinforcement_cap);
    let base = policy
        .base_strength
        .checked_add(gain)
        .ok_or_else(reject)?
        .min(policy.strength_cap);
    let elapsed = current_clock - anchor;

    match policy.algorithm {
        Algorithm::Exponential => {
            let factor = fixed_power(policy.decay_rate, elapsed);
            u64::try_from(u128::from(base) * u128::from(factor) / u128::from(FIXED_SCALE))
                .map_err(|_| Error::Corrupt)
        }
        Algorithm::Segmented => {
            let span = policy.half_life_rounds * 2;
            let remaining = span.saturating_sub(elapsed);
            u64::try_from(u128::from(base) * u128::from(remaining) / u128::from(span))
                .map_err(|_| Error::Corrupt)
        }
        Algorithm::None => Ok(base),
    }
}

/// 定点二进制幂；每一步向下取整，保持跨平台可重放且不使用浮点数。
fn fixed_power(mut base: u64, mut exponent: u64) -> u64 {
    let mut factor = FIXED_SCALE;
    while exponent > 0 {
        if exponent & 1 == 1 {
            factor = factor * base / FIXED_SCALE;
        }
        exponent >>= 1;
        if exponent > 0 {
            base = base * base / FIXED_SCALE;
        }
    }
    factor
}

#[cfg(test)]
mod tests;
