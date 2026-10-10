//! 对外故障只保留稳定码与文案，不透传 prompt、凭据或供应商响应。

use aoidos_llm::error::RunError;
use aoidos_llm::schedule::FinishReason;
use serde::Serialize;

/// 事件和快照共用的脱敏错误形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Fault {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl Fault {
    /// 注入端口只能传已脱敏的稳定码与文案；不传原始错误 Display。
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    /// Adds bounded, non-sensitive structured context for command errors.
    #[must_use]
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }
    pub(crate) fn bad_request() -> Self {
        Self::new("app.bad-request", "回合参数不合法")
    }
    pub(crate) fn not_found() -> Self {
        Self::new("app.not-found", "回合不存在或已清退")
    }
    pub(crate) fn busy() -> Self {
        Self::new("app.busy", "已有回合正在进行")
    }
    pub(crate) fn event() -> Self {
        Self::new("app.event-failed", "回合事件无法准备")
    }
    pub(crate) fn limit() -> Self {
        Self::new("llm.bad-response", "生成正文超过容量上限")
    }
    pub(crate) fn provider_build(error: aoidos_llm::providers::ProviderBuildError) -> Self {
        use aoidos_llm::providers::ProviderBuildError;
        let (code, message) = match error {
            ProviderBuildError::UnknownProvider => ("app.not-found", "供应商不存在"),
            ProviderBuildError::MissingKey => ("llm.missing-key", "尚未设置密钥"),
            ProviderBuildError::InvalidProfile => ("app.bad-request", "配置不合法"),
            ProviderBuildError::Transport => ("llm.network", "传输客户端无法初始化"),
        };
        Self::new(code, message)
    }
    /// 已提交字符后的头看门狗故障不能按未交付错误展示。
    pub(crate) fn generation(error: &RunError, delivered: bool) -> (Self, Option<FinishReason>) {
        let (code, finish) = match error {
            RunError::Provider(source) => {
                (format!("llm.{}", source.code_at_boundary(delivered)), None)
            }
            RunError::Stalled if delivered => ("llm.aborted".into(), None),
            RunError::Stalled => ("llm.stalled".into(), None),
            RunError::EmptyOutput { finish } => ("llm.empty-output".into(), Some(*finish)),
            RunError::Budget { reason } if reason == "token-estimation-needs-calibration" => {
                ("engine.invalid-phase".into(), None)
            }
            RunError::Budget { .. } => ("budget.exceeded".into(), None),
            RunError::InvalidPolicy(_) => ("app.bad-request".into(), None),
        };
        let message = match code.as_str() {
            "llm.auth" => "供应商认证失败",
            "llm.quota" => "供应商额度不足",
            "llm.rate-limited" => "请求过于频繁",
            "llm.network" => "网络连接失败",
            "llm.tls" => "安全连接验证失败",
            "llm.stalled" => "生成未能及时交付正文",
            "llm.empty-output" => "生成未返回正文",
            "llm.aborted" => "生成中断，已提交正文保留",
            "budget.exceeded" => "费用额度不足",
            "engine.invalid-phase" => "自动请求已暂停，请重新标定 Token 估算",
            "app.bad-request" => "回合参数不合法",
            _ => "供应商响应不合法",
        };
        (Self::new(code, message), finish)
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Fault {}

impl From<aoidos_store::error::StoreError> for Fault {
    fn from(error: aoidos_store::error::StoreError) -> Self {
        Self::new(format!("store.{}", error.code()), "记录存储操作失败")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn store_faults_and_display_are_stable_and_do_not_include_raw_data() {
        let error = Fault::from(aoidos_store::error::StoreError::Corrupt(
            "private raw data".into(),
        ));
        assert_eq!(error.code, "store.corrupt");
        assert!(error.to_string().starts_with("store.corrupt:"));
        assert!(!error.to_string().contains("private raw data"));
        let _: &dyn std::error::Error = &error;
    }

    #[test]
    fn basic_fault_constructors_and_structured_detail_keep_public_shape() {
        assert_eq!(Fault::bad_request().code, "app.bad-request");
        assert_eq!(Fault::not_found().code, "app.not-found");
        assert_eq!(Fault::busy().code, "app.busy");
        assert_eq!(Fault::event().code, "app.event-failed");
        assert_eq!(Fault::limit().code, "llm.bad-response");
        let detailed = Fault::new("engine.invalid-phase", "暂停")
            .with_detail(serde_json::json!({"reason":"budget"}));
        assert_eq!(detailed.detail.unwrap()["reason"], "budget");
    }
}
