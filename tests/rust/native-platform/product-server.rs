//! 有界一次请求 / 一次响应的回环服务；只保留计数，不记录密钥或 prompt。

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
pub struct Server {
    pub endpoint: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    pub fn new(count: Arc<AtomicUsize>, mode: Arc<AtomicUsize>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
        let worker = std::thread::spawn(move || {
            while !shutdown.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        let count = count.clone();
                        let mode = mode.load(Ordering::SeqCst);
                        std::thread::spawn(move || {
                            let _ = respond(socket, &count, mode);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            endpoint,
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn respond(mut socket: TcpStream, count: &AtomicUsize, mode: usize) -> std::io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut raw = Vec::new();
    let mut part = [0_u8; 4096];
    let boundary;
    loop {
        let size = socket.read(&mut part)?;
        if size == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        raw.extend_from_slice(&part[..size]);
        if raw.len() > 2 * 1024 * 1024 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            boundary = end + 4;
            break;
        }
    }
    let headers = std::str::from_utf8(&raw[..boundary]).map_err(invalid_utf8)?;
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .ok_or(std::io::ErrorKind::InvalidData)?;
    if length > 2 * 1024 * 1024 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    while raw.len() < boundary + length {
        let size = socket.read(&mut part)?;
        if size == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        raw.extend_from_slice(&part[..size]);
    }
    let request: serde_json::Value =
        serde_json::from_slice(&raw[boundary..boundary + length]).map_err(invalid_json)?;
    count.fetch_add(1, Ordering::SeqCst);
    if mode == 3 {
        socket.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Ok(());
    }
    socket.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    )?;
    let chat = request["messages"].is_array();
    let prompt = request["prompt"]
        .as_str()
        .or_else(|| request["messages"].as_array()?.last()?["content"].as_str())
        .ok_or(std::io::ErrorKind::InvalidData)?;
    let text = if prompt.ends_with("[AOIDOS:CHECK-PROPOSAL]\n") {
        if mode == 5 {
            r#"{"kind":"check","ruleId":"pbta-explore-v1","actorId":"player","reason":"门是否打开需要判定"}"#
        } else {
            r#"{"kind":"noCheck"}"#
        }
    } else if prompt.ends_with("[AOIDOS:SCENE-PROPOSAL]\n") {
        r#"{"kind":"stay"}"#
    } else if mode == 6 {
        "丢失的终态已恢复。"
    } else {
        "钟声穿过雾气。🕯️"
    };
    let mode = if prompt.ends_with("[AOIDOS:NARRATION]\n") {
        mode
    } else {
        0
    };
    if mode != 2 {
        let frame = format!(
            "data: {}\n\n",
            if chat {
                serde_json::json!({"choices":[{"delta":{"content":text},"finish_reason":null}]})
            } else {
                serde_json::json!({"choices":[{"text":text,"finish_reason":null}]})
            }
        );
        // 在 UTF-8 多字节内部拆分传输，验证真实 adapter 增量解码。
        let split = frame
            .as_bytes()
            .iter()
            .position(|byte| *byte >= 128)
            .map_or(frame.len() / 2, |index| index + 1);
        socket.write_all(&frame.as_bytes()[..split])?;
        socket.flush()?;
        std::thread::sleep(Duration::from_millis(20));
        socket.write_all(&frame.as_bytes()[split..])?;
        socket.flush()?;
    }
    if mode == 1 {
        std::thread::sleep(Duration::from_secs(3));
        return Ok(());
    }
    if mode == 4 {
        return Ok(());
    }
    std::thread::sleep(Duration::from_millis(80));
    let terminal = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({"choices":[{"text":"","finish_reason":"stop"}],"usage":{"prompt_tokens":128,"completion_tokens":16,"total_tokens":144}})
    );
    socket.write_all(terminal.as_bytes())?;
    Ok(())
}
fn invalid_utf8(_: std::str::Utf8Error) -> std::io::Error {
    std::io::ErrorKind::InvalidData.into()
}
fn invalid_json(_: serde_json::Error) -> std::io::Error {
    std::io::ErrorKind::InvalidData.into()
}
