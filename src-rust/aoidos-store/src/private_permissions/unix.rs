//! POSIX 私有文件权限。

use std::path::Path;

use crate::error::{Result, StoreError};

/// 设置 0600：只允许当前用户读取和写入。
pub(crate) fn restrict_to_current_user(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .map_err(StoreError::from_io)?
        .permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions).map_err(StoreError::from_io)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn restricts_file_to_owner_read_and_write() {
        let dir = crate::private_permissions::temp_test_dir("private-mode");
        let file = dir.join("secret");
        std::fs::write(&file, b"previous data").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&file, permissions).unwrap();

        crate::atomic::write_atomic_private(&file, b"secret").unwrap();

        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"secret");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
