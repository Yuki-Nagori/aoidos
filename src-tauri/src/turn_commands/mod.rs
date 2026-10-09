//! 回合命令只适配运行服务与 IPC；模型执行 / 提交 / 门禁归 aoidos-engine。

use crate::{events, ipc::CmdError};
use aoidos_engine::fault::Fault;
use aoidos_engine::ports::{EventPort, PreparedEvent, TurnEvent};
use aoidos_engine::turn::Coordinator;
use aoidos_llm::proxy::SystemProxySnapshot;
use std::sync::{Arc, Mutex, PoisonError};

/// 统一运行服务；产品后续接入同一个协调器，不新建调试 / private 门禁。
pub struct TurnService {
    pub coordinator: Coordinator,
    pub diagnostics: Arc<Mutex<aoidos_engine::record::calibration::Calibration>>,
    #[cfg(debug_assertions)]
    pub proxy_snapshot: SystemProxySnapshot,
}

impl TurnService {
    /// # Errors
    /// 协调器容量参数非法时拒绝初始化。
    pub fn new(
        events: Arc<dyn EventPort>,
        proxy_snapshot: SystemProxySnapshot,
    ) -> Result<Self, Fault> {
        #[cfg(not(debug_assertions))]
        let _ = proxy_snapshot;
        Ok(Self {
            coordinator: Coordinator::new(events, 16)?,
            diagnostics: Arc::new(Mutex::new(
                aoidos_engine::record::calibration::Calibration::default(),
            )),
            #[cfg(debug_assertions)]
            proxy_snapshot,
        })
    }
}

/// 窗口投递函数，仅接收已准备信封和登记事件名。
pub type WindowDelivery = dyn Fn(&str, serde_json::Value) -> Result<(), Fault> + Send + Sync;

/// 平台投递回调的生命周期；关闭时释放 AppHandle 引用，避免 managed state 引用闭环。
pub struct WindowEvents {
    deliver: Mutex<Option<Arc<WindowDelivery>>>,
}
impl WindowEvents {
    /// 所有事件域共用窗口句柄生命周期；关闭后统一拒绝投递。
    pub(crate) fn dispatch(&self, name: &str, payload: serde_json::Value) -> Result<(), Fault> {
        let deliver = self
            .deliver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(window_closed)?;
        deliver(name, payload)
    }
    /// 注入平台投递；内核不接触窗口类型。
    pub fn new(
        deliver: impl Fn(&str, serde_json::Value) -> Result<(), Fault> + Send + Sync + 'static,
    ) -> Self {
        Self {
            deliver: Mutex::new(Some(Arc::new(deliver))),
        }
    }
    /// 所有生产者退出后关闭，释放捕获的窗口句柄；后续投递显式失败。
    pub fn close(&self) {
        let previous = self
            .deliver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(previous);
    }
}
impl EventPort for WindowEvents {
    fn prepare(&self, event: TurnEvent) -> Result<PreparedEvent, Fault> {
        let prepared =
            events::prepare(event.name(), event.turn_id(), &event).map_err(event_prepare_error)?;
        Ok(PreparedEvent {
            seq: prepared.seq,
            envelope: prepared.payload,
            event,
        })
    }
    fn deliver(&self, event: PreparedEvent) -> Result<(), Fault> {
        self.dispatch(event.event.name(), event.envelope)
    }
    fn retire(&self, turn_id: &str) {
        events::retire_stream(turn_id);
    }
}
fn window_closed() -> Fault {
    Fault::new("app.event-failed", "窗口投递已关闭")
}
fn event_prepare_error(_: CmdError) -> Fault {
    Fault::new("app.event-failed", "回合事件无法准备")
}

/// JSON 边界解码成同型输入；未知 role / 字段按统一错误返回，不漏框架字符串错误。
///
/// # Errors
/// 输入形状不符合 ProviderInput 返回 app.bad-request。
pub fn decode_input(
    input: serde_json::Value,
) -> Result<aoidos_llm::provider::ProviderInput, CmdError> {
    aoidos_json::from_value(input).map_err(invalid_input)
}
fn invalid_input(_: aoidos_json::Error) -> CmdError {
    CmdError::new("app.bad-request", "回合输入形状不合法", None)
}

/// 已接纳的 UUID；空快照已创建，实际执行失败由事件 / 快照报告。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedTurn {
    pub turn_id: String,
}

/// 开发调试入口仅运行本地夹具，035 账本与 022 正式 GrammarSpec 接入前不发付费请求。
///
/// # Errors
/// 无效输入、未知配置 / 护栏 / 供应商、缺密钥、busy 或存储错误。
#[cfg(debug_assertions)]
pub fn submit(
    service: &TurnService,
    profile_id: String,
    input: aoidos_llm::provider::ProviderInput,
    guard_spec_id: String,
) -> Result<AcceptedTurn, CmdError> {
    // 未知护栏必须先拒绝；不得访问凭据或占用门禁。
    if guard_spec_id != aoidos_engine::fixture::GUARD_SPEC_ID {
        return Err(CmdError::new("app.not-found", "护栏配置不存在", None));
    }
    let (profile, auth) = crate::llm_commands::freeze_submission(&profile_id)?;
    submit_frozen(service, profile, auth.as_ref(), input).map_err(CmdError::from)
}

#[cfg(debug_assertions)]
pub(crate) fn submit_frozen(
    service: &TurnService,
    frozen: aoidos_llm::config::FrozenProfile,
    auth: Option<&aoidos_llm::proxy::ProxyAuth>,
    input: aoidos_llm::provider::ProviderInput,
) -> Result<AcceptedTurn, Fault> {
    use aoidos_engine::{ports::MemoryWriter, request::PreparedGeneration};
    let request = PreparedGeneration::local_fixture_from_frozen(
        frozen,
        &service.proxy_snapshot,
        auth,
        input,
    )?
    .with_calibration(
        "local-fixture",
        &aoidos_engine::record::estimator::EstimatorRevision::ConservativeV2,
        service.diagnostics.clone(),
        false,
    );
    let turn_id = service
        .coordinator
        .submit(request, Arc::new(MemoryWriter))?;
    Ok(AcceptedTurn { turn_id })
}
#[cfg(test)]
mod tests;
