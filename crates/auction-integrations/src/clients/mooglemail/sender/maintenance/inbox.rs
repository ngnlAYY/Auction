//! 入站邮件当前资产快照与只读探查；take 恢复据此判断动作是否已生效。

use std::fmt;

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use tokio::time::{Instant, timeout_at};

use super::super::{MoogleMailSender, proof};
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailOperation, MoogleMailWorld,
    client::MoogleMailReadRound, parser::MoogleMailSummary,
};
use crate::safe_diagnostics;

/// 入站邮件当前资产快照；take 恢复据此判断动作是否已生效。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailInboxAssets {
    pub sender: String,
    pub equipment_ids: Vec<String>,
    pub credits: i64,
    pub cod: i64,
}

impl fmt::Debug for MoogleMailInboxAssets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailInboxAssets")
            .field("sender", &"[REDACTED]")
            .field("equipment_ids", &self.equipment_ids)
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .finish()
    }
}

impl MoogleMailSender {
    /// 只读确认入站邮件是否仍存在并返回当前附件 ID。
    ///
    /// 先在完整 Inbox/Read Mail 观测中定位既有 MID，再读取详情；历史其他邮件不进入业务。
    /// 两表都缺失仍不能证明 Take，必须由远端明确的 “Mail does not exist” 页面证明；
    /// 登录页或解析漂移一律返回错误，不能收口 take 动作。
    pub async fn inspect_inbox_attachments(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        external_mail_id: &str,
    ) -> Result<Option<MoogleMailInboxAssets>, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        let round = self.client.read_round(cookie, world, false)?;
        let result = timeout_at(deadline, async {
            let inbox = self
                .client
                .observe_incoming_list(cookie, &round, self.verification_pages, false)
                .await?;
            proof::check_deadline(deadline, world)?;
            if !inbox.complete {
                self.observe_budget(
                    world,
                    auction_application::external_metrics::MailPhase::SharedRead,
                    auction_application::external_metrics::MailBudget::Pages,
                );
                return Err(MoogleMailError::protocol_drift(MoogleMailOperation::Inbox, world));
            }
            let summary =
                inbox.summaries.iter().find(|summary| summary.external_mail_id == external_mail_id);
            if let Some(summary) = summary {
                return self
                    .inspect_inbox_attachments_in_round(
                        cookie,
                        world,
                        external_mail_id,
                        &round,
                        Some(summary),
                    )
                    .await;
            }
            let read = self
                .client
                .observe_incoming_list(cookie, &round, self.verification_pages, true)
                .await?;
            proof::check_deadline(deadline, world)?;
            if !read.complete {
                self.observe_budget(
                    world,
                    auction_application::external_metrics::MailPhase::SharedRead,
                    auction_application::external_metrics::MailBudget::Pages,
                );
                return Err(MoogleMailError::protocol_drift(MoogleMailOperation::ReadMail, world));
            }
            let summary =
                read.summaries.iter().find(|summary| summary.external_mail_id == external_mail_id);
            self.inspect_inbox_attachments_in_round(
                cookie,
                world,
                external_mail_id,
                &round,
                summary,
            )
            .await
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "read_inbox_assets_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            self.observe_budget(
                world,
                auction_application::external_metrics::MailPhase::SharedRead,
                auction_application::external_metrics::MailBudget::Time,
            );
            proof::scan_timeout(world)
        })?;
        if let Err(error) = proof::check_deadline(deadline, world) {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::SharedRead,
                &error,
            );
            return Err(error);
        }
        result
    }

    async fn inspect_inbox_attachments_in_round(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        external_mail_id: &str,
        round: &MoogleMailReadRound,
        summary: Option<&MoogleMailSummary>,
    ) -> Result<Option<MoogleMailInboxAssets>, MoogleMailError> {
        let target = self.client.detail(world, external_mail_id)?;
        let document = self.client.get_in_round(&target, cookie, round).await?;
        let state = self
            .client
            .diagnose_parse(
                &target,
                &document,
                self.parser.parse_action_state_or_missing(&document, world),
            )
            .await?;
        let Some(state) = state else {
            self.client.check_read_round(cookie, round).await?;
            return Ok(None);
        };
        if let Some(summary) = summary {
            // 历史摘要仅定位原 MID；仍执行完整详情与装备 key 校验，不从截断显示名重建身份。
            let mut message = self
                .client
                .diagnose_parse(&target, &document, self.parser.parse_detail(&document, summary))
                .await?;
            message.clear_secrets();
        }
        let sender = state.sender().to_owned();
        let mut equipment_ids = state.equipment_ids;
        equipment_ids.sort_unstable();
        self.client.check_read_round(cookie, round).await?;
        Ok(Some(MoogleMailInboxAssets {
            sender,
            equipment_ids,
            credits: state.credits,
            cod: state.cod,
        }))
    }
}
