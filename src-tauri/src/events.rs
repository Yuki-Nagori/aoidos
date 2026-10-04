//! 事件发送纯逻辑：每流单调 seq 与契约信封。事件名必须先在
//! ai-docs/architecture/ipc-contract.md 登记；信封不带版本号（契约总则）。
//! 需要 AppHandle 的真实发送是装配层胶水（src-tauri/src/lib.rs `emit_event`）——
//! tauri::test 的 mock 在 Windows 触发 STATUS_ENTRYPOINT_NOT_FOUND，不可用。

use serde::Serialize;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

// 进程级单例；seq 的生命周期与进程一致（监听者以快照对齐，不依赖跨进程连续）。
static STREAM_SEQS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

fn stream_seqs() -> &'static Mutex<HashMap<String, u64>> {
    STREAM_SEQS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取「事件名 + 流标识」的下一个 seq：同流单调递增，异流互不占号。
///
/// 条目随进程存活、不清退：每流一个 `u64`，量级为 O(流数)（桌面单机可忽略）。
pub fn next_seq(event: &str, stream_id: &str) -> u64 {
    let key = format!("{event}:{stream_id}");
    let mut streams = stream_seqs().lock().expect("stream seq mutex poisoned");
    let next = streams.entry(key).or_insert(0);
    *next += 1;
    *next
}

/// 契约信封 `{ seq, data }`。`data` 需要能重绘状态（契约：快照对齐，不重放）。
pub fn envelope<T: Serialize + ?Sized>(
    event: &str,
    stream_id: &str,
    data: &T,
) -> serde_json::Value {
    serde_json::json!({ "seq": next_seq(event, stream_id), "data": data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    // STREAM_SEQS 是进程级单例：流标识必须带唯一后缀，避免测试间互相推进 seq。
    fn unique_stream(tag: &str) -> String {
        format!(
            "{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn same_stream_increments_other_stream_independent() {
        let stream = unique_stream("seq");
        assert_eq!(next_seq("llm:turn:chunk", &stream), 1);
        assert_eq!(next_seq("llm:turn:chunk", &stream), 2);
        assert_eq!(
            next_seq("llm:turn:chunk", &format!("{stream}-other")),
            1,
            "不同流互不占号"
        );
        assert_eq!(
            next_seq("llm:turn:done", &stream),
            1,
            "事件名也是流的一部分"
        );
    }

    #[test]
    fn empty_stream_id_is_its_own_stream() {
        // 契约：stream_id 为空表示该事件无流概念——同一事件名下共享一个计数。
        let event = "llm:turn:chunk";
        assert_eq!(next_seq(event, ""), 1);
        assert_eq!(next_seq(event, ""), 2);
        assert_eq!(
            next_seq(event, &unique_stream("other")),
            1,
            "非空流与无流概念互不占号"
        );
    }

    #[test]
    fn envelope_carries_seq_and_data() {
        let stream = unique_stream("envelope");
        let value = envelope("llm:turn:chunk", &stream, &serde_json::json!({ "k": 1 }));
        assert_eq!(value["seq"], 1);
        assert_eq!(value["data"]["k"], 1);
        assert!(value.get("v").is_none(), "信封不带版本号（契约总则）");
    }
}
