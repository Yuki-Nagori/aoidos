//! 不经过浮点的金额解析、计算与 IPC 表示。

use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};

const NANOS_PER_UNIT: i64 = 1_000_000_000;
const MAX_SCALE: u32 = 9;

fn is_plain_decimal(value: &str) -> bool {
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (value, None),
    };
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.is_none_or(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Currency {
    Cny,
    Usd,
}

impl Currency {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cny => "CNY",
            Self::Usd => "USD",
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Currency {
    type Err = MoneyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "CNY" => Ok(Self::Cny),
            "USD" => Ok(Self::Usd),
            _ => Err(MoneyError::Currency),
        }
    }
}

impl Serialize for Currency {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// 以 `10^-9` 个币种单位储存的非负金额。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NanoMoney(i64);

impl NanoMoney {
    pub const ZERO: Self = Self(0);

    /// Reconstructs a persisted non-negative nano amount.
    pub fn from_nanos(value: i64) -> Result<Self, MoneyError> {
        if value < 0 {
            Err(MoneyError::InvalidAmount)
        } else {
            Ok(Self(value))
        }
    }

    /// 从十进制字符串精确解析，最多九位有效小数，不接受负值或指数写法。
    pub fn parse(value: &str) -> Result<Self, MoneyError> {
        if !is_plain_decimal(value) {
            return Err(MoneyError::InvalidAmount);
        }
        let decimal = Decimal::from_str_exact(value).map_err(|_| MoneyError::InvalidAmount)?;
        let normalized = decimal.normalize();
        if normalized.is_sign_negative() || normalized.scale() > MAX_SCALE {
            return Err(MoneyError::InvalidAmount);
        }
        let nanos = normalized
            .checked_mul(Decimal::from(NANOS_PER_UNIT))
            .and_then(|scaled| scaled.to_i64())
            .ok_or(MoneyError::Overflow)?;
        Ok(Self(nanos))
    }

    #[must_use]
    pub const fn nanos(self) -> i64 {
        self.0
    }

    #[must_use]
    pub fn decimal(self) -> Decimal {
        Decimal::new(self.0, MAX_SCALE)
    }

    #[must_use]
    pub fn to_decimal_string(self) -> String {
        self.decimal().normalize().to_string()
    }

    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(MoneyError::Overflow)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, MoneyError> {
        self.0
            .checked_sub(other.0)
            .filter(|value| *value >= 0)
            .map(Self)
            .ok_or(MoneyError::Overflow)
    }

    /// 向上取整到纳币，用于发出请求前的保守预留。
    pub fn from_decimal_ceil(value: Decimal) -> Result<Self, MoneyError> {
        Self::from_decimal_round(value, RoundingStrategy::ToPositiveInfinity)
    }

    /// 按 HALF_UP 规则结算实际消耗。
    pub fn from_decimal_half_up(value: Decimal) -> Result<Self, MoneyError> {
        Self::from_decimal_round(value, RoundingStrategy::MidpointAwayFromZero)
    }

    fn from_decimal_round(value: Decimal, strategy: RoundingStrategy) -> Result<Self, MoneyError> {
        if value.is_sign_negative() {
            return Err(MoneyError::InvalidAmount);
        }
        let rounded = value.round_dp_with_strategy(MAX_SCALE, strategy);
        Self::parse(&rounded.normalize().to_string())
    }
}

impl fmt::Display for NanoMoney {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_decimal_string())
    }
}

impl Serialize for NanoMoney {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_decimal_string())
    }
}

impl<'de> Deserialize<'de> for NanoMoney {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

/// 供应商的单价（币种单位 / unit_tokens），以纳币保存。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnitPrice(i128);

impl UnitPrice {
    pub fn parse(value: &str) -> Result<Self, MoneyError> {
        if !is_plain_decimal(value) {
            return Err(MoneyError::InvalidPrice);
        }
        let price = Decimal::from_str_exact(value).map_err(|_| MoneyError::InvalidPrice)?;
        let normalized = price.normalize();
        if normalized.is_sign_negative() || normalized.scale() > MAX_SCALE {
            return Err(MoneyError::InvalidPrice);
        }
        let nanos = normalized
            .mantissa()
            .checked_mul(10_i128.pow(MAX_SCALE - normalized.scale()))
            .ok_or(MoneyError::Overflow)?;
        Ok(Self(nanos))
    }

    const fn nanos(&self) -> i128 {
        self.0
    }

    #[must_use]
    pub fn to_decimal_string(&self) -> String {
        let whole = self.0 / i128::from(NANOS_PER_UNIT);
        let fraction = self.0 % i128::from(NANOS_PER_UNIT);
        if fraction == 0 {
            return whole.to_string();
        }
        format!("{whole}.{fraction:09}")
            .trim_end_matches('0')
            .to_owned()
    }
}

impl fmt::Display for UnitPrice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_decimal_string())
    }
}

impl Serialize for UnitPrice {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_decimal_string())
    }
}

impl<'de> Deserialize<'de> for UnitPrice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

/// 未舍入的精确请求费用：纳币分子除以 `unit_tokens`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactCost {
    numerator_nanos: i128,
    denominator: u64,
}

impl ExactCost {
    /// 向上取整到纳币，用于发送前的保守预留。
    pub fn reserve(self) -> Result<NanoMoney, MoneyError> {
        self.round(false)
    }

    /// 对完整 usage 按 HALF_UP 舍入到纳币。
    pub fn settle(self) -> Result<NanoMoney, MoneyError> {
        self.round(true)
    }

    fn round(self, half_up: bool) -> Result<NanoMoney, MoneyError> {
        let denominator = i128::from(self.denominator);
        let whole = self.numerator_nanos / denominator;
        let remainder = self.numerator_nanos % denominator;
        let round_up = if half_up {
            remainder.checked_mul(2).ok_or(MoneyError::Overflow)? >= denominator
        } else {
            remainder != 0
        };
        let nanos = whole
            .checked_add(i128::from(round_up))
            .ok_or(MoneyError::Overflow)?;
        i64::try_from(nanos)
            .map(NanoMoney)
            .map_err(|_| MoneyError::Overflow)
    }
}

/// 按供应商单价计算一次请求的精确费用，不进行逐 Token 舍入。
pub fn cost(
    input_uncached: u64,
    input_cached: u64,
    output: u64,
    unit_tokens: u64,
    input_uncached_price: &UnitPrice,
    input_cached_price: &UnitPrice,
    output_price: &UnitPrice,
) -> Result<ExactCost, MoneyError> {
    if unit_tokens == 0 {
        return Err(MoneyError::InvalidPrice);
    }
    let input_cost = i128::from(input_uncached)
        .checked_mul(input_uncached_price.nanos())
        .ok_or(MoneyError::Overflow)?;
    let cached_cost = i128::from(input_cached)
        .checked_mul(input_cached_price.nanos())
        .ok_or(MoneyError::Overflow)?;
    let output_cost = i128::from(output)
        .checked_mul(output_price.nanos())
        .ok_or(MoneyError::Overflow)?;
    let numerator_nanos = input_cost
        .checked_add(cached_cost)
        .and_then(|total| total.checked_add(output_cost))
        .ok_or(MoneyError::Overflow)?;
    Ok(ExactCost {
        numerator_nanos,
        denominator: unit_tokens,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    Currency,
    InvalidAmount,
    InvalidPrice,
    Overflow,
}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Currency => "unsupported currency",
            Self::InvalidAmount => "invalid amount",
            Self::InvalidPrice => "invalid price",
            Self::Overflow => "amount overflow",
        })
    }
}

impl std::error::Error for MoneyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_serializes_only_exact_decimal_strings() {
        assert_eq!(
            NanoMoney::parse("12.340000000").unwrap().nanos(),
            12_340_000_000
        );
        assert_eq!(
            NanoMoney::parse("0.000000001").unwrap().to_string(),
            "0.000000001"
        );
        assert!(NanoMoney::parse("0.0000000001").is_err());
        assert!(NanoMoney::parse("1e3").is_err());
        assert!(NanoMoney::parse("-1").is_err());
        assert!(NanoMoney::parse("-0").is_err());
        assert!(NanoMoney::parse("+1").is_err());
        assert!(NanoMoney::parse("1_000").is_err());
        assert!(NanoMoney::parse(" 1").is_err());
        let encoded = serde_json::to_string(&NanoMoney::parse("1.25").unwrap()).unwrap();
        assert_eq!(encoded, r#""1.25""#);
        assert!(serde_json::from_str::<NanoMoney>("1.25").is_err());
        assert!(serde_json::from_str::<NanoMoney>(r#""-1""#).is_err());
    }

    #[test]
    fn arithmetic_is_checked_and_nonnegative() {
        assert_eq!(
            NanoMoney::parse("1.25")
                .unwrap()
                .checked_add(NanoMoney::parse("0.75").unwrap())
                .unwrap(),
            NanoMoney::parse("2").unwrap()
        );
        assert!(
            NanoMoney::ZERO
                .checked_sub(NanoMoney::parse("0.000000001").unwrap())
                .is_err()
        );
        assert!(
            NanoMoney(i64::MAX)
                .checked_add(NanoMoney::parse("0.000000001").unwrap())
                .is_err()
        );
    }

    #[test]
    fn reserves_ceil_but_settles_half_up_at_nano_precision() {
        let third = Decimal::from_str_exact("0.0000000001").unwrap();
        assert_eq!(NanoMoney::from_decimal_ceil(third).unwrap().nanos(), 1);
        let half = Decimal::from_str_exact("0.0000000005").unwrap();
        assert_eq!(NanoMoney::from_decimal_half_up(half).unwrap().nanos(), 1);
        let negative = Decimal::from_str_exact("-0.1").unwrap();
        assert_eq!(
            NanoMoney::from_decimal_ceil(negative),
            Err(MoneyError::InvalidAmount)
        );
    }

    #[test]
    fn computes_uncached_cached_and_output_cost_without_float_rounding() {
        let input = UnitPrice::parse("0.3").unwrap();
        let cached = UnitPrice::parse("0.006").unwrap();
        let output = UnitPrice::parse("1.2").unwrap();
        let exact = cost(
            1_000_000, 100_000, 500_000, 1_000_000, &input, &cached, &output,
        )
        .unwrap();
        assert_eq!(exact.reserve().unwrap().to_string(), "0.9006");
        assert_eq!(exact.settle().unwrap().to_string(), "0.9006");
    }

    #[test]
    fn unit_prices_require_plain_nonnegative_decimal_strings() {
        assert!(UnitPrice::parse("-0").is_err());
        assert!(UnitPrice::parse("+1").is_err());
        assert!(UnitPrice::parse("1_000").is_err());
        assert!(UnitPrice::parse("1e3").is_err());
        assert!(UnitPrice::parse("0.0000000001").is_err());
    }

    #[test]
    fn unit_prices_serialize_as_canonical_decimal_strings() {
        let price = UnitPrice::parse("12.340000000").unwrap();
        assert_eq!(price.to_string(), "12.34");
        assert_eq!(serde_json::to_string(&price).unwrap(), r#""12.34""#);
        assert_eq!(
            serde_json::from_str::<UnitPrice>(r#""12.34""#).unwrap(),
            price
        );
        assert!(serde_json::from_str::<UnitPrice>("12.34").is_err());
        assert!(serde_json::from_str::<UnitPrice>(r#""-1""#).is_err());
        assert_eq!(UnitPrice::parse("12").unwrap().to_string(), "12");
    }

    #[test]
    fn cost_rounding_uses_exact_integer_remainder() {
        let tiny = UnitPrice::parse("0.000000001").unwrap();
        let large = UnitPrice::parse("9000000000").unwrap();
        let tokens = 9_007_199_254_740_991;
        let exact = cost(
            1,
            0,
            tokens,
            tokens,
            &tiny,
            &UnitPrice::parse("0").unwrap(),
            &large,
        )
        .unwrap();
        assert_eq!(exact.reserve().unwrap().nanos(), 9_000_000_000_000_000_001);
        assert_eq!(exact.settle().unwrap().nanos(), 9_000_000_000_000_000_000);
    }

    #[test]
    fn exact_cost_settlement_rounds_below_at_and_above_half_up() {
        for (numerator_nanos, denominator, expected_nanos) in [(1, 3, 0), (1, 2, 1), (2, 3, 1)] {
            let exact = ExactCost {
                numerator_nanos,
                denominator,
            };
            assert_eq!(exact.settle().unwrap().nanos(), expected_nanos);
        }
    }

    #[test]
    fn currency_codes_are_stable_and_reject_other_currencies() {
        assert_eq!("CNY".parse::<Currency>().unwrap(), Currency::Cny);
        assert_eq!(serde_json::to_string(&Currency::Usd).unwrap(), r#""USD""#);
        assert!("EUR".parse::<Currency>().is_err());
        assert_eq!(Currency::Cny.code(), "CNY");
        assert_eq!(Currency::Cny.to_string(), "CNY");
        assert_eq!(Currency::Usd.to_string(), "USD");
        assert_eq!(
            serde_json::from_str::<Currency>(r#""CNY""#).unwrap(),
            Currency::Cny
        );
        assert!(serde_json::from_str::<Currency>(r#""EUR""#).is_err());
        assert!(serde_json::from_str::<Currency>("1").is_err());
    }

    #[test]
    fn cost_rejects_zero_unit_and_checked_arithmetic_overflow() {
        let price = UnitPrice::parse("10000000000000000000").unwrap();
        let zero = UnitPrice::parse("0").unwrap();
        assert_eq!(
            cost(0, 0, 0, 0, &zero, &zero, &zero),
            Err(MoneyError::InvalidPrice)
        );

        assert_eq!(
            cost(u64::MAX, 0, 0, 1, &price, &zero, &zero),
            Err(MoneyError::Overflow)
        );
        assert_eq!(
            cost(9_000_000_000, 9_000_000_000, 0, 1, &price, &price, &zero),
            Err(MoneyError::Overflow)
        );
    }

    #[test]
    fn money_errors_have_stable_display_messages() {
        assert_eq!(MoneyError::Currency.to_string(), "unsupported currency");
        assert_eq!(MoneyError::InvalidAmount.to_string(), "invalid amount");
        assert_eq!(MoneyError::InvalidPrice.to_string(), "invalid price");
        assert_eq!(MoneyError::Overflow.to_string(), "amount overflow");
    }

    #[test]
    fn persisted_nano_amounts_reject_negative_values() {
        assert_eq!(NanoMoney::from_nanos(-1), Err(MoneyError::InvalidAmount));
        assert_eq!(NanoMoney::from_nanos(0).unwrap(), NanoMoney::ZERO);
    }
}
