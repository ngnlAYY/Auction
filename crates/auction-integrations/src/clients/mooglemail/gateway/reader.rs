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
        if config.maximum_pages == 0
            || config.maximum_pages > MAX_HARD_PAGES
            || config.fetch_budget.is_zero()
        {
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
        let client = Arc::new(client.as_ref().clone().with_ownership(credentials.ownership()));
        Ok(Self { client, credentials, config, parser: MoogleMailParser })
    }

    /// 读取同一运行时凭据的远端自身身份；None 仅表示没有配置预期绑定。
    ///
    /// 当前尚未取得可信认证自身页面协议，配置存在时明确返回 identity_unproven。
    /// 不读取 persona、草稿收件人或历史 Sent 代替证明，也不以余额检查标记 verified。
    /// 后续受支持解析器必须复用此会话、client、ownership 和 fetch_budget。
    pub async fn read_verified_player_identity(
        &self,
        world: MoogleMailWorld,
    ) -> Result<Option<auction_application::VerifiedMailPlayerIdentity>, MoogleMailError> {
        let session = self.credentials.begin(world)?;
        if !session.has_expected_player_name() {
            return Ok(None);
        }
        Err(operation_error(
            MoogleMailOperation::Inbox,
            world,
            MoogleMailErrorCategory::IdentityUnproven,
        ))
    }

    /// 读取一个 world 的未完成邮件窗口；cursor 是最后一封已完成邮件的 external_mail_id。
    pub async fn fetch(
        &self,
        world: MoogleMailWorld,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        self.fetch_with_deadline(world, cursor, limit, None).await
    }

    /// 使用调用方提供的绝对截止时刻读取 inbox。
    pub async fn fetch_with_deadline(
        &self,
        world: MoogleMailWorld,
        cursor: Option<&str>,
        limit: usize,
        requested_deadline: Option<std::time::Instant>,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        if limit == 0
            || limit > MAX_FETCH_LIMIT
            || cursor.is_some_and(|value| !valid_opaque_id(value))
        {
            return Err(configuration(world));
        }
        let deadline = match requested_deadline {
            Some(deadline) => tokio::time::Instant::from_std(deadline),
            None => tokio::time::Instant::now()
                .checked_add(self.config.fetch_budget)
                .ok_or_else(|| configuration(world))?,
        };
        let mut session = self.credentials.begin(world)?;
        for _ in 0..64 {
            match tokio::time::timeout_at(
                deadline,
                self.fetch_with_cookie(world, cursor, limit, session.cookie(), deadline),
            )
            .await
            {
                Err(_) => return Err(fetch_budget_exhausted(world)),
                Ok(Ok(page)) => return Ok(page),
                Ok(Err(error)) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    tokio::time::timeout_at(deadline, session.invalidate_current())
                        .await
                        .map_err(|_| fetch_budget_exhausted(world))??;
                    if session.advance().is_err() {
                        return Err(error);
                    }
                }
                Ok(Err(error)) => return Err(error),
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

    /// 在有界页数和绝对截止时间内按收件人、主题和装备剩余资产核对 Sent。
    pub async fn lookup_sent(
        &self,
        request: MoogleMailSentLookupRequest,
    ) -> Result<MoogleMailSentLookupReport, MoogleMailError> {
        if !request.validate(self.config.maximum_pages) {
            return Err(operation_error(
                MoogleMailOperation::Sent,
                request.world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let configured_deadline = std::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(request.world))?;
        let deadline = tokio::time::Instant::from_std(request.deadline.min(configured_deadline));
        let session = self.credentials.begin(request.world)?;
        self.lookup_sent_with_cookie(&request, session.cookie(), deadline).await
    }

    async fn lookup_sent_with_cookie(
        &self,
        request: &MoogleMailSentLookupRequest,
        cookie: &MoogleMailCookie,
        deadline: tokio::time::Instant,
    ) -> Result<MoogleMailSentLookupReport, MoogleMailError> {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for page_index in 0..request.maximum_pages {
            let target = self.client.sent_page(request.world, page_index)?;
            let document = tokio::time::timeout_at(deadline, self.client.get(&target, cookie))
                .await
                .map_err(|_| fetch_budget_exhausted(request.world))??;
            let page =
                self.parser.scan_sent_page(&document, request.world, page_index, &mut seen)?;
            if tokio::time::Instant::now() >= deadline {
                return Err(fetch_budget_exhausted(request.world));
            }
            for summary in page.summaries.iter().filter(|summary| {
                summary.recipient.eq_ignore_ascii_case(&request.recipient)
                    && summary.subject == request.subject
            }) {
                let detail_target =
                    self.client.sent_detail(request.world, &summary.external_mail_id)?;
                let detail_document =
                    tokio::time::timeout_at(deadline, self.client.get(&detail_target, cookie))
                        .await
                        .map_err(|_| fetch_budget_exhausted(request.world))??;
                let snapshot = self.parser.parse_sent_detail(
                    &detail_document,
                    summary,
                    request.observed_at,
                )?;
                if tokio::time::Instant::now() >= deadline {
                    return Err(fetch_budget_exhausted(request.world));
                }
                if snapshot.recipient.eq_ignore_ascii_case(&request.recipient)
                    && snapshot.subject == request.subject
                    && snapshot.remaining_credits == 0
                    && snapshot.cod_pending == 0
                    && (snapshot.collected
                        || snapshot.remaining_equipment_ids
                            == vec![request.external_equipment_id.clone()])
                {
                    candidates.push(snapshot);
                    if candidates.len() >= 2 {
                        return Ok(MoogleMailSentLookupReport {
                            status: MoogleMailSentLookupStatus::Multiple,
                            candidates,
                            pages_scanned: page_index + 1,
                            scan_complete: false,
                            observed_at: request.observed_at,
                        });
                    }
                }
            }
            if !page.has_next_page {
                let status = match candidates.as_slice() {
                    [] => MoogleMailSentLookupStatus::NotObserved,
                    [candidate] if candidate.collected => {
                        MoogleMailSentLookupStatus::CollectedUnproven
                    }
                    [_] => MoogleMailSentLookupStatus::Unique,
                    _ => MoogleMailSentLookupStatus::Multiple,
                };
                return Ok(MoogleMailSentLookupReport {
                    status,
                    candidates,
                    pages_scanned: page_index + 1,
                    scan_complete: true,
                    observed_at: request.observed_at,
                });
            }
        }
        Ok(MoogleMailSentLookupReport {
            status: MoogleMailSentLookupStatus::Incomplete,
            candidates,
            pages_scanned: request.maximum_pages,
            scan_complete: false,
            observed_at: request.observed_at,
        })
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
                self.client
                    .diagnose_parse(
                        &target,
                        &document,
                        self.parser.parse_balance(&document, world, observed_at),
                    )
                    .await
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
            let sent_page = self
                .client
                .diagnose_parse(
                    &target,
                    &document,
                    self.parser.scan_sent_page(&document, world, page_index, &mut seen),
                )
                .await?;
            if let Some(summary) = sent_page
                .summaries
                .iter()
                .find(|summary| summary.external_mail_id == external_mail_id)
            {
                let detail_target = self.client.sent_detail(world, external_mail_id)?;
                let detail_document = self.client.get(&detail_target, cookie).await?;
                let snapshot = self
                    .client
                    .diagnose_parse(
                        &detail_target,
                        &detail_document,
                        self.parser.parse_sent_detail(&detail_document, summary, observed_at),
                    )
                    .await?;
                if snapshot.external_mail_id != external_mail_id
                    || snapshot.world != world
                    || !snapshot.recipient.eq_ignore_ascii_case(&summary.recipient)
                    || snapshot.subject != summary.subject
                    || snapshot.sent_at != summary.sent_at
                {
                    return self
                        .client
                        .diagnose_parse(
                            &detail_target,
                            &detail_document,
                            Err(operation_error(
                                MoogleMailOperation::SentDetail,
                                world,
                                MoogleMailErrorCategory::IdentityMismatch,
                            )),
                        )
                        .await;
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
        deadline: tokio::time::Instant,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        let mut seen = HashSet::new();
        let mut window = VecDeque::with_capacity(limit);
        let mut found_cursor = false;
        let mut reached_end = false;
        for page_index in 0..self.config.maximum_pages {
            if deadline.checked_duration_since(tokio::time::Instant::now()).is_none() {
                return Err(fetch_budget_exhausted(world));
            }
            let target = self.client.inbox_page(world, page_index)?;
            let document = self.client.get(&target, cookie).await?;
            let inbox_page = self
                .client
                .diagnose_parse(
                    &target,
                    &document,
                    self.parser.scan_inbox_page(&document, world, page_index, &mut seen),
                )
                .await?;
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
        let had_candidates = !window.is_empty();
        let detail_deadline =
            deadline.checked_sub(std::time::Duration::from_millis(100)).unwrap_or(deadline);
        let mut messages = Vec::with_capacity(window.len());
        for summary in window.into_iter().rev() {
            let target = self.client.detail(world, &summary.external_mail_id)?;
            let document =
                match tokio::time::timeout_at(detail_deadline, self.client.get(&target, cookie))
                    .await
                {
                    Ok(Ok(document)) => document,
                    Err(_) => break,
                    Ok(Err(error)) => return Err(error),
                };
            let message = self
                .client
                .diagnose_parse(&target, &document, self.parser.parse_detail(&document, &summary))
                .await?;
            if message.external_mail_id != summary.external_mail_id
                || message.world != summary.world
                || message.received_at != summary.received_at
                || !message.sender.eq_ignore_ascii_case(&summary.sender)
            {
                return self
                    .client
                    .diagnose_parse(
                        &target,
                        &document,
                        Err(MoogleMailError::new(
                            MoogleMailOperation::Detail,
                            world,
                            MoogleMailErrorCategory::IdentityMismatch,
                            false,
                        )),
                    )
                    .await;
            }
            messages.push(message);
        }
        if messages.is_empty() && !had_candidates {
            return Ok(MoogleMailPage { messages, next_cursor: cursor.map(str::to_owned) });
        }
        if messages.is_empty() {
            return Err(fetch_budget_exhausted(world));
        }
        // 没有任何 allowlist 命中的详情时不能推进 cursor，否则后续补充 allowlist 会永久错过
        // 已经扫描过的邮件；有成功解析的邮件时才允许跳过同一窗口内明确忽略的发件人。
        let next_cursor = messages.last().map(|message| message.external_mail_id.clone());
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

fn fetch_budget_exhausted(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::RemoteTemporary,
        true,
    )
}
