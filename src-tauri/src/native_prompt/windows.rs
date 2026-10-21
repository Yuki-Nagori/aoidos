//! Windows 原生密钥输入：使用不持久化凭据的 CredUI 密码对话框。

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;

use super::NativePromptText;
use aoidos_store::error::{Result, StoreError};
use windows_sys::Win32::Security::Credentials::{
    CREDUI_FLAGS_ALWAYS_SHOW_UI, CREDUI_FLAGS_DO_NOT_PERSIST, CREDUI_FLAGS_GENERIC_CREDENTIALS,
    CREDUI_FLAGS_KEEP_USERNAME, CREDUI_INFOW, CredUIPromptForCredentialsW,
};

/// Windows 原生取消测试用固定标题定位 `CredUI` 窗口。
#[cfg(any(test, feature = "native-smoke"))]
const PROMPT_CAPTION: &str = "Aoidos API 密钥";

/// 弹出 `CredUI` 密码对话框。
///
/// # Errors
/// `CredUI` 初始化或调用失败（非用户取消）时返回 store 域 `io` 错误。
pub(super) fn prompt_native_key(
    provider_label: &str,
    text: &NativePromptText,
) -> Result<Option<String>> {
    let caption: Vec<u16> = OsStr::new(&text.title)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let message: Vec<u16> = OsStr::new(&text.message.replace("{provider}", provider_label))
        .encode_wide()
        .chain(Some(0))
        .collect();
    let info = CREDUI_INFOW {
        cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
        hwndParent: std::ptr::null_mut(),
        pszMessageText: message.as_ptr(),
        pszCaptionText: caption.as_ptr(),
        hbmBanner: std::ptr::null_mut(),
    };
    let mut user: [u16; 512] = [0; 512];
    let user_label: Vec<u16> = OsStr::new(provider_label).encode_wide().collect();
    let label_len = user_label.len().min(511);
    user[..label_len].copy_from_slice(&user_label[..label_len]);
    let mut password: [u16; 512] = [0; 512];
    let mut save = 0i32;
    let flags = CREDUI_FLAGS_GENERIC_CREDENTIALS
        | CREDUI_FLAGS_DO_NOT_PERSIST
        | CREDUI_FLAGS_KEEP_USERNAME
        | CREDUI_FLAGS_ALWAYS_SHOW_UI;
    // SAFETY: info.cbSize 与结构体实际大小一致，message / caption 均为
    // NUL 结尾的宽字符串；user / password 缓冲按容量如实传递；save 为合法
    // 出参。DO_NOT_PERSIST 下 API 不写 OS 凭据存储。
    let result = unsafe {
        CredUIPromptForCredentialsW(
            &raw const info,
            std::ptr::null(),
            std::ptr::null(),
            0,
            user.as_mut_ptr(),
            user.len() as u32,
            password.as_mut_ptr(),
            password.len() as u32,
            &raw mut save,
            flags,
        )
    };
    let outcome = map_cred_ui_result(result, &password);
    // 明文生命周期只到本函数：缓冲用后即清，栈上残留不外泄。
    password.fill(0);
    user.fill(0);
    outcome
}

/// `CredUI` 返回码 → 结果的纯映射（单测直测三分支；明文只存在于返回值）。
fn map_cred_ui_result(result: u32, password: &[u16]) -> Result<Option<String>> {
    match result {
        windows_sys::Win32::Foundation::NO_ERROR => {
            let len = password
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(password.len());
            Ok(Some(String::from_utf16_lossy(&password[..len])))
        }
        windows_sys::Win32::Foundation::ERROR_CANCELLED => Ok(None),
        code => Err(StoreError::Io {
            code: "io",
            source: io::Error::other(format!("credential ui failed: {code}")),
        }),
    }
}

// 合成消息只投递给本测试进程，避免同标题的真实应用窗口被取消。
#[cfg(any(test, feature = "native-smoke"))]
fn own_prompt_window(caption: &[u16]) -> windows_sys::Win32::Foundation::HWND {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};
    // SAFETY: 调用方提供 NUL 结尾标题；查询到的窗口仅用于核对进程 ID。
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), caption.as_ptr());
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &raw mut pid);
        if pid == std::process::id() {
            hwnd
        } else {
            std::ptr::null_mut()
        }
    }
}

#[cfg(feature = "native-smoke")]
pub(super) fn verify_native_input() -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GWL_STYLE, GetWindowLongW, PostMessageW, SendMessageW, WM_CLOSE,
        WM_COMMAND, WM_SETTEXT,
    };
    static FILLED: AtomicBool = AtomicBool::new(false);
    unsafe extern "system" fn fill(hwnd: windows_sys::Win32::Foundation::HWND, _: isize) -> i32 {
        // SAFETY: hwnd 来自当前进程窗口的 EnumChildWindows；只选 ES_PASSWORD
        // 控件，固定 UTF-16 合成值在 SendMessage 返回前保持有效。
        unsafe {
            if GetWindowLongW(hwnd, GWL_STYLE) & 0x20 != 0 {
                let value: Vec<u16> = "aoidos-native-fixture"
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                SendMessageW(hwnd, WM_SETTEXT, 0, value.as_ptr() as isize);
                FILLED.store(true, Ordering::SeqCst);
            }
        }
        1
    }
    for accept in [true, false] {
        FILLED.store(false, Ordering::SeqCst);
        let helper = std::thread::spawn(move || {
            let caption: Vec<u16> = PROMPT_CAPTION.encode_utf16().chain(Some(0)).collect();
            for _ in 0..600 {
                // SAFETY: 字符串 NUL 结尾；只操作匹配当前进程 ID 的测试窗口。
                unsafe {
                    let hwnd = own_prompt_window(&caption);
                    if !hwnd.is_null() {
                        if accept {
                            EnumChildWindows(hwnd, Some(fill), 0);
                            if FILLED.load(Ordering::SeqCst) {
                                PostMessageW(hwnd, WM_COMMAND, 1, 0);
                                return true;
                            }
                        } else {
                            PostMessageW(hwnd, WM_CLOSE, 0, 0);
                            return true;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        });
        let result = prompt_native_key("test fixture", &NativePromptText::default())?;
        if !helper.join().unwrap_or(false)
            || (accept && result.as_deref() != Some("aoidos-native-fixture"))
            || (!accept && result.is_some())
        {
            return Err(StoreError::Corrupt(
                "native input verification failed".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

    #[test]
    fn cred_ui_result_maps_all_three_outcomes() {
        let mut buffer: Vec<u16> = "sk-1234".encode_utf16().collect();
        buffer.push(0);
        buffer.extend_from_slice(&[0x7f; 8]);
        assert_eq!(
            map_cred_ui_result(windows_sys::Win32::Foundation::NO_ERROR, &buffer).unwrap(),
            Some(String::from("sk-1234"))
        );
        assert_eq!(
            map_cred_ui_result(windows_sys::Win32::Foundation::NO_ERROR, &[0x41; 4]).unwrap(),
            Some(String::from("AAAA"))
        );
        assert_eq!(
            map_cred_ui_result(windows_sys::Win32::Foundation::ERROR_CANCELLED, &buffer).unwrap(),
            None
        );
        assert_eq!(map_cred_ui_result(1385, &buffer).unwrap_err().code(), "io");
    }

    /// 取消路径直测：以 WM_CLOSE 关闭当前进程的真实 CredUI，确认映射为 Ok(None)。
    #[test]
    fn cred_ui_dialog_cancel_returns_none() {
        let caption: Vec<u16> = PROMPT_CAPTION.encode_utf16().chain(Some(0)).collect();
        let handle = std::thread::spawn(|| {
            prompt_native_key("provider deepseek", &NativePromptText::default())
        });
        let mut closed = false;
        for _ in 0..600 {
            let hwnd = own_prompt_window(&caption);
            if !hwnd.is_null() {
                // SAFETY: 窗口已核对为当前进程；WM_CLOSE 是合法取消消息。
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
                closed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(closed, "CredUI 对话框未在超时内出现");
        assert_eq!(
            handle
                .join()
                .expect("prompt thread must not panic")
                .unwrap(),
            None
        );
    }
}
