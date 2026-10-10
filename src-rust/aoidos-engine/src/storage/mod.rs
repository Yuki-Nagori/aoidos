//! 共享业务存储所有者；壳只注入根目录和事件端口，不持有领域连接。

use crate::{
    fault::Fault,
    migration::{Migration, MigrationEvents},
    preferences::Preferences,
    record::session::RecordEvents,
    records::Records,
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

pub struct Storage {
    pub records: Arc<Records>,
    pub migration: Arc<Migration>,
    pub preferences: Arc<Preferences>,
    database: Mutex<Option<rusqlite::Connection>>,
    memory: aoidos_memory::repository::Repository,
}
impl Storage {
    /// 共享迁移编号只有一个来源；失败保留诊断但拒绝业务入口。
    pub fn new(
        root: &Path,
        migration_events: Arc<dyn MigrationEvents>,
        record_events: Arc<dyn RecordEvents>,
    ) -> Self {
        let migration = Arc::new(Migration::new(migration_events));
        let database = migration
            .open_verified(
                &root.join("storage.sqlite"),
                &[
                    aoidos_store::applied::SCHEMA,
                    aoidos_memory::schema::SCHEMA,
                    aoidos_theme::schema::SCHEMA,
                    aoidos_locale::schema::SCHEMA,
                    aoidos_billing::schema::SCHEMA,
                ],
                |connection| {
                    let valid:i64=connection.query_row("SELECT count(*)=2 AND sum(name='id' AND type='TEXT' AND pk=1)=1 AND sum(name='content_hash' AND type='TEXT' AND \"notnull\"=1)=1 FROM pragma_table_info('store_applied')",[],|row|row.get(0)).map_err(aoidos_store::db::sqlite_error)?;
                    if valid!=1 {return Err(aoidos_store::error::StoreError::Corrupt("business applied schema mismatch".into()));}
                    aoidos_memory::schema::verify(connection)?;
                    aoidos_theme::schema::verify(connection)?;
                    aoidos_locale::schema::verify(connection)?;
                    aoidos_billing::schema::verify(connection)
                        .map_err(aoidos_store::db::sqlite_error)?;
                    Ok(())
                },
            )
            .ok();
        Self {
            records: Arc::new(Records::new(root.to_owned(), record_events)),
            migration,
            preferences: Arc::new(Preferences::new(root)),
            database: Mutex::new(database),
            memory: aoidos_memory::repository::Repository::new(root),
        }
    }
    /// 在会话锁 → 数据库锁内借用记忆仓储；回调不能递归取得这两个锁或 await。
    /// # Errors
    /// 未登记会话、pending 世界 / 回退、策略不可用或持久化失败拒绝。
    pub fn with_memory<T>(
        &self,
        session_id: &str,
        job: impl FnOnce(
            &aoidos_memory::repository::Repository,
            &mut rusqlite::Connection,
            &crate::record::memory::SourcePort<'_>,
        ) -> aoidos_memory::error::Result<T>,
    ) -> aoidos_memory::error::Result<T> {
        let session = self.records.get(session_id).map_err(memory_context_error)?;
        let session = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let port = crate::record::memory::SourcePort::new(&session);
        self.with_database(|conn| Ok(job(&self.memory, conn, &port)))
            .map_err(memory_context_error)?
    }
    /// 回合生产者退出后释放 SQLite 和记录句柄，偏好已在写入时确认。
    /// # Errors
    /// 记录关闭同步失败返回 store.*；不删除存档。
    pub fn close(&self) -> Result<(), Fault> {
        self.database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.records.close_all()
    }
    /// # Errors
    /// 初始化 / 迁移失败的业务服务返回 not-ready；诊断查询不经过此门禁。
    pub fn ready(&self) -> Result<(), Fault> {
        self.with_database(|_| Ok(()))
    }
    /// 登记解释器借用唯一连接；领域条件、投影与 applied 标记在回调内同事务提交。
    /// 回调不递归获取本存储锁；阻塞调用由引擎 blocking 边界调度，不在持锁期间 await。
    /// # Errors
    /// 未开放 / 已关闭为 not-ready；原领域错误原样返回，不另建连接或迁移生命周期。
    pub fn with_database<T>(
        &self,
        job: impl FnOnce(&mut rusqlite::Connection) -> Result<T, Fault>,
    ) -> Result<T, Fault> {
        let mut database = self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let connection = database.as_mut().ok_or_else(not_ready)?;
        job(connection)
    }
}
fn not_ready() -> Fault {
    Fault::new("app.not-ready", "业务存储未开放，请查看迁移诊断")
}

fn memory_context_error(error: Fault) -> aoidos_memory::error::Error {
    use aoidos_memory::error::{Error, Reason};
    match error.code.as_str() {
        "app.not-found" => Error::NotFound,
        "app.bad-request" => Error::Rejected(Reason::InvalidSchema),
        _ => Error::Rejected(Reason::RecoveryRequired),
    }
}

#[cfg(test)]
mod tests;
