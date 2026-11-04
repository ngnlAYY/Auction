//! MoogleMail 与论坛物理请求、等待、预算及导入延迟的 Prometheus 记录器。

use std::sync::Arc;

use auction_application::{ExternalMetricsEvent, ExternalMetricsObserver, ReplyDelayBound};
use prometheus::{Gauge, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry};

use super::super::types::MetricsError;

const WAIT_BUCKETS: &[f64] =
    &[0.001, 0.01, 0.05, 0.1, 0.5, 0.75, 1.0, 2.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0];
const PAGE_BUCKETS: &[f64] = &[0.0, 1.0, 2.0, 3.0, 5.0, 8.0, 10.0, 15.0, 20.0, 50.0, 100.0];
const DELAY_BUCKETS: &[f64] =
    &[1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0, 86400.0];

struct ExternalClientMetrics {
    mail_requests: IntCounterVec,
    mail_results: IntCounterVec,
    mail_signals: IntCounterVec,
    mail_gate_wait: HistogramVec,
    mail_lock_wait: HistogramVec,
    mail_budget_exhaustions: IntCounterVec,
    mail_request_interval: Gauge,
    forum_requests: IntCounterVec,
    forum_results: IntCounterVec,
    forum_signals: IntCounterVec,
    forum_gate_wait: HistogramVec,
    forum_request_interval: Gauge,
    forum_topic_scan_pages: HistogramVec,
    forum_write_unknown: IntCounterVec,
    forum_pm_operations: IntCounterVec,
    forum_budget_exhaustions: IntCounterVec,
    forum_reply_delay_estimate: HistogramVec,
    forum_reply_delay_samples: IntCounterVec,
}

impl ExternalMetricsObserver for ExternalClientMetrics {
    fn observe(&self, event: ExternalMetricsEvent) {
        match event {
            ExternalMetricsEvent::MailRequestStarted { world, operation } => {
                self.mail_requests.with_label_values(&[world.label(), operation.label()]).inc();
            }
            ExternalMetricsEvent::MailRequestFinished { world, operation, category } => {
                self.mail_results
                    .with_label_values(&[world.label(), operation.label(), category.label()])
                    .inc();
            }
            ExternalMetricsEvent::MailRequestSignal { world, operation, signal } => {
                self.mail_signals
                    .with_label_values(&[world.label(), operation.label(), signal.label()])
                    .inc();
            }
            ExternalMetricsEvent::MailGateWait { world, operation, outcome, waited } => {
                self.mail_gate_wait
                    .with_label_values(&[world.label(), operation.label(), outcome.label()])
                    .observe(waited.as_secs_f64());
            }
            ExternalMetricsEvent::MailLockWait { world, operation, lock, outcome, waited } => {
                self.mail_lock_wait
                    .with_label_values(&[
                        world.label(),
                        operation.label(),
                        lock.label(),
                        outcome.label(),
                    ])
                    .observe(waited.as_secs_f64());
            }
            ExternalMetricsEvent::MailBudgetExhausted { world, phase, budget } => {
                self.mail_budget_exhaustions
                    .with_label_values(&[world.label(), phase.label(), budget.label()])
                    .inc();
            }
            ExternalMetricsEvent::MailRequestInterval { interval } => {
                self.mail_request_interval.set(interval.as_secs_f64());
            }
            ExternalMetricsEvent::ForumRequestStarted { world, operation } => {
                self.forum_requests.with_label_values(&[world.label(), operation.label()]).inc();
            }
            ExternalMetricsEvent::ForumRequestFinished { world, operation, category } => {
                self.forum_results
                    .with_label_values(&[world.label(), operation.label(), category.label()])
                    .inc();
            }
            ExternalMetricsEvent::ForumRequestSignal { world, operation, signal } => {
                self.forum_signals
                    .with_label_values(&[world.label(), operation.label(), signal.label()])
                    .inc();
            }
            ExternalMetricsEvent::ForumGateWait { world, operation, outcome, waited } => {
                self.forum_gate_wait
                    .with_label_values(&[world.label(), operation.label(), outcome.label()])
                    .observe(waited.as_secs_f64());
            }
            ExternalMetricsEvent::ForumRequestInterval { interval } => {
                self.forum_request_interval.set(interval.as_secs_f64());
            }
            ExternalMetricsEvent::ForumTopicScanPages { world, mode, measure, outcome, pages } => {
                self.forum_topic_scan_pages
                    .with_label_values(&[
                        world.label(),
                        mode.label(),
                        measure.label(),
                        outcome.label(),
                    ])
                    .observe(pages as f64);
            }
            ExternalMetricsEvent::ForumWriteUnknown { world, operation } => {
                self.forum_write_unknown
                    .with_label_values(&[world.label(), operation.label()])
                    .inc();
            }
            ExternalMetricsEvent::ForumPmOperation { world, phase, outcome, category } => {
                self.forum_pm_operations
                    .with_label_values(&[
                        world.label(),
                        phase.label(),
                        outcome.label(),
                        category.label(),
                    ])
                    .inc();
            }
            ExternalMetricsEvent::ForumBudgetExhausted { world, phase } => {
                self.forum_budget_exhaustions
                    .with_label_values(&[world.label(), phase.label()])
                    .inc();
            }
            ExternalMetricsEvent::ForumReplyDelayEstimate {
                world,
                phase,
                method,
                lower,
                midpoint,
                upper,
            } => {
                for (bound, value) in [
                    (ReplyDelayBound::Lower, lower),
                    (ReplyDelayBound::Midpoint, midpoint),
                    (ReplyDelayBound::Upper, upper),
                ] {
                    self.forum_reply_delay_estimate
                        .with_label_values(&[
                            world.label(),
                            phase.label(),
                            method.label(),
                            bound.label(),
                        ])
                        .observe(value.as_secs_f64());
                }
            }
            ExternalMetricsEvent::ForumReplyDelaySample { world, phase, method, outcome } => {
                self.forum_reply_delay_samples
                    .with_label_values(&[
                        world.label(),
                        phase.label(),
                        method.label(),
                        outcome.label(),
                    ])
                    .inc();
            }
        }
    }
}

pub(crate) fn build(registry: &Registry) -> Result<Arc<dyn ExternalMetricsObserver>, MetricsError> {
    let metrics = ExternalClientMetrics {
        mail_requests: counter(
            "auction_mooglemail_requests_total",
            "MoogleMail requests started at the physical execute boundary.",
            &["world", "operation"],
        )?,
        mail_results: counter(
            "auction_mooglemail_request_results_total",
            "MoogleMail request boundary results, including canceled requests.",
            &["world", "operation", "category"],
        )?,
        mail_signals: counter(
            "auction_mooglemail_request_signals_total",
            "Recognized MoogleMail busy, rate-limit, and timeout signals.",
            &["world", "operation", "kind"],
        )?,
        mail_gate_wait: histogram(
            "auction_mooglemail_request_gate_wait_seconds",
            "MoogleMail request gate mutex and interval wait.",
            &["world", "operation", "outcome"],
            WAIT_BUCKETS,
        )?,
        mail_lock_wait: histogram(
            "auction_mooglemail_lock_wait_seconds",
            "MoogleMail exclusive or incoming lock acquisition wait.",
            &["world", "operation", "lock", "outcome"],
            WAIT_BUCKETS,
        )?,
        mail_budget_exhaustions: counter(
            "auction_mooglemail_budget_exhaustions_total",
            "MoogleMail logical budget exhaustions by owner phase.",
            &["world", "phase", "budget"],
        )?,
        mail_request_interval: gauge(
            "auction_mooglemail_request_interval_seconds",
            "Configured shared MoogleMail request interval.",
        )?,
        forum_requests: counter(
            "auction_forum_requests_total",
            "Forum requests started at the physical execute boundary.",
            &["world", "operation"],
        )?,
        forum_results: counter(
            "auction_forum_request_results_total",
            "Forum request boundary results, including canceled requests.",
            &["world", "operation", "category"],
        )?,
        forum_signals: counter(
            "auction_forum_request_signals_total",
            "Recognized forum rate-limit and timeout signals.",
            &["world", "operation", "kind"],
        )?,
        forum_gate_wait: histogram(
            "auction_forum_request_gate_wait_seconds",
            "Forum request gate mutex and interval wait.",
            &["world", "operation", "outcome"],
            WAIT_BUCKETS,
        )?,
        forum_request_interval: gauge(
            "auction_forum_request_interval_seconds",
            "Configured shared forum request interval.",
        )?,
        forum_topic_scan_pages: histogram(
            "auction_forum_topic_scan_pages",
            "Discovered and processed forum topic pages.",
            &["world", "mode", "measure", "outcome"],
            PAGE_BUCKETS,
        )?,
        forum_write_unknown: counter(
            "auction_forum_write_unknown_total",
            "Forum logical writes with an unknown remote outcome.",
            &["world", "operation"],
        )?,
        forum_pm_operations: counter(
            "auction_forum_pm_operations_total",
            "Forum PM prepare, send, and reconcile outcomes.",
            &["world", "phase", "outcome", "category"],
        )?,
        forum_budget_exhaustions: counter(
            "auction_forum_budget_exhaustions_total",
            "Forum logical item or profile time budget exhaustions.",
            &["world", "phase"],
        )?,
        forum_reply_delay_estimate: histogram(
            "auction_forum_reply_delay_estimate_seconds",
            "Bounded forum reply import delay estimates.",
            &["world", "phase", "method", "bound"],
            DELAY_BUCKETS,
        )?,
        forum_reply_delay_samples: counter(
            "auction_forum_reply_delay_samples_total",
            "Forum reply delay sample coverage and missing reasons.",
            &["world", "phase", "method", "outcome"],
        )?,
    };
    for metric in [
        &metrics.mail_requests,
        &metrics.mail_results,
        &metrics.mail_signals,
        &metrics.mail_budget_exhaustions,
        &metrics.forum_requests,
        &metrics.forum_results,
        &metrics.forum_signals,
        &metrics.forum_write_unknown,
        &metrics.forum_pm_operations,
        &metrics.forum_budget_exhaustions,
        &metrics.forum_reply_delay_samples,
    ] {
        registry.register(Box::new(metric.clone())).map_err(MetricsError::Registry)?;
    }
    for metric in [
        &metrics.mail_gate_wait,
        &metrics.mail_lock_wait,
        &metrics.forum_gate_wait,
        &metrics.forum_topic_scan_pages,
        &metrics.forum_reply_delay_estimate,
    ] {
        registry.register(Box::new(metric.clone())).map_err(MetricsError::Registry)?;
    }
    registry
        .register(Box::new(metrics.mail_request_interval.clone()))
        .map_err(MetricsError::Registry)?;
    registry
        .register(Box::new(metrics.forum_request_interval.clone()))
        .map_err(MetricsError::Registry)?;
    Ok(Arc::new(metrics))
}

fn counter(name: &str, help: &str, labels: &[&str]) -> Result<IntCounterVec, MetricsError> {
    IntCounterVec::new(Opts::new(name, help), labels).map_err(MetricsError::Registry)
}

fn histogram(
    name: &str,
    help: &str,
    labels: &[&str],
    buckets: &[f64],
) -> Result<HistogramVec, MetricsError> {
    let options = HistogramOpts::new(name, help).buckets(buckets.to_vec());
    HistogramVec::new(options, labels).map_err(MetricsError::Registry)
}

fn gauge(name: &str, help: &str) -> Result<Gauge, MetricsError> {
    Gauge::with_opts(Opts::new(name, help)).map_err(MetricsError::Registry)
}
