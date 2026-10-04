//! 多开检测：对数据目录内的锁文件取 OS 级独占锁；进程退出（含崩溃）由内核自动释放。

use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

use fs4::{FileExt, TryLockError};

use super::error::{Result, StoreError};

/// 持有期间阻止其他实例写存储。
///
/// Drop 只解锁，不删除锁文件：锁绑定在 inode / 文件对象上，先解锁再删名字会让
/// 另一个进程锁住新创建的同名文件，于是两个进程同时持有独占锁。
#[must_use = "丢弃 InstanceLock 会立刻释放锁"]
#[derive(Debug)]
pub struct InstanceLock {
    file: File,
}

/// 尝试获取实例锁；已有实例持有时报 `already-running`。
///
/// # Errors
///
/// 锁被占用时返回 `already-running`。创建目录、打开文件或写入 PID 的 I/O 失败
/// 保持原来的 `permission` / `io` 等码，不会伪装成已经有实例在运行。
pub fn acquire(data_dir: &Path) -> Result<InstanceLock> {
    fs::create_dir_all(data_dir).map_err(StoreError::from_io)?;
    let path = data_dir.join("storage.lock");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(StoreError::from_io)?;
    if let Err(err) = lock_exclusive(&file) {
        return Err(map_lock_error(err, &path));
    }
    write_pid(&mut file)?;
    Ok(InstanceLock { file })
}

fn lock_exclusive(file: &File) -> std::result::Result<(), LockAcquireError> {
    // Rust 1.89 起 `File::try_lock` 是固有方法，同名 trait 方法不会被选中（task 013）。
    // 去掉 fs4、改走标准库 `File::try_lock` 之后，可以删掉 `FileExt::` 前缀。
    lock_until_stable(|| FileExt::try_lock(file))
}

fn lock_until_stable(
    mut attempt: impl FnMut() -> std::result::Result<(), TryLockError>,
) -> std::result::Result<(), LockAcquireError> {
    loop {
        if let Some(done) = settle(attempt()) {
            return done;
        }
    }
}

// `None`：被信号打断，调用方再试。放在非泛型函数里，避免每个闭包单态化出
// 一份生产路径走不到的分支，导致 llvm-cov 行覆盖缺口（task 013）。
// 覆盖率改为「任一单态化走到即算覆盖」之后，可以收回这个拆分。
fn settle(
    result: std::result::Result<(), TryLockError>,
) -> Option<std::result::Result<(), LockAcquireError>> {
    match result {
        Ok(()) => Some(Ok(())),
        Err(TryLockError::WouldBlock) => Some(Err(LockAcquireError::Contended)),
        Err(TryLockError::Error(err)) if err.kind() == io::ErrorKind::Interrupted => None,
        Err(TryLockError::Error(err)) => Some(Err(LockAcquireError::Io(err))),
    }
}

enum LockAcquireError {
    Contended,
    Io(io::Error),
}

fn map_lock_error(err: LockAcquireError, lock_path: &Path) -> StoreError {
    match err {
        LockAcquireError::Contended => StoreError::AlreadyRunning {
            lock_path: lock_path.to_path_buf(),
        },
        LockAcquireError::Io(source) => StoreError::from_io(source),
    }
}

fn write_pid(file: &mut File) -> Result<()> {
    let bytes = format!("{}\n", std::process::id());
    file.seek(SeekFrom::Start(0)).map_err(StoreError::from_io)?;
    file.write_all(bytes.as_bytes())
        .map_err(StoreError::from_io)?;
    // 覆盖写不截断；上次 PID 更长时会留下尾巴（`10000\n` 再写成 `99\n`）。
    file.set_len(bytes.len() as u64)
        .map_err(StoreError::from_io)?;
    file.sync_all().map_err(StoreError::from_io)
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // 同 `FileExt::try_lock`：固有方法 `File::unlock` 会盖过 trait 方法（task 013）。
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::SystemTime;

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

    #[test]
    fn second_acquire_fails_release_allows_retry() {
        let dir = tdir("lock-basic");
        let guard = acquire(&dir).unwrap();

        let second = acquire(&dir).unwrap_err();
        assert_eq!(second.code(), "already-running");

        drop(guard);
        let lock_path = dir.join("storage.lock");
        assert!(
            lock_path.exists(),
            "release keeps the lock file so the name stays on one inode"
        );
        let third = acquire(&dir).unwrap();
        drop(third);
        // Windows 的字节范围锁是强制的，持有期间另一个句柄读不了；Unix 的 flock 是建议锁（task 013）。
        // 释放后再读，三端看到的都是最后持有者写下的 PID。
        let text = fs::read_to_string(&lock_path).unwrap();
        assert_eq!(text, format!("{}\n", std::process::id()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_pid_truncates_a_longer_previous_value() {
        let dir = tdir("lock-pid");
        let path = dir.join("storage.lock");
        let previous = format!("{}0000\n", std::process::id());
        fs::write(&path, previous).unwrap();
        let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        write_pid(&mut file).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{}\n", std::process::id())
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lock_errors_only_map_contention_to_already_running() {
        let path = Path::new("storage.lock");
        let contended = map_lock_error(LockAcquireError::Contended, path);
        assert_eq!(contended.code(), "already-running");

        let denied = map_lock_error(
            LockAcquireError::Io(io::Error::new(io::ErrorKind::PermissionDenied, "acl")),
            path,
        );
        assert_eq!(denied.code(), "permission");

        let interrupted = map_lock_error(
            LockAcquireError::Io(io::Error::new(io::ErrorKind::Interrupted, "signal")),
            path,
        );
        assert_eq!(interrupted.code(), "io");
    }

    #[test]
    fn lock_until_stable_retries_interruption_and_reports_contention() {
        let mut calls = 0;
        let err = lock_until_stable(|| {
            calls += 1;
            if calls == 1 {
                Err(TryLockError::Error(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "signal",
                )))
            } else {
                Err(TryLockError::Error(io::Error::from(
                    io::ErrorKind::PermissionDenied,
                )))
            }
        })
        .unwrap_err();
        assert_eq!(calls, 2);
        assert_eq!(
            map_lock_error(err, Path::new("storage.lock")).code(),
            "permission"
        );

        let contended = lock_until_stable(|| Err(TryLockError::WouldBlock)).unwrap_err();
        assert_eq!(
            map_lock_error(contended, Path::new("storage.lock")).code(),
            "already-running"
        );
        assert!(lock_until_stable(|| Ok(())).is_ok());
    }
}
