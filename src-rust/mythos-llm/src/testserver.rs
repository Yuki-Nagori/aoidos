//! 测试夹具：脚本化本地 HTTP / TLS 服务。
//!
//! 每个物理请求一条新连接（响应固定 `Connection: close`），脚本段按序写出；
//! `Abort` 用 SO_LINGER(0) 制造 RST 断连，`Hold` 挂起连接用于看门狗 / 取消测试。
//! TLS 模式用 rcgen 自签名证书服务 `localhost`，配合客户端证书校验失败做 TLS 分类夹具。

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// 一段响应脚本；每个连接独立完整执行一遍。
#[derive(Clone)]
pub(crate) enum Segment {
    /// 响应状态行。
    Status(u16),
    /// 响应头一行（不含 CRLF）。
    Header(String),
    /// 写一段响应体字节并 flush（首个 BodyChunk 自动补齐头终止空行）。
    BodyChunk(Vec<u8>),
    /// 等待一段时长再继续（模拟延迟到达的分片）。
    Delay(Duration),
    /// 立即以 RST 断开连接。
    Abort,
    /// 挂起直到服务器关闭（模拟服务端停发）。
    Hold,
}

/// 记录到的一次请求。
#[derive(Clone, Debug)]
pub(crate) struct RecordedRequest {
    pub method: String,
    pub path: String,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }
}

pub(crate) struct ScriptedServer {
    pub addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    cancel: CancellationToken,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// TLS 模式下的服务端证书，供测试加入信任根。
    certificate: Option<rustls::pki_types::CertificateDer<'static>>,
}

impl ScriptedServer {
    /// 启动明文 HTTP 夹具；每个连接执行同一脚本。
    pub async fn start(script: Vec<Segment>) -> Self {
        Self::spawn(vec![script], None, None).await
    }

    /// 启动明文 HTTP 夹具；第 n 条连接执行第 n 个脚本，超出后重复最后一个
    /// （用于「先失败后成功」「阶梯逐步返回」类场景）。
    pub async fn start_sequence(scripts: Vec<Vec<Segment>>) -> Self {
        Self::spawn(scripts, None, None).await
    }

    /// 启动自签名 TLS 夹具（证书 SAN=localhost）。
    pub async fn start_tls(script: Vec<Segment>) -> Self {
        crate::install_ring_provider();
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])
            .expect("self-signed certificate");
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![certified.cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    certified.signing_key.serialize_der().into(),
                ),
            )
            .expect("server config");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        Self::spawn(
            vec![script],
            Some(acceptor),
            Some(certified.cert.der().clone()),
        )
        .await
    }

    async fn spawn(
        scripts: Vec<Vec<Segment>>,
        acceptor: Option<tokio_rustls::TlsAcceptor>,
        certificate: Option<rustls::pki_types::CertificateDer<'static>>,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(accept_loop(
            listener,
            acceptor,
            scripts,
            requests.clone(),
            cancel.clone(),
        ));
        Self {
            addr,
            requests,
            cancel,
            handle: Mutex::new(Some(handle)),
            certificate,
        }
    }

    /// TLS 夹具的服务端证书（明文夹具返回 None）。
    pub fn certificate(&self) -> Option<&rustls::pki_types::CertificateDer<'static>> {
        self.certificate.as_ref()
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("requests lock").clone()
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().expect("requests lock").len()
    }

    /// 停止接收并等待 accept 循环退出（在飞连接经取消 token 各自收尾）；重复调用为空操作。
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        let handle = self.handle.lock().expect("handle").take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }
}

async fn accept_loop(
    listener: tokio::net::TcpListener,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
    scripts: Vec<Vec<Segment>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    cancel: CancellationToken,
) {
    let mut connection_index: usize = 0;
    loop {
        let (stream, _) = tokio::select! {
            _ = cancel.cancelled() => break,
            // 夹具监听回环，accept 失败属测试环境异常，直接 panic 暴露。
            accepted = listener.accept() => accepted.expect("fixture accept"),
        };
        let script = scripts
            .get(connection_index.min(scripts.len() - 1))
            .expect("at least one script")
            .clone();
        connection_index += 1;
        tokio::spawn(handle_connection(
            stream,
            acceptor.clone(),
            script,
            requests.clone(),
            cancel.clone(),
        ));
    }
}

/// 类型化连接：Abort 需要对底层 TcpStream 设置 SO_LINGER(0)。
enum Conn {
    Plain(tokio::net::TcpStream),
    // TlsStream 超过 1 KiB，装箱避免整体枚举膨胀。
    Tls(Box<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>),
}

impl Conn {
    fn underlying(&self) -> &tokio::net::TcpStream {
        match self {
            Self::Plain(stream) => stream,
            Self::Tls(tls) => tls.get_ref().0,
        }
    }

    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.write_all(buf).await,
            Self::Tls(tls) => tls.write_all(buf).await,
        }
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush().await,
            Self::Tls(tls) => tls.flush().await,
        }
    }

    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf).await,
            Self::Tls(tls) => tls.read(buf).await,
        }
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
    script: Vec<Segment>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    cancel: CancellationToken,
) {
    let mut conn = match acceptor {
        None => Conn::Plain(stream),
        Some(acceptor) => match acceptor.accept(stream).await {
            Ok(tls) => Conn::Tls(Box::new(tls)),
            Err(_) => return,
        },
    };
    let recorded = match read_request(&mut conn, &cancel).await {
        Some(recorded) => recorded,
        None => return,
    };
    requests.lock().expect("requests lock").push(recorded);
    run_script(&mut conn, &script, &cancel).await;
}

/// 读取并解析一个 HTTP/1.1 请求（含 Content-Length 正文）。
async fn read_request(conn: &mut Conn, cancel: &CancellationToken) -> Option<RecordedRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    // 头部上限 64 KiB，防御畸形请求占住夹具。
    loop {
        if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            return None;
        }
        let read = tokio::select! {
            _ = cancel.cancelled() => return None,
            read = conn.read(&mut chunk) => match read {
                Ok(0) | Err(_) => return None,
                Ok(n) => n,
            },
        };
        buffer.extend_from_slice(&chunk[..read]);
    }
    let head_end = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("loop exits only when found");
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?.to_owned();
    let mut parts = request_line.split(' ');
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let content_length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < content_length {
        let read = tokio::select! {
            _ = cancel.cancelled() => return None,
            read = conn.read(&mut chunk) => match read {
                Ok(0) | Err(_) => return None,
                Ok(n) => n,
            },
        };
        body.extend_from_slice(&chunk[..read]);
    }
    Some(RecordedRequest {
        method,
        path,
        headers,
        body,
    })
}

async fn run_script(conn: &mut Conn, script: &[Segment], cancel: &CancellationToken) {
    let mut head_open = true;
    for segment in script {
        match segment {
            Segment::Status(code) => {
                let reason = match *code {
                    200 => "OK",
                    401 => "Unauthorized",
                    402 => "Payment Required",
                    404 => "Not Found",
                    429 => "Too Many Requests",
                    _ => "Status",
                };
                let _ = conn
                    .write_all(format!("HTTP/1.1 {code} {reason}\r\n").as_bytes())
                    .await;
            }
            Segment::Header(line) => {
                let _ = conn.write_all(format!("{line}\r\n").as_bytes()).await;
            }
            Segment::BodyChunk(bytes) => {
                if head_open {
                    let _ = conn.write_all(b"\r\n").await;
                    head_open = false;
                }
                let _ = conn.write_all(bytes).await;
                let _ = conn.flush().await;
            }
            Segment::Delay(duration) => {
                tokio::time::sleep(*duration).await;
            }
            Segment::Abort => {
                // SO_LINGER(0) 后 drop 发 RST，客户端读到的是连接层错误而非干净 EOF。
                // linger 为零时 drop 不阻塞；弃用警告针对非零 linger 的阻塞风险。
                #[allow(deprecated)]
                let _ = conn.underlying().set_linger(Some(Duration::ZERO));
                return;
            }
            Segment::Hold => {
                cancel.cancelled().await;
                return;
            }
        }
    }
}

// ---- SSE 帧构造助手：与 DeepSeek adapter 的解析形状一一对应，多个测试模块共用。----

/// chat delta.content / completion text 共用的正文帧。
pub(crate) fn sse_text_frame(text: &str) -> String {
    let escaped = serde_json::json!(text).to_string();
    format!("data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{escaped}}}}}]}}\n\n")
}

/// completion 端点 choices[0].text 帧形状。
pub(crate) fn sse_completion_frame(text: &str) -> String {
    let escaped = serde_json::json!(text).to_string();
    format!("data: {{\"choices\":[{{\"index\":0,\"text\":{escaped}}}]}}\n\n")
}

/// chat reasoning_content 帧（不进正文、不重置计时器）。
pub(crate) fn sse_reasoning_frame(text: &str) -> String {
    let escaped = serde_json::json!(text).to_string();
    format!(
        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"reasoning_content\":{escaped}}}}}]}}\n\n"
    )
}

/// finish 帧；reason 取 stop / length / aborted / tool_calls 等。
pub(crate) fn sse_finish_frame(reason: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"{reason}\"}}]}}\n\n"
    )
}

/// usage-only 帧（空 choices 合法）。
pub(crate) fn sse_usage_frame(prompt_tokens: u64, completion_tokens: u64) -> String {
    format!(
        "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{prompt_tokens},\"completion_tokens\":{completion_tokens}}}}}\n\n"
    )
}

/// 200 + SSE 头 + 单段响应体。
pub(crate) fn sse_ok_script(body: &str) -> Vec<Segment> {
    vec![
        Segment::Status(200),
        Segment::Header("Content-Type: text/event-stream".into()),
        Segment::Header("Connection: close".into()),
        Segment::BodyChunk(body.as_bytes().to_vec()),
    ]
}

/// 非 200 状态的失败响应（不带正文；空行终止响应头）。
pub(crate) fn http_error_script(status: u16) -> Vec<Segment> {
    vec![
        Segment::Status(status),
        Segment::Header("Connection: close".into()),
        Segment::Header(String::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn trusted_tls_exchange_covers_stream_paths() {
        crate::install_ring_provider();
        let server = ScriptedServer::start_tls(sse_ok_script(&format!(
            "{}data: [DONE]\n\n",
            sse_finish_frame("stop")
        )))
        .await;
        let cert = reqwest::tls::Certificate::from_der(
            server
                .certificate()
                .expect("tls fixture has a certificate")
                .as_ref(),
        )
        .expect("certificate parses");
        let client = reqwest::Client::builder()
            .add_root_certificate(cert)
            .build()
            .expect("client with trusted root");
        let response = client
            .post(format!("https://localhost:{}/x", server.addr.port()))
            .body("q")
            .send()
            .await
            .expect("trusted tls exchange");
        assert_eq!(response.status().as_u16(), 200);
        // 请求经 TLS 读路径记录，响应经 TLS 写路径送回。
        assert_eq!(server.request_count(), 1);
        assert_eq!(server.requests()[0].path, "/x");
    }

    #[tokio::test]
    async fn tls_abort_disconnects_the_client() {
        crate::install_ring_provider();
        // 可信根 + Abort 脚本：握手成功后立即 RST，覆盖 TLS 连接的底层断开路径。
        let server = ScriptedServer::start_tls(vec![
            Segment::Status(200),
            Segment::Header("Connection: close".into()),
            Segment::Header(String::new()),
            Segment::Abort,
        ])
        .await;
        let cert = reqwest::tls::Certificate::from_der(
            server
                .certificate()
                .expect("tls fixture has a certificate")
                .as_ref(),
        )
        .expect("certificate parses");
        let client = reqwest::Client::builder()
            .add_root_certificate(cert)
            .build()
            .expect("client with trusted root");
        let result = client
            .post(format!("https://localhost:{}/x", server.addr.port()))
            .body("q")
            .send()
            .await;
        assert!(result.is_err(), "connection should be reset");
    }

    #[tokio::test]
    async fn plaintext_garbage_to_tls_port_fails_handshake() {
        let server = ScriptedServer::start_tls(sse_ok_script("data: [DONE]\n\n")).await;
        let mut plain = tokio::net::TcpStream::connect(server.addr)
            .await
            .expect("connect");
        plain
            .write_all(b"GET / HTTP/1.1\r\n\r\n")
            .await
            .expect("write plaintext");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.request_count(), 0);
    }

    #[tokio::test]
    async fn split_body_reads_continue_until_content_length() {
        // 平台无关地钉住正文续读循环：Unix 回环上头与正文常合并在首次 read 到达，
        // 该循环不执行会造成覆盖率缺口（CI run 37482413333 的 ubuntu / macOS 失败）。
        // CL 设为 4 且首写只给 2 字节：无论头与首段是否合并，循环必然再读一次；
        // 以「请求被记录」为完成信号轮询，不用固定 sleep。
        let server = ScriptedServer::start(sse_ok_script("data: [DONE]\n\n")).await;
        let mut client = tokio::net::TcpStream::connect(server.addr)
            .await
            .expect("connect");
        client
            .write_all(b"POST /split HTTP/1.1\r\nContent-Length: 4\r\n\r\nab")
            .await
            .expect("write head with partial body");
        client.flush().await.expect("flush");
        tokio::time::sleep(Duration::from_millis(10)).await;
        client.write_all(b"cd").await.expect("write rest");
        client.flush().await.expect("flush rest");
        for _ in 0..200 {
            if server.request_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            server.request_count(),
            1,
            "request must complete via body loop"
        );
        let recorded = &server.requests()[0];
        assert_eq!(recorded.path, "/split");
        assert_eq!(recorded.body, b"abcd");
    }

    #[tokio::test]
    async fn malformed_clients_do_not_break_the_fixture() {
        let server = ScriptedServer::start(sse_ok_script("data: [DONE]\n\n")).await;

        // 连上即断：读请求得到 EOF。
        let closed = tokio::net::TcpStream::connect(server.addr)
            .await
            .expect("connect");
        drop(closed);

        // 只发超长垃圾头（无空行终止）：超出头部上限后放弃。
        let mut garbage = tokio::net::TcpStream::connect(server.addr)
            .await
            .expect("connect");
        garbage
            .write_all(&vec![b'G'; 70 * 1024])
            .await
            .expect("write garbage");
        drop(garbage);

        // 声明 Content-Length 后分段发送正文再断开：正文读取走分片与 EOF 路径。
        let mut partial = tokio::net::TcpStream::connect(server.addr)
            .await
            .expect("connect");
        partial
            .write_all(b"POST /partial HTTP/1.1\r\nContent-Length: 10\r\n\r\n")
            .await
            .expect("write head");
        partial.flush().await.expect("flush head");
        tokio::time::sleep(Duration::from_millis(20)).await;
        partial
            .write_all(b"abcd")
            .await
            .expect("write partial body");
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(partial);

        tokio::time::sleep(Duration::from_millis(20)).await;
        // 畸形连接都不产生请求记录。
        assert_eq!(server.request_count(), 0);
    }

    #[tokio::test]
    async fn shutdown_interrupts_pending_body_read() {
        let server = ScriptedServer::start(sse_ok_script("data: [DONE]\n\n")).await;
        let mut stalled = tokio::net::TcpStream::connect(server.addr)
            .await
            .expect("connect");
        stalled
            .write_all(b"POST /stalled HTTP/1.1\r\nContent-Length: 99\r\n\r\nshort")
            .await
            .expect("write head");
        stalled.flush().await.expect("flush");
        tokio::time::sleep(Duration::from_millis(20)).await;
        server.shutdown().await;
        // 连接持有方仍存活，但服务器已停止；重复关停为空操作。
        server.shutdown().await;
        assert_eq!(server.request_count(), 0);
        drop(stalled);
    }
}
