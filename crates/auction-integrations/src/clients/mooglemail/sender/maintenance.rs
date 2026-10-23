//! 定期维护读取：compose 草稿快照、受控 discard、Sent 摘要与实收探查。
//!
//! 这三个读取只在持有同一凭据排他会话时调用；它们不参与发送证明，也不把读取结果直接
//! 提升为投递成功。discard 只丢弃调用方已精确匹配到本地已知投递的草稿。

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
        let fields = state.message_fields();
        Ok(MoogleMailComposeSnapshot {
            recipient: fields.recipient.clone(),
            subject: fields.subject.clone(),
            body: fields.body.clone(),
            credits: state.credits(),
            cod: state.cod(),
            attachment_count: state.attachment_count(),
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
        // 原快照与本次 POST 之间重新取得了 session；持锁重读后逐字段一致才有权丢弃。
        if expected.clean
            || fields.recipient != expected.recipient
            || fields.subject != expected.subject
            || fields.body != expected.body
            || state.credits() != expected.credits
            || state.cod() != expected.cod
            || state.attachment_count() != expected.attachment_count
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
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        let mut seen = HashSet::new();
        let mut summaries = Vec::new();
        for page_index in 0..self.verification_pages {
            if Instant::now() >= deadline {
                return Err(proof::scan_timeout(world));
            }
            let target = self.client.sent_page(world, page_index)?;
            let document = timeout_at(deadline, self.client.get(&target, cookie))
                .await
                .map_err(|_| proof::scan_timeout(world))??;
            let page = self.parser.scan_sent_page(&document, world, page_index, &mut seen)?;
            summaries.extend(page.summaries.into_iter().map(|summary| MoogleMailSentListEntry {
                external_mail_id: summary.external_mail_id,
                recipient: summary.recipient,
                subject: summary.subject,
                sent_at: summary.sent_at,
            }));
            if !page.has_next_page {
                return Ok(summaries);
            }
        }
        Err(self.scan_page_limit(world, "maintenance", seen.len()))
    }

    /// 只读确认入站邮件是否仍存在并返回当前附件 ID。
    ///
    /// 邮件详情直接按 MID 读取：Inbox 列表只展示未读邮件，扫描列表会把“已读但未领取”
    /// 误判为不存在。缺失必须由远端明确的 “Mail does not exist” 页面证明，登录页或解析
    /// 漂移一律返回错误，不能收口 take 动作。
    pub async fn inspect_inbox_attachments(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        external_mail_id: &str,
    ) -> Result<Option<MoogleMailInboxAssets>, MoogleMailError> {
        let target = self.client.detail(world, external_mail_id)?;
        let document = self.client.get(&target, cookie).await?;
        let source = std::str::from_utf8(&document).map_err(|_| unavailable(world))?;
        if source.trim().is_empty() {
            return Err(unavailable(world));
        }
        let html = scraper::Html::parse_document(source);
        let envelope_selector =
            super::super::parser::selector("#mmail_outer", MoogleMailOperation::Detail, world)?;
        let form_selector =
            super::super::parser::selector("form#mailform", MoogleMailOperation::Detail, world)?;
        let panes_selector = super::super::parser::selector(
            "#mmail_attachpanes",
            MoogleMailOperation::Detail,
            world,
        )?;
        let present = html.select(&envelope_selector).count() == 1
            && html.select(&form_selector).count() == 1
            && html.select(&panes_selector).count() == 1;
        if !present {
            if super::super::parser::has_result_message(&html, world)? {
                let selector = super::super::parser::selector(
                    "#messagebox_inner",
                    MoogleMailOperation::Detail,
                    world,
                )?;
                let text = html
                    .select(&selector)
                    .flat_map(|item| item.text())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if text.contains("does not exist") {
                    return Ok(None);
                }
            }
            return Err(unavailable(world));
        }
        // scraper 的 DOM 不是 Send，解析在 await 之前同步完成，避免把 DOM 带进 future。
        let manifest = super::super::parser::parse_detail_asset_manifest(&html, world)?;
        let mut equipment_ids = manifest.equipment_ids;
        equipment_ids.sort_unstable();
        equipment_ids.dedup();
        Ok(Some(MoogleMailInboxAssets {
            equipment_ids,
            credits: manifest.credits,
            cod: manifest.cod,
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
        let detail = self.client.sent_detail(world, &summary.external_mail_id)?;
        let document = self.client.get(&detail, cookie).await?;
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
        Ok(MoogleMailSentObservation {
            external_mail_id: proof.external_mail_id,
            subject: proof.subject,
            remaining_credits: proof.remaining_credits,
            cod_pending: proof.cod_pending,
            remaining_equipment_ids: proof.remaining_equipment_ids,
            collected,
            body_digest: proof.body_digest,
        })
    }
}

fn protocol_drift(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::protocol_drift(MoogleMailOperation::Compose, world)
}

/// POST 后的读取/解析失败仍属未知外写；保留已有加密诊断引用，不降级成可重试读错误。
fn discard_unknown(error: MoogleMailError) -> MoogleMailError {
    let mut unknown = write_unknown(error.world, MoogleMailOperation::Discard);
    unknown.evidence_id = error.evidence_id;
    unknown.evidence_status = error.evidence_status;
    unknown.evidence_failure = error.evidence_failure;
    unknown
}

fn unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Sent,
        world,
        MoogleMailErrorCategory::RemoteTemporary,
        true,
    )
}
