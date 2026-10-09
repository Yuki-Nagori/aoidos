//! 原生密码输入与私有权限：平台差异只在此按编译目标分发。
//! 命令层统一在 UI 主线程调用，不接触 Webview 密钥，也不按 OS 分支。
//! Windows 用 CredUI，macOS 用 NSSecureTextField / NSAlert，Linux 用 GTK 密码 Entry。

use aoidos_store::error::Result;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
pub(crate) mod windows;

#[cfg(target_os = "linux")]
use linux::prompt_native_key;
#[cfg(target_os = "macos")]
use macos::prompt_native_key;
#[cfg(unix)]
pub(crate) use unix::restrict_to_current_user;
#[cfg(windows)]
use windows::prompt_native_key;
#[cfg(windows)]
pub(crate) use windows::restrict_to_current_user;

/// 同型原生输入入口；调用方需在 UI 主线程执行。取消为 None，失败是 store.*。
pub type NativePrompt = fn(&str) -> Result<Option<String>>;

/// 返回本平台原生输入器；不把运行期平台差异暴露给使用点。
#[must_use]
pub fn platform_prompt() -> NativePrompt {
    prompt_native_key
}

#[cfg(all(feature = "native-smoke", target_os = "linux"))]
pub use linux::verify_native_input;
/// 真实原生 UI 确认 / 取消烟测，仅显式测试特性提供，不进入默认发布产物。
///
/// # Errors
/// 缺桌面会话、主线程不匹配或真实 UI 结果不符时失败。
#[cfg(all(feature = "native-smoke", target_os = "macos"))]
pub use macos::verify_native_input;

#[cfg(all(feature = "native-smoke", windows))]
pub use windows::verify_native_input;
