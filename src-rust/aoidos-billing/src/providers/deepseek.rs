//! DeepSeek API 官方 Token 价格快照。

use crate::{
    money::{Currency, UnitPrice},
    price::{PeakPriceTier, PriceVersion, RecurringPriceSchedule},
};

const CHECKED_AT: &str = "2026-10-21T00:00:00Z";
const EFFECTIVE_FROM: &str = "2026-10-21T00:00:00Z";
const SCHEDULE: &str = "deepseek-api-utc-weekday-holiday-2026-v2";
const USAGE_MAPPING: &str = "deepseek-chat-completion-usage-v2";
/// 历史请求没有独立缓存写入计数；恢复时按零写入量解释。
pub const LEGACY_USAGE_MAPPING: &str = "deepseek-chat-completion-usage-v1";
const CNY_SOURCE: &str = "https://api-docs.deepseek.com/zh-cn/quick_start/pricing/";
const USD_SOURCE: &str = "https://api-docs.deepseek.com/quick_start/pricing/";

/// DeepSeek profile 的初始币种预选；界面仍允许用户改选 USD。
#[must_use]
pub const fn default_currency() -> Currency {
    Currency::Cny
}

/// 返回 DeepSeek Flash 的 CNY / USD 官方价目；返回值尚未登记到账本。
///
/// 缓存写入按未命中输入费率计价，因为 DeepSeek 文档只区分缓存命中与未命中，
/// 没有单独的缓存写入折扣。调用方须让用户选择币种并确认后再登记。
pub fn flash_prices() -> Vec<PriceVersion> {
    [
        (
            Currency::Cny,
            "deepseek-flash-cny-2026-10-21-v3",
            "0.02",
            "1",
            "4",
            "0.04",
            "2",
            "8",
            CNY_SOURCE,
        ),
        (
            Currency::Usd,
            "deepseek-flash-usd-2026-10-21-v3",
            "0.003",
            "0.15",
            "0.6",
            "0.006",
            "0.3",
            "1.2",
            USD_SOURCE,
        ),
    ]
    .into_iter()
    .map(
        |(
            currency,
            version_id,
            cached,
            uncached,
            output,
            peak_cached,
            peak_uncached,
            peak_output,
            source_url,
        )| {
            let input_uncached = price(uncached);
            PriceVersion {
                version_id: version_id.into(),
                provider_id: "deepseek".into(),
                model_id: "deepseek-flash".into(),
                route_policy: "direct".into(),
                currency,
                unit_tokens: 1_000_000,
                input_uncached: input_uncached.clone(),
                input_cached: price(cached),
                input_cached_write: Some(input_uncached),
                output: price(output),
                schedule: Some(RecurringPriceSchedule {
                    version: SCHEDULE.into(),
                    peak: PeakPriceTier {
                        input_uncached: price(peak_uncached),
                        input_cached: price(peak_cached),
                        input_cached_write: Some(price(peak_uncached)),
                        output: price(peak_output),
                    },
                }),
                usage_mapping_version: USAGE_MAPPING.into(),
                source_kind: "official-provider-price".into(),
                source_url: Some(source_url.into()),
                checked_at: CHECKED_AT.into(),
                effective_from: EFFECTIVE_FROM.into(),
                valid_until: None,
            }
        },
    )
    .collect()
}

fn price(value: &str) -> UnitPrice {
    UnitPrice::parse(value).expect("内置价目必须是有效十进制数")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_prices_are_valid_and_keep_native_currency_amounts() {
        assert_eq!(default_currency(), Currency::Cny);
        let prices = flash_prices();
        assert_eq!(prices.len(), 2);
        assert_eq!(prices[0].version_id, "deepseek-flash-cny-2026-10-21-v3");
        assert_eq!(prices[1].version_id, "deepseek-flash-usd-2026-10-21-v3");
        assert_eq!(prices[0].usage_mapping_version, USAGE_MAPPING);
        assert_eq!(USAGE_MAPPING, "deepseek-chat-completion-usage-v2");
        assert_eq!(prices[0].currency, Currency::Cny);
        assert_eq!(prices[0].input_uncached.to_decimal_string(), "1");
        assert_eq!(prices[0].input_cached.to_decimal_string(), "0.02");
        assert_eq!(
            prices[0]
                .input_cached_write
                .as_ref()
                .unwrap()
                .to_decimal_string(),
            "1"
        );
        assert_eq!(prices[0].output.to_decimal_string(), "4");
        assert_eq!(prices[1].currency, Currency::Usd);
        assert_eq!(prices[1].input_uncached.to_decimal_string(), "0.15");
        assert_eq!(prices[1].input_cached.to_decimal_string(), "0.003");
        assert_eq!(prices[1].output.to_decimal_string(), "0.6");
        for price in prices {
            price.validate().unwrap();
            assert_eq!(price.unit_tokens, 1_000_000);
            assert!(price.schedule.is_some());
        }
    }
}
