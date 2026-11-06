//! 同轮次 MID 与列表候选集合影子比较；不执行 POST，也不改写旧证明裁决。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use auction_application::sent_baseline::{SentBaselineEvent, SentBaselineReason};
use tokio::time::Instant;

use super::{MoogleMailSentBaseline, SentBaselineKind, after::SentCandidates, unavailable};
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailSendRequest, MoogleMailSender,
    client::MoogleMailReadRound,
};

impl MoogleMailSender {
    pub(in crate::clients::mooglemail::sender) fn check_response_hint(
        &self,
        baseline: &MoogleMailSentBaseline,
        response_mail_id: Option<&str>,
    ) -> Result<(), MoogleMailError> {
        let Some(mail_id) = response_mail_id else {
            return Ok(());
        };
        let valid = match &baseline.kind {
            SentBaselineKind::Full(seen) | SentBaselineKind::Shadow { seen, .. } => {
                !seen.contains(mail_id)
            }
            SentBaselineKind::Watermark(boundary) => boundary.is_new(mail_id) == Ok(true),
        };
        if !valid {
            self.shadow_uncomparable(baseline, SentBaselineReason::ResponseHintConflict);
            if matches!(baseline.kind, SentBaselineKind::Watermark(_)) {
                safe_diagnostics::warn(
                    "mooglemail_baseline",
                    "validate_response_hint",
                    safe_diagnostics::project(&SentBaselineReason::ResponseHintConflict),
                    DiagnosticContext {
                        world: diagnostic_world(baseline.world.as_str()),
                        ..Default::default()
                    },
                );
            }
            return Err(unavailable(baseline.world));
        }
        Ok(())
    }

    pub(in crate::clients::mooglemail::sender) fn shadow_uncomparable(
        &self,
        baseline: &MoogleMailSentBaseline,
        reason: SentBaselineReason,
    ) {
        if let SentBaselineKind::Shadow { sample, .. } = &baseline.kind {
            sample.finish(SentBaselineEvent::Uncomparable(reason));
        }
    }

    pub(in crate::clients::mooglemail::sender) async fn compare_sent_shadow(
        &self,
        request: &MoogleMailSendRequest,
        cookie: &MoogleMailCookie,
        baseline: &MoogleMailSentBaseline,
        legacy: &SentCandidates,
        round: &MoogleMailReadRound,
        deadline: Instant,
    ) {
        let SentBaselineKind::Shadow { boundary: Some(boundary), sample, .. } = &baseline.kind
        else {
            return;
        };
        let shadow =
            match self.watermark_candidates(request, cookie, round, boundary, deadline).await {
                Ok(shadow) => shadow,
                Err(error) => {
                    sample.finish(SentBaselineEvent::Uncomparable(error.reason));
                    return;
                }
            };
        let old_candidates = legacy.candidate_ids(request);
        let new_candidates = shadow.candidate_ids(request);
        let mid_sets_equal = legacy.mids == shadow.mids;
        let candidate_sets_equal = old_candidates == new_candidates;
        if !mid_sets_equal || !candidate_sets_equal {
            tracing::debug!(
                world = %request.world,
                legacy_only_mids = legacy.mids.difference(&shadow.mids).count(),
                watermark_only_mids = shadow.mids.difference(&legacy.mids).count(),
                legacy_only_candidates = old_candidates.difference(&new_candidates).count(),
                watermark_only_candidates = new_candidates.difference(&old_candidates).count(),
                "MoogleMail Sent 基线影子差集计数"
            );
        }
        sample.finish(SentBaselineEvent::Evaluation {
            branch: shadow.branch(boundary, request),
            mid_sets_equal,
            candidate_sets_equal,
        });
    }
}
