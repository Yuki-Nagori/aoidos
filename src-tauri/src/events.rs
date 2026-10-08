//! 事件信封与每流序号。事件名先登记到通信契约；准备与投递分开，由平台 adapter 投递。
//! 序号按事件名和流标识分别计数，不跨进程续号；调用方须串行发送同一流。

use crate::ipc::CmdError;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

// JSON 数字在前端由 JS number 接收，超过该值将无法精确比较相邻序号。
const MAX_SEQ: u64 = (1 << 53) - 1;
type StreamSeqs = HashMap<(String, String), u64>;
static STREAM_SEQS: OnceLock<Mutex<StreamSeqs>> = OnceLock::new();

fn stream_seqs() -> &'static Mutex<StreamSeqs> {
    STREAM_SEQS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 为事件名和流标识分配下一个序号，从 1 开始。空标识仅在同一事件名下共享计数。
/// 回合生产者退出且 ring 驱逐后经 `retire_stream` 清退；不保证实际投递成功。
///
/// # Errors
///
/// 计数器锁中毒或序号达到 JS 安全整数上限时返回 `app.event-failed`，不会回绕。
pub fn next_seq(event: &str, stream_id: &str) -> Result<u64, CmdError> {
    let mut streams = stream_seqs().lock().map_err(seq_lock_error)?;
    let seq = streams
        .entry((event.to_owned(), stream_id.to_owned()))
        .or_insert(0);
    let next = seq
        .checked_add(1)
        .filter(|value| *value <= MAX_SEQ)
        .ok_or_else(seq_exhausted)?;
    *seq = next;
    Ok(next)
}

fn seq_lock_error<T>(_: PoisonError<T>) -> CmdError {
    event_error("事件序号锁不可用")
}

fn seq_exhausted() -> CmdError {
    event_error("事件序号已达到安全整数上限")
}

fn event_error(message: &str) -> CmdError {
    CmdError::new("app.event-failed", message, None)
}

/// 构造 `{ seq, data }` 信封。先序列化 data，再分配序号，序列化失败不占号。
/// data 的语义由具体事件定义；构造成功后的投递失败仍会占号，监听方以快照恢复缺口。
///
/// # Errors
///
/// data 无法序列化、序号锁不可用或计数达到上限时返回 `app.event-failed`。
pub fn envelope<T: Serialize + ?Sized>(
    event: &str,
    stream_id: &str,
    data: &T,
) -> Result<serde_json::Value, CmdError> {
    Ok(prepare(event, stream_id, data)?.payload)
}

/// 可分步确认的信封；正文提交期间序号仅被预留，不自动确认到快照。
pub struct PreparedEnvelope {
    pub seq: u64,
    pub payload: serde_json::Value,
}

/// 先序列化再预留，不投递、不提交业务状态。
///
/// # Errors
/// 同 [`envelope`]；序列化失败不占号，已预留的序号不复用。
pub fn prepare<T: Serialize + ?Sized>(
    event: &str,
    stream_id: &str,
    data: &T,
) -> Result<PreparedEnvelope, CmdError> {
    let data = serde_json::to_value(data).map_err(payload_error)?;
    let seq = next_seq(event, stream_id)?;
    Ok(PreparedEnvelope {
        seq,
        payload: serde_json::json!({ "seq": seq, "data": data }),
    })
}

/// 生产者退出后清退该流的所有事件名；不会影响其他 UUID 的计数。
pub fn retire_stream(stream_id: &str) {
    stream_seqs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(_, id), _| id != stream_id);
}

fn payload_error(_: serde_json::Error) -> CmdError {
    event_error("事件载荷无法序列化")
}
#[cfg(test)]
pub(crate) fn force_sequence(event: &str, id: &str, seq: u64) {
    stream_seqs()
        .lock()
        .unwrap()
        .insert((event.into(), id.into()), seq);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_STREAM: AtomicU64 = AtomicU64::new(0);
    fn unique_stream(tag: &str) -> String {
        format!("{tag}-{}", TEST_STREAM.fetch_add(1, Ordering::Relaxed))
    }

    #[test]
    fn same_stream_increments_other_stream_independent() {
        let stream = unique_stream("seq");
        assert_eq!(next_seq("llm:turn:chunk", &stream).unwrap(), 1);
        assert_eq!(next_seq("llm:turn:chunk", &stream).unwrap(), 2);
        assert_eq!(
            next_seq("llm:turn:chunk", &format!("{stream}-other")).unwrap(),
            1
        );
        assert_eq!(next_seq("llm:turn:done", &stream).unwrap(), 1);
    }

    #[test]
    fn empty_stream_id_is_its_own_stream() {
        let event = unique_stream("empty");
        assert_eq!(next_seq(&event, "").unwrap(), 1);
        assert_eq!(next_seq(&event, "").unwrap(), 2);
        assert_eq!(next_seq(&event, "other").unwrap(), 1);
    }

    #[test]
    fn colon_in_stream_does_not_alias_another_event() {
        let event = unique_stream("colon");
        assert_eq!(next_seq(&event, "part:id").unwrap(), 1);
        assert_eq!(next_seq(&format!("{event}:part"), "id").unwrap(), 1);
    }

    #[test]
    fn envelope_carries_seq_and_data() {
        let stream = unique_stream("envelope");
        let value = envelope("llm:turn:chunk", &stream, &serde_json::json!({ "k": 1 })).unwrap();
        assert_eq!(value, serde_json::json!({ "seq": 1, "data": { "k": 1 } }));
    }

    #[test]
    fn invalid_payload_does_not_consume_seq() {
        let stream = unique_stream("invalid");
        let invalid = HashMap::from([((1, 2), "value")]);
        assert_eq!(
            envelope("event", &stream, &invalid).unwrap_err().code(),
            "app.event-failed"
        );
        assert_eq!(next_seq("event", &stream).unwrap(), 1);
    }

    #[test]
    fn exhausted_seq_does_not_wrap_or_advance() {
        let stream = unique_stream("max");
        stream_seqs()
            .lock()
            .unwrap()
            .insert(("event".into(), stream.clone()), MAX_SEQ - 1);
        assert_eq!(next_seq("event", &stream).unwrap(), MAX_SEQ);
        assert_eq!(
            next_seq("event", &stream).unwrap_err().code(),
            "app.event-failed"
        );
        assert_eq!(
            stream_seqs().lock().unwrap()[&("event".into(), stream)],
            MAX_SEQ
        );
    }

    #[test]
    fn poisoned_lock_is_a_command_error() {
        let error = seq_lock_error(PoisonError::new(()));
        assert_eq!(error.code(), "app.event-failed");
    }

    #[test]
    fn concurrent_reservations_are_unique_and_contiguous() {
        let stream = unique_stream("concurrent");
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let stream = stream.clone();
                std::thread::spawn(move || {
                    (0..25)
                        .map(|_| next_seq("event", &stream).unwrap())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut seqs: Vec<_> = workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect();
        seqs.sort_unstable();
        assert_eq!(seqs, (1..=100).collect::<Vec<_>>());
    }
}
