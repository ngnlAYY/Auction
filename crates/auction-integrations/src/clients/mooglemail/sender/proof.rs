//! 完整 Sent 基线与新增邮件的只读身份、正文和资产证明。

use super::*;
use crate::clients::mooglemail::parser::{canonical_mail_body_digest, subject_is_list_projection};
use tokio::time::{Instant, timeout_at};

/// 同一独占凭据会话内取得的完整 Sent 基线；不可克隆，随 prepared 句柄一次消费。
/// 只有扫描到末页才能构造，不持久化、不跨会话缓存，也不把缺页当作空历史。
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
        let mut seen = HashSet::new();
        for page_index in 0..self.verification_pages {
            check_deadline(deadline, world)?;
            let target = self.client.sent_page(world, page_index)?;
            let document = timeout_at(deadline, self.client.get(&target, cookie))
                .await
                .map_err(|_| scan_timeout(world))??;
            let page = self.parser.scan_sent_page(&document, world, page_index, &mut seen)?;
            check_deadline(deadline, world)?;
            if !page.has_next_page {
                return Ok(MoogleMailSentBaseline { world, seen });
            }
        }
        // 未扫描到尾页不能排除同标题旧邮件；此时尚未执行任何 POST。
        Err(self.scan_page_limit(world, "baseline", seen.len()))
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
        let mut seen = HashSet::new();
        let mut verified: Option<String> = None;
        for page_index in 0..self.verification_pages {
            check_deadline(deadline, world)?;
            let target = self.client.sent_page(world, page_index)?;
            let document = timeout_at(deadline, self.client.get(&target, cookie))
                .await
                .map_err(|_| scan_timeout(world))??;
            let page = self.parser.scan_sent_page(&document, world, page_index, &mut seen)?;
            check_deadline(deadline, world)?;
            for summary in page.summaries.into_iter().filter(|summary| {
                !baseline.contains(&summary.external_mail_id)
                    && summary.world == world
                    && summary.recipient.eq_ignore_ascii_case(&request.message.recipient)
                    && subject_is_list_projection(&summary.subject, &request.message.subject)
            }) {
                let detail_target = self.client.sent_detail(world, &summary.external_mail_id)?;
                let detail = timeout_at(deadline, self.client.get(&detail_target, cookie))
                    .await
                    .map_err(|_| scan_timeout(world))??;
                let proof = self.parser.parse_sent_content_proof(&detail, &summary)?;
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
            if !page.has_next_page {
                check_deadline(deadline, world)?;
                return verified
                    .filter(|mail_id| response_mail_id.is_none_or(|hint| hint == mail_id))
                    .ok_or_else(|| verification_unavailable(world));
            }
        }
        Err(self.scan_page_limit(world, "after_send", seen.len()))
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

fn check_deadline(deadline: Instant, world: MoogleMailWorld) -> Result<(), MoogleMailError> {
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
