//! 全仓唯一的落盘实现：唯一 tmp + fsync + rename 原子写，与文件自愈工具。
//! 普通文件覆写经本模块；SQLite 事务与备份、多开锁文件由各自模块维护。

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::sleep;
use std::time::Duration;

use super::error::{Result, StoreError};

const RETRIES: u32 = 5;
const BASE_DELAY_MS: u64 = 25;
// 这两个号码只在 Windows 上是占用（ACCESS_DENIED / SHARING_VIOLATION，task 013）。
// Unix 上同号是 EIO / EPIPE。Windows 把它们报成可区分的 ErrorKind 之后，可以删掉按号码判断的分支。
#[cfg(windows)]
const WINDOWS_ACCESS_DENIED: i32 = 5;
#[cfg(windows)]
const WINDOWS_SHARING_VIOLATION: i32 = 32;

static TMP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// 原子写任意字节：同目录唯一 tmp → fsync → rename 覆盖。父目录不存在则自动创建。
///
/// 目标被操作系统拒绝替换（Windows 共享冲突，或三端通用的 busy）时按 25ms 指数退避重试，
/// 耗尽后报 `locked` 并清理 tmp。目标是目录时三端都立刻报 `io`，不进入占用重试。
/// rename 成功后的父目录同步若失败，返回错误时目标可能已经更新。
///
/// # Errors
///
/// 父目录无法创建、写入失败、重试耗尽，或 rename 遇到非占用错误时返回 [`StoreError`]。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    // 相对文件名 `file.txt` 的 parent 是空路径，create_dir_all("") 直接成功。
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(StoreError::from_io)?;
    let tmp = tmp_sibling(path)?;
    write_tmp(&tmp, bytes)?;
    rename_with_retry(&tmp, path)
}

/// [`write_atomic`] 的 UTF-8 文本便捷封装。
///
/// # Errors
///
/// 与 [`write_atomic`] 相同。
pub fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    write_atomic(path, text.as_bytes())
}

/// 清理目录内残留的 `*.tmp`（崩溃 / 失败遗留），返回清理数量。
///
/// # Errors
///
/// 目录无法读取，或某个临时文件无法删除时返回。
pub fn clean_temp_files(dir: &Path) -> Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(dir).map_err(StoreError::from_io)? {
        let entry = entry.map_err(StoreError::from_io)?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "tmp") {
            fs::remove_file(&path).map_err(StoreError::from_io)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// JSONL 自愈：文件必须以换行结尾；尾行不完整（缺换行）则截断到上一完整行。
/// 返回是否发生截断。空文件 / 已完整返回 `false`。
///
/// # Errors
///
/// 文件无法读取或截断无法落盘时返回。
pub fn truncate_incomplete_jsonl(path: &Path) -> Result<bool> {
    let bytes = fs::read(path).map_err(StoreError::from_io)?;
    if bytes.is_empty() || bytes.last() == Some(&b'\n') {
        return Ok(false);
    }
    let new_len = match bytes.iter().rposition(|&b| b == b'\n') {
        Some(cut) => cut as u64 + 1,
        None => 0, // 没有完整行可留，清空，避免留下半行
    };
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(StoreError::from_io)?;
    file.set_len(new_len).map_err(StoreError::from_io)?;
    file.sync_all().map_err(StoreError::from_io)?;
    Ok(true)
}

fn tmp_sibling(target: &Path) -> Result<PathBuf> {
    let file_name = target
        .file_name()
        .ok_or_else(|| StoreError::InvalidPath(target.display().to_string()))?;
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(format!(".{}.{seq}.tmp", std::process::id()));
    Ok(target.with_file_name(tmp_name))
}

fn write_tmp(tmp: &Path, bytes: &[u8]) -> Result<()> {
    // create_new 原子地拒绝既有文件与符号链接；创建失败时不清理，因为该路径不属于本次写入。
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(tmp)
        .map_err(StoreError::from_io)?;
    let result = write_tmp_inner(&mut file, bytes);
    drop(file);
    finish_tmp_write(result, tmp)
}

fn finish_tmp_write(result: Result<()>, tmp: &Path) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(tmp);
            Err(err)
        }
    }
}

fn write_tmp_inner(file: &mut File, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes).map_err(StoreError::from_io)?;
    file.sync_all().map_err(StoreError::from_io)
}

fn rename_with_retry(tmp: &Path, target: &Path) -> Result<()> {
    replace_with_retry(tmp, target, &mut || fs::rename(tmp, target))
}

// 共享一个重试实现，避免各闭包的泛型实例各自缺少成功 / 失败路径，造成覆盖率汇总缺口（issue #1）。
fn replace_with_retry(
    tmp: &Path,
    target: &Path,
    rename: &mut dyn FnMut() -> io::Result<()>,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        match rename() {
            Ok(()) => return sync_parent(target),
            Err(err) => {
                if !should_retry_rename(&err, target, attempt) {
                    let _ = fs::remove_file(tmp);
                    return Err(finish_rename_err(err, target));
                }
                sleep(retry_delay(attempt));
                attempt += 1;
            }
        }
    }
}

fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(BASE_DELAY_MS << attempt)
}

// 目录目标三端都立刻失败（task 013）。Windows 对「文件 rename 到目录」也报 ACCESS_DENIED，
// 若先按占用重试，目录会在 Windows 上等满退避、在 Unix 上马上返回 `io`。
fn should_retry_rename(err: &io::Error, target: &Path, attempt: u32) -> bool {
    attempt < RETRIES && !target.is_dir() && is_replace_busy(err)
}

// `WouldBlock` / `ResourceBusy` / `ExecutableFileBusy` 三端都表示目标暂时不能替换。
fn is_replace_busy(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::ResourceBusy | io::ErrorKind::ExecutableFileBusy
    ) || is_windows_sharing_violation(err)
}

#[cfg(windows)]
fn is_windows_sharing_violation(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(WINDOWS_ACCESS_DENIED) | Some(WINDOWS_SHARING_VIOLATION)
    )
}

#[cfg(not(windows))]
fn is_windows_sharing_violation(_: &io::Error) -> bool {
    false
}

fn finish_rename_err(err: io::Error, target: &Path) -> StoreError {
    if target.is_dir() {
        return directory_target_error();
    }
    if is_replace_busy(&err) {
        StoreError::LockedTimeout {
            path: target.to_path_buf(),
        }
    } else {
        StoreError::from_io(err)
    }
}

fn directory_target_error() -> StoreError {
    StoreError::Io {
        code: "io",
        source: io::Error::new(
            io::ErrorKind::IsADirectory,
            "atomic replace target is a directory",
        ),
    }
}

/// rename 已经发布目录项之后，尽力刷父目录元数据。
///
/// 权限拒绝或卷不支持目录 flush 时，保留已经完成的替换结果（task 013）。
/// 其它同步错误仍上抛，调用方不能据此认定替换尚未发生。
///
/// # Errors
///
/// 打开父目录或 flush 失败，且不是「卷不支持目录同步」时返回。
pub(crate) fn sync_parent(target: &Path) -> Result<()> {
    accept_dir_sync(sync_dir(parent_for_sync(target)))
}

fn parent_for_sync(target: &Path) -> &Path {
    match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    let file = open_dir(dir)?;
    file.sync_all()
}

fn accept_dir_sync(result: io::Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(err) if is_dir_sync_unsupported(&err) => Ok(()),
        Err(err) => Err(StoreError::from_io(err)),
    }
}

fn is_dir_sync_unsupported(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
    )
}

#[cfg(windows)]
fn open_dir(dir: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // 不带 FILE_FLAG_BACKUP_SEMANTICS 时，打开目录得到 ACCESS_DENIED，目录项无法 flush（task 013）。
    // FlushFileBuffers 还要求写权限。Windows 允许普通只读句柄打开并 flush 目录之前，这两个条件都要留着。
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(dir)
}

#[cfg(not(windows))]
fn open_dir(dir: &Path) -> io::Result<File> {
    File::open(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Instant, SystemTime};

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
    fn write_creates_parents_and_overwrites() {
        let dir = tdir("atomic-basic");
        let path = dir.join("nested/deeper/data.txt");
        write_text_atomic(&path, "first").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");
        write_text_atomic(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(clean_temp_files(&dir).unwrap(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_atomic_rejects_directory_without_retrying() {
        let dir = tdir("atomic-dir-target");
        let target = dir.join("occupied");
        fs::create_dir_all(&target).unwrap();
        let started = Instant::now();
        let err = write_atomic(&target, b"x").unwrap_err();
        assert!(
            started.elapsed() < Duration::from_millis(700),
            "a directory is not a locked file"
        );
        assert_eq!(err.code(), "io");
        assert_eq!(clean_temp_files(&dir).unwrap(), 0, "tmp must be cleaned");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn write_atomic_reports_locked_when_target_held_open() {
        // std 默认带 FILE_SHARE_DELETE，占着的文件仍可被 rename。剥掉该共享位，
        // 模拟杀软 / 同步盘（task 013）。Unix 的 rename 不因文件打开而失败，所以此测试只在 Windows 编译；
        // 标准库改成默认不共享 DELETE 之后可以删。
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;

        let dir = tdir("atomic-locked");
        let target = dir.join("data.txt");
        fs::write(&target, b"old").unwrap();
        let _held = File::options()
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&target)
            .unwrap();
        let started = Instant::now();
        let err = write_atomic(&target, b"new").unwrap_err();
        assert!(
            started.elapsed() >= Duration::from_millis(700),
            "busy target must back off"
        );
        assert_eq!(err.code(), "locked");
        assert_eq!(clean_temp_files(&dir).unwrap(), 0);
        drop(_held);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finish_rename_err_matches_on_every_os() {
        let dir = tdir("atomic-map");
        let as_dir = dir.join("target-dir");
        fs::create_dir(&as_dir).unwrap();
        let as_file = dir.join("target-file");

        for kind in [
            io::ErrorKind::WouldBlock,
            io::ErrorKind::ResourceBusy,
            io::ErrorKind::ExecutableFileBusy,
        ] {
            let err = finish_rename_err(io::Error::new(kind, "busy"), &as_file);
            assert_eq!(err.code(), "locked", "{kind:?} is busy on every OS");
        }

        let dir_err = finish_rename_err(io::Error::from_raw_os_error(5), &as_dir);
        assert_eq!(dir_err.code(), "io", "directory target is io everywhere");

        let missing = finish_rename_err(io::Error::from(io::ErrorKind::NotFound), &as_file);
        assert_eq!(missing.code(), "not-found");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_sharing_codes_are_locked() {
        let dir = tdir("atomic-map-win");
        let file = dir.join("target-file");
        for raw in [WINDOWS_ACCESS_DENIED, WINDOWS_SHARING_VIOLATION] {
            let err = finish_rename_err(io::Error::from_raw_os_error(raw), &file);
            assert_eq!(err.code(), "locked", "raw {raw} is a Windows sharing code");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_eio_and_epipe_are_not_locked() {
        let dir = tdir("atomic-map-unix");
        let file = dir.join("target-file");
        // 5 / 32 在 Unix 上是 EIO / EPIPE（task 013）。Windows 改用可区分的 ErrorKind 后，这个测试和原始错误码分支一起删。
        for raw in [5, 32] {
            let err = finish_rename_err(io::Error::from_raw_os_error(raw), &file);
            assert_eq!(err.code(), "io", "raw {raw} must not be locked off Windows");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn retry_policy_is_shared() {
        assert_eq!(retry_delay(0), Duration::from_millis(25));
        assert_eq!(retry_delay(1), Duration::from_millis(50));
        assert_eq!(retry_delay(4), Duration::from_millis(400));

        let dir = tdir("atomic-policy");
        let file = dir.join("file");
        let folder = dir.join("folder");
        fs::create_dir(&folder).unwrap();
        let busy = io::Error::new(io::ErrorKind::ResourceBusy, "busy");
        assert!(should_retry_rename(&busy, &file, 0));
        assert!(!should_retry_rename(&busy, &file, RETRIES));
        assert!(!should_retry_rename(&busy, &folder, 0));
        let missing = io::Error::from(io::ErrorKind::NotFound);
        assert!(!should_retry_rename(&missing, &file, 0));
        assert!(!is_replace_busy(&missing));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_retries_once_then_publishes() {
        let dir = tdir("atomic-retry");
        let target = dir.join("data.txt");
        let tmp = dir.join("data.txt.tmp");
        fs::write(&tmp, b"new").unwrap();
        let mut calls = 0;
        replace_with_retry(&tmp, &target, &mut || {
            calls += 1;
            if calls == 1 {
                Err(io::Error::new(io::ErrorKind::ResourceBusy, "busy"))
            } else {
                fs::rename(&tmp, &target)
            }
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(fs::read(&target).unwrap(), b"new");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_exhausts_busy_retries_and_preserves_target() {
        let dir = tdir("atomic-retry-exhausted");
        let target = dir.join("data.txt");
        let tmp = dir.join("data.txt.tmp");
        fs::write(&target, b"old").unwrap();
        fs::write(&tmp, b"new").unwrap();
        // Unix 打开的文件仍可被 rename；注入 busy 让三端都验证完整的重试耗尽路径（issue #1）。
        let mut calls = 0;
        let err = replace_with_retry(&tmp, &target, &mut || {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::ResourceBusy))
        })
        .unwrap_err();
        assert_eq!(calls, RETRIES + 1, "initial attempt plus five retries");
        assert!(matches!(err, StoreError::LockedTimeout { path } if path == target));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(!tmp.exists(), "failed replacement must clean up tmp");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_stops_on_non_busy_error_and_preserves_target() {
        let dir = tdir("atomic-retry-io");
        let target = dir.join("data.txt");
        let tmp = dir.join("data.txt.tmp");
        fs::write(&target, b"old").unwrap();
        fs::write(&tmp, b"new").unwrap();
        let mut calls = 0;
        let err = replace_with_retry(&tmp, &target, &mut || {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::NotFound))
        })
        .unwrap_err();
        assert_eq!(calls, 1, "non-busy errors must not retry");
        assert_eq!(err.code(), "not-found");
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(!tmp.exists(), "failed replacement must clean up tmp");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_tmp_cleans_up_when_create_fails() {
        let dir = tdir("atomic-tmp-fail");
        let err = write_tmp(&dir, b"x").unwrap_err();
        assert_ne!(err.code(), "locked");
        assert!(dir.is_dir());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_tmp_preserves_existing_file_on_create_conflict() {
        let dir = tdir("atomic-tmp-conflict");
        let tmp = dir.join("existing.tmp");
        fs::write(&tmp, b"keep").unwrap();
        assert_eq!(write_tmp(&tmp, b"new").unwrap_err().code(), "io");
        assert_eq!(fs::read(&tmp).unwrap(), b"keep");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_tmp_write_removes_owned_file() {
        let dir = tdir("atomic-tmp-write-fail");
        let tmp = dir.join("owned.tmp");
        fs::write(&tmp, b"partial").unwrap();
        // 只读句柄确定地拒绝写入，不依赖磁盘配额或平台特有设备。
        let mut file = File::open(&tmp).unwrap();
        let result = write_tmp_inner(&mut file, b"new");
        drop(file);
        assert!(finish_tmp_write(result, &tmp).is_err());
        assert!(!tmp.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn tmp_sibling_preserves_non_utf8_file_name() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = tdir("atomic-non-utf8");
        let path = dir.join(OsStr::from_bytes(b"data-\xff.txt"));
        let tmp = tmp_sibling(&path).unwrap();
        assert!(
            tmp.file_name()
                .unwrap()
                .as_bytes()
                .starts_with(b"data-\xff.txt.")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parent_for_sync_uses_dot_for_a_bare_file_name() {
        assert_eq!(parent_for_sync(Path::new("file.txt")), Path::new("."));
        assert_eq!(parent_for_sync(Path::new("dir/file.txt")), Path::new("dir"));
    }

    #[test]
    fn sync_parent_reports_a_missing_directory() {
        let err = sync_parent(Path::new("mythos-store-missing-parent/file.txt")).unwrap_err();
        assert_eq!(err.code(), "not-found");
    }

    #[test]
    fn accept_dir_sync_ignores_unsupported_volumes() {
        assert!(accept_dir_sync(Ok(())).is_ok());
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::InvalidInput,
            io::ErrorKind::Unsupported,
        ] {
            assert!(accept_dir_sync(Err(io::Error::new(kind, "skip"))).is_ok());
        }
        let err = accept_dir_sync(Err(io::Error::from(io::ErrorKind::NotFound))).unwrap_err();
        assert_eq!(err.code(), "not-found");
    }

    #[test]
    fn tmp_sibling_rejects_paths_without_file_name() {
        assert!(tmp_sibling(Path::new("a/..")).is_err());
    }

    #[test]
    fn clean_temp_files_removes_only_tmp() {
        let dir = tdir("atomic-clean");
        fs::write(dir.join("a.tmp"), b"x").unwrap();
        fs::write(dir.join("b.tmp"), b"x").unwrap();
        fs::write(dir.join("keep.txt"), b"x").unwrap();
        assert_eq!(clean_temp_files(&dir).unwrap(), 2);
        assert!(dir.join("keep.txt").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncate_incomplete_jsonl_cuts_partial_tail() {
        let dir = tdir("jsonl-heal");
        let path = dir.join("log.jsonl");
        fs::write(&path, b"{\"a\":1}\n{\"a\":2}\n{\"a\":3").unwrap();
        assert!(truncate_incomplete_jsonl(&path).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}\n{\"a\":2}\n");

        fs::write(&path, b"{\"a\":1}\n").unwrap();
        assert!(!truncate_incomplete_jsonl(&path).unwrap());

        fs::write(&path, b"{\"a\":1").unwrap();
        assert!(truncate_incomplete_jsonl(&path).unwrap());
        assert!(fs::read(&path).unwrap().is_empty());

        fs::write(&path, b"").unwrap();
        assert!(!truncate_incomplete_jsonl(&path).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }
}
