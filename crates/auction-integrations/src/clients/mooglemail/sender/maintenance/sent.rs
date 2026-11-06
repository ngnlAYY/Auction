//! Sent 列表摘要的有界扫描与单封实收状态读取；观测本身不证明投递成功。

use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailWorld, parser::MoogleMailSentSummary,
};
use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use super::super::{MoogleMailSender, proof};
use chrono::{DateTime, Utc};
use std::fmt;
use tokio::time::{Instant, timeout_at};

/// Sent 列表摘要；`collected` 只有读取详情后才可知。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSentListEntry {
    pub external_mail_id: String,
    pub recipient: String,
    pub subject: String,
    pub sent_at: DateTime<Utc>,
}

impl fmt::Debug for MoogleMailSentListEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSentListEntry")
            .field("external_mail_id", &self.external_mail_id)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("sent_at", &self.sent_at)
            .finish()
    }
}

/// 单封 Sent 邮件的当前待领取状态。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSentObservation {
    pub external_mail_id: String,
    /// 详情页完整收件人；列表投影不能作为最终身份。
    pub recipient: String,
    pub subject: String,
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub remaining_equipment_ids: Vec<String>,
    pub collected: bool,
    /// 详情正文的规范摘要；通知类恢复证明据此逐字比对，正文本身不返回。
    pub body_digest: [u8; 32],
}

impl fmt::Debug for MoogleMailSentObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSentObservation")
            .field("external_mail_id", &self.external_mail_id)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("remaining_credits", &self.remaining_credits)
            .field("cod_pending", &self.cod_pending)
            .field("remaining_equipment_ids", &self.remaining_equipment_ids)
            .field("collected", &self.collected)
            .field("body_digest", &self.body_digest)
            .finish()
    }
}

impl MoogleMailSender {
    /// 有界扫描 Sent 列表，只返回摘要；逐封详情由调用方按需探查。
    pub async fn scan_sent_summaries(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<Vec<MoogleMailSentListEntry>, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        // 维护扫描允许复用共享读取轮次，不强制新的请求屏障。
        let round = self.client.read_round(cookie, world, false)?;
        let list = timeout_at(
            deadline,
            self.client.observe_sent_list(cookie, &round, self.verification_pages),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "read_sent_list_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            self.observe_budget(
                world,
                auction_application::external_metrics::MailPhase::SentScan,
                auction_application::external_metrics::MailBudget::Time,
            );
            proof::scan_timeout(world)
        })??;
        if let Err(error) = proof::check_deadline(deadline, world) {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::SentScan,
                &error,
            );
            return Err(error);
        }
        if !list.complete {
            self.observe_budget(
                world,
                auction_application::external_metrics::MailPhase::SentScan,
                auction_application::external_metrics::MailBudget::Pages,
            );
            return Err(self.scan_page_limit(world, "maintenance", list.summaries.len()));
        }
        let summaries = list
            .summaries
            .iter()
            .map(|summary| MoogleMailSentListEntry {
                external_mail_id: summary.external_mail_id.clone(),
                recipient: summary.recipient.clone(),
                subject: summary.subject.clone(),
                sent_at: summary.sent_at,
            })
            .collect();
        if let Err(error) = proof::check_deadline(deadline, world) {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::SentScan,
                &error,
            );
            return Err(error);
        }
        // 维护仍需要完整 Sent 才能判断候选邮件缺席；只把成功枚举登记为水位与每日校验。
        self.observe_scan(
            world,
            auction_application::mail::MailScanEvent::Verification {
                stream: auction_application::mail::MailScanStream::Sent,
                outcome: auction_application::mail::MailScanOutcome::Complete,
                reason: auction_application::mail::MailScanReason::RequestedFull,
            },
        );
        self.remember_sent_scan(
            cookie,
            world,
            list.summaries.first().map(|summary| auction_application::mail::MailScanWatermark {
                mail_id: summary.external_mail_id.clone(),
                observed_at: summary.sent_at,
            }),
            true,
        )
        .await;
        Ok(summaries)
    }

    /// 读取单封 Sent 的实收状态；读取失败由调用方保留上一轮证据。
    pub async fn inspect_sent(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        summary: &MoogleMailSentListEntry,
        _observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailSentObservation, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        let result = timeout_at(deadline, async {
            let round = self.client.read_round(cookie, world, false)?;
            let detail = self.client.sent_detail(world, &summary.external_mail_id)?;
            let document = self.client.get_in_round(&detail, cookie, &round).await?;
            let parser_summary = MoogleMailSentSummary {
                external_mail_id: summary.external_mail_id.clone(),
                world,
                recipient: summary.recipient.clone(),
                subject: summary.subject.clone(),
                sent_at: summary.sent_at,
            };
            let proof = self
                .client
                .diagnose_parse(
                    &detail,
                    &document,
                    self.parser.parse_sent_content_proof(&document, &parser_summary),
                )
                .await?;
            let collected = proof.remaining_credits == 0
                && proof.cod_pending == 0
                && proof.remaining_equipment_ids.is_empty();
            self.client.check_read_round(cookie, &round).await?;
            Ok(MoogleMailSentObservation {
                external_mail_id: proof.external_mail_id,
                recipient: proof.recipient,
                subject: proof.subject,
                remaining_credits: proof.remaining_credits,
                cod_pending: proof.cod_pending,
                remaining_equipment_ids: proof.remaining_equipment_ids,
                collected,
                body_digest: proof.body_digest,
            })
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "read_sent_state_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            proof::scan_timeout(world)
        })?;
        proof::check_deadline(deadline, world)?;
        result
    }
}
