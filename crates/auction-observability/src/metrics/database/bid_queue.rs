//! 出价队列的等待与结果计量：区分「进程内按场次排队」与「跨实例会话锁」两段等待。
//!
//! 只使用闭集标签，不携带拍卖、lot、命令 ID 或 SQL；关闭指标记录器时为 no-op，不影响排队、
//! 事务或错误结果。

use std::time::Duration;

use super::DatabaseRecorder;

/// 等待阶段：进程内按场次排队，或等待其它实例释放 PostgreSQL 会话级 advisory lock。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BidQueueWaitStage {
    InProcess,
    CrossInstance,
}

impl BidQueueWaitStage {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::InProcess => "in_process",
            Self::CrossInstance => "cross_instance",
        }
    }
}

/// 一次排队请求的最终结果；取消与超时都按实际发生的事件计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BidQueueOutcome {
    Acquired,
    TimedOut,
    Canceled,
    Failed,
}

impl BidQueueOutcome {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Acquired => "acquired",
            Self::TimedOut => "timed_out",
            Self::Canceled => "canceled",
            Self::Failed => "failed",
        }
    }
}

impl DatabaseRecorder {
    /// 记录一段实际发生的排队等待；未等待时不调用，避免零值污染分位。
    pub fn bid_queue_wait(&self, stage: BidQueueWaitStage, duration: Duration) {
        if let Some(metrics) = &self.0 {
            metrics
                .bid_queue_wait
                .with_label_values(&[stage.label()])
                .observe(duration.as_secs_f64());
        }
    }

    /// 记录一次排队请求的最终结果；只在请求真正经过队列时调用。
    pub fn bid_queue_outcome(&self, outcome: BidQueueOutcome) {
        if let Some(metrics) = &self.0 {
            metrics.bid_queue_requests.with_label_values(&[outcome.label()]).inc();
        }
    }
}
