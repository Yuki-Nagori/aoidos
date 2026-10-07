//! 有界文本读取：限制磁盘输入，区分首次缺失、非法路径与损坏内容。

use std::io::{self, Read};
use std::path::Path;

use crate::error::{Result, StoreError};
use crate::paths::validate_missing_parent;

/// 读取最多 `max_bytes` 字节的 UTF-8 文件；真正缺失返回 `None`。
/// 长度先于解码检查，截断多字节字符不会把超限误报成 I/O 错误。
///
/// # Errors
/// 超限或非法 UTF-8 返回 `corrupt`，祖先不是目录返回 `io`，
/// 其余打开 / 读取失败保留存储 I/O 错误码；错误文本不包含文件正文。
pub fn read_text_bounded(path: &Path, max_bytes: u32) -> Result<Option<String>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            validate_missing_parent(path)?;
            return Ok(None);
        }
        Err(error) => return Err(StoreError::from_io(error)),
    };
    let mut bytes = Vec::new();
    file.take(u64::from(max_bytes) + 1)
        .read_to_end(&mut bytes)
        .map_err(StoreError::from_io)?;
    decode_bounded(bytes, max_bytes).map(Some)
}

fn decode_bounded(bytes: Vec<u8>, max_bytes: u32) -> Result<String> {
    if bytes.len() as u64 > u64::from(max_bytes) {
        return Err(StoreError::Corrupt("text file too large".into()));
    }
    String::from_utf8(bytes).map_err(invalid_utf8)
}

fn invalid_utf8(_: std::string::FromUtf8Error) -> StoreError {
    StoreError::Corrupt("text file is not valid UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_precede_utf8_and_never_include_body_in_errors() {
        assert_eq!(decode_bounded(b"abc".to_vec(), 3).unwrap(), "abc");
        assert_eq!(decode_bounded(Vec::new(), 0).unwrap(), "");
        for bytes in [vec![0xff], vec![b'a', 0xe4], b"secret".to_vec()] {
            let error = decode_bounded(bytes, 1).unwrap_err();
            assert_eq!(error.code(), "corrupt");
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[test]
    fn reads_missing_files_and_rejects_blocked_ancestors_and_directories() {
        let dir = std::env::temp_dir().join(format!("mythos-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            read_text_bounded(&dir.join("missing/nested.json"), 4).unwrap(),
            None
        );
        assert!(validate_missing_parent(Path::new("missing-relative.json")).is_ok());
        assert_eq!(
            read_text_bounded(&dir.join("invalid\0name"), 4)
                .unwrap_err()
                .code(),
            "io"
        );
        let file = dir.join("data");
        std::fs::write(&file, b"text").unwrap();
        assert_eq!(
            read_text_bounded(&file, 4).unwrap().as_deref(),
            Some("text")
        );
        assert_eq!(read_text_bounded(&file, 3).unwrap_err().code(), "corrupt");
        for path in [file.join("a.json"), file.join("nested/a.json")] {
            assert_eq!(read_text_bounded(&path, 4).unwrap_err().code(), "io");
        }
        assert!(matches!(
            read_text_bounded(&dir, 4).unwrap_err().code(),
            "io" | "permission"
        ));
        std::fs::write(&file, [0xff]).unwrap();
        assert_eq!(read_text_bounded(&file, 4).unwrap_err().code(), "corrupt");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
