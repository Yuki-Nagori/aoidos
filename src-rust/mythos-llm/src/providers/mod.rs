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
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DeepSeek => "deepseek",
        }
    }

    /// 生产端点（HTTPS）；测试与显式配置经装配层注入回环地址。
    pub fn default_base(&self) -> &'static str {
        match self {
            Self::DeepSeek => "https://api.deepseek.com",
        }
    }

    /// 按持久身份解析；未知 id 返回 `None`，由上层映射 app.not-found。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "deepseek" => Some(Self::DeepSeek),
            _ => None,
        }
    }
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
}
