//! 平台事件和记录提交端口；这里只传普通数据，不依赖 Tauri 或记录文件格式。

use crate::fault::Fault;
use futures::future::BoxFuture;
use mythos_llm::schedule::FinishReason;
use serde::Serialize;

/// 类型化终态只允许契约中的合法组合；取消不能携带错误或收尾原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Terminal {
    Completed {
        finish_reason: FinishReason,
    },
    Cancelled,
    Failed {
        error: Fault,
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_reason: Option<FinishReason>,
    },
}

/// IPC 的终态词汇，不扩展进行中状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Completed,
    Cancelled,
    Failed,
}

impl Terminal {
    /// 返回公开契约的终态词汇。
    #[must_use]
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Completed { .. } => Outcome::Completed,
            Self::Cancelled => Outcome::Cancelled,
            Self::Failed { .. } => Outcome::Failed,
        }
    }
    /// 取消省略原因；失败仅返回已知的真实原因。
    #[must_use]
    pub fn finish_reason(&self) -> Option<FinishReason> {
        match self {
            Self::Completed { finish_reason } => Some(*finish_reason),
            Self::Cancelled => None,
            Self::Failed { finish_reason, .. } => *finish_reason,
        }
    }
    /// 仅失败终态包含脱敏错误。
    #[must_use]
    pub fn error(&self) -> Option<&Fault> {
        match self {
            Self::Failed { error, .. } => Some(error),
            Self::Completed { .. } | Self::Cancelled => None,
        }
    }
    pub(crate) fn failed(error: Fault) -> Self {
        Self::Failed {
            error,
            finish_reason: None,
        }
    }
}

/// 序号按事件种类独立递增；终态 data 携带最后确认的 chunk 序号。
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TurnEvent {
    Chunk {
        #[serde(rename = "turnId")]
        turn_id: String,
        delta: String,
    },
    Done {
        #[serde(rename = "turnId")]
        turn_id: String,
        outcome: Outcome,
        #[serde(rename = "chunkSeq")]
        chunk_seq: u64,
        #[serde(rename = "finishReason", skip_serializing_if = "Option::is_none")]
        finish_reason: Option<FinishReason>,
    },
    Failed {
        #[serde(rename = "turnId")]
        turn_id: String,
        code: String,
        message: String,
        #[serde(rename = "chunkSeq")]
        chunk_seq: u64,
        #[serde(rename = "finishReason", skip_serializing_if = "Option::is_none")]
        finish_reason: Option<FinishReason>,
    },
}

impl TurnEvent {
    /// 名称是契约常量，平台适配不按任意字符串猜测载荷。
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Chunk { .. } => "llm:turn:chunk",
            Self::Done { .. } => "llm:turn:done",
            Self::Failed { .. } => "llm:turn:failed",
        }
    }
    /// 事件所属的回合 UUID。
    #[must_use]
    pub fn turn_id(&self) -> &str {
        match self {
            Self::Chunk { turn_id, .. }
            | Self::Done { turn_id, .. }
            | Self::Failed { turn_id, .. } => turn_id,
        }
    }
}

/// 已完成序列化及序号预留的事件；确认前不能对窗口投递。
pub struct PreparedEvent {
    pub seq: u64,
    pub event: TurnEvent,
    pub envelope: serde_json::Value,
}

/// 序号与平台投递的分步接口。同一回合由串行所有者调用。
pub trait EventPort: Send + Sync {
    /// # Errors
    /// 序列化或序号预留失败返回脱敏 `app.event-failed`，不得提交正文。
    fn prepare(&self, event: TurnEvent) -> Result<PreparedEvent, Fault>;
    /// # Errors
    /// 投递失败不撤销已确认正文 / 基线，监听者可主动恢复快照。
    fn deliver(&self, event: PreparedEvent) -> Result<(), Fault>;
    /// 仅在终态且生产者退出后调用；清除该 UUID 的所有事件序号。
    fn retire(&self, turn_id: &str);
}

/// 022 接入的输出提交边界；Future 开始后必须由所有者等待到一致边界。
pub trait OutputWriter: Send + Sync {
    /// # Errors
    /// 写入失败返回脱敏 store.*，该增量不进入快照或 chunk。
    fn append<'a>(&'a self, turn_id: &'a str, text: &'a str) -> BoxFuture<'a, Result<(), Fault>>;
    /// # Errors
    /// 封口失败保留已提交前文，协调器展示 failed，不伪造 completed。
    fn finish<'a>(
        &'a self,
        turn_id: &'a str,
        terminal: &'a Terminal,
    ) -> BoxFuture<'a, Result<(), Fault>>;
}

/// 内存调试写入方；确认只代表内存接纳，不宣称持久记录已落盘。
#[derive(Default)]
pub struct MemoryWriter;
impl OutputWriter for MemoryWriter {
    fn append<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<(), Fault>> {
        Box::pin(async { Ok(()) })
    }
    fn finish<'a>(&'a self, _: &'a str, _: &'a Terminal) -> BoxFuture<'a, Result<(), Fault>> {
        Box::pin(async { Ok(()) })
    }
}
