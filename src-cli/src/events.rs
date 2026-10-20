//! CLI 事件出口使用有界队列；终端慢时由领域快照恢复，不阻塞存储或 Provider。

use aoidos_engine::{
    fault::Fault,
    game::state::{PhaseEvent, PhaseEvents},
    migration::{MigrationEvent, MigrationEvents},
    ports::{EventPort, PreparedEvent, TurnEvent},
    record::session::{Appended, RecordEvents},
};
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::mpsc;

pub const EVENT_CAPACITY: usize = 256;

#[derive(Debug)]
pub enum RuntimeEvent {
    Turn(TurnEvent),
    Phase(Box<PhaseEvent>),
    Record(Appended),
    Migration(MigrationEvent),
}

/// 在事件序号预留与 CLI 消费之间提供有界、非阻塞的进程内桥接。
pub struct EventBridge {
    sender: mpsc::Sender<RuntimeEvent>,
    sequences: Mutex<BTreeMap<(String, String), u64>>,
    overflowed: AtomicBool,
}

impl EventBridge {
    #[must_use]
    pub fn new(sender: mpsc::Sender<RuntimeEvent>) -> Self {
        Self {
            sender,
            sequences: Mutex::new(BTreeMap::new()),
            overflowed: AtomicBool::new(false),
        }
    }

    pub fn take_overflowed(&self) -> bool {
        self.overflowed.swap(false, Ordering::AcqRel)
    }

    fn reserve(&self, name: &str, id: &str) -> Result<u64, Fault> {
        let mut sequences = self
            .sequences
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sequence = sequences.entry((name.into(), id.into())).or_default();
        *sequence = sequence.checked_add(1).ok_or_else(event_failed)?;
        Ok(*sequence)
    }

    fn retire_id(&self, id: &str) {
        self.sequences
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(_, stream_id), _| stream_id != id);
    }

    fn send(&self, event: RuntimeEvent) -> Result<(), Fault> {
        match self.sender.try_send(event) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.overflowed.store(true, Ordering::Release);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(event_failed()),
        }
    }
}

impl EventPort for EventBridge {
    fn prepare(&self, event: TurnEvent) -> Result<PreparedEvent, Fault> {
        let seq = self.reserve(event.name(), event.turn_id())?;
        let data = serde_json::to_value(&event).map_err(|_| event_failed())?;
        Ok(PreparedEvent {
            seq,
            event,
            envelope: serde_json::json!({ "seq": seq, "data": data }),
        })
    }

    fn deliver(&self, event: PreparedEvent) -> Result<(), Fault> {
        self.send(RuntimeEvent::Turn(event.event))
    }

    fn retire(&self, turn_id: &str) {
        self.retire_id(turn_id);
    }
}

impl PhaseEvents for EventBridge {
    fn prepare(&self, event: &PhaseEvent) -> Result<u64, Fault> {
        self.reserve(event.name(), &event.state().session_id)
    }

    fn deliver(&self, _seq: u64, event: PhaseEvent) -> Result<(), Fault> {
        self.send(RuntimeEvent::Phase(Box::new(event)))
    }

    fn retire(&self, session_id: &str) {
        self.retire_id(session_id);
    }
}

impl RecordEvents for EventBridge {
    fn prepare(&self, event: &Appended) -> Result<u64, Fault> {
        self.reserve("engine:record:appended", &event.session_id)
    }

    fn deliver(&self, _seq: u64, event: Appended) -> Result<(), Fault> {
        self.send(RuntimeEvent::Record(event))
    }

    fn retire(&self, session_id: &str) {
        self.retire_id(session_id);
    }
}

impl MigrationEvents for EventBridge {
    fn prepare(&self, event: &MigrationEvent) -> Result<u64, Fault> {
        self.reserve(event.name(), event.id())
    }

    fn deliver(&self, _seq: u64, event: MigrationEvent) -> Result<(), Fault> {
        self.send(RuntimeEvent::Migration(event))
    }

    fn retire(&self, id: &str) {
        self.retire_id(id);
    }
}

fn event_failed() -> Fault {
    Fault::new("app.event-failed", "终端事件队列不可用，可重新读取当前状态")
}

#[cfg(test)]
mod tests {
    use super::{EventBridge, RuntimeEvent};
    use aoidos_engine::ports::TurnEvent;
    use tokio::sync::mpsc;

    #[test]
    fn full_event_queue_drops_notification_and_requests_snapshot_recovery() {
        let (sender, mut receiver) = mpsc::channel(1);
        let bridge = EventBridge::new(sender);

        bridge
            .send(RuntimeEvent::Turn(TurnEvent::Chunk {
                turn_id: "turn-1".into(),
                delta: "first".into(),
            }))
            .unwrap();
        bridge
            .send(RuntimeEvent::Turn(TurnEvent::Chunk {
                turn_id: "turn-1".into(),
                delta: "dropped".into(),
            }))
            .unwrap();

        assert!(matches!(receiver.try_recv(), Ok(RuntimeEvent::Turn(_))));
        assert!(bridge.take_overflowed());
        assert!(!bridge.take_overflowed());
    }

    #[test]
    fn closed_event_queue_reports_unavailable_adapter() {
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let bridge = EventBridge::new(sender);

        assert!(
            bridge
                .send(RuntimeEvent::Turn(TurnEvent::Chunk {
                    turn_id: "turn-1".into(),
                    delta: "text".into(),
                }))
                .is_err()
        );
        assert!(!bridge.take_overflowed());
    }
}
