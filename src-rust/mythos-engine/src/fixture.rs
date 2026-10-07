//! 开发夹具：只复用冻结供应商的能力声明，start 不调用供应商，不产生付费网络请求。

use futures::{FutureExt, StreamExt, stream};
use mythos_llm::provider::{
    Provider, ProviderCapabilities, ProviderDelta, ProviderFinish, ProviderRequest, RequestMode,
    StartFuture,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// 020 调试接口仅登记此夹具；正式 GrammarSpec 注册与持久写入由 022 接入。
pub const GUARD_SPEC_ID: &str = "debug-fixture-v1";

/// 元数据沿用实际供应商能力；传输始终是 Rust 内建的本地可重放增量。
pub struct FixtureProvider(pub Arc<dyn Provider>);
impl Provider for FixtureProvider {
    fn capabilities(&self, model: &str, mode: RequestMode) -> ProviderCapabilities {
        self.0.capabilities(model, mode)
    }
    fn start(&self, _: ProviderRequest, _: CancellationToken) -> StartFuture<'_> {
        async {
            Ok(stream::iter([
                Ok(ProviderDelta::Text("本地夹具正文。".into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ])
            .boxed())
        }
        .boxed()
    }
}
