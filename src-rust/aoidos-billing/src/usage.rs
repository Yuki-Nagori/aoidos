//! 有界地归一化服务商用量；缺失值保持未知。

use crate::{
    money::{ExactCost, cost},
    price::{PriceError, PriceVersion},
};

/// 归一化 Token 用量；推理是输出子集，缓存是输入子集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedUsage {
    pub input_total: u64,
    pub input_cached: u64,
    pub input_uncached: u64,
    pub output_total: u64,
    pub reasoning_included: Option<u64>,
    pub total_tokens: u64,
}

impl NormalizedUsage {
    /// Maps common provider totals while validating subset relationships and overflow.
    pub fn from_provider(
        input_total: u64,
        input_cached: Option<u64>,
        output_total: u64,
        reasoning_included: Option<u64>,
    ) -> Result<Self, PriceError> {
        let input_cached = input_cached.unwrap_or(0);
        if input_cached > input_total
            || reasoning_included.is_some_and(|reasoning| reasoning > output_total)
        {
            return Err(PriceError::InvalidUsage);
        }
        let input_uncached = input_total - input_cached;
        let total_tokens = input_total
            .checked_add(output_total)
            .ok_or(PriceError::InvalidUsage)?;
        Ok(Self {
            input_total,
            input_cached,
            input_uncached,
            output_total,
            reasoning_included,
            total_tokens,
        })
    }

    /// Returns exact unrounded cost; callers choose reservation or settlement rounding.
    pub fn exact_cost(&self, price: &PriceVersion) -> Result<ExactCost, PriceError> {
        price.validate()?;
        cost(
            self.input_uncached,
            self.input_cached,
            self.output_total,
            price.unit_tokens,
            &price.input_uncached,
            &price.input_cached,
            &price.output,
        )
        .map_err(PriceError::Money)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        money::{Currency, MoneyError, UnitPrice},
        price::PriceVersion,
    };

    fn price() -> PriceVersion {
        PriceVersion {
            version_id: "v1".into(),
            provider_id: "provider".into(),
            model_id: "model".into(),
            route_policy: "direct".into(),
            currency: Currency::Usd,
            unit_tokens: 1000,
            input_uncached: UnitPrice::parse("2").unwrap(),
            input_cached: UnitPrice::parse("1").unwrap(),
            output: UnitPrice::parse("4").unwrap(),
            usage_mapping_version: "v1".into(),
            source_kind: "user".into(),
            source_url: None,
            checked_at: "2026-10-11T00:00:00Z".into(),
            effective_from: "2026-10-11T00:00:00Z".into(),
            valid_until: None,
        }
    }

    #[test]
    fn normalizes_cache_and_reasoning_without_double_counting() {
        let usage = NormalizedUsage::from_provider(100, Some(25), 50, Some(20)).unwrap();
        assert_eq!(usage.input_uncached, 75);
        assert_eq!(usage.total_tokens, 150);
        assert_eq!(usage.reasoning_included, Some(20));
    }

    #[test]
    fn rejects_inconsistent_or_overflowing_usage() {
        assert_eq!(
            NormalizedUsage::from_provider(10, Some(11), 5, None),
            Err(PriceError::InvalidUsage)
        );
        assert_eq!(
            NormalizedUsage::from_provider(10, None, 5, Some(6)),
            Err(PriceError::InvalidUsage)
        );
        assert_eq!(
            NormalizedUsage::from_provider(u64::MAX, None, 1, None),
            Err(PriceError::InvalidUsage)
        );
    }

    #[test]
    fn prices_normalized_usage_and_propagates_invalid_price_data() {
        let usage = NormalizedUsage::from_provider(100, Some(25), 50, None).unwrap();
        assert_eq!(
            usage
                .exact_cost(&price())
                .unwrap()
                .settle()
                .unwrap()
                .to_string(),
            "0.375"
        );

        let mut invalid = price();
        invalid.unit_tokens = 0;
        assert_eq!(usage.exact_cost(&invalid), Err(PriceError::InvalidUnit));

        let overflowing = NormalizedUsage {
            input_total: u64::MAX,
            input_cached: u64::MAX,
            input_uncached: u64::MAX,
            output_total: u64::MAX,
            reasoning_included: None,
            total_tokens: u64::MAX,
        };
        let mut expensive = price();
        expensive.unit_tokens = 1;
        expensive.input_uncached = UnitPrice::parse("9223372036.854775807").unwrap();
        expensive.input_cached = expensive.input_uncached.clone();
        expensive.output = expensive.input_uncached.clone();
        assert!(matches!(
            overflowing.exact_cost(&expensive),
            Err(PriceError::Money(MoneyError::Overflow))
        ));
    }
}
