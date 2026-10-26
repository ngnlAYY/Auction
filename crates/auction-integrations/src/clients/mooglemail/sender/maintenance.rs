//! 定期维护读取：compose 草稿快照、受控 discard、Sent 摘要与实收探查。
//!
//! 普通列表和详情不持有 compose 排他会话；只有草稿读取与 discard 需要窄互斥。
//! 观测本身不提升为投递成功；discard 只丢弃调用方已精确匹配到本地已知投递的草稿。

use super::*;
use chrono::{DateTime, Utc};
use tokio::time::{Instant, timeout_at};

/// compose 页当前状态的只读投影。
pub struct MoogleMailComposeSnapshot {
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    pub cod: Option<i64>,
    pub attachment_count: usize,
    /// 字典序排列的精确装备集合，不以附件计数代替资产身份。
    pub equipment_ids: Vec<String>,
    pub clean: bool,
}

impl MoogleMailComposeSnapshot {
    pub const fn is_clean(&self) -> bool {
        self.clean
    }
}

/// 受控 discard 的结果；已干净时不做任何远端写入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoogleMailDiscardOutcome {
    AlreadyClean,
    Discarded,
}

/// 入站邮件当前资产快照；take 恢复据此判断动作是否已生效。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailInboxAssets {
    pub sender: String,
    pub equipment_ids: Vec<String>,
    pub credits: i64,
    pub cod: i64,
}

/// Sent 列表摘要；`collected` 只有读取详情后才可知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSentListEntry {
    pub external_mail_id: String,
    pub recipient: String,
    pub subject: String,
    pub sent_at: DateTime<Utc>,
}

/// 单封 Sent 邮件的当前待领取状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSentObservation {
    pub external_mail_id: String,
    pub subject: String,
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub remaining_equipment_ids: Vec<String>,
    pub collected: bool,
    /// 详情正文的规范摘要；通知类恢复证明据此逐字比对，正文本身不返回。
    pub body_digest: [u8; 32],
}

impl MoogleMailSender {
    /// 只读读取 compose 页，返回草稿字段；解析失败按协议漂移处理。
    pub async fn inspect_compose(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailComposeSnapshot, MoogleMailError> {
        let compose = self.client.compose(world)?;
        let document = self.client.get(&compose, cookie).await?;
        let state = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await?;
        let clean = state.is_clean();
        let mut fields = state.message_fields();
        let mut equipment_ids = state.equipment_ids().to_vec();
        equipment_ids.sort_unstable();
        Ok(MoogleMailComposeSnapshot {
            recipient: std::mem::take(&mut fields.recipient),
            subject: std::mem::take(&mut fields.subject),
            body: std::mem::take(&mut fields.body),
            credits: state.credits(),
            cod: state.cod(),
            attachment_count: state.attachment_count(),
            equipment_ids,
            clean,
        })
    }

    /// 丢弃 compose 草稿并在 POST 后重新读取证明页面已干净。
    /// 只允许对已登记一次性尝试且精确匹配已知投递的草稿调用；失败只读核对，不自动重发 discard。
    pub async fn discard_compose(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        expected: &auction_application::MoogleMailboxComposeState,
    ) -> Result<MoogleMailDiscardOutcome, MoogleMailError> {
        let compose = self.client.compose(world)?;
        let document = self.client.get(&compose, cookie).await?;
        let state = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await?;
        if state.is_clean() {
            return Ok(MoogleMailDiscardOutcome::AlreadyClean);
        }
        let fields = state.message_fields();
        let mut equipment_ids = state.equipment_ids().iter().collect::<Vec<_>>();
        equipment_ids.sort_unstable();
        // 原快照与本次 POST 之间重新取得了 session；持锁重读后逐字段一致才有权丢弃。
        if expected.clean
            || fields.recipient != expected.recipient
            || fields.subject != expected.subject
            || fields.body != expected.body
            || state.credits() != expected.credits
            || state.cod() != expected.cod
            || state.attachment_count() != expected.attachment_count
            || !equipment_ids.into_iter().eq(expected.equipment_ids.iter())
        {
            return Err(protocol_drift(world));
        }
        let form =
            MoogleMailForm::discard(state.token(), &fields).map_err(|_| protocol_drift(world))?;
        self.client.post(&compose, cookie, form).await?;
        let document = self.client.get(&compose, cookie).await.map_err(discard_unknown)?;
        let after = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await
            .map_err(discard_unknown)?;
        if after.is_clean() {
            Ok(MoogleMailDiscardOutcome::Discarded)
        } else {
            Err(write_unknown(world, MoogleMailOperation::Discard))
        }
    }

    /// 有界扫描 Sent 列表，只返回摘要；逐封详情由调用方按需探查。
    pub async fn scan_sent_summaries(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<Vec<MoogleMailSentListEntry>, MoogleMailError> {
        self.scan_sent_summaries_with_freshness(cookie, world, false).await
    }

    /// 显式请求屏障后的完整 Sent 观测，供需要排除先前在途读取的恢复调用使用。
    pub async fn scan_sent_summaries_fresh(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<Vec<MoogleMailSentListEntry>, MoogleMailError> {
        self.scan_sent_summaries_with_freshness(cookie, world, true).await
    }

    async fn scan_sent_summaries_with_freshness(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        fresh: bool,
    ) -> Result<Vec<MoogleMailSentListEntry>, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        let round = self.client.read_round(cookie, world, fresh)?;
        let list = timeout_at(
            deadline,
            self.client.observe_sent_list(cookie, &round, self.verification_pages),
        )
        .await
        .map_err(|_| proof::scan_timeout(world))??;
        proof::check_deadline(deadline, world)?;
        if !list.complete {
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
        proof::check_deadline(deadline, world)?;
        Ok(summaries)
    }

    /// 已读历史列表与 Take/持久 pending 恢复共用数据；不得据此创建新入站命令。
    pub async fn scan_read_summaries(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<Vec<super::super::parser::MoogleMailSummary>, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        let round = self.client.read_round(cookie, world, false)?;
        let list = timeout_at(
            deadline,
            self.client.observe_incoming_list(cookie, &round, self.verification_pages, true),
        )
        .await
        .map_err(|_| proof::scan_timeout(world))??;
        proof::check_deadline(deadline, world)?;
        if !list.complete {
            return Err(MoogleMailError::protocol_drift(MoogleMailOperation::ReadMail, world));
        }
        let summaries = list.summaries.clone();
        proof::check_deadline(deadline, world)?;
        Ok(summaries)
    }

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
        .map_err(|_| proof::scan_timeout(world))?;
        proof::check_deadline(deadline, world)?;
        result
    }

    async fn inspect_inbox_attachments_in_round(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        external_mail_id: &str,
        round: &super::super::client::MoogleMailReadRound,
        summary: Option<&super::super::parser::MoogleMailSummary>,
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
            let parser_summary = super::super::parser::MoogleMailSentSummary {
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
                subject: proof.subject,
                remaining_credits: proof.remaining_credits,
                cod_pending: proof.cod_pending,
                remaining_equipment_ids: proof.remaining_equipment_ids,
                collected,
                body_digest: proof.body_digest,
            })
        })
        .await
        .map_err(|_| proof::scan_timeout(world))?;
        proof::check_deadline(deadline, world)?;
        result
    }
}

fn protocol_drift(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::protocol_drift(MoogleMailOperation::Compose, world)
}

/// POST 后的读取/解析失败仍属未知外写；保留已有加密诊断引用，不降级成可重试读错误。
fn discard_unknown(error: MoogleMailError) -> MoogleMailError {
    let category = if error.category == MoogleMailErrorCategory::SessionExpired {
        MoogleMailErrorCategory::SessionExpired
    } else {
        MoogleMailErrorCategory::WriteUnknown
    };
    let mut unknown =
        MoogleMailError::after_write(MoogleMailOperation::Discard, error.world, category);
    unknown.evidence_id = error.evidence_id;
    unknown.evidence_status = error.evidence_status;
    unknown.evidence_failure = error.evidence_failure;
    unknown
}
