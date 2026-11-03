//! W1.4 局部影子指标；闭集预置零值，不把任意 world 或远端身份带入标签。

use std::sync::Arc;

use auction_application::{
    SentBaselineBranch, SentBaselineEvent, SentBaselineObserver, SentBaselinePhase,
    SentBaselineReason,
};
use prometheus::{IntCounterVec, Opts, Registry};

use super::types::MetricsError;

struct SentBaselineMetrics {
    evaluations: IntCounterVec,
    mismatch: IntCounterVec,
    uncomparable: IntCounterVec,
    fallback: IntCounterVec,
}

impl SentBaselineObserver for SentBaselineMetrics {
    fn observe(&self, world: &str, event: SentBaselineEvent) {
        let world = match world {
            "main" => "main",
            "isekai" => "isekai",
            _ => return,
        };
        match event {
            SentBaselineEvent::Evaluation { branch, mid_sets_equal, candidate_sets_equal } => {
                self.evaluations.with_label_values(&[world, branch.label()]).inc();
                if !mid_sets_equal {
                    self.mismatch.with_label_values(&[world, "mid_set"]).inc();
                }
                if !candidate_sets_equal {
                    self.mismatch.with_label_values(&[world, "candidate_set"]).inc();
                }
            }
            SentBaselineEvent::Uncomparable(reason) => {
                self.uncomparable.with_label_values(&[world, reason.label()]).inc()
            }
            SentBaselineEvent::Fallback { phase, reason } => {
                self.fallback.with_label_values(&[world, phase.label(), reason.label()]).inc()
            }
        }
    }
}

pub(super) fn build(registry: &Registry) -> Result<Arc<dyn SentBaselineObserver>, MetricsError> {
    let metrics = SentBaselineMetrics {
        evaluations: counter(
            "shadow_evaluations",
            "Completed comparable sent baseline shadow evaluations.",
            &["world", "branch"],
        )?,
        mismatch: counter(
            "shadow_mismatch",
            "Unexpected sent baseline shadow set mismatches.",
            &["world", "class"],
        )?,
        uncomparable: counter(
            "shadow_uncomparable",
            "Sent baseline shadow observations that cannot establish equivalence.",
            &["world", "reason"],
        )?,
        fallback: counter(
            "fallback",
            "Attempts to enumerate full Sent using the original pre-send context.",
            &["world", "phase", "reason"],
        )?,
    };
    for world in ["main", "isekai"] {
        for branch in SentBaselineBranch::ALL {
            metrics.evaluations.with_label_values(&[world, branch.label()]);
        }
        for class in ["mid_set", "candidate_set"] {
            metrics.mismatch.with_label_values(&[world, class]);
        }
        for reason in SentBaselineReason::ALL {
            metrics.uncomparable.with_label_values(&[world, reason.label()]);
            for phase in SentBaselinePhase::ALL {
                metrics.fallback.with_label_values(&[world, phase.label(), reason.label()]);
            }
        }
    }
    for counter in
        [&metrics.evaluations, &metrics.mismatch, &metrics.uncomparable, &metrics.fallback]
    {
        registry.register(Box::new(counter.clone())).map_err(MetricsError::Registry)?;
    }
    Ok(Arc::new(metrics))
}

fn counter(suffix: &str, help: &str, labels: &[&str]) -> Result<IntCounterVec, MetricsError> {
    IntCounterVec::new(
        Opts::new(format!("auction_mooglemail_sent_baseline_{suffix}_total"), help),
        labels,
    )
    .map_err(MetricsError::Registry)
}
