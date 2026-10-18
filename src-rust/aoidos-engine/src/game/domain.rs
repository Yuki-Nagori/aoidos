//! 可信剧本 / 世界解释器与冻结请求的接入边界；产品参数不包含规则、SQL 或 profile。

use super::catalog::{ConfirmedWorldView, SceneCatalog, SceneRuleMatch};
use super::presence::ScenePresenceSnapshot;
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
use aoidos_llm::{
    guard::GuardSpec,
    provider::{ProviderInput, RequestMode},
};
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
    fn request_mode(&self) -> RequestMode {
        match self.shape() {
            Shape::Completion => RequestMode::Completion,
            Shape::Chat | Shape::ChatPrefix => RequestMode::Chat,
        }
    }
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

    /// runtime 已知 session 身份时，为回合冻结稳定的计费范围。
    /// 自定义 / 测试 factory 可沿用旧行为；产品计费实现会覆盖此方法。
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
/// 已通过可信规则求值并成功持久化领取的固定场景提示。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneHint {
    pub hint_id: String,
    pub text: String,
    pub target_scope: String,
}
/// 只读完成通知的独立消费端；世界解释器不负责记忆调度，也不反向依赖消费者 crate。
pub trait CompletedRounds: Send + Sync {
    /// `completed` 事实确认且记录 / 领域锁释放后通知；重放 / 回退不重复通知，不在回调发付费请求。
    fn completed(&self, round: CompletedRound);

    /// 已初始化的记忆周目；轻量测试消费者可不接入记忆运行时。
    fn run_id(&self) -> Option<String> {
        None
    }

    /// 在来源记录追加前持久化有界生命周期事件，关闭记录与记忆之间的崩溃窗口。
    /// # Errors
    /// 事件身份冲突、超限或存储不可用时拒绝提交来源记录。
    fn prepare_cycle_event(
        &self,
        _event: aoidos_memory::model::CycleLifecycleEvent,
    ) -> Result<(), Fault> {
        Ok(())
    }

    /// 消费已提交且仍属于当前历史分支的生命周期事件；失败时保留队列供下次恢复。
    fn drain_cycle_events(&self) -> Result<(), Fault> {
        Ok(())
    }

    /// 为新周目或回退后的当前节点建立基线；已存在的节点保持原始基线不变。
    /// # Errors
    /// 历史修订、来源场景、证据或持久化状态不一致时失败关闭。
    fn ensure_current_node(
        &self,
        _scene_id: &str,
        _history_revision: u64,
        _baseline: &[aoidos_memory::model::SceneRuleEvidence],
    ) -> Result<(), Fault> {
        Ok(())
    }

    /// 消费 023 当前目录产生的规则匹配并原子领取一条登记提示；仅在持久化成功后返回文案。
    /// 调用方必须从最新 applied 世界生成匹配结果；该操作校验 revision 一致性，不改变世界事实。
    /// # Errors
    /// 周目未初始化、生命周期事件待恢复、历史修订不符、规则结果无效或存储失败时拒绝。
    fn claim_scene_hint(
        &self,
        _session: &crate::record::session::Session,
        _scene_id: &str,
        _history_revision: u64,
        _world_revision: &str,
        _matches: &[SceneRuleMatch],
    ) -> Result<Option<SceneHint>, Fault> {
        Err(super::invalid_phase())
    }
}
/// 每 session 的唯一世界所有者；条件核验、变更与 applied 必须共享 SQL 事务。
/// 没有注册实现的会话可读记录，不能猜测世界属性后继续推进。
pub trait Domain: WorldPort + HistoryPort + Send {
    fn catalog(&self) -> &SceneCatalog;
    fn baseline_scene(&self) -> Option<ScenePosition>;
    /// 读取当前完整在场快照；仅查询，不写入状态。
    /// # Errors
    /// 待恢复、策略缺失或 applied 历史无效时失败关闭。
    fn scene_presence(&self, _session: &Session) -> Result<ScenePresenceSnapshot, Fault> {
        Err(super::invalid_phase())
    }
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
