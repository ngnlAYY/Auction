//! Public relay 与逐连接 WS 写出的延迟代理；不参与路由或发送判定。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, Opts};

use super::{MetricsError, ProcessMetrics};

/// `scope` 标签的唯一取值：公开实时只有单拍卖流。保留该标签使指标标签集合与记录规则不变，
/// 不导出 stream 字符串或拍卖标识。
const AUCTION_SCOPE: &str = "auction";

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
        // 无样本时保留零 count；p99 不应解释为零耗时。
        relay.with_label_values(&[AUCTION_SCOPE]);
        push.with_label_values(&[AUCTION_SCOPE]);
        for stage in ["relay", "push"] {
            skipped.with_label_values(&[AUCTION_SCOPE, stage]);
        }
        registry.register(Box::new(relay.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(push.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(skipped.clone())).map_err(MetricsError::Registry)?;
        Ok(Self(Arc::new(Metrics { relay: Some(relay), push: Some(push), skipped: Some(skipped) })))
    }

    /// 成功交给本地 Hub，不代表任何连接或客户端已收到。
    pub fn relay(&self, created_at: DateTime<Utc>) {
        self.observe(self.0.relay.as_ref(), "relay", created_at);
    }

    /// 一个事件副本成功完成 WS 写出；不记录失败、Ping 或 HTTP 补读。
    pub fn push(&self, created_at: DateTime<Utc>) {
        self.observe(self.0.push.as_ref(), "push", created_at);
    }

    fn observe(
        &self,
        histogram: Option<&HistogramVec>,
        stage: &'static str,
        created_at: DateTime<Utc>,
    ) {
        let Some(histogram) = histogram else { return };
        // 负时差表示时钟或证据异常，不以零延迟掩盖。
        let Ok(duration) = (Utc::now() - created_at).to_std() else {
            if let Some(skipped) = &self.0.skipped {
                skipped.with_label_values(&[AUCTION_SCOPE, stage]).inc();
            }
            return;
        };
        histogram.with_label_values(&[AUCTION_SCOPE]).observe(duration.as_secs_f64());
    }
}

fn histogram(name: &str) -> Result<HistogramVec, MetricsError> {
    let options = HistogramOpts::new(name, "持久事件创建至服务端观测终点的秒数。")
        .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0, 30.0, 120.0]);
    HistogramVec::new(options, &["scope"]).map_err(MetricsError::Registry)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::RealtimeRecorder;
    use crate::metrics::ProcessMetrics;

    #[test]
    fn latency_series_only_carry_the_auction_scope() {
        let process =
            ProcessMetrics::new(true, "auction-test", "public", "0.0.0").expect("启用进程指标");
        let recorder = RealtimeRecorder::register(&process).expect("注册实时延迟指标");
        recorder.relay(Utc::now() - Duration::seconds(1));
        // 未来的创建时间是证据异常：不记零延迟，只累加 skipped。
        recorder.push(Utc::now() + Duration::hours(1));
        let text = String::from_utf8(process.encode().expect("编码指标")).expect("UTF-8 指标文本");
        for expected in [
            r#"auction_realtime_relay_duration_seconds_count{scope="auction"} 1"#,
            r#"auction_realtime_push_duration_seconds_count{scope="auction"} 0"#,
            r#"auction_realtime_observations_skipped_total{scope="auction",stage="push"} 1"#,
            r#"auction_realtime_observations_skipped_total{scope="auction",stage="relay"} 0"#,
        ] {
            assert!(text.lines().any(|line| line == expected), "缺少 {expected}: {text}");
        }
        let realtime = text.lines().filter(|line| line.starts_with("auction_realtime_"));
        for line in realtime {
            assert!(line.contains(r#"scope="auction""#), "scope 标签只能取 auction: {line}");
        }
    }
}
