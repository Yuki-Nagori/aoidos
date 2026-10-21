//! 桌面与 CLI 共用的进程内应用服务；入口适配器负责事件投递与用户交互。

pub mod billing;
pub mod profiles;
pub mod runs;

use aoidos_engine::{
    fault::Fault,
    game::{
        product::{Product, ProfileFactory, ProfileSource},
        runtime::Service,
        state::PhaseEvents,
    },
    ports::EventPort,
    record::calibration::Calibration,
    storage::Storage,
    turn::Coordinator,
};
use aoidos_llm::proxy::SystemProxySnapshot;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// 持有一组对局共享的协调器、诊断状态与启动时代理快照。
///
/// GUI 与 CLI 各自构造本服务，但使用同一数据根和领域实现；事件出口由各入口注入。
pub struct TurnRuntime {
    pub coordinator: Coordinator,
    pub diagnostics: Arc<Mutex<Calibration>>,
    pub proxy_snapshot: SystemProxySnapshot,
}

impl TurnRuntime {
    /// 创建单进程回合运行服务。
    ///
    /// # Errors
    /// engine 协调器容量参数不合法时返回脱敏 Fault。
    pub fn new(
        events: Arc<dyn EventPort>,
        proxy_snapshot: SystemProxySnapshot,
    ) -> Result<Self, Fault> {
        Ok(Self {
            coordinator: Coordinator::new(events, 16)?,
            diagnostics: Arc::new(Mutex::new(Calibration::default())),
            proxy_snapshot,
        })
    }
}

/// 共享游戏应用的完成装配；入口只负责把平台事件和可信 profile 来源注入进来。
pub struct GameServices {
    pub turns: TurnRuntime,
    pub product: Product,
    pub runtime: Service,
    pub factory: Arc<ProfileFactory>,
    pub billing: billing::BillingService,
    pub runs: runs::RunLifecycleService,
}

impl GameServices {
    /// 组合存储、产品入口和回合运行时，保证它们共用同一协调器与 profile factory。
    ///
    /// # Errors
    /// engine 回合协调器参数不合法时返回脱敏 Fault。
    pub fn new(
        root: &Path,
        storage: Arc<Storage>,
        turn_events: Arc<dyn EventPort>,
        phase_events: Arc<dyn PhaseEvents>,
        profiles: Arc<dyn ProfileSource>,
        proxy_snapshot: SystemProxySnapshot,
    ) -> Result<Self, Fault> {
        let turns = TurnRuntime::new(turn_events, proxy_snapshot)?;
        let factory = ProfileFactory::new(
            profiles,
            storage.clone(),
            turns.proxy_snapshot.clone(),
            turns.diagnostics.clone(),
        );
        let product = Product::new(
            root,
            storage.clone(),
            turns.coordinator.clone(),
            phase_events,
            factory.clone(),
        );
        let runtime = Service::new(turns.coordinator.clone(), factory.clone());
        let billing = billing::BillingService::new(storage.clone());
        let runs = runs::RunLifecycleService::new(storage, factory.clone());

        Ok(Self {
            turns,
            product,
            runtime,
            factory,
            billing,
            runs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoidos_engine::{ports::PreparedEvent, ports::TurnEvent};

    struct SilentEvents;

    impl EventPort for SilentEvents {
        fn prepare(&self, event: TurnEvent) -> Result<PreparedEvent, Fault> {
            let _ = event;
            unreachable!("构造运行服务不会投递事件")
        }

        fn deliver(&self, event: PreparedEvent) -> Result<(), Fault> {
            let _ = event;
            unreachable!("构造运行服务不会投递事件")
        }

        fn retire(&self, turn_id: &str) {
            let _ = turn_id;
        }
    }

    #[test]
    fn runtime_owns_shared_turn_state_and_accepts_entry_event_port() {
        let runtime =
            TurnRuntime::new(Arc::new(SilentEvents), SystemProxySnapshot::capture()).unwrap();

        assert_eq!(
            runtime.coordinator.snapshot("missing").unwrap_err().code,
            "app.not-found"
        );
        assert_eq!(Arc::strong_count(&runtime.diagnostics), 1);
    }
}
