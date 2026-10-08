//! SQLite 幂等提交原语；条件 / 领域 SQL 由调用方提供，标记与更新同事务。

use crate::error::{Result, StoreError};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

/// 基建标记表由共享迁移列表恰一次创建，不含世界属性 schema。
pub const SCHEMA: &str =
    "CREATE TABLE store_applied (id TEXT PRIMARY KEY, content_hash TEXT NOT NULL);";
/// 核验已应用内容；同 id 不同 hash 属于损坏，不能当成可重试操作。
/// # Errors
/// SQL 读取失败或内容身份冲突返回存储错误。
pub fn contains(conn: &Connection, id: &str, hash: &str) -> Result<bool> {
    let previous: Option<String> = conn
        .query_row(
            "SELECT content_hash FROM store_applied WHERE id=?1",
            [id],
            read_hash,
        )
        .optional()
        .map_err(sql_error)?;
    match previous {
        None => Ok(false),
        Some(previous) if previous == hash => Ok(true),
        Some(_) => Err(StoreError::Corrupt(
            "applied mutation content mismatch".into(),
        )),
    }
}
fn read_hash(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get(0)
}
/// SQL 更新和 applied 标记恰一次提交，事务失败不删除上游持久意图。
/// # Errors
/// 条件不吻合、SQL / commit 失败返回存储错误；调用方须保留 pending 并重开核验。
pub fn apply_once(
    conn: &mut Connection,
    id: &str,
    hash: &str,
    apply: &mut dyn FnMut(&Transaction<'_>) -> Result<()>,
) -> Result<bool> {
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    if contains(&transaction, id, hash)? {
        return Ok(false);
    }
    apply(&transaction)?;
    transaction
        .execute(
            "INSERT INTO store_applied(id,content_hash) VALUES (?1,?2)",
            rusqlite::params![id, hash],
        )
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)?;
    Ok(true)
}
fn sql_error(error: rusqlite::Error) -> StoreError {
    crate::db::sqlite_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changes_and_marker_commit_together_with_conflicts_and_sql_failures() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(
            "CREATE TABLE fixture(value INTEGER NOT NULL); INSERT INTO fixture VALUES(0);",
        )
        .unwrap();
        assert!(
            apply_once(&mut conn, "m1", "hash-a", &mut |tx| {
                tx.execute("UPDATE fixture SET value=value+1", [])
                    .map_err(sql_error)?;
                Ok(())
            })
            .unwrap()
        );
        fn increment(tx: &Transaction<'_>) -> Result<()> {
            tx.execute("UPDATE fixture SET value=value+1", [])
                .map(drop)
                .map_err(sql_error)
        }
        assert!(apply_once(&mut conn, "m-extra", "hash-extra", &mut increment).unwrap());
        assert!(!apply_once(&mut conn, "m-extra", "hash-extra", &mut increment).unwrap());
        assert!(contains(&conn, "m1", "hash-b").is_err());
        assert!(
            apply_once(&mut conn, "m2", "hash-c", &mut |tx| {
                tx.execute("UPDATE fixture SET value=100", [])
                    .map_err(sql_error)?;
                Err(StoreError::Corrupt("condition conflict".into()))
            })
            .is_err()
        );
        assert!(!contains(&conn, "m2", "hash-c").unwrap());
        assert!(
            apply_once(&mut conn, "m3", "hash-d", &mut |tx| {
                tx.execute("INVALID SQL", []).map(drop).map_err(sql_error)
            })
            .is_err()
        );
        let value: i64 = conn
            .query_row("SELECT value FROM fixture", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 2);
    }
}
