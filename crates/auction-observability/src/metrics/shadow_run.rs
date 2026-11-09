//! 共享影子运行指标：按 owner/branch 统计可比较样本，按 owner 统计差异。
//!
//! 只接受应用层闭集（[`auction_application::shadow_run::ShadowOwner`]），所有 owner 与分支在构建时
//! 预置零值；未登记的分支不会创建新 series。不可比较样本不计入 evaluations，避免把
//! 没有证据的分支误读成通过。差异日志由应用层 `record_*` helper 输出。

use std::sync::Arc;

use auction_application::shadow_run::{ShadowOutcome, ShadowOwner, ShadowRecorder, ShadowSample};
use prometheus::{IntCounterVec, Opts, Registry};

use super::types::MetricsError;

struct PrometheusShadowRecorder {
    evaluations: IntCounterVec,
    mismatches: IntCounterVec,
}

impl ShadowRecorder for PrometheusShadowRecorder {
    fn record(&self, sample: ShadowSample) {
        // 应用层 helper 已校验；这里再拒绝一次，避免直接持有记录器的调用点扩张 label 基数。
        if !sample.owner.branches().contains(&sample.branch) {
            return;
        }
        if sample.outcome != ShadowOutcome::Uncomparable {
            self.evaluations.with_label_values(&[sample.owner.label(), sample.branch]).inc();
        }
        if sample.outcome == ShadowOutcome::Mismatch {
            self.mismatches.with_label_values(&[sample.owner.label()]).inc();
        }
    }
}

pub(super) fn build(registry: &Registry) -> Result<Arc<dyn ShadowRecorder>, MetricsError> {
    let evaluations = IntCounterVec::new(
        Opts::new(
            "auction_shadow_evaluations_total",
            "Comparable shadow evaluations per owner and decision branch.",
        ),
        &["owner", "branch"],
    )
    .map_err(MetricsError::Registry)?;
    let mismatches = IntCounterVec::new(
        Opts::new(
            "auction_shadow_mismatch_total",
            "Shadow decisions differing from the legacy path.",
        ),
        &["owner"],
    )
    .map_err(MetricsError::Registry)?;
    for owner in ShadowOwner::ALL {
        mismatches.with_label_values(&[owner.label()]);
        for branch in owner.branches() {
            evaluations.with_label_values(&[owner.label(), branch]);
        }
    }
    registry.register(Box::new(evaluations.clone())).map_err(MetricsError::Registry)?;
    registry.register(Box::new(mismatches.clone())).map_err(MetricsError::Registry)?;
    Ok(Arc::new(PrometheusShadowRecorder { evaluations, mismatches }))
}

/// 关闭指标或未装配 exporter 时保持同一比较与日志路径。
pub(super) fn noop() -> Arc<dyn ShadowRecorder> {
    auction_application::shadow_run::noop_shadow_recorder()
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };

    use auction_application::shadow_run::{
        ShadowOutcome, ShadowOwner, record_comparison, record_sample, record_uncomparable,
    };
    use prometheus::{Encoder, Registry, TextEncoder};

    use super::{build, noop};
    use crate::metrics::AuditFailureClass;

    fn encode(registry: &Registry) -> String {
        let mut output = Vec::new();
        TextEncoder::new().encode(&registry.gather(), &mut output).expect("encode prometheus text");
        String::from_utf8(output).expect("utf8")
    }

    fn counter_value(registry: &Registry, family: &str, labels: &[(&str, &str)]) -> f64 {
        let families = registry.gather();
        let family = families.iter().find(|entry| entry.name() == family).expect("family");
        let metric = family
            .get_metric()
            .iter()
            .find(|metric| {
                labels.iter().all(|(name, value)| {
                    metric
                        .get_label()
                        .iter()
                        .any(|label| label.name() == *name && label.value() == *value)
                })
            })
            .expect("series");
        metric.get_counter().get_value()
    }

    #[test]
    fn builds_zero_series_for_every_owner_branch() {
        let registry = Registry::new();
        build(&registry).expect("build shadow recorder");
        let text = encode(&registry);
        for owner in ShadowOwner::ALL {
            assert!(
                counter_value(
                    &registry,
                    "auction_shadow_mismatch_total",
                    &[("owner", owner.label())]
                ) == 0.0,
                "owner 差异计数应预置零值: {owner:?}"
            );
            for branch in owner.branches() {
                assert!(
                    counter_value(
                        &registry,
                        "auction_shadow_evaluations_total",
                        &[("owner", owner.label()), ("branch", branch)]
                    ) == 0.0,
                    "分支样本应预置零值: {} / {branch}",
                    owner.label()
                );
            }
        }
        assert!(text.contains("auction_shadow_evaluations_total"));
        assert!(text.contains("auction_shadow_mismatch_total"));
    }

    /// 演示 owner：审计就绪判定。分支标签来自真实 `AuditFailureClass` 闭集，
    /// 因此共享框架的分支声明与存储层分类不能漂移。
    #[test]
    fn audit_branches_match_audit_failure_classes() {
        let declared = ShadowOwner::AuditReadiness.branches();
        let actual = AuditFailureClass::ALL.iter().map(|class| class.label()).collect::<Vec<_>>();
        assert_eq!(declared, actual.as_slice());
    }

    #[test]
    fn recording_counts_comparable_samples_and_mismatches() {
        let registry = Registry::new();
        let recorder = build(&registry).expect("build shadow recorder");
        // 旧口径任一失败阻断；分类口径只有 non-transient 阻断。
        for class in AuditFailureClass::ALL {
            let legacy_blocks = true;
            let candidate_blocks = class != AuditFailureClass::Transient;
            record_comparison(
                recorder.as_ref(),
                ShadowOwner::AuditReadiness,
                class.label(),
                &legacy_blocks,
                &candidate_blocks,
            );
        }
        record_uncomparable(
            recorder.as_ref(),
            ShadowOwner::AuditReadiness,
            "unknown",
            "query_failed",
        );
        for class in AuditFailureClass::ALL {
            assert_eq!(
                counter_value(
                    &registry,
                    "auction_shadow_evaluations_total",
                    &[("owner", "audit_readiness"), ("branch", class.label())]
                ),
                1.0,
                "每个分支恰好一个可比较样本: {}",
                class.label()
            );
        }
        // 不可比较样本不增加 evaluations。
        assert_eq!(
            counter_value(
                &registry,
                "auction_shadow_evaluations_total",
                &[("owner", "audit_readiness"), ("branch", "unknown")]
            ),
            1.0
        );
        assert_eq!(
            counter_value(
                &registry,
                "auction_shadow_mismatch_total",
                &[("owner", "audit_readiness")]
            ),
            1.0,
            "只有 transient 分支是预期差异"
        );
    }

    #[test]
    fn undeclared_branch_does_not_create_series() {
        let registry = Registry::new();
        let recorder = build(&registry).expect("build shadow recorder");
        let before = encode(&registry);
        record_sample(
            recorder.as_ref(),
            ShadowOwner::AuditReadiness,
            "not_declared",
            ShadowOutcome::Mismatch,
        );
        assert_eq!(encode(&registry), before, "未登记分支不能扩张 series");
    }

    #[test]
    fn process_metrics_exposes_shadow_recorder() {
        let metrics = crate::metrics::ProcessMetrics::new(true, "auction-test", "worker", "0.0.0")
            .expect("build process metrics");
        let initial =
            String::from_utf8(metrics.encode().expect("encode metrics")).expect("utf8 metrics");
        assert!(
            initial.lines().any(|line| line.starts_with("auction_shadow_evaluations_total{")
                && line.contains("owner=\"audit_readiness\"")
                && line.contains("branch=\"transient\"")
                && line.trim_end().ends_with(" 0")),
            "共享框架应在所有角色预置样本零值: {initial}"
        );
        record_comparison(
            metrics.shadow().as_ref(),
            ShadowOwner::AuditReadiness,
            "permanent",
            &true,
            &false,
        );
        let updated =
            String::from_utf8(metrics.encode().expect("encode metrics")).expect("utf8 metrics");
        assert!(
            updated.lines().any(|line| line.starts_with("auction_shadow_mismatch_total{")
                && line.contains("owner=\"audit_readiness\"")
                && line.trim_end().ends_with(" 1")),
            "装配根注入的记录器应累加差异: {updated}"
        );
    }

    #[test]
    fn noop_recorder_is_inert() {
        let recorder = noop();
        record_sample(
            recorder.as_ref(),
            ShadowOwner::AuditReadiness,
            "transient",
            ShadowOutcome::Match,
        );
        record_uncomparable(recorder.as_ref(), ShadowOwner::AuditReadiness, "transient", "timeout");
    }

    /// 差异日志必须结构化且不依赖具体行格式。
    #[test]
    fn mismatch_and_uncomparable_emit_structured_logs() {
        #[derive(Clone, Default)]
        struct SharedWriter(Arc<Mutex<Vec<u8>>>);

        impl Write for SharedWriter {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("writer").extend_from_slice(buffer);
                Ok(buffer.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for SharedWriter {
            type Writer = Self;

            fn make_writer(&'writer self) -> Self::Writer {
                self.clone()
            }
        }

        let writer = SharedWriter::default();
        let subscriber = tracing_subscriber::fmt().json().with_writer(writer.clone()).finish();
        let registry = Registry::new();
        let recorder = build(&registry).expect("build shadow recorder");
        tracing::subscriber::with_default(subscriber, || {
            record_comparison(
                recorder.as_ref(),
                ShadowOwner::AuditReadiness,
                "permanent",
                &true,
                &false,
            );
            record_uncomparable(
                recorder.as_ref(),
                ShadowOwner::AuditReadiness,
                "unknown",
                "query_failed",
            );
        });
        let output = String::from_utf8(writer.0.lock().expect("writer").clone()).expect("utf8");
        assert!(output.contains("\"target\":\"auction::shadow\""), "target 缺失: {output}");
        assert!(output.contains("\"owner\":\"audit_readiness\""), "owner 缺失: {output}");
        assert!(output.contains("\"branch\":\"permanent\""), "branch 缺失: {output}");
        assert!(output.contains("\"outcome\":\"mismatch\""), "mismatch 缺失: {output}");
        assert!(output.contains("WARN"), "差异必须为 WARN: {output}");
    }
}
