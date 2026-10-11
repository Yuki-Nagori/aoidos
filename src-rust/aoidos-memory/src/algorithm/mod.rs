//! 纯记忆算法：输入冻结策略、有效增益与逻辑时钟，不访问仓储或外部状态。

use crate::{
    error::{Error, Reason, Result},
    model::{Algorithm, MAX_INTEGER, Policy, reject},
};

const FIXED_SCALE: u64 = 1_000_000;

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
