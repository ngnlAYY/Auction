//! 寄售 Worker 循环的闭集与指标映射：每个循环映射到 Worker 固定的循环标签。

use std::time::Duration;

use auction_application::core::diagnostics::SafeErrorKind;
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult};

/// 供指标装配映射到 Worker 固定循环标签的闭集。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsignmentLoop {
    /// 单并发装备解析。
    Resolve,
    /// 本地补扫与租约回收。
    Maintenance,
    /// 持久准入就绪心跳。
    Readiness,
    /// Take / Return 处置执行。
    Disposition,
    /// 隔离重检。
    Quarantine,
    /// 季窗冻结、季前通知与季末关闭。
    Seasons,
    /// 处罚账号工作执行。
    Sanctions,
    /// 权利通知投递。
    RightNotices,
    /// 退回处置、自动重列与付款提醒。
    Recovery,
    /// 寄售派发 executor。
    Dispatch,
    /// 补寄处置授权新轮。
    Redispatch,
    /// 下游义务（isekai 原件交割、卖款、退款）授权首轮。
    DownstreamRounds,
    /// 退件 Take executor。
    ReturnTakes,
    /// 订单复核消费端。
    OrderRechecks,
    /// 收款与确领推进（只读 Sent，经既有写口记录证据、认证与确领）。
    Collections,
}

pub(crate) fn observe_worker_iteration(
    metrics: &dyn WorkerRecorder,
    kind: ConsignmentLoop,
    result: WorkerResult,
    duration: Duration,
    failed: u32,
) {
    let loop_kind = match kind {
        ConsignmentLoop::Resolve => WorkerLoop::ConsignmentResolve,
        ConsignmentLoop::Maintenance => WorkerLoop::ConsignmentMaintenance,
        ConsignmentLoop::Readiness => WorkerLoop::ConsignmentReadiness,
        ConsignmentLoop::Disposition
        | ConsignmentLoop::Quarantine
        | ConsignmentLoop::Seasons
        | ConsignmentLoop::Sanctions
        | ConsignmentLoop::RightNotices
        | ConsignmentLoop::Recovery
        | ConsignmentLoop::Dispatch
        | ConsignmentLoop::Redispatch
        | ConsignmentLoop::DownstreamRounds
        | ConsignmentLoop::ReturnTakes
        | ConsignmentLoop::OrderRechecks
        | ConsignmentLoop::Collections => WorkerLoop::ConsignmentMaintenance,
    };
    metrics.observe_iteration(loop_kind, result, duration);
    for _ in 0..failed {
        metrics.observe_unpersisted_error(
            WorkerLoop::ConsignmentMaintenance,
            SafeErrorKind::CONSIGNMENT_AUTO_CLOSE_FAILED,
        );
    }
}
