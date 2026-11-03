//! 局部影子记录及取消收尾；记录内容不含远端身份，关闭 exporter 不改变判定。

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use auction_application::{SentBaselineEvent, SentBaselineObserver, SentBaselineReason};

use crate::clients::mooglemail::MoogleMailWorld;

pub(super) fn record(
    observer: &dyn SentBaselineObserver,
    world: MoogleMailWorld,
    event: SentBaselineEvent,
) {
    match event {
        SentBaselineEvent::Evaluation { branch, mid_sets_equal, candidate_sets_equal } => {
            if mid_sets_equal && candidate_sets_equal {
                tracing::debug!(%world, branch = branch.label(), mid_sets_equal, candidate_sets_equal, "MoogleMail Sent 基线影子集合一致");
            } else {
                tracing::warn!(%world, branch = branch.label(), mid_sets_equal, candidate_sets_equal, "MoogleMail Sent 基线影子集合存在非预期差异");
            }
        }
        SentBaselineEvent::Uncomparable(reason) => {
            tracing::debug!(%world, reason = reason.label(), "MoogleMail Sent 基线影子不可比较")
        }
        SentBaselineEvent::Fallback { phase, reason } => {
            tracing::warn!(%world, phase = phase.label(), reason = reason.label(), "MoogleMail Sent 边界回退完整扫描")
        }
    }
    observer.observe(world.as_str(), event);
}

pub(super) struct ShadowSample {
    observer: Arc<dyn SentBaselineObserver>,
    world: MoogleMailWorld,
    pending: AtomicBool,
}

impl ShadowSample {
    pub(super) fn new(observer: Arc<dyn SentBaselineObserver>, world: MoogleMailWorld) -> Self {
        Self { observer, world, pending: AtomicBool::new(true) }
    }

    pub(super) fn finish(&self, event: SentBaselineEvent) {
        if self.pending.swap(false, Ordering::AcqRel) {
            record(self.observer.as_ref(), self.world, event);
        }
    }
}

impl Drop for ShadowSample {
    fn drop(&mut self) {
        // prepared 未消费、拒绝、超时或 future 取消都不能伪装成一次一致样本。
        self.finish(SentBaselineEvent::Uncomparable(SentBaselineReason::Canceled));
    }
}
