//! 压缩内部入口复用全应用 lease；默认关闭，失败或取消不提交摘要。

use super::{
    calibration::CompressionPolicy, format::RecapOrigin, recap::RecapCandidate, session::Session,
};
use crate::{fault::Fault, ports::Outcome, request::PreparedGeneration, turn::Coordinator};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub struct Trigger {
    pub authorized: bool,
    pub now_seconds: u64,
    pub idle_seconds: u64,
    pub blocks: usize,
    pub tokens: u32,
    pub compression_needed: bool,
}
/// 工厂在门槛与 lease 确认后才执行；统一动态边界避免复制整段异步执行代码。
pub type RequestFactory<'a> =
    dyn FnOnce(&RecapCandidate) -> Result<PreparedGeneration, Fault> + Send + 'a;
/// 产品设置接入前保持默认关闭；不隐式创建后台定时器或预算。
pub struct Compression {
    policy: Arc<Mutex<CompressionPolicy>>,
    session: Arc<Mutex<Session>>,
    coordinator: Coordinator,
    cancel: Arc<Mutex<Option<CancellationToken>>>,
}
impl Compression {
    pub fn new(session: Arc<Mutex<Session>>, coordinator: Coordinator) -> Self {
        Self {
            policy: Arc::new(Mutex::new(CompressionPolicy::default())),
            session,
            coordinator,
            cancel: Arc::new(Mutex::new(None)),
        }
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .enabled = enabled;
    }
    pub fn resume(&self) {
        self.policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .resume();
    }
    /// 玩家返回时停止可选请求；执行器等待生产者退出后才释放 lease。
    pub fn cancel(&self) {
        if let Some(cancel) = self
            .cancel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            cancel.cancel();
        }
    }
    /// 只有授权与五个门槛均满足才构造请求；运行期间只持有同一个 lease。
    /// # Errors
    /// 门禁占用、生成失败、取消或候选过期不落库，下一次由显式触发重试。
    pub async fn attempt(
        &self,
        trigger: Trigger,
        candidate: RecapCandidate,
        prepare: Box<RequestFactory<'_>>,
    ) -> Result<Option<u64>, Fault> {
        let (lease, cancel) = {
            let mut active = self
                .cancel
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let policy = self
                .policy
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !trigger.authorized
                || !policy.eligible(
                    trigger.now_seconds,
                    trigger.idle_seconds,
                    trigger.blocks,
                    trigger.tokens,
                    trigger.compression_needed,
                )
            {
                return Ok(None);
            }
            let lease = self.coordinator.acquire()?;
            let cancel = lease.cancellation();
            *active = Some(cancel.clone());
            (lease, cancel)
        };
        let mut attempt = Attempt {
            policy: self.policy.clone(),
            outcome: None,
            cancel: self.cancel.clone(),
        };
        let session = self.session.clone();
        let candidate = crate::blocking::run(move || {
            session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .validate_recap_candidate(&candidate)?;
            Ok(candidate)
        })
        .await;
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let candidate = candidate?;
        self.policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .started(trigger.now_seconds);
        attempt.outcome = Some(false);
        let request = prepare(&candidate);
        if cancel.is_cancelled() {
            attempt.outcome = None;
            return Ok(None);
        }
        let run = self.coordinator.run_private(&lease, request?);
        tokio::pin!(run);
        let result = tokio::select! {result=&mut run=>result,()=cancel.cancelled()=>{lease.cancel();run.await}};
        if cancel.is_cancelled() {
            attempt.outcome = None;
            return Ok(None);
        }
        let result = result?;
        let text = completed_text(result);
        let session = self.session.clone();
        let seq = crate::blocking::run(move || commit(session, cancel, candidate, text)).await?;
        attempt.outcome = seq.map(|_| true);
        Ok(seq)
    }
}
fn completed_text(result: crate::turn::PrivateResult) -> Option<String> {
    (result.outcome == Outcome::Completed).then_some(result.text)
}

struct Attempt {
    policy: Arc<Mutex<CompressionPolicy>>,
    outcome: Option<bool>,
    cancel: Arc<Mutex<Option<CancellationToken>>>,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        self.cancel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(success) = self.outcome {
            self.policy
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .finished(success);
        }
    }
}
fn commit(
    session: Arc<Mutex<Session>>,
    cancel: CancellationToken,
    candidate: RecapCandidate,
    text: Option<String>,
) -> Result<Option<u64>, Fault> {
    let mut session = session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cancel.is_cancelled() {
        return Ok(None);
    }
    let Some(text) = text else {
        return Ok(None);
    };
    session
        .commit_recap(candidate, text, RecapOrigin::Background)
        .map(Some)
}

#[cfg(test)]
mod tests;
