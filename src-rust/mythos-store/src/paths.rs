//! 路径组件消毒与工作区标识：防穿越、Windows 保留名与非法字符。

use std::path::{Path, PathBuf};

use super::error::{Result, StoreError};

// Windows 将“祖先是普通文件”也报告为 NotFound（task 017 / issue #7）；只允许真正缺失的目录链。
// 从最近父路径向上找到首个存在的目录，不创建目录，也不吞掉权限等错误。
pub(crate) fn validate_missing_parent(path: &Path) -> Result<()> {
    for parent in path
        .ancestors()
        .skip(1)
        .filter(|p| !p.as_os_str().is_empty())
    {
        let metadata = match std::fs::metadata(parent) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            result => result.map_err(StoreError::from_io)?,
        };
        if !metadata.is_dir() {
            return Err(StoreError::from_io(std::io::Error::from(
                std::io::ErrorKind::NotADirectory,
            )));
        }
        break;
    }
    Ok(())
}

// Windows 设备名，大小写不敏感，按第一个 `.` 之前的 stem 匹配（task 013）。不是一般的非法字符表。
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

const SCRIPT_ID_MAX: usize = 64;
const SCRIPT_HASH_LEN: usize = 8;

/// 单个路径组件消毒：剥离非法字符与控制字符、Windows 保留名加 `_` 前缀、
/// 去尾部点与空格。消毒后为空则报 `invalid-path`。
///
/// Windows 在设备名检查之前会去掉尾部的 `.` 和空格，所以这里先 trim 再比对，
/// 避免 `CON ` 落成 `CON`。
///
/// # Errors
///
/// 消毒后为空时返回 `invalid-path`。
pub fn sanitize_component(raw: &str) -> Result<String> {
    let stripped: String = raw.chars().filter(|c| !is_illegal(*c)).collect();
    let trimmed = stripped.trim_end_matches(['.', ' ']).to_string();
    if trimmed.is_empty() {
        return Err(StoreError::InvalidPath(raw.to_string()));
    }
    let stem = trimmed.split('.').next().unwrap_or("");
    if is_reserved_stem(stem) {
        Ok(format!("_{trimmed}"))
    } else {
        Ok(trimmed)
    }
}

fn is_illegal(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()
}

fn is_reserved_stem(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    WINDOWS_RESERVED.contains(&upper.as_str())
}

/// 剧本工作区标识：`[a-z0-9-]{1,64}`。ASCII 字母数字保留、空格 / 下划线 /
/// 连字符转连字符（折叠连续）、其余字符剔除；剔除后为空回退 `script-{hash8}`；
/// 超过 64 字符时保留前 55 位再追加 `-{hash8}`（原始名 FNV-1a 的低 32 位）。
/// Windows 保留设备名追加 `-0`，使标识与 [`join_under_root`] 消毒后的目录名相同。
#[must_use]
pub fn script_id_from_name(name: &str) -> String {
    let mut kebab = String::new();
    let mut last_was_dash = true;
    for c in name.chars() {
        let mapped = if c.is_ascii_alphanumeric() {
            Some(c.to_ascii_lowercase())
        } else if matches!(c, ' ' | '_' | '-') {
            Some('-')
        } else {
            None
        };
        match mapped {
            Some('-') if last_was_dash => {}
            Some(c) => {
                last_was_dash = c == '-';
                kebab.push(c);
            }
            None => {}
        }
    }
    while kebab.ends_with('-') {
        kebab.pop();
    }
    let id = if kebab.is_empty() {
        format!("script-{}", short_hash(name.as_bytes()))
    } else if kebab.len() > SCRIPT_ID_MAX {
        let keep = SCRIPT_ID_MAX - SCRIPT_HASH_LEN - 1;
        format!("{}-{}", &kebab[..keep], short_hash(name.as_bytes()))
    } else {
        kebab
    };
    avoid_reserved(id)
}

fn avoid_reserved(id: String) -> String {
    if is_reserved_stem(&id) {
        format!("{id}-0")
    } else {
        id
    }
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn short_hash(data: &[u8]) -> String {
    format!("{:08x}", fnv1a(data) as u32)
}

/// 归一化路径分隔符：拼接而来的路径可能混合 `/` 与 `\`，交给 SQLite 等
/// C 库前在 Windows 将 `/` 转为 `\`（混合分隔符会报 `PATH_NOT_FOUND`）。
///
/// 保留操作系统原始路径编码；Unix 的 `\` 是合法文件名字符，不转换。
#[cfg(windows)]
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let wide: Vec<_> = path
        .as_os_str()
        .encode_wide()
        .map(|c| {
            if c == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                c
            }
        })
        .collect();
    PathBuf::from(OsString::from_wide(&wide))
}

/// 归一化路径分隔符。Unix 原样保留路径，不转换反斜杠或非 UTF-8 字节。
#[cfg(not(windows))]
pub fn normalize(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// 在数据根下拼装相对路径。组件不得为空、`..`、`.`，不得含路径分隔符或盘符。
/// 仅校验组件文本，不解析符号链接；数据根及其子目录必须由应用控制。
///
/// # Errors
///
/// 组件非法，或消毒后为空时返回 `invalid-path`。
pub fn join_under_root(root: &Path, components: &[&str]) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for &component in components {
        if component.is_empty()
            || component == ".."
            || component == "."
            || component.contains(['/', '\\', ':'])
        {
            return Err(StoreError::InvalidPath(component.to_string()));
        }
        path.push(sanitize_component(component)?);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_script_id(id: &str) -> bool {
        (1..=SCRIPT_ID_MAX).contains(&id.len())
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    #[test]
    fn sanitize_keeps_normal_names() {
        assert_eq!(sanitize_component("剧本 One.txt").unwrap(), "剧本 One.txt");
        assert_eq!(sanitize_component("a<b>:c").unwrap(), "abc");
    }

    #[test]
    fn sanitize_blocks_windows_reserved_names() {
        assert_eq!(sanitize_component("CON").unwrap(), "_CON");
        assert_eq!(sanitize_component("con.txt").unwrap(), "_con.txt");
        assert_eq!(sanitize_component("Lpt9").unwrap(), "_Lpt9");
        assert_eq!(sanitize_component("CON ").unwrap(), "_CON");
        assert_eq!(sanitize_component("NUL ").unwrap(), "_NUL");
        assert_eq!(sanitize_component("COM1 ").unwrap(), "_COM1");
        assert_eq!(sanitize_component("CON.").unwrap(), "_CON");
    }

    #[test]
    fn sanitize_trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_component("name. . ").unwrap(), "name");
    }

    #[test]
    fn sanitize_rejects_empty_result() {
        let err = sanitize_component("???").unwrap_err();
        assert_eq!(err.code(), "invalid-path");
        assert_eq!(
            sanitize_component(" . ").unwrap_err().code(),
            "invalid-path"
        );
        assert_eq!(
            sanitize_component("...").unwrap_err().code(),
            "invalid-path"
        );
    }

    #[test]
    fn script_id_normalizes_ascii() {
        assert_eq!(script_id_from_name("My Script"), "my-script");
        assert_eq!(script_id_from_name("script"), "script");
        assert_eq!(script_id_from_name("  --_a_--  "), "a");
    }

    #[test]
    fn script_id_falls_back_for_cjk() {
        let id = script_id_from_name("黑塔");
        assert!(id.starts_with("script-"));
        assert_eq!(id.len(), 15);
        assert_eq!(id, script_id_from_name("黑塔"), "same name same id");
        assert!(is_script_id(&id));
    }

    #[test]
    fn script_id_truncates_long_names_with_hash() {
        let exact = "a".repeat(SCRIPT_ID_MAX);
        assert_eq!(script_id_from_name(&exact), exact);

        let long = "b".repeat(100);
        let id = script_id_from_name(&long);
        assert_eq!(id.len(), SCRIPT_ID_MAX);
        assert!(id.starts_with(&"b".repeat(55)));
        assert_eq!(id.as_bytes()[55], b'-');
        assert_ne!(id, script_id_from_name(&"c".repeat(100)));
        assert!(is_script_id(&id));
    }

    #[test]
    fn script_id_avoids_windows_reserved_names() {
        for (name, id) in [
            ("CON", "con-0"),
            ("aux", "aux-0"),
            ("NUL", "nul-0"),
            ("COM1", "com1-0"),
            ("lpt9", "lpt9-0"),
        ] {
            assert_eq!(script_id_from_name(name), id);
            assert!(is_script_id(id));
        }
    }

    #[test]
    fn script_id_is_unchanged_by_join_under_root() {
        let root = Path::new("/data");
        for name in ["CON", "aux", "com1", "My Script", "黑塔"] {
            let id = script_id_from_name(name);
            let joined = join_under_root(root, &["workspaces", &id]).unwrap();
            assert_eq!(
                joined.file_name().unwrap(),
                std::ffi::OsStr::new(&id),
                "{name} must keep its script id as the directory name"
            );
        }
    }

    #[test]
    fn short_hash_uses_low_32_bits() {
        let hash = fnv1a(b"mythos");
        assert_ne!(
            hash as u32,
            (hash >> 32) as u32,
            "fixture must distinguish the two halves"
        );
        assert_eq!(short_hash(b"mythos"), format!("{:08x}", hash as u32));
    }

    #[test]
    fn join_under_root_builds_path() {
        let root = Path::new("/data");
        let p = join_under_root(root, &["workspaces", "abc", "t.jsonl"]).unwrap();
        assert_eq!(p, PathBuf::from("/data/workspaces/abc/t.jsonl"));
    }

    #[test]
    fn join_under_root_rejects_traversal_and_separators() {
        let root = Path::new("/data");
        for bad in ["..", ".", "", "a/b", "a\\b", "C:evil"] {
            assert_eq!(
                join_under_root(root, &[bad]).unwrap_err().code(),
                "invalid-path",
                "component {bad:?} must be rejected"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn normalize_unifies_mixed_separators() {
        let normalized = normalize(Path::new(r"C:\data\nested/storage.sqlite"));
        assert_eq!(normalized, Path::new(r"C:\data\nested\storage.sqlite"));
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let path = PathBuf::from(OsString::from_wide(&[0xd800, u16::from(b'/'), 0x0061]));
        assert_eq!(
            normalize(&path)
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>(),
            [0xd800, u16::from(b'\\'), 0x0061]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn normalize_keeps_unix_separators() {
        let normalized = normalize(Path::new("/data/nested/storage.sqlite"));
        assert_eq!(normalized, Path::new("/data/nested/storage.sqlite"));
    }

    #[cfg(unix)]
    #[test]
    fn normalize_preserves_non_utf8_and_backslashes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"/data/\xff\\name.sqlite"));
        assert_eq!(
            normalize(path).as_os_str().as_bytes(),
            path.as_os_str().as_bytes()
        );
    }
}
