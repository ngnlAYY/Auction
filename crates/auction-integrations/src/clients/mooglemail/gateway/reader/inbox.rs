//! 收件窗口读取：持久读取日志、前缀/完整列表、逐封详情与游标推进。
//!
//! 游标只能取详情身份与完整发件人白名单都已通过的连续前缀；任何解析失败、
//! 待读摘要失配或未授权发件人都失败关闭，不越过可能含资产的邮件。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Duration,
};

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::mail::{MailPollProgressStage, MailPollReadSummary};

use crate::safe_diagnostics;

use super::super::super::protocol::valid_opaque_id;
use super::error::{fetch_budget_exhausted, read_journal_unavailable};
use super::window::pending_anchor;
use super::*;

impl MoogleMailGateway {
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
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "acquire_inbox_session_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })??;
        match tokio::time::timeout_at(
            deadline,
            self.fetch_with_cookie(world, cursor, limit, session.cookie(), deadline, journal),
        )
        .await
        {
            Err(diagnostic_cause) => {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "fetch_inbox_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                Err(fetch_budget_exhausted(world))
            }
            Ok(Ok(page)) => {
                tokio::time::timeout_at(deadline, session.check_current()).await.map_err(
                    |diagnostic_cause| {
                        safe_diagnostics::warn(
                            "mooglemail_read",
                            "check_inbox_owner_timeout",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext {
                                world: diagnostic_world(world.as_str()),
                                ..Default::default()
                            },
                        );
                        fetch_budget_exhausted(world)
                    },
                )??;
                if tokio::time::Instant::now() >= deadline {
                    return Err(fetch_budget_exhausted(world));
                }
                Ok(page)
            }
            Ok(Err(error)) if error.category == MoogleMailErrorCategory::SessionExpired => {
                tokio::time::timeout_at(deadline, session.invalidate_current()).await.map_err(
                    |diagnostic_cause| {
                        safe_diagnostics::warn(
                            "mooglemail_read",
                            "invalidate_inbox_session_timeout",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext {
                                world: diagnostic_world(world.as_str()),
                                ..Default::default()
                            },
                        );
                        fetch_budget_exhausted(world)
                    },
                )??;
                Err(error)
            }
            Ok(Err(error)) => Err(error),
        }
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
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_read",
                        "claim_read_journal",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    read_journal_unavailable(world)
                })?;
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
        if recovering {
            // 恢复窗口的待读 MID 已知：最低 MID 就是自然下界，读到它即可停止翻页。
            let anchor = pending_anchor(&window);
            // 索引只借用本批 pending，按列表线性定位，避免每个 MID 重扫整个邮箱。
            let mut pending: HashMap<_, _> =
                window.iter().map(|summary| (summary.external_mail_id.as_str(), summary)).collect();
            let inbox = self.recovery_rows(world, cookie, &round, false, anchor.as_ref()).await?;
            for summary in inbox.summaries() {
                if let Some(expected) = pending.remove(summary.external_mail_id.as_str())
                    && (summary.world != expected.world
                        || summary.received_at != expected.received_at)
                {
                    return Err(protocol_error(world));
                }
            }
            if !pending.is_empty() {
                let read = self.recovery_rows(world, cookie, &round, true, anchor.as_ref()).await?;
                for summary in read.summaries() {
                    if let Some(expected) = pending.remove(summary.external_mail_id.as_str())
                        && (summary.world != expected.world
                            || summary.received_at != expected.received_at)
                    {
                        return Err(protocol_error(world));
                    }
                }
            }
            // 历史列表只定位既有账本 MID；两表都缺失仍按原 MID 读取，不删除或新增 pending。
        } else {
            // 列表显示名不代表完整身份；窗口只是候选，白名单与内容证明在详情解析后判断。
            window = self.select_incoming_window(world, cookie, &round, cursor, limit).await?;
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
            store.stage_reads(claim.clone(), world.as_str().to_owned(), summaries).await.map_err(
                |diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_read",
                        "stage_read_journal",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    read_journal_unavailable(world)
                },
            )?;
        }
        let detail_deadline = deadline.checked_sub(Duration::from_millis(100)).unwrap_or(deadline);
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
                Err(diagnostic_cause) => {
                    safe_diagnostics::warn(
                        "mooglemail_read",
                        "fetch_detail_timeout",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    break;
                }
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
