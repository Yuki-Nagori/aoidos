//! 四个阶段流复用窗口生命周期；快照确认与窗口投递仍由 engine 分开执行。

use crate::{events, turn_commands::WindowEvents};
use mythos_engine::{
    fault::Fault,
    game::state::{PhaseEvent, PhaseEvents},
};

impl PhaseEvents for WindowEvents {
    fn prepare(&self, event: &PhaseEvent) -> Result<u64, Fault> {
        events::prepare(event.name(), &event.state().session_id, event)
            .map(|prepared| prepared.seq)
            .map_err(prepare_error)
    }
    fn deliver(&self, seq: u64, event: PhaseEvent) -> Result<(), Fault> {
        self.dispatch(
            event.name(),
            serde_json::json!({ "seq": seq, "data": event }),
        )
    }
    fn retire(&self, id: &str) {
        events::retire_stream(id);
    }
}
pub(super) fn prepare_error(_: crate::ipc::CmdError) -> Fault {
    Fault::new("app.event-failed", "阶段事件无法准备")
}
