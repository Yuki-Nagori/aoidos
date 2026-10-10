//! 不可变价格身份；仅凭模型名不能确定计价版本。

use crate::money::{Currency, MoneyError, UnitPrice};
use std::fmt;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_ID_BYTES: usize = 128;
const MAX_SOURCE_URL_BYTES: usize = 2_048;

/// 用于估算一次物理请求的服务商路由与不可变价格数据。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PriceVersion {
    pub version_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub route_policy: String,
    pub currency: Currency,
    pub unit_tokens: u64,
    pub input_uncached: UnitPrice,
    pub input_cached: UnitPrice,
    pub output: UnitPrice,
    pub usage_mapping_version: String,
    pub source_kind: String,
    pub source_url: Option<String>,
    pub checked_at: String,
    pub effective_from: String,
    pub valid_until: Option<String>,
}

impl PriceVersion {
    /// Validates bounded, non-empty identity fields and a usable effective interval.
    pub fn validate(&self) -> Result<(), PriceError> {
        for value in [
            &self.version_id,
            &self.provider_id,
            &self.model_id,
            &self.route_policy,
            &self.usage_mapping_version,
            &self.source_kind,
            &self.checked_at,
            &self.effective_from,
        ] {
            if value.trim().is_empty()
                || value.len() > MAX_ID_BYTES
                || value.chars().any(char::is_control)
            {
                return Err(PriceError::InvalidIdentity);
            }
        }
        if self.unit_tokens == 0 {
            return Err(PriceError::InvalidUnit);
        }
        if self
            .source_url
            .as_ref()
            .is_some_and(|url| url.is_empty() || url.len() > MAX_SOURCE_URL_BYTES)
        {
            return Err(PriceError::InvalidSource);
        }
        parse_timestamp(&self.checked_at)?;
        let effective_from = parse_timestamp(&self.effective_from)?;
        if self
            .valid_until
            .as_deref()
            .map(parse_timestamp)
            .transpose()?
            .is_some_and(|valid_until| valid_until <= effective_from)
        {
            return Err(PriceError::InvalidInterval);
        }
        Ok(())
    }

    /// Checks an instant against the price's half-open effective interval.
    pub fn is_effective_at(&self, timestamp: &str) -> Result<bool, PriceError> {
        self.validate()?;
        let timestamp = parse_timestamp(timestamp)?;
        let effective_from = parse_timestamp(&self.effective_from)?;
        let valid_until = self
            .valid_until
            .as_deref()
            .map(parse_timestamp)
            .transpose()?;
        Ok(timestamp >= effective_from && valid_until.is_none_or(|end| timestamp < end))
    }

    /// Returns a bounded default run budget derived from a caller-selected reference workload.
    /// The result remains in this price version's currency and is never converted.
    pub fn reference_budget(
        &self,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<crate::money::NanoMoney, PriceError> {
        self.validate()?;
        let total = crate::money::cost(
            input_tokens,
            0,
            output_tokens,
            self.unit_tokens,
            &self.input_uncached,
            &self.input_cached,
            &self.output,
        )?;
        total.settle().map_err(PriceError::Money)
    }

    /// Prices the complete input at its more expensive tier and the full output cap.
    pub fn upper_bound_cost(
        &self,
        max_input_tokens: u64,
        max_output_tokens: u64,
    ) -> Result<crate::money::ExactCost, PriceError> {
        self.validate()?;
        let input = self.input_uncached.clone().max(self.input_cached.clone());
        crate::money::cost(
            max_input_tokens,
            0,
            max_output_tokens,
            self.unit_tokens,
            &input,
            &input,
            &self.output,
        )
        .map_err(PriceError::Money)
    }
}

fn parse_timestamp(value: &str) -> Result<OffsetDateTime, PriceError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| PriceError::InvalidInterval)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceError {
    InvalidIdentity,
    InvalidUnit,
    InvalidSource,
    InvalidInterval,
    InvalidUsage,
    Money(MoneyError),
}

impl From<MoneyError> for PriceError {
    fn from(error: MoneyError) -> Self {
        Self::Money(error)
    }
}

impl fmt::Display for PriceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidIdentity => "invalid price identity",
            Self::InvalidUnit => "invalid price unit",
            Self::InvalidSource => "invalid price source",
            Self::InvalidInterval => "invalid price interval",
            Self::InvalidUsage => "invalid token usage",
            Self::Money(error) => return error.fmt(f),
        })
    }
}

impl std::error::Error for PriceError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PriceVersion {
        PriceVersion {
            version_id: "price-1".into(),
            provider_id: "deepseek-direct".into(),
            model_id: "deepseek-chat".into(),
            route_policy: "direct".into(),
            currency: Currency::Cny,
            unit_tokens: 1_000_000,
            input_uncached: UnitPrice::parse("2").unwrap(),
            input_cached: UnitPrice::parse("1").unwrap(),
            output: UnitPrice::parse("8").unwrap(),
            usage_mapping_version: "openai-usage-v1".into(),
            source_kind: "user-entered".into(),
            source_url: None,
            checked_at: "2026-10-11T00:00:00Z".into(),
            effective_from: "2026-10-11T00:00:00Z".into(),
            valid_until: None,
        }
    }

    #[test]
    fn price_version_requires_full_bounded_identity() {
        assert!(fixture().validate().is_ok());
        let mut invalid = fixture();
        invalid.provider_id = "  ".into();
        assert_eq!(invalid.validate(), Err(PriceError::InvalidIdentity));
        let mut invalid = fixture();
        invalid.unit_tokens = 0;
        assert_eq!(invalid.validate(), Err(PriceError::InvalidUnit));
        let mut invalid = fixture();
        invalid.valid_until = Some("2026-10-10T00:00:00Z".into());
        assert_eq!(invalid.validate(), Err(PriceError::InvalidInterval));
        let mut invalid = fixture();
        invalid.checked_at = "not-a-timestamp".into();
        assert_eq!(invalid.validate(), Err(PriceError::InvalidInterval));
    }

    #[test]
    fn price_effective_intervals_compare_instants_across_offsets() {
        let mut price = fixture();
        price.effective_from = "2026-10-11T00:00:00-08:00".into();
        price.valid_until = Some("2026-10-11T01:00:00-08:00".into());

        assert!(!price.is_effective_at("2026-10-11T04:00:00Z").unwrap());
        assert!(price.is_effective_at("2026-10-11T08:30:00Z").unwrap());
        assert!(!price.is_effective_at("2026-10-11T09:00:00Z").unwrap());
        assert!(!price.is_effective_at("2026-10-11T00:00:00Z").unwrap());
    }

    #[test]
    fn price_estimates_reference_and_conservative_request_costs() {
        let price = fixture();
        assert_eq!(
            price.reference_budget(1_000_000, 500_000).unwrap(),
            crate::money::NanoMoney::parse("6").unwrap()
        );
        let reserved = price.upper_bound_cost(1_000_000, 500_000).unwrap();
        assert_eq!(
            reserved.settle().unwrap(),
            crate::money::NanoMoney::parse("6").unwrap()
        );
    }

    #[test]
    fn price_validation_and_estimation_preserve_typed_failures() {
        let mut price = fixture();
        price.version_id = format!("{}\n", price.version_id);
        assert_eq!(price.validate(), Err(PriceError::InvalidIdentity));

        let mut price = fixture();
        price.source_url = Some(String::new());
        assert_eq!(price.validate(), Err(PriceError::InvalidSource));

        let mut price = fixture();
        price.source_url = Some("x".repeat(2_049));
        assert_eq!(price.reference_budget(1, 1), Err(PriceError::InvalidSource));

        let price = fixture();
        assert!(matches!(
            price.reference_budget(u64::MAX, u64::MAX),
            Err(PriceError::Money(_))
        ));

        let mut price = fixture();
        price.unit_tokens = 1;
        price.input_uncached = UnitPrice::parse("100000000000").unwrap();
        assert!(matches!(
            price.reference_budget(u64::MAX, 0),
            Err(PriceError::Money(_))
        ));

        assert_eq!(PriceError::InvalidUsage.to_string(), "invalid token usage");
        assert_eq!(
            PriceError::InvalidSource.to_string(),
            "invalid price source"
        );
        assert_eq!(
            PriceError::InvalidIdentity.to_string(),
            "invalid price identity"
        );
        assert_eq!(PriceError::InvalidUnit.to_string(), "invalid price unit");
        assert_eq!(
            PriceError::InvalidInterval.to_string(),
            "invalid price interval"
        );
        assert_eq!(
            PriceError::Money(MoneyError::InvalidPrice).to_string(),
            "invalid price"
        );
        assert_eq!(
            PriceError::from(MoneyError::Overflow),
            PriceError::Money(MoneyError::Overflow)
        );
        let _: &dyn std::error::Error = &PriceError::InvalidUnit;
    }
}
