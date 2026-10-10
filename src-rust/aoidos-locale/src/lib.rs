//! 界面语言选择与系统语言解析；持久化借用应用共享 SQLite 连接。

pub mod preference;

pub mod schema {
    pub const VERSION: u32 = 1;
    pub const SCHEMA: &str = "CREATE TABLE locale_preference (id INTEGER PRIMARY KEY CHECK (id = 1), version INTEGER NOT NULL CHECK (version = 1), locale TEXT NOT NULL CHECK (locale IN ('system', 'zh-Hans', 'en'))) STRICT;";

    /// 检查语言偏好表与冻结 schema 相符。
    ///
    /// # Errors
    /// 表结构不匹配时返回 store.corrupt。
    pub fn verify(
        connection: &rusqlite::Connection,
    ) -> Result<(), aoidos_store::error::StoreError> {
        let valid: i64 = connection
            .query_row(
                "SELECT count(*)=3 AND sum(name='id' AND type='INTEGER' AND pk=1)=1 AND sum(name='version' AND type='INTEGER' AND \"notnull\"=1)=1 AND sum(name='locale' AND type='TEXT' AND \"notnull\"=1)=1 FROM pragma_table_info('locale_preference')",
                [],
                |row| row.get(0),
            )
            .map_err(aoidos_store::db::sqlite_error)?;
        if valid != 1 {
            return Err(aoidos_store::error::StoreError::Corrupt(
                "locale preference schema mismatch".into(),
            ));
        }
        Ok(())
    }
}
