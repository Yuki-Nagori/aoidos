//! 供应商适配层：每个厂商一个模块，统一实现 [`crate::provider::Provider`]。
//!
//! 新增厂商 = 新增模块 + 在 [`ProviderId`] 登记身份与默认端点；核心流水线
//! （护栏、调度、看门狗）不感知厂商差异。profile 冻结、凭据与代理配置归 019，
//! 本层只提供「按身份选择 adapter」的依据。

mod deepseek;

pub use deepseek::DeepSeek;

/// 供应商身份；与持久 profile 的 providerId 同源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    DeepSeek,
}

impl ProviderId {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DeepSeek => "deepseek",
        }
    }

    /// 生产端点（HTTPS）；测试与显式配置经装配层注入回环地址。
    #[must_use]
    pub fn default_base(&self) -> &'static str {
        match self {
            Self::DeepSeek => "https://api.deepseek.com",
        }
    }

    /// 按持久身份解析；未知 id 返回 `None`，由上层映射 app.not-found。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "deepseek" => Some(Self::DeepSeek),
            _ => None,
        }
    }
}

/// 冻结 profile 构建失败；不包含 endpoint、凭据或原始客户端错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBuildError {
    UnknownProvider,
    MissingKey,
    InvalidProfile,
    Transport,
}

/// 消费冻结凭据 / 代理配置，组装供应商；不发送网络请求。
///
/// # Errors
/// 未知供应商、缺凭据、结构不合法或客户端构建失败返回对应脱敏类别。
pub fn build_provider(
    frozen: crate::config::FrozenProfile,
    snapshot: &crate::proxy::SystemProxySnapshot,
    auth: Option<&crate::proxy::ProxyAuth>,
) -> Result<std::sync::Arc<dyn crate::provider::Provider>, ProviderBuildError> {
    let profile = frozen.profile;
    profile.validate().map_err(invalid_profile)?;
    // v1 adapter 只实现非 thinking 叙事，不静默忽略 profile 的 true。
    if profile.thinking {
        return Err(ProviderBuildError::InvalidProfile);
    }
    let id = ProviderId::parse(&profile.provider_id).ok_or(ProviderBuildError::UnknownProvider)?;
    let key = frozen.credential.ok_or(ProviderBuildError::MissingKey)?;
    let client =
        crate::proxy::build_client(&profile.proxy, snapshot, auth).map_err(invalid_transport)?;
    match id {
        ProviderId::DeepSeek => Ok(std::sync::Arc::new(DeepSeek::with_client(
            id.default_base().into(),
            key,
            client,
        ))),
    }
}
fn invalid_profile(_: crate::config::ProfileError) -> ProviderBuildError {
    ProviderBuildError::InvalidProfile
}
fn invalid_transport(_: reqwest::Error) -> ProviderBuildError {
    ProviderBuildError::Transport
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_identity() {
        let deepseek = ProviderId::DeepSeek;
        assert_eq!(ProviderId::parse(deepseek.as_str()), Some(deepseek));
        assert_eq!(ProviderId::parse("openai"), None);
    }

    #[test]
    fn default_bases_are_https() {
        assert!(ProviderId::DeepSeek.default_base().starts_with("https://"));
    }
    #[test]
    fn frozen_factory_preserves_profile_validation_and_sanitizes_transport_failure() {
        use crate::config::{FrozenProfile, LlmProfile, ProfileMode, ProxyConfig};
        use crate::proxy::SystemProxySnapshot;
        let profile = LlmProfile {
            profile_id: "p".into(),
            provider_id: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            mode: ProfileMode::Completion,
            thinking: false,
            sampling: crate::sampling::Sampling {
                temperature: 1.0,
                max_tokens: 64,
            },
            proxy: ProxyConfig::System,
        };
        let mut invalid = profile.clone();
        invalid.model.clear();
        assert_eq!(
            build_provider(
                FrozenProfile {
                    profile: invalid,
                    credential: None
                },
                &SystemProxySnapshot::default(),
                None
            )
            .err()
            .unwrap(),
            ProviderBuildError::InvalidProfile
        );
        assert_eq!(
            build_provider(
                FrozenProfile {
                    profile,
                    credential: Some(secrecy::SecretString::from("fixture".to_owned()))
                },
                &SystemProxySnapshot {
                    https: Some("::".into())
                },
                None
            )
            .err()
            .unwrap(),
            ProviderBuildError::Transport
        );
    }
}
