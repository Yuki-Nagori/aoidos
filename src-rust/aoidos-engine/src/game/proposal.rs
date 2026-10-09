//! 有界结构化候选；不从叙事正文挖指令，不发修复 prompt。

use crate::{fault::Fault, ports::Outcome, record::format, turn::PrivateResult};
use aoidos_llm::schedule::FinishReason;
use serde::Deserialize;

pub const MAX_PROPOSAL_BYTES: usize = 8 * 1024;
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CheckProposal {
    Check {
        rule_id: String,
        actor_id: String,
        reason: String,
    },
    NoCheck {},
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SceneProposal {
    Scene { scene_id: String },
    Stay {},
}
fn decode<T: serde::de::DeserializeOwned>(result: &PrivateResult) -> Result<T, Fault> {
    if result.outcome != Outcome::Completed
        || !matches!(
            result.finish_reason,
            Some(FinishReason::Stop | FinishReason::Guard)
        )
        || result.text.len() > MAX_PROPOSAL_BYTES
    {
        return Err(invalid_proposal());
    }
    aoidos_json::decode(&result.text, MAX_PROPOSAL_BYTES).map_err(decode_protocol)
}
fn decode_protocol(_: aoidos_json::Error) -> Fault {
    invalid_proposal()
}
fn invalid_proposal() -> Fault {
    Fault::new("llm.bad-response", "内部提议结构或终态不合法")
}
/// 只接受候选 ID；合法性 / 可行动角色由当前场景注册域再次校验。
/// # Errors
/// 未知字段、重复键、非法 ID、超限理由或不完整响应为 bad-response。
pub fn check(result: &PrivateResult) -> Result<CheckProposal, Fault> {
    let proposal: CheckProposal = decode(result)?;
    if let CheckProposal::Check {
        rule_id,
        actor_id,
        reason,
    } = &proposal
        && (!format::valid_id(rule_id)
            || !format::valid_id(actor_id)
            || reason.trim().is_empty()
            || reason.len() > 512)
    {
        return Err(invalid_proposal());
    }
    Ok(proposal)
}
/// 场景候选只含登记 ID / stay，不接受模型路径或世界补丁。
/// # Errors
/// 同 check 的结构 / 终态拒绝，不在解析后自动重发。
pub fn scene(result: &PrivateResult) -> Result<SceneProposal, Fault> {
    let proposal: SceneProposal = decode(result)?;
    if let SceneProposal::Scene { scene_id } = &proposal
        && !format::valid_id(scene_id)
    {
        return Err(invalid_proposal());
    }
    Ok(proposal)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn result(text: &str) -> PrivateResult {
        PrivateResult {
            turn_id: uuid::Uuid::new_v4().to_string(),
            text: text.into(),
            outcome: Outcome::Completed,
            finish_reason: Some(FinishReason::Stop),
        }
    }
    #[test]
    fn proposals_reject_foreign_fields_duplicate_keys_and_truncated_finishes() {
        assert_eq!(
            check(&result(r#"{"kind":"noCheck"}"#)).unwrap(),
            CheckProposal::NoCheck {}
        );
        assert!(
            check(&result(
                r#"{"kind":"check","ruleId":"search","actorId":"player","reason":"雾中寻找出口"}"#
            ))
            .is_ok()
        );
        for text in [
            r#"{"kind":"noCheck","worldPatch":{}}"#,
            r#"{"kind":"noCheck","kind":"check"}"#,
            r#"{"kind":"check","ruleId":"/","actorId":"p","reason":"x"}"#,
            "{",
        ] {
            assert!(check(&result(text)).is_err());
        }
        let mut truncated = result(r#"{"kind":"noCheck"}"#);
        truncated.finish_reason = Some(FinishReason::Length);
        assert!(check(&truncated).is_err());
        assert_eq!(
            scene(&result(r#"{"kind":"stay"}"#)).unwrap(),
            SceneProposal::Stay {}
        );
        assert!(scene(&result(r#"{"kind":"scene","sceneId":"room"}"#)).is_ok());
        assert!(scene(&result(r#"{"kind":"scene","sceneId":"/"}"#)).is_err());
        assert!(scene(&result(&"x".repeat(MAX_PROPOSAL_BYTES + 1))).is_err());
    }
}
