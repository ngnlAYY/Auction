//! 增量扫描指标：计划、前缀、每日校验与影子比较都只接受闭集标签，
//! 不把 MID、邮件内容、错误原文或凭据带入指标。

use std::sync::Arc;

use auction_application::mail::{
    MailScanEvent, MailScanMode, MailScanObserver, MailScanOutcome, MailScanReason, MailScanStream,
};
use prometheus::{IntCounterVec, Opts, Registry};

use super::types::MetricsError;

struct MailScanMetrics {
    plan: IntCounterVec,
    prefix: IntCounterVec,
    verification: IntCounterVec,
    shadow: IntCounterVec,
}

impl MailScanObserver for MailScanMetrics {
    fn observe(&self, world: &str, event: MailScanEvent) {
        let world = match world {
            "main" => "main",
            "isekai" => "isekai",
            _ => return,
        };
        match event {
            MailScanEvent::Plan { stream, mode, reason } => {
                self.plan
                    .with_label_values(&[world, stream.label(), mode.label(), reason.label()])
                    .inc();
            }
            MailScanEvent::Prefix { stream, outcome, reason } => {
                self.prefix
                    .with_label_values(&[
                        world,
                        stream.label(),
                        outcome.label(),
                        reason.map_or("none", MailScanReason::label),
                    ])
                    .inc();
            }
            MailScanEvent::Verification { stream, outcome, reason } => {
                self.verification
                    .with_label_values(&[world, stream.label(), outcome.label(), reason.label()])
                    .inc();
            }
            MailScanEvent::Shadow { stream, outcome, mid_sets_equal } => {
                self.shadow
                    .with_label_values(&[
                        world,
                        stream.label(),
                        outcome.label(),
                        if mid_sets_equal { "match" } else { "mismatch" },
                    ])
                    .inc();
            }
        }
    }
}

pub(super) fn build(registry: &Registry) -> Result<Arc<dyn MailScanObserver>, MetricsError> {
    let metrics = MailScanMetrics {
        plan: counter(
            "plan",
            "Mail scan plans by stream and mode; full reasons exclude daily checks.",
            &["world", "stream", "mode", "reason"],
        )?,
        prefix: counter(
            "prefix",
            "Incremental MID prefix scans and their closed-set fallback reasons.",
            &["world", "stream", "outcome", "reason"],
        )?,
        verification: counter(
            "verification",
            "Complete or daily verification scans; page limits warn without blocking.",
            &["world", "stream", "outcome", "reason"],
        )?,
        shadow: counter(
            "shadow",
            "Incremental versus full new-MID set comparisons.",
            &["world", "stream", "outcome", "result"],
        )?,
    };
    // 只预置实际可达的闭集组合；未列出的标签仍会被计数器按需创建，
    // 因此这里不是“允许任意值”，而是让稳定分支一开始就可见。
    const FULL_REASONS: [MailScanReason; 6] = [
        MailScanReason::NoCheckpoint,
        MailScanReason::NoWatermark,
        MailScanReason::NoFullVerification,
        MailScanReason::InvalidCheckpoint,
        MailScanReason::DailyVerification,
        MailScanReason::RequestedFull,
    ];
    const PREFIX_FALLBACKS: [MailScanReason; 7] = [
        MailScanReason::AnchorMissing,
        MailScanReason::PageDiscontinuity,
        MailScanReason::OrderViolation,
        MailScanReason::InvalidMid,
        MailScanReason::PageLimit,
        MailScanReason::ReadUnavailable,
        MailScanReason::AnchorChanged,
    ];
    for world in ["main", "isekai"] {
        for stream in MailScanStream::ALL {
            for reason in FULL_REASONS {
                metrics.plan.with_label_values(&[
                    world,
                    stream.label(),
                    MailScanMode::Full.label(),
                    reason.label(),
                ]);
            }
            metrics.plan.with_label_values(&[
                world,
                stream.label(),
                MailScanMode::Incremental.label(),
                MailScanReason::Incremental.label(),
            ]);
            metrics.prefix.with_label_values(&[
                world,
                stream.label(),
                MailScanOutcome::Complete.label(),
                "none",
            ]);
            for reason in PREFIX_FALLBACKS {
                metrics.prefix.with_label_values(&[
                    world,
                    stream.label(),
                    MailScanOutcome::Error.label(),
                    reason.label(),
                ]);
            }
            metrics.verification.with_label_values(&[
                world,
                stream.label(),
                MailScanOutcome::PageLimit.label(),
                MailScanReason::PageLimit.label(),
            ]);
            for reason in FULL_REASONS {
                metrics.verification.with_label_values(&[
                    world,
                    stream.label(),
                    MailScanOutcome::Complete.label(),
                    reason.label(),
                ]);
            }
            for (outcome, reason) in [
                (MailScanOutcome::Error, MailScanReason::DailyVerification),
                (MailScanOutcome::Canceled, MailScanReason::Canceled),
            ] {
                metrics.verification.with_label_values(&[
                    world,
                    stream.label(),
                    outcome.label(),
                    reason.label(),
                ]);
            }
            for result in ["match", "mismatch"] {
                metrics.shadow.with_label_values(&[
                    world,
                    stream.label(),
                    MailScanOutcome::Complete.label(),
                    result,
                ]);
            }
        }
    }
    for counter in [&metrics.plan, &metrics.prefix, &metrics.verification, &metrics.shadow] {
        registry.register(Box::new(counter.clone())).map_err(MetricsError::Registry)?;
    }
    Ok(Arc::new(metrics))
}

fn counter(suffix: &str, help: &str, labels: &[&str]) -> Result<IntCounterVec, MetricsError> {
    IntCounterVec::new(Opts::new(format!("auction_mooglemail_scan_{suffix}_total"), help), labels)
        .map_err(MetricsError::Registry)
}
