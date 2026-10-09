use super::*;

#[test]
fn blocked_parent_and_directory_cleanup_fail_explicitly() {
    let root = std::env::temp_dir().join(format!("aoidos-journal-error-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    assert!(Journal::create(&root.join("invalid.jsonl"), b"missing LF").is_err());
    assert!(Journal::create(Path::new(""), b"{}\n").is_err());
    let file = root.join("blocked");
    fs::write(&file, b"file").unwrap();
    assert!(Journal::create(&file.join("record.jsonl"), b"{}\n").is_err());
    assert!(remove(&root).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn journal_round_trip_bounds_and_exclusive_creation() {
    let dir = std::env::temp_dir().join(format!("aoidos-journal-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("record.jsonl");
    let mut writer = Journal::create(&path, b"{}\n").unwrap();
    assert!(Journal::create(&path, b"{}\n").is_err());
    assert_eq!(writer.append(b"{\"seq\":1}\n", false).unwrap(), 3);
    writer.sync().unwrap();
    assert_eq!(writer.offset(), 13);
    drop(writer);
    let mut lines = Vec::new();
    scan(&path, 32, &mut |offset, line| {
        lines.push((offset, line.to_owned()));
        Ok(())
    })
    .unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(read_at(&path, 3, 10, 32).unwrap(), "{\"seq\":1}\n");
    assert!(read_at(&path, 0, 33, 32).is_err());
    assert!(read_at(&path, 13, 1, 32).is_err());
    assert_eq!(Journal::open(&path).unwrap().offset(), 13);
    assert!(scan(&path, 2, &mut |_, _| Ok(())).is_err());
    assert!(scan(&path, 32, &mut |_, _| Err(corrupt())).is_err());
    for invalid in [b"".as_slice(), b"\n", b"x", b"x\ny\n"] {
        assert!(validate_line(invalid).is_err());
    }
    fs::write(&path, b"{}\npartial").unwrap();
    assert!(scan(&path, 32, &mut |_, _| Ok(())).is_err());
    fs::write(&path, [0xff, b'\n']).unwrap();
    assert!(scan(&path, 32, &mut |_, _| Ok(())).is_err());
    assert!(read_at(&path, 0, 2, 32).is_err());
    remove(&path).unwrap();
    remove(&path).unwrap();
    fs::remove_dir_all(dir).unwrap();
}

struct FaultIo {
    bytes: io::Cursor<Vec<u8>>,
    fail: &'static str,
}
impl Write for FaultIo {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.fail == "write" {
            return Err(io::ErrorKind::StorageFull.into());
        }
        self.bytes.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.fail == "flush" {
            Err(io::ErrorKind::Other.into())
        } else {
            Ok(())
        }
    }
}
impl Seek for FaultIo {
    fn seek(&mut self, s: SeekFrom) -> io::Result<u64> {
        if self.fail == "seek" {
            Err(io::ErrorKind::Other.into())
        } else {
            self.bytes.seek(s)
        }
    }
}
impl JournalIo for FaultIo {
    fn sync(&mut self) -> io::Result<()> {
        if self.fail == "sync" {
            Err(io::ErrorKind::Other.into())
        } else {
            Ok(())
        }
    }
    fn truncate(&mut self, n: u64) -> io::Result<()> {
        if self.fail == "truncate" {
            Err(io::ErrorKind::Other.into())
        } else {
            self.bytes.get_mut().truncate(n as usize);
            Ok(())
        }
    }
}
#[test]
fn injected_boundary_failures_do_not_confirm_new_offsets() {
    for fail in ["write", "flush", "sync"] {
        let mut io = FaultIo {
            bytes: io::Cursor::new(Vec::new()),
            fail,
        };
        assert!(append_boundary(&mut io, b"{}\n", true).is_err());
    }
    for fail in ["truncate", "seek", "sync"] {
        let mut io = FaultIo {
            bytes: io::Cursor::new(vec![1, 2, 3]),
            fail,
        };
        assert!(rollback(&mut io, 0).is_err());
    }
    let mut io = FaultIo {
        bytes: io::Cursor::new(vec![1, 2, 3]),
        fail: "",
    };
    rollback(&mut io, 0).unwrap();
    assert!(io.bytes.get_ref().is_empty());
    append_boundary(&mut io, b"{}\n", false).unwrap();
}

#[cfg(feature = "test-support")]
#[test]
fn real_file_faults_rollback_known_writes_and_freeze_uncertain_syncs() {
    let dir = std::env::temp_dir().join(format!("aoidos-journal-faults-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    for (name, stage, uncertain) in [
        ("write", FaultStage::Write, false),
        ("flush", FaultStage::Flush, false),
        ("sync", FaultStage::Sync, true),
        ("rollback", FaultStage::Rollback, true),
    ] {
        let path = dir.join(format!("{name}.jsonl"));
        let mut log = Journal::create(&path, b"{}\n").unwrap();
        log.inject_fault(stage);
        assert!(log.append(b"{\"seq\":1}\n", true).is_err());
        assert_eq!(log.offset(), 3);
        assert_eq!(log.is_frozen(), uncertain);
        if uncertain {
            assert!(log.append(b"{}\n", true).is_err());
        } else {
            assert_eq!(fs::read(&path).unwrap(), b"{}\n");
            log.append(b"{}\n", true).unwrap();
        }
    }
    let path = dir.join("sync.jsonl");
    let mut log = Journal::open(&path).unwrap();
    log.inject_fault(FaultStage::Sync);
    assert!(log.sync().is_err());
    assert!(log.is_frozen());
    let mut memory = io::Cursor::new(vec![1, 2, 3]);
    JournalIo::sync(&mut memory).unwrap();
    JournalIo::truncate(&mut memory, 1).unwrap();
    assert_eq!(memory.get_ref(), &vec![1]);
    assert!(Journal::create(&dir.join("invalid"), b"invalid").is_err());
    let path = dir.join("tail.jsonl");
    fs::write(&path, b"{}\npartial").unwrap();
    let backup = dir.join("tail.bak");
    assert!(preserve_and_repair(&path, &backup).unwrap());
    assert_eq!(fs::read(&backup).unwrap(), b"{}\npartial");
    assert_eq!(fs::read(&path).unwrap(), b"{}\n");
    assert!(!preserve_and_repair(&path, &backup).unwrap());
    fs::write(&path, []).unwrap();
    assert!(!preserve_and_repair(&path, &backup).unwrap());
    fs::remove_dir_all(dir).unwrap();
}
