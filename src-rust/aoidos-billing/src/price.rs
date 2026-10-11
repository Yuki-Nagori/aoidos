//! 不可变价格身份；仅凭模型名不能确定计价版本。

use crate::money::{Currency, MoneyError, TokenPrices, UnitPrice};
use std::fmt;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_ID_BYTES: usize = 128;
const MAX_SOURCE_URL_BYTES: usize = 2_048;
const OPENCODE_GO_DEEPSEEK_SCHEDULE: &str = "opencode-go-deepseek-utc-weekday-v1";
const DEEPSEEK_API_SCHEDULE: &str = "deepseek-api-utc-weekday-holiday-2026-v1";

impl RecurringPriceSchedule {
    fn is_peak_at(&self, timestamp: &str) -> Result<bool, PriceError> {
        let utc = parse_timestamp(timestamp)?.to_offset(time::UtcOffset::UTC);
        if self.version == DEEPSEEK_API_SCHEDULE {
            let china_offset =
                time::UtcOffset::from_hms(8, 0, 0).map_err(|_| PriceError::InvalidSchedule)?;
            if chinese_public_holiday_2026(utc.to_offset(china_offset)) {
                return Ok(false);
            }
        }
        let timestamp = utc;
        let weekday = timestamp.weekday().number_from_monday();
        let minute = u16::from(timestamp.hour()) * 60 + u16::from(timestamp.minute());
        Ok((1..=5).contains(&weekday)
            && ((60..240).contains(&minute) || (360..600).contains(&minute)))
    }
}

/// 2026 年国务院公布的中国公众假日；未覆盖年份不授予假日折扣，按峰价保守估算。
fn chinese_public_holiday_2026(timestamp: OffsetDateTime) -> bool {
    let date = timestamp.date();
    if date.year() != 2026 {
        return false;
    }
    let month = date.month() as u8;
    let day = date.day();
    matches!(
        (month, day),
        (1, 1..=3)
            | (2, 15..=23)
            | (4, 4..=6)
            | (5, 1..=5)
            | (6, 19..=21)
            | (9, 25..=27)
            | (10, 1..=7)
    )
}

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
    #[serde(default)]
    pub input_cached_write: Option<UnitPrice>,
    pub output: UnitPrice,
    #[serde(default)]
    pub schedule: Option<RecurringPriceSchedule>,
    pub usage_mapping_version: String,
    pub source_kind: String,
    pub source_url: Option<String>,
    pub checked_at: String,
    pub effective_from: String,
    pub valid_until: Option<String>,
}

/// OpenCode Go DeepSeek 的峰时单价；基础单价字段表示谷时价格。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecurringPriceSchedule {
    pub version: String,
    pub peak: PeakPriceTier,
}

/// 每个 Token 类别独立保存峰时价格，避免缓存档位被误套普通输入单价。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeakPriceTier {
    pub input_uncached: UnitPrice,
    pub input_cached: UnitPrice,
    pub input_cached_write: Option<UnitPrice>,
    pub output: UnitPrice,
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
        if let Some(schedule) = &self.schedule {
            let supported = match schedule.version.as_str() {
                OPENCODE_GO_DEEPSEEK_SCHEDULE => {
                    self.provider_id == "opencode-go"
                        && matches!(
                            self.model_id.as_str(),
                            "deepseek-v4.1-flash"
                                | "deepseek-v4-pro"
                                | "deepseek-v4-flash"
                                | "deepseek-v4-flash-vision-exp"
                        )
                }
                DEEPSEEK_API_SCHEDULE => {
                    self.provider_id == "deepseek"
                        && matches!(self.model_id.as_str(), "deepseek-flash" | "deepseek-v4-pro")
                }
                _ => false,
            };
            if !supported {
                return Err(PriceError::InvalidSchedule);
            }
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
        self.upper_bound_cost(input_tokens, output_tokens)?
            .settle()
            .map_err(PriceError::Money)
    }

    /// 根据持久发送时刻选择该次请求的原币费率。
    pub fn token_prices_at(
        &self,
        timestamp: &str,
        cache_write_tokens: u64,
    ) -> Result<TokenPrices<'_>, PriceError> {
        let fallback = &UnitPrice::ZERO;
        if let Some(schedule) = &self.schedule
            && schedule.is_peak_at(timestamp)?
        {
            let cache_write = schedule.peak.input_cached_write.as_ref();
            if cache_write_tokens > 0 && cache_write.is_none() {
                return Err(PriceError::CacheWritePriceMissing);
            }
            return Ok(TokenPrices {
                input_uncached: &schedule.peak.input_uncached,
                input_cached: &schedule.peak.input_cached,
                input_cached_write: cache_write.unwrap_or(fallback),
                output: &schedule.peak.output,
            });
        }
        if cache_write_tokens > 0 && self.input_cached_write.is_none() {
            return Err(PriceError::CacheWritePriceMissing);
        }
        Ok(TokenPrices {
            input_uncached: &self.input_uncached,
            input_cached: &self.input_cached,
            input_cached_write: self.input_cached_write.as_ref().unwrap_or(fallback),
            output: &self.output,
        })
    }

    /// Prices the complete input at its more expensive tier and the full output cap.
    pub fn upper_bound_cost(
        &self,
        max_input_tokens: u64,
        max_output_tokens: u64,
    ) -> Result<crate::money::ExactCost, PriceError> {
        self.validate()?;
        let schedule = self.schedule.as_ref();
        let peak = schedule.map(|schedule| &schedule.peak);
        let write_price = self
            .input_cached_write
            .as_ref()
            .ok_or(PriceError::CacheWritePriceMissing)?;
        let peak_write_price = peak
            .map(|peak| {
                peak.input_cached_write
                    .as_ref()
                    .ok_or(PriceError::CacheWritePriceMissing)
            })
            .transpose()?
            .unwrap_or(write_price);
        let input = self
            .input_uncached
            .clone()
            .max(self.input_cached.clone())
            .max(write_price.clone())
            .max(peak.map_or_else(
                || UnitPrice::ZERO.clone(),
                |peak| peak.input_uncached.clone(),
            ))
            .max(peak.map_or_else(|| UnitPrice::ZERO.clone(), |peak| peak.input_cached.clone()))
            .max(peak_write_price.clone());
        let output = peak.map_or_else(
            || self.output.clone(),
            |peak| self.output.clone().max(peak.output.clone()),
        );
        crate::money::cost_with_cache_write(
            max_input_tokens,
            0,
            0,
            max_output_tokens,
            self.unit_tokens,
            &TokenPrices {
                input_uncached: &input,
                input_cached: &input,
                input_cached_write: &input,
                output: &output,
            },
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
    CacheWritePriceMissing,
    InvalidSchedule,
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
            Self::CacheWritePriceMissing => "cache-write price is missing",
            Self::InvalidSchedule => "invalid recurring price schedule",
            Self::Money(error) => return error.fmt(f),
        })
    }
}

impl std::error::Error for PriceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::NormalizedUsage;

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
            input_cached_write: Some(UnitPrice::parse("3").unwrap()),
            output: UnitPrice::parse("8").unwrap(),
            schedule: None,
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
            crate::money::NanoMoney::parse("7").unwrap()
        );
        let reserved = price.upper_bound_cost(1_000_000, 500_000).unwrap();
        assert_eq!(
            reserved.settle().unwrap(),
            crate::money::NanoMoney::parse("7").unwrap()
        );
    }

    fn scheduled_price() -> PriceVersion {
        let mut price = fixture();
        price.provider_id = "opencode-go".into();
        price.model_id = "deepseek-v4.1-flash".into();
        price.schedule = Some(RecurringPriceSchedule {
            version: OPENCODE_GO_DEEPSEEK_SCHEDULE.into(),
            peak: PeakPriceTier {
                input_uncached: UnitPrice::parse("3").unwrap(),
                input_cached: UnitPrice::parse("1.5").unwrap(),
                input_cached_write: Some(UnitPrice::ZERO),
                output: UnitPrice::parse("9").unwrap(),
            },
        });
        price
    }

    #[test]
    fn go_deepseek_schedule_uses_utc_weekday_half_open_windows() {
        let price = scheduled_price();
        for timestamp in [
            "2026-10-12T00:59:59Z",
            "2026-10-12T04:00:00Z",
            "2026-10-12T05:59:59Z",
            "2026-10-12T10:00:00Z",
            "2026-10-10T02:00:00Z",
            "2026-10-11T02:00:00Z",
        ] {
            assert_eq!(
                price.token_prices_at(timestamp, 0).unwrap().output,
                &price.output
            );
        }
        for timestamp in [
            "2026-10-12T01:00:00Z",
            "2026-10-12T03:59:59Z",
            "2026-10-12T06:00:00Z",
            "2026-10-12T09:59:59Z",
        ] {
            assert_eq!(
                price.token_prices_at(timestamp, 0).unwrap().output,
                &price.schedule.as_ref().unwrap().peak.output
            );
        }
        assert_eq!(
            price
                .token_prices_at("2026-10-12T06:00:00+02:00", 0)
                .unwrap()
                .output,
            &price.output
        );
    }

    #[test]
    fn scheduled_settlement_uses_dispatch_instant_and_reserve_covers_peak() {
        let price = scheduled_price();
        let usage =
            NormalizedUsage::from_provider_with_cache_write(100, None, Some(10), 20, None).unwrap();
        assert_eq!(
            usage
                .exact_cost_at(&price, "2026-10-12T01:00:00Z")
                .unwrap()
                .settle()
                .unwrap(),
            crate::money::NanoMoney::parse("0.00045").unwrap()
        );
        assert_eq!(
            usage
                .exact_cost_at(&price, "2026-10-12T04:00:00Z")
                .unwrap()
                .settle()
                .unwrap(),
            crate::money::NanoMoney::parse("0.00037").unwrap()
        );
        assert_eq!(
            price.upper_bound_cost(100, 20).unwrap().settle().unwrap(),
            crate::money::NanoMoney::parse("0.00048").unwrap()
        );
    }

    #[test]
    fn recurring_schedule_is_limited_to_known_opencode_go_models() {
        let mut price = scheduled_price();
        price.provider_id = "deepseek-direct".into();
        assert_eq!(price.validate(), Err(PriceError::InvalidSchedule));
        let mut price = scheduled_price();
        price.model_id = "deepseek-chat".into();
        assert_eq!(price.validate(), Err(PriceError::InvalidSchedule));
        let mut price = scheduled_price();
        price.schedule.as_mut().unwrap().version = "unknown".into();
        assert_eq!(price.validate(), Err(PriceError::InvalidSchedule));
    }

    #[test]
    fn deepseek_api_peak_schedule_excludes_2026_chinese_holidays() {
        let mut price = scheduled_price();
        price.provider_id = "deepseek".into();
        price.model_id = "deepseek-flash".into();
        price.schedule.as_mut().unwrap().version = DEEPSEEK_API_SCHEDULE.into();
        let peak_output = &price.schedule.as_ref().unwrap().peak.output;

        assert_eq!(
            price
                .token_prices_at("2026-10-12T01:00:00Z", 0)
                .unwrap()
                .output,
            peak_output
        );
        assert_eq!(
            price
                .token_prices_at("2026-10-01T01:30:00Z", 0)
                .unwrap()
                .output,
            &price.output
        );
        assert_eq!(
            price
                .token_prices_at("2026-10-10T01:30:00Z", 0)
                .unwrap()
                .output,
            &price.output
        );
        assert_eq!(
            price
                .token_prices_at("2027-10-04T01:30:00Z", 0)
                .unwrap()
                .output,
            peak_output
        );
    }

    #[test]
    fn deepseek_api_schedule_covers_every_day_of_the_2026_public_holidays() {
        let mut price = scheduled_price();
        price.provider_id = "deepseek".into();
        price.model_id = "deepseek-flash".into();
        price.schedule.as_mut().unwrap().version = DEEPSEEK_API_SCHEDULE.into();
        let china_offset = time::UtcOffset::from_hms(8, 0, 0).unwrap();
        let mut weekday_holidays = 0;

        for (month, first_day, last_day) in [
            (1, 1, 3),
            (2, 15, 23),
            (4, 4, 6),
            (5, 1, 5),
            (6, 19, 21),
            (9, 25, 27),
            (10, 1, 7),
        ] {
            for day in first_day..=last_day {
                let date = time::Date::from_calendar_date(
                    2026,
                    time::Month::try_from(month).unwrap(),
                    day,
                )
                .unwrap();
                if (1..=5).contains(&date.weekday().number_from_monday()) {
                    weekday_holidays += 1;
                }
                let peak_instant = date
                    .with_time(time::Time::from_hms(9, 30, 0).unwrap())
                    .assume_offset(china_offset)
                    .to_offset(time::UtcOffset::UTC)
                    .format(&Rfc3339)
                    .unwrap();
                assert_eq!(
                    price.token_prices_at(&peak_instant, 0).unwrap().output,
                    &price.output,
                    "{date} is an official 2026 Chinese public holiday"
                );
            }
        }
        assert_eq!(weekday_holidays, 19);

        // Make-up work Saturdays remain off-peak because the provider's rule is Mon–Fri.
        for timestamp in ["2026-02-14T01:30:00Z", "2026-02-28T01:30:00Z"] {
            assert_eq!(
                price.token_prices_at(timestamp, 0).unwrap().output,
                &price.output
            );
        }
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

    #[test]
    fn deepseek_api_schedule_rejects_other_providers_and_models() {
        let mut price = scheduled_price();
        price.schedule.as_mut().unwrap().version = DEEPSEEK_API_SCHEDULE.into();
        assert_eq!(price.validate(), Err(PriceError::InvalidSchedule));
        price.provider_id = "deepseek".into();
        assert_eq!(price.validate(), Err(PriceError::InvalidSchedule));
        for model in ["deepseek-flash", "deepseek-v4-pro"] {
            price.model_id = model.into();
            assert_eq!(price.validate(), Ok(()));
        }
    }

    #[test]
    fn cache_write_prices_are_required_only_for_reported_writes_in_the_selected_tier() {
        let mut price = scheduled_price();
        price.input_cached_write = None;
        price.schedule.as_mut().unwrap().peak.input_cached_write = None;
        for timestamp in ["2026-10-12T01:00:00Z", "2026-10-12T04:00:00Z"] {
            assert_eq!(
                price
                    .token_prices_at(timestamp, 0)
                    .unwrap()
                    .input_cached_write,
                &UnitPrice::ZERO
            );
            assert!(matches!(
                price.token_prices_at(timestamp, 1),
                Err(PriceError::CacheWritePriceMissing)
            ));
        }
        assert_eq!(
            PriceError::CacheWritePriceMissing.to_string(),
            "cache-write price is missing"
        );
        assert_eq!(
            PriceError::InvalidSchedule.to_string(),
            "invalid recurring price schedule"
        );
    }
}
