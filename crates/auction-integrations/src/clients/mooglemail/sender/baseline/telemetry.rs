//! 局部影子记录及取消收尾；记录内容不含远端身份，关闭 exporter 不改变判定。

use auction_application::core::diagnostics::{DiagnosticContext, SafeErrorKind, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use auction_application::sent_baseline::{
    SentBaselineEvent, SentBaselineObserver, SentBaselineReason,
};

use crate::clients::mooglemail::MoogleMailWorld;

pub(super) fn record(
    observer: &dyn SentBaselineObserver,
    world: MoogleMailWorld,
    event: SentBaselineEvent,
) {
    observer.observe(world.as_str(), event);
    match event {
        SentBaselineEvent::Evaluation { branch, mid_sets_equal, candidate_sets_equal } => {
            if mid_sets_equal && candidate_sets_equal {
                tracing::debug!(
                    world = world.as_str(),
                    branch = branch.label(),
                    mid_sets_equal,
                    candidate_sets_equal,
                    "MoogleMail Sent 基线影子集合一致"
                );
            } else {
                safe_diagnostics::warn(
                    "mooglemail_baseline",
                    "compare_shadow_sets",
                    safe_diagnostics::category(SafeErrorKind::SHADOW_MISMATCH),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
            }
        }
        SentBaselineEvent::Uncomparable(reason) => {
            safe_diagnostics::debug(
                "mooglemail_baseline",
                "compare_shadow_sample",
                safe_diagnostics::project(&reason),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }
        SentBaselineEvent::Fallback { phase, reason } => {
            let phase = match phase {
                auction_application::sent_baseline::SentBaselinePhase::BeforeSend => {
                    "fallback_before_send"
                }
                auction_application::sent_baseline::SentBaselinePhase::AfterSend => {
                    "fallback_after_send"
                }
            };
            safe_diagnostics::warn(
                "mooglemail_baseline",
                phase,
                safe_diagnostics::project(&reason),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }
    }
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
