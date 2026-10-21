//! 周目隔离与恢复共用的有界文件枚举与身份原语。

use std::{ffi::OsString, fs, path::Path};

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

/// 有界读取的目录快照；`complete == false` 表示还有条目未返回，调用方不得据此执行删除。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedDirectoryListing {
    pub entries: Vec<OsString>,
    pub complete: bool,
}

/// 列出数据根内目录的直接条目名称，不递归，并明确报告是否触及条目上限。
///
/// 返回名称按平台原生形式保留；领域层负责筛选允许的文件名。超出上限时只返回前 `limit`
/// 项并设 `complete` 为 `false`，调用方必须停止删除或要求更大的明确预算，不能把部分结果当完整清单。
/// 这是路径式有界遍历，不是原子目录快照；组件检查与目录读取之间仍有竞态窗口。调用方须在应用
/// 生命周期锁内使用可信数据根，并在后续移动前重新核验每个目标对象的类型与身份。未筛选的条目
/// 仅占用扫描预算，不会因链接、子目录、特殊文件或非 UTF-8 名称影响其他对象的处理。
///
/// # Errors
/// 目录路径越界或读取失败时返回存储错误。
pub fn list_directory_entries(
    root: &Path,
    relative_directory: &Path,
    limit: usize,
) -> Result<BoundedDirectoryListing> {
    validate_relative_path(relative_directory)?;
    validate_path_chain(root, relative_directory)?;
    let directory = root.join(relative_directory);
    if !fs::symlink_metadata(&directory)
        .map_err(StoreError::from_io)?
        .is_dir()
    {
        return Err(StoreError::InvalidPath("expected a directory".into()));
    }

    let mut entries = Vec::with_capacity(limit.min(1024));
    let mut complete = true;
    for item in fs::read_dir(&directory).map_err(StoreError::from_io)? {
        let item = item.map_err(StoreError::from_io)?;
        if entries.len() == limit {
            complete = false;
            break;
        }
        entries.push(item.file_name());
    }
    validate_path_chain(root, relative_directory)?;
    entries.sort_unstable();
    Ok(BoundedDirectoryListing { entries, complete })
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
    let (_, identity) = verified_identity(root, relative_path)?;
    Ok(encode_identity(identity))
}

/// 返回普通文件的身份摘要；目录或其他对象类型会被拒绝。
///
/// # Errors
/// 路径无法核验或目标不是普通文件时返回存储错误。
pub fn file_identity_hash(root: &Path, relative_path: &Path) -> Result<String> {
    let (kind, identity) = verified_identity(root, relative_path)?;
    if kind != ObjectKind::File {
        return Err(StoreError::InvalidPath("expected a regular file".into()));
    }
    Ok(encode_identity(identity))
}

/// 返回普通目录的身份摘要；文件或其他对象类型会被拒绝。
///
/// # Errors
/// 路径无法核验或目标不是普通目录时返回存储错误。
pub fn directory_identity_hash(root: &Path, relative_path: &Path) -> Result<String> {
    let (kind, identity) = verified_identity(root, relative_path)?;
    if kind != ObjectKind::Directory {
        return Err(StoreError::InvalidPath("expected a directory".into()));
    }
    Ok(encode_identity(identity))
}

fn verified_identity(root: &Path, relative_path: &Path) -> Result<(ObjectKind, ObjectId)> {
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

    Ok((kind, identity))
}

fn encode_identity(identity: ObjectId) -> String {
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
    encode_sha256(digest.finalize())
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
            file_identity_hash(&fixture.0, Path::new("first.jsonl")).unwrap(),
            identity
        );
        assert!(matches!(
            directory_identity_hash(&fixture.0, Path::new("first.jsonl")),
            Err(StoreError::InvalidPath(_))
        ));
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
        assert_eq!(
            directory_identity_hash(&fixture.0, Path::new("memory")).unwrap(),
            identity
        );
        assert!(matches!(
            file_identity_hash(&fixture.0, Path::new("memory")),
            Err(StoreError::InvalidPath(_))
        ));
        fs::rename(&first, &moved).unwrap();
        assert_eq!(
            directory_identity_hash(&fixture.0, Path::new("quarantined-memory")).unwrap(),
            identity
        );
    }

    #[test]
    fn directory_listing_reports_when_its_bound_truncates_results() {
        let fixture = Fixture::new();
        let nested = fixture.0.join("files");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("first"), b"1").unwrap();
        fs::write(nested.join("second"), b"2").unwrap();

        let exact = list_directory_entries(&fixture.0, Path::new("files"), 2).unwrap();
        assert_eq!(exact.entries.len(), 2);
        assert!(exact.complete);

        let listing = list_directory_entries(&fixture.0, Path::new("files"), 1).unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert!(!listing.complete);
    }

    #[cfg(unix)]
    #[test]
    fn directory_listing_preserves_unrelated_symlink_names_for_domain_filtering() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let directory = fixture.0.join("files");
        fs::create_dir(&directory).unwrap();
        fs::write(fixture.0.join("target"), b"content").unwrap();
        symlink(fixture.0.join("target"), directory.join("link")).unwrap();

        let listing = list_directory_entries(&fixture.0, Path::new("files"), 16).unwrap();
        assert_eq!(listing.entries, ["link"]);
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
