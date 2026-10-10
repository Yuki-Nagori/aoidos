//! Windows 平台实现：降级凭据文件的 DACL 收紧与 `CredUI` 原生密钥输入。

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use super::NativePromptText;
use aoidos_store::error::{Result, StoreError};

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::Credentials::{
    CREDUI_FLAGS_ALWAYS_SHOW_UI, CREDUI_FLAGS_DO_NOT_PERSIST, CREDUI_FLAGS_GENERIC_CREDENTIALS,
    CREDUI_FLAGS_KEEP_USERNAME, CREDUI_INFOW, CredUIPromptForCredentialsW,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

// TOKEN_QUERY / TOKEN_USER 的常量定义挂在未启用的 feature 门后；
// 用 winnt.h 同值常量（TOKEN_QUERY=8, TOKEN_USER=1），不为此拉宽特性面。
const TOKEN_QUERY: u32 = 8;
const TOKEN_USER: u32 = 1;

// ---- 降级文件权限：受保护 DACL，仅当前用户。----

/// 把 `path` 的 DACL 替换为仅当前用户（受保护 DACL，不继承任何继承 ACE）。
///
/// # Errors
/// 取用户 SID 或应用 DACL 失败时返回 store 域 `io` 错误（失败时私有写入方不写正文或发布文件）。
pub(crate) fn restrict_to_current_user(path: &Path) -> Result<()> {
    // SAFETY: GetCurrentProcess 无前置条件，返回恒有效的伪句柄。
    let sid = sid_from_process(unsafe { GetCurrentProcess() });
    if sid.is_null() {
        return Err(StoreError::Io {
            code: "io",
            source: io::Error::other("resolve current user sid failed"),
        });
    }
    let result = apply_owner_dacl(path, sid);
    // SAFETY: sid 来自 sid_from_process 的独立 LocalAlloc，ACL API 只借用；
    // 无论成功或失败都在此恰好释放一次。
    unsafe {
        windows_sys::Win32::Foundation::LocalFree(sid);
    }
    result
}

/// 从进程句柄取主令牌并提取用户 SID。
/// 返回裸指针（LocalFree 句柄）由调用方释放；[`apply_owner_dacl`] 只借用；失败返回 null。
fn sid_from_process(process: HANDLE) -> *mut core::ffi::c_void {
    // SAFETY: OpenProcessToken 只写 token 指针；process 由调用方保证有效
    //（生产路径是 GetCurrentProcess 伪句柄，恒有效）。FFI 出参用裸引用
    //（&raw mut）而非 &mut：不给跨边界写入挂 noalias 义务。
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &raw mut token) == 0 {
            return std::ptr::null_mut();
        }
        sid_from_token(token)
    }
}

/// 从已打开的令牌提取用户 SID；复制到 `LocalAlloc` 独立分配——不能直接返回
/// `GetTokenInformation` 缓冲内的指针，缓冲随 Vec drop 释放，悬垂 SID 会让后续
/// ACL 调用间歇性报 1332 / 1336（use-after-free）。
fn sid_from_token(token: HANDLE) -> *mut core::ffi::c_void {
    // SAFETY: token 由 sid_from_process 成功打开；两次 GetTokenInformation
    // 先探尺寸再读入；CloseHandle 与打开一一配对；TOKEN_USER 首字段是 PSID，
    // 以指针实际宽度防御短缓冲后再解引用；CopySid 目标为等长 LocalAlloc。
    unsafe {
        let mut needed = 0u32;
        GetTokenInformation(
            token,
            TOKEN_USER as i32,
            std::ptr::null_mut(),
            0,
            &raw mut needed,
        );
        let mut buffer = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TOKEN_USER as i32,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        );
        windows_sys::Win32::Foundation::CloseHandle(token);
        if ok == 0 || needed < std::mem::size_of::<*mut core::ffi::c_void>() as u32 {
            return std::ptr::null_mut();
        }
        // TOKEN_USER { User: SID_AND_ATTRIBUTES { Sid: PSID } }：首字段是指针。
        let token_sid = std::ptr::read_unaligned(buffer.as_ptr().cast::<*mut core::ffi::c_void>());
        if token_sid.is_null() {
            return std::ptr::null_mut();
        }
        // GetSidLength 需要有效 SID；复制到独立分配，生命周期与返回值绑定。
        let length = windows_sys::Win32::Security::GetLengthSid(token_sid.cast());
        let owned = windows_sys::Win32::System::Memory::LocalAlloc(
            windows_sys::Win32::System::Memory::LMEM_ZEROINIT,
            length as usize,
        );
        if owned.is_null() {
            return std::ptr::null_mut();
        }
        if windows_sys::Win32::Security::CopySid(length, owned.cast(), token_sid.cast()) == 0 {
            windows_sys::Win32::Foundation::LocalFree(owned);
            return std::ptr::null_mut();
        }
        owned
    }
}

/// 用给定 SID 构造「仅当前用户」的受保护 DACL 并应用到 `path`。
/// 拆成独立函数是为了让两个 Win32 失败分支可用构造性输入真实触发
///（非法 SID / 不存在的路径），不必为测试注入整个 FFI 门面。
fn apply_owner_dacl(path: &Path, sid: *const core::ffi::c_void) -> Result<()> {
    // SAFETY: EXPLICIT_ACCESS_W 是全零合法的 POD，zeroed 后逐字段显式填齐。
    let mut explicit: EXPLICIT_ACCESS_W = unsafe { std::mem::zeroed() };
    explicit.grfAccessPermissions = windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    explicit.grfAccessMode = SET_ACCESS;
    explicit.grfInheritance = 0;
    // 手工填 Trustee（TRUSTEE_IS_SID=0 + 二进制 SID）：不用
    // BuildExplicitAccessWithNameW——它生成的 form 是 NAME，把 SID 串当
    // 账户名走 LookupAccountName，本地化环境报 1332。
    explicit.Trustee.ptstrName = sid.cast_mut().cast();
    explicit.Trustee.TrusteeForm = 0; // TRUSTEE_IS_SID
    // SAFETY: explicit 已全字段填齐；SetEntriesInAclW 读 Trustee 并写出新 ACL；
    // SetNamedSecurityInfoW 读 NUL 结尾的宽路径与 ACL；LocalFree 与
    // SetEntriesInAclW 的分配一一配对。
    unsafe {
        let mut acl: *mut ACL = std::ptr::null_mut();
        // 单条允许 ACE：传入旧 ACL 为空即不合并继承项，受保护 DACL 由此产生。
        if SetEntriesInAclW(1, &raw const explicit, std::ptr::null(), &raw mut acl) != ERROR_SUCCESS
        {
            return Err(StoreError::Io {
                code: "io",
                source: io::Error::other("build owner-only dacl failed"),
            });
        }
        let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let set_result = SetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl.cast(),
            std::ptr::null_mut(),
        );
        windows_sys::Win32::Foundation::LocalFree(acl.cast());
        if set_result != ERROR_SUCCESS {
            return Err(StoreError::Io {
                code: "io",
                source: io::Error::other("apply owner-only dacl failed"),
            });
        }
    }
    Ok(())
}

/// 测试断言：DACL 恒等于仅一条允许 ACE（当前用户），且不透出明文。
/// 用 icacls 的输出做结构性断言：无组授权、有当前用户完全控制。
#[cfg(test)]
pub(crate) fn assert_owner_only(path: &Path) {
    let output = std::process::Command::new("icacls")
        .arg(path)
        .output()
        .expect("icacls runs");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !text.contains("Everyone") && !text.contains("BUILTIN\\Users:"),
        "icacls 输出含组授权：{text}"
    );
    assert!(text.contains(":(F)"), "应有当前用户完全控制：{text}");
}

// ---- 原生密钥输入：CredUI 密码对话框。----
//
// 返回 `Ok(Some(key))`（用户确认）/ `Ok(None)`（用户取消，保留旧值）；
// `DO_NOT_PERSIST` 禁止 OS 把输入写入凭据管理器（保存路径由调用方显式
// 选择 CredentialStore）。明文只存在于返回值中，不进日志 / Debug。

/// `CredUI` 对话框标题；取消路径直测按它查找窗口。
const PROMPT_CAPTION: &str = "Aoidos API 密钥";

/// 弹出 `CredUI` 密码对话框。
///
/// # Errors
/// `CredUI` 初始化或调用失败（非用户取消）时返回 store 域 `io` 错误。
pub fn prompt_native_key(provider_label: &str, text: &NativePromptText) -> Result<Option<String>> {
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
    // 明文生命周期只到本函数：缓冲用后即清，栈上残留不外泄（返回值由
    // SecretString 接管并自带 drop 清零）。
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

/// Windows 真实确认 / 取消验证：只给当前测试进程的 CredUI 窗口发合成输入。
#[cfg(feature = "native-smoke")]
pub fn verify_native_input() -> Result<()> {
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
    use crate::temp_test_dir;
    use std::time::Duration;
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

    #[test]
    fn sid_extraction_rejects_dead_handles() {
        // 空进程句柄 / 空令牌：提取返回 null（restrict 的 null 分支据此报错）。
        assert!(sid_from_process(std::ptr::null_mut()).is_null());
        assert!(sid_from_token(std::ptr::null_mut()).is_null());
    }

    #[test]
    fn owner_dacl_reports_real_failures() {
        // 非法 SID（revision 0 的全零缓冲）：SetEntriesInAclW 拒绝。
        let bogus_sid = [0u8; 8];
        let dir = temp_test_dir("dacl-bad-sid");
        std::fs::write(dir.join("f"), b"x").unwrap();
        let target = dir.join("f");
        assert!(apply_owner_dacl(&target, bogus_sid.as_ptr().cast::<core::ffi::c_void>()).is_err());
        // 合法 SID + 不存在的路径：SetNamedSecurityInfoW 拒绝。
        // SAFETY: GetCurrentProcess 无前置条件，返回恒有效的伪句柄。
        let sid = sid_from_process(unsafe { GetCurrentProcess() });
        assert!(!sid.is_null());
        let missing = dir.join("missing").join("f");
        assert!(apply_owner_dacl(&missing, sid).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restrict_current_user_dacl_is_owner_only() {
        // 成功路径：真实文件的结构断言（credentials 的降级测试亦覆盖）。
        let dir = temp_test_dir("dacl-ok");
        let target = dir.join("credentials.json");
        std::fs::write(&target, b"x").unwrap();
        restrict_to_current_user(&target).unwrap();
        assert_owner_only(&target);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cred_ui_result_maps_all_three_outcomes() {
        // 确认：截到 NUL 为止转 String。
        let mut buffer: Vec<u16> = "sk-1234".encode_utf16().collect();
        buffer.push(0);
        buffer.extend_from_slice(&[0x7f; 8]);
        assert_eq!(
            map_cred_ui_result(windows_sys::Win32::Foundation::NO_ERROR, &buffer).unwrap(),
            Some(String::from("sk-1234"))
        );
        // 无 NUL 的满缓冲按全长消费。
        assert_eq!(
            map_cred_ui_result(windows_sys::Win32::Foundation::NO_ERROR, &[0x41; 4]).unwrap(),
            Some(String::from("AAAA"))
        );
        // 用户取消：Ok(None)，不是错误。
        assert_eq!(
            map_cred_ui_result(windows_sys::Win32::Foundation::ERROR_CANCELLED, &buffer).unwrap(),
            None
        );
        // 其余失败码：store 域 io 错误，诊断不含输入缓冲。
        let error = map_cred_ui_result(1385, &buffer).unwrap_err();
        assert_eq!(error.code(), "io");
    }

    /// 取消路径直测：弹出真实 CredUI 对话框，以 WM_CLOSE 关闭等价用户取消，
    /// 断言映射为 `Ok(None)`（命令层据此保留旧值）。这是 Windows 原生输入
    /// 路径的实际验证记录（019 验收）；无交互窗口站的环境会失败并暴露。
    /// 轮询窗口 60s：全量测试并行时对话框线程可能被调度饥饿（实测 10s 不够）。
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
        let result = handle.join().expect("prompt thread must not panic");
        assert_eq!(result.expect("cancel is not an error"), None);
    }
}
