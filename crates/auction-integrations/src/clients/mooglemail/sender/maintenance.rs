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
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub remaining_equipment_ids: Vec<String>,
    pub collected: bool,
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
    /// 只允许对精确匹配已知投递的草稿调用；失败时不重试，交由下一轮维护重新读取。
    pub async fn discard_compose(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
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
        let form = MoogleMailForm::discard(state.token(), &state.message_fields())
            .map_err(|_| protocol_drift(world))?;
        self.client.post(&compose, cookie, form).await?;
        let document = self.client.get(&compose, cookie).await?;
        let after = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await?;
        if after.is_clean() {
            Ok(MoogleMailDiscardOutcome::Discarded)
        } else {
            Err(protocol_drift(world))
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
            .ok_or_else(|| unavailable(world))?;
        let mut seen = HashSet::new();
        let mut summaries = Vec::new();
        for page_index in 0..self.verification_pages {
            if Instant::now() >= deadline {
                return Err(unavailable(world));
            }
            let target = self.client.sent_page(world, page_index)?;
            let document = timeout_at(deadline, self.client.get(&target, cookie))
                .await
                .map_err(|_| unavailable(world))??;
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
        Err(unavailable(world))
    }

    /// 读取单封 Sent 的实收状态；读取失败由调用方保留上一轮证据。
    pub async fn inspect_sent(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        summary: &MoogleMailSentListEntry,
        observed_at: DateTime<Utc>,
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
        let snapshot = self
            .client
            .diagnose_parse(
                &detail,
                &document,
                self.parser.parse_sent_detail(&document, &parser_summary, observed_at),
            )
            .await?;
        Ok(MoogleMailSentObservation {
            external_mail_id: snapshot.external_mail_id,
            remaining_credits: snapshot.remaining_credits,
            cod_pending: snapshot.cod_pending,
            remaining_equipment_ids: snapshot.remaining_equipment_ids,
            collected: snapshot.collected,
        })
    }
}

fn protocol_drift(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::protocol_drift(MoogleMailOperation::Compose, world)
}

fn unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Sent,
        world,
        MoogleMailErrorCategory::RemoteTemporary,
        true,
    )
}
