//! 平台适配层：原生凭据输入与降级文件的权限收紧。
//!
//! 每个平台一个模块，行为都有确定性的编译期分发（`#[cfg]`），不做运行期
//! 探测；三平台的支持状态（019 实测口径，「未验证」不得宣称支持）：
//!
//! | 能力                          | Windows            | macOS / Linux（unix）        |
//! | ----------------------------- | ------------------ | ---------------------------- |
//! | 原生密钥输入                  | CredUI（已验证）   | 未验证 → [`NativePrompt::Unverified`] |
//! | 降级文件权限                  | 受保护 DACL        | 0600                         |
//! | OS 凭据库                     | Credential Manager | keyring 默认特性，未验证     |
//!
//! OS 凭据库本身无需平台代码：keyring 默认特性在 macOS / Linux 走 Keychain /
//! Secret Service，不可用或无会话时由 [`crate::credentials::CredentialVault`]
//! 降级为仅当前用户可读的私有文件。原生输入在 macOS / Linux 的具体方案
//! （Keychain 授权对话框 / 无标准安全输入）尚未验证——任务 019 前置条件要求
//! 记录阻塞并修订设计后才能更换入口，因此分发显式返回
//! [`NativePrompt::Unverified`]，由命令层以 `app.bad-request` 拒绝，不静默
//! 降级为 Webview 明文表单。

use std::path::Path;

use mythos_store::error::Result;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
pub(crate) mod windows;

/// 原生密钥输入的平台可用性。
#[derive(Debug, Clone, Copy)]
pub enum NativePrompt {
    /// 平台实现就绪（Windows：CredUI 对话框）。
    Ready(fn(&str) -> Result<Option<String>>),
    /// 平台尚未验证；调用方必须显式拒绝，不得降级为 Webview 明文输入。
    Unverified,
}

/// 按编译目标分发原生输入能力；每个平台都有确定性结果（直测断言）。
pub fn platform_prompt() -> NativePrompt {
    #[cfg(windows)]
    {
        NativePrompt::Ready(windows::prompt_native_key)
    }
    #[cfg(not(windows))]
    {
        NativePrompt::Unverified
    }
}

/// 把降级凭据文件收紧为仅当前用户可读：Windows 用受保护 DACL，
/// Unix 用 0600（不把 0600 当 Windows 权限）。
///
/// # Errors
/// 平台权限 API 失败时返回 store 域 `io` 错误（文件已写入成功，失败只意味
/// 权限仍是默认继承——下次写入会重试收紧）。
pub(crate) fn restrict_to_current_user(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        windows::restrict_to_current_user(path)
    }
    #[cfg(unix)]
    {
        unix::restrict_to_current_user(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_dispatches_to_credui() {
        assert!(matches!(platform_prompt(), NativePrompt::Ready(_)));
    }

    #[cfg(not(windows))]
    #[test]
    fn unverified_platforms_dispatch_explicitly() {
        assert!(matches!(platform_prompt(), NativePrompt::Unverified));
    }
}
