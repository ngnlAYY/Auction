//! 基线之外候选的完整身份、正文与资产证明；候选枚举由 baseline owner 负责。

use auction_application::diagnostics::{DiagnosticContext, SafeErrorKind, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use super::*;
use crate::clients::mooglemail::parser::{canonical_mail_body_digest, subject_is_list_projection};
use auction_application::mail_recipient_is_list_projection;
use tokio::time::{Instant, timeout_at};

impl MoogleMailSender {
    pub(super) async fn verify_new_sent(
        &self,
        request: &MoogleMailSendRequest,
        cookie: &MoogleMailCookie,
        baseline: &MoogleMailSentBaseline,
        response_mail_id: Option<&str>,
    ) -> Result<String, MoogleMailError> {
        let world = request.world;
        self.check_response_hint(baseline, response_mail_id)?;
        if let Err(error) = self.check_baseline_scope(cookie, baseline) {
            self.shadow_uncomparable(
                baseline,
                auction_application::SentBaselineReason::ScopeChanged,
            );
            return Err(error);
        }
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| scan_timeout(world))?;
        // POST 完成/取消屏障之后建立 fresh 轮次，不能提升发送前或发送中的观测。
        let round = self.client.read_round(cookie, world, true)?;
        let candidates =
            match self.observe_new_sent(request, cookie, baseline, &round, deadline).await {
                Ok(candidates) => candidates,
                Err(error) => {
                    self.shadow_uncomparable(baseline, super::baseline::error_reason(&error));
                    return Err(error);
                }
            };
        let result = self
            .verify_candidates(request, cookie, &candidates, response_mail_id, &round, deadline)
            .await;
        // 旧证明先完成；影子超时或差异不能改写其裁决，也不占用另一份总预算。
        if result.as_ref().is_err_and(|error| error.is_session_expired()) {
            self.shadow_uncomparable(
                baseline,
                auction_application::SentBaselineReason::ReadUnavailable,
            );
        } else {
            self.compare_sent_shadow(request, cookie, baseline, &candidates, &round, deadline)
                .await;
        }
        let verified = result?;
        self.check_baseline_scope(cookie, baseline)?;
        if let Some(seen) = baseline.full_seen() {
            let mut next_baseline = seen.clone();
            next_baseline.extend(
                candidates.summaries().iter().map(|summary| summary.external_mail_id.clone()),
            );
            self.remember_sent_baseline(cookie, world, next_baseline);
        }
        Ok(verified)
    }

    async fn verify_candidates(
        &self,
        request: &MoogleMailSendRequest,
        cookie: &MoogleMailCookie,
        candidates: &super::baseline::SentCandidates,
        response_mail_id: Option<&str>,
        round: &super::super::client::MoogleMailReadRound,
        deadline: Instant,
    ) -> Result<String, MoogleMailError> {
        let world = request.world;
        let expected_body = canonical_mail_body_digest(&request.message.body);
        let mut expected_equipment = request.equipment_ids.iter().collect::<Vec<_>>();
        expected_equipment.sort_unstable();
        let mut verified: Option<String> = None;
        for summary in candidates.summaries().iter().filter(|summary| {
            candidates.mids.contains(&summary.external_mail_id)
                && is_list_candidate(summary, request)
        }) {
            check_deadline(deadline, world)?;
            let target = self.client.sent_detail(world, &summary.external_mail_id)?;
            let detail =
                within_deadline(world, deadline, self.client.get_in_round(&target, cookie, round))
                    .await?;
            let proof = within_deadline(
                world,
                deadline,
                self.client.diagnose_parse(
                    &target,
                    &detail,
                    self.parser.parse_sent_content_proof(&detail, summary),
                ),
            )
            .await?;
            if proof.body_digest != expected_body {
                if response_mail_id == Some(proof.external_mail_id.as_str()) {
                    return Err(verification_unavailable(world));
                }
                continue;
            }
            // 不变量 4、6：同正文的资产缺失可能来自立即领取，不能忽略后误认另一封成功。
            if proof.world != world
                || !proof.recipient.eq_ignore_ascii_case(&request.message.recipient)
                || proof.subject != request.message.subject
                || proof.remaining_credits != request.credits
                || proof.cod_pending != request.cod
                || !proof.remaining_equipment_ids.iter().eq(expected_equipment.iter().copied())
                || verified.is_some()
            {
                return Err(verification_unavailable(world));
            }
            verified = Some(proof.external_mail_id);
        }
        within_deadline(world, deadline, self.client.check_read_round(cookie, round)).await?;
        verified
            .filter(|mail_id| response_mail_id.is_none_or(|hint| hint == mail_id))
            .ok_or_else(|| verification_unavailable(world))
    }

    /// 只输出计数与闭集阶段；不把 MID、收件人、正文或凭据带入普通日志。
    pub(super) fn scan_page_limit(
        &self,
        world: MoogleMailWorld,
        phase: &'static str,
        _observed_messages: usize,
    ) -> MoogleMailError {
        safe_diagnostics::warn(
            "mooglemail_baseline",
            match phase {
                "baseline" => "scan_full_baseline",
                "maintenance" => "scan_maintenance",
                "after_send" => "scan_sent_prefix",
                "after_send_boundary" => "scan_sent_boundary",
                _ => "scan_sent_proof",
            },
            safe_diagnostics::category(SafeErrorKind::SENT_SCAN_PAGE_LIMIT),
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        MoogleMailError::new(
            MoogleMailOperation::Sent,
            world,
            MoogleMailErrorCategory::SentScanPageLimit,
            true,
        )
    }
}

pub(super) fn check_deadline(
    deadline: Instant,
    world: MoogleMailWorld,
) -> Result<(), MoogleMailError> {
    if Instant::now() >= deadline { Err(scan_timeout(world)) } else { Ok(()) }
}

pub(super) fn scan_timeout(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Sent,
        world,
        MoogleMailErrorCategory::SentScanTimeout,
        true,
    )
}

fn verification_unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Sent,
        world,
        MoogleMailErrorCategory::RemoteTemporary,
        false,
    )
}

/// 列表投影只筛选候选；最终收件人、主题与资产仍必须由详情精确证明。
pub(super) fn is_list_candidate(
    summary: &super::super::MoogleMailSentSummary,
    request: &MoogleMailSendRequest,
) -> bool {
    summary.world == request.world
        && mail_recipient_is_list_projection(&summary.recipient, &request.message.recipient)
        && subject_is_list_projection(&summary.subject, &request.message.subject)
}

/// 各次回退共用调用方原截止点；过期时不派发新的只读请求。
pub(super) async fn within_deadline<T>(
    world: MoogleMailWorld,
    deadline: Instant,
    future: impl std::future::Future<Output = Result<T, MoogleMailError>>,
) -> Result<T, MoogleMailError> {
    check_deadline(deadline, world)?;
    let result = timeout_at(deadline, future).await.map_err(|elapsed| {
        tracing::trace!(%world, timeout = %elapsed, "MoogleMail Sent 只读预算耗尽");
        scan_timeout(world)
    })??;
    check_deadline(deadline, world)?;
    Ok(result)
}
