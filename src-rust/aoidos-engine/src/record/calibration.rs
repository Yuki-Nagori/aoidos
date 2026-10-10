//! 估算诊断只保存聚合计数，不保存 prompt；系数更新只影响下一次投影。

use aoidos_llm::{
    provider::Usage,
    schedule::{AttemptIdentity, AttemptOutcome, BudgetDenial, BudgetPort},
};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bucket {
    pub provider: String,
    pub model: String,
    pub shape: String,
    pub estimator_version: u32,
}
#[derive(Debug, Clone)]
pub struct Sample {
    pub estimate: u32,
    pub actual_input: Option<u64>,
    pub actual_output: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct Report {
    pub samples: usize,
    pub p95: Option<f64>,
    pub maximum_ratio: Option<f64>,
    pub paused: bool,
}
struct Series {
    samples: VecDeque<f64>,
    underestimated: u8,
    paused: bool,
}
#[derive(Default)]
pub struct Calibration {
    buckets: BTreeMap<Bucket, Series>,
}
impl Calibration {
    /// 只有对应冻结 prompt 的完整 usage 才参与校正；缺 usage 不当零。
    /// 每桶最近 256 个完整样本，总桶数最多 128，拒绝无界诊断缓存。
    pub fn observe(&mut self, bucket: Bucket, sample: Sample) -> bool {
        let (Some(input), Some(_output)) = (sample.actual_input, sample.actual_output) else {
            return false;
        };
        if sample.estimate == 0
            || bucket.estimator_version == 0
            || bucket.provider.len() > 128
            || bucket.model.len() > 256
            || bucket.shape.len() > 32
        {
            return false;
        }
        if !self.buckets.contains_key(&bucket) && self.buckets.len() == 128 {
            return false;
        }
        let series = self.buckets.entry(bucket).or_insert_with(|| Series {
            samples: VecDeque::new(),
            underestimated: 0,
            paused: false,
        });
        let ratio = input as f64 / f64::from(sample.estimate);
        series.underestimated = if ratio > 1.2 {
            series.underestimated.saturating_add(1)
        } else {
            0
        };
        if series.underestimated >= 3 {
            series.paused = true;
        }
        if series.samples.len() == 256 {
            series.samples.pop_front();
        }
        series.samples.push_back(ratio);
        true
    }
    /// P95 至少需要 32 个完整样本；告警不悄悄修改当前回合系数。
    pub fn report(&self, bucket: &Bucket) -> Report {
        let Some(series) = self.buckets.get(bucket) else {
            return Report {
                samples: 0,
                p95: None,
                maximum_ratio: None,
                paused: false,
            };
        };
        let mut ratios = series.samples.iter().copied().collect::<Vec<_>>();
        ratios.sort_by(f64::total_cmp);
        Report {
            samples: ratios.len(),
            p95: (ratios.len() >= 32).then(|| ratios[(ratios.len() * 95).div_ceil(100) - 1]),
            maximum_ratio: ratios.last().copied(),
            paused: series.paused,
        }
    }
    /// 用户完成重标定后显式解除自动推进 / 压缩暂停。
    pub fn resume(&mut self, bucket: &Bucket) {
        if let Some(series) = self.buckets.get_mut(bucket) {
            series.paused = false;
            series.underestimated = 0;
        }
    }
}

/// 每个冻结请求拥有同一估计，重试与温度阶梯分别按物理尝试结算。
pub struct CalibrationBudget {
    pub(crate) inner: Arc<dyn BudgetPort>,
    pub(crate) diagnostics: Arc<Mutex<Calibration>>,
    pub(crate) bucket: Bucket,
    pub(crate) estimate: u32,
    pub(crate) automatic: bool,
}
impl BudgetPort for CalibrationBudget {
    fn reserve(&self, attempt: &AttemptIdentity) -> Result<(), BudgetDenial> {
        if self.automatic
            && self
                .diagnostics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .report(&self.bucket)
                .paused
        {
            return Err(BudgetDenial {
                reason: "token-estimation-needs-calibration".into(),
            });
        }
        self.inner.reserve(attempt)
    }
    fn settle(&self, attempt: &AttemptIdentity, outcome: &AttemptOutcome) {
        self.inner.settle(attempt, outcome);
        if let AttemptOutcome::Settled {
            usage:
                Some(Usage {
                    prompt_tokens,
                    completion_tokens,
                    ..
                }),
        } = outcome
        {
            self.diagnostics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .observe(
                    self.bucket.clone(),
                    Sample {
                        estimate: self.estimate,
                        actual_input: Some(*prompt_tokens),
                        actual_output: Some(*completion_tokens),
                    },
                );
        }
    }
}

/// 压缩只是计划，执行器仍需取得 020 lease 和授权预算。
#[derive(Debug, Default)]
pub struct CompressionPolicy {
    pub enabled: bool,
    failures: u8,
    pub last_attempt_seconds: Option<u64>,
}
impl CompressionPolicy {
    pub fn eligible(
        &self,
        now_seconds: u64,
        idle_seconds: u64,
        blocks: usize,
        tokens: u32,
        compression_needed: bool,
    ) -> bool {
        self.enabled
            && self.failures < 3
            && idle_seconds >= 60
            && self
                .last_attempt_seconds
                .is_none_or(|last| now_seconds.saturating_sub(last) >= 300)
            && blocks >= 16
            && tokens >= 4096
            && compression_needed
    }
    pub fn started(&mut self, now_seconds: u64) {
        self.last_attempt_seconds = Some(now_seconds);
    }
    pub fn finished(&mut self, success: bool) {
        self.failures = if success {
            0
        } else {
            self.failures.saturating_add(1)
        };
    }
    pub fn resume(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_usage_buckets_p95_and_warning_do_not_change_estimates() {
        let bucket = Bucket {
            provider: "local-fixture".into(),
            model: "synthetic".into(),
            shape: "chat".into(),
            estimator_version: 1,
        };
        let mut calibration = Calibration::default();
        assert!(!calibration.observe(
            bucket.clone(),
            Sample {
                estimate: 100,
                actual_input: None,
                actual_output: Some(1)
            }
        ));
        for _ in 0..32 {
            assert!(calibration.observe(
                bucket.clone(),
                Sample {
                    estimate: 100,
                    actual_input: Some(130),
                    actual_output: Some(1)
                }
            ));
        }
        let report = calibration.report(&bucket);
        assert_eq!(report.p95, Some(1.3));
        assert_eq!(report.maximum_ratio, Some(1.3));
        assert!(report.paused);
        calibration.resume(&bucket);
        assert!(!calibration.report(&bucket).paused);
        let mut policy = CompressionPolicy::default();
        assert!(!policy.eligible(1000, 60, 16, 4096, true));
        policy.enabled = true;
        assert!(policy.eligible(1000, 60, 16, 4096, true));
        policy.started(1000);
        assert!(!policy.eligible(1001, 60, 16, 4096, true));
        for _ in 0..3 {
            policy.finished(false);
        }
        assert!(!policy.eligible(1300, 60, 16, 4096, true));
        policy.resume();
        assert!(policy.eligible(1300, 60, 16, 4096, true));
        policy.finished(true);
    }
    #[test]
    fn diagnostics_have_bounded_buckets_samples_and_no_missing_usage_zero() {
        let mut calibration = Calibration::default();
        let bucket = Bucket {
            provider: "local".into(),
            model: "m".into(),
            shape: "chat".into(),
            estimator_version: 2,
        };
        assert_eq!(calibration.report(&bucket).samples, 0);
        calibration.resume(&bucket);
        assert!(!calibration.observe(
            bucket.clone(),
            Sample {
                estimate: 0,
                actual_input: Some(0),
                actual_output: Some(0)
            }
        ));
        for n in 0..300 {
            assert!(calibration.observe(
                bucket.clone(),
                Sample {
                    estimate: 100,
                    actual_input: Some(if n == 299 { 100 } else { 130 }),
                    actual_output: Some(0)
                }
            ));
        }
        assert_eq!(calibration.report(&bucket).samples, 256);
        for n in 1..128 {
            let mut other = bucket.clone();
            other.model = format!("m{n}");
            assert!(calibration.observe(
                other,
                Sample {
                    estimate: 1,
                    actual_input: Some(0),
                    actual_output: Some(0)
                }
            ));
        }
        let mut other = bucket.clone();
        other.model = "overflow".into();
        assert!(!calibration.observe(
            other,
            Sample {
                estimate: 1,
                actual_input: Some(1),
                actual_output: Some(1)
            }
        ));
        let mut invalid = bucket;
        invalid.estimator_version = 0;
        assert!(!calibration.observe(
            invalid,
            Sample {
                estimate: 1,
                actual_input: Some(1),
                actual_output: Some(1)
            }
        ));
    }
    #[tokio::test]
    async fn physical_attempt_usage_is_observed_and_original_budget_is_preserved() {
        use crate::{
            record::estimator::EstimatorRevision,
            test_support::{Events, generation},
            turn::Coordinator,
        };
        use aoidos_llm::provider::{ProviderDelta, ProviderFinish};
        let diagnostics = Arc::new(Mutex::new(Calibration::default()));
        let coordinator = Coordinator::new(Arc::new(Events::default()), 16).unwrap();
        let bucket = Bucket {
            provider: "local-fixture".into(),
            model: "m".into(),
            shape: "completion".into(),
            estimator_version: 2,
        };
        for usage in [
            None,
            Some(Usage::default()),
            Some(Usage {
                prompt_tokens: 100,
                completion_tokens: 2,
                ..Usage::default()
            }),
            Some(Usage {
                prompt_tokens: 100,
                completion_tokens: 2,
                ..Usage::default()
            }),
            Some(Usage {
                prompt_tokens: 100,
                completion_tokens: 2,
                ..Usage::default()
            }),
        ] {
            let mut deltas = vec![Ok(ProviderDelta::Text("合成摘要".into()))];
            if let Some(usage) = usage {
                deltas.push(Ok(ProviderDelta::Usage(usage)));
            }
            deltas.push(Ok(ProviderDelta::Finish(ProviderFinish::Stop)));
            let request = generation(deltas, false).0.with_calibration(
                "local-fixture",
                &EstimatorRevision::ConservativeV2,
                diagnostics.clone(),
                true,
            );
            let lease = coordinator.acquire().unwrap();
            coordinator.run_private(&lease, request).await.unwrap();
        }
        assert_eq!(diagnostics.lock().unwrap().report(&bucket).samples, 4);
        assert!(diagnostics.lock().unwrap().report(&bucket).paused);
        let (request, calls) = generation(
            vec![
                Ok(ProviderDelta::Text("不应发出".into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ],
            false,
        );
        let request = request.with_calibration(
            "local-fixture",
            &EstimatorRevision::ConservativeV2,
            diagnostics.clone(),
            true,
        );
        let lease = coordinator.acquire().unwrap();
        assert_eq!(
            coordinator
                .run_private(&lease, request)
                .await
                .err()
                .unwrap()
                .code,
            "engine.invalid-phase"
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        diagnostics.lock().unwrap().resume(&bucket);
        drop(lease);
        let request = crate::test_support::generation(
            vec![
                Ok(ProviderDelta::Text("恰一次".into())),
                Ok(ProviderDelta::Usage(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 2,
                    ..Usage::default()
                })),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ],
            false,
        )
        .0
        .with_calibration(
            "local-fixture",
            &EstimatorRevision::ConservativeV2,
            diagnostics.clone(),
            false,
        )
        .with_calibration(
            "local-fixture",
            &EstimatorRevision::ConservativeV2,
            diagnostics.clone(),
            false,
        );
        let lease = coordinator.acquire().unwrap();
        coordinator.run_private(&lease, request).await.unwrap();
        assert_eq!(diagnostics.lock().unwrap().report(&bucket).samples, 5);
    }
}
