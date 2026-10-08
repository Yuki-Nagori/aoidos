//! 会话登记表只接受 Rust 冻结的身份与 header；Webview 不能传磁盘路径。

use crate::{
    fault::Fault,
    record::{
        format::{self, Header},
        session::{RecordEvents, Session, store_fault},
    },
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
};

pub struct Records {
    root: PathBuf,
    events: Arc<dyn RecordEvents>,
    sessions: Mutex<BTreeMap<String, Arc<Mutex<Session>>>>,
    closing: AtomicBool,
}
impl Records {
    pub fn new(root: PathBuf, events: Arc<dyn RecordEvents>) -> Self {
        Self {
            root,
            events,
            sessions: Mutex::new(BTreeMap::new()),
            closing: AtomicBool::new(false),
        }
    }
    /// 023 从可信剧本创建会话；未知 schema 不从 Webview 补造。
    /// # Errors
    /// 已登记身份、容量上限、格式或存储失败拒绝。
    pub fn create(&self, header: Header) -> Result<Arc<Mutex<Session>>, Fault> {
        header.validate().map_err(store_fault)?;
        let path = self.path(&header.script_id, &header.session_id)?;
        let mut registry = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_open()?;
        if registry.contains_key(&header.session_id) {
            return Err(Fault::bad_request());
        }
        if registry.len() >= 16 {
            return Err(Fault::new("app.busy", "打开会话数量已达上限"));
        }
        let id = header.session_id.clone();
        let session = Arc::new(Mutex::new(
            Session::create(path, header, self.events.clone()).map_err(store_fault)?,
        ));
        registry.insert(id, session.clone());
        Ok(session)
    }
    /// 已登记会话只返回同一个单写者，不从只读查询副作用打开文件。
    /// # Errors
    /// 未知会话返回 app.not-found，非法 UUID 为 bad-request。
    pub fn get(&self, id: &str) -> Result<Arc<Mutex<Session>>, Fault> {
        if !format::valid_uuid(id) {
            return Err(Fault::bad_request());
        }
        let registry = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_open()?;
        registry
            .get(id)
            .cloned()
            .ok_or_else(|| Fault::new("app.not-found", "会话未打开"))
    }
    /// 023 已核验会话目录后重新打开；同身份已有写者不重复构造。
    /// # Errors
    /// 恢复失败 / 身份不吻合 / 容量已满拒绝，不启动付费请求。
    pub fn open(&self, script: &str, id: &str) -> Result<Arc<Mutex<Session>>, Fault> {
        let path = self.path(script, id)?;
        let mut registry = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_open()?;
        if let Some(session) = registry.get(id) {
            if session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .header()
                .script_id
                != script
            {
                return Err(Fault::bad_request());
            }
            return Ok(session.clone());
        }
        if registry.len() >= 16 {
            return Err(Fault::new("app.busy", "打开会话数量已达上限"));
        }
        let session = Session::open(path, self.events.clone())?;
        if session.header().session_id != id || session.header().script_id != script {
            return Err(Fault::new("store.corrupt", "会话身份与目录不吻合"));
        }
        let session = Arc::new(Mutex::new(session));
        registry.insert(id.into(), session.clone());
        Ok(session)
    }
    /// 应用所有回合退出后同步并释放会话文件，平台序号随后清退。
    /// # Errors
    /// 任一同步失败仍保留磁盘事实，返回首个 store.* 诊断。
    pub fn close_all(&self) -> Result<(), Fault> {
        let sessions = {
            let mut registry = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.closing.store(true, Ordering::SeqCst);
            std::mem::take(&mut *registry)
        };
        let mut failure = None;
        for (id, session) in sessions {
            if let Err(error) = self.close_session(&id, session) {
                failure.get_or_insert(error);
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    /// 回合生产者退出后释放单个会话，避免已结束周目永久占用登记容量。
    /// # Errors
    /// 非法 / 未登记身份拒绝；同步失败仍冻结旧写者并保留原文件。
    pub fn close(&self, id: &str) -> Result<(), Fault> {
        if !format::valid_uuid(id) {
            return Err(Fault::bad_request());
        }
        let mut registry = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_open()?;
        let session = registry
            .get(id)
            .cloned()
            .ok_or_else(|| Fault::new("app.not-found", "会话未打开"))?;
        // 登记锁覆盖冻结与序号清退，防止同身份的新写者被旧 close 清退。
        let result = self.close_session(id, session);
        registry.remove(id);
        result
    }
    fn close_session(&self, id: &str, session: Arc<Mutex<Session>>) -> Result<(), Fault> {
        let result = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .close();
        self.events.retire(id);
        result
    }
    fn path(&self, script: &str, id: &str) -> Result<PathBuf, Fault> {
        if !format::valid_uuid(id) || mythos_store::paths::script_id_from_name(script) != script {
            return Err(Fault::bad_request());
        }
        mythos_store::paths::join_under_root(
            &self.root,
            &["workspaces", script, "transcript", &format!("{id}.jsonl")],
        )
        .map_err(store_fault)
    }
    fn ensure_open(&self) -> Result<(), Fault> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(Fault::new("app.not-ready", "记录服务已关闭"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::test_support::{Events, header};
    #[test]
    fn close_keeps_identity_registered_until_old_writer_and_events_are_retired() {
        struct ClosingEvents {
            base: Events,
            retiring: Mutex<Option<std::sync::mpsc::Sender<()>>>,
            resume: Mutex<std::sync::mpsc::Receiver<()>>,
        }
        impl RecordEvents for ClosingEvents {
            fn prepare(&self, event: &crate::record::session::Appended) -> Result<u64, Fault> {
                self.base.prepare(event)
            }
            fn deliver(
                &self,
                seq: u64,
                event: crate::record::session::Appended,
            ) -> Result<(), Fault> {
                self.base.deliver(seq, event)
            }
            fn retire(&self, _: &str) {
                if let Some(tx) = self.retiring.lock().unwrap().take() {
                    tx.send(()).unwrap();
                    self.resume.lock().unwrap().recv().unwrap();
                }
            }
        }
        let root = std::env::temp_dir().join(format!("mythos-close-race-{}", uuid::Uuid::new_v4()));
        let (tx, rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let events = Arc::new(ClosingEvents {
            base: Events::default(),
            retiring: Mutex::new(Some(tx)),
            resume: Mutex::new(resume_rx),
        });
        let records = Arc::new(Records::new(root.clone(), events));
        let header = header();
        let old = records.create(header.clone()).unwrap();
        let closer = records.clone();
        let id = header.session_id.clone();
        let close = std::thread::spawn(move || closer.close(&id).unwrap());
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        // retire 尚未结束时，同身份登记必须仍被串行化。
        assert!(records.sessions.try_lock().is_err());
        resume_tx.send(()).unwrap();
        close.join().unwrap();
        let reopened = records.open("demo", &header.session_id).unwrap();
        assert!(!Arc::ptr_eq(&old, &reopened));
        reopened
            .lock()
            .unwrap()
            .append(crate::record::format::Body::PlayerSpeech {
                player_id: "p".into(),
                text: "新写者".into(),
                mode: None,
                content_range: None,
            })
            .unwrap();
        assert!(
            old.lock()
                .unwrap()
                .append(crate::record::format::Body::PlayerSpeech {
                    player_id: "p".into(),
                    text: "旧写者".into(),
                    mode: None,
                    content_range: None
                })
                .is_err()
        );
        records.close_all().unwrap();
        drop(old);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn registered_identity_capacity_close_and_reopen_are_checked() {
        let root = std::env::temp_dir().join(format!("mythos-registry-{}", uuid::Uuid::new_v4()));
        let events = Arc::new(Events::default());
        let records = Records::new(root.clone(), events.clone());
        let first = header();
        let session = records.create(first.clone()).unwrap();
        assert!(Arc::ptr_eq(
            &session,
            &records.open("demo", &first.session_id).unwrap()
        ));
        assert!(records.open("other", &first.session_id).is_err());
        assert!(records.create(first.clone()).is_err());
        assert!(records.get("bad").is_err());
        assert!(records.get(&uuid::Uuid::new_v4().to_string()).is_err());
        assert!(records.path("../escape", &first.session_id).is_err());
        for _ in 1..16 {
            records.create(header()).unwrap();
        }
        assert!(records.create(header()).is_err());
        assert!(
            records
                .open("demo", &uuid::Uuid::new_v4().to_string())
                .is_err()
        );
        assert!(records.close("bad").is_err());
        assert!(records.close(&uuid::Uuid::new_v4().to_string()).is_err());
        let release = records
            .sessions
            .lock()
            .unwrap()
            .keys()
            .find(|id| **id != first.session_id)
            .unwrap()
            .clone();
        records.close(&release).unwrap();
        records.create(header()).unwrap();
        session.lock().unwrap().fail_sync();
        assert!(records.close_all().is_err());
        assert!(records.get(&first.session_id).is_err());
        assert!(records.create(header()).is_err());
        assert!(records.open("demo", &first.session_id).is_err());
        let reopened = Records::new(root.clone(), events.clone());
        assert_eq!(
            reopened
                .open("demo", &first.session_id)
                .unwrap()
                .lock()
                .unwrap()
                .header()
                .session_id,
            first.session_id
        );
        assert!(
            reopened
                .open("demo", &uuid::Uuid::new_v4().to_string())
                .is_err()
        );
        reopened.close_all().unwrap();
        let mismatch = header();
        let path = reopened
            .path("demo", &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let id = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let mismatched = Session::create(path, mismatch, events.clone()).unwrap();
        drop(mismatched);
        let other = Records::new(root.clone(), events);
        assert_eq!(other.open("demo", &id).err().unwrap().code, "store.corrupt");
        other.close_all().unwrap();
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
}
