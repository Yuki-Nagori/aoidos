//! 可信剧本 / 世界解释器与冻结请求的接入边界；产品参数不包含规则、SQL 或 profile。

use super::catalog::{ConfirmedWorldView, SceneCatalog};
use crate::{
    fault::Fault,
    record::{
        facts::{CheckPlan, DiceMode, ScenePosition, WorldMutation},
        history::HistoryPort,
        projection::{Budget, Shape},
        session::Session,
        world::WorldPort,
    },
    request::PreparedGeneration,
};
use aoidos_llm::{guard::GuardSpec, provider::ProviderInput};
use std::sync::Arc;

/// 每次接纳冻结一次，后续子调用只使用该份请求构造器；不再次读取用户设置。
pub struct FrozenRound {
    pub profile_id: String,
    pub profile_revision: String,
    pub dice_mode: DiceMode,
    pub generation: Arc<dyn Generation>,
}
/// 供应商和凭据在接纳前解析；内部提议 / 公开正文复用同一预算端口。
pub trait Generation: Send + Sync {
    fn shape(&self) -> Shape;
    fn projection_budget(&self) -> Budget;
    /// # Errors
    /// 能力、Token 估算 / 预算或请求构造失败返回脱敏 Fault，不做额外重试。
    fn prepare(
        &self,
        input: ProviderInput,
        guard: GuardSpec,
        automatic: bool,
    ) -> Result<PreparedGeneration, Fault>;
}
/// 全应用配置入口；无可用配置在分配 lease / roundId 和追加输入之前拒绝。
pub trait RoundFactory: Send + Sync {
    /// # Errors
    /// 未登记 profile、凭据缺失、代理 / 能力或偏好不合法均拒绝接纳。
    fn freeze(&self) -> Result<FrozenRound, Fault>;

    /// Freezes a round with its stable billing scope when the runtime knows the session identity.
    /// Custom/test factories may keep the legacy behavior; product billing overrides this method.
    fn freeze_for_run(&self, _run_id: &str) -> Result<FrozenRound, Fault> {
        self.freeze()
    }
}

/// 只读完成通知；消费者绑定当前因果修订幂等消费，不能反向改写本轮事实。
#[derive(Debug, Clone)]
pub struct CompletedRound {
    pub session_id: String,
    pub round_id: String,
    pub history_revision: u64,
    pub through_seq: u64,
}
/// 只读完成通知的独立消费端；世界解释器不负责记忆调度，也不反向依赖消费者 crate。
pub trait CompletedRounds: Send + Sync {
    /// completed 事实确认后通知一次；重放 / 回退不再次通知，不在回调发付费请求。
    fn completed(&self, round: CompletedRound);
}
/// 每 session 的唯一世界所有者；条件核验、变更与 applied 必须共享 SQL 事务。
/// 没有注册实现的会话可读记录，不能猜测世界属性后继续推进。
pub trait Domain: WorldPort + HistoryPort + Send {
    fn catalog(&self) -> &SceneCatalog;
    fn baseline_scene(&self) -> Option<ScenePosition>;
    /// # Errors
    /// 从已 applied 的当前世界构造有依据的视图；pending / 未知解释器拒绝。
    fn confirmed(&self, session: &Session) -> Result<ConfirmedWorldView, Fault>;
    /// # Errors
    /// 世界只读上下文超限 / 引用失效拒绝；预算截取沿用记录投影层。
    fn context(&self, view: &ConfirmedWorldView) -> Result<String, Fault>;
    /// 冻结目录白名单中的确定性计划；模型提议不提供公式、修正或结果阈值。
    /// # Errors
    /// 未登记规则 / actor、视图过期或规则参数非法拒绝。
    fn check_plan(
        &self,
        rule: &str,
        actor: &str,
        view: &ConfirmedWorldView,
    ) -> Result<CheckPlan, Fault>;
    /// 从登记后果生成最多 32 个固定身份意图；恢复已有 settlementPlanned 时不再调用。
    /// # Errors
    /// 未登记后果、视图过期、身份或载荷超限拒绝，不接收模型 SQL。
    fn consequences(
        &self,
        session: &Session,
        round: &super::recovery::RoundFacts,
        view: &ConfirmedWorldView,
    ) -> Result<Vec<WorldMutation>, Fault>;
    /// # Errors
    /// 只接受目录已登记的出口条件；未命中返回 None，不制造新的结束规则。
    fn exit_reason(&self, scene: &str, view: &ConfirmedWorldView) -> Result<Option<String>, Fault>;
}
