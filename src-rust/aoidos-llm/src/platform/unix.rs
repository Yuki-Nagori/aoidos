//! macOS / Linux 平台实现：降级凭据文件收紧为 0600。
//!
//! 原生输入由 macos / linux 模块承接，本模块只维护 Unix 权限。

use std::path::Path;

use aoidos_store::error::{Result, StoreError};

/// 0600：仅当前用户读写（不把 Unix 的 0600 语义套在 Windows 上）。
///
/// # Errors
/// 权限 API 失败时按 store 域码返回（失败时私有写入方不会写正文或发布文件）。
pub(crate) fn restrict_to_current_user(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(StoreError::from_io)?
        .permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms).map_err(StoreError::from_io)
}
