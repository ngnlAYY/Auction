//! 限定维度的 Prometheus collector；只创建实际观测到的标签组合。

use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, IntGaugeVec, Opts, Registry};

use crate::MetricsError;

pub(super) struct DatabaseMetrics {
    pub(super) transactions: IntCounterVec,
    pub(super) attempts: IntCounterVec,
    pub(super) conflicts: IntCounterVec,
    pub(super) conflicted: IntCounterVec,
    pub(super) retries: IntCounterVec,
    pub(super) exhausted: IntCounterVec,
    pub(super) acquire_duration: HistogramVec,
    pub(super) hold_duration: HistogramVec,
    pub(super) connections: IntGaugeVec,
    pub(super) reserved: IntGaugeVec,
    pub(super) bid_queue_wait: HistogramVec,
    pub(super) bid_queue_requests: IntCounterVec,
}

impl DatabaseMetrics {
    pub(super) fn build(registry: &Registry) -> Result<Self, MetricsError> {
        let counter = |name, help, labels: &[&str]| {
            let value = IntCounterVec::new(Opts::new(name, help), labels)
                .map_err(MetricsError::Registry)?;
            registry.register(Box::new(value.clone())).map_err(MetricsError::Registry)?;
            Ok::<_, MetricsError>(value)
        };
        let histogram = |name, help, labels: &[&str]| {
            let value = HistogramVec::new(
                HistogramOpts::new(name, help)
                    .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 30.0, 120.0]),
                labels,
            )
            .map_err(MetricsError::Registry)?;
            registry.register(Box::new(value.clone())).map_err(MetricsError::Registry)?;
            Ok::<_, MetricsError>(value)
        };
        let gauge = |name, help, labels: &[&str]| {
            let value =
                IntGaugeVec::new(Opts::new(name, help), labels).map_err(MetricsError::Registry)?;
            registry.register(Box::new(value.clone())).map_err(MetricsError::Registry)?;
            Ok::<_, MetricsError>(value)
        };
        Ok(Self {
            transactions: counter(
                "auction_db_transactions_total",
                "Logical transaction operations.",
                &["owner", "outcome", "phase"],
            )?,
            attempts: counter(
                "auction_db_transaction_attempts_total",
                "Actual transaction attempts.",
                &["owner", "phase"],
            )?,
            conflicts: counter(
                "auction_db_transaction_conflicts_total",
                "Conflicted attempts, counted once.",
                &["owner", "kind", "phase", "site"],
            )?,
            conflicted: counter(
                "auction_db_transaction_conflicted_total",
                "Operations with at least one conflict.",
                &["owner", "phase"],
            )?,
            retries: counter(
                "auction_db_transaction_retries_total",
                "New attempts after a conflict.",
                &["owner", "phase"],
            )?,
            exhausted: counter(
                "auction_db_transaction_retry_exhausted_total",
                "Owner confirmed retry exhaustion.",
                &["owner", "phase"],
            )?,
            acquire_duration: histogram(
                "auction_worker_db_acquire_duration_seconds",
                "Connection acquisition duration.",
                &["loop", "outcome"],
            )?,
            hold_duration: histogram(
                "auction_worker_db_connection_hold_duration_seconds",
                "Application lease duration.",
                &["loop"],
            )?,
            connections: gauge(
                "auction_worker_db_connections",
                "Connections held by a Worker loop.",
                &["loop"],
            )?,
            reserved: gauge(
                "auction_db_reserved_connections",
                "Long lived owned connections.",
                &["purpose"],
            )?,
            bid_queue_wait: histogram(
                "auction_bid_queue_wait_seconds",
                "Time spent waiting for the per-auction bid queue.",
                &["stage"],
            )?,
            bid_queue_requests: counter(
                "auction_bid_queue_requests_total",
                "Bid commands that passed through the per-auction queue.",
                &["outcome"],
            )?,
        })
    }
}
