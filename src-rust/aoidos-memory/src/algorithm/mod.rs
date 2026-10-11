//! 纯记忆算法：输入冻结策略、有效增益与逻辑时钟，不访问仓储或外部状态。

use crate::{
    error::{Error, Reason, Result},
    model::{Algorithm, MAX_CANDIDATES, MAX_INTEGER, Policy, reject, valid_id, valid_short_id},
};

const FIXED_SCALE: u64 = 1_000_000;
pub const MAX_ARCHIVE_CANDIDATES: usize = 10_000;
const MAX_OPERATION_TARGETS: usize = 16;

/// 候选与它触及的稳定素材 / 现有条目；位置由调用方从原始素材登记顺序取得。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateWork {
    pub candidate_id: String,
    pub earliest_source_position: u64,
    pub material_ids: Vec<String>,
    pub target_entry_ids: Vec<String>,
}

/// 把共享素材或目标的候选组合为原子依赖组，并按素材顺序稳定排列。
///
/// 独立候选形成单项组；同组最多触及 16 个不同条目，与仓储操作上限一致。
/// 此函数只分组，不校验候选权限、不构造或提交操作。
///
/// # Errors
/// 非法 / 重复身份、空或超限素材集合、非法位置、候选数超限、目标过多拒绝规划。
pub fn dependency_groups(candidates: &[CandidateWork]) -> Result<Vec<Vec<String>>> {
    if candidates.len() > MAX_CANDIDATES {
        return Err(reject());
    }

    let mut candidate_ids = std::collections::BTreeSet::new();
    for candidate in candidates {
        if !valid_short_id(&candidate.candidate_id)
            || !candidate_ids.insert(candidate.candidate_id.as_str())
            || candidate.earliest_source_position > MAX_INTEGER
            || candidate.material_ids.is_empty()
            || candidate.material_ids.len() > MAX_CANDIDATES
            || candidate.target_entry_ids.len() > MAX_OPERATION_TARGETS
            || !unique_valid_ids(&candidate.material_ids)
            || !unique_valid_ids(&candidate.target_entry_ids)
        {
            return Err(reject());
        }
    }

    let mut order = (0..candidates.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        candidates[*left]
            .earliest_source_position
            .cmp(&candidates[*right].earliest_source_position)
            .then_with(|| {
                candidates[*left]
                    .candidate_id
                    .as_bytes()
                    .cmp(candidates[*right].candidate_id.as_bytes())
            })
    });

    let mut visited = vec![false; candidates.len()];
    let mut groups = Vec::new();
    for &start in &order {
        if visited[start] {
            continue;
        }
        let mut pending = vec![start];
        let mut members = Vec::new();
        while let Some(current) = pending.pop() {
            if visited[current] {
                continue;
            }
            visited[current] = true;
            members.push(current);
            for &candidate in &order {
                if !visited[candidate] && dependent(&candidates[current], &candidates[candidate]) {
                    pending.push(candidate);
                }
            }
        }

        let targets = members
            .iter()
            .flat_map(|index| candidates[*index].target_entry_ids.iter())
            .collect::<std::collections::BTreeSet<_>>();
        if targets.len() > MAX_OPERATION_TARGETS {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        members.sort_by(|left, right| {
            candidates[*left]
                .earliest_source_position
                .cmp(&candidates[*right].earliest_source_position)
                .then_with(|| {
                    candidates[*left]
                        .candidate_id
                        .as_bytes()
                        .cmp(candidates[*right].candidate_id.as_bytes())
                })
        });
        groups.push(
            members
                .into_iter()
                .map(|index| candidates[index].candidate_id.clone())
                .collect(),
        );
    }
    Ok(groups)
}

fn dependent(left: &CandidateWork, right: &CandidateWork) -> bool {
    left.material_ids
        .iter()
        .any(|id| right.material_ids.contains(id))
        || left
            .target_entry_ids
            .iter()
            .any(|id| right.target_entry_ids.contains(id))
}

fn unique_valid_ids(ids: &[String]) -> bool {
    let unique = ids.iter().collect::<std::collections::BTreeSet<_>>();
    unique.len() == ids.len() && ids.iter().all(|id| valid_id(id))
}

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
