//! 应用默认与剧本显式覆盖使用的记忆参数；最终策略在周目创建时冻结。

use super::{Algorithm, Policy};
use crate::error::{Error, Reason, Result};
use serde::{Deserialize, Deserializer, Serialize};

/// 可编辑参数不含运行期版本号；版本由配置仓储递增并写入周目快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyParameters {
    pub algorithm: Algorithm,
    pub base_strength: u64,
    pub reinforcement_gain: u64,
    pub reinforcement_cap: u64,
    pub strength_cap: u64,
    pub half_life_rounds: u64,
    pub repeat_window_rounds: u64,
    pub repeat_limit: u64,
    pub revival_cooldown_rounds: u64,
    pub active_capacity: u64,
    pub rate_table_version: u32,
    pub decay_rate: u64,
}

impl PolicyParameters {
    /// 开发初始值与 007 / 算法文档一致；这些值未经过产品效果标定。
    pub fn development_defaults() -> Self {
        Self {
            algorithm: Algorithm::Exponential,
            base_strength: 500_000,
            reinforcement_gain: 150_000,
            reinforcement_cap: 500_000,
            strength_cap: 1_000_000,
            half_life_rounds: 100,
            repeat_window_rounds: 20,
            repeat_limit: 2,
            revival_cooldown_rounds: 20,
            active_capacity: 1_000,
            rate_table_version: 1,
            decay_rate: 993_092,
        }
    }

    /// 校验参数组合，并转换为带配置版本的完整周目策略。
    /// # Errors
    /// 越界、跨字段冲突或未知策略版本均拒绝整组参数。
    pub fn into_policy(self, config_version: u64, app_config_version: u64) -> Result<Policy> {
        let policy = Policy {
            version: 1,
            algorithm_version: 1,
            config_version,
            app_config_version,
            algorithm: self.algorithm,
            base_strength: self.base_strength,
            reinforcement_gain: self.reinforcement_gain,
            reinforcement_cap: self.reinforcement_cap,
            strength_cap: self.strength_cap,
            half_life_rounds: self.half_life_rounds,
            repeat_window_rounds: self.repeat_window_rounds,
            repeat_limit: self.repeat_limit,
            revival_cooldown_rounds: self.revival_cooldown_rounds,
            active_capacity: self.active_capacity,
            rate_table_version: self.rate_table_version,
            decay_rate: self.decay_rate,
        };
        policy.validate()?;
        Ok(policy)
    }

    /// 按字段应用已登记的剧本覆盖，再由调用方校验完整组合。
    pub fn apply(&self, overrides: &PolicyOverrides) -> Self {
        Self {
            algorithm: overrides.algorithm.unwrap_or(self.algorithm),
            base_strength: overrides.base_strength.unwrap_or(self.base_strength),
            reinforcement_gain: overrides
                .reinforcement_gain
                .unwrap_or(self.reinforcement_gain),
            reinforcement_cap: overrides
                .reinforcement_cap
                .unwrap_or(self.reinforcement_cap),
            strength_cap: overrides.strength_cap.unwrap_or(self.strength_cap),
            half_life_rounds: overrides.half_life_rounds.unwrap_or(self.half_life_rounds),
            repeat_window_rounds: overrides
                .repeat_window_rounds
                .unwrap_or(self.repeat_window_rounds),
            repeat_limit: overrides.repeat_limit.unwrap_or(self.repeat_limit),
            revival_cooldown_rounds: overrides
                .revival_cooldown_rounds
                .unwrap_or(self.revival_cooldown_rounds),
            active_capacity: overrides.active_capacity.unwrap_or(self.active_capacity),
            rate_table_version: overrides
                .rate_table_version
                .unwrap_or(self.rate_table_version),
            decay_rate: overrides.decay_rate.unwrap_or(self.decay_rate),
        }
    }
}

/// 剧本只保存显式覆盖；字段缺失表示继承，显式 null 属于非法配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyOverrides {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub algorithm: Option<Algorithm>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub base_strength: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub reinforcement_gain: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub reinforcement_cap: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub strength_cap: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub half_life_rounds: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub repeat_window_rounds: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub repeat_limit: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub revival_cooldown_rounds: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub active_capacity: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub rate_table_version: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub decay_rate: Option<u64>,
}

/// 返回应用配置的已确认版本和完整参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppConfig {
    pub config_version: u64,
    pub algorithm_version: u32,
    pub parameters: PolicyParameters,
    pub default_parameters: PolicyParameters,
}

/// 返回剧本显式覆盖、对应应用基值及合并后的完整预览。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptConfig {
    pub script_id: String,
    pub config_version: u64,
    pub app_config_version: u64,
    pub algorithm_version: u32,
    pub overrides: PolicyOverrides,
    pub inherited: PolicyParameters,
    pub effective: PolicyParameters,
}

/// CAS 保存成功后返回新的已确认版本，不表示当前周目的策略被重算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigSaved {
    pub config_version: u64,
}

impl PolicyOverrides {
    /// 局部值同样受参数上限约束；跨字段约束在与应用默认合并后检查。
    /// # Errors
    /// 任一显式值越界或包含未支持版本时拒绝覆盖。
    pub fn validate(&self) -> Result<()> {
        if self
            .base_strength
            .is_some_and(|value| !(50_000..=1_000_000).contains(&value))
            || self.reinforcement_gain.is_some_and(|value| value > 500_000)
            || self.reinforcement_cap.is_some_and(|value| value > 950_000)
            || self
                .strength_cap
                .is_some_and(|value| !(100_000..=1_000_000).contains(&value))
            || self
                .half_life_rounds
                .is_some_and(|value| !(10..=10_000).contains(&value))
            || self
                .repeat_window_rounds
                .is_some_and(|value| !(1..=10_000).contains(&value))
            || self
                .repeat_limit
                .is_some_and(|value| !(1..=10).contains(&value))
            || self
                .revival_cooldown_rounds
                .is_some_and(|value| !(1..=10_000).contains(&value))
            || self
                .active_capacity
                .is_some_and(|value| !(100..=10_000).contains(&value))
            || self.rate_table_version.is_some_and(|value| value != 1)
            || self
                .decay_rate
                .is_some_and(|value| !(1..=1_000_000).contains(&value))
        {
            return Err(Error::Rejected(Reason::InvalidSchema));
        }
        Ok(())
    }
}

/// 缺省字段可省略，显式 null 不等于 unset。
fn deserialize_present<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
