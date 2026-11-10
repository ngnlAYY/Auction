//! Public relay 与逐连接 WS 写出的延迟代理；不参与路由或发送判定。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, Opts};

use super::{MetricsError, ProcessMetrics};

/// 固定 stream 范围，不导出 stream 字符串或拍卖标识。
#[derive(Debug, Clone, Copy)]
pub enum RealtimeScope {
    Auction,
    Discovery,
}

impl RealtimeScope {
    const ALL: [Self; 2] = [Self::Auction, Self::Discovery];

    fn label(self) -> &'static str {
        match self {
            Self::Auction => "auction",
            Self::Discovery => "discovery",
        }
    }
}

#[derive(Default)]
struct Metrics {
    relay: Option<HistogramVec>,
    push: Option<HistogramVec>,
    skipped: Option<IntCounterVec>,
}

/// 每个 Public 注册一次；关闭指标时不创建 collector。
#[derive(Clone, Default)]
pub struct RealtimeRecorder(Arc<Metrics>);

impl RealtimeRecorder {
    /// 由 Public 装配根注册，再注入 Hub。
    pub fn register(process: &ProcessMetrics) -> Result<Self, MetricsError> {
        let Some(registry) = process.enabled_registry() else {
            return Ok(Self::default());
        };
        let relay = histogram("auction_realtime_relay_duration_seconds")?;
        let push = histogram("auction_realtime_push_duration_seconds")?;
        let skipped = IntCounterVec::new(
            Opts::new("auction_realtime_observations_skipped_total", "时间无效的延迟观测数。"),
            &["scope", "stage"],
        )
        .map_err(MetricsError::Registry)?;
        for scope in RealtimeScope::ALL {
            // 无样本时保留零 count；p99 不应解释为零耗时。
            relay.with_label_values(&[scope.label()]);
            push.with_label_values(&[scope.label()]);
            for stage in ["relay", "push"] {
                skipped.with_label_values(&[scope.label(), stage]);
            }
        }
        registry.register(Box::new(relay.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(push.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(skipped.clone())).map_err(MetricsError::Registry)?;
        Ok(Self(Arc::new(Metrics { relay: Some(relay), push: Some(push), skipped: Some(skipped) })))
    }

    /// 成功交给本地 Hub，不代表任何连接或客户端已收到。
    pub fn relay(&self, scope: RealtimeScope, created_at: DateTime<Utc>) {
        self.observe(self.0.relay.as_ref(), scope, "relay", created_at);
    }

    /// 一个事件副本成功完成 WS 写出；不记录失败、Ping 或 HTTP 补读。
    pub fn push(&self, scope: RealtimeScope, created_at: DateTime<Utc>) {
        self.observe(self.0.push.as_ref(), scope, "push", created_at);
    }

    fn observe(
        &self,
        histogram: Option<&HistogramVec>,
        scope: RealtimeScope,
        stage: &'static str,
        created_at: DateTime<Utc>,
    ) {
        let Some(histogram) = histogram else { return };
        // 负时差表示时钟或证据异常，不以零延迟掩盖。
        let Ok(duration) = (Utc::now() - created_at).to_std() else {
            if let Some(skipped) = &self.0.skipped {
                skipped.with_label_values(&[scope.label(), stage]).inc();
            }
            return;
        };
        histogram.with_label_values(&[scope.label()]).observe(duration.as_secs_f64());
    }
}

fn histogram(name: &str) -> Result<HistogramVec, MetricsError> {
    let options = HistogramOpts::new(name, "持久事件创建至服务端观测终点的秒数。")
        .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0, 30.0, 120.0]);
    HistogramVec::new(options, &["scope"]).map_err(MetricsError::Registry)
}
