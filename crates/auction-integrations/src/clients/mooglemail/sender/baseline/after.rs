//! 写后只读候选枚举；旧路径裁决与影子比较分离，边界失效不能补造发送前历史。

use std::{collections::HashSet, sync::Arc};

use auction_application::{
    SentBaselineBranch, SentBaselineEvent, SentBaselinePhase, SentBaselineReason,
};
use tokio::time::Instant;

use super::{MoogleMailSentBaseline, SentBaselineKind, error_reason, unavailable};
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailSendRequest, MoogleMailSender,
    MoogleMailSentSummary,
    client::{
        MoogleMailReadRound, MoogleMailSentList, MoogleMailSentPrefix, SentBoundary,
        SentBoundaryScan,
    },
    sender::proof::{is_list_candidate, within_deadline},
};

enum SentObserved {
    Legacy(Arc<MoogleMailSentPrefix>),
    Boundary(SentBoundaryScan),
    Complete(Arc<MoogleMailSentList>),
}

pub(in crate::clients::mooglemail::sender) struct SentCandidates {
    observed: SentObserved,
    pub(in crate::clients::mooglemail::sender) mids: HashSet<String>,
}

impl SentCandidates {
    pub(in crate::clients::mooglemail::sender) fn summaries(&self) -> &[MoogleMailSentSummary] {
        match &self.observed {
            SentObserved::Legacy(list) => &list.summaries,
            SentObserved::Boundary(list) => &list.summaries,
            SentObserved::Complete(list) => &list.summaries,
        }
    }

    pub(super) fn candidate_ids(&self, request: &MoogleMailSendRequest) -> HashSet<&str> {
        self.summaries()
            .iter()
            .filter(|summary| {
                self.mids.contains(&summary.external_mail_id) && is_list_candidate(summary, request)
            })
            .map(|summary| summary.external_mail_id.as_str())
            .collect()
    }

    pub(super) fn branch(
        &self,
        boundary: &SentBoundary,
        request: &MoogleMailSendRequest,
    ) -> SentBaselineBranch {
        let candidates = self.candidate_ids(request);
        if candidates.is_empty() {
            return SentBaselineBranch::NoCandidate;
        }
        if candidates.len() > 1 {
            return SentBaselineBranch::MultipleCandidates;
        }
        if matches!(self.observed, SentObserved::Complete(_)) {
            return SentBaselineBranch::FullFallback;
        }
        if matches!(boundary, SentBoundary::Empty) {
            return SentBaselineBranch::Empty;
        }
        let same_minute = self.summaries().iter().any(|summary| {
            self.mids.contains(&summary.external_mail_id)
                && Some(summary.sent_at) == boundary.sent_at()
        });
        match (&self.observed, same_minute) {
            (SentObserved::Boundary(scan), true) if scan.pages_scanned > 1 => {
                SentBaselineBranch::SameMinuteCrossPage
            }
            (_, true) => SentBaselineBranch::SameMinute,
            _ => SentBaselineBranch::NewMinute,
        }
    }
}

impl MoogleMailSender {
    pub(in crate::clients::mooglemail::sender) async fn observe_new_sent(
        &self,
        request: &MoogleMailSendRequest,
        cookie: &MoogleMailCookie,
        baseline: &MoogleMailSentBaseline,
        round: &MoogleMailReadRound,
        deadline: Instant,
    ) -> Result<SentCandidates, MoogleMailError> {
        match &baseline.kind {
            SentBaselineKind::Full(seen) | SentBaselineKind::Shadow { seen, .. } => {
                let list = within_deadline(
                    request.world,
                    deadline,
                    self.client.observe_sent_prefix(cookie, round, self.verification_pages, seen),
                )
                .await?;
                if !list.bounded {
                    return Err(self.scan_page_limit(
                        request.world,
                        "after_send",
                        list.summaries.len(),
                    ));
                }
                let mids = list
                    .summaries
                    .iter()
                    .filter(|summary| !seen.contains(&summary.external_mail_id))
                    .map(|summary| summary.external_mail_id.clone())
                    .collect();
                Ok(SentCandidates { observed: SentObserved::Legacy(list), mids })
            }
            SentBaselineKind::Watermark(boundary) => self
                .watermark_candidates(request, cookie, round, boundary, deadline)
                .await
                .map_err(|failure| failure.error),
        }
    }

    pub(super) async fn watermark_candidates(
        &self,
        request: &MoogleMailSendRequest,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        boundary: &SentBoundary,
        deadline: Instant,
    ) -> Result<SentCandidates, WatermarkFailure> {
        let world = request.world;
        let scan = within_deadline(
            world,
            deadline,
            self.client.observe_sent_boundary(
                cookie,
                round,
                self.verification_pages,
                Some(boundary),
                deadline,
            ),
        )
        .await
        .map_err(WatermarkFailure::read)?;
        let observed = match scan.check_after(boundary) {
            Ok(()) => SentObserved::Boundary(scan),
            Err(reason) => {
                if !matches!(
                    reason,
                    SentBaselineReason::AnchorMissing
                        | SentBaselineReason::PageLimit
                        | SentBaselineReason::PageDiscontinuity
                ) {
                    // 不变量 4、6：无发送前完整集合时，MID/时间前提失效不可事后恢复，保持写入未知。
                    return Err(WatermarkFailure::premise(world, reason));
                }
                drop(scan);
                self.baseline_event(
                    world,
                    SentBaselineEvent::Fallback { phase: SentBaselinePhase::AfterSend, reason },
                );
                let list = within_deadline(
                    world,
                    deadline,
                    self.client.observe_sent_list(cookie, round, self.verification_pages),
                )
                .await
                .map_err(WatermarkFailure::read)?;
                if !list.complete {
                    return Err(WatermarkFailure::read(self.scan_page_limit(
                        world,
                        "after_send",
                        list.summaries.len(),
                    )));
                }
                if let Err(reason) = boundary.check_complete(&list.summaries) {
                    return Err(WatermarkFailure::premise(world, reason));
                }
                SentObserved::Complete(list)
            }
        };
        let mut result = SentCandidates { observed, mids: HashSet::new() };
        let mut mids = HashSet::new();
        for summary in result.summaries() {
            if boundary
                .is_new(&summary.external_mail_id)
                .map_err(|reason| WatermarkFailure::premise(world, reason))?
            {
                mids.insert(summary.external_mail_id.clone());
            }
        }
        result.mids = mids;
        Ok(result)
    }
}

/// 保留闭集原因供影子记录；实际裁决仍使用原有错误与写入未知边界。
pub(super) struct WatermarkFailure {
    pub(super) reason: SentBaselineReason,
    pub(super) error: MoogleMailError,
}

impl WatermarkFailure {
    fn read(error: MoogleMailError) -> Self {
        Self { reason: error_reason(&error), error }
    }

    fn premise(
        world: crate::clients::mooglemail::MoogleMailWorld,
        reason: SentBaselineReason,
    ) -> Self {
        tracing::warn!(%world, phase = "after_send", reason = reason.label(), "MoogleMail 发送前边界前提失效，无法事后重建旧集合");
        Self { reason, error: unavailable(world) }
    }
}
