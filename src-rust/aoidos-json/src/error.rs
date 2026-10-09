/// 不保存输入或第三方诊断，避免上层错误意外带出密钥 / 模型正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    SizeLimit,
    /// 字节不是合法 UTF-8；不携带原始字节或解码器诊断。
    InvalidUtf8,
    /// 语法、重复键、非有限数值或尾随非空白不合法。
    InvalidJson,
    /// 领域 DTO 解码失败；调用方统一处理缺字段与类型错误，不猜测第三方错误文本。
    InvalidShape,
    InvalidEncoding,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SizeLimit => "JSON input exceeds byte limit",
            Self::InvalidUtf8 => "JSON input is not valid UTF-8",
            Self::InvalidJson => "invalid JSON or duplicate object key",
            Self::InvalidShape => "JSON does not match the requested schema",
            Self::InvalidEncoding => "value cannot be encoded as JSON",
        })
    }
}
impl std::error::Error for Error {}
impl Error {
    pub(crate) fn invalid_json(_: serde_json::Error) -> Self {
        Self::InvalidJson
    }
    pub(crate) fn invalid_shape(_: serde_json::Error) -> Self {
        Self::InvalidShape
    }
    pub(crate) fn invalid_encoding(_: serde_json::Error) -> Self {
        Self::InvalidEncoding
    }
    pub(crate) fn invalid_utf8(_: std::str::Utf8Error) -> Self {
        Self::InvalidUtf8
    }
}
