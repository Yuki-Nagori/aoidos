//! SQLite 打开与顺序迁移：`PRAGMA user_version` + 事务包裹，失败回滚拒启；
//! pending 迁移前经 SQLite backup API 做一致快照到同级 `backups/`（保留 3 份）。
//! 热 WAL 连接上直接拷主文件会丢掉尚未 checkpoint 的提交。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, DatabaseName, TransactionBehavior};

use super::atomic;
use super::error::{Result, StoreError};
use super::paths;

const BACKUPS_TO_KEEP: usize = 3;
// 迁移备份或尚未退出的读方会短暂占库。等待 5 秒，而不是把启动立刻报成失败（task 013）。
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 打开（必要时创建）数据库并推进迁移。`migrations[v]` 把 user_version 从 v
/// 推到 v+1；任一迁移失败即回滚并上抛，数据库保持在旧版本。
///
/// 比当前二进制新的库在改日志模式之前就拒绝，避免把只读打开写成 WAL。
/// 调用方先持有 [`super::lock::InstanceLock`]；同一目录只放一个业务库，备份共用 `backups/`。
/// 迁移 SQL 必须是可信的内嵌语句，不含 BEGIN / COMMIT / ROLLBACK；事务边界由 runner 管理。
///
/// # Errors
///
/// 路径无法创建、文件不是可读的 SQLite 库、迁移失败，或库版本高于二进制时返回。
pub fn open(path: &Path, migrations: &[&str]) -> Result<Connection> {
    // 入参可能带混合分隔符（剧本 / 配置拼接），先经路径层归一化再交给 SQLite。
    let path = paths::normalize(path);
    make_parent_dirs(&path)?;
    let mut conn = Connection::open(&path).map_err(err_open)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(err_pragma)?;
    let mut version = query_user_version(&conn)?;
    let target = migrations.len() as u32;
    if version > target {
        return Err(newer_than_binary(version, target));
    }
    enable_wal(&conn)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(err_pragma)?;
    while version < target {
        if version > 0 {
            backup(&conn, &path, version)?;
        }
        apply_migration(&mut conn, version, migrations[version as usize])?;
        version += 1;
    }
    Ok(conn)
}

fn newer_than_binary(version: u32, target: u32) -> StoreError {
    StoreError::Corrupt(format!(
        "database user_version {version} is newer than binary target {target}"
    ))
}

fn apply_migration(conn: &mut Connection, version: u32, sql: &str) -> Result<()> {
    // RAII 在 SQL、版本更新或 COMMIT 失败时回滚，调用方不会拿到仍在事务中的连接。
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err_begin)?;
    if let Err(source) = tx.execute_batch(sql) {
        return Err(StoreError::Migration {
            version: version + 1,
            source,
        });
    }
    tx.pragma_update(None, "user_version", version + 1)
        .map_err(err_commit)?;
    tx.commit().map_err(err_commit)
}

fn query_user_version(conn: &Connection) -> Result<u32> {
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(err_user_version)?;
    match u32::try_from(v) {
        Ok(version) => Ok(version),
        Err(_) => Err(StoreError::Corrupt(format!("negative user_version {v}"))),
    }
}

fn enable_wal(conn: &Connection) -> Result<()> {
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(err_pragma)?;
    require_wal(&mode)
}

fn require_wal(mode: &str) -> Result<()> {
    if mode.eq_ignore_ascii_case("wal") {
        Ok(())
    } else {
        Err(StoreError::Io {
            code: "io",
            source: io::Error::other(format!("journal_mode is {mode}, want wal")),
        })
    }
}

fn backup(conn: &Connection, db_path: &Path, from_version: u32) -> Result<PathBuf> {
    let backups_dir = parent_dir(db_path).join("backups");
    fs::create_dir_all(&backups_dir).map_err(StoreError::from_io)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    // 版本和时间戳都可能进位；文件名只是载体，排序以解析出的整数为准。
    let stem = format!("storage-v{from_version}-{nanos:020}");
    let dest = backups_dir.join(format!("{stem}.sqlite"));
    let tmp = backups_dir.join(format!("{stem}.sqlite.tmp"));
    install_backup(conn, &tmp, &dest)?;
    prune_backups(&backups_dir)?;
    Ok(dest)
}

fn install_backup(conn: &Connection, tmp: &Path, dest: &Path) -> Result<()> {
    if dest.is_dir() {
        return Err(directory_dest(dest));
    }
    if let Err(err) = write_backup(conn, tmp) {
        return Err(abandon_partial(tmp, err));
    }
    if let Err(err) = fs::rename(tmp, dest) {
        return Err(abandon_partial(tmp, map_publish_error(err, dest)));
    }
    atomic::sync_parent(dest)
}

fn write_backup(conn: &Connection, tmp: &Path) -> Result<()> {
    conn.backup(DatabaseName::Main, tmp, None)
        .map_err(err_backup)?;
    sync_file(tmp)
}

fn abandon_partial(tmp: &Path, err: StoreError) -> StoreError {
    let _ = fs::remove_file(tmp);
    err
}

fn directory_dest(dest: &Path) -> StoreError {
    StoreError::Io {
        code: "io",
        source: io::Error::new(
            io::ErrorKind::IsADirectory,
            format!("backup destination is a directory: {}", dest.display()),
        ),
    }
}

fn sync_file(path: &Path) -> Result<()> {
    // Windows 的 FlushFileBuffers 要求写权限，只读打开得到 ACCESS_DENIED（task 013）。
    // Unix 对只读 fd 做 fsync 可以成功。Windows 允许只读句柄 flush 之前，三端都用写句柄。
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(StoreError::from_io)?;
    file.sync_all().map_err(StoreError::from_io)
}

// 父目录不存在时，Windows 的 rename 常报 ACCESS_DENIED(5)，Unix 报 ENOENT（task 013）。
// 两种都是父目录缺失，对外统一 `not-found`。Windows 改为 NOT_FOUND 之后可以只信 ErrorKind。
fn map_publish_error(err: io::Error, dest: &Path) -> StoreError {
    if publish_parent_missing(dest) {
        StoreError::Io {
            code: "not-found",
            source: io::Error::new(
                io::ErrorKind::NotFound,
                format!("backup parent missing: {}", dest.display()),
            ),
        }
    } else {
        StoreError::from_io(err)
    }
}

fn publish_parent_missing(dest: &Path) -> bool {
    match dest.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => !parent.exists(),
        _ => false,
    }
}

// 只认 `storage-v{version}-{nanos}.sqlite`。其它 `*.sqlite` 不占名额，也不删。
// 版本和时间戳都会进位，`storage-v10` 的字典序比 `storage-v9` 更小，所以按解析出的整数排序（task 013）。
fn prune_backups(dir: &Path) -> Result<()> {
    let mut ranked = Vec::new();
    for entry in fs::read_dir(dir).map_err(StoreError::from_io)? {
        let entry = entry.map_err(StoreError::from_io)?;
        let path = entry.path();
        if let Some(rank) = backup_rank(&path) {
            ranked.push((rank, path));
        }
    }
    if ranked.len() > BACKUPS_TO_KEEP {
        ranked.sort_by_key(|(rank, _)| *rank);
        let remove_count = ranked.len() - BACKUPS_TO_KEEP;
        for (_, path) in ranked.into_iter().take(remove_count) {
            fs::remove_file(path).map_err(StoreError::from_io)?;
        }
    }
    Ok(())
}

fn backup_rank(path: &Path) -> Option<(u128, u32)> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("storage-v")?.strip_suffix(".sqlite")?;
    let (version, nanos) = rest.split_once('-')?;
    if version.is_empty()
        || nanos.is_empty()
        || !version.bytes().all(|byte| byte.is_ascii_digit())
        || !nanos.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((nanos.parse().ok()?, version.parse().ok()?))
}

fn make_parent_dirs(path: &Path) -> Result<()> {
    fs::create_dir_all(parent_dir(path)).map_err(StoreError::from_io)
}

fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn err_open(source: rusqlite::Error) -> StoreError {
    db_err("open", source)
}

fn err_pragma(source: rusqlite::Error) -> StoreError {
    db_err("pragma", source)
}

fn err_user_version(source: rusqlite::Error) -> StoreError {
    db_err("user_version", source)
}

fn err_begin(source: rusqlite::Error) -> StoreError {
    db_err("begin", source)
}

fn err_commit(source: rusqlite::Error) -> StoreError {
    db_err("commit", source)
}

fn err_backup(source: rusqlite::Error) -> StoreError {
    db_err("backup", source)
}

fn db_err(stage: &str, source: rusqlite::Error) -> StoreError {
    StoreError::Io {
        code: sqlite_store_code(&source),
        source: io::Error::other(format!("{stage}: {source}")),
    }
}

fn sqlite_store_code(source: &rusqlite::Error) -> &'static str {
    match source.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DiskFull) => "disk-full",
        Some(rusqlite::ErrorCode::PermissionDenied) => "permission",
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => "locked",
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => "corrupt",
        _ => "io",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql_err() -> rusqlite::Error {
        rusqlite::Error::InvalidColumnName("x".into())
    }

    fn sqlite_failure(code: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None)
    }

    #[test]
    fn stage_mappers_wrap_rusqlite_error() {
        for (message, mapper) in [
            ("open", err_open as fn(rusqlite::Error) -> StoreError),
            ("pragma", err_pragma as fn(rusqlite::Error) -> StoreError),
            (
                "user_version",
                err_user_version as fn(rusqlite::Error) -> StoreError,
            ),
            ("begin", err_begin as fn(rusqlite::Error) -> StoreError),
            ("commit", err_commit as fn(rusqlite::Error) -> StoreError),
            ("backup", err_backup as fn(rusqlite::Error) -> StoreError),
        ] {
            let wrapped = mapper(sql_err());
            assert_eq!(wrapped.code(), "io");
            assert!(wrapped.to_string().contains(message));
        }
    }

    #[test]
    fn db_err_wraps_rusqlite_error_with_stage() {
        let err = db_err("open", sql_err());
        assert_eq!(err.code(), "io");
        assert!(
            err.to_string().contains("open"),
            "stage must appear in message"
        );
    }

    #[test]
    fn sqlite_errors_use_store_codes() {
        for (raw, code) in [
            (rusqlite::ffi::SQLITE_FULL, "disk-full"),
            (rusqlite::ffi::SQLITE_PERM, "permission"),
            (rusqlite::ffi::SQLITE_BUSY, "locked"),
            (rusqlite::ffi::SQLITE_LOCKED, "locked"),
            (rusqlite::ffi::SQLITE_CORRUPT, "corrupt"),
            (rusqlite::ffi::SQLITE_NOTADB, "corrupt"),
            (rusqlite::ffi::SQLITE_IOERR, "io"),
        ] {
            assert_eq!(
                db_err("open", sqlite_failure(raw)).code(),
                code,
                "raw {raw}"
            );
        }
    }

    #[test]
    fn require_wal_rejects_other_modes() {
        assert!(require_wal("wal").is_ok());
        assert!(require_wal("WAL").is_ok());
        let err = require_wal("delete").unwrap_err();
        assert_eq!(err.code(), "io");
    }

    fn tdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mythos-store-{}-{}",
            tag,
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const V1: &str = "CREATE TABLE heroes(id INTEGER PRIMARY KEY, name TEXT NOT NULL);";
    const V2: &str = "ALTER TABLE heroes ADD COLUMN level INTEGER NOT NULL DEFAULT 1;
CREATE TABLE items(id INTEGER PRIMARY KEY);";

    #[test]
    fn migrations_apply_in_order_and_are_idempotent() {
        let dir = tdir("db-migrate");
        let path = dir.join("nested/storage.sqlite");
        let migrations = [V1, V2];

        let conn = open(&path, &migrations).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert!(mode.eq_ignore_ascii_case("wal"));
        let busy_ms: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy_ms, BUSY_TIMEOUT.as_millis() as i64);
        for table in ["heroes", "items"] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} must exist");
        }
        drop(conn);

        let again = open(&path, &migrations).unwrap();
        let version: i64 = again
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2, "reopen must not re-run migrations");
        assert_eq!(
            fs::read_dir(dir.join("nested/backups")).unwrap().count(),
            1,
            "version 1 to 2 backs up once; reopening must not add a backup"
        );
        drop(again);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_migration_rolls_back_and_reports() {
        let dir = tdir("db-rollback");
        let path = dir.join("storage.sqlite");
        let bad = [
            V1,
            "CREATE TABLE partial(id INTEGER); INSERT INTO heroes(name) VALUES ('discard'); ALTER TABLE missing ADD COLUMN x INTEGER;",
        ];
        let err = open(&path, &bad).unwrap_err();
        assert_eq!(err.code(), "migration");

        let conn = open(&path, &[V1]).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1, "must stay on last good version");
        let count: i64 = conn
            .query_row("SELECT count(*) FROM heroes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 0,
            "successful statements before the failure must roll back"
        );
        let partial: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'partial'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(partial, 0, "DDL must also roll back");
        drop(conn);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_commit_rolls_back_and_leaves_connection_reusable() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        apply_migration(&mut conn, 0, V1).unwrap();
        // 延迟外键检查让语句全部成功、COMMIT 才失败，验证版本更新也随事务回滚。
        let sql = "CREATE TABLE parents(id INTEGER PRIMARY KEY);
CREATE TABLE children(parent_id INTEGER REFERENCES parents(id) DEFERRABLE INITIALLY DEFERRED);
INSERT INTO children VALUES (42);";
        assert_eq!(apply_migration(&mut conn, 1, sql).unwrap_err().code(), "io");
        assert!(
            conn.is_autocommit(),
            "failed commit must not leave a transaction open"
        );
        assert_eq!(query_user_version(&conn).unwrap(), 1);
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('parents', 'children')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "schema changes must roll back with the failed commit"
        );
        apply_migration(&mut conn, 1, V2).unwrap();
        assert_eq!(query_user_version(&conn).unwrap(), 2);
    }

    #[test]
    fn newer_database_is_rejected_without_rewriting_journal_mode() {
        let dir = tdir("db-corrupt");
        let path = dir.join("storage.sqlite");
        {
            let conn = open(&path, &[V1, V2]).unwrap();
            conn.execute_batch("PRAGMA user_version = 9; PRAGMA journal_mode = DELETE;")
                .unwrap();
        }
        let err = open(&path, &[V1, V2]).unwrap_err();
        assert_eq!(err.code(), "corrupt");
        let conn = Connection::open(&path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert!(
            mode.eq_ignore_ascii_case("delete"),
            "rejecting a newer database must not convert it to wal"
        );
        drop(conn);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn negative_user_version_is_rejected_as_corrupt() {
        let dir = tdir("db-negative");
        let path = dir.join("storage.sqlite");
        {
            let conn = open(&path, &[V1]).unwrap();
            conn.execute_batch("PRAGMA user_version = -1").unwrap();
        }
        let err = open(&path, &[V1]).unwrap_err();
        assert_eq!(err.code(), "corrupt");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn upgrade_backs_up_and_keeps_three() {
        let dir = tdir("db-backup");
        let path = dir.join("storage.sqlite");
        {
            let _conn = open(&path, &[V1]).unwrap();
        }
        let backups_dir = dir.join("backups");
        fs::create_dir_all(&backups_dir).unwrap();
        for _ in 0..4 {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let fake = backups_dir.join(format!("storage-v1-{stamp}.sqlite"));
            std::thread::sleep(std::time::Duration::from_nanos(1));
            fs::write(fake, b"old db").unwrap();
        }

        let _conn = open(&path, &[V1, V2]).unwrap();
        let kept: Vec<_> = fs::read_dir(dir.join("backups"))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(kept.len(), 3, "backups pruned to three");
        let has_real = kept
            .iter()
            .filter_map(|entry| fs::read(entry.path()).ok())
            .any(|content| !content.starts_with(b"old db"));
        assert!(
            has_real,
            "the pre-migration backup must be among the kept ones"
        );
        drop(_conn);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backup_snapshot_includes_uncheckpointed_rows() {
        let dir = tdir("db-wal-backup");
        let path = dir.join("storage.sqlite");
        let v1 = "CREATE TABLE heroes(id INTEGER PRIMARY KEY, name TEXT NOT NULL);
INSERT INTO heroes(name) VALUES ('kept');";
        let v2 = "ALTER TABLE heroes ADD COLUMN level INTEGER NOT NULL DEFAULT 1;";
        // 连接保持打开：小事务不会触发 WAL checkpoint，主文件本身还看不到这一行（task 013）。
        // 若备份改回拷贝主文件，这个断言会失败。
        let conn = open(&path, &[v1, v2]).unwrap();
        let mut backups: Vec<_> = fs::read_dir(dir.join("backups"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(backups.len(), 1);
        backups.sort();
        let backup = Connection::open(&backups[0]).unwrap();
        let version: i64 = backup
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
        let name: String = backup
            .query_row("SELECT name FROM heroes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "kept");
        drop(backup);
        drop(conn);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prune_keeps_newest_timestamp_not_lexicographic_name() {
        let dir = tdir("db-prune");
        fs::write(dir.join("storage-v10-1.sqlite"), b"old-v10").unwrap();
        fs::write(dir.join("storage-v9-3.sqlite"), b"new-v9").unwrap();
        fs::write(dir.join("storage-v2-2.sqlite"), b"mid").unwrap();
        fs::write(dir.join("storage-v1-4.sqlite"), b"newest").unwrap();
        fs::write(dir.join("notes.sqlite"), b"keep-me").unwrap();
        fs::write(dir.join("storage-v1-notanumber.sqlite"), b"skip").unwrap();
        prune_backups(&dir).unwrap();
        assert!(!dir.join("storage-v10-1.sqlite").exists());
        assert!(dir.join("storage-v9-3.sqlite").exists());
        assert!(dir.join("storage-v2-2.sqlite").exists());
        assert!(dir.join("storage-v1-4.sqlite").exists());
        assert!(dir.join("notes.sqlite").exists());
        assert!(dir.join("storage-v1-notanumber.sqlite").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_backup_rejects_a_directory_destination() {
        let dir = tdir("db-backup-dir");
        let conn = Connection::open_in_memory().unwrap();
        let dest = dir.join("dir-dest");
        fs::create_dir(&dest).unwrap();
        let tmp = dir.join("partial.sqlite.tmp");
        let err = install_backup(&conn, &tmp, &dest).unwrap_err();
        assert_eq!(err.code(), "io");
        assert!(!tmp.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_backup_removes_partial_when_rename_fails() {
        let dir = tdir("db-backup-rename");
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t(id INTEGER);").unwrap();
        let tmp = dir.join("partial.sqlite.tmp");
        let dest = dir.join("missing").join("out.sqlite");
        let err = install_backup(&conn, &tmp, &dest).unwrap_err();
        assert_eq!(err.code(), "not-found");
        assert!(!tmp.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_backup_cleans_up_when_sqlite_backup_fails() {
        let dir = tdir("db-backup-fail");
        let conn = Connection::open_in_memory().unwrap();
        let tmp = dir.join("blocked");
        fs::create_dir(&tmp).unwrap();
        let dest = dir.join("out.sqlite");
        let err = install_backup(&conn, &tmp, &dest).unwrap_err();
        assert_ne!(err.code(), "locked");
        assert!(tmp.is_dir());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_file_reports_missing_path() {
        let err = sync_file(Path::new("mythos-store-missing-backup-file")).unwrap_err();
        assert_eq!(err.code(), "not-found");
    }

    #[test]
    fn map_publish_error_is_not_found_only_when_the_parent_is_missing() {
        let dir = tdir("db-map-rename");
        let present = dir.join("out.sqlite");
        let err = map_publish_error(io::Error::from(io::ErrorKind::PermissionDenied), &present);
        assert_eq!(err.code(), "permission");
        let missing = dir.join("nope").join("out.sqlite");
        let err = map_publish_error(io::Error::from_raw_os_error(5), &missing);
        assert_eq!(err.code(), "not-found");
        assert!(!publish_parent_missing(Path::new("out.sqlite")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parent_dir_of_a_bare_file_is_dot() {
        assert_eq!(parent_dir(Path::new("storage.sqlite")), Path::new("."));
        assert_eq!(
            parent_dir(Path::new("dir/storage.sqlite")),
            Path::new("dir")
        );
    }

    #[test]
    fn open_fails_when_parent_is_a_file() {
        let dir = tdir("db-blocked");
        let blocked = dir.join("blocked");
        fs::write(&blocked, b"not a dir").unwrap();
        let err = open(&blocked.join("db.sqlite"), &[V1]).unwrap_err();
        // Windows 对「路径组件是文件」报 ERROR_ALREADY_EXISTS(183)，Kind 是 AlreadyExists，码仍是 `io`（task 013）。
        // Unix 是 ENOTDIR，同样收成 `io`。两边 Kind 分叉之前不要改成按 Kind 断言。
        assert_eq!(err.code(), "io");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_fails_when_path_is_a_directory() {
        let dir = tdir("db-isdir");
        let target = dir.join("as-db");
        fs::create_dir_all(&target).unwrap();
        let err = open(&target, &[V1]).unwrap_err();
        assert_eq!(err.code(), "io");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_fails_on_non_sqlite_file() {
        let dir = tdir("db-garbage");
        let path = dir.join("storage.sqlite");
        fs::write(&path, vec![0u8; 4096]).unwrap();
        let err = open(&path, &[V1]).unwrap_err();
        assert_eq!(err.code(), "corrupt", "a non-sqlite file is not a database");
        fs::remove_dir_all(&dir).unwrap();
    }
}
