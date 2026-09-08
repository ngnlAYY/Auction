//! MoogleMail 收件、Sent 和余额的有界读取流程。

use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, Utc};

use super::super::protocol::valid_opaque_id;
use super::*;

/// MoogleMail 收件箱读取 gateway。
pub struct MoogleMailGateway {
    client: Arc<MoogleMailClient>,
    credentials: MoogleMailCredentialSet,
    config: MoogleMailGatewayConfig,
    parser: MoogleMailParser,
}

impl MoogleMailGateway {
    /// 构造一个固定 world 由调用方传入的通用 gateway。
    pub fn new(
        client: Arc<MoogleMailClient>,
        credentials: MoogleMailCredentialSet,
        config: MoogleMailGatewayConfig,
    ) -> Result<Self, MoogleMailError> {
        if config.maximum_pages == 0 || config.maximum_pages > MAX_HARD_PAGES {
            return Err(configuration(MoogleMailWorld::Main));
        }
        for (index, sender) in config.sender_allowlist.iter().enumerate() {
            if sender.is_empty()
                || sender != sender.trim()
                || sender.len() > 256
                || sender.chars().any(char::is_control)
                || config.sender_allowlist[..index]
                    .iter()
                    .any(|previous| previous.eq_ignore_ascii_case(sender))
            {
                return Err(configuration(MoogleMailWorld::Main));
            }
        }
        Ok(Self { client, credentials, config, parser: MoogleMailParser })
    }

    /// 读取一个 world 的未完成邮件窗口；cursor 是最后一封已完成邮件的 external_mail_id。
    pub async fn fetch(
        &self,
        world: MoogleMailWorld,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        if limit == 0
            || limit > MAX_FETCH_LIMIT
            || cursor.is_some_and(|value| !valid_opaque_id(value))
        {
            return Err(configuration(world));
        }
        let mut session = self.credentials.begin(world)?;
        for _ in 0..64 {
            match self.fetch_with_cookie(world, cursor, limit, session.cookie()).await {
                Ok(page) => return Ok(page),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    if session.advance().is_err() {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(operation_error(
            MoogleMailOperation::Inbox,
            world,
            MoogleMailErrorCategory::CredentialUnavailable,
        ))
    }

    /// 按 exact remote ID 扫描 world-specific Sent 列表并读取详情证明。
    pub async fn read_sent(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        if !valid_opaque_id(external_mail_id) || observed_at.timestamp_nanos_opt().is_none() {
            return Err(operation_error(
                MoogleMailOperation::Sent,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let mut session = self.credentials.begin(world)?;
        for _ in 0..64 {
            match self
                .read_sent_with_cookie(world, external_mail_id, observed_at, session.cookie())
                .await
            {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    if session.advance().is_err() {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(operation_error(
            MoogleMailOperation::Sent,
            world,
            MoogleMailErrorCategory::CredentialUnavailable,
        ))
    }

    /// 读取并校验一个 world 的 Item Shop Credits 余额。
    pub async fn read_balance(
        &self,
        world: MoogleMailWorld,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailBalanceSnapshot, MoogleMailError> {
        if observed_at.timestamp_nanos_opt().is_none() {
            return Err(operation_error(
                MoogleMailOperation::Balance,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let mut session = self.credentials.begin(world)?;
        for _ in 0..64 {
            let result = async {
                let target = self.client.balance(world)?;
                let document = self.client.get(&target, session.cookie()).await?;
                self.parser.parse_balance(&document, world, observed_at)
            }
            .await;
            match result {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    if session.advance().is_err() {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(operation_error(
            MoogleMailOperation::Balance,
            world,
            MoogleMailErrorCategory::CredentialUnavailable,
        ))
    }

    async fn read_sent_with_cookie(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        observed_at: DateTime<Utc>,
        cookie: &MoogleMailCookie,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        let mut seen = HashSet::new();
        for page_index in 0..self.config.maximum_pages {
            let target = self.client.sent_page(world, page_index)?;
            let document = self.client.get(&target, cookie).await?;
            let sent_page = self.parser.scan_sent_page(&document, world, page_index, &mut seen)?;
            if let Some(summary) = sent_page
                .summaries
                .iter()
                .find(|summary| summary.external_mail_id == external_mail_id)
            {
                let detail_target = self.client.sent_detail(world, external_mail_id)?;
                let detail_document = self.client.get(&detail_target, cookie).await?;
                let snapshot =
                    self.parser.parse_sent_detail(&detail_document, summary, observed_at)?;
                if snapshot.external_mail_id != external_mail_id
                    || snapshot.world != world
                    || !snapshot.recipient.eq_ignore_ascii_case(&summary.recipient)
                    || snapshot.subject != summary.subject
                    || snapshot.sent_at != summary.sent_at
                {
                    return Err(operation_error(
                        MoogleMailOperation::SentDetail,
                        world,
                        MoogleMailErrorCategory::IdentityMismatch,
                    ));
                }
                return Ok(snapshot);
            }
            if !sent_page.has_next_page {
                return Err(operation_error(
                    MoogleMailOperation::Sent,
                    world,
                    MoogleMailErrorCategory::NotObserved,
                ));
            }
        }
        Err(operation_error(
            MoogleMailOperation::Sent,
            world,
            MoogleMailErrorCategory::ProtocolDrift,
        ))
    }

    async fn fetch_with_cookie(
        &self,
        world: MoogleMailWorld,
        cursor: Option<&str>,
        limit: usize,
        cookie: &MoogleMailCookie,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        let mut seen = HashSet::new();
        let mut window = VecDeque::with_capacity(limit);
        let mut found_cursor = false;
        let mut reached_end = false;
        for page_index in 0..self.config.maximum_pages {
            let target = self.client.inbox_page(world, page_index)?;
            let document = self.client.get(&target, cookie).await?;
            let inbox_page =
                self.parser.scan_inbox_page(&document, world, page_index, &mut seen)?;
            for summary in inbox_page.summaries {
                if found_cursor {
                    continue;
                }
                if cursor.is_some_and(|value| value == summary.external_mail_id) {
                    found_cursor = true;
                    continue;
                }
                // allowlist 必须在占用详情配额前生效，不能让大量无关发件人挤掉真正需要解析的邮件。
                if self.allowed_sender(&summary.sender) {
                    // 页面按远端收件箱顺序扫描；窗口满时丢弃队首，保留游标之前最旧的
                    // limit 条，避免 backlog 超过 limit 时跳过尚未处理的旧邮件。
                    if window.len() == limit {
                        window.pop_front();
                    }
                    window.push_back(summary);
                }
            }
            if found_cursor {
                break;
            }
            if !inbox_page.has_next_page {
                reached_end = true;
                break;
            }
        }
        if !found_cursor && !reached_end {
            return Err(protocol_error(world));
        }
        let newest = window.front().map(|summary| summary.external_mail_id.clone());
        let mut messages = Vec::with_capacity(window.len());
        for summary in window.into_iter().rev() {
            let target = self.client.detail(world, &summary.external_mail_id)?;
            let document = self.client.get(&target, cookie).await?;
            let message = self.parser.parse_detail(&document, &summary)?;
            if message.external_mail_id != summary.external_mail_id
                || message.world != summary.world
                || message.received_at != summary.received_at
                || !message.sender.eq_ignore_ascii_case(&summary.sender)
            {
                return Err(MoogleMailError::new(
                    MoogleMailOperation::Detail,
                    world,
                    MoogleMailErrorCategory::IdentityMismatch,
                    false,
                ));
            }
            messages.push(message);
        }
        // 没有任何 allowlist 命中的详情时不能推进 cursor，否则后续补充 allowlist 会永久错过
        // 已经扫描过的邮件；有成功解析的邮件时才允许跳过同一窗口内明确忽略的发件人。
        let next_cursor = if messages.is_empty() { cursor.map(str::to_owned) } else { newest };
        Ok(MoogleMailPage { messages, next_cursor })
    }

    fn allowed_sender(&self, sender: &str) -> bool {
        self.config.sender_allowlist_disabled
            || self
                .config
                .sender_allowlist
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(sender))
    }
}
