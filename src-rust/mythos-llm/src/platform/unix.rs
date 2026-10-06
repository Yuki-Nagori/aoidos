//! macOS / Linux 平台实现：降级凭据文件收紧为 0600。
//!
//! 原生密钥输入在两个平台均未验证（见模块文档），由 [`super`] 的分发显式
//! 返回 `NativePrompt::Unverified`，本模块不提供输入实现。

use std::path::Path;

use mythos_store::error::{Result, StoreError};

/// 0600：仅当前用户读写（不把 Unix 的 0600 语义套在 Windows 上）。
///
/// # Errors
/// 权限 API 失败时按 store 域码返回（文件已写入成功，失败只意味着权限仍是
/// 默认继承——下次写入会重试收紧）。
pub(crate) fn restrict_to_current_user(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(StoreError::from_io)?
        .permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms).map_err(StoreError::from_io)
}
