//! 接纳前校验与冻结；校验失败不能占用前台 lease。

use crate::fault::Fault;
use aoidos_llm::provider::{Provider, ProviderInput, RequestMode};
use aoidos_llm::schedule::{BudgetPort, GenerationRequest, RunPolicy, validate_ladder};
use std::sync::Arc;

/// 输入容量限制属于本地调试与运行资源边界，不作为模型上下文窗口估算。
pub const MAX_INPUT_BYTES: usize = 256 * 1024;

/// 已校验的不可变运行参数；构造成功后才能创建回合 / 分配 UUID。
pub struct PreparedGeneration {
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) request: GenerationRequest,
    pub(crate) policy: RunPolicy,
    pub(crate) budget: Arc<dyn BudgetPort>,
    calibration_base: Option<Arc<dyn BudgetPort>>,
    pub(crate) private_limit: usize,
}

impl PreparedGeneration {
    /// 按内部目标收紧收集器容量；提议使用 8 KiB，正文 / recap 保持既有上限。
    /// # Errors
    /// 空上限或超出全局正文硬上限拒绝。
    pub fn with_private_limit(mut self, bytes: usize) -> Result<Self, Fault> {
        if bytes == 0 || bytes > crate::turn::MAX_TEXT_BYTES {
            return Err(Fault::bad_request());
        }
        self.private_limit = bytes;
        Ok(self)
    }
    /// 绑定冻结输入的估计；每次物理请求保留原预算预留 / 结算行为。
    /// 自动请求在连续低估后暂停，手动请求仍由原预算裁决。
    pub fn with_calibration(
        mut self,
        provider: &str,
        revision: &crate::record::estimator::EstimatorRevision,
        diagnostics: Arc<std::sync::Mutex<crate::record::calibration::Calibration>>,
        automatic: bool,
    ) -> Self {
        use crate::record::{
            calibration::{Bucket, CalibrationBudget},
            estimator::estimate_input,
        };
        let input = &self.request.provider_request.input;
        let shape = match input {
            ProviderInput::Completion(_) => "completion",
            ProviderInput::Chat(input) if input.assistant_prefix.is_some() => "chatPrefix",
            ProviderInput::Chat(_) => "chat",
        };
        self.budget = Arc::new(CalibrationBudget {
            inner: self
                .calibration_base
                .get_or_insert_with(|| self.budget.clone())
                .clone(),
            diagnostics,
            bucket: Bucket {
                provider: provider.into(),
                model: self.request.provider_request.model.clone(),
                shape: shape.into(),
                estimator_version: revision.version(),
            },
            estimate: revision.scale(estimate_input(input)),
            automatic,
        });
        self
    }
    /// 从冻结 profile / 凭据 / 代理组装请求；预算实现由调用方注入。
    ///
    /// # Errors
    /// 形态、能力、配置、凭据或客户端构建失败返回稳定脱敏错误，不占门禁。
    pub fn from_frozen(
        frozen: aoidos_llm::config::FrozenProfile,
        snapshot: &aoidos_llm::proxy::SystemProxySnapshot,
        auth: Option<&aoidos_llm::proxy::ProxyAuth>,
        input: ProviderInput,
        guard: aoidos_llm::guard::GuardSpec,
        budget: Arc<dyn BudgetPort>,
    ) -> Result<Self, Fault> {
        let profile = frozen.profile.clone();
        if RequestMode::from(profile.mode) != input.mode() {
            return Err(Fault::bad_request());
        }
        let provider = aoidos_llm::providers::build_provider(frozen, snapshot, auth)
            .map_err(Fault::provider_build)?;
        let policy = RunPolicy::default_for(
            profile.sampling.temperature,
            &provider.capabilities(&profile.model, input.mode()),
        );
        Self::new(
            provider,
            GenerationRequest {
                provider_request: aoidos_llm::provider::ProviderRequest {
                    model: profile.model,
                    input,
                    sampling: profile.sampling,
                    stops: Vec::new(),
                },
                guard,
            },
            policy,
            budget,
        )
    }

    /// 本地开发夹具复用冻结与校验流程，替换 start 的传输实现而不发送收费请求。
    ///
    /// # Errors
    /// 同 [`Self::from_frozen`]。
    #[cfg(debug_assertions)]
    pub fn local_fixture_from_frozen(
        frozen: aoidos_llm::config::FrozenProfile,
        snapshot: &aoidos_llm::proxy::SystemProxySnapshot,
        auth: Option<&aoidos_llm::proxy::ProxyAuth>,
        input: ProviderInput,
    ) -> Result<Self, Fault> {
        Self::local_fixture_with_guard(
            frozen,
            snapshot,
            auth,
            input,
            aoidos_llm::guard::GuardSpec::default(),
        )
    }
    /// 同源护栏的本地夹具入口，始终替换供应商传输，不发收费请求。
    /// # Errors
    /// 与冻结请求相同的能力 / 参数校验。
    #[cfg(debug_assertions)]
    pub fn local_fixture_with_guard(
        frozen: aoidos_llm::config::FrozenProfile,
        snapshot: &aoidos_llm::proxy::SystemProxySnapshot,
        auth: Option<&aoidos_llm::proxy::ProxyAuth>,
        input: ProviderInput,
        guard: aoidos_llm::guard::GuardSpec,
    ) -> Result<Self, Fault> {
        let mut request = Self::from_frozen(
            frozen,
            snapshot,
            auth,
            input,
            guard,
            Arc::new(aoidos_llm::schedule::AllowAll),
        )?;
        request.provider = Arc::new(crate::fixture::FixtureProvider(request.provider));
        Ok(request)
    }

    /// 冻结请求并验证形态、输入、采样、stop 子集及运行策略。
    ///
    /// # Errors
    /// 无效输入或能力失配返回 `app.bad-request`，不发网络请求、不占用门禁。
    pub fn new(
        provider: Arc<dyn Provider>,
        mut request: GenerationRequest,
        policy: RunPolicy,
        budget: Arc<dyn BudgetPort>,
    ) -> Result<Self, Fault> {
        let p = &request.provider_request;
        let caps = provider.capabilities(&p.model, p.mode());
        if p.model.trim().is_empty()
            || p.model.len() > 256
            || p.model.chars().any(char::is_control)
            || !(match p.mode() {
                RequestMode::Completion => caps.completion,
                RequestMode::Chat => caps.chat,
            })
            || !p.sampling.temperature.is_finite()
            || p.sampling.temperature < caps.temperature_min
            || p.sampling.temperature > caps.temperature_max
            || p.sampling.max_tokens == 0
            || p.sampling.max_tokens > caps.max_output_tokens
            || policy.max_requests == 0
            || policy.max_requests > 3
            || policy.transport_retries > 2
            || policy.head_budget.is_zero()
            || policy.idle_budget.is_zero()
            || validate_ladder(
                &policy,
                p.sampling.temperature,
                caps.temperature_min,
                caps.temperature_max,
            )
            .is_err()
        {
            return Err(Fault::bad_request());
        }
        let bytes = match &p.input {
            ProviderInput::Completion(input) if !input.prompt.trim().is_empty() => {
                input.prompt.len()
            }
            ProviderInput::Chat(input)
                if !input.messages.is_empty() && input.messages.len() <= 128 =>
            {
                let mut bytes = 0;
                for message in &input.messages {
                    if message.content.trim().is_empty() {
                        return Err(Fault::bad_request());
                    }
                    bytes += message.content.len();
                    if bytes > MAX_INPUT_BYTES {
                        return Err(Fault::bad_request());
                    }
                }
                match &input.assistant_prefix {
                    Some(prefix) if caps.prefix && !prefix.trim().is_empty() => {
                        bytes + prefix.len()
                    }
                    Some(_) => return Err(Fault::bad_request()),
                    None => bytes,
                }
            }
            _ => return Err(Fault::bad_request()),
        };
        if bytes > MAX_INPUT_BYTES {
            return Err(Fault::bad_request());
        }
        request.provider_request.stops = request.guard.server_stops(caps.stop_limit);
        Ok(Self {
            provider,
            request,
            policy,
            budget,
            calibration_base: None,
            private_limit: crate::turn::MAX_TEXT_BYTES,
        })
    }
}
