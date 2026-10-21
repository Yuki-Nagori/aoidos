//! Windows 私有文件权限：受保护 DACL 只授权当前用户。

use std::{io, os::windows::ffi::OsStrExt, path::Path};

use crate::error::{Result, StoreError};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const TOKEN_QUERY: u32 = 8;
const TOKEN_USER: u32 = 1;

/// 将文件 DACL 替换为仅当前用户可访问的受保护 ACL。
pub(crate) fn restrict_to_current_user(path: &Path) -> Result<()> {
    // SAFETY: GetCurrentProcess 返回恒有效的伪句柄；sid 单独分配并在此释放。
    let sid = sid_from_process(unsafe { GetCurrentProcess() });
    if sid.is_null() {
        return Err(io_error("resolve current user sid failed"));
    }
    let result = apply_owner_dacl(path, sid);
    // SAFETY: sid 由 sid_from_process 使用 LocalAlloc 分配，ACL API 只借用它。
    unsafe {
        windows_sys::Win32::Foundation::LocalFree(sid);
    }
    result
}

fn sid_from_process(process: windows_sys::Win32::Foundation::HANDLE) -> *mut core::ffi::c_void {
    // SAFETY: 进程句柄有效；成功打开的 token 在 sid_from_token 中关闭。
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &raw mut token) == 0 {
            return std::ptr::null_mut();
        }
        sid_from_token(token)
    }
}

fn sid_from_token(token: windows_sys::Win32::Foundation::HANDLE) -> *mut core::ffi::c_void {
    // SAFETY: token 由调用方成功打开；SID 被复制到独立 LocalAlloc，避免引用 Vec 缓冲。
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
        let token_sid = std::ptr::read_unaligned(buffer.as_ptr().cast::<*mut core::ffi::c_void>());
        if token_sid.is_null() {
            return std::ptr::null_mut();
        }
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

fn apply_owner_dacl(path: &Path, sid: *const core::ffi::c_void) -> Result<()> {
    // SAFETY: 全零结构体随后逐字段填好，Trustee 指向调用方持有的 SID。
    let mut access: EXPLICIT_ACCESS_W = unsafe { std::mem::zeroed() };
    access.grfAccessPermissions = windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    access.grfAccessMode = SET_ACCESS;
    access.grfInheritance = 0;
    access.Trustee.ptstrName = sid.cast_mut().cast();
    access.Trustee.TrusteeForm = 0;

    // SAFETY: ACL API 只借用 access / path 并分配 ACL；所有权在此通过 LocalFree 释放。
    unsafe {
        let mut acl: *mut ACL = std::ptr::null_mut();
        if SetEntriesInAclW(1, &raw const access, std::ptr::null(), &raw mut acl)
            != windows_sys::Win32::Foundation::ERROR_SUCCESS
        {
            return Err(io_error("build owner-only dacl failed"));
        }
        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let status = SetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl.cast(),
            std::ptr::null_mut(),
        );
        windows_sys::Win32::Foundation::LocalFree(acl.cast());
        if status != windows_sys::Win32::Foundation::ERROR_SUCCESS {
            return Err(io_error("apply owner-only dacl failed"));
        }
    }
    Ok(())
}

fn io_error(message: &'static str) -> StoreError {
    StoreError::Io {
        code: "io",
        source: io::Error::other(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_sid_and_missing_path() {
        let dir = crate::private_permissions::temp_test_dir("private-dacl-invalid");
        std::fs::write(dir.join("file"), b"secret").unwrap();
        assert!(apply_owner_dacl(&dir.join("file"), [0u8; 8].as_ptr().cast()).is_err());

        // SAFETY: GetCurrentProcess 返回恒有效的伪句柄；SID 在断言后释放。
        let sid = sid_from_process(unsafe { GetCurrentProcess() });
        assert!(!sid.is_null());
        assert!(apply_owner_dacl(&dir.join("missing"), sid).is_err());
        // SAFETY: sid 由 sid_from_process 使用 LocalAlloc 分配。
        unsafe { windows_sys::Win32::Foundation::LocalFree(sid) };
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writes_a_protected_owner_only_dacl() {
        let dir = crate::private_permissions::temp_test_dir("private-dacl-owner");
        let file = dir.join("credentials.json");
        std::fs::write(&file, b"inherited or otherwise broad prior permissions").unwrap();
        crate::atomic::write_atomic_private(&file, b"secret").unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), b"secret");
        assert_protected_owner_only_dacl(&file);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn assert_protected_owner_only_dacl(path: &Path) {
        use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;
        use windows_sys::Win32::Security::{
            ACCESS_ALLOWED_ACE, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
            GetSecurityDescriptorControl, SE_DACL_PROTECTED,
        };
        use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

        let current_sid = sid_from_process(unsafe { GetCurrentProcess() });
        assert!(!current_sid.is_null());
        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut dacl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: path 为 NUL 结尾 UTF-16；输出指针均指向有效变量；返回的
        // security descriptor 由 LocalFree 释放，DACL 仅在 descriptor 存活期间借用。
        let status = unsafe {
            GetNamedSecurityInfoW(
                wide_path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut dacl,
                std::ptr::null_mut(),
                &raw mut descriptor,
            )
        };
        assert_eq!(status, windows_sys::Win32::Foundation::ERROR_SUCCESS);
        assert!(!descriptor.is_null() && !dacl.is_null());

        let mut control = 0u16;
        let mut revision = 0u32;
        // SAFETY: descriptor 由 GetNamedSecurityInfoW 成功分配，输出变量有效。
        assert_ne!(
            unsafe {
                GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision)
            },
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0, "DACL 必须禁止继承");
        // SAFETY: dacl 指向 descriptor 持有的有效 ACL。
        let acl = unsafe { &*dacl };
        assert_eq!(acl.AceCount, 1, "DACL 只能含一条授权 ACE");
        let mut ace = std::ptr::null_mut();
        // SAFETY: ACL 恰有一个 ACE，索引 0 有效；返回指针由 descriptor 所有。
        assert_ne!(unsafe { GetAce(dacl, 0, &raw mut ace) }, 0);
        assert!(!ace.is_null());
        // SAFETY: 该条目由 SetEntriesInAclW 构造为 ACCESS_ALLOWED_ACE。
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        assert_eq!(allowed.Header.AceType, 0, "唯一 ACE 必须是允许项");
        assert_eq!(allowed.Mask, FILE_ALL_ACCESS);
        let ace_sid = std::ptr::addr_of!(allowed.SidStart).cast_mut().cast();
        // SAFETY: ACE 保存在有效 ACL 内且包含完整 SID；current_sid 独立分配有效。
        assert_ne!(
            unsafe { EqualSid(ace_sid, current_sid) },
            0,
            "唯一授权对象必须是当前用户"
        );

        // SAFETY: SID 与安全描述符均由本函数分配，且 API 调用已完成。
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(current_sid);
            windows_sys::Win32::Foundation::LocalFree(descriptor);
        }
    }
}
