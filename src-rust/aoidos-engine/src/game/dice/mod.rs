//! 冻结 PbtA 计划与 chacha20-v1 无偏骰判；恢复只读取既有骰值。

use crate::{
    fault::Fault,
    record::{
        facts::{CheckPlan, ResultPolicy},
        format::{self, Body, CheckResult, Modifier, RngTrace, Roll, Source},
    },
};
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};

/// 剧本注册层提供的固定规则 / 角色 / 后果，模型不能覆盖这些字段。
pub struct PlanSpec<'a> {
    pub rule_id: &'a str,
    pub actor_id: &'a str,
    pub modifiers: Vec<Modifier>,
    pub branches: [String; 3],
    pub world_revision: &'a str,
}

/// 冻结默认 PbtA 计划，摘要覆盖身份、规则、修正来源和世界 revision。
/// # Errors
/// 未登记身份形状、空 revision、修正超限或非法来源拒绝。
pub fn freeze_pbta(spec: PlanSpec<'_>) -> Result<CheckPlan, Fault> {
    let modifier = modifier_total(&spec.modifiers)?;
    if !format::valid_id(spec.rule_id)
        || !format::valid_id(spec.actor_id)
        || spec.branches.iter().any(|id| !format::valid_id(id))
        || spec.world_revision.is_empty()
        || spec.world_revision.len() > 256
    {
        return Err(Fault::bad_request());
    }
    let expression = if modifier == 0 {
        "2d6".into()
    } else {
        format!("2d6{modifier:+}")
    };
    let mut plan = CheckPlan {
        plan_id: uuid::Uuid::new_v4().to_string(),
        rule_id: spec.rule_id.into(),
        rule_version: 1,
        actor_id: spec.actor_id.into(),
        expression,
        modifiers: spec.modifiers,
        result_policy: ResultPolicy {
            kind: "pbta-2d6-v1".into(),
            dc: None,
        },
        success_branch_id: spec.branches[0].clone(),
        costly_success_branch_id: spec.branches[1].clone(),
        failure_branch_id: spec.branches[2].clone(),
        world_revision: spec.world_revision.into(),
        plan_hash: String::new(),
    };
    plan.plan_hash = plan_hash(&plan)?;
    Ok(plan)
}
pub(super) fn plan_hash(plan: &CheckPlan) -> Result<String, Fault> {
    let mut value = aoidos_json::to_value(plan).map_err(encode_error)?;
    value
        .as_object_mut()
        .expect("CheckPlan serializes as an object")
        .remove("planHash");
    // 摘要协议要求排序后的紧凑 JSON + LF；规范化本身不修改已存记录。
    let mut canonical = aoidos_json::canonical_string(&value).map_err(encode_error)?;
    canonical.push('\n');
    Ok(format::hash(canonical.as_bytes()))
}
fn encode_error(_: aoidos_json::Error) -> Fault {
    Fault::bad_request()
}
fn modifier_total(modifiers: &[Modifier]) -> Result<i32, Fault> {
    if modifiers.len() > 32
        || modifiers.iter().any(|m| {
            !(-100..=100).contains(&m.value)
                || !format::valid_id(&m.source.kind)
                || !format::valid_id(&m.source.id)
        })
    {
        return Err(Fault::bad_request());
    }
    let total = modifiers.iter().map(|m| m.value).sum::<i32>();
    if !(-3..=3).contains(&total) {
        return Err(Fault::bad_request());
    }
    Ok(total)
}
/// 掷骰前重核计划 hash / 规则和冻结 revision，避免使用过期能力。
/// # Errors
/// 未支持的规则版本、被改写计划或过期世界返回 invalid-phase。
pub fn validate_plan(plan: &CheckPlan, world_revision: &str) -> Result<(), Fault> {
    let modifier = modifier_total(&plan.modifiers).map_err(invalid_plan)?;
    if !format::valid_id(&plan.plan_id)
        || !format::valid_id(&plan.rule_id)
        || !format::valid_id(&plan.actor_id)
        || [
            &plan.success_branch_id,
            &plan.costly_success_branch_id,
            &plan.failure_branch_id,
        ]
        .iter()
        .any(|id| !format::valid_id(id))
        || plan.world_revision.is_empty()
        || plan.world_revision.len() > 256
        || plan.world_revision != world_revision
        || plan.plan_hash != plan_hash(plan)?
        || plan.rule_version != 1
        || plan.result_policy.kind != "pbta-2d6-v1"
        || plan.result_policy.dc.is_some()
        || format::dice_expression(&plan.expression) != Some((2, 6, modifier))
    {
        return Err(Fault::new(
            "engine.invalid-phase",
            "骰判计划已过期或不受支持",
        ));
    }
    Ok(())
}
fn invalid_plan(_: Fault) -> Fault {
    Fault::new("engine.invalid-phase", "骰判计划已过期或不受支持")
}
/// 三档均是推进结果；失败不在此决定对局结束。
pub fn classify(total: i32) -> CheckResult {
    match total {
        10.. => CheckResult::Success,
        7..=9 => CheckResult::CostlySuccess,
        _ => CheckResult::Failure,
    }
}
/// 只在新计划执行时取得系统熵，失败不会生成备用 seed。
/// # Errors
/// 系统熵不可用返回稳定诊断，不输出原始 OS 错误。
pub fn fresh_seed() -> Result<[u8; 32], Fault> {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).map_err(entropy_error)?;
    Ok(seed)
}
fn entropy_error(_: getrandom::Error) -> Fault {
    Fault::new("engine.invalid-phase", "系统随机源不可用")
}
/// 固定 stream / word position，从 little-endian u32 流做拒绝采样。
/// # Errors
/// 计划不合法、过期或采样硬上限触发时拒绝，不提交部分骰子。
pub fn roll(plan: &CheckPlan, world_revision: &str, seed: [u8; 32]) -> Result<Body, Fault> {
    validate_plan(plan, world_revision)?;
    let mut rng = ChaCha20Rng::from_seed(seed);
    rng.set_stream(0);
    rng.set_word_pos(0);
    let (rolls, consumed) = sample(2, 6, || rng.next_u32())?;
    let total =
        rolls.iter().map(|r| r.value as i32).sum::<i32>() + modifier_total(&plan.modifiers)?;
    Ok(Body::Dice {
        expression: plan.expression.clone(),
        rolls,
        total,
        source: Source {
            kind: "rule".into(),
            id: plan.rule_id.clone(),
        },
        plan_id: plan.plan_id.clone(),
        rng: RngTrace {
            algorithm: "chacha20-v1".into(),
            mapping_version: 1,
            seed: seed.iter().map(|b| format!("{b:02x}")).collect(),
            start_counter: "0".into(),
            end_counter: consumed.to_string(),
        },
        modifiers: plan.modifiers.clone(),
    })
}
fn sample(
    count: u32,
    sides: u32,
    mut word: impl FnMut() -> u32,
) -> Result<(Vec<Roll>, u64), Fault> {
    let limit = (1u64 << 32) / u64::from(sides) * u64::from(sides);
    let mut rolls = Vec::new();
    let mut consumed = 0;
    while rolls.len() < (count as usize) {
        if consumed == 4096 {
            return Err(Fault::new("engine.invalid-phase", "骰判采样超过工作上限"));
        }
        let x = u64::from(word());
        consumed += 1;
        if x < limit {
            rolls.push(Roll {
                sides,
                value: (x % u64::from(sides) + 1) as u32,
            });
        }
    }
    Ok((rolls, consumed))
}

#[cfg(test)]
mod tests;
