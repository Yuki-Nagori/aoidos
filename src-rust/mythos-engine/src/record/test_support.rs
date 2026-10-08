//! 合成记录夹具，只在内联测试编译，不包含玩家数据或收费请求。

use super::{
    format::{self, Header},
    session::{Appended, RecordEvents, Session},
};
use crate::fault::Fault;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Default)]
pub(crate) struct Events {
    pub seq: AtomicU64,
    pub fail_prepare: AtomicBool,
    pub fail_delivery: AtomicBool,
    pub delivered: Mutex<Vec<Appended>>,
}
impl RecordEvents for Events {
    fn prepare(&self, _: &Appended) -> Result<u64, Fault> {
        if self.fail_prepare.load(Ordering::SeqCst) {
            return Err(Fault::event());
        }
        Ok(self.seq.fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn deliver(&self, _: u64, event: Appended) -> Result<(), Fault> {
        if self.fail_delivery.load(Ordering::SeqCst) {
            return Err(Fault::event());
        }
        self.delivered.lock().unwrap().push(event);
        Ok(())
    }
}
pub(crate) fn header() -> Header {
    let prefix = "[MYTHOS:STATIC]\n只推进已知场景，不替玩家发言。\n[/MYTHOS:STATIC]\n".to_owned();
    Header {
        kind: "header".into(),
        format_version: 1,
        grammar_version: 1,
        projection_version: 1,
        script_id: "demo".into(),
        session_id: uuid::Uuid::new_v4().to_string(),
        created_at: format::now(),
        static_prefix_hash: format::hash(prefix.as_bytes()),
        static_prefix: prefix,
        script_revision: format::hash(b"script"),
    }
}
pub(crate) struct Fixture {
    pub path: PathBuf,
    pub events: Arc<Events>,
    pub session: Session,
}
impl Fixture {
    pub fn new() -> Self {
        let header = header();
        let path = std::env::temp_dir()
            .join(format!("mythos-fixture-{}", header.session_id))
            .join(format!("{}.jsonl", header.session_id));
        let events = Arc::new(Events::default());
        let session = Session::create(path.clone(), header, events.clone()).unwrap();
        Self {
            path,
            events,
            session,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.path.parent().unwrap());
    }
}

/// 单个旧长块已退出逐字尾部，当前输入仍必需，用于候选提交与压缩协议测试。
pub(crate) fn candidate(session: &mut Session) -> super::recap::RecapCandidate {
    use super::{
        format::Body,
        projection::{self, Budget, Shape, WorldView},
        session::Target,
    };
    session
        .append(Body::Narration {
            text: "前文".repeat(1000),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: crate::ports::Terminal::Completed {
                finish_reason: mythos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "当前".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    let working = session.projection_working_set().unwrap();
    let plan = projection::project(
        &working.view(),
        &WorldView {
            context: "",
            needs_recovery: false,
        },
        &Budget {
            context_limit: Some(65536),
            tail: 100,
            ..Budget::default()
        },
        &Target::Narration,
        Shape::Chat,
    )
    .unwrap();
    session
        .prepare_recap(
            &working,
            &plan,
            session.index.keys().next_back().copied().unwrap() - 1,
            session.index.keys().next_back().copied().unwrap() - 1,
        )
        .unwrap()
}
