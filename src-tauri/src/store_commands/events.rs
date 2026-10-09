//! 迁移回调只尝试入队；数据库不等待窗口，快照保留已预留基线。

use crate::{events, turn_commands::WindowEvents};
use aoidos_engine::{
    fault::Fault,
    migration::{MigrationEvent, MigrationEvents},
    record::session::{Appended, RecordEvents},
};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, mpsc};

struct Queued {
    name: &'static str,
    id: String,
    payload: serde_json::Value,
}
pub struct StorageEvents {
    sender: Mutex<Option<mpsc::Sender<Queued>>>,
    diagnostic: Mutex<(Option<String>, bool)>,
    window: Arc<WindowEvents>,
    finished: Notify,
    stopped: std::sync::atomic::AtomicBool,
}
impl StorageEvents {
    pub fn new(window: Arc<WindowEvents>) -> Arc<Self> {
        let (sender, mut receiver) = mpsc::channel::<Queued>(32);
        let port = Arc::new(Self {
            sender: Mutex::new(Some(sender)),
            diagnostic: Mutex::new((None, false)),
            window,
            finished: Notify::new(),
            stopped: std::sync::atomic::AtomicBool::new(false),
        });
        let worker = port.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = receiver.recv().await {
                if worker.window.dispatch(event.name, event.payload).is_err() {
                    worker.mark(&event.id);
                }
            }
            worker
                .stopped
                .store(true, std::sync::atomic::Ordering::Release);
            worker.finished.notify_waiters();
        });
        port
    }
    fn mark(&self, id: &str) {
        let mut diagnostic = self
            .diagnostic
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if diagnostic.0.as_deref() == Some(id) {
            diagnostic.1 = true;
        }
    }
    fn send(&self, name: &'static str, id: &str, payload: serde_json::Value) -> Result<(), Fault> {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = sender.as_ref().ok_or_else(unavailable)?.try_send(Queued {
            name,
            id: id.into(),
            payload,
        });
        if result.is_err() {
            self.mark(id);
            return Err(unavailable());
        }
        Ok(())
    }
    pub fn failed(&self, id: &str) -> bool {
        let diagnostic = self
            .diagnostic
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        diagnostic.0.as_deref() == Some(id) && diagnostic.1
    }
    /// 生产者退出后关闭队列并等待尾部投递，随后可释放窗口句柄。
    pub async fn shutdown(&self) {
        self.sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        loop {
            let notified = self.finished.notified();
            if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    }
}
fn unavailable() -> Fault {
    Fault::new("app.event-failed", "存储事件队列不可用，可主动恢复快照")
}
fn prepare_error(_: crate::ipc::CmdError) -> Fault {
    unavailable()
}
impl RecordEvents for StorageEvents {
    fn retire(&self, id: &str) {
        events::retire_stream(id);
    }
    fn prepare(&self, event: &Appended) -> Result<u64, Fault> {
        Ok(
            events::prepare("engine:record:appended", &event.session_id, event)
                .map_err(prepare_error)?
                .seq,
        )
    }
    fn deliver(&self, seq: u64, event: Appended) -> Result<(), Fault> {
        self.send(
            "engine:record:appended",
            &event.session_id,
            serde_json::json!({"seq":seq,"data":event}),
        )
    }
}
impl MigrationEvents for StorageEvents {
    fn delivery_error(&self, id: &str) -> Option<Fault> {
        self.failed(id).then(unavailable)
    }
    fn prepare(&self, event: &MigrationEvent) -> Result<u64, Fault> {
        {
            let mut diagnostic = self
                .diagnostic
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if diagnostic.0.as_deref() != Some(event.id()) {
                *diagnostic = (Some(event.id().into()), false);
            }
        }
        Ok(events::prepare(event.name(), event.id(), event)
            .map_err(prepare_error)?
            .seq)
    }
    fn deliver(&self, seq: u64, event: MigrationEvent) -> Result<(), Fault> {
        self.send(
            event.name(),
            event.id(),
            serde_json::json!({"seq":seq,"data":event}),
        )
    }
    fn retire(&self, id: &str) {
        events::retire_stream(id);
        let mut diagnostic = self
            .diagnostic
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if diagnostic.0.as_deref() == Some(id) {
            *diagnostic = (None, false);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn unique() -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!(
            "storage-events-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }
    fn event(id: &str) -> MigrationEvent {
        MigrationEvent::Done {
            migration_id: id.into(),
            from: 0,
            to: 1,
            current: 1,
        }
    }
    #[tokio::test]
    async fn full_closed_and_late_delivery_keep_only_current_migration_diagnostic() {
        let window = Arc::new(WindowEvents::new(|_, _| {
            Err(Fault::new("app.event-failed", "合成窗口失败"))
        }));
        let (sender, receiver) = mpsc::channel(32);
        let port = StorageEvents {
            sender: Mutex::new(Some(sender)),
            diagnostic: Mutex::new((None, false)),
            window: window.clone(),
            finished: Notify::new(),
            stopped: std::sync::atomic::AtomicBool::new(true),
        };
        let id = unique();
        let value = event(&id);
        assert_eq!(MigrationEvents::prepare(&port, &value).unwrap(), 1);
        for seq in 1..=32 {
            MigrationEvents::deliver(&port, seq, event(&id)).unwrap();
        }
        assert!(MigrationEvents::deliver(&port, 33, event(&id)).is_err());
        assert!(port.delivery_error(&id).is_some());
        MigrationEvents::retire(&port, "other");
        assert!(port.failed(&id));
        MigrationEvents::retire(&port, &id);
        assert!(!port.failed(&id));
        let new = unique();
        MigrationEvents::prepare(&port, &event(&new)).unwrap();
        port.mark(&id);
        assert!(!port.failed(&new));
        drop(receiver);
        assert!(MigrationEvents::deliver(&port, 1, event(&new)).is_err());
        port.shutdown().await;
        assert!(MigrationEvents::deliver(&port, 2, event(&new)).is_err());
        let record = Appended {
            view_epoch: "epoch".into(),
            session_id: unique(),
            record_seq: 1,
            kind: "narration".into(),
            turn_id: None,
        };
        let seq = RecordEvents::prepare(&port, &record).unwrap();
        assert_eq!(seq, 1);
        assert!(RecordEvents::deliver(&port, seq, record.clone()).is_err());
        RecordEvents::retire(&port, &record.session_id);
        crate::events::force_sequence("store:migration:done", &new, (1_u64 << 53) - 1);
        assert!(MigrationEvents::prepare(&port, &event(&new)).is_err());
        MigrationEvents::retire(&port, &new);
        let live = StorageEvents::new(window);
        let id = unique();
        let seq = MigrationEvents::prepare(live.as_ref(), &event(&id)).unwrap();
        MigrationEvents::deliver(live.as_ref(), seq, event(&id)).unwrap();
        live.shutdown().await;
        assert!(live.failed(&id));
        MigrationEvents::retire(live.as_ref(), &id);
    }
}
