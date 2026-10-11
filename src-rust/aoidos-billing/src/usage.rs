//! 有界地归一化服务商用量；缺失值保持未知。

use crate::{
    money::{ExactCost, TokenPrices, cost_with_cache_write},
    price::{PriceError, PriceVersion},
};

/// 归一化 Token 用量；推理是输出子集，缓存是输入子集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedUsage {
    pub input_total: u64,
    pub input_cached: u64,
    pub input_cached_write: u64,
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
        Self::from_provider_with_cache_write(
            input_total,
            input_cached,
            None,
            output_total,
            reasoning_included,
        )
    }

    /// 分别归一化供应商报告的缓存读取与缓存创建 Token。
    pub fn from_provider_with_cache_write(
        input_total: u64,
        input_cached: Option<u64>,
        input_cached_write: Option<u64>,
        output_total: u64,
        reasoning_included: Option<u64>,
    ) -> Result<Self, PriceError> {
        let input_cached = input_cached.unwrap_or(0);
        let input_cached_write = input_cached_write.unwrap_or(0);
        let cached_total = input_cached
            .checked_add(input_cached_write)
            .ok_or(PriceError::InvalidUsage)?;
        if cached_total > input_total
            || reasoning_included.is_some_and(|reasoning| reasoning > output_total)
        {
            return Err(PriceError::InvalidUsage);
        }
        let input_uncached = input_total - cached_total;
        let total_tokens = input_total
            .checked_add(output_total)
            .ok_or(PriceError::InvalidUsage)?;
        Ok(Self {
            input_total,
            input_cached,
            input_cached_write,
            input_uncached,
            output_total,
            reasoning_included,
            total_tokens,
        })
    }

    /// Returns exact unrounded cost; callers choose reservation or settlement rounding.
    pub fn exact_cost(&self, price: &PriceVersion) -> Result<ExactCost, PriceError> {
        price.validate()?;
        if price.schedule.is_some() {
            return Err(PriceError::InvalidSchedule);
        }
        if self.input_cached_write > 0 && price.input_cached_write.is_none() {
            return Err(PriceError::CacheWritePriceMissing);
        }
        self.exact_cost_with_prices(
            price,
            TokenPrices {
                input_uncached: &price.input_uncached,
                input_cached: &price.input_cached,
                input_cached_write: price
                    .input_cached_write
                    .as_ref()
                    .unwrap_or(&crate::money::UnitPrice::ZERO),
                output: &price.output,
            },
        )
    }

    /// 根据请求发送时刻选择不可变时段价格后计算精确费用。
    pub fn exact_cost_at(
        &self,
        price: &PriceVersion,
        dispatch_at: &str,
    ) -> Result<ExactCost, PriceError> {
        price.validate()?;
        let prices = price.token_prices_at(dispatch_at, self.input_cached_write)?;
        self.exact_cost_with_prices(price, prices)
    }

    fn exact_cost_with_prices(
        &self,
        price: &PriceVersion,
        prices: TokenPrices<'_>,
    ) -> Result<ExactCost, PriceError> {
        cost_with_cache_write(
            self.input_uncached,
            self.input_cached,
            self.input_cached_write,
            self.output_total,
            price.unit_tokens,
            &prices,
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
            input_cached_write: Some(UnitPrice::parse("3").unwrap()),
            output: UnitPrice::parse("4").unwrap(),
            schedule: None,
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
        let usage =
            NormalizedUsage::from_provider_with_cache_write(100, Some(20), Some(30), 50, None)
                .unwrap();
        assert_eq!(usage.input_uncached, 50);
        assert_eq!(usage.input_cached, 20);
        assert_eq!(usage.input_cached_write, 30);
        assert_eq!(usage.total_tokens, 150);
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
            NormalizedUsage::from_provider_with_cache_write(10, Some(6), Some(5), 5, None),
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

        let cached_write =
            NormalizedUsage::from_provider_with_cache_write(100, Some(20), Some(30), 50, None)
                .unwrap();
        assert_eq!(
            cached_write
                .exact_cost(&price())
                .unwrap()
                .settle()
                .unwrap()
                .to_string(),
            "0.41"
        );

        let mut invalid = price();
        invalid.unit_tokens = 0;
        assert_eq!(usage.exact_cost(&invalid), Err(PriceError::InvalidUnit));

        let mut missing_cache_price = price();
        missing_cache_price.input_cached_write = None;
        assert_eq!(
            cached_write.exact_cost(&missing_cache_price),
            Err(PriceError::CacheWritePriceMissing)
        );

        let overflowing = NormalizedUsage {
            input_total: u64::MAX,
            input_cached: u64::MAX,
            input_cached_write: 0,
            input_uncached: u64::MAX,
            output_total: u64::MAX,
            reasoning_included: None,
            total_tokens: u64::MAX,
        };
        let mut expensive = price();
        expensive.unit_tokens = 1;
        expensive.input_uncached = UnitPrice::parse("9223372036.854775807").unwrap();
        expensive.input_cached = expensive.input_uncached.clone();
        expensive.input_cached_write = Some(expensive.input_uncached.clone());
        expensive.output = expensive.input_uncached.clone();
        assert!(matches!(
            overflowing.exact_cost(&expensive),
            Err(PriceError::Money(MoneyError::Overflow))
        ));
    }

    #[test]
    fn scheduled_usage_requires_a_dispatch_timestamp() {
        let mut price = price();
        price.provider_id = "deepseek".into();
        price.model_id = "deepseek-flash".into();
        price.schedule = Some(crate::price::RecurringPriceSchedule {
            version: "deepseek-api-utc-weekday-holiday-2026-v1".into(),
            peak: crate::price::PeakPriceTier {
                input_uncached: UnitPrice::parse("3").unwrap(),
                input_cached: UnitPrice::parse("1.5").unwrap(),
                input_cached_write: Some(UnitPrice::parse("4").unwrap()),
                output: UnitPrice::parse("6").unwrap(),
            },
        });
        let usage = NormalizedUsage::from_provider(100, Some(25), 50, None).unwrap();
        assert_eq!(usage.exact_cost(&price), Err(PriceError::InvalidSchedule));
        assert_eq!(
            usage
                .exact_cost_at(&price, "2026-10-12T01:00:00Z")
                .unwrap()
                .settle()
                .unwrap()
                .to_string(),
            "0.5625"
        );
    }
}
