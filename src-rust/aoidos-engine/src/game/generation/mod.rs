//! 正式回合的冻结生成适配；同一轮的提议与公开正文复用 Provider、采样和预算。

use super::domain::{FrozenRound, Generation};
use crate::{
    fault::Fault,
    record::{
        calibration::Calibration,
        format,
        projection::{Budget, Shape},
    },
    request::PreparedGeneration,
};
use aoidos_llm::{
    config::{FrozenProfile, LlmProfile, ProfileMode},
    guard::GuardSpec,
    provider::{Provider, ProviderInput, ProviderRequest, RequestMode},
    proxy::{ProxyAuth, SystemProxySnapshot},
    schedule::{BudgetPort, GenerationRequest, RunPolicy},
};
use std::sync::{Arc, Mutex};

pub struct ProfileGeneration {
    provider: Arc<dyn Provider>,
    profile: LlmProfile,
    shape: Shape,
    projection: Budget,
    budget: Arc<dyn BudgetPort>,
    diagnostics: Arc<Mutex<Calibration>>,
}
impl ProfileGeneration {
    /// 凭据和代理已冻结；构造只校验能力，不启动网络或占用回合门禁。
    /// # Errors
    /// 缺凭据、未知供应商、非法配置 / 模型窗口或传输构造失败拒绝接纳。
    pub fn freeze(
        frozen: FrozenProfile,
        snapshot: &SystemProxySnapshot,
        auth: Option<&ProxyAuth>,
        dice_mode: crate::record::facts::DiceMode,
        budget: Arc<dyn BudgetPort>,
        diagnostics: Arc<Mutex<Calibration>>,
    ) -> Result<FrozenRound, Fault> {
        let profile = frozen.profile.clone();
        let provider = aoidos_llm::providers::build_provider(frozen, snapshot, auth)
            .map_err(Fault::provider_build)?;
        Self::with_provider(profile, provider, dice_mode, budget, diagnostics)
    }
    /// 可信 Rust 装配可注入真实 adapter 的本地传输；端点和 Provider 不接受 IPC 参数。
    /// # Errors
    /// 配置结构、调用形态、窗口、采样和输出额度不满足能力时拒绝。
    pub fn with_provider(
        profile: LlmProfile,
        provider: Arc<dyn Provider>,
        dice_mode: crate::record::facts::DiceMode,
        budget: Arc<dyn BudgetPort>,
        diagnostics: Arc<Mutex<Calibration>>,
    ) -> Result<FrozenRound, Fault> {
        profile.validate().map_err(invalid_profile)?;
        if profile.thinking {
            return Err(Fault::bad_request());
        }
        let mode = RequestMode::from(profile.mode);
        let caps = provider.capabilities(&profile.model, mode);
        let shape = match profile.mode {
            ProfileMode::Completion if caps.completion => Shape::Completion,
            ProfileMode::Chat if caps.chat && caps.prefix => Shape::ChatPrefix,
            ProfileMode::Chat if caps.chat => Shape::Chat,
            _ => return Err(Fault::bad_request()),
        };
        if !profile.sampling.temperature.is_finite()
            || profile.sampling.temperature < caps.temperature_min
            || profile.sampling.temperature > caps.temperature_max
        {
            return Err(Fault::bad_request());
        }
        let projection =
            Budget::for_model(&caps, profile.sampling.max_tokens).map_err(projection_error)?;
        let revision = format::hash(
            aoidos_json::canonical_string(&profile)
                .map_err(profile_encoding)?
                .as_bytes(),
        );
        Ok(FrozenRound {
            profile_id: profile.profile_id.clone(),
            profile_revision: revision,
            dice_mode,
            generation: Arc::new(Self {
                provider,
                profile,
                shape,
                projection,
                budget,
                diagnostics,
            }),
        })
    }
}
impl Generation for ProfileGeneration {
    fn shape(&self) -> Shape {
        self.shape
    }
    fn projection_budget(&self) -> Budget {
        self.projection.clone()
    }
    fn prepare(
        &self,
        input: ProviderInput,
        guard: GuardSpec,
        automatic: bool,
    ) -> Result<PreparedGeneration, Fault> {
        if input.mode() != RequestMode::from(self.profile.mode) {
            return Err(Fault::bad_request());
        }
        let caps = self
            .provider
            .capabilities(&self.profile.model, input.mode());
        PreparedGeneration::new(
            self.provider.clone(),
            GenerationRequest {
                provider_request: ProviderRequest {
                    model: self.profile.model.clone(),
                    input,
                    sampling: self.profile.sampling,
                    stops: vec![],
                },
                guard,
            },
            RunPolicy::default_for(self.profile.sampling.temperature, &caps),
            self.budget.clone(),
        )
        .map(|request| {
            request.with_calibration(
                &self.profile.provider_id,
                &self.projection.estimator,
                self.diagnostics.clone(),
                automatic,
            )
        })
    }
}
fn invalid_profile(_: aoidos_llm::config::ProfileError) -> Fault {
    Fault::bad_request()
}
fn projection_error(error: crate::record::projection::ProjectionError) -> Fault {
    error.fault()
}
fn profile_encoding(_: aoidos_json::Error) -> Fault {
    Fault::bad_request()
}

#[cfg(test)]
mod tests;
