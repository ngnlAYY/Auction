//! 数据库连接池快照的 Prometheus collector。

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use prometheus::{
    Counter, Gauge, IntCounter, Opts,
    core::{Collector, Desc},
    proto::MetricFamily,
};

use super::types::{MetricsError, PoolMetricsSource};

/// SQLx 当前公开能力对应的连接池 Prometheus collector。
pub(super) struct PoolMetricsCollector {
    source: Arc<dyn PoolMetricsSource>,
    acquired_connections: Gauge,
    idle_connections: Gauge,
    total_connections: Gauge,
    max_connections: Gauge,
    constructing_connections: Gauge,
    acquires: IntCounter,
    acquire_duration: Counter,
    empty_acquires: IntCounter,
    empty_acquire_wait_duration: Counter,
    canceled_acquires: IntCounter,
    last_acquires: AtomicU64,
    last_acquire_duration_nanos: AtomicU64,
    last_empty_acquires: AtomicU64,
    last_empty_acquire_wait_duration_nanos: AtomicU64,
    last_canceled_acquires: AtomicU64,
}

impl PoolMetricsCollector {
    pub(super) fn new(source: Arc<dyn PoolMetricsSource>) -> Result<Self, MetricsError> {
        let prefix = source.backend().metric_prefix();
        let metric = |name: &str, help: &str| {
            Gauge::with_opts(Opts::new(format!("auction_{prefix}_pool_{name}"), help))
                .map_err(MetricsError::Registry)
        };

        Ok(Self {
            source,
            acquired_connections: metric(
                "acquired_connections",
                "Current number of database pool connections checked out.",
            )?,
            idle_connections: metric(
                "idle_connections",
                "Current number of idle database pool connections.",
            )?,
            total_connections: metric(
                "total_connections",
                "Current number of database pool connections.",
            )?,
            max_connections: metric(
                "max_connections",
                "Configured maximum number of database pool connections.",
            )?,
            constructing_connections: metric(
                "constructing_connections",
                "Current number of tracked database pool connection growth attempts.",
            )?,
            acquires: IntCounter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_acquires_total"),
                "Total successful database pool acquires.",
            ))
            .map_err(MetricsError::Registry)?,
            acquire_duration: Counter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_acquire_duration_seconds_total"),
                "Total time spent acquiring database pool connections in seconds.",
            ))
            .map_err(MetricsError::Registry)?,
            empty_acquires: IntCounter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_empty_acquires_total"),
                "Total database pool acquires that started without an idle connection.",
            ))
            .map_err(MetricsError::Registry)?,
            empty_acquire_wait_duration: Counter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_empty_acquire_wait_duration_seconds_total"),
                "Total wait time for database pool acquires that started without an idle connection.",
            ))
            .map_err(MetricsError::Registry)?,
            canceled_acquires: IntCounter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_canceled_acquires_total"),
                "Total database pool acquires canceled before completion.",
            ))
            .map_err(MetricsError::Registry)?,
            last_acquires: AtomicU64::new(0),
            last_acquire_duration_nanos: AtomicU64::new(0),
            last_empty_acquires: AtomicU64::new(0),
            last_empty_acquire_wait_duration_nanos: AtomicU64::new(0),
            last_canceled_acquires: AtomicU64::new(0),
        })
    }
}

impl Collector for PoolMetricsCollector {
    fn desc(&self) -> Vec<&Desc> {
        [
            self.acquired_connections.desc(),
            self.idle_connections.desc(),
            self.total_connections.desc(),
            self.max_connections.desc(),
            self.constructing_connections.desc(),
            self.acquires.desc(),
            self.acquire_duration.desc(),
            self.empty_acquires.desc(),
            self.empty_acquire_wait_duration.desc(),
            self.canceled_acquires.desc(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let snapshot = self.source.snapshot();
        self.acquired_connections.set(f64::from(snapshot.acquired_connections));
        self.idle_connections.set(f64::from(snapshot.idle_connections));
        self.total_connections.set(f64::from(snapshot.total_connections));
        self.max_connections.set(f64::from(snapshot.max_connections));
        self.constructing_connections.set(f64::from(snapshot.constructing_connections));
        increment_counter(&self.acquires, &self.last_acquires, snapshot.acquires, 1.0);
        increment_counter(
            &self.acquire_duration,
            &self.last_acquire_duration_nanos,
            snapshot.acquire_duration_nanos,
            1_000_000_000.0,
        );
        increment_counter(
            &self.empty_acquires,
            &self.last_empty_acquires,
            snapshot.empty_acquires,
            1.0,
        );
        increment_counter(
            &self.empty_acquire_wait_duration,
            &self.last_empty_acquire_wait_duration_nanos,
            snapshot.empty_acquire_wait_duration_nanos,
            1_000_000_000.0,
        );
        increment_counter(
            &self.canceled_acquires,
            &self.last_canceled_acquires,
            snapshot.canceled_acquires,
            1.0,
        );

        [
            self.acquired_connections.collect(),
            self.idle_connections.collect(),
            self.total_connections.collect(),
            self.max_connections.collect(),
            self.constructing_connections.collect(),
            self.acquires.collect(),
            self.acquire_duration.collect(),
            self.empty_acquires.collect(),
            self.empty_acquire_wait_duration.collect(),
            self.canceled_acquires.collect(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

fn increment_counter<C>(counter: &C, last: &AtomicU64, current: u64, divisor: f64)
where
    C: CounterIncrement,
{
    loop {
        let previous = last.load(Ordering::Relaxed);
        if current <= previous {
            return;
        }
        let delta = current - previous;
        if last.compare_exchange(previous, current, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            // 这里需要通过具体 counter 类型增加；由下面的 trait 适配器统一处理浮点与整数。
            counter_increment(counter, delta, divisor);
            return;
        }
    }
}

trait CounterIncrement {
    fn increment(&self, delta: u64, divisor: f64);
}

impl CounterIncrement for IntCounter {
    fn increment(&self, delta: u64, _divisor: f64) {
        self.inc_by(delta);
    }
}

impl CounterIncrement for Counter {
    fn increment(&self, delta: u64, divisor: f64) {
        self.inc_by(delta as f64 / divisor);
    }
}

fn counter_increment<C>(counter: &C, delta: u64, divisor: f64)
where
    C: CounterIncrement,
{
    counter.increment(delta, divisor);
}
