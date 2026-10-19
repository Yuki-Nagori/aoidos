use std::path::Path;

pub fn peak_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").expect("读取 Linux 测试进程 RSS");
        let kibibytes = status
            .lines()
            .find_map(|line| line.strip_prefix("VmHWM:"))
            .expect("Linux 进程状态应包含 VmHWM")
            .split_whitespace()
            .next()
            .expect("Linux VmHWM 应包含数值")
            .parse::<u64>()
            .expect("Linux VmHWM 应为整数 KiB");
        Some(kibibytes.checked_mul(1024).expect("Linux RSS 字节数溢出"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub fn disk_sizes(root: &Path) -> serde_json::Value {
    let mut files = serde_json::Map::new();
    visit_files(root, root, &mut files).expect("读取压力实验文件大小");
    assert!(
        files.contains_key("storage.sqlite"),
        "未采集到 SQLite 主数据库"
    );
    assert!(
        files.keys().any(|path| path.contains("/transcript/")),
        "未采集到记录正文文件"
    );
    serde_json::Value::Object(files)
}

pub fn disk_summary(root: &Path) -> serde_json::Value {
    let files = disk_sizes(root);
    let files = files.as_object().expect("磁盘文件列表应为对象");
    let mut total_bytes = 0_u64;
    let mut sqlite_bytes = 0_u64;
    let mut backup_bytes = 0_u64;
    let mut memory_bytes = 0_u64;
    let mut transcript_bytes = 0_u64;
    let mut wal_bytes = 0_u64;
    let mut shm_bytes = 0_u64;
    for (path, value) in files {
        let bytes = value.as_u64().expect("文件大小应为非负整数");
        total_bytes += bytes;
        if path == "storage.sqlite" {
            sqlite_bytes = bytes;
        } else if path.starts_with("backups/") {
            backup_bytes += bytes;
        } else if path.contains("/memory/") {
            memory_bytes += bytes;
        } else if path.contains("/transcript/") {
            transcript_bytes += bytes;
        } else if path == "storage.sqlite-wal" {
            wal_bytes = bytes;
        } else if path == "storage.sqlite-shm" {
            shm_bytes = bytes;
        }
    }
    serde_json::json!({
        "fileCount": files.len(),
        "totalBytes": total_bytes,
        "sqliteBytes": sqlite_bytes,
        "backupBytes": backup_bytes,
        "memoryBytes": memory_bytes,
        "transcriptBytes": transcript_bytes,
        "walBytes": wal_bytes,
        "shmBytes": shm_bytes,
    })
}

fn visit_files(
    root: &Path,
    path: &Path,
    files: &mut serde_json::Map<String, serde_json::Value>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_files(root, &path, files)?;
        } else {
            let metadata = entry.metadata()?;
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative, serde_json::json!(metadata.len()));
        }
    }
    Ok(())
}
