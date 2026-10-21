//! 周目隔离与恢复共用的文件系统身份原语。

use std::{fs, path::Path};

#[cfg(any(windows, not(any(unix, windows))))]
use std::io;

use sha2::{Digest, Sha256};

use crate::error::{Result, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectId {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    WindowsLow { volume: u32, index: u64 },
    #[cfg(windows)]
    WindowsHigh { volume: u64, id: u128 },
}

/// 返回数据根目录下普通文件或目录的本机文件系统身份摘要。
///
/// 相对路径只能包含普通路径组件；根目录以内的符号链接和特殊文件会被拒绝。同一对象在同一台
/// 机器上改名后摘要保持不变，可用于删除 journal 确认隔离前后的对象一致。摘要不是内容校验；
/// 对象删除后文件系统可能复用身份，因此只用于单机短期恢复流程。调用方仍须在移动后重新核验。
/// 路径组件检查与文件系统调用不是原子操作；调用方必须在应用生命周期锁内使用，并将数据根视为可信。
///
/// # Errors
/// 路径不存在、越出数据根、路径组件是符号链接 / 特殊文件，或无法读取对象身份时返回存储错误。
pub fn identity_hash(root: &Path, relative_path: &Path) -> Result<String> {
    validate_relative_path(relative_path)?;
    validate_path_chain(root, relative_path)?;
    let (kind, identity) = platform_identity(&root.join(relative_path))?;
    validate_path_chain(root, relative_path)?;
    let (kind_after, identity_after) = platform_identity(&root.join(relative_path))?;
    if kind != kind_after || identity != identity_after {
        return Err(StoreError::InvalidPath(
            "filesystem identity changed".into(),
        ));
    }

    let mut digest = Sha256::new();
    match identity {
        #[cfg(unix)]
        ObjectId::Unix { device, inode } => {
            digest.update([0]);
            digest.update(device.to_le_bytes());
            digest.update(inode.to_le_bytes());
        }
        #[cfg(windows)]
        ObjectId::WindowsLow { volume, index } => {
            digest.update([1]);
            digest.update(volume.to_le_bytes());
            digest.update(index.to_le_bytes());
        }
        #[cfg(windows)]
        ObjectId::WindowsHigh { volume, id } => {
            digest.update([2]);
            digest.update(volume.to_le_bytes());
            digest.update(id.to_le_bytes());
        }
    }
    Ok(encode_sha256(digest.finalize()))
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(StoreError::InvalidPath(path.to_string_lossy().into_owned()));
    }
    Ok(())
}

fn validate_path_chain(root: &Path, relative_path: &Path) -> Result<()> {
    let target = root.join(relative_path);
    let mut current = root.to_owned();
    for component in relative_path.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(StoreError::InvalidPath(
                relative_path.to_string_lossy().into_owned(),
            ));
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(StoreError::from_io)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(StoreError::InvalidPath("symbolic link in data path".into()));
        }
        if current != target && !file_type.is_dir() {
            return Err(StoreError::InvalidPath(
                "non-directory parent in data path".into(),
            ));
        }
        if current == target && !(file_type.is_file() || file_type.is_dir()) {
            return Err(StoreError::InvalidPath(
                "expected a regular file or directory".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn platform_identity(path: &Path) -> Result<(ObjectKind, ObjectId)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path).map_err(StoreError::from_io)?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_file() {
        ObjectKind::File
    } else if file_type.is_dir() {
        ObjectKind::Directory
    } else {
        return Err(StoreError::InvalidPath(
            "expected a regular file or directory".into(),
        ));
    };
    Ok((
        kind,
        ObjectId::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
    ))
}

#[cfg(windows)]
fn platform_identity(path: &Path) -> Result<(ObjectKind, ObjectId)> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
            OPEN_EXISTING,
        },
    };

    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `path_wide` 以 NUL 结尾且在调用期间有效；其余指针参数为空指针，符合 Win32 可选参数约定。
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(StoreError::from_io(io::Error::last_os_error()));
    }
    // SAFETY: `CreateFileW` 返回有效句柄，且此处将所有权唯一转交给 `File`，之后由其关闭。
    let file = unsafe { fs::File::from_raw_handle(handle.cast()) };
    // SAFETY: 该结构体是纯输出缓冲区，零初始化后由 Win32 API 完整填充。
    let mut basic: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `handle` 仍由 `file` 持有且有效，`basic` 是可写的正确类型缓冲区。
    if unsafe { GetFileInformationByHandle(handle, &mut basic) } == 0 {
        return Err(StoreError::from_io(io::Error::last_os_error()));
    }
    if basic.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(StoreError::InvalidPath("reparse point in data path".into()));
    }
    let kind = if basic.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        ObjectKind::Directory
    } else {
        ObjectKind::File
    };

    // SAFETY: 该结构体是纯输出缓冲区，零初始化后由 Win32 API 完整填充。
    let mut high_resolution: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: `handle` 仍有效；输出指针指向可写的 `FILE_ID_INFO`，长度与结构体大小一致。
    let identity = if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut high_resolution as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } != 0
    {
        ObjectId::WindowsHigh {
            volume: high_resolution.VolumeSerialNumber,
            id: u128::from_le_bytes(high_resolution.FileId.Identifier),
        }
    } else {
        ObjectId::WindowsLow {
            volume: basic.dwVolumeSerialNumber,
            index: (u64::from(basic.nFileIndexHigh) << 32) | u64::from(basic.nFileIndexLow),
        }
    };
    drop(file);
    Ok((kind, identity))
}

#[cfg(not(any(unix, windows)))]
fn platform_identity(_path: &Path) -> Result<(ObjectKind, ObjectId)> {
    Err(StoreError::from_io(io::Error::new(
        io::ErrorKind::Unsupported,
        "filesystem identity is unsupported on this target",
    )))
}

fn encode_sha256(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(71);
    encoded.push_str("sha256:");
    for byte in bytes.as_ref() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, sync::atomic::AtomicU32};

    use super::*;

    static NEXT_FIXTURE: AtomicU32 = AtomicU32::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aoidos-filesystem-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn identity_is_stable_across_rename_and_differs_between_files() {
        let fixture = Fixture::new();
        let first = fixture.0.join("first.jsonl");
        let second = fixture.0.join("second.jsonl");
        fs::write(&first, b"same bytes").unwrap();
        fs::write(&second, b"same bytes").unwrap();

        let identity = identity_hash(&fixture.0, Path::new("first.jsonl")).unwrap();
        assert_eq!(
            identity_hash(&fixture.0, Path::new("first.jsonl")).unwrap(),
            identity
        );
        assert_ne!(
            identity_hash(&fixture.0, Path::new("second.jsonl")).unwrap(),
            identity
        );

        let moved = fixture.0.join("moved.jsonl");
        fs::rename(&first, &moved).unwrap();
        assert_eq!(
            identity_hash(&fixture.0, Path::new("moved.jsonl")).unwrap(),
            identity
        );
    }

    #[test]
    fn identity_accepts_directories() {
        let fixture = Fixture::new();
        let first = fixture.0.join("memory");
        let moved = fixture.0.join("quarantined-memory");
        fs::create_dir(&first).unwrap();

        let identity = identity_hash(&fixture.0, Path::new("memory")).unwrap();
        fs::rename(&first, &moved).unwrap();
        assert_eq!(
            identity_hash(&fixture.0, Path::new("quarantined-memory")).unwrap(),
            identity
        );
    }

    #[cfg(unix)]
    #[test]
    fn identity_rejects_link_components_and_paths_outside_the_root() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let target = fixture.0.join("target");
        fs::write(&target, b"content").unwrap();
        symlink(&target, fixture.0.join("link")).unwrap();
        symlink(&fixture.0, fixture.0.join("linked-dir")).unwrap();

        assert!(matches!(
            identity_hash(&fixture.0, Path::new("link")),
            Err(StoreError::InvalidPath(_))
        ));
        assert!(matches!(
            identity_hash(&fixture.0, Path::new("linked-dir/target")),
            Err(StoreError::InvalidPath(_))
        ));
        assert!(matches!(
            identity_hash(&fixture.0, Path::new("../target")),
            Err(StoreError::InvalidPath(_))
        ));
    }
}
