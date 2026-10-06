//! LLM 域错误：结构化类别 + 错误链分类。
//!
//! `code()` 返回裸码，调用方加 `llm.` 前缀（契约见 ai-docs/architecture/ipc-contract.md）。
//! TLS 分类对 error source 链做类型下钻，不匹配错误字符串；reqwest / hyper 原始错误不进
//! `Display` 与 IPC 载荷（脱敏：类别 + 可选 HTTP 状态）。

use std::error::Error as StdError;

/// 传输层错误类别（不含调度器生成的看门狗 / 空输出 / 取消）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// HTTP 401 或 adapter 明确识别的认证拒绝。
    Auth,
    /// HTTP 402 或明确的余额 / 配额不足。
    Quota,
    /// HTTP 429。
    RateLimited,
    /// 传输失败（连接、DNS、5xx、中断）。是否按可重试处理由交付边界决定。
    Network { source: TransportClass },
    /// TLS 证书 / 握手失败，不重试、不关闭校验。
    Tls,
    /// 协议或能力拒绝：非成功状态、SSE / JSON / UTF-8 损坏、未知 `finish、content_filter` 等。
    BadResponse {
        reason: &'static str,
        status: Option<u16>,
    },
    /// 流提前结束且未带合法 finish / 结束标记；按交付边界映射 network / aborted。
    Interrupted,
    /// 服务端 aborted / `insufficient_system_resource；按交付边界映射` bad-response / aborted。
    ServerAborted,
}

/// 传输错误的可观测细分（已脱敏，供诊断 detail）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportClass {
    /// TCP / 连接层失败。
    Connect,
    /// 请求体读取中断。
    Body,
    /// 超时（本项目不设 reqwest 超时，出现即异常路径）。
    Timeout,
}

/// reqwest 错误的发生阶段，用于 [`ProviderError::from_reqwest`] 的部位归类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPhase {
    /// `send()` 尚未取得响应头。
    Send,
    /// 响应体流读取中。
    Body,
}

impl TransportClass {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Body => "body",
            Self::Timeout => "timeout",
        }
    }
}

impl ProviderError {
    /// 判别码（命名空间 `llm` 由调用方前缀）。交付边界相关的两类见
    /// [`code_at_boundary`](Self::code_at_boundary)。
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Quota => "quota",
            Self::RateLimited => "rate-limited",
            // Interrupted / ServerAborted 在首交付边界另有映射，见 code_at_boundary。
            Self::Network { .. } | Self::Interrupted => "network",
            Self::Tls => "tls",
            Self::BadResponse { .. } | Self::ServerAborted => "bad-response",
        }
    }

    /// 按首交付边界定码：首字节后发生的中断 / 服务端中止归 `aborted`，保留前文不重试。
    #[must_use]
    pub fn code_at_boundary(&self, delivered: bool) -> &'static str {
        if delivered {
            match self {
                Self::Interrupted | Self::ServerAborted | Self::Network { .. } => "aborted",
                other => other.code(),
            }
        } else {
            self.code()
        }
    }

    /// 是否属于传输重试路径（仅首交付前生效；TLS 永不重试）。
    #[must_use]
    pub fn transport_retryable(&self) -> bool {
        matches!(
            self,
            Self::Network { .. } | Self::RateLimited | Self::Interrupted
        )
    }

    /// 对 reqwest 错误做结构分类：TLS 走 source 链下钻（reqwest 0.13 的传输错误
    /// kind 多为 Request，不能据此判本地构造失败），其余按调用部位归网络。
    /// `phase` 区分 send 阶段（连接 / 头，归 Connect）与响应体阶段（归 Body）。
    #[must_use]
    pub fn from_reqwest(error: &reqwest::Error, phase: RequestPhase) -> Self {
        let mut source: Option<&(dyn StdError + 'static)> = Some(error);
        while let Some(node) = source {
            if node_downcast_contains_tls(node) {
                return Self::Tls;
            }
            source = node.source();
        }
        // 本项目不给 reqwest 设超时，不区分 timeout 类；看门狗归调度器。
        let class = match phase {
            RequestPhase::Send => TransportClass::Connect,
            RequestPhase::Body => TransportClass::Body,
        };
        Self::Network { source: class }
    }

    /// 从 HTTP 状态码分类非 200 响应（响应正文不读取、不携带）。
    #[must_use]
    pub fn from_status(status: u16) -> Self {
        match status {
            401 => Self::Auth,
            402 => Self::Quota,
            429 => Self::RateLimited,
            500..=599 => Self::Network {
                source: TransportClass::Connect,
            },
            other => Self::BadResponse {
                reason: "http-status",
                status: Some(other),
            },
        }
    }
}

/// 判断错误节点（含 `io::Error` 的 `get_ref` 内嵌链）是否携带 rustls 错误。
/// 标定过的真实链形如 reqwest → `hyper_util` → io(Other) → io(InvalidData) → rustls。
fn node_downcast_contains_tls(node: &(dyn StdError + 'static)) -> bool {
    if node.downcast_ref::<rustls::Error>().is_some() {
        return true;
    }
    if let Some(io) = node.downcast_ref::<std::io::Error>()
        && let Some(inner) = io.get_ref()
    {
        let inner: &(dyn StdError + 'static) = inner;
        if node_downcast_contains_tls(inner) {
            return true;
        }
    }
    node.source().is_some_and(node_downcast_contains_tls)
}

/// 调度层错误：在 [`ProviderError`] 之外补充看门狗、空输出与本地策略失败。
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    Provider(ProviderError),
    /// 头阶段截止（首字节前），传输重试耗尽后以此失败。
    Stalled,
    /// 合法结束却无已交付正文（温度阶梯耗尽或不可进入）。
    EmptyOutput {
        finish: crate::schedule::FinishReason,
    },
    /// 本地预算端口拒绝（035 落地前仅测试夹具使用；码属 budget 命名空间）。
    Budget {
        reason: String,
    },
    /// 运行策略配置非法（阶梯不递增 / 超步数 / 超出模型温度范围）。
    InvalidPolicy(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth => write!(f, "authentication rejected"),
            Self::Quota => write!(f, "provider quota exhausted"),
            Self::RateLimited => write!(f, "rate limited"),
            Self::Network { source } => write!(f, "transport failure [{}]", source.code()),
            Self::Tls => write!(f, "tls failure"),
            Self::BadResponse { reason, status } => match status {
                Some(status) => write!(f, "bad response [{reason}] status {status}"),
                None => write!(f, "bad response [{reason}]"),
            },
            Self::Interrupted => write!(f, "stream ended without a legal finish"),
            Self::ServerAborted => write!(f, "server aborted the request"),
        }
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_tls_chain() -> Box<dyn StdError + Send + Sync + 'static> {
        let rustls_error = rustls::Error::General("synthetic".into());
        let io = std::io::Error::new(std::io::ErrorKind::InvalidData, rustls_error);
        Box::new(io)
    }

    #[test]
    fn io_wrapped_rustls_error_is_detected() {
        assert!(node_downcast_contains_tls(synthetic_tls_chain().as_ref()));
    }

    #[test]
    fn direct_rustls_error_is_detected() {
        let error = rustls::Error::General("synthetic".into());
        assert!(node_downcast_contains_tls(&error));
    }

    #[test]
    fn plain_io_error_is_not_tls() {
        let io = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
        assert!(!node_downcast_contains_tls(&io));
    }

    #[test]
    fn nested_io_without_rustls_is_not_tls() {
        // get_ref 内嵌链走到尽头也没有 rustls：返回 false 而非误报。
        let nested = std::io::Error::other(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        assert!(!node_downcast_contains_tls(&nested));
    }

    #[test]
    fn nested_chain_rustls_is_detected() {
        // 真实链里 rustls 之上会叠两层 io::Error（hyper_util 包装 + rustls 转换）。
        let inner = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::General("synthetic".into()),
        );
        let outer = std::io::Error::other(inner);
        assert!(node_downcast_contains_tls(&outer));
    }

    #[test]
    fn codes_cover_all_variants() {
        assert_eq!(ProviderError::Auth.code(), "auth");
        assert_eq!(ProviderError::Quota.code(), "quota");
        assert_eq!(ProviderError::RateLimited.code(), "rate-limited");
        assert_eq!(
            ProviderError::Network {
                source: TransportClass::Connect
            }
            .code(),
            "network"
        );
        assert_eq!(ProviderError::Tls.code(), "tls");
        assert_eq!(
            ProviderError::BadResponse {
                reason: "json",
                status: Some(400)
            }
            .code(),
            "bad-response"
        );
        assert_eq!(ProviderError::Interrupted.code(), "network");
        assert_eq!(ProviderError::ServerAborted.code(), "bad-response");
    }

    #[test]
    fn boundary_mapping_for_interruptions() {
        let cases = [
            ProviderError::Interrupted,
            ProviderError::ServerAborted,
            ProviderError::Network {
                source: TransportClass::Body,
            },
        ];
        for error in &cases {
            assert_eq!(error.code_at_boundary(false), {
                if matches!(error, ProviderError::ServerAborted) {
                    "bad-response"
                } else {
                    "network"
                }
            });
            assert_eq!(error.code_at_boundary(true), "aborted");
        }
    }

    #[test]
    fn boundary_mapping_keeps_other_codes() {
        assert_eq!(ProviderError::Auth.code_at_boundary(true), "auth");
        assert_eq!(ProviderError::Tls.code_at_boundary(false), "tls");
    }

    #[test]
    fn transport_retryable_set() {
        assert!(
            ProviderError::Network {
                source: TransportClass::Connect
            }
            .transport_retryable()
        );
        assert!(ProviderError::RateLimited.transport_retryable());
        assert!(ProviderError::Interrupted.transport_retryable());
        assert!(!ProviderError::Tls.transport_retryable());
        assert!(!ProviderError::Auth.transport_retryable());
        assert!(!ProviderError::Quota.transport_retryable());
        assert!(
            !ProviderError::BadResponse {
                reason: "json",
                status: None
            }
            .transport_retryable()
        );
        assert!(!ProviderError::ServerAborted.transport_retryable());
    }

    #[test]
    fn status_classification_table() {
        assert!(matches!(
            ProviderError::from_status(401),
            ProviderError::Auth
        ));
        assert!(matches!(
            ProviderError::from_status(402),
            ProviderError::Quota
        ));
        assert!(matches!(
            ProviderError::from_status(429),
            ProviderError::RateLimited
        ));
        assert!(matches!(
            ProviderError::from_status(500),
            ProviderError::Network { .. }
        ));
        assert!(matches!(
            ProviderError::from_status(503),
            ProviderError::Network { .. }
        ));
        assert!(matches!(
            ProviderError::from_status(404),
            ProviderError::BadResponse {
                status: Some(404),
                ..
            }
        ));
    }

    #[derive(Debug)]
    struct Wrap(Box<dyn StdError + Send + Sync + 'static>);

    impl std::fmt::Display for Wrap {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "wrapped transport error")
        }
    }

    impl StdError for Wrap {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(&*self.0)
        }
    }

    #[test]
    fn source_chain_through_non_io_wrapper_is_detected() {
        // 非 io 中间层的 source 链同样能下钻到 rustls。
        let wrapped = Wrap(synthetic_tls_chain());
        assert!(node_downcast_contains_tls(&wrapped));
        assert!(!wrapped.to_string().is_empty());
    }

    #[test]
    fn display_covers_all_variants() {
        let errors = [
            ProviderError::Auth,
            ProviderError::Quota,
            ProviderError::RateLimited,
            ProviderError::Network {
                source: TransportClass::Body,
            },
            ProviderError::Tls,
            ProviderError::BadResponse {
                reason: "http-status",
                status: Some(418),
            },
            ProviderError::BadResponse {
                reason: "sse",
                status: None,
            },
            ProviderError::Interrupted,
            ProviderError::ServerAborted,
        ];
        for error in &errors {
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn transport_class_codes() {
        assert_eq!(TransportClass::Connect.code(), "connect");
        assert_eq!(TransportClass::Body.code(), "body");
        assert_eq!(TransportClass::Timeout.code(), "timeout");
    }
}
