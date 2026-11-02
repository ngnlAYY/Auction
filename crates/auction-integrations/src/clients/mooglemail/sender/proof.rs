//! 完整 Sent 基线与新增邮件的只读身份、正文和资产证明。

use super::*;
use crate::clients::mooglemail::parser::{canonical_mail_body_digest, subject_is_list_projection};
use auction_application::mail_recipient_is_list_projection;
use tokio::time::{Instant, timeout_at};

/// 同一独占凭据会话内取得的完整 Sent 基线；不可克隆，随 prepared 句柄一次消费。
/// 只有本次 fresh 轮次扫描到末页才能构造；不持久化，也不把缺页当作空历史。
pub(crate) struct MoogleMailSentBaseline {
    pub(super) world: MoogleMailWorld,
    pub(super) seen: HashSet<String>,
}

impl MoogleMailSender {
    /// 在持久 sending fence 前完成全部只读基线扫描，由调用方一直持有原 session。
    pub(crate) async fn prepare_sent_baseline(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailSentBaseline, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| scan_timeout(world))?;
        let round = self.client.read_round(cookie, world, true)?;
        check_deadline(deadline, world)?;
        let list = timeout_at(
            deadline,
            self.client.observe_sent_list(cookie, &round, self.verification_pages),
        )
        .await
        .map_err(|_| scan_timeout(world))??;
        check_deadline(deadline, world)?;
        if !list.complete {
            // 未扫描到尾页不能排除同标题旧邮件；此时尚未执行任何 POST。
            return Err(self.scan_page_limit(world, "baseline", list.summaries.len()));
        }
        let seen = list.summaries.iter().map(|summary| summary.external_mail_id.clone()).collect();
        check_deadline(deadline, world)?;
        Ok(MoogleMailSentBaseline { world, seen })
    }

    pub(super) async fn verify_new_sent(
        &self,
        request: &MoogleMailSendRequest,
        cookie: &MoogleMailCookie,
        baseline: &HashSet<String>,
        response_mail_id: Option<&str>,
    ) -> Result<String, MoogleMailError> {
        let world = request.world;
        if response_mail_id.is_some_and(|mail_id| baseline.contains(mail_id)) {
            return Err(verification_unavailable(world));
        }
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| scan_timeout(world))?;
        let expected_body = canonical_mail_body_digest(&request.message.body);
        let mut expected_equipment = request.equipment_ids.iter().collect::<Vec<_>>();
        expected_equipment.sort_unstable();
        // POST 的完成/取消屏障之外再建立 fresh 轮次，绝不接受发送前或发送中的观测。
        let round = self.client.read_round(cookie, world, true)?;
        check_deadline(deadline, world)?;
        let list = timeout_at(
            deadline,
            self.client.observe_sent_list(cookie, &round, self.verification_pages),
        )
        .await
        .map_err(|_| scan_timeout(world))??;
        check_deadline(deadline, world)?;
        if !list.complete {
            return Err(self.scan_page_limit(world, "after_send", list.summaries.len()));
        }
        let mut verified: Option<String> = None;
        for summary in list.summaries.iter().filter(|summary| {
            !baseline.contains(&summary.external_mail_id)
                && summary.world == world
                && mail_recipient_is_list_projection(&summary.recipient, &request.message.recipient)
                && subject_is_list_projection(&summary.subject, &request.message.subject)
        }) {
            // 列表解析或候选过滤可能耗尽预算；过期 timeout 的首次 poll 也不能派发新详情读取。
            check_deadline(deadline, world)?;
            let target = self.client.sent_detail(world, &summary.external_mail_id)?;
            let detail = timeout_at(deadline, self.client.get_in_round(&target, cookie, &round))
                .await
                .map_err(|_| scan_timeout(world))??;
            check_deadline(deadline, world)?;
            let proof = timeout_at(
                deadline,
                self.client.diagnose_parse(
                    &target,
                    &detail,
                    self.parser.parse_sent_content_proof(&detail, summary),
                ),
            )
            .await
            .map_err(|_| scan_timeout(world))??;
            check_deadline(deadline, world)?;
            if proof.body_digest != expected_body {
                if response_mail_id == Some(proof.external_mail_id.as_str()) {
                    return Err(verification_unavailable(world));
                }
                continue;
            }
            // 同一正文的资产缺失可能来自立即领取，不能忽略后误认另一封为本次成功。
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
        check_deadline(deadline, world)?;
        timeout_at(deadline, self.client.check_read_round(cookie, &round))
            .await
            .map_err(|_| scan_timeout(world))??;
        check_deadline(deadline, world)?;
        verified
            .filter(|mail_id| response_mail_id.is_none_or(|hint| hint == mail_id))
            .ok_or_else(|| verification_unavailable(world))
    }

    /// 只输出计数与闭集阶段；不把 MID、收件人、正文或凭据带入普通日志。
    pub(super) fn scan_page_limit(
        &self,
        world: MoogleMailWorld,
        phase: &'static str,
        observed_messages: usize,
    ) -> MoogleMailError {
        tracing::warn!(
            %world,
            phase,
            maximum_pages = self.verification_pages,
            observed_messages,
            failure_code = "sent_scan_page_limit",
            "MoogleMail Sent scan reached page limit before final page"
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
