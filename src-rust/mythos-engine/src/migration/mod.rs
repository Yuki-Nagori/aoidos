//! SQLite 打开流的真实状态；已提交步骤和投递失败分别确认。

use crate::fault::Fault;
use mythos_store::{db, error::StoreError};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Running,
    Completed,
    Failed,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Step {
    pub from: u32,
    pub to: u32,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sequences {
    pub progress: u64,
    pub done: u64,
    pub failed: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub phase: Phase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,
    pub to: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_step: Option<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<Step>,
    pub seq: Sequences,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Fault>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_error: Option<Fault>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MigrationEvent {
    Progress {
        #[serde(rename = "migrationId")]
        migration_id: String,
        from: u32,
        to: u32,
        current: u32,
        target: u32,
    },
    Done {
        #[serde(rename = "migrationId")]
        migration_id: String,
        from: u32,
        to: u32,
        current: u32,
    },
    Failed {
        #[serde(rename = "migrationId")]
        migration_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<u32>,
        to: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        current: Option<u32>,
        #[serde(rename = "failedStep", skip_serializing_if = "Option::is_none")]
        failed_step: Option<Step>,
        code: String,
        message: String,
    },
}
impl MigrationEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Progress { .. } => "store:migration:progress",
            Self::Done { .. } => "store:migration:done",
            Self::Failed { .. } => "store:migration:failed",
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Progress { migration_id, .. }
            | Self::Done { migration_id, .. }
            | Self::Failed { migration_id, .. } => migration_id,
        }
    }
}
pub trait MigrationEvents: Send + Sync {
    /// # Errors
    /// 载荷 / 序号准备失败只更新 deliveryError，不更改 SQL 结果。
    fn prepare(&self, event: &MigrationEvent) -> Result<u64, Fault>;
    /// 必须非阻塞入队，不在 SQLite 回调内等窗口。
    /// # Errors
    /// 队列满或关闭返回 app.event-failed，快照仍保持已确认版本。
    fn deliver(&self, seq: u64, event: MigrationEvent) -> Result<(), Fault>;
    /// producer 退出后允许清退之前终态的序号。
    fn retire(&self, id: &str);
    fn delivery_error(&self, _id: &str) -> Option<Fault> {
        None
    }
}
pub struct Migration {
    snapshot: Arc<Mutex<Snapshot>>,
    events: Arc<dyn MigrationEvents>,
    gate: Mutex<()>,
}
impl Migration {
    /// idle 查询不打开 / 创建数据库。
    /// # Errors
    /// 静态版本读取失败返回存储错误。
    pub fn idle(path: &Path, events: Arc<dyn MigrationEvents>) -> Result<Self, StoreError> {
        let version = db::current_version(path)?;
        Ok(Self {
            snapshot: Arc::new(Mutex::new(Snapshot {
                phase: Phase::Idle,
                migration_id: None,
                from: Some(version),
                to: version,
                current: Some(version),
                last_step: None,
                failed_step: None,
                seq: Sequences::default(),
                error: None,
                delivery_error: None,
            })),
            events,
            gate: Mutex::new(()),
        })
    }
    /// setup 可在初始版本未知时构造诊断状态，打开失败不会使其不可查询。
    pub fn new(events: Arc<dyn MigrationEvents>) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(Snapshot {
                phase: Phase::Idle,
                migration_id: None,
                from: None,
                to: 0,
                current: None,
                last_step: None,
                failed_step: None,
                seq: Sequences::default(),
                error: None,
                delivery_error: None,
            })),
            events,
            gate: Mutex::new(()),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(error) = snapshot
            .migration_id
            .as_ref()
            .and_then(|id| self.events.delivery_error(id))
        {
            snapshot.delivery_error = Some(error);
        }
        snapshot
    }
    /// 一次打开形成唯一 completed / failed；无待迁移同样发布 done。
    /// # Errors
    /// 真正 SQL / 文件失败返回 store.*；平台投递失败不撤销已提交迁移。
    pub fn open(&self, path: &Path, migrations: &[&str]) -> Result<rusqlite::Connection, Fault> {
        self.open_verified(path, migrations, |_| Ok(()))
    }
    /// 共享库结构核验属于打开流程，核验失败不得发布 completed。
    /// # Errors
    /// 迁移或结构核验失败保留 failed 快照，并拒绝业务入口。
    pub fn open_verified(
        &self,
        path: &Path,
        migrations: &[&str],
        validate: impl FnOnce(&rusqlite::Connection) -> Result<(), StoreError>,
    ) -> Result<rusqlite::Connection, Fault> {
        let _guard = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = self.snapshot().migration_id;
        if let Some(previous) = previous {
            self.events.retire(&previous);
        }
        let id = uuid::Uuid::new_v4().to_string();
        *self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Snapshot {
            phase: Phase::Running,
            migration_id: Some(id.clone()),
            from: None,
            to: migrations.len() as u32,
            current: None,
            last_step: None,
            failed_step: None,
            seq: Sequences::default(),
            error: None,
            delivery_error: None,
        };
        let version = match db::current_version(path) {
            Ok(version) => version,
            Err(error) => return Err(self.fail(error)),
        };
        {
            let mut state = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.from = Some(version);
            state.current = Some(version);
        }
        let result = db::open_with_progress(path, migrations, &mut |step| {
            {
                let mut state = self
                    .snapshot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.current = Some(step.to);
                state.last_step = Some(Step {
                    from: step.from,
                    to: step.to,
                });
            }
            self.publish(MigrationEvent::Progress {
                migration_id: id.clone(),
                from: step.from,
                to: step.to,
                current: step.to,
                target: migrations.len() as u32,
            });
        });
        match result {
            Ok(connection) => {
                if let Err(error) = validate(&connection) {
                    return Err(self.fail(error));
                }
                self.snapshot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .phase = Phase::Completed;
                self.publish(MigrationEvent::Done {
                    migration_id: id,
                    from: version,
                    to: migrations.len() as u32,
                    current: migrations.len() as u32,
                });
                Ok(connection)
            }
            Err(error) => Err(self.fail(error)),
        }
    }
    fn fail(&self, error: StoreError) -> Fault {
        let failure = Fault::new(format!("store.{}", error.code()), "存储初始化或迁移失败");
        let step = match error {
            StoreError::Migration { version, .. } => Some(Step {
                from: version - 1,
                to: version,
            }),
            _ => None,
        };
        {
            let mut state = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.phase = Phase::Failed;
            state.error = Some(failure.clone());
            state.failed_step = step;
        }
        let state = self.snapshot();
        self.publish(MigrationEvent::Failed {
            migration_id: state.migration_id.expect("running flow owns UUID"),
            from: state.from,
            to: state.to,
            current: state.current,
            failed_step: step,
            code: failure.code.clone(),
            message: failure.message.clone(),
        });
        failure
    }
    fn publish(&self, event: MigrationEvent) {
        let prepared = self.events.prepare(&event);
        match prepared {
            Ok(seq) if seq > 0 && seq <= crate::record::format::MAX_SEQ => {
                let previous = self.snapshot();
                let last = match &event {
                    MigrationEvent::Progress { .. } => previous.seq.progress,
                    MigrationEvent::Done { .. } => previous.seq.done,
                    MigrationEvent::Failed { .. } => previous.seq.failed,
                };
                if seq <= last {
                    self.delivery_error();
                    return;
                }
                {
                    let mut state = self
                        .snapshot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match &event {
                        MigrationEvent::Progress { .. } => state.seq.progress = seq,
                        MigrationEvent::Done { .. } => state.seq.done = seq,
                        MigrationEvent::Failed { .. } => state.seq.failed = seq,
                    }
                }
                if self.events.deliver(seq, event).is_err() {
                    self.delivery_error();
                }
            }
            _ => self.delivery_error(),
        }
    }
    fn delivery_error(&self) {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .delivery_error = Some(Fault::new(
            "app.event-failed",
            "迁移事件未能投递，可主动恢复快照",
        ));
    }
}

#[cfg(test)]
mod tests;
