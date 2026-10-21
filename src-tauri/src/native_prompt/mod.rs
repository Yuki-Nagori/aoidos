//! Tauri 桌面入口的原生密钥输入；平台差异只在此按编译目标分发。
//! 命令层使用同一个接口，并由 Tauri 将调用调度到 UI 主线程。

use aoidos_store::error::Result;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux::prompt_native_key;
#[cfg(target_os = "macos")]
use macos::prompt_native_key;
#[cfg(windows)]
use windows::prompt_native_key;

/// 各平台共享的原生输入文案；参数内容由受信任的 locale 资源提供。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePromptText {
    pub title: String,
    pub message: String,
    pub save: String,
    pub cancel: String,
}

impl Default for NativePromptText {
    fn default() -> Self {
        Self {
            title: "Aoidos API 密钥".into(),
            message: "输入 {provider} 的 API 密钥；取消将保留当前已保存的密钥。".into(),
            save: "保存".into(),
            cancel: "取消".into(),
        }
    }
}

/// 同型原生输入入口；调用方需在 UI 主线程执行。取消为 None，失败是 store.*。
pub type NativePrompt = fn(&str, &NativePromptText) -> Result<Option<String>>;

/// 返回本平台原生输入器；不把运行期平台差异暴露给使用点。
#[must_use]
pub fn platform_prompt() -> NativePrompt {
    prompt_native_key
}

/// 真实原生 UI 确认 / 取消烟测，仅显式测试特性提供，不进入默认发布产物。
///
/// # Errors
/// 缺桌面会话、主线程不匹配或真实 UI 结果不符时失败。
#[cfg(feature = "native-smoke")]
pub fn verify_native_input() -> Result<()> {
    platform_verify::run()
}

#[cfg(all(feature = "native-smoke", target_os = "linux"))]
mod platform_verify {
    pub(super) use super::linux::verify_native_input as run;
}
#[cfg(all(feature = "native-smoke", target_os = "macos"))]
mod platform_verify {
    pub(super) use super::macos::verify_native_input as run;
}
#[cfg(all(feature = "native-smoke", windows))]
mod platform_verify {
    pub(super) use super::windows::verify_native_input as run;
}
