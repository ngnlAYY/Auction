use std::{
    collections::{HashMap, HashSet},
    fmt, str,
};

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use scraper::{ElementRef, Html, Selector};
use serde::{
    Deserializer,
    de::{self, IgnoredAny, MapAccess, Visitor},
};
use sha2::{Digest, Sha256};
use url::Url;

use super::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
    protocol::{
        MoogleMailEquipmentKey, MoogleMailToken, valid_decimal_id, valid_hex_key, valid_line,
        valid_opaque_id,
    },
};

const MAX_INBOX_ROWS_PER_PAGE: usize = 1000;
const MAX_SENT_ROWS_PER_PAGE: usize = MAX_INBOX_ROWS_PER_PAGE;
const MAX_ATTACHMENT_ENTRIES: usize = 1000;
const MAX_SUMMARY_SUBJECT_BYTES: usize = 512;
const MAX_MESSAGE_TEXT_BYTES: usize = 8 * 1024 * 1024;
const INBOX_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";

/// 收件箱列表中不含正文和附件的安全摘要。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSummary {
    /// 远端 `mid`。
    pub external_mail_id: String,
    /// 邮件所在世界。
    pub world: MoogleMailWorld,
    /// 发件人显示名。
    pub sender: String,
    /// 收件箱标题。
    pub subject: String,
    /// 远端 HVT-8 时间转换后的 UTC 时间。
    pub received_at: DateTime<Utc>,
}

impl fmt::Debug for MoogleMailSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSummary")
            .field("external_mail_id", &self.external_mail_id)
            .field("world", &self.world)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("received_at", &self.received_at)
            .finish()
    }
}

impl MoogleMailSummary {
    /// 校验摘要可以进入详情读取队列。
    pub fn validate(&self) -> bool {
        valid_opaque_id(&self.external_mail_id)
            && valid_line(&self.sender, 256, false)
            && valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, true)
            && self.received_at.timestamp_nanos_opt().is_some()
    }
}

/// 一件 MoogleMail 装备附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailAttachment {
    /// 远端装备 ID。
    pub equipment_id: String,
    /// 读取装备页所需的短生命周期 key；默认脱敏。
    pub key: MoogleMailEquipmentKey,
}

/// 通过摘要与详情双重校验后的 typed 邮件 envelope。
///
/// 该类型不保存 HTML、mmtoken 或 Cookie，但正文和装备 key 仍属于敏感的短生命周期数据，
/// 只能在应用处理完成后尽快丢弃或调用 `clear_secrets`。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailMessage {
    /// 远端邮件 ID。
    pub external_mail_id: String,
    /// 邮件世界。
    pub world: MoogleMailWorld,
    /// 收件时间。
    pub received_at: DateTime<Utc>,
    /// 认证详情页中的发件人。
    pub sender: String,
    /// 认证详情页中的标题。
    pub subject: String,
    /// 认证详情页 textarea 正文。
    pub body: String,
    /// 附带的 Credits 数量。
    pub credits: i64,
    /// 附带的 COD 数量。
    pub cod: i64,
    /// 附带装备。
    pub attachments: Vec<MoogleMailAttachment>,
    /// 详情页是否提供整封退回动作证明。
    pub returnable: bool,
    /// 详情页是否提供领取附件动作证明。
    pub claimable: bool,
}

/// 详情页动作编排所需的短生命周期状态。
///
/// 该状态为动作前后身份与资产证明保留 token、邮件身份、正文和附件清单，
/// 但不保存装备 key 或原始 HTML。`Debug` 对身份与正文字段脱敏。
pub struct MoogleMailActionState {
    pub(crate) token: MoogleMailToken,
    recipient: String,
    sender: String,
    subject: String,
    body: String,
    equipment_ids: Vec<String>,
    credits: i64,
    cod: i64,
    attachment_count: usize,
    /// 详情页是否声明可以退回整封邮件。
    pub returnable: bool,
    /// 详情页是否声明可以领取全部附件。
    pub claimable: bool,
}

impl fmt::Debug for MoogleMailActionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailActionState")
            .field("token", &"[REDACTED]")
            .field("recipient", &"[REDACTED]")
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("equipment_count", &self.equipment_ids.len())
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachment_count)
            .field("returnable", &self.returnable)
            .field("claimable", &self.claimable)
            .finish()
    }
}

impl MoogleMailActionState {
    pub(crate) fn token(&self) -> &MoogleMailToken {
        &self.token
    }

    pub(crate) fn sender(&self) -> &str {
        &self.sender
    }

    pub(crate) fn equipment_ids(&self) -> &[String] {
        &self.equipment_ids
    }

    pub(crate) fn credits(&self) -> i64 {
        self.credits
    }

    pub(crate) fn cod(&self) -> i64 {
        self.cod
    }

    pub(crate) fn attachment_count(&self) -> usize {
        self.attachment_count
    }
}

impl fmt::Debug for MoogleMailMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailMessage")
            .field("external_mail_id", &self.external_mail_id)
            .field("world", &self.world)
            .field("received_at", &self.received_at)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachments.len())
            .field("returnable", &self.returnable)
            .field("claimable", &self.claimable)
            .finish()
    }
}

impl MoogleMailMessage {
    /// 校验 envelope 的稳定字段、不变量和附件唯一性。
    pub fn validate(&self) -> bool {
        if !valid_opaque_id(&self.external_mail_id)
            || self.received_at.timestamp_nanos_opt().is_none()
            || !valid_line(&self.sender, 256, false)
            || !valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, true)
            || !valid_message_body(&self.body)
            || self.credits < 0
            || self.cod < 0
            || (self.cod > 0 && self.credits == 0 && self.attachments.is_empty())
        {
            return false;
        }
        let mut seen = HashSet::with_capacity(self.attachments.len());
        self.attachments.iter().all(|attachment| {
            valid_decimal_id(&attachment.equipment_id)
                && valid_hex_key(attachment.key.expose())
                && seen.insert(attachment.equipment_id.as_str())
        })
    }

    /// 返回用于入站幂等和审计关联的稳定语义摘要。
    pub fn content_digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        write_digest_string(&mut digest, "mooglemail-envelope-v1");
        let received_at = self.received_at.to_rfc3339();
        let sender = self.sender.to_ascii_lowercase();
        for value in [
            self.external_mail_id.as_str(),
            self.world.as_str(),
            received_at.as_str(),
            sender.as_str(),
            self.subject.as_str(),
            self.body.as_str(),
        ] {
            write_digest_string(&mut digest, value);
        }
        digest.update(self.credits.to_be_bytes());
        digest.update(self.cod.to_be_bytes());
        digest.update([u8::from(self.returnable), u8::from(self.claimable)]);
        let mut equipment_ids = self
            .attachments
            .iter()
            .map(|attachment| attachment.equipment_id.as_str())
            .collect::<Vec<_>>();
        equipment_ids.sort_unstable();
        for equipment_id in equipment_ids {
            write_digest_string(&mut digest, equipment_id);
            if let Some(attachment) =
                self.attachments.iter().find(|item| item.equipment_id == equipment_id)
            {
                digest.update(attachment.key.expose().as_bytes());
            }
        }
        let bytes = digest.finalize();
        let mut result = [0_u8; 32];
        result.copy_from_slice(&bytes);
        result
    }

    /// 尽力清除当前 envelope 持有的装备 key；正文由所有权 drop 回收。
    pub fn clear_secrets(&mut self) {
        for attachment in &mut self.attachments {
            attachment.key.clear();
        }
    }
}

/// 单页收件箱解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailInboxPage {
    /// 按远端 newest-first 顺序排列的摘要。
    pub summaries: Vec<MoogleMailSummary>,
    /// 是否存在严格验证过的下一页链接。
    pub has_next_page: bool,
}

/// 已发送邮件列表中可用于 exact ID 查找的安全摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSentSummary {
    /// 远端 `mid`。
    pub external_mail_id: String,
    /// 邮件所在世界。
    pub world: MoogleMailWorld,
    /// 收件人显示名。
    pub recipient: String,
    /// 已发送邮件标题。
    pub subject: String,
    /// 远端 HVT-8 时间转换后的 UTC 时间。
    pub sent_at: DateTime<Utc>,
}

impl MoogleMailSentSummary {
    /// 校验摘要可以进入 Sent 详情读取队列。
    pub fn validate(&self) -> bool {
        valid_opaque_id(&self.external_mail_id)
            && valid_line(&self.recipient, 256, false)
            && valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, false)
            && self.sent_at.timestamp_nanos_opt().is_some()
    }
}

/// 单页已发送邮件解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSentPage {
    /// 按远端 newest-first 顺序排列的摘要。
    pub summaries: Vec<MoogleMailSentSummary>,
    /// 是否存在严格验证过的下一页链接。
    pub has_next_page: bool,
}

/// 精确 remote ID 的已发送邮件与当前待领取资产快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSentSnapshot {
    /// 远端邮件 ID。
    pub external_mail_id: String,
    /// 邮件所在世界。
    pub world: MoogleMailWorld,
    /// 收件人。
    pub recipient: String,
    /// 标题。
    pub subject: String,
    /// 发送时间。
    pub sent_at: DateTime<Utc>,
    /// 当前仍附在邮件中的 Credits。
    pub remaining_credits: i64,
    /// 当前仍待支付的 COD。
    pub cod_pending: i64,
    /// 当前仍附在邮件中的装备 ID，必须按字典序排列且无重复。
    pub remaining_equipment_ids: Vec<String>,
    /// 页面是否明确证明邮件已领取完毕。
    pub collected: bool,
    /// 本次读取的观察时间。
    pub observed_at: DateTime<Utc>,
}

impl MoogleMailSentSnapshot {
    /// 校验 Sent snapshot 的身份、时间和 remaining/collected 矩阵。
    pub fn validate(&self) -> bool {
        valid_opaque_id(&self.external_mail_id)
            && valid_line(&self.recipient, 256, false)
            && valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, false)
            && self.sent_at.timestamp_nanos_opt().is_some()
            && self.observed_at.timestamp_nanos_opt().is_some()
            && !self.observed_at.lt(&self.sent_at)
            && self.remaining_credits >= 0
            && self.cod_pending >= 0
            && canonical_equipment_ids(&self.remaining_equipment_ids)
            && self.collected
                == (self.remaining_credits == 0
                    && self.cod_pending == 0
                    && self.remaining_equipment_ids.is_empty())
    }
}

/// 一次同步读取的 world wallet Credits 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailBalanceSnapshot {
    /// 余额所属世界。
    pub world: MoogleMailWorld,
    /// 页面与脚本均证明的 Credits 余额。
    pub credits: i64,
    /// 本次读取的观察时间。
    pub observed_at: DateTime<Utc>,
}

impl MoogleMailBalanceSnapshot {
    /// 校验余额 world、数值和观察时间。
    pub fn validate(&self) -> bool {
        self.credits >= 0 && self.observed_at.timestamp_nanos_opt().is_some()
    }
}

/// 无状态的 MoogleMail HTML 解析器。
#[derive(Debug, Clone, Copy, Default)]
pub struct MoogleMailParser;

impl MoogleMailParser {
    /// 严格解析一页 inbox，并将跨页重复 ID 视为协议漂移。
    pub fn scan_inbox_page(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        page: usize,
        seen: &mut HashSet<String>,
    ) -> Result<MoogleMailInboxPage, MoogleMailError> {
        let source =
            str::from_utf8(document).map_err(|_| protocol(MoogleMailOperation::Inbox, world))?;
        if source.trim().is_empty() {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }
        let document = Html::parse_document(source);
        let table_selector = selector("table#mmail_list", MoogleMailOperation::Inbox, world)?;
        let tables = document.select(&table_selector).collect::<Vec<_>>();
        if tables.len() != 1 {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }
        let row_selector = selector("tr", MoogleMailOperation::Inbox, world)?;
        let rows = tables[0].select(&row_selector).collect::<Vec<_>>();
        if rows.is_empty() || rows.len().saturating_sub(1) > MAX_INBOX_ROWS_PER_PAGE {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }
        if !valid_inbox_header(rows[0], world)? {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }

        let cell_selector = selector("td", MoogleMailOperation::Inbox, world)?;
        let mut summaries = Vec::with_capacity(rows.len().saturating_sub(1));
        let mut empty_row = false;
        for row in rows.iter().skip(1) {
            let cells = row.select(&cell_selector).collect::<Vec<_>>();
            let empty_selector = selector("td#mmail_nnm", MoogleMailOperation::Inbox, world)?;
            let is_empty = row.select(&empty_selector).count() == 1;
            if is_empty {
                if cells.len() != 1 || empty_row || !summaries.is_empty() {
                    return Err(protocol(MoogleMailOperation::Inbox, world));
                }
                empty_row = true;
                continue;
            }
            if empty_row || cells.len() != 4 {
                return Err(protocol(MoogleMailOperation::Inbox, world));
            }
            let onclick = row
                .value()
                .attr("onclick")
                .ok_or_else(|| protocol(MoogleMailOperation::Inbox, world))?;
            let external_mail_id =
                extract_mid(onclick).ok_or_else(|| protocol(MoogleMailOperation::Inbox, world))?;
            if !seen.insert(external_mail_id.clone()) {
                return Err(protocol(MoogleMailOperation::Inbox, world));
            }
            let sender = normalize_text(cells[0].text().collect::<String>());
            let subject = normalize_text(cells[1].text().collect::<String>());
            let received_at = parse_page_time(
                &normalize_text(cells[2].text().collect::<String>()),
                MoogleMailOperation::Inbox,
                world,
            )?;
            if !valid_line(&sender, 256, false)
                || !valid_line(&subject, MAX_SUMMARY_SUBJECT_BYTES, true)
            {
                return Err(protocol(MoogleMailOperation::Inbox, world));
            }
            summaries.push(MoogleMailSummary {
                external_mail_id,
                world,
                sender,
                subject,
                received_at,
            });
        }
        if !empty_row && summaries.is_empty() {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }
        let has_next_page =
            parse_next_page(&document, world, page, "inbox", MoogleMailOperation::Inbox)?;
        Ok(MoogleMailInboxPage { summaries, has_next_page })
    }

    /// 严格解析一页 Sent 列表，并将跨页重复 ID 视为协议漂移。
    pub fn scan_sent_page(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        page: usize,
        seen: &mut HashSet<String>,
    ) -> Result<MoogleMailSentPage, MoogleMailError> {
        let source =
            str::from_utf8(document).map_err(|_| protocol(MoogleMailOperation::Sent, world))?;
        if source.trim().is_empty() {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }
        let document = Html::parse_document(source);
        let table_selector = selector("table#mmail_list", MoogleMailOperation::Sent, world)?;
        let tables = document.select(&table_selector).collect::<Vec<_>>();
        if tables.len() != 1 {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }
        let row_selector = selector("tr", MoogleMailOperation::Sent, world)?;
        let rows = tables[0].select(&row_selector).collect::<Vec<_>>();
        if rows.is_empty() || rows.len().saturating_sub(1) > MAX_SENT_ROWS_PER_PAGE {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }
        if !valid_sent_header(rows[0], world)? {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }

        let cell_selector = selector("td", MoogleMailOperation::Sent, world)?;
        let mut summaries = Vec::with_capacity(rows.len().saturating_sub(1));
        let mut empty_row = false;
        for row in rows.iter().skip(1) {
            let cells = row.select(&cell_selector).collect::<Vec<_>>();
            let empty_selector = selector("td#mmail_nnm", MoogleMailOperation::Sent, world)?;
            let is_empty = row.select(&empty_selector).count() == 1;
            if is_empty {
                if cells.len() != 1 || empty_row || !summaries.is_empty() {
                    return Err(protocol(MoogleMailOperation::Sent, world));
                }
                empty_row = true;
                continue;
            }
            if empty_row || cells.len() != 4 {
                return Err(protocol(MoogleMailOperation::Sent, world));
            }
            let onclick = row
                .value()
                .attr("onclick")
                .ok_or_else(|| protocol(MoogleMailOperation::Sent, world))?;
            let external_mail_id =
                extract_mid(onclick).ok_or_else(|| protocol(MoogleMailOperation::Sent, world))?;
            if !seen.insert(external_mail_id.clone()) {
                return Err(protocol(MoogleMailOperation::Sent, world));
            }
            let recipient = normalize_text(cells[0].text().collect::<String>());
            let subject = normalize_text(cells[1].text().collect::<String>());
            let sent_at = parse_page_time(
                &normalize_text(cells[2].text().collect::<String>()),
                MoogleMailOperation::Sent,
                world,
            )?;
            let read_state = normalize_text(cells[3].text().collect::<String>());
            if !valid_line(&recipient, 256, false)
                || !valid_line(&subject, MAX_SUMMARY_SUBJECT_BYTES, false)
                || !valid_line(&read_state, 64, true)
            {
                return Err(protocol(MoogleMailOperation::Sent, world));
            }
            summaries.push(MoogleMailSentSummary {
                external_mail_id,
                world,
                recipient,
                subject,
                sent_at,
            });
        }
        if !empty_row && summaries.is_empty() {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }
        let has_next_page =
            parse_next_page(&document, world, page, "sent", MoogleMailOperation::Sent)?;
        Ok(MoogleMailSentPage { summaries, has_next_page })
    }

    /// 严格解析与 Sent 摘要身份一致的详情快照。
    pub fn parse_sent_detail(
        &self,
        document: &[u8],
        summary: &MoogleMailSentSummary,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        if !summary.validate() || observed_at < summary.sent_at {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let source = str::from_utf8(document).map_err(|_| {
            detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            )
        })?;
        if source.trim().is_empty() {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let document = Html::parse_document(source);
        parse_token_for_operation(&document, summary.world, MoogleMailOperation::SentDetail)?;
        let envelope_selector =
            selector("#mmail_outer", MoogleMailOperation::SentDetail, summary.world)?;
        let form_selector =
            selector("form#mailform", MoogleMailOperation::SentDetail, summary.world)?;
        let panes_selector =
            selector("#mmail_attachpanes", MoogleMailOperation::SentDetail, summary.world)?;
        let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
        let forms = document.select(&form_selector).collect::<Vec<_>>();
        let panes = document.select(&panes_selector).collect::<Vec<_>>();
        if envelopes.len() != 1 || forms.len() != 1 || panes.len() != 1 {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        if envelopes[0].select(&form_selector).count() != 1
            || envelopes[0].select(&panes_selector).count() != 1
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let fields =
            parse_detail_fields_for(forms[0], summary.world, MoogleMailOperation::SentDetail)?;
        if !fields.recipient.eq_ignore_ascii_case(&summary.recipient)
            || fields.subject != summary.subject
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::IdentityMismatch,
            ));
        }

        let script = script_text(&document, summary.world, MoogleMailOperation::SentDetail)?;
        let executable = executable_javascript(&script).ok_or_else(|| {
            detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            )
        })?;
        let count = required_script_value(&executable, "attach_count");
        let cod = required_script_value(&executable, "attach_cod");
        let state = required_script_value(&executable, "mail_state");
        if count.is_none()
            || cod.is_none()
            || state != Some(1)
            || count.is_some_and(|value| !(0..=MAX_ATTACHMENT_ENTRIES as i64).contains(&value))
            || cod.is_some_and(|value| value < 0)
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let (Some(count), Some(cod)) = (count, cod) else {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        };

        let list_selector =
            selector("#mmail_attachlist", MoogleMailOperation::SentDetail, summary.world)?;
        let all_lists = document.select(&list_selector).collect::<Vec<_>>();
        let nested_lists = panes[0].select(&list_selector).collect::<Vec<_>>();
        if all_lists.len() != nested_lists.len() || nested_lists.len() > 1 {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let (remaining_equipment_ids, remaining_credits, entries) =
            if let Some(list) = nested_lists.first() {
                let (equipment_ids, credits, entries) =
                    parse_attachment_manifest(*list, summary.world)?;
                (equipment_ids, credits, entries)
            } else {
                (Vec::new(), 0, 0)
            };
        if entries as i64 != count
            || (count == 0 && !nested_lists.is_empty())
            || (count > 0 && nested_lists.len() != 1)
            || (cod > 0 && count == 0)
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }

        let current_cod_selector =
            selector("#mmail_currentcod", MoogleMailOperation::SentDetail, summary.world)?;
        let current_cod = document.select(&current_cod_selector).collect::<Vec<_>>();
        if current_cod.len() > 1
            || current_cod.first().is_some_and(|element| {
                parse_current_cod(&normalize_text(element.text().collect::<String>())) != Some(cod)
            })
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let mut remaining_equipment_ids = remaining_equipment_ids;
        remaining_equipment_ids.sort_unstable();
        let snapshot = MoogleMailSentSnapshot {
            external_mail_id: summary.external_mail_id.clone(),
            world: summary.world,
            recipient: summary.recipient.clone(),
            subject: summary.subject.clone(),
            sent_at: summary.sent_at,
            remaining_credits,
            cod_pending: cod,
            collected: count == 0 && cod == 0,
            remaining_equipment_ids,
            observed_at,
        };
        if !snapshot.validate() {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        Ok(snapshot)
    }

    /// 严格解析 Item Shop 余额，并要求可见余额与脚本余额唯一且一致。
    pub fn parse_balance(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailBalanceSnapshot, MoogleMailError> {
        if observed_at.timestamp_nanos_opt().is_none() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let source =
            str::from_utf8(document).map_err(|_| protocol(MoogleMailOperation::Balance, world))?;
        if source.trim().is_empty() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let document = Html::parse_document(source);
        let networth_selector = selector("#networth", MoogleMailOperation::Balance, world)?;
        let networth = document.select(&networth_selector).collect::<Vec<_>>();
        if networth.len() != 1 {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let visible_credits =
            parse_networth_credits(&normalize_text(networth[0].text().collect::<String>()));
        let script = script_text(&document, world, MoogleMailOperation::Balance)?;
        let executable = executable_javascript(&script)
            .ok_or_else(|| protocol(MoogleMailOperation::Balance, world))?;
        let script_credits = required_script_value(&executable, "current_credits");
        if visible_credits.is_none()
            || script_credits.is_none()
            || visible_credits != script_credits
        {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let Some(credits) = visible_credits else {
            return Err(protocol(MoogleMailOperation::Balance, world));
        };
        let snapshot = MoogleMailBalanceSnapshot { world, credits, observed_at };
        if !snapshot.validate() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        Ok(snapshot)
    }

    /// 严格解析单封详情，并以收件箱摘要校验稳定身份。
    pub fn parse_detail(
        &self,
        document: &[u8],
        summary: &MoogleMailSummary,
    ) -> Result<MoogleMailMessage, MoogleMailError> {
        if !summary.validate() {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::Configuration));
        }
        let source = str::from_utf8(document)
            .map_err(|_| detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift))?;
        if source.trim().is_empty() {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let document = Html::parse_document(source);
        parse_token(&document, summary.world)?;
        if has_result_message(&document, summary.world)? {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }

        let envelope_selector =
            selector("#mmail_outer", MoogleMailOperation::Detail, summary.world)?;
        let form_selector = selector("form#mailform", MoogleMailOperation::Detail, summary.world)?;
        let panes_selector =
            selector("#mmail_attachpanes", MoogleMailOperation::Detail, summary.world)?;
        let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
        let forms = document.select(&form_selector).collect::<Vec<_>>();
        let panes = document.select(&panes_selector).collect::<Vec<_>>();
        if envelopes.len() != 1 || forms.len() != 1 || panes.len() != 1 {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        if envelopes[0].select(&form_selector).count() != 1
            || envelopes[0].select(&panes_selector).count() != 1
        {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let fields = parse_detail_fields(forms[0], summary.world)?;
        if !fields.sender.eq_ignore_ascii_case(&summary.sender) {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::IdentityMismatch));
        }
        let textarea_selector = selector("textarea", MoogleMailOperation::Detail, summary.world)?;
        let textareas = forms[0].select(&textarea_selector).collect::<Vec<_>>();
        if textareas.len() != 1 {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let body = textareas[0].text().collect::<String>();
        if !valid_message_body(&body) {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let assets = parse_detail_assets(&document, &body, summary.world)?;
        let (returnable, claimable) = parse_detail_actions(envelopes[0], summary.world)?;
        let has_assets = !assets.attachments.is_empty() || assets.credits > 0;
        if claimable != has_assets {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let message = MoogleMailMessage {
            external_mail_id: summary.external_mail_id.clone(),
            world: summary.world,
            received_at: summary.received_at,
            sender: fields.sender,
            subject: fields.subject,
            body,
            credits: assets.credits,
            cod: assets.cod,
            attachments: assets.attachments,
            returnable,
            claimable,
        };
        if !message.validate() {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        Ok(message)
    }

    /// 严格解析详情页中可执行的 return/take 动作状态。
    pub fn parse_action_state(
        &self,
        source_bytes: &[u8],
        world: MoogleMailWorld,
    ) -> Result<MoogleMailActionState, MoogleMailError> {
        let source = str::from_utf8(source_bytes)
            .map_err(|_| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        if source.trim().is_empty() {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let document = Html::parse_document(source);
        if has_result_message(&document, world)? {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let token = parse_token(&document, world)?;
        let envelope_selector = selector("#mmail_outer", MoogleMailOperation::Detail, world)?;
        let form_selector = selector("form#mailform", MoogleMailOperation::Detail, world)?;
        let panes_selector = selector("#mmail_attachpanes", MoogleMailOperation::Detail, world)?;
        let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
        let forms = document.select(&form_selector).collect::<Vec<_>>();
        let panes = document.select(&panes_selector).collect::<Vec<_>>();
        if envelopes.len() != 1
            || forms.len() != 1
            || panes.len() != 1
            || envelopes[0].select(&form_selector).count() != 1
            || envelopes[0].select(&panes_selector).count() != 1
        {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let fields = parse_detail_fields(forms[0], world)?;
        let textarea_selector = selector("textarea", MoogleMailOperation::Detail, world)?;
        let textareas = forms[0].select(&textarea_selector).collect::<Vec<_>>();
        if textareas.len() != 1 {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let body = textareas[0].text().collect::<String>();
        if !valid_message_body(&body) {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let assets = parse_detail_asset_manifest(&document, world)?;
        let (returnable, claimable) = parse_detail_actions(envelopes[0], world)?;
        let has_assets = !assets.equipment_ids.is_empty() || assets.credits > 0;
        if claimable != has_assets {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        Ok(MoogleMailActionState {
            token,
            recipient: fields.recipient,
            sender: fields.sender,
            subject: fields.subject,
            body,
            equipment_ids: assets.equipment_ids,
            credits: assets.credits,
            cod: assets.cod,
            attachment_count: assets.entry_count,
            returnable,
            claimable,
        })
    }

    /// 通过返回后的收件箱结构和目标 `mid` 缺席证明整封退回已完成。
    pub fn prove_return_completed(
        &self,
        source_bytes: &[u8],
        world: MoogleMailWorld,
        external_mail_id: &str,
    ) -> Result<(), MoogleMailError> {
        let operation = MoogleMailOperation::ReturnMessage;
        if !valid_opaque_id(external_mail_id) {
            return Err(action_proof_error(operation, world));
        }
        let source =
            str::from_utf8(source_bytes).map_err(|_| action_proof_error(operation, world))?;
        if source.trim().is_empty() {
            return Err(action_proof_error(operation, world));
        }
        let document = Html::parse_document(source);
        if has_result_message(&document, world).map_err(|_| action_proof_error(operation, world))? {
            return Err(action_proof_error(operation, world));
        }
        let form_selector = selector("form#mailform", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
        let table_selector = selector("table#mmail_list", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
        let reference_selector = selector("[href], [onclick], [action]", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
        let tables = document.select(&table_selector).collect::<Vec<_>>();
        if document.select(&form_selector).next().is_some()
            || tables.len() != 1
            || document.select(&reference_selector).any(|element| {
                ["href", "onclick", "action"].iter().any(|attribute| {
                    element
                        .value()
                        .attr(attribute)
                        .is_some_and(|value| attribute_references_mail_id(value, external_mail_id))
                })
            })
        {
            return Err(action_proof_error(operation, world));
        }
        let row_selector =
            selector("tr", operation, world).map_err(|_| action_proof_error(operation, world))?;
        let cell_selector =
            selector("td", operation, world).map_err(|_| action_proof_error(operation, world))?;
        let empty_selector = selector("td#mmail_nnm", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
        let rows = tables[0].select(&row_selector).collect::<Vec<_>>();
        if rows.len() < 2
            || !valid_inbox_header(rows[0], world)
                .map_err(|_| action_proof_error(operation, world))?
        {
            return Err(action_proof_error(operation, world));
        }
        for row in &rows[1..] {
            let cells = row.select(&cell_selector).count();
            if row.select(&empty_selector).count() == 1 && cells == 1 {
                continue;
            }
            let mail_id = row.value().attr("onclick").and_then(extract_mid);
            if cells != 4 || mail_id.as_deref().is_none_or(|mail_id| mail_id == external_mail_id) {
                return Err(action_proof_error(operation, world));
            }
        }
        Ok(())
    }

    /// 通过动作前后的邮件身份、资产归零和正文回执证明附件领取已完成。
    pub fn prove_take_completed(
        &self,
        before: &MoogleMailActionState,
        source_bytes: &[u8],
        world: MoogleMailWorld,
    ) -> Result<(), MoogleMailError> {
        let operation = MoogleMailOperation::Take;
        match self.parse_action_state(source_bytes, world) {
            Ok(after) => {
                if after.recipient != before.recipient
                    || after.sender != before.sender
                    || after.subject != before.subject
                    || !after.equipment_ids.is_empty()
                    || after.credits != 0
                    || after.cod != 0
                    || after.attachment_count != 0
                    || after.claimable
                {
                    return Err(action_proof_error(operation, world));
                }
                prove_take_identity_and_receipts(
                    before,
                    &after.recipient,
                    &after.sender,
                    &after.subject,
                    &after.body,
                    world,
                )
            }
            Err(_) => {
                let (fields, body) = parse_terminal_empty_detail(source_bytes, world)?;
                prove_take_identity_and_receipts(
                    before,
                    &fields.recipient,
                    &fields.sender,
                    &fields.subject,
                    &body,
                    world,
                )
            }
        }
    }
}

struct DetailFields {
    recipient: String,
    sender: String,
    subject: String,
}

struct DetailAssets {
    attachments: Vec<MoogleMailAttachment>,
    credits: i64,
    cod: i64,
}

struct DetailAssetManifest {
    equipment_ids: Vec<String>,
    credits: i64,
    cod: i64,
    entry_count: usize,
}

fn parse_terminal_empty_detail(
    source_bytes: &[u8],
    world: MoogleMailWorld,
) -> Result<(DetailFields, String), MoogleMailError> {
    let operation = MoogleMailOperation::Take;
    let source = str::from_utf8(source_bytes).map_err(|_| action_proof_error(operation, world))?;
    if source.trim().is_empty() {
        return Err(action_proof_error(operation, world));
    }
    let document = Html::parse_document(source);
    if has_result_message(&document, world).map_err(|_| action_proof_error(operation, world))? {
        return Err(action_proof_error(operation, world));
    }
    parse_token_for_operation(&document, world, operation)
        .map_err(|_| action_proof_error(operation, world))?;

    let envelope_selector = selector("#mmail_outer", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let form_selector = selector("form#mailform", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let all_form_selector =
        selector("form", operation, world).map_err(|_| action_proof_error(operation, world))?;
    let panes_selector = selector("#mmail_attachpanes", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let buttons_selector = selector("#mmail_showbuttons", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
    let forms = document.select(&form_selector).collect::<Vec<_>>();
    let panes = document.select(&panes_selector).collect::<Vec<_>>();
    let buttons = document.select(&buttons_selector).collect::<Vec<_>>();
    if envelopes.len() != 1
        || forms.len() != 1
        || document.select(&all_form_selector).count() != 1
        || panes.len() != 1
        || buttons.len() != 1
        || envelopes[0].select(&form_selector).count() != 1
        || envelopes[0].select(&panes_selector).count() != 1
        || envelopes[0].select(&buttons_selector).count() != 1
    {
        return Err(action_proof_error(operation, world));
    }

    let onclick_selector = selector("[onclick]", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let asset_selector =
        selector("#mmail_attachlist, #mmail_attachcount, #newcod", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
    let equipment_selector = selector("[onmouseover]", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    if document.select(&onclick_selector).next().is_some()
        || document.select(&asset_selector).next().is_some()
        || document.select(&equipment_selector).next().is_some()
        || ["attach_count", "attach_cod", "mail_state", "equips.set("]
            .iter()
            .any(|marker| source.contains(marker))
    {
        return Err(action_proof_error(operation, world));
    }

    let fields = parse_detail_fields_for(forms[0], world, operation)
        .map_err(|_| action_proof_error(operation, world))?;
    let textarea_selector =
        selector("textarea", operation, world).map_err(|_| action_proof_error(operation, world))?;
    let textareas = forms[0].select(&textarea_selector).collect::<Vec<_>>();
    if textareas.len() != 1 {
        return Err(action_proof_error(operation, world));
    }
    let body = textareas[0].text().collect::<String>();
    if !valid_message_body(&body) {
        return Err(action_proof_error(operation, world));
    }
    Ok((fields, body))
}

fn prove_take_identity_and_receipts(
    before: &MoogleMailActionState,
    recipient: &str,
    sender: &str,
    subject: &str,
    body: &str,
    world: MoogleMailWorld,
) -> Result<(), MoogleMailError> {
    let operation = MoogleMailOperation::Take;
    if recipient != before.recipient || sender != before.sender || subject != before.subject {
        return Err(action_proof_error(operation, world));
    }
    let base = canonical_mail_body(&before.body);
    let observed = canonical_mail_body(body);
    let Some(receipts) = observed.strip_prefix(&base) else {
        return Err(action_proof_error(operation, world));
    };
    if (before.cod > 0 && take_cod_receipt_amount(receipts) != Some(before.cod))
        || (before.cod == 0 && receipts.contains("CoD Paid:"))
        || !valid_removed_attachment_receipt(receipts, before.attachment_count > 0)
    {
        return Err(action_proof_error(operation, world));
    }
    Ok(())
}

fn canonical_mail_body(body: &str) -> String {
    body.replace("\r\n", "\n").trim_end_matches([' ', '\t', '\n']).to_owned()
}

fn take_cod_receipt_amount(value: &str) -> Option<i64> {
    const MARKER: &str = "CoD Paid: ";
    let matches = value.match_indices(MARKER).take(2).collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let remainder = &value[matches[0].0 + MARKER.len()..];
    let amount_end = remainder
        .find(|character: char| !character.is_ascii_digit() && character != ',')
        .unwrap_or(remainder.len());
    if amount_end == 0 || !remainder[amount_end..].starts_with(" Credits") {
        return None;
    }
    parse_canonical_credits(&remainder[..amount_end])
}

fn valid_removed_attachment_receipt(value: &str, expected: bool) -> bool {
    const MARKER: &str = "Removed attachment:";
    let matches = value.match_indices(MARKER).take(2).collect::<Vec<_>>();
    if !expected {
        return matches.is_empty();
    }
    if matches.len() != 1 {
        return false;
    }
    let remainder = &value[matches[0].0 + MARKER.len()..];
    let line =
        remainder.trim_start_matches([' ', '\t']).split(['\r', '\n']).next().unwrap_or_default();
    !line.trim().is_empty()
}

fn valid_inbox_header(
    row: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<bool, MoogleMailError> {
    let selector = selector("th", MoogleMailOperation::Inbox, world)?;
    let headers = row.select(&selector).collect::<Vec<_>>();
    let expected = ["From", "Subject", "Sent", "Read"];
    Ok(headers.len() == expected.len()
        && headers.iter().zip(expected).all(|(header, expected)| {
            normalize_text(header.text().collect::<String>()) == expected
        }))
}

fn valid_sent_header(row: ElementRef<'_>, world: MoogleMailWorld) -> Result<bool, MoogleMailError> {
    let selector = selector("th", MoogleMailOperation::Sent, world)?;
    let headers = row.select(&selector).collect::<Vec<_>>();
    let expected = ["To", "Subject", "Sent", "Read"];
    Ok(headers.len() == expected.len()
        && headers.iter().zip(expected).all(|(header, expected)| {
            normalize_text(header.text().collect::<String>()) == expected
        }))
}

fn parse_next_page(
    document: &Html,
    world: MoogleMailWorld,
    current_page: usize,
    filter: &str,
    operation: MoogleMailOperation,
) -> Result<bool, MoogleMailError> {
    let pager_selector = selector("#mmail_pager", operation, world)?;
    let pagers = document.select(&pager_selector).collect::<Vec<_>>();
    if pagers.len() != 1 {
        return Err(protocol(operation, world));
    }
    let controls = direct_children(pagers[0], "div");
    if controls.len() != 2 {
        return Err(protocol(operation, world));
    }
    let next = controls[1];
    let links = direct_children(next, "a");
    let spans = direct_children(next, "span");
    if links.is_empty() && spans.len() == 1 && direct_element_children(next).len() == 1 {
        if normalize_text(spans[0].text().collect::<String>()) != "Next >" {
            return Err(protocol(operation, world));
        }
        return Ok(false);
    }
    if links.len() != 1
        || !spans.is_empty()
        || direct_element_children(next).len() != 1
        || normalize_text(links[0].text().collect::<String>()) != "Next >"
    {
        return Err(protocol(operation, world));
    }
    let href = links[0].value().attr("href").ok_or_else(|| protocol(operation, world))?;
    if !valid_next_page_href(href, world, current_page.saturating_add(1), filter) {
        return Err(protocol(operation, world));
    }
    Ok(true)
}

fn valid_next_page_href(
    raw: &str,
    world: MoogleMailWorld,
    expected_page: usize,
    filter: &str,
) -> bool {
    if raw.is_empty()
        || raw != raw.trim()
        || raw.chars().any(char::is_control)
        || !raw.starts_with('/')
    {
        return false;
    }
    if raw.starts_with("//") {
        return false;
    }
    let base = match Url::parse("https://hentaiverse.org/") {
        Ok(base) => base,
        Err(_) => return false,
    };
    let parsed = match base.join(raw) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != world.path()
    {
        return false;
    }
    let pairs = parsed.query_pairs().collect::<Vec<_>>();
    if pairs.len() != 4 {
        return false;
    }
    let mut values = HashMap::with_capacity(4);
    for (key, value) in pairs {
        if values.insert(key.into_owned(), value.into_owned()).is_some() {
            return false;
        }
    }
    values.get("s").map(String::as_str) == Some("Bazaar")
        && values.get("ss").map(String::as_str) == Some("mm")
        && values.get("filter").map(String::as_str) == Some(filter)
        && values.get("page").and_then(|value| value.parse::<usize>().ok()) == Some(expected_page)
}

fn parse_token(
    document: &Html,
    world: MoogleMailWorld,
) -> Result<MoogleMailToken, MoogleMailError> {
    parse_token_for_operation(document, world, MoogleMailOperation::Detail)
}

fn parse_token_for_operation(
    document: &Html,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<MoogleMailToken, MoogleMailError> {
    let form_selector = selector("form#mailform", operation, world)?;
    let token_selector = selector("input[name='mmtoken']", operation, world)?;
    let forms = document.select(&form_selector).collect::<Vec<_>>();
    if forms.len() != 1 {
        return Err(detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let inputs = forms[0].select(&token_selector).collect::<Vec<_>>();
    if inputs.len() != 1 {
        return Err(detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let value = inputs[0].value().attr("value").ok_or_else(|| {
        detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift)
    })?;
    MoogleMailToken::new(value)
        .ok_or_else(|| detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift))
}

fn has_result_message(document: &Html, world: MoogleMailWorld) -> Result<bool, MoogleMailError> {
    let selector = selector("#messagebox_inner", MoogleMailOperation::Detail, world)?;
    let boxes = document.select(&selector).collect::<Vec<_>>();
    if boxes.len() > 1 {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(boxes
        .into_iter()
        .flat_map(|item| direct_children(item, "p"))
        .any(|paragraph| !normalize_text(paragraph.text().collect::<String>()).is_empty()))
}

fn parse_detail_fields(
    form: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<DetailFields, MoogleMailError> {
    parse_detail_fields_for(form, world, MoogleMailOperation::Detail)
}

fn parse_detail_fields_for(
    form: ElementRef<'_>,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<DetailFields, MoogleMailError> {
    let left_selector = selector("#mmail_left", operation, world)?;
    let row_selector = selector("tr", operation, world)?;
    let div_selector = selector("div", operation, world)?;
    let input_selector = selector("input", operation, world)?;
    let lefts = form.select(&left_selector).collect::<Vec<_>>();
    if lefts.len() != 1 {
        return Err(detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let mut values = HashMap::<String, String>::with_capacity(3);
    for row in lefts[0].select(&row_selector) {
        let divs = row.select(&div_selector).collect::<Vec<_>>();
        let label = divs
            .first()
            .map(|div| {
                normalize_text(div.text().collect::<String>()).trim_end_matches(':').to_owned()
            })
            .unwrap_or_default();
        if !matches!(label.as_str(), "To" | "From" | "Subject") {
            continue;
        }
        let inputs = row.select(&input_selector).collect::<Vec<_>>();
        let value =
            inputs.first().and_then(|input| input.value().attr("value")).ok_or_else(|| {
                detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift)
            })?;
        if inputs.len() != 1 || values.insert(label, value.to_owned()).is_some() {
            return Err(detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift));
        }
    }
    let recipient = values.get("To").map(String::as_str).unwrap_or_default().trim();
    let sender = values.get("From").map(String::as_str).unwrap_or_default();
    let subject = values.get("Subject").map(String::as_str).unwrap_or_default();
    if recipient.is_empty()
        || !valid_line(recipient, 256, false)
        || !valid_line(sender, 256, false)
        || !valid_line(subject, MAX_SUMMARY_SUBJECT_BYTES, true)
    {
        return Err(detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(DetailFields {
        recipient: recipient.to_owned(),
        sender: sender.to_owned(),
        subject: subject.to_owned(),
    })
}

fn parse_detail_assets(
    document: &Html,
    body: &str,
    world: MoogleMailWorld,
) -> Result<DetailAssets, MoogleMailError> {
    let manifest = parse_detail_asset_manifest(document, world)?;
    let wanted = manifest.equipment_ids.iter().cloned().collect::<HashSet<_>>();
    let keys = equipment_keys(document, body, &wanted, world)?;
    let attachments = manifest
        .equipment_ids
        .into_iter()
        .map(|equipment_id| {
            let key = keys
                .get(&equipment_id)
                .cloned()
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            Ok(MoogleMailAttachment { equipment_id, key })
        })
        .collect::<Result<Vec<_>, MoogleMailError>>()?;
    Ok(DetailAssets { attachments, credits: manifest.credits, cod: manifest.cod })
}

/// 动作前置核对只需附件 ID 与金额，不应为远端写入链路额外保留装备 key。
fn parse_detail_asset_manifest(
    document: &Html,
    world: MoogleMailWorld,
) -> Result<DetailAssetManifest, MoogleMailError> {
    let panes_selector = selector("#mmail_attachpanes", MoogleMailOperation::Detail, world)?;
    let list_selector = selector("#mmail_attachlist", MoogleMailOperation::Detail, world)?;
    let panes = document.select(&panes_selector).collect::<Vec<_>>();
    if panes.len() != 1 {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let script = script_text(document, world, MoogleMailOperation::Detail)?;
    let executable = executable_javascript(&script)
        .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    let count = required_script_value(&executable, "attach_count")
        .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    let cod = required_script_value(&executable, "attach_cod")
        .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    let state = required_script_value(&executable, "mail_state")
        .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    if !(0..=MAX_ATTACHMENT_ENTRIES as i64).contains(&count) || cod < 0 || state != 1 {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let all_lists = document.select(&list_selector).collect::<Vec<_>>();
    let nested_lists = panes[0].select(&list_selector).collect::<Vec<_>>();
    if all_lists.len() != nested_lists.len() || nested_lists.len() > 1 {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let (equipment_ids, credits, entries) = if let Some(list) = nested_lists.first() {
        parse_attachment_manifest(*list, world)?
    } else {
        (Vec::new(), 0, 0)
    };
    if entries as i64 != count
        || (count == 0 && !nested_lists.is_empty())
        || (count > 0 && nested_lists.len() != 1)
        || (cod > 0 && count == 0)
    {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(DetailAssetManifest { equipment_ids, credits, cod, entry_count: entries })
}

fn parse_attachment_manifest(
    list: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<(Vec<String>, i64, usize), MoogleMailError> {
    let entries = direct_element_children(list);
    if entries.is_empty() || entries.len() > MAX_ATTACHMENT_ENTRIES {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let list_text = normalize_text(list.text().collect::<String>());
    let child_text = normalize_text(
        entries.iter().map(|entry| entry.text().collect::<String>()).collect::<String>(),
    );
    if list_text != child_text {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let onmouseover_selector = selector("[onmouseover]", MoogleMailOperation::Detail, world)?;
    let mut equipment_ids = Vec::new();
    let mut seen = HashSet::new();
    let mut credits = 0_i64;
    for entry in &entries {
        let mut hooks = entry.select(&onmouseover_selector).collect::<Vec<_>>();
        if entry.value().attr("onmouseover").is_some() {
            hooks.insert(0, *entry);
        }
        let text = normalize_text(entry.text().collect::<String>());
        if !hooks.is_empty() {
            if hooks.len() != 1 {
                return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            let script = hooks[0]
                .value()
                .attr("onmouseover")
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            let equipment_id = extract_equipment_id(script)
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if normalize_text(hooks[0].text().collect::<String>()) != text
                || !seen.insert(equipment_id.clone())
            {
                return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            equipment_ids.push(equipment_id);
        } else {
            let amount = parse_credits_text(&text)
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if credits != 0 || amount <= 0 {
                return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            credits = amount;
        }
    }
    Ok((equipment_ids, credits, entries.len()))
}

fn parse_credits_text(value: &str) -> Option<i64> {
    let value = value.strip_suffix(" Credits")?;
    let value = value.strip_suffix('x').unwrap_or(value);
    let amount = parse_canonical_credits(value)?;
    (amount > 0).then_some(amount)
}

fn parse_canonical_credits(value: &str) -> Option<i64> {
    if value.is_empty() {
        return None;
    }
    let groups = value.split(',').collect::<Vec<_>>();
    if groups.len() > 1 {
        if groups[0].is_empty()
            || groups[0].len() > 3
            || !groups[0].bytes().all(|byte| byte.is_ascii_digit())
            || groups[0].starts_with('0')
            || groups[1..]
                .iter()
                .any(|group| group.len() != 3 || !group.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
    } else if (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits = value.replace(',', "");
    digits.parse::<i64>().ok()
}

fn parse_current_cod(value: &str) -> Option<i64> {
    let value = value.strip_prefix("Current CoD:").or_else(|| value.strip_prefix("CoD:"))?.trim();
    let value = value.strip_suffix(" Credits").unwrap_or(value).trim();
    parse_canonical_credits(value)
}

fn parse_networth_credits(value: &str) -> Option<i64> {
    let value = value.strip_prefix("Credits")?.trim();
    parse_canonical_credits(value)
}

fn canonical_equipment_ids(values: &[String]) -> bool {
    values.iter().enumerate().all(|(index, value)| {
        valid_decimal_id(value) && (index == 0 || values[index - 1].as_str() < value.as_str())
    })
}

fn required_script_value(source: &str, name: &str) -> Option<i64> {
    let needle = format!("var {name}");
    let mut matches = source.match_indices(&needle);
    let first = matches.find(|(index, _)| {
        *index == 0
            || source[..*index]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_whitespace() || character == ';')
    })?;
    if matches.next().is_some() {
        return None;
    }
    let mut index = first.0 + needle.len();
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
        index += 1;
    }
    if source.as_bytes().get(index) != Some(&b'=') {
        return None;
    }
    index += 1;
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
        index += 1;
    }
    let start = index;
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_digit()) {
        index += 1;
    }
    if start == index {
        return None;
    }
    let value = source[start..index].parse::<i64>().ok()?;
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
        index += 1;
    }
    (source.as_bytes().get(index) == Some(&b';')).then_some(value)
}

fn script_text(
    document: &Html,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<String, MoogleMailError> {
    let selector = selector("script", operation, world)?;
    let mut source = String::new();
    for script in document.select(&selector) {
        source.push_str(&script.text().collect::<String>());
        source.push('\n');
    }
    Ok(source)
}

/// 把注释和字符串替换为空格，防止伪造的变量赋值或函数调用进入协议状态。
fn executable_javascript(source: &str) -> Option<String> {
    #[derive(Clone, Copy)]
    enum State {
        Code,
        Single,
        Double,
        Template,
        LineComment,
        BlockComment,
    }
    let mut state = State::Code;
    let mut escaped = false;
    let mut output = source.as_bytes().to_vec();
    let mut index = 0;
    while index < output.len() {
        let byte = source.as_bytes()[index];
        match state {
            State::Code => match byte {
                b'/' if source.as_bytes().get(index + 1) == Some(&b'/') => {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 1;
                    state = State::LineComment;
                }
                b'/' if source.as_bytes().get(index + 1) == Some(&b'*') => {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 1;
                    state = State::BlockComment;
                }
                b'\'' => {
                    output[index] = b' ';
                    state = State::Single;
                }
                b'"' => {
                    output[index] = b' ';
                    state = State::Double;
                }
                b'`' => {
                    output[index] = b' ';
                    state = State::Template;
                }
                _ => {}
            },
            State::LineComment => {
                if byte == b'\n' {
                    state = State::Code;
                } else {
                    output[index] = b' ';
                }
            }
            State::BlockComment => {
                if byte == b'*' && source.as_bytes().get(index + 1) == Some(&b'/') {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 1;
                    state = State::Code;
                } else if byte != b'\n' && byte != b'\r' {
                    output[index] = b' ';
                }
            }
            State::Single | State::Double | State::Template => {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if matches!(
                    (state, byte),
                    (State::Single, b'\'') | (State::Double, b'"') | (State::Template, b'`')
                ) {
                    state = State::Code;
                } else if (byte == b'\n' || byte == b'\r') && !matches!(state, State::Template) {
                    return None;
                }
                if !matches!(byte, b'\n' | b'\r') {
                    output[index] = b' ';
                }
            }
        }
        index += 1;
    }
    matches!(state, State::Code | State::LineComment)
        .then(|| String::from_utf8(output).ok())
        .flatten()
}

fn equipment_keys(
    document: &Html,
    body: &str,
    wanted: &HashSet<String>,
    world: MoogleMailWorld,
) -> Result<HashMap<String, MoogleMailEquipmentKey>, MoogleMailError> {
    let mut keys = body_equipment_keys(body, wanted, world)?;
    let mut script_keys = HashMap::new();
    let script_selector = selector("script", MoogleMailOperation::Detail, world)?;
    for script in document.select(&script_selector) {
        let source = script.text().collect::<String>();
        let Some(masked) = executable_javascript(&source) else {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        };
        let starts = masked.match_indices('{').map(|(index, _)| index).collect::<Vec<_>>();
        for start in starts {
            let candidate = &source[start..];
            if !wanted.iter().any(|equipment_id| has_json_property(candidate, equipment_id)) {
                continue;
            }
            let (found, saw_wanted) = decode_equipment_root(candidate, wanted)
                .map_err(|_| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if !saw_wanted {
                continue;
            }
            for (equipment_id, key) in found {
                if script_keys.insert(equipment_id.clone(), key).is_some() {
                    return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
                }
            }
        }
    }
    for (equipment_id, key) in script_keys {
        if let Some(body_key) = keys.get(&equipment_id) {
            if body_key != &key {
                return Err(detail_error(world, MoogleMailErrorCategory::IdentityMismatch));
            }
        } else {
            keys.insert(equipment_id, key);
        }
    }
    if keys.len() != wanted.len() || wanted.iter().any(|id| !keys.contains_key(id)) {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(keys)
}

fn body_equipment_keys(
    body: &str,
    wanted: &HashSet<String>,
    world: MoogleMailWorld,
) -> Result<HashMap<String, MoogleMailEquipmentKey>, MoogleMailError> {
    let mut keys = HashMap::new();
    let mut offset = 0;
    while let Some(relative) = body[offset..].find("/equip/") {
        let start = offset + relative + "/equip/".len();
        let mut index = start;
        while body.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_digit()) {
            index += 1;
        }
        if index == start || body.as_bytes().get(index) != Some(&b'/') {
            offset = start;
            continue;
        }
        let equipment_id = &body[start..index];
        let key_start = index + 1;
        let key_end = key_start.saturating_add(10);
        if key_end > body.len() || !valid_hex_key(&body[key_start..key_end]) {
            offset = key_start;
            continue;
        }
        if body.as_bytes().get(key_end).is_some_and(|byte| byte.is_ascii_hexdigit()) {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        if wanted.contains(equipment_id) {
            let key = MoogleMailEquipmentKey::new(&body[key_start..key_end])
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if keys.insert(equipment_id.to_owned(), key).is_some() {
                return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
        }
        offset = key_end;
    }
    Ok(keys)
}

fn has_json_property(source: &str, name: &str) -> bool {
    let needle = format!("\"{name}\"");
    let mut offset = 0;
    while offset < source.len() {
        let Some(relative) = source[offset..].find(&needle) else {
            return false;
        };
        let mut index = offset + relative + needle.len();
        while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
            index += 1;
        }
        if source.as_bytes().get(index) == Some(&b':') {
            return true;
        }
        offset = index;
    }
    false
}

fn decode_equipment_root(
    source: &str,
    wanted: &HashSet<String>,
) -> Result<(HashMap<String, MoogleMailEquipmentKey>, bool), ()> {
    let mut decoder = serde_json::Deserializer::from_str(source);
    decoder.deserialize_map(EquipmentRootVisitor { wanted }).map_err(|_| ())
}

struct EquipmentRootVisitor<'a> {
    wanted: &'a HashSet<String>,
}

impl<'de> Visitor<'de> for EquipmentRootVisitor<'_> {
    type Value = (HashMap<String, MoogleMailEquipmentKey>, bool);

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MoogleMail equipment JSON object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashMap::new();
        while let Some(name) = map.next_key::<String>()? {
            if self.wanted.contains(&name) {
                if keys.contains_key(&name) {
                    return Err(de::Error::custom("重复装备 JSON 字段"));
                }
                let item = map.next_value::<EquipmentJsonItem>()?;
                keys.insert(name, item.0);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        let saw_wanted = !keys.is_empty();
        Ok((keys, saw_wanted))
    }
}

struct EquipmentJsonItem(MoogleMailEquipmentKey);

struct EquipmentJsonItemVisitor;

impl<'de> Visitor<'de> for EquipmentJsonItemVisitor {
    type Value = EquipmentJsonItem;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("包含唯一 k 字段的装备对象")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut key = None;
        while let Some(name) = map.next_key::<String>()? {
            if name == "k" {
                if key.is_some() {
                    return Err(de::Error::custom("重复装备 key 字段"));
                }
                let value = map.next_value::<String>()?;
                let parsed = MoogleMailEquipmentKey::new(&value)
                    .ok_or_else(|| de::Error::custom("装备 key 格式无效"))?;
                key = Some(parsed);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        key.map(EquipmentJsonItem).ok_or_else(|| de::Error::custom("装备对象缺少 key 字段"))
    }
}

impl<'de> serde::Deserialize<'de> for EquipmentJsonItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(EquipmentJsonItemVisitor)
    }
}

fn extract_equipment_id(value: &str) -> Option<String> {
    let marker = "equips.set(";
    let matches = value.match_indices(marker).take(2).collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let (marker_start, _) = matches[0];
    if marker_start > 0
        && !value[..marker_start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_whitespace() || character == ';')
    {
        return None;
    }
    let start = marker_start + marker.len();
    let remainder = value[start..].trim_start();
    let end = remainder.find(|character: char| !character.is_ascii_digit())?;
    let id = &remainder[..end];
    (valid_decimal_id(id) && remainder[end..].trim_start().starts_with(',')).then(|| id.to_owned())
}

fn parse_detail_actions(
    envelope: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<(bool, bool), MoogleMailError> {
    let selector = selector("[onclick]", MoogleMailOperation::Detail, world)?;
    let mut returns = 0;
    let mut claims = 0;
    for action in envelope.select(&selector) {
        let value = action.value().attr("onclick").unwrap_or_default();
        let normalized =
            value.chars().filter(|character| !character.is_whitespace()).collect::<String>();
        if matches!(normalized.as_str(), "mooglemail.return_mail()" | "mooglemail.return_mail();") {
            returns += 1;
        } else if normalized.contains("return_mail") {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        if matches!(
            normalized.as_str(),
            "mooglemail.remove_attachment(0)" | "mooglemail.remove_attachment(0);"
        ) {
            claims += 1;
        } else if normalized.contains("remove_attachment") {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
    }
    if returns > 1 || claims > 1 {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok((returns == 1, claims == 1))
}

fn attribute_references_mail_id(value: &str, external_mail_id: &str) -> bool {
    let bytes = value.as_bytes();
    for (index, _) in value.match_indices("mid=") {
        if index != 0 && !matches!(bytes[index - 1], b'?' | b'&') {
            continue;
        }
        let start = index + 4;
        let end = value[start..]
            .find(|character: char| {
                matches!(character, '&' | '#' | '\'' | '"') || character.is_whitespace()
            })
            .map_or(value.len(), |offset| start + offset);
        if start == end {
            return true;
        }
        let Some(decoded) = decode_form_component(&value[start..end]) else {
            return true;
        };
        if !valid_opaque_id(&decoded) || decoded == external_mail_id {
            return true;
        }
    }
    false
}

fn extract_mid(value: &str) -> Option<String> {
    let mut found = None;
    let bytes = value.as_bytes();
    for (index, _) in value.match_indices("mid=") {
        if index != 0 && !matches!(bytes[index - 1], b'?' | b'&') {
            continue;
        }
        if found.is_some() {
            return None;
        }
        let start = index + 4;
        let end = value[start..]
            .find(|character: char| {
                matches!(character, '&' | '#' | '\'' | '"') || character.is_whitespace()
            })
            .map_or(value.len(), |offset| start + offset);
        if start == end {
            return None;
        }
        let encoded = format!("mid={}", &value[start..end]);
        let decoded = decode_form_component(encoded.strip_prefix("mid=")?)?;
        if !valid_opaque_id(&decoded) {
            return None;
        }
        found = Some(decoded);
    }
    found
}

fn decode_form_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' => {
                let high = hex_digit(*bytes.get(index + 1)?)?;
                let low = hex_digit(*bytes.get(index + 2)?)?;
                decoded.push((high << 4) | low);
                index += 2;
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn parse_page_time(
    value: &str,
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
) -> Result<DateTime<Utc>, MoogleMailError> {
    let naive = NaiveDateTime::parse_from_str(value, INBOX_TIME_FORMAT)
        .map_err(|_| protocol(operation, world))?;
    let offset = FixedOffset::west_opt(8 * 60 * 60).ok_or_else(|| protocol(operation, world))?;
    offset
        .from_local_datetime(&naive)
        .single()
        .map(|value| value.with_timezone(&Utc))
        .ok_or_else(|| protocol(operation, world))
}

fn valid_message_body(value: &str) -> bool {
    value.len() <= MAX_MESSAGE_TEXT_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

fn normalize_text(value: String) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn direct_children<'a>(element: ElementRef<'a>, tag: &str) -> Vec<ElementRef<'a>> {
    element
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|child| child.value().name() == tag)
        .collect()
}

fn direct_element_children(element: ElementRef<'_>) -> Vec<ElementRef<'_>> {
    element.children().filter_map(ElementRef::wrap).collect()
}

fn selector(
    value: &str,
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
) -> Result<Selector, MoogleMailError> {
    Selector::parse(value).map_err(|_| protocol(operation, world))
}

fn protocol(operation: MoogleMailOperation, world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(operation, world, MoogleMailErrorCategory::ProtocolDrift, false)
}

fn detail_error(world: MoogleMailWorld, category: MoogleMailErrorCategory) -> MoogleMailError {
    detail_error_for(MoogleMailOperation::Detail, world, category)
}

fn detail_error_for(
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(operation, world, category, false)
}

fn action_proof_error(operation: MoogleMailOperation, world: MoogleMailWorld) -> MoogleMailError {
    detail_error_for(operation, world, MoogleMailErrorCategory::WriteUnknown)
}

fn write_digest_string(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}
