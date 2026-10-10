//! 受限主题与剧本皮肤能力；不依赖 Tauri、Vue 或业务引擎。

pub mod catalog;
pub mod preference;
pub mod skin;

pub mod schema {
    pub const VERSION: u32 = 3;
    pub const SCHEMA: &str = "CREATE TABLE theme_preference (id INTEGER PRIMARY KEY CHECK (id = 1), version INTEGER NOT NULL CHECK (version = 1), theme TEXT NOT NULL CHECK (length(theme) BETWEEN 1 AND 64)) STRICT;";

    /// 检查共享业务库中的主题偏好表结构。
    ///
    /// # Errors
    /// 表结构与 026 冻结的单行 schema 不一致时返回存储错误。
    pub fn verify(
        connection: &rusqlite::Connection,
    ) -> Result<(), aoidos_store::error::StoreError> {
        let valid: i64 = connection
            .query_row(
                "SELECT count(*)=3 AND sum(name='id' AND type='INTEGER' AND pk=1)=1 AND sum(name='version' AND type='INTEGER' AND \"notnull\"=1)=1 AND sum(name='theme' AND type='TEXT' AND \"notnull\"=1)=1 FROM pragma_table_info('theme_preference')",
                [],
                |row| row.get(0),
            )
            .map_err(aoidos_store::db::sqlite_error)?;
        if valid != 1 {
            return Err(aoidos_store::error::StoreError::Corrupt(
                "theme preference schema mismatch".into(),
            ));
        }
        Ok(())
    }
}
