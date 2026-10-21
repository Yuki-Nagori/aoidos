//! 周目隔离与恢复共用的有界文件枚举与身份原语。

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

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

/// 单次隔离树清理结果；`complete == false` 表示仍有条目待后续批次处理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupBatch {
    pub visited_entries: usize,
    pub complete: bool,
    /// 深度优先遍历栈，供调用方持久化后在下一批继续。
    pub continuation: Option<Vec<u8>>,
}

/// 可按身份隔离的文件系统对象类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineKind {
    File,
    Directory,
}

/// 单个隔离项的幂等执行结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineResult {
    Moved,
    AlreadyMoved,
    SkippedAbsent,
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

/// 按冻结身份将单个文件或目录移入同一数据根内的隔离目录。
///
/// 目标父目录按需逐级创建并拒绝链接；移动前比较源对象身份，移动后再次比较隔离对象身份。
/// 重试时仅当源缺席且隔离项身份匹配才视为已完成；两边同时存在、身份变化或路径异常均失败关闭。
/// `expected_identity == None` 只接受“文件与隔离目标均缺席”，用于冻结时不存在的空记忆目录。
/// 此操作只移动目录项，不递归遍历目录内容；源与目标都位于可信数据根下。
///
/// # Errors
/// 路径越界、对象类型 / 身份变化、目标已占用或重命名 / 目录同步失败时返回存储错误。
pub fn quarantine_entry(
    root: &Path,
    source: &Path,
    quarantine: &Path,
    kind: QuarantineKind,
    expected_identity: Option<&str>,
) -> Result<QuarantineResult> {
    validate_relative_path(source)?;
    validate_relative_path(quarantine)?;
    if source == quarantine {
        return Err(StoreError::InvalidPath(
            "source equals quarantine path".into(),
        ));
    }

    ensure_parent_directories(root, quarantine)?;
    validate_path_chain(
        root,
        quarantine
            .parent()
            .ok_or_else(|| StoreError::InvalidPath("quarantine parent is missing".into()))?,
    )?;

    let source_path = root.join(source);
    let quarantine_path = root.join(quarantine);
    let source_identity = optional_identity(root, source, kind)?;
    let quarantine_identity = optional_identity(root, quarantine, kind)?;
    let Some(expected_identity) = expected_identity else {
        if kind != QuarantineKind::Directory
            || source_identity.is_some()
            || quarantine_identity.is_some()
        {
            return Err(StoreError::InvalidPath("expected absent directory".into()));
        }
        return Ok(QuarantineResult::SkippedAbsent);
    };

    match (source_identity.as_deref(), quarantine_identity.as_deref()) {
        (Some(source_id), None) if source_id == expected_identity => {
            fs::rename(&source_path, &quarantine_path).map_err(StoreError::from_io)?;
            crate::atomic::sync_parent(&source_path)?;
            crate::atomic::sync_parent(&quarantine_path)?;
            if optional_identity(root, quarantine, kind)?.as_deref() != Some(expected_identity) {
                return Err(StoreError::InvalidPath(
                    "quarantine identity changed".into(),
                ));
            }
            Ok(QuarantineResult::Moved)
        }
        (None, Some(quarantine_id)) if quarantine_id == expected_identity => {
            Ok(QuarantineResult::AlreadyMoved)
        }
        (Some(_), Some(_)) => Err(StoreError::InvalidPath(
            "source and quarantine both exist".into(),
        )),
        (Some(_), None) | (None, Some(_)) => Err(StoreError::InvalidPath(
            "filesystem identity changed".into(),
        )),
        (None, None) => Err(StoreError::from_io(std::io::Error::from(
            std::io::ErrorKind::NotFound,
        ))),
    }
}

/// 有界清除一个已隔离的文件或目录树，不跟随符号链接。
///
/// 目录按深度优先逐项处理，每次只读取一个直接子项；预算耗尽时返回编码后的路径栈，调用方
/// 必须持久化并在下批传回，否则深目录可能无法取得进展。顶层对象若仍存在，必须与冻结身份
/// 一致；缺席表示先前批次已完成。`expected_identity == None` 只接受原本不存在且仍不存在的目录。
///
/// # Errors
/// 身份变化、链接或特殊文件、路径越界、非空目录竞争及文件系统操作失败时返回错误。
pub fn purge_quarantined_tree_batch(
    root: &Path,
    quarantine: &Path,
    kind: QuarantineKind,
    expected_identity: Option<&str>,
    continuation: Option<&[u8]>,
    limit: usize,
) -> Result<CleanupBatch> {
    validate_relative_path(quarantine)?;
    if limit == 0 {
        return Err(StoreError::InvalidPath(
            "cleanup limit must be positive".into(),
        ));
    }
    if kind == QuarantineKind::File && continuation.is_some() {
        return Err(StoreError::InvalidPath(
            "file cleanup cannot carry a directory cursor".into(),
        ));
    }

    let top_identity = optional_identity(root, quarantine, kind)?;
    let Some(expected_identity) = expected_identity else {
        if kind != QuarantineKind::Directory || top_identity.is_some() {
            return Err(StoreError::InvalidPath(
                "unexpected quarantine object".into(),
            ));
        }
        return Ok(CleanupBatch {
            visited_entries: 0,
            complete: true,
            continuation: None,
        });
    };
    let Some(top_identity) = top_identity else {
        return Ok(CleanupBatch {
            visited_entries: 0,
            complete: true,
            continuation: None,
        });
    };
    if top_identity != expected_identity {
        return Err(StoreError::InvalidPath(
            "quarantine identity changed".into(),
        ));
    }

    if kind == QuarantineKind::File {
        remove_quarantined_file(root, quarantine)?;
        return Ok(CleanupBatch {
            visited_entries: 1,
            complete: true,
            continuation: None,
        });
    }

    let (_, root_identity) = platform_identity(&root.join(quarantine))?;
    let mut stack = match continuation {
        Some(bytes) => decode_cleanup_stack(quarantine, bytes)?,
        None => vec![quarantine.to_owned()],
    };
    let mut visited_entries = 0;
    while let Some(current) = stack.last() {
        if visited_entries >= limit {
            return Ok(CleanupBatch {
                visited_entries,
                complete: false,
                continuation: Some(encode_cleanup_stack(&stack)?),
            });
        }
        let metadata = match metadata_without_links(root, current) {
            Ok(metadata) => metadata,
            Err(StoreError::Io { ref source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                stack.pop();
                continue;
            }
            Err(error) => return Err(error),
        };
        visited_entries += 1;
        let (_, identity) = platform_identity(&root.join(current))?;
        if !same_filesystem(root_identity, identity) {
            return Err(StoreError::InvalidPath(
                "quarantine tree crosses a filesystem boundary".into(),
            ));
        }
        let file_type = metadata.file_type();
        if file_type.is_file() {
            remove_quarantined_file(root, current)?;
            stack.pop();
            continue;
        }
        if !file_type.is_dir() {
            return Err(StoreError::InvalidPath("special file in quarantine".into()));
        }

        let listing = list_directory_entries(root, current, 1)?;
        if let Some(name) = listing.entries.into_iter().next() {
            let mut child = current.clone();
            child.push(name);
            stack.push(child);
            continue;
        }

        validate_path_chain(root, current)?;
        fs::remove_dir(root.join(current)).map_err(StoreError::from_io)?;
        crate::atomic::sync_parent(&root.join(current))?;
        stack.pop();
    }

    Ok(CleanupBatch {
        visited_entries,
        complete: true,
        continuation: None,
    })
}

#[cfg(unix)]
const CLEANUP_STACK_MAGIC: &[u8; 6] = b"AOCS\x01U";
#[cfg(windows)]
const CLEANUP_STACK_MAGIC: &[u8; 6] = b"AOCS\x01W";
#[cfg(not(any(unix, windows)))]
const CLEANUP_STACK_MAGIC: &[u8; 6] = b"AOCS\x01X";
const MAX_CLEANUP_STACK_DEPTH: usize = 4096;
const MAX_CLEANUP_STACK_BYTES: usize = 262_144;

fn encode_cleanup_stack(stack: &[PathBuf]) -> Result<Vec<u8>> {
    if stack.is_empty() || stack.len() > MAX_CLEANUP_STACK_DEPTH {
        return Err(StoreError::InvalidPath(
            "cleanup stack depth is invalid".into(),
        ));
    }
    let mut bytes = CLEANUP_STACK_MAGIC.to_vec();
    bytes.extend_from_slice(
        &u32::try_from(stack.len())
            .map_err(|_| StoreError::InvalidPath("cleanup stack is too deep".into()))?
            .to_le_bytes(),
    );
    for path in stack {
        validate_relative_path(path)?;
        let encoded = crate::paths::encode_native_os_str(path.as_os_str())?;
        bytes.extend_from_slice(
            &u32::try_from(encoded.len())
                .map_err(|_| StoreError::InvalidPath("cleanup path is too long".into()))?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&encoded);
        if bytes.len() > MAX_CLEANUP_STACK_BYTES {
            return Err(StoreError::InvalidPath(
                "cleanup stack exceeds its size limit".into(),
            ));
        }
    }
    Ok(bytes)
}

fn decode_cleanup_stack(quarantine: &Path, bytes: &[u8]) -> Result<Vec<PathBuf>> {
    if bytes.len() > MAX_CLEANUP_STACK_BYTES || !bytes.starts_with(CLEANUP_STACK_MAGIC) {
        return Err(StoreError::InvalidPath("cleanup stack is malformed".into()));
    }
    let mut cursor = CLEANUP_STACK_MAGIC.len();
    let count = read_cleanup_u32(bytes, &mut cursor)? as usize;
    if count == 0 || count > MAX_CLEANUP_STACK_DEPTH {
        return Err(StoreError::InvalidPath(
            "cleanup stack depth is invalid".into(),
        ));
    }
    let mut stack = Vec::with_capacity(count);
    for _ in 0..count {
        let length = read_cleanup_u32(bytes, &mut cursor)? as usize;
        let end = cursor
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| StoreError::InvalidPath("cleanup stack is truncated".into()))?;
        let path = PathBuf::from(crate::paths::decode_native_os_str(&bytes[cursor..end])?);
        validate_relative_path(&path)?;
        if !path.starts_with(quarantine)
            || stack
                .last()
                .is_some_and(|parent: &PathBuf| path.parent() != Some(parent.as_path()))
        {
            return Err(StoreError::InvalidPath(
                "cleanup stack escaped its quarantine".into(),
            ));
        }
        stack.push(path);
        cursor = end;
    }
    if cursor != bytes.len() || stack.first().is_none_or(|path| path != quarantine) {
        return Err(StoreError::InvalidPath("cleanup stack is malformed".into()));
    }
    Ok(stack)
}

fn read_cleanup_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32> {
    let end = cursor
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| StoreError::InvalidPath("cleanup stack is truncated".into()))?;
    let value = u32::from_le_bytes(
        bytes[*cursor..end]
            .try_into()
            .map_err(|_| StoreError::InvalidPath("cleanup stack is malformed".into()))?,
    );
    *cursor = end;
    Ok(value)
}

fn same_filesystem(left: ObjectId, right: ObjectId) -> bool {
    #[cfg(unix)]
    {
        let (ObjectId::Unix { device: left, .. }, ObjectId::Unix { device: right, .. }) =
            (left, right);
        left == right
    }

    #[cfg(windows)]
    {
        let volume = |identity| match identity {
            ObjectId::WindowsLow { volume, .. } => u64::from(volume),
            ObjectId::WindowsHigh { volume, .. } => volume,
        };
        volume(left) == volume(right)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        false
    }
}

/// 只删除空隔离目录；非空时由调用方保留 journal 并停止推进。
///
/// # Errors
/// 符号链接、路径异常或目录操作失败时返回错误。
pub fn remove_empty_quarantine_directory(root: &Path, relative_path: &Path) -> Result<()> {
    validate_relative_path(relative_path)?;
    match metadata_without_links(root, relative_path) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            validate_path_chain(root, relative_path)?;
            fs::remove_dir(root.join(relative_path)).map_err(StoreError::from_io)?;
            crate::atomic::sync_parent(&root.join(relative_path))
        }
        Ok(_) => Err(StoreError::InvalidPath(
            "expected an empty quarantine directory".into(),
        )),
        Err(StoreError::Io { ref source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn remove_quarantined_file(root: &Path, relative_path: &Path) -> Result<()> {
    validate_path_chain(root, relative_path)?;
    let metadata = fs::symlink_metadata(root.join(relative_path)).map_err(StoreError::from_io)?;
    if !metadata.file_type().is_file() {
        return Err(StoreError::InvalidPath("expected a regular file".into()));
    }
    fs::remove_file(root.join(relative_path)).map_err(StoreError::from_io)?;
    crate::atomic::sync_parent(&root.join(relative_path))
}

fn metadata_without_links(root: &Path, relative_path: &Path) -> Result<fs::Metadata> {
    validate_relative_path(relative_path)?;
    if !validate_existing_parent_chain(root, relative_path)? {
        return Err(StoreError::from_io(std::io::Error::from(
            std::io::ErrorKind::NotFound,
        )));
    }
    let target = root.join(relative_path);
    let metadata = fs::symlink_metadata(&target).map_err(StoreError::from_io)?;
    if metadata.file_type().is_symlink() {
        return Err(StoreError::InvalidPath(
            "symbolic link in quarantine".into(),
        ));
    }
    validate_path_chain(root, relative_path)?;
    Ok(metadata)
}

fn validate_existing_parent_chain(root: &Path, relative_path: &Path) -> Result<bool> {
    let mut current = root.to_owned();
    let Some(parent) = relative_path.parent() else {
        return Ok(true);
    };
    for component in parent.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(StoreError::InvalidPath(
                relative_path.to_string_lossy().into_owned(),
            ));
        };
        current.push(name);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(StoreError::from_io(error)),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(StoreError::InvalidPath("invalid quarantine parent".into()));
        }
    }
    Ok(true)
}

fn optional_identity(
    root: &Path,
    relative_path: &Path,
    kind: QuarantineKind,
) -> Result<Option<String>> {
    match kind {
        QuarantineKind::File => file_identity_hash(root, relative_path),
        QuarantineKind::Directory => directory_identity_hash(root, relative_path),
    }
    .map(Some)
    .or_else(|error| match error {
        StoreError::Io { ref source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        error => Err(error),
    })
}

fn ensure_parent_directories(root: &Path, relative_path: &Path) -> Result<()> {
    let parent = relative_path
        .parent()
        .ok_or_else(|| StoreError::InvalidPath("quarantine parent is missing".into()))?;
    let mut current = root.to_owned();
    for component in parent.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(StoreError::InvalidPath(
                relative_path.to_string_lossy().into_owned(),
            ));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(StoreError::InvalidPath("invalid quarantine parent".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(StoreError::from_io)?;
                if !fs::symlink_metadata(&current)
                    .map_err(StoreError::from_io)?
                    .file_type()
                    .is_dir()
                {
                    return Err(StoreError::InvalidPath("invalid quarantine parent".into()));
                }
                crate::atomic::sync_parent(&current)?;
            }
            Err(error) => return Err(StoreError::from_io(error)),
        }
    }
    Ok(())
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
    fn quarantine_moves_a_file_once_and_recognizes_a_completed_move() {
        let fixture = Fixture::new();
        let source = Path::new("workspaces/demo/transcript/run.jsonl");
        let quarantine = Path::new("deleting/op/transcript/run.jsonl");
        let source_path = fixture.0.join(source);
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, b"recorded history").unwrap();
        let identity = file_identity_hash(&fixture.0, source).unwrap();

        assert_eq!(
            quarantine_entry(
                &fixture.0,
                source,
                quarantine,
                QuarantineKind::File,
                Some(&identity),
            )
            .unwrap(),
            QuarantineResult::Moved
        );
        assert!(!source_path.exists());
        assert_eq!(
            fs::read(fixture.0.join(quarantine)).unwrap(),
            b"recorded history"
        );
        assert_eq!(
            quarantine_entry(
                &fixture.0,
                source,
                quarantine,
                QuarantineKind::File,
                Some(&identity),
            )
            .unwrap(),
            QuarantineResult::AlreadyMoved
        );
    }

    #[test]
    fn quarantine_moves_a_directory_as_one_entry_without_expanding_its_contents() {
        let fixture = Fixture::new();
        let source = Path::new("workspaces/demo/memory/runs/run-id");
        let quarantine = Path::new("deleting/op/memory/run-id");
        let source_path = fixture.0.join(source);
        fs::create_dir_all(source_path.join("nested")).unwrap();
        fs::write(source_path.join("nested/body.jsonl"), b"memory text").unwrap();
        let identity = directory_identity_hash(&fixture.0, source).unwrap();

        assert_eq!(
            quarantine_entry(
                &fixture.0,
                source,
                quarantine,
                QuarantineKind::Directory,
                Some(&identity),
            )
            .unwrap(),
            QuarantineResult::Moved
        );
        assert!(!source_path.exists());
        assert_eq!(
            fs::read(fixture.0.join(quarantine).join("nested/body.jsonl")).unwrap(),
            b"memory text"
        );
        assert_eq!(
            quarantine_entry(
                &fixture.0,
                source,
                quarantine,
                QuarantineKind::Directory,
                Some(&identity),
            )
            .unwrap(),
            QuarantineResult::AlreadyMoved
        );
    }

    #[test]
    fn quarantine_rejects_identity_conflicts_and_skips_only_an_absent_directory() {
        let fixture = Fixture::new();
        let source = Path::new("workspaces/demo/transcript/run.jsonl");
        let quarantine = Path::new("deleting/op/transcript/run.jsonl");
        fs::create_dir_all(fixture.0.join(source).parent().unwrap()).unwrap();
        fs::create_dir_all(fixture.0.join(quarantine).parent().unwrap()).unwrap();
        fs::write(fixture.0.join(source), b"history").unwrap();
        fs::write(fixture.0.join(quarantine), b"keep this object").unwrap();
        let source_identity = file_identity_hash(&fixture.0, source).unwrap();

        assert!(
            quarantine_entry(
                &fixture.0,
                source,
                quarantine,
                QuarantineKind::File,
                Some(&source_identity),
            )
            .is_err()
        );
        assert_eq!(
            fs::read(fixture.0.join(quarantine)).unwrap(),
            b"keep this object"
        );

        let missing = Path::new("workspaces/demo/memory/runs/empty");
        let absent_quarantine = Path::new("deleting/op/memory/empty");
        assert_eq!(
            quarantine_entry(
                &fixture.0,
                missing,
                absent_quarantine,
                QuarantineKind::Directory,
                None,
            )
            .unwrap(),
            QuarantineResult::SkippedAbsent
        );
        fs::create_dir_all(fixture.0.join(missing).parent().unwrap()).unwrap();
        fs::create_dir(fixture.0.join(missing)).unwrap();
        assert!(
            quarantine_entry(
                &fixture.0,
                missing,
                absent_quarantine,
                QuarantineKind::Directory,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn quarantine_tree_cleanup_resumes_with_a_fixed_entry_budget() {
        let fixture = Fixture::new();
        let quarantine = Path::new("deleting/op/memory/run-id");
        let directory = fixture.0.join(quarantine);
        fs::create_dir_all(directory.join("nested/deeper")).unwrap();
        for index in 0..7 {
            fs::write(
                directory.join(format!("nested/deeper/{index}.jsonl")),
                b"memory",
            )
            .unwrap();
        }
        let identity = directory_identity_hash(&fixture.0, quarantine).unwrap();
        let mut total_visited = 0;
        let mut continuation = None;

        for _ in 0..8 {
            let batch = purge_quarantined_tree_batch(
                &fixture.0,
                quarantine,
                QuarantineKind::Directory,
                Some(&identity),
                continuation.as_deref(),
                3,
            )
            .unwrap();
            if total_visited == 0 {
                assert_eq!(batch.visited_entries, 3);
                assert!(!batch.complete, "目录访问也占用本批预算");
            }
            assert!(batch.visited_entries <= 3);
            total_visited += batch.visited_entries;
            continuation = batch.continuation.clone();
            if batch.complete {
                assert!(!directory.exists());
                assert!(total_visited >= 10);
                return;
            }
        }

        panic!("bounded cleanup did not finish within expected batches");
    }

    #[test]
    fn quarantine_tree_cleanup_is_idempotent_for_missing_roots_and_rejects_identity_changes() {
        let fixture = Fixture::new();
        let quarantine = Path::new("deleting/op/transcript/run.jsonl");
        fs::create_dir_all(fixture.0.join(quarantine).parent().unwrap()).unwrap();
        fs::write(fixture.0.join(quarantine), b"history").unwrap();
        let identity = file_identity_hash(&fixture.0, quarantine).unwrap();
        let replacement = Path::new("deleting/op/transcript/replacement.jsonl");
        fs::write(fixture.0.join(replacement), b"replacement").unwrap();
        let replacement_identity = file_identity_hash(&fixture.0, replacement).unwrap();
        assert_ne!(identity, replacement_identity);
        fs::remove_file(fixture.0.join(quarantine)).unwrap();

        let missing = purge_quarantined_tree_batch(
            &fixture.0,
            quarantine,
            QuarantineKind::File,
            Some(&identity),
            None,
            1,
        )
        .unwrap();
        assert!(missing.complete);
        assert_eq!(missing.visited_entries, 0);

        // 旧文件仍存在时先比较文件身份，避免删除后文件系统复用标识。
        fs::rename(fixture.0.join(replacement), fixture.0.join(quarantine)).unwrap();
        assert!(
            purge_quarantined_tree_batch(
                &fixture.0,
                quarantine,
                QuarantineKind::File,
                Some(&identity),
                None,
                1,
            )
            .is_err()
        );
        assert_eq!(
            fs::read(fixture.0.join(quarantine)).unwrap(),
            b"replacement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_tree_cleanup_refuses_to_follow_or_remove_symlinks() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let quarantine = Path::new("deleting/op/memory/run-id");
        let directory = fixture.0.join(quarantine);
        fs::create_dir_all(&directory).unwrap();
        fs::write(fixture.0.join("outside"), b"preserve").unwrap();
        symlink(fixture.0.join("outside"), directory.join("link")).unwrap();
        let identity = directory_identity_hash(&fixture.0, quarantine).unwrap();

        assert!(
            purge_quarantined_tree_batch(
                &fixture.0,
                quarantine,
                QuarantineKind::Directory,
                Some(&identity),
                None,
                8,
            )
            .is_err()
        );
        assert_eq!(fs::read(fixture.0.join("outside")).unwrap(), b"preserve");
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
