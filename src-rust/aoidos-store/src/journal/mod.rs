//! 单写者 JSONL 原语：有界扫描、排他创建、确认 offset 和不确定写入冻结。

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::atomic::sync_parent;
use crate::error::{Result, StoreError};

/// 每次追加只提交一条完整行；业务 JSON 与身份校验由消费模块完成。
pub struct Journal {
    file: Box<dyn JournalIo>,
    offset: u64,
    frozen: bool,
}

impl Journal {
    /// 仅测试支持特性提供单次真实文件故障，不在产品默认特性中编译。
    #[cfg(feature = "test-support")]
    pub fn inject_fault(&mut self, stage: FaultStage) {
        let old = std::mem::replace(&mut self.file, Box::new(io::Cursor::new(Vec::new())));
        self.file = Box::new(FaultIo {
            inner: old,
            stage,
            fired: false,
        });
    }
    /// 排他创建日志并同步首行及父目录，不覆盖既有文件。
    /// # Errors
    /// 首行非法、文件存在或落盘失败返回存储错误；失败文件留待核验。
    pub fn create(path: &Path, first_line: &[u8]) -> Result<Self> {
        validate_line(first_line)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(StoreError::from_io)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(StoreError::from_io)?;
        file.write_all(first_line).map_err(StoreError::from_io)?;
        file.sync_all().map_err(StoreError::from_io)?;
        sync_parent(path)?;
        Ok(Self {
            file: Box::new(file),
            offset: first_line.len() as u64,
            frozen: false,
        })
    }

    /// 只打开已经由领域扫描校验的日志，不自动创建或修复。
    /// # Errors
    /// 打开 / 获取末尾位置失败返回存储错误。
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(StoreError::from_io)?;
        let offset = file.seek(SeekFrom::End(0)).map_err(StoreError::from_io)?;
        Ok(Self {
            file: Box::new(file),
            offset,
            frozen: false,
        })
    }

    /// 返回前一确认边界，不把失败写入计入其中。
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// 写完整 LF 后才确认；high / 封口要求同步，回滚失败后永久冻结此句柄。
    /// # Errors
    /// 写入 / 同步失败返回原始存储码；冻结句柄拒绝进一步追加。
    pub fn append(&mut self, line: &[u8], high: bool) -> Result<u64> {
        validate_line(line)?;
        if self.frozen {
            return Err(corrupt());
        }
        let start = self.offset;
        if let Err((error, uncertain)) = append_boundary(self.file.as_mut(), line, high) {
            // fsync 失败也可能已经发布；保留文件并冻结，不能把回滚当作耐久承诺。
            self.frozen = uncertain || rollback(self.file.as_mut(), start).is_err();
            return Err(StoreError::from_io(error));
        }
        self.offset += line.len() as u64;
        Ok(start)
    }

    /// 写入结果不确定时禁止继续使用本句柄。
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// 会话关闭强制同步，两种耐久等级均适用。
    /// # Errors
    /// 同步失败冻结句柄并返回存储错误。
    pub fn sync(&mut self) -> Result<()> {
        match self.file.sync() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.frozen = true;
                Err(StoreError::from_io(error))
            }
        }
    }
}

trait JournalIo: Write + Seek + Send {
    fn sync(&mut self) -> io::Result<()>;
    fn truncate(&mut self, len: u64) -> io::Result<()>;
}
impl JournalIo for File {
    fn sync(&mut self) -> io::Result<()> {
        self.sync_all()
    }
    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.set_len(len)
    }
}
impl JournalIo for io::Cursor<Vec<u8>> {
    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.get_mut().truncate(len as usize);
        Ok(())
    }
}
/// 明确故障点用于提交边界验证；partial 写入 / flush 可回滚，sync 与回滚失败不确定。
#[cfg(feature = "test-support")]
pub enum FaultStage {
    Write,
    Flush,
    Sync,
    Rollback,
}
#[cfg(feature = "test-support")]
struct FaultIo {
    inner: Box<dyn JournalIo>,
    stage: FaultStage,
    fired: bool,
}
#[cfg(feature = "test-support")]
impl Write for FaultIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.fired && matches!(self.stage, FaultStage::Write | FaultStage::Rollback) {
            self.fired = true;
            self.inner.write_all(&bytes[..bytes.len().min(3)])?;
            return Err(io::ErrorKind::StorageFull.into());
        }
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        if !self.fired && matches!(self.stage, FaultStage::Flush) {
            self.fired = true;
            return Err(io::ErrorKind::Other.into());
        }
        self.inner.flush()
    }
}
#[cfg(feature = "test-support")]
impl Seek for FaultIo {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}
#[cfg(feature = "test-support")]
impl JournalIo for FaultIo {
    fn sync(&mut self) -> io::Result<()> {
        if !self.fired && matches!(self.stage, FaultStage::Sync) {
            self.fired = true;
            return Err(io::ErrorKind::Other.into());
        }
        self.inner.sync()
    }
    fn truncate(&mut self, len: u64) -> io::Result<()> {
        if matches!(self.stage, FaultStage::Rollback) {
            return Err(io::ErrorKind::Other.into());
        }
        self.inner.truncate(len)
    }
}
fn append_boundary(
    io: &mut dyn JournalIo,
    line: &[u8],
    high: bool,
) -> std::result::Result<(), (io::Error, bool)> {
    io.write_all(line).map_err(known_error)?;
    io.flush().map_err(known_error)?;
    if high {
        io.sync().map_err(uncertain_error)?;
    }
    Ok(())
}
fn known_error(error: io::Error) -> (io::Error, bool) {
    (error, false)
}
fn uncertain_error(error: io::Error) -> (io::Error, bool) {
    (error, true)
}
fn rollback(io: &mut dyn JournalIo, offset: u64) -> io::Result<()> {
    io.truncate(offset)?;
    io.seek(SeekFrom::Start(offset))?;
    io.sync()
}
fn corrupt() -> StoreError {
    StoreError::Corrupt("invalid or uncertain journal boundary".into())
}
fn validate_line(line: &[u8]) -> Result<()> {
    if line.len() < 2 || line.last() != Some(&b'\n') || line[..line.len() - 1].contains(&b'\n') {
        return Err(corrupt());
    }
    Ok(())
}

/// 逐行扫描，不载入整份日志。回调只接收一个 UTF-8 完整行（含 LF）。
/// # Errors
/// 超长、非法 UTF-8、半行和回调校验失败均停止，禁止跳过中间损坏。
pub fn scan(
    path: &Path,
    max_line: usize,
    visit: &mut dyn FnMut(u64, &str) -> Result<()>,
) -> Result<()> {
    let file = File::open(path).map_err(StoreError::from_io)?;
    let mut reader = BufReader::new(file);
    let mut offset = 0;
    loop {
        let mut line = Vec::new();
        reader
            .by_ref()
            .take(max_line as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(StoreError::from_io)?;
        if line.is_empty() {
            return Ok(());
        }
        if line.len() > max_line {
            return Err(corrupt());
        }
        validate_line(&line)?;
        let text = std::str::from_utf8(&line).map_err(invalid_utf8)?;
        visit(offset, text)?;
        offset += line.len() as u64;
    }
}
fn invalid_utf8(_: std::str::Utf8Error) -> StoreError {
    corrupt()
}

/// 按索引读取单个有界片段；短读不会冒充完整记录。
/// # Errors
/// 偏移 / 长度失配、非法 UTF-8 或磁盘错误返回存储错误。
pub fn read_at(path: &Path, offset: u64, len: usize, max: usize) -> Result<String> {
    if len > max {
        return Err(corrupt());
    }
    let mut file = File::open(path).map_err(StoreError::from_io)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(StoreError::from_io)?;
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes).map_err(StoreError::from_io)?;
    String::from_utf8(bytes).map_err(invalid_owned_utf8)
}
fn invalid_owned_utf8(_: std::string::FromUtf8Error) -> StoreError {
    corrupt()
}

/// 仅半行尾部需要修复；先排他备份并同步，再进行有界尾部截断。
/// # Errors
/// 备份或同步失败不修改源文件；中间内容仍由领域扫描校验。
pub fn preserve_and_repair(path: &Path, backup: &Path) -> Result<bool> {
    let mut source = File::open(path).map_err(StoreError::from_io)?;
    if source.metadata().map_err(StoreError::from_io)?.len() == 0 {
        return Ok(false);
    }
    source
        .seek(SeekFrom::End(-1))
        .map_err(StoreError::from_io)?;
    let mut last = [0];
    source.read_exact(&mut last).map_err(StoreError::from_io)?;
    if last[0] == b'\n' {
        return Ok(false);
    }
    source
        .seek(SeekFrom::Start(0))
        .map_err(StoreError::from_io)?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(backup)
        .map_err(StoreError::from_io)?;
    io::copy(&mut source, &mut destination).map_err(StoreError::from_io)?;
    destination.sync_all().map_err(StoreError::from_io)?;
    sync_parent(backup)?;
    drop(source);
    drop(destination);
    crate::atomic::truncate_incomplete_jsonl(path)
}

/// 清理已经确认封口的 sidecar 并同步目录；不存在视为已清理。
/// # Errors
/// 删除 / 目录同步失败保留错误，调用方不得重写封口块。
pub fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::from_io(error)),
    }
}

#[cfg(test)]
mod tests;
