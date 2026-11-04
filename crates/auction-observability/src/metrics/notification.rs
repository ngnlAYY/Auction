//! W14.8 通知生命周期、失败、送达延迟与邮资估算的 Prometheus 适配器。
//!
//! 所有 label 都来自 `auction_application` 的闭集枚举；构建时预置全部零值序列，
//! 因此 scrape 在尚未产生任何通知时也能看到稳定族与标签集合。

use std::sync::Arc;

use auction_application::{
    NotificationFailure, NotificationKind, NotificationRecorder, NotificationStage,
};
use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry};

use super::types::MetricsError;

/// 送达延迟：从 outbox 创建到终态落库，覆盖分钟级排队与小时级维护恢复。
const LATENCY_BUCKETS: &[f64] =
    &[1.0, 5.0, 15.0, 60.0, 120.0, 300.0, 900.0, 1800.0, 3600.0, 7200.0, 21600.0, 86400.0];

struct PrometheusNotificationRecorder {
    /// `[kind][stage]`，按闭集顺序缓存句柄，逐笔记录不需要 label 哈希或 Arc 克隆。
    stages: [[prometheus::IntCounter; NotificationStage::ALL.len()]; NotificationKind::ALL.len()],
    failures: IntCounterVec,
    latency: HistogramVec,
    postage_estimate: IntCounterVec,
}

impl PrometheusNotificationRecorder {
    fn build(registry: &Registry) -> Result<Arc<dyn NotificationRecorder>, MetricsError> {
        let stage_family = IntCounterVec::new(
            Opts::new(
                "auction_notification_events_total",
                "Outbound notification lifecycle events by closed notification kind and stage.",
            ),
            &["kind", "stage"],
        )
        .map_err(MetricsError::Registry)?;
        let failures = IntCounterVec::new(
            Opts::new(
                "auction_notification_failures_total",
                "Outbound notification failures by closed notification kind and reason.",
            ),
            &["kind", "reason"],
        )
        .map_err(MetricsError::Registry)?;
        let latency = HistogramVec::new(
            HistogramOpts::new(
                "auction_notification_delivery_latency_seconds",
                "Outbound notification latency from creation to terminal outcome.",
            )
            .buckets(LATENCY_BUCKETS.to_vec()),
            &["kind"],
        )
        .map_err(MetricsError::Registry)?;
        let postage_estimate = IntCounterVec::new(
            Opts::new(
                "auction_notification_postage_estimate_credits_total",
                "Estimated postage credits consumed by sent notifications per kind.",
            ),
            &["kind"],
        )
        .map_err(MetricsError::Registry)?;

        // 预置整个闭集零值：kind × stage、kind × reason、延迟直方图与邮资计数器。
        let stages = NotificationKind::ALL.map(|kind| {
            NotificationStage::ALL
                .map(|stage| stage_family.with_label_values(&[kind.label(), stage.label()]))
        });
        for kind in NotificationKind::ALL {
            for failure in NotificationFailure::ALL {
                failures.with_label_values(&[kind.label(), failure.label()]);
            }
            latency.with_label_values(&[kind.label()]);
            postage_estimate.with_label_values(&[kind.label()]);
        }
        // 注册共享同一底层收集器的克隆句柄，保留原 Vec 供逐笔记录使用。
        registry.register(Box::new(stage_family.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(failures.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(latency.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(postage_estimate.clone())).map_err(MetricsError::Registry)?;

        Ok(Arc::new(Self { stages, failures, latency, postage_estimate }))
    }
}

impl NotificationRecorder for PrometheusNotificationRecorder {
    fn observe_stage(&self, kind: NotificationKind, stage: NotificationStage) {
        self.stages[kind as usize][stage as usize].inc();
    }

    fn observe_failure(&self, kind: NotificationKind, failure: NotificationFailure) {
        self.failures.with_label_values(&[kind.label(), failure.label()]).inc();
    }

    fn observe_delivery_latency(&self, kind: NotificationKind, latency: std::time::Duration) {
        self.latency.with_label_values(&[kind.label()]).observe(latency.as_secs_f64());
    }

    fn observe_postage_estimate(&self, kind: NotificationKind, credits: u64) {
        if credits == 0 {
            return;
        }
        self.postage_estimate.with_label_values(&[kind.label()]).inc_by(credits);
    }
}

pub(super) fn build(registry: &Registry) -> Result<Arc<dyn NotificationRecorder>, MetricsError> {
    PrometheusNotificationRecorder::build(registry)
}

// 缓存按枚举判别值寻址；闭集顺序在编译期核对，新增或重排变体时不能静默串写指标。
const _: () = {
    let mut index = 0;
    while index < NotificationKind::ALL.len() {
        assert!(NotificationKind::ALL[index] as usize == index);
        index += 1;
    }
    index = 0;
    while index < NotificationStage::ALL.len() {
        assert!(NotificationStage::ALL[index] as usize == index);
        index += 1;
    }
};

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use prometheus::Registry;

    use self::parent::*;
    use super as parent;

    fn scrape(registry: &Registry) -> String {
        use prometheus::{Encoder, TextEncoder};
        let mut buffer = Vec::new();
        TextEncoder::new().encode(&registry.gather(), &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    fn count(sample: &str, labels: &str) -> f64 {
        sample
            .lines()
            .find(|line| line.starts_with("auction_notification_") && line.contains(labels))
            .and_then(|line| line.rsplit(' ').next())
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("缺少样本 {labels}"))
    }

    #[test]
    fn closed_sets_are_zero_initialized() {
        let registry = Registry::new();
        build(&registry).unwrap();
        let sample = scrape(&registry);
        for kind in NotificationKind::ALL {
            for stage in NotificationStage::ALL {
                let labels = format!("kind=\"{}\",stage=\"{}\"", kind.label(), stage.label());
                assert_eq!(count(&sample, &labels), 0.0, "{labels} 必须有零值");
            }
            for failure in NotificationFailure::ALL {
                let labels = format!("kind=\"{}\",reason=\"{}\"", kind.label(), failure.label());
                assert_eq!(count(&sample, &labels), 0.0, "{labels} 必须有零值");
            }
            // 直方图与邮资计数器也必须先出现稳定的零值族。
            assert!(sample.contains(&format!(
                "auction_notification_delivery_latency_seconds_count{{kind=\"{}\"}} 0",
                kind.label()
            )));
            assert!(sample.contains(&format!(
                "auction_notification_postage_estimate_credits_total{{kind=\"{}\"}} 0",
                kind.label()
            )));
        }
    }

    #[test]
    fn observations_accumulate_without_dynamic_labels() {
        let registry = Registry::new();
        let recorder = build(&registry).unwrap();
        let kind = NotificationKind::SettlementPaymentReminder;
        recorder.observe_stage(kind, NotificationStage::Created);
        recorder.observe_stage(kind, NotificationStage::Sent);
        recorder.observe_stage(kind, NotificationStage::Failed);
        recorder.observe_failure(kind, NotificationFailure::InsufficientPostage);
        recorder.observe_delivery_latency(kind, Duration::from_secs(90));
        recorder.observe_postage_estimate(kind, 3);
        let sample = scrape(&registry);
        assert_eq!(count(&sample, &format!("kind=\"{}\",stage=\"sent\"", kind.label())), 1.0);
        assert_eq!(
            count(&sample, &format!("kind=\"{}\",reason=\"insufficient_postage\"", kind.label())),
            1.0
        );
        assert_eq!(
            count(
                &sample,
                &format!(
                    "auction_notification_postage_estimate_credits_total{{kind=\"{}\"}}",
                    kind.label()
                )
            ),
            3.0
        );
        assert!(sample.contains(&format!(
            "auction_notification_delivery_latency_seconds_sum{{kind=\"{}\"}} 90",
            kind.label()
        )));
    }

    #[test]
    fn zero_postage_estimate_is_not_emitted() {
        let registry = Registry::new();
        let recorder = build(&registry).unwrap();
        recorder.observe_postage_estimate(NotificationKind::LotWatchMail, 0);
        let sample = scrape(&registry);
        assert_eq!(
            count(
                &sample,
                &format!(
                    "auction_notification_postage_estimate_credits_total{{kind=\"{}\"}}",
                    NotificationKind::LotWatchMail.label()
                )
            ),
            0.0
        );
    }
}
