//! 收件窗口读取：持久读取日志、前缀/完整列表、逐封详情与游标推进。
//!
//! 游标只能取详情身份与完整发件人白名单都已通过的连续前缀；任何解析失败、
//! 待读摘要失配或未授权发件人都失败关闭，不越过可能含资产的邮件。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Duration,
};

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::mail::{MailPollClaim, MailPollReadJournal};
use auction_application::mail::{MailPollProgressStage, MailPollReadSummary};

use super::super::super::protocol::valid_opaque_id;
use super::super::MoogleMailCredentialSession;
use super::consignment_candidate::{
    DetailRoute, bind_candidate, bind_unparsed_candidate, detail_route, list_is_candidate,
    pending_error,
};
use super::error::{fetch_budget_exhausted, read_journal_unavailable};
use super::window::pending_anchor;
use super::{MoogleMailGateway, ReadJournalContext};
use crate::clients::mooglemail::error::{MoogleMailError, MoogleMailErrorCategory};
use crate::clients::mooglemail::gateway::model::{
    MoogleMailConsignmentCandidate, MoogleMailPage, MoogleMailPageMessage,
};
use crate::clients::mooglemail::gateway::{MAX_FETCH_LIMIT, configuration, protocol_error};
use crate::clients::mooglemail::parser::MoogleMailSummary;
use crate::clients::mooglemail::protocol::{MoogleMailOperation, MoogleMailWorld};
use crate::safe_diagnostics;

struct ConsignmentFetch<'a> {
    session: &'a MoogleMailCredentialSession,
    candidates: bool,
}

/// 测试观察：门控打开且列表投影命中候选时，进入 4D1 候选分流的次数。
#[cfg(test)]
pub(crate) static CANDIDATE_ROUTE_DECISIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// 从持久待读快照重建列表摘要；旧版行缺少列表字段时按既有方式推导，
/// 新行必须原样还原首次读取的标记与原始分钟。
fn summary_from_read_snapshot(
    summary: MailPollReadSummary,
    world: MoogleMailWorld,
) -> Result<MoogleMailSummary, MoogleMailError> {
    let world_marker = match summary.list_world_marker.as_deref() {
        Some("main") => Some(MoogleMailWorld::Main),
        Some("isekai") => Some(MoogleMailWorld::Isekai),
        Some(_) => return Err(protocol_error(world, MoogleMailOperation::Inbox)),
        None => None,
    };
    let received_minute_raw = summary
        .received_minute_raw
        .unwrap_or_else(|| summary.received_at.strftime("%Y-%m-%d %H:%M").to_string());
    let summary = MoogleMailSummary {
        external_mail_id: summary.external_mail_id,
        world,
        sender: summary.sender,
        subject: summary.subject,
        received_at: summary.received_at,
        received_minute_raw,
        world_marker,
    };
    if !summary.validate() {
        return Err(configuration(world, MoogleMailOperation::Inbox));
    }
    Ok(summary)
}

/// 门控与列表投影必须同时成立才允许进入 4D1 候选分流。
fn candidate_route_enabled(consignment_candidates: bool, sender: &str, subject: &str) -> bool {
    consignment_candidates && list_is_candidate(sender, subject)
}

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
        self.fetch_window(world, cursor, limit, requested_deadline, None, false).await
    }

    /// 持久轮询专用入口：先登记列表身份，已读但未提交的邮件按 MID 优先恢复。
    pub async fn fetch_claimed(
        &self,
        world: MoogleMailWorld,
        limit: usize,
        requested_deadline: Option<std::time::Instant>,
        journal: &dyn MailPollReadJournal,
        claim: &MailPollClaim,
        consignment_candidates: bool,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        if !claim.validate() {
            return Err(configuration(world, MoogleMailOperation::Inbox));
        }
        self.fetch_window(
            world,
            claim.cursor_value.as_deref(),
            limit,
            requested_deadline,
            Some((journal, claim)),
            consignment_candidates,
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
        consignment_candidates: bool,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        if limit == 0
            || limit > MAX_FETCH_LIMIT
            || cursor.is_some_and(|value| !valid_opaque_id(value))
        {
            return Err(configuration(world, MoogleMailOperation::Inbox));
        }
        // 空白名单且未显式放行所有人时不可能授权任何详情，避免无意义地改变远端已读状态。
        // 拒绝拉取但保留原队列与游标，等待配置修正；不以列表显示名推断任何发件人身份。
        if !self.config.sender_allowlist_disabled && self.config.sender_allowlist.is_empty() {
            return Err(configuration(world, MoogleMailOperation::Inbox));
        }
        let configured_deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world, MoogleMailOperation::Inbox))?;
        let deadline = requested_deadline.map_or(configured_deadline, |deadline| {
            tokio::time::Instant::from_std(deadline).min(configured_deadline)
        });
        // 门控打开时 4D1 候选要在同一会话上证明认证 UID（read_identity_in_session 要求独占
        // guard），故持独占会话读完整个窗口，使详情与身份绑定出自同一凭据；关闭时仍是不持锁的读会话。
        let acquire = async {
            if consignment_candidates {
                self.credentials.begin_exclusive(world, MoogleMailOperation::Inbox).await
            } else {
                self.credentials.begin_read(world, MoogleMailOperation::Inbox).await
            }
        };
        let session =
            tokio::time::timeout_at(deadline, acquire).await.map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "acquire_inbox_session_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                fetch_budget_exhausted(world, MoogleMailOperation::Inbox)
            })??;
        match tokio::time::timeout_at(
            deadline,
            self.fetch_with_cookie(
                world,
                cursor,
                limit,
                ConsignmentFetch { session: &session, candidates: consignment_candidates },
                deadline,
                journal,
            ),
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
                Err(fetch_budget_exhausted(world, MoogleMailOperation::Inbox))
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
                        fetch_budget_exhausted(world, MoogleMailOperation::Inbox)
                    },
                )??;
                if tokio::time::Instant::now() >= deadline {
                    return Err(fetch_budget_exhausted(world, MoogleMailOperation::Inbox));
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
                        fetch_budget_exhausted(world, MoogleMailOperation::Inbox)
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
        fetch: ConsignmentFetch<'_>,
        deadline: tokio::time::Instant,
        journal: Option<ReadJournalContext<'_>>,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        let ConsignmentFetch { session, candidates: consignment_candidates } = fetch;
        let cookie = session.cookie();
        self.report_poll_progress(journal, MailPollProgressStage::Fetching, None);
        let round = self.client.read_round(cookie, world, false)?;
        let mut seen = HashSet::new();
        let mut window = VecDeque::with_capacity(limit);
        let mut missing_list_snapshot = HashSet::new();
        if let Some((store, claim)) = journal {
            let pending = store
                .pending_reads(
                    claim.clone(),
                    world.as_str().to_owned(),
                    u16::try_from(limit)
                        .map_err(|_| configuration(world, MoogleMailOperation::Inbox))?,
                )
                .await
                .map_err(safe_diagnostics::warn_map(
                    "mooglemail_read",
                    "claim_read_journal",
                    || read_journal_unavailable(world),
                ))?;
            if pending.len() > limit {
                return Err(protocol_error(world, MoogleMailOperation::Inbox));
            }
            for summary in pending.into_iter().rev() {
                if !summary.validate()
                    || summary.world_id != world.as_str()
                    || !seen.insert(summary.external_mail_id.clone())
                {
                    return Err(protocol_error(world, MoogleMailOperation::Inbox));
                }
                if summary.received_minute_raw.is_none() {
                    missing_list_snapshot.insert(summary.external_mail_id.clone());
                }
                window.push_back(summary_from_read_snapshot(summary, world)?);
            }
        }
        let recovering = !window.is_empty();
        if recovering {
            // 恢复窗口的待读 MID 已知：最低 MID 就是自然下界，读到它即可停止翻页。
            let anchor = pending_anchor(&window);
            let mut pending: HashMap<_, _> = window
                .iter()
                .map(|summary| {
                    (summary.external_mail_id.clone(), (summary.world, summary.received_at))
                })
                .collect();
            let mut resolved = HashMap::new();
            let inbox = self.recovery_rows(world, cookie, &round, false, anchor.as_ref()).await?;
            for summary in inbox.summaries() {
                if let Some((expected_world, expected_at)) =
                    pending.remove(&summary.external_mail_id)
                    && (summary.world != expected_world || summary.received_at != expected_at)
                {
                    return Err(protocol_error(world, MoogleMailOperation::Inbox));
                }
                if missing_list_snapshot.remove(&summary.external_mail_id) {
                    resolved.insert(summary.external_mail_id.clone(), summary.clone());
                }
            }
            if !pending.is_empty() {
                let read = self.recovery_rows(world, cookie, &round, true, anchor.as_ref()).await?;
                for summary in read.summaries() {
                    if let Some((expected_world, expected_at)) =
                        pending.remove(&summary.external_mail_id)
                        && (summary.world != expected_world || summary.received_at != expected_at)
                    {
                        return Err(protocol_error(world, MoogleMailOperation::Inbox));
                    }
                    if missing_list_snapshot.remove(&summary.external_mail_id) {
                        resolved.insert(summary.external_mail_id.clone(), summary.clone());
                    }
                }
            }
            for (external_mail_id, found) in resolved {
                if let Some(summary) =
                    window.iter_mut().find(|summary| summary.external_mail_id == external_mail_id)
                {
                    summary.world_marker = found.world_marker;
                    summary.received_minute_raw = found.received_minute_raw;
                }
            }
            // 旧版行没有列表字段。门控打开且候选无法从本轮列表补证时失败关闭为 pending，
            // 下一轮仍会先读列表，找到同一行后即可原样恢复。
            if consignment_candidates
                && missing_list_snapshot.iter().any(|external_mail_id| {
                    window.iter().any(|summary| {
                        summary.external_mail_id == *external_mail_id
                            && list_is_candidate(&summary.sender, &summary.subject)
                    })
                })
            {
                return Err(pending_error(
                    world,
                    super::consignment::ConsignmentReadPending::TransientReadUnavailable,
                ));
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
                    list_world_marker: summary
                        .world_marker
                        .map(|marker| marker.as_str().to_owned()),
                    received_minute_raw: Some(summary.received_minute_raw.clone()),
                })
                .collect();
            // 原子登记整窗后才允许首个详情 GET；取消或提交失败时队列仍可恢复。
            store.stage_reads(claim.clone(), world.as_str().to_owned(), summaries).await.map_err(
                safe_diagnostics::warn_map("mooglemail_read", "stage_read_journal", || {
                    read_journal_unavailable(world)
                }),
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
            let candidate_shaped =
                candidate_route_enabled(consignment_candidates, &summary.sender, &summary.subject);
            #[cfg(test)]
            if candidate_shaped {
                CANDIDATE_ROUTE_DECISIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
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
            let parsed = self
                .client
                .diagnose_parse(&target, &document, self.parser.parse_detail(&document, &summary))
                .await;
            let message = match parsed {
                Ok(message) => message,
                Err(strict_error) if candidate_shaped => {
                    // 严格解析失败（如重复原文 EID）的候选形状邮件：诊断已记录，改以宽容解析
                    // 落成待隔离观察，不让整页读取失败而挡住同窗口后续邮件。
                    let row = super::consignment_candidate::row_from_summary(&summary);
                    let candidate = bind_unparsed_candidate(
                        self,
                        session,
                        world,
                        deadline,
                        &row,
                        &document,
                        strict_error,
                    )
                    .await?;
                    messages.push(MoogleMailPageMessage::UnparsedConsignmentCandidate(Box::new(
                        candidate,
                    )));
                    continue;
                }
                Err(error) => return Err(error),
            };
            // MID、world 与接收时间取自同一摘要，无需逐字段复核；parser 改为从页面读取时须恢复。
            // 业务身份与白名单只使用详情完整发件人；旧待读摘要的截断名称不参与授权。
            // 不匹配时保留队列，也不执行任何业务命令。
            if !self.allowed_sender(&message.sender) {
                return Err(configuration(world, MoogleMailOperation::Inbox));
            }
            if candidate_shaped
                && detail_route(&message.sender, &message.subject, true) == DetailRoute::Consignment
            {
                let row = super::consignment_candidate::row_from_summary(&summary);
                let bound =
                    match bind_candidate(self, session, world, deadline, &row, &document).await {
                        Ok(bound) => bound,
                        Err(reason) => return Err(pending_error(world, reason)),
                    };
                messages.push(MoogleMailPageMessage::ConsignmentCandidate {
                    message,
                    candidate: Box::new(MoogleMailConsignmentCandidate { row, bound }),
                });
            } else {
                messages.push(MoogleMailPageMessage::Legacy(message));
            }
        }
        self.client.check_read_round(cookie, &round).await?;
        if messages.is_empty() && !had_candidates {
            return Ok(MoogleMailPage { messages, next_cursor: cursor.map(str::to_owned) });
        }
        if messages.is_empty() {
            return Err(fetch_budget_exhausted(world, MoogleMailOperation::Inbox));
        }
        let next_cursor = messages.last().map(|message| message.external_mail_id().to_owned());
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

    pub(super) fn allowed_sender(&self, sender: &str) -> bool {
        self.config.sender_allowlist_disabled
            || self
                .config
                .sender_allowlist
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(sender))
    }
}

/// 寄售读取失败的闭集分类；只有明确可重试或本地授权问题保留 pending。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsignmentReadClassification {
    /// 保留原 pending 定位，稍后重读。
    Pending(super::consignment::ConsignmentReadPending),
    /// 认证已成立但清单不明；交给上层形成可隔离观察。
    ObservedUnclear,
}

/// 把远端读取错误归入寄售闭集；不得把临时失败或 allowlist 问题变成普通空信。
pub(crate) fn classify_consignment_read_error(
    error: MoogleMailError,
) -> ConsignmentReadClassification {
    use super::consignment::ConsignmentReadPending;
    match error.category {
        MoogleMailErrorCategory::RateLimited => {
            ConsignmentReadClassification::Pending(ConsignmentReadPending::RateLimited)
        }
        MoogleMailErrorCategory::AccountBusy => {
            ConsignmentReadClassification::Pending(ConsignmentReadPending::AccountBusy)
        }
        MoogleMailErrorCategory::SessionExpired
        | MoogleMailErrorCategory::CredentialUnavailable
        | MoogleMailErrorCategory::RemoteTemporary
        | MoogleMailErrorCategory::SentScanPageLimit
        | MoogleMailErrorCategory::SentScanTimeout
        | MoogleMailErrorCategory::PersistenceUnavailable
        | MoogleMailErrorCategory::Configuration => {
            ConsignmentReadClassification::Pending(ConsignmentReadPending::TransientReadUnavailable)
        }
        MoogleMailErrorCategory::ResponseTooLarge
        | MoogleMailErrorCategory::ProtocolDrift
        | MoogleMailErrorCategory::IdentityMismatch
        | MoogleMailErrorCategory::IdentityUnproven
        | MoogleMailErrorCategory::NotObserved
        | MoogleMailErrorCategory::WriteUnknown => ConsignmentReadClassification::ObservedUnclear,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, atomic::Ordering};
    use std::time::{Duration, Instant};

    use auction_application::admin::AdminCredentialRuntimeOwnership;
    use auction_application::core::ApplicationFuture;
    use auction_application::mail::{MailPollClaim, MailPollReadJournal, MailPollReadSummary};
    use auction_application::settlement::MailRuntimeBinding;
    use auction_time::Timestamp;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use url::Url;

    use super::super::consignment::ConsignmentReadPending;
    use super::super::consignment_candidate::{DetailRoute, detail_route, list_is_candidate};
    use super::{CANDIDATE_ROUTE_DECISIONS, candidate_route_enabled, summary_from_read_snapshot};
    use super::{ConsignmentReadClassification, classify_consignment_read_error};
    use crate::clients::mooglemail::parser::{
        ConsignmentListRow, parse_consignment_detail, parse_consignment_list_row,
    };
    use crate::clients::mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
        MoogleMailError, MoogleMailErrorCategory, MoogleMailGateway, MoogleMailGatewayConfig,
        MoogleMailOperation, MoogleMailPageMessage, MoogleMailParser, MoogleMailWorld,
    };

    const LIST: &str =
        include_str!("../../../../../tests/fixtures/mooglemail/consign-list-main.html");
    const DUPLICATE_DETAIL: &str = include_str!(
        "../../../../../tests/fixtures/mooglemail/consign-detail-main-duplicate-eid.html"
    );
    const SINGLE_DETAIL: &str =
        include_str!("../../../../../tests/fixtures/mooglemail/consign-detail-main-single.html");

    #[test]
    fn disabled_gate_keeps_the_old_detail_result_for_consign_with_duplicate_eid() {
        let mut seen = HashSet::new();
        let page = MoogleMailParser
            .scan_read_mail_page(LIST.as_bytes(), MoogleMailWorld::Main, 0, &mut seen)
            .expect("固定列表页");
        let mut summary = page
            .summaries
            .into_iter()
            .find(|summary| summary.external_mail_id == "900001")
            .expect("列表含 consign 行");
        // 重复原文 EID 的详情页在旧通用解析下必须失败；门控关闭时不得改走 4D1。
        summary.external_mail_id = "900004".to_owned();
        assert!(list_is_candidate(&summary.sender, &summary.subject));
        assert_eq!(detail_route(&summary.sender, &summary.subject, false), DetailRoute::Legacy);
        assert!(MoogleMailParser.parse_detail(DUPLICATE_DETAIL.as_bytes(), &summary).is_err());
    }

    #[test]
    fn gate_is_required_after_candidate_shape_is_known() {
        assert!(!candidate_route_enabled(false, "FixtureSeller", "consign"));
        assert!(!candidate_route_enabled(false, "MoogleMail", "[RTS] Auction payment overdue"));
        assert!(candidate_route_enabled(true, "FixtureSeller", "consign"));
        assert!(!candidate_route_enabled(true, "FixtureSeller", "hello"));
    }

    #[test]
    fn recovered_candidate_keeps_list_detail_consistency_and_manifest() {
        let row = parse_consignment_list_row(LIST.as_bytes(), MoogleMailWorld::Main, "900001")
            .expect("固定列表页")
            .expect("列表含候选行");
        let first = parse_consignment_detail(SINGLE_DETAIL.as_bytes(), MoogleMailWorld::Main, &row);
        let snapshot = MailPollReadSummary {
            external_mail_id: row.external_mail_id.clone(),
            world_id: row.world.as_str().to_owned(),
            sender: row.sender.clone(),
            subject: row.subject.clone(),
            received_at: row.received_at,
            list_world_marker: row.world_marker.map(|marker| marker.as_str().to_owned()),
            received_minute_raw: Some(row.received_minute_raw),
        };
        let restored =
            summary_from_read_snapshot(snapshot, MoogleMailWorld::Main).expect("恢复列表摘要");
        let restored_row = ConsignmentListRow {
            external_mail_id: restored.external_mail_id.clone(),
            world: restored.world,
            world_marker: restored.world_marker,
            sender: restored.sender.clone(),
            subject: restored.subject.clone(),
            received_minute_raw: restored.received_minute_raw.clone(),
            received_at: restored.received_at,
        };
        let recovered = parse_consignment_detail(
            SINGLE_DETAIL.as_bytes(),
            MoogleMailWorld::Main,
            &restored_row,
        );
        assert!(first.list_detail_consistent);
        assert_eq!(first.list_detail_consistent, recovered.list_detail_consistent);
        assert_eq!(first.manifest_digest(), recovered.manifest_digest());
    }

    struct ValidOwner;

    impl AdminCredentialRuntimeOwnership for ValidOwner {
        fn check(&self) -> ApplicationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn is_valid(&self) -> bool {
            true
        }

        fn mail_runtime_binding(&self) -> Option<MailRuntimeBinding> {
            Some(MailRuntimeBinding { generation: 1 })
        }

        fn revoke_internal_use(&self) {}
    }

    struct EmptyJournal;

    impl MailPollReadJournal for EmptyJournal {
        fn pending_reads(
            &self,
            _claim: MailPollClaim,
            _world_id: String,
            _limit: u16,
        ) -> ApplicationFuture<'_, Vec<MailPollReadSummary>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn stage_reads(
            &self,
            _claim: MailPollClaim,
            _world_id: String,
            _summaries: Vec<MailPollReadSummary>,
        ) -> ApplicationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    struct FixtureServer {
        endpoint: Url,
        requests: Arc<Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn fixture_server(detail: &'static str) -> FixtureServer {
        fixture_server_with_list(detail, list_page()).await
    }

    async fn fixture_server_with_list(detail: &'static str, list: &'static str) -> FixtureServer {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.expect("回环监听");
        let address = listener.local_addr().expect("回环地址");
        let endpoint = Url::parse(&format!("http://{address}/")).expect("回环 endpoint");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let mut data = Vec::new();
                    let mut chunk = [0_u8; 2048];
                    while !data.windows(4).any(|window| window == b"\r\n\r\n") {
                        let Ok(read) = stream.read(&mut chunk).await else { return };
                        if read == 0 {
                            return;
                        }
                        data.extend_from_slice(&chunk[..read]);
                    }
                    let line = String::from_utf8_lossy(&data)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    recorded.lock().expect("请求记录").push(line.clone());
                    let body = if line.contains("mid=900004") {
                        detail
                    } else if line.contains("s=Character") {
                        include_str!(
                            "../../../../../tests/fixtures/mooglemail/consign-identity-main.html"
                        )
                    } else if line.contains("mid=900005") {
                        SINGLE_DETAIL
                    } else {
                        list
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        FixtureServer { endpoint, requests, task }
    }

    fn list_page() -> &'static str {
        r#"<!DOCTYPE html><html><body>
<div id="world_text"><div><p>Currently playing on Persistent</p></div></div>
<table id="mmail_list">
<tr><th>From</th><th>Subject</th><th>Sent</th><th>Read</th></tr>
<tr onclick="document.location='https://hentaiverse.org/?s=Bazaar&amp;ss=mm&amp;filter=read&amp;mid=900004'">
<td>FixtureSeller</td><td>consign</td><td>2026-11-02 03:45</td><td>2026-11-02 03:45</td>
</tr></table>
<div id="mmail_pager"><div></div><div><span>Next &gt;</span></div></div>
</body></html>"#
    }

    /// 同页两封候选：新的 900005 在前，重复 EID 的 900004 是更早的一封，先被读取。
    fn list_page_with_following_mail() -> &'static str {
        r#"<!DOCTYPE html><html><body>
<div id="world_text"><div><p>Currently playing on Persistent</p></div></div>
<table id="mmail_list">
<tr><th>From</th><th>Subject</th><th>Sent</th><th>Read</th></tr>
<tr onclick="document.location='https://hentaiverse.org/?s=Bazaar&amp;ss=mm&amp;filter=read&amp;mid=900005'">
<td>FixtureSeller</td><td>consign</td><td>2026-11-02 03:46</td><td>2026-11-02 03:46</td>
</tr>
<tr onclick="document.location='https://hentaiverse.org/?s=Bazaar&amp;ss=mm&amp;filter=read&amp;mid=900004'">
<td>FixtureSeller</td><td>consign</td><td>2026-11-02 03:45</td><td>2026-11-02 03:45</td>
</tr></table>
<div id="mmail_pager"><div></div><div><span>Next &gt;</span></div></div>
</body></html>"#
    }

    fn poll_claim() -> MailPollClaim {
        MailPollClaim {
            mailbox_key: "fixture-mailbox".to_owned(),
            worker_id: "fixture-worker".to_owned(),
            claim_token: uuid::Uuid::new_v4(),
            cursor_value: None,
            state_version: 1,
            paused: false,
            attempt_count: 1,
            lease_until: Timestamp::now() + auction_time::Duration::seconds(60),
        }
    }

    async fn fixture_gateway(server: &FixtureServer) -> MoogleMailGateway {
        let client = MoogleMailClient::with_config(MoogleMailClientConfig {
            endpoint: server.endpoint.clone(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval: Duration::from_millis(1),
            request_timeout: Duration::from_secs(2),
        })
        .expect("回环客户端");
        let credentials = MoogleMailCredentialSet::from_owned_cookie(
            MoogleMailCookie::new("ipb_member_id=fixture; ipb_pass_hash=fixture")
                .expect("夹具 Cookie"),
            Arc::new(ValidOwner),
        )
        .expect("夹具凭据");
        MoogleMailGateway::new(
            Arc::new(client),
            credentials,
            MoogleMailGatewayConfig {
                maximum_pages: 10,
                sender_allowlist: vec!["FixtureSeller".to_owned()],
                sender_allowlist_disabled: false,
                fetch_budget: Duration::from_secs(3),
            },
        )
        .expect("夹具 gateway")
    }

    /// 回显请求 UID 的论坛资料替身：只让认证身份读取成功，不连论坛。
    struct EchoProfileReader;

    impl auction_application::forum::ForumTargetProfileReader for EchoProfileReader {
        fn read_target_profile(
            &self,
            target_uid: String,
            _deadline: Instant,
        ) -> auction_application::settlement::InternalEvidenceFuture<
            '_,
            auction_application::forum::ForumTargetProfileEvidence,
        > {
            Box::pin(async move {
                Ok(auction_application::forum::ForumTargetProfileEvidence {
                    target_uid,
                    player_name: "FixtureBot".to_owned(),
                    observed_at: Timestamp::now(),
                    protocol_version: "forum_target_profile_mid_v1",
                    digest: [1; 32],
                })
            })
        }
    }

    struct NoInvalidation;

    impl auction_application::admin::AdminCredentialRuntimeWriter for NoInvalidation {
        fn mark_invalid(
            &self,
            _credential_id: uuid::Uuid,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    /// 具备持久凭据身份与论坛资料能力的网关：门控打开的候选才能建立 4D1 会话绑定。
    async fn identity_gateway(server: &FixtureServer) -> MoogleMailGateway {
        let client = MoogleMailClient::with_config(MoogleMailClientConfig {
            endpoint: server.endpoint.clone(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval: Duration::from_millis(1),
            request_timeout: Duration::from_secs(2),
        })
        .expect("回环客户端");
        let credentials =
            crate::clients::mooglemail::MoogleMailCredentialSet::from_runtime_credentials(
                vec![crate::clients::mooglemail::MoogleMailRuntimeCredential {
                    credential_id: uuid::Uuid::new_v4(),
                    state_version: 1,
                    world: None,
                    player_name: None,
                    cookie: MoogleMailCookie::new("ipb_member_id=fixture; ipb_pass_hash=fixture")
                        .expect("夹具 Cookie"),
                }],
                Arc::new(NoInvalidation),
                Some(Arc::new(ValidOwner)),
            )
            .expect("夹具凭据");
        MoogleMailGateway::new(
            Arc::new(client),
            credentials,
            MoogleMailGatewayConfig {
                maximum_pages: 10,
                sender_allowlist: vec!["FixtureSeller".to_owned()],
                sender_allowlist_disabled: false,
                fetch_budget: Duration::from_secs(3),
            },
        )
        .expect("夹具 gateway")
        .with_forum_profile_reader(Arc::new(EchoProfileReader))
    }

    async fn fetch_with_gate(
        gateway: &MoogleMailGateway,
        consignment_candidates: bool,
    ) -> Result<crate::clients::mooglemail::MoogleMailPage, MoogleMailError> {
        gateway
            .fetch_claimed(
                MoogleMailWorld::Main,
                10,
                Some(Instant::now() + Duration::from_secs(3)),
                &EmptyJournal,
                &poll_claim(),
                consignment_candidates,
            )
            .await
    }

    /// 候选分流计数是进程内全局量：读它的测试与会触发分流的测试须串行。
    static ROUTE_COUNTER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn disabled_gate_fetch_returns_legacy_for_consign_detail() {
        let _serial = ROUTE_COUNTER_LOCK.lock().await;
        CANDIDATE_ROUTE_DECISIONS.store(0, Ordering::SeqCst);
        let server = fixture_server(SINGLE_DETAIL).await;
        let gateway = fixture_gateway(&server).await;
        let page = fetch_with_gate(&gateway, false).await.expect("门控关闭沿用通用解析");
        assert_eq!(page.messages.len(), 1);
        assert!(matches!(page.messages[0], MoogleMailPageMessage::Legacy(_)));
        assert_eq!(CANDIDATE_ROUTE_DECISIONS.load(Ordering::SeqCst), 0);
        let requests = server.requests.lock().expect("请求记录").clone();
        assert!(requests.iter().any(|request| request.contains("mid=900004")));
        assert!(!requests.iter().any(|request| request.contains("s=Character")));
    }

    #[tokio::test]
    async fn disabled_gate_fetch_keeps_generic_failure_for_duplicate_eid() {
        let _serial = ROUTE_COUNTER_LOCK.lock().await;
        CANDIDATE_ROUTE_DECISIONS.store(0, Ordering::SeqCst);
        let server = fixture_server(DUPLICATE_DETAIL).await;
        let gateway = fixture_gateway(&server).await;
        let result = fetch_with_gate(&gateway, false).await;
        let error = match result {
            Ok(page) => panic!("门控关闭时必须沿用旧解析结果，实际 {:?}", page.messages.len()),
            Err(error) => error,
        };
        // 重复原文 EID 在旧通用解析下失败；门控关闭不得改走 4D1。
        assert_eq!(error.category, MoogleMailErrorCategory::ProtocolDrift);
        assert_eq!(CANDIDATE_ROUTE_DECISIONS.load(Ordering::SeqCst), 0);
        let requests = server.requests.lock().expect("请求记录").clone();
        assert!(requests.iter().any(|request| request.contains("mid=900004")));
        assert!(!requests.iter().any(|request| request.contains("s=Character")));
    }

    /// 生产入口：门控打开时候选经独占会话读取，并带回认证身份绑定。
    #[tokio::test]
    async fn enabled_gate_binds_single_eid_candidate_through_production_entry() {
        let _serial = ROUTE_COUNTER_LOCK.lock().await;
        let server = fixture_server(SINGLE_DETAIL).await;
        let gateway = identity_gateway(&server).await;
        let page = fetch_with_gate(&gateway, true).await.expect("候选应被绑定而不是身份未证");
        assert_eq!(page.messages.len(), 1);
        let MoogleMailPageMessage::ConsignmentCandidate { candidate, .. } = &page.messages[0]
        else {
            panic!("门控打开的 consign 应交出候选：{:?}", page.messages[0]);
        };
        assert!(!candidate.bound.binding().authenticated_uid.is_empty());
        assert!(!candidate.bound.observation().duplicate_raw_eid);
    }

    /// 门控关闭时仍用不持锁的读会话：另一处持有独占 guard 时不被阻塞。
    #[tokio::test]
    async fn disabled_gate_fetch_is_not_blocked_by_an_exclusive_session() {
        let server = fixture_server(SINGLE_DETAIL).await;
        let gateway = identity_gateway(&server).await;
        let _held = gateway
            .credentials
            .begin_exclusive(MoogleMailWorld::Main, MoogleMailOperation::Detail)
            .await
            .expect("独占会话");
        let page = tokio::time::timeout(Duration::from_secs(2), fetch_with_gate(&gateway, false))
            .await
            .expect("门控关闭的读取不得等待独占 guard")
            .expect("门控关闭沿用通用解析");
        assert!(matches!(page.messages[0], MoogleMailPageMessage::Legacy(_)));
    }

    #[tokio::test]
    async fn enabled_gate_hands_over_duplicate_eid_candidate_and_reads_following_mail() {
        let _serial = ROUTE_COUNTER_LOCK.lock().await;
        let server =
            fixture_server_with_list(DUPLICATE_DETAIL, list_page_with_following_mail()).await;
        let gateway = identity_gateway(&server).await;
        let page = fetch_with_gate(&gateway, true).await.expect("重复 EID 候选不得让整页失败");
        assert_eq!(page.messages.len(), 2);
        // 严格解析失败的候选只交出宽容观察，且观察标明重复原文 EID 供寄售决定进隔离。
        let MoogleMailPageMessage::UnparsedConsignmentCandidate(candidate) = &page.messages[0]
        else {
            panic!("重复 EID 候选应以无通用解析结果的形态交出：{:?}", page.messages[0]);
        };
        assert_eq!(candidate.row.external_mail_id, "900004");
        assert!(candidate.bound.observation().duplicate_raw_eid);
        // 同页后续邮件照常读取，游标取最后一封。
        assert!(matches!(page.messages[1], MoogleMailPageMessage::ConsignmentCandidate { .. }));
        assert_eq!(page.next_cursor.as_deref(), Some("900005"));
    }

    /// 升级前登记的旧版待读行：没有列表世界标记与原始分钟。
    struct LegacyPendingJournal;

    impl MailPollReadJournal for LegacyPendingJournal {
        fn pending_reads(
            &self,
            _claim: MailPollClaim,
            world_id: String,
            _limit: u16,
        ) -> ApplicationFuture<'_, Vec<MailPollReadSummary>> {
            Box::pin(async move {
                Ok(vec![MailPollReadSummary {
                    external_mail_id: "900002".to_owned(),
                    world_id,
                    sender: "FixtureSeller".to_owned(),
                    subject: "consign".to_owned(),
                    received_at: Timestamp::from_ymd_hms(2026, 11, 2, 3, 40, 0).expect("夹具时刻"),
                    list_world_marker: None,
                    received_minute_raw: None,
                }])
            })
        }

        fn stage_reads(
            &self,
            _claim: MailPollClaim,
            _world_id: String,
            _summaries: Vec<MailPollReadSummary>,
        ) -> ApplicationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn enabled_gate_keeps_legacy_candidate_pending_without_list_snapshot() {
        let server = fixture_server(SINGLE_DETAIL).await;
        let gateway = fixture_gateway(&server).await;
        let result = gateway
            .fetch_claimed(
                MoogleMailWorld::Main,
                10,
                Some(Instant::now() + Duration::from_secs(3)),
                &LegacyPendingJournal,
                &poll_claim(),
                true,
            )
            .await;
        let error = match result {
            Ok(page) => {
                panic!("旧版候选行缺列表字段时必须失败关闭，实际 {} 封", page.messages.len())
            }
            Err(error) => error,
        };
        // 本轮列表补不到原始分钟与世界标记：门控打开时停在 pending，不得用推导值读详情，
        // 否则 list_detail_consistent 与清单摘要会与首次读取悄悄不一致。
        assert_eq!(error.category, MoogleMailErrorCategory::CredentialUnavailable);
        let requests = server.requests.lock().expect("请求记录").clone();
        assert!(!requests.iter().any(|request| request.contains("mid=900002")));
    }

    fn error(category: MoogleMailErrorCategory) -> MoogleMailError {
        MoogleMailError::new(MoogleMailOperation::Detail, MoogleMailWorld::Main, category, false)
    }

    #[test]
    fn consignment_read_error_classification_covers_every_category() {
        let cases = [
            (
                MoogleMailErrorCategory::RateLimited,
                ConsignmentReadClassification::Pending(ConsignmentReadPending::RateLimited),
            ),
            (
                MoogleMailErrorCategory::AccountBusy,
                ConsignmentReadClassification::Pending(ConsignmentReadPending::AccountBusy),
            ),
            (
                MoogleMailErrorCategory::SessionExpired,
                ConsignmentReadClassification::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                ),
            ),
            (
                MoogleMailErrorCategory::CredentialUnavailable,
                ConsignmentReadClassification::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                ),
            ),
            (
                MoogleMailErrorCategory::RemoteTemporary,
                ConsignmentReadClassification::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                ),
            ),
            (
                MoogleMailErrorCategory::SentScanPageLimit,
                ConsignmentReadClassification::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                ),
            ),
            (
                MoogleMailErrorCategory::SentScanTimeout,
                ConsignmentReadClassification::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                ),
            ),
            (
                MoogleMailErrorCategory::PersistenceUnavailable,
                ConsignmentReadClassification::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                ),
            ),
            (
                MoogleMailErrorCategory::Configuration,
                ConsignmentReadClassification::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                ),
            ),
            (
                MoogleMailErrorCategory::ResponseTooLarge,
                ConsignmentReadClassification::ObservedUnclear,
            ),
            (
                MoogleMailErrorCategory::ProtocolDrift,
                ConsignmentReadClassification::ObservedUnclear,
            ),
            (
                MoogleMailErrorCategory::IdentityMismatch,
                ConsignmentReadClassification::ObservedUnclear,
            ),
            (
                MoogleMailErrorCategory::IdentityUnproven,
                ConsignmentReadClassification::ObservedUnclear,
            ),
            (MoogleMailErrorCategory::NotObserved, ConsignmentReadClassification::ObservedUnclear),
            (MoogleMailErrorCategory::WriteUnknown, ConsignmentReadClassification::ObservedUnclear),
        ];
        for (category, expected) in cases {
            assert_eq!(classify_consignment_read_error(error(category)), expected, "{category:?}");
        }
    }
}
