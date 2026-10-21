//! 私有文件的当前用户权限策略；具体权限表示由目标平台实现。

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::restrict_to_current_user;
#[cfg(windows)]
pub(crate) use windows::restrict_to_current_user;

#[cfg(not(any(unix, windows)))]
use crate::error::Result;
#[cfg(not(any(unix, windows)))]
use std::path::Path;

#[cfg(not(any(unix, windows)))]
pub(crate) fn restrict_to_current_user(_path: &Path) -> Result<()> {
    Err(crate::error::StoreError::InvalidPath(
        "private file permissions are unsupported on this platform".into(),
    ))
}

#[cfg(test)]
pub(crate) fn temp_test_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "aoidos-private-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
