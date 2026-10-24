//! MoogleMail 收件、Sent 和余额的有界读取流程。

use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, Utc};

use super::super::protocol::valid_opaque_id;
use super::*;
use crate::clients::mooglemail::parser::subject_is_list_projection;
use auction_application::{
    MailPollClaim, MailPollProgressObserver, MailPollProgressStage, MailPollReadJournal,
    MailPollReadSummary,
};

type ReadJournalContext<'a> = (&'a dyn MailPollReadJournal, &'a MailPollClaim);

/// MoogleMail 收件箱读取 gateway。
pub struct MoogleMailGateway {
    pub(super) client: Arc<MoogleMailClient>,
    pub(super) credentials: MoogleMailCredentialSet,
    pub(super) config: MoogleMailGatewayConfig,
    pub(super) parser: MoogleMailParser,
    pub(super) profile_reader: Option<Arc<dyn auction_application::ForumTargetProfileReader>>,
    progress_observer: Option<Arc<dyn MailPollProgressObserver>>,
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
        Ok(Self {
            client,
            credentials,
            config,
            parser: MoogleMailParser,
            profile_reader: None,
            progress_observer: None,
        })
    }

    /// 注入进程内观测；仅持有 claim 的轮询路径报告真实读取目标，不参与协议或授权决定。
    pub fn with_progress_observer(mut self, observer: Arc<dyn MailPollProgressObserver>) -> Self {
        self.progress_observer = Some(observer);
        self
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
        self.fetch_window(world, cursor, limit, requested_deadline, None).await
    }

    /// 持久轮询专用入口：先登记列表身份，已读但未提交的邮件按 MID 优先恢复。
    pub async fn fetch_claimed(
        &self,
        world: MoogleMailWorld,
        limit: usize,
        requested_deadline: Option<std::time::Instant>,
        journal: &dyn MailPollReadJournal,
        claim: &MailPollClaim,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        if !claim.validate() {
            return Err(configuration(world));
        }
        self.fetch_window(
            world,
            claim.cursor_value.as_deref(),
            limit,
            requested_deadline,
            Some((journal, claim)),
        )
        .await
    }

    async fn fetch_window(
        &self,
        world: MoogleMailWorld,
        cursor: Option<&str>,
        limit: usize,
        requested_deadline: Option<std::time::Instant>,
        journal: Option<ReadJournalContext<'_>>,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        if limit == 0
            || limit > MAX_FETCH_LIMIT
            || cursor.is_some_and(|value| !valid_opaque_id(value))
        {
            return Err(configuration(world));
        }
        // 空白名单且未显式放行所有人时不可能授权任何详情，避免无意义地改变远端已读状态。
        // 拒绝拉取但保留原队列与游标，等待配置修正；不以列表显示名推断任何发件人身份。
        if !self.config.sender_allowlist_disabled && self.config.sender_allowlist.is_empty() {
            return Err(configuration(world));
        }
        let configured_deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let deadline = requested_deadline.map_or(configured_deadline, |deadline| {
            tokio::time::Instant::from_std(deadline).min(configured_deadline)
        });
        let mut session = tokio::time::timeout_at(
            deadline,
            self.credentials.begin_read(world, MoogleMailOperation::Inbox),
        )
        .await
        .map_err(|_| fetch_budget_exhausted(world))??;
        match tokio::time::timeout_at(
            deadline,
            self.fetch_with_cookie(world, cursor, limit, session.cookie(), deadline, journal),
        )
        .await
        {
            Err(_) => Err(fetch_budget_exhausted(world)),
            Ok(Ok(page)) => {
                session.check_current().await?;
                Ok(page)
            }
            Ok(Err(error)) if error.category == MoogleMailErrorCategory::SessionExpired => {
                tokio::time::timeout_at(deadline, session.invalidate_current())
                    .await
                    .map_err(|_| fetch_budget_exhausted(world))??;
                Err(error)
            }
            Ok(Err(error)) => Err(error),
        }
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
        let mut session = self.credentials.begin_read(world, MoogleMailOperation::Sent).await?;
        match self
            .read_sent_with_cookie(world, external_mail_id, observed_at, session.cookie(), false)
            .await
        {
            Ok(snapshot) => {
                session.check_current().await?;
                Ok(snapshot)
            }
            Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                session.invalidate_current().await?;
                Err(error)
            }
            Err(error) => Err(error),
        }
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
        let session = tokio::time::timeout_at(
            deadline,
            self.credentials.begin_read(request.world, MoogleMailOperation::Sent),
        )
        .await
        .map_err(|_| fetch_budget_exhausted(request.world))??;
        let report = self.lookup_sent_with_cookie(&request, session.cookie(), deadline).await?;
        session.check_current().await?;
        Ok(report)
    }

    async fn lookup_sent_with_cookie(
        &self,
        request: &MoogleMailSentLookupRequest,
        cookie: &MoogleMailCookie,
        deadline: tokio::time::Instant,
    ) -> Result<MoogleMailSentLookupReport, MoogleMailError> {
        let round = self.client.read_round(cookie, request.world, false)?;
        let list = tokio::time::timeout_at(
            deadline,
            self.client.observe_sent_list(cookie, &round, request.maximum_pages),
        )
        .await
        .map_err(|_| fetch_budget_exhausted(request.world))??;
        let mut candidates = Vec::new();
        for summary in list.summaries.iter().filter(|summary| {
            summary.recipient.eq_ignore_ascii_case(&request.recipient)
                && subject_is_list_projection(&summary.subject, &request.subject)
        }) {
            let detail_target =
                self.client.sent_detail(request.world, &summary.external_mail_id)?;
            let document = tokio::time::timeout_at(
                deadline,
                self.client.get_in_round(&detail_target, cookie, &round),
            )
            .await
            .map_err(|_| fetch_budget_exhausted(request.world))??;
            let snapshot = self
                .client
                .diagnose_parse(
                    &detail_target,
                    &document,
                    self.parser.parse_sent_detail(
                        &document,
                        summary,
                        request.observed_at.min(round.observed_at()),
                    ),
                )
                .await?;
            if tokio::time::Instant::now() >= deadline {
                return Err(fetch_budget_exhausted(request.world));
            }
            if snapshot.recipient.eq_ignore_ascii_case(&request.recipient)
                && snapshot.subject == request.subject
                && snapshot.remaining_credits == 0
                && snapshot.cod_pending == 0
                && (snapshot.collected
                    || snapshot.remaining_equipment_ids.as_slice()
                        == std::slice::from_ref(&request.external_equipment_id))
            {
                candidates.push(snapshot);
                if candidates.len() >= 2 {
                    break;
                }
            }
        }
        self.client.check_read_round(cookie, &round).await?;
        if tokio::time::Instant::now() >= deadline {
            return Err(fetch_budget_exhausted(request.world));
        }
        let status = if candidates.len() >= 2 {
            MoogleMailSentLookupStatus::Multiple
        } else if !list.complete {
            MoogleMailSentLookupStatus::Incomplete
        } else {
            match candidates.as_slice() {
                [] => MoogleMailSentLookupStatus::NotObserved,
                [candidate] if candidate.collected => MoogleMailSentLookupStatus::CollectedUnproven,
                _ => MoogleMailSentLookupStatus::Unique,
            }
        };
        Ok(MoogleMailSentLookupReport {
            status,
            candidates,
            pages_scanned: list.pages_scanned,
            scan_complete: list.complete,
            observed_at: request.observed_at.min(round.observed_at()),
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
        let mut session = self.credentials.begin_read(world, MoogleMailOperation::Balance).await?;
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
            Ok(snapshot) => {
                session.check_current().await?;
                Ok(snapshot)
            }
            Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                session.invalidate_current().await?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) async fn read_sent_with_cookie(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        observed_at: DateTime<Utc>,
        cookie: &MoogleMailCookie,
        fresh: bool,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        let round = self.client.read_round(cookie, world, fresh)?;
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        tokio::time::timeout_at(deadline, async {
            let list =
                self.client.observe_sent_list(cookie, &round, self.config.maximum_pages).await?;
            if !list.complete {
                return Err(operation_error(
                    MoogleMailOperation::Sent,
                    world,
                    MoogleMailErrorCategory::ProtocolDrift,
                ));
            }
            let summary = list
                .summaries
                .iter()
                .find(|summary| summary.external_mail_id == external_mail_id)
                .ok_or_else(|| {
                    operation_error(
                        MoogleMailOperation::Sent,
                        world,
                        MoogleMailErrorCategory::NotObserved,
                    )
                })?;
            let target = self.client.sent_detail(world, external_mail_id)?;
            let document = self.client.get_in_round(&target, cookie, &round).await?;
            let snapshot = self
                .client
                .diagnose_parse(
                    &target,
                    &document,
                    self.parser.parse_sent_detail(
                        &document,
                        summary,
                        observed_at.min(round.observed_at()),
                    ),
                )
                .await?;
            if snapshot.external_mail_id != external_mail_id
                || snapshot.world != world
                || !snapshot.recipient.eq_ignore_ascii_case(&summary.recipient)
                || !subject_is_list_projection(&summary.subject, &snapshot.subject)
                || snapshot.sent_at != summary.sent_at
            {
                return self
                    .client
                    .diagnose_parse(
                        &target,
                        &document,
                        Err(operation_error(
                            MoogleMailOperation::SentDetail,
                            world,
                            MoogleMailErrorCategory::IdentityMismatch,
                        )),
                    )
                    .await;
            }
            self.client.check_read_round(cookie, &round).await?;
            Ok(snapshot)
        })
        .await
        .map_err(|_| fetch_budget_exhausted(world))?
    }

    async fn fetch_with_cookie(
        &self,
        world: MoogleMailWorld,
        cursor: Option<&str>,
        limit: usize,
        cookie: &MoogleMailCookie,
        deadline: tokio::time::Instant,
        journal: Option<ReadJournalContext<'_>>,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        self.report_poll_progress(journal, MailPollProgressStage::Fetching, None);
        let round = self.client.read_round(cookie, world, false)?;
        let mut seen = HashSet::new();
        let mut window = VecDeque::with_capacity(limit);
        if let Some((store, claim)) = journal {
            let pending = store
                .pending_reads(
                    claim.clone(),
                    world.as_str().to_owned(),
                    u16::try_from(limit).map_err(|_| configuration(world))?,
                )
                .await
                .map_err(|_| read_journal_unavailable(world))?;
            if pending.len() > limit {
                return Err(protocol_error(world));
            }
            for summary in pending.into_iter().rev() {
                if !summary.validate()
                    || summary.world_id != world.as_str()
                    || !seen.insert(summary.external_mail_id.clone())
                {
                    return Err(protocol_error(world));
                }
                let summary = MoogleMailSummary {
                    external_mail_id: summary.external_mail_id,
                    world,
                    sender: summary.sender,
                    subject: summary.subject,
                    received_at: summary.received_at,
                };
                if !summary.validate() {
                    return Err(configuration(world));
                }
                window.push_back(summary);
            }
        }
        let recovering = !window.is_empty();
        // 必须先扫完列表，再允许任何详情 GET 改变 Inbox/Read Mail 成员关系。
        let inbox = self
            .client
            .observe_incoming_list(cookie, &round, self.config.maximum_pages, false)
            .await?;
        if !inbox.complete {
            return Err(protocol_error(world));
        }
        if recovering {
            let missing_from_inbox = window.iter().any(|pending| {
                !inbox
                    .summaries
                    .iter()
                    .any(|summary| summary.external_mail_id == pending.external_mail_id)
            });
            let read = if missing_from_inbox {
                let read = self
                    .client
                    .observe_incoming_list(cookie, &round, self.config.maximum_pages, true)
                    .await?;
                if !read.complete {
                    return Err(protocol_error(world));
                }
                Some(read)
            } else {
                None
            };
            for pending in &window {
                let found = inbox
                    .summaries
                    .iter()
                    .find(|summary| summary.external_mail_id == pending.external_mail_id)
                    .or_else(|| {
                        read.as_ref().and_then(|list| {
                            list.summaries.iter().find(|summary| {
                                summary.external_mail_id == pending.external_mail_id
                            })
                        })
                    });
                if found.is_some_and(|summary| {
                    summary.world != pending.world || summary.received_at != pending.received_at
                }) {
                    return Err(protocol_error(world));
                }
                // 历史列表只定位既有账本 MID；不把其他 Read Mail 行加入待处理窗口。
                // 两个列表都缺失仍按原 MID 读取并核验，绝不丢弃持久 pending。
            }
        } else {
            for summary in &inbox.summaries {
                if cursor.is_some_and(|value| value == summary.external_mail_id) {
                    break;
                }
                // 列表显示名不代表完整身份；先有界保留最旧窗口，白名单只能在详情解析后判断。
                // 窗口满时丢弃队首，避免 backlog 超过 limit 时跳过尚未处理的旧邮件。
                if window.len() == limit {
                    window.pop_front();
                }
                window.push_back(summary.clone());
            }
        }
        let had_candidates = !window.is_empty();
        if had_candidates
            && !recovering
            && let Some((store, claim)) = journal
        {
            let summaries = window
                .iter()
                .rev()
                .map(|summary| MailPollReadSummary {
                    external_mail_id: summary.external_mail_id.clone(),
                    world_id: world.as_str().to_owned(),
                    sender: summary.sender.clone(),
                    subject: summary.subject.clone(),
                    received_at: summary.received_at,
                })
                .collect();
            // 原子登记整窗后才允许首个详情 GET；取消或提交失败时队列仍可恢复。
            store
                .stage_reads(claim.clone(), world.as_str().to_owned(), summaries)
                .await
                .map_err(|_| read_journal_unavailable(world))?;
        }
        let detail_deadline =
            deadline.checked_sub(std::time::Duration::from_millis(100)).unwrap_or(deadline);
        let mut messages = Vec::with_capacity(window.len());
        for summary in window.into_iter().rev() {
            self.report_poll_progress(
                journal,
                MailPollProgressStage::ReadingDetail,
                Some(&summary.external_mail_id),
            );
            let target = self.client.detail(world, &summary.external_mail_id)?;
            let document = match tokio::time::timeout_at(
                detail_deadline,
                self.client.get_in_round(&target, cookie, &round),
            )
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
            // 业务身份与白名单只使用详情完整发件人；旧待读摘要的截断名称不参与授权。
            // 不匹配时保留队列并失败关闭，不越过可能含资产的邮件，也不执行任何业务命令。
            if !self.allowed_sender(&message.sender) {
                return Err(configuration(world));
            }
            messages.push(message);
        }
        self.client.check_read_round(cookie, &round).await?;
        if messages.is_empty() && !had_candidates {
            return Ok(MoogleMailPage { messages, next_cursor: cursor.map(str::to_owned) });
        }
        if messages.is_empty() {
            return Err(fetch_budget_exhausted(world));
        }
        // cursor 只取详情完整性与完整发件人白名单均已通过的连续前缀，不跳过未获授权的邮件。
        let next_cursor = messages.last().map(|message| message.external_mail_id.clone());
        Ok(MoogleMailPage { messages, next_cursor })
    }

    fn report_poll_progress(
        &self,
        journal: Option<ReadJournalContext<'_>>,
        stage: MailPollProgressStage,
        mail_id: Option<&str>,
    ) {
        if let (Some(observer), Some((_, claim))) = (&self.progress_observer, journal) {
            observer.update(claim, stage, mail_id);
        }
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

fn read_journal_unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::PersistenceUnavailable,
        true,
    )
}

fn fetch_budget_exhausted(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::RemoteTemporary,
        true,
    )
}
