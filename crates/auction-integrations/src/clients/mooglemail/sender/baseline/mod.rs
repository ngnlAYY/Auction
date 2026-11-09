//! 发送前基线与只读候选选择；临时边界不推进持久账本，不替代详情和资产证明。

mod after;
mod prepare;
mod shadow;
mod telemetry;

use std::{collections::HashSet, sync::Arc};

use auction_application::sent_baseline::{
    SentBaselineMode, SentBaselineObserver, SentBaselineReason,
};

use super::MoogleMailSender;
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailWorld,
    client::{MoogleMailSentScope, SentBoundary},
};

pub(super) use after::SentCandidates;
use telemetry::ShadowSample;

/// 同一独占凭据会话取得的临时上下文；不可克隆，随 prepared 句柄一次消费。
/// Full 来自完整枚举或其有效缓存；Watermark 只覆盖 fresh 最新分钟组，不持久化。
pub(crate) struct MoogleMailSentBaseline {
    pub(super) world: MoogleMailWorld,
    scope: MoogleMailSentScope,
    kind: SentBaselineKind,
}

enum SentBaselineKind {
    Full(HashSet<String>),
    Shadow { seen: HashSet<String>, boundary: Option<SentBoundary>, sample: ShadowSample },
    Watermark(SentBoundary),
}

impl MoogleMailSentBaseline {
    pub(super) fn full_seen(&self) -> Option<&HashSet<String>> {
        match &self.kind {
            SentBaselineKind::Full(seen) | SentBaselineKind::Shadow { seen, .. } => Some(seen),
            SentBaselineKind::Watermark(_) => None,
        }
    }
}

impl MoogleMailSender {
    /// 只由装配根注入；模式不参与 W7.7 在线开关，prepared 上下文保留捕获时的路径。
    pub fn with_sent_baseline(
        mut self,
        mode: SentBaselineMode,
        observer: Arc<dyn SentBaselineObserver>,
    ) -> Self {
        self.sent_baseline_mode = mode;
        self.sent_baseline_observer = observer;
        self
    }

    /// 允许本次写入推进读取修订，但禁止换凭据、world、endpoint 或 owner 代次。
    pub(super) fn check_baseline_scope(
        &self,
        cookie: &MoogleMailCookie,
        baseline: &MoogleMailSentBaseline,
    ) -> Result<(), MoogleMailError> {
        self.client.check_sent_scope(cookie, &baseline.scope)
    }

    fn baseline_event(
        &self,
        world: MoogleMailWorld,
        event: auction_application::sent_baseline::SentBaselineEvent,
    ) {
        telemetry::record(self.sent_baseline_observer.as_ref(), world, event);
    }
}

fn unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        crate::clients::mooglemail::MoogleMailOperation::Sent,
        world,
        crate::clients::mooglemail::MoogleMailErrorCategory::RemoteTemporary,
        false,
    )
}

pub(super) fn error_reason(error: &MoogleMailError) -> SentBaselineReason {
    use crate::clients::mooglemail::MoogleMailErrorCategory;
    match error.category {
        MoogleMailErrorCategory::SentScanTimeout => SentBaselineReason::Timeout,
        MoogleMailErrorCategory::SentScanPageLimit => SentBaselineReason::PageLimit,
        _ => SentBaselineReason::ReadUnavailable,
    }
}

#[cfg(test)]
mod tests;
