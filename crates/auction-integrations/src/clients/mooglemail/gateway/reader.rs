//! MoogleMail 收件、Sent 和余额的有界读取流程。

use auction_application::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex as StdMutex},
};

use chrono::{DateTime, Utc};

use self::parent::*;
use super as parent;
use super::super::client::{MoogleMailIncomingList, MoogleMailIncomingPrefix, MoogleMailReadRound};
use super::super::protocol::valid_opaque_id;
use crate::clients::mooglemail::parser::subject_is_list_projection;
use auction_application::{
    MailPollClaim, MailPollProgressObserver, MailPollProgressStage, MailPollReadJournal,
    MailPollReadSummary, MailScanAnchor, MailScanCheckpoint, MailScanEvent, MailScanObserver,
    MailScanOutcome, MailScanReason, MailScanStream, MailScanWatermark,
    advance_mail_scan_watermark, mail_recipient_is_list_projection, plan_mail_scan,
};

type ReadJournalContext<'a> = (&'a dyn MailPollReadJournal, &'a MailPollClaim);

/// 与完整枚举共用的窗口选择：游标之前的行不进入窗口；超过上限时保留最旧窗口，
/// 让未处理的旧邮件在下一轮优先提交，而不是被新邮件挤出游标。
fn incoming_window_rows(
    rows: &[MoogleMailSummary],
    cursor: Option<&str>,
    limit: usize,
) -> VecDeque<MoogleMailSummary> {
    let mut window = VecDeque::with_capacity(limit.min(rows.len()));
    for summary in rows {
        if cursor.is_some_and(|value| value == summary.external_mail_id) {
            break;
        }
        if window.len() == limit {
            window.pop_front();
        }
        window.push_back(summary.clone());
    }
    window
}

/// 恢复窗口使用的列表观测：命中已知最低待读 MID 的前缀，或完整枚举回退。
enum RecoveryRows {
    Prefix(MoogleMailIncomingPrefix),
    Full(Arc<MoogleMailIncomingList>),
}

impl RecoveryRows {
    fn summaries(&self) -> &[MoogleMailSummary] {
        match self {
            Self::Prefix(scan) => &scan.summaries,
            Self::Full(list) => &list.summaries,
        }
    }
}

/// 目标已知的最小 MID 是自然下界：读到它即可停止，更新邮件仍在本前缀中。
fn pending_anchor(window: &VecDeque<MoogleMailSummary>) -> Option<MailScanAnchor> {
    window
        .iter()
        .filter_map(|summary| {
            auction_application::numeric_mail_scan_id(&summary.external_mail_id)
                .map(|value| (value, summary.external_mail_id.as_str()))
        })
        .min_by_key(|(value, _)| *value)
        .and_then(|(_, mail_id)| MailScanAnchor::from_cursor(mail_id))
}

/// 比较完整枚举与前缀的“新增”MID 集合；游标之前的历史行不参与。
fn new_mail_ids(rows: &[MoogleMailSummary], cursor: Option<&str>) -> HashSet<String> {
    let anchor = cursor.and_then(auction_application::numeric_mail_scan_id);
    rows.iter()
        .filter(|summary| {
            anchor.is_none_or(|anchor| {
                auction_application::numeric_mail_scan_id(&summary.external_mail_id)
                    .is_some_and(|value| value > anchor)
            })
        })
        .map(|summary| summary.external_mail_id.clone())
        .collect()
}

/// MoogleMail 收件箱读取 gateway。
pub struct MoogleMailGateway {
    pub(super) client: Arc<MoogleMailClient>,
    pub(super) credentials: MoogleMailCredentialSet,
    pub(super) config: MoogleMailGatewayConfig,
    pub(super) parser: MoogleMailParser,
    pub(super) profile_reader: Option<Arc<dyn auction_application::ForumTargetProfileReader>>,
    progress_observer: Option<Arc<dyn MailPollProgressObserver>>,
    /// 进程内每日校验检查点；重启只会多做一次完整扫描，永远不会跳过未提交游标之后的行。
    scan_checkpoints: StdMutex<HashMap<(MoogleMailWorld, MailScanStream), MailScanCheckpoint>>,
    scan_observer: Arc<dyn MailScanObserver>,
}

impl MoogleMailGateway {
    /// 配置的列表页数硬上限；恢复核对只能在同一上限内请求更小预算。
    pub fn maximum_pages(&self) -> usize {
        self.config.maximum_pages
    }

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
            if !auction_domain::valid_text(sender, 256)
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
            scan_checkpoints: StdMutex::new(HashMap::new()),
            scan_observer: auction_application::noop_mail_scan_observer(),
        })
    }

    /// 注入增量扫描的闭集观测；观察失败不改变读取或证明结果。
    pub fn with_scan_observer(mut self, observer: Arc<dyn MailScanObserver>) -> Self {
        self.scan_observer = observer;
        self
    }

    /// 注入进程内观测；仅持有 claim 的轮询路径报告真实读取目标，不参与协议或授权决定。
    pub fn with_progress_observer(mut self, observer: Arc<dyn MailPollProgressObserver>) -> Self {
        self.progress_observer = Some(observer);
        self
    }

    fn scan_plan(
        &self,
        world: MoogleMailWorld,
        stream: MailScanStream,
    ) -> auction_application::MailScanPlan {
        let checkpoint = self
            .scan_checkpoints
            .lock()
            .ok()
            .and_then(|checkpoints| checkpoints.get(&(world, stream)).cloned());
        plan_mail_scan(checkpoint.as_ref(), Utc::now(), self.client.mail_scan_daily_full_interval())
    }

    /// 只推进进程内检查点；水位不是提交游标，轮询仍以 claim.cursor_value 为锚点。
    fn record_scan_checkpoint(
        &self,
        world: MoogleMailWorld,
        stream: MailScanStream,
        newest: Option<MailScanWatermark>,
        observed_at: DateTime<Utc>,
        full: bool,
    ) {
        let Ok(mut checkpoints) = self.scan_checkpoints.lock() else {
            tracing::warn!("MoogleMail 扫描检查点锁不可用；本轮结果照常返回");
            return;
        };
        let key = (world, stream);
        let previous = checkpoints.get(&key).cloned();
        let watermark = newest
            .map(|newest| {
                advance_mail_scan_watermark(
                    previous.as_ref().and_then(|c| c.watermark.as_ref()),
                    &newest,
                )
            })
            .or_else(|| previous.as_ref().and_then(|c| c.watermark.clone()));
        let last_full_scan_at = if full {
            Some(observed_at)
        } else {
            previous.and_then(|checkpoint| checkpoint.last_full_scan_at)
        };
        checkpoints.insert(key, MailScanCheckpoint { watermark, last_full_scan_at });
    }

    fn observe_scan(&self, world: MoogleMailWorld, event: MailScanEvent) {
        self.scan_observer.observe(world.as_str(), event);
    }

    /// 目标已知的恢复读取：读到最低待读 MID 即停；锚点缺失时回退完整枚举并保持失败关闭。
    async fn recovery_rows(
        &self,
        world: MoogleMailWorld,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        read_mail: bool,
        anchor: Option<&MailScanAnchor>,
    ) -> Result<RecoveryRows, MoogleMailError> {
        let stream = if read_mail { MailScanStream::ReadMail } else { MailScanStream::Inbox };
        if let Some(anchor) = anchor {
            let scan = self
                .client
                .observe_incoming_prefix(
                    cookie,
                    round,
                    self.config.maximum_pages,
                    read_mail,
                    anchor,
                )
                .await?;
            if scan.anchor_found {
                self.observe_scan(
                    world,
                    MailScanEvent::Prefix {
                        stream,
                        outcome: MailScanOutcome::Complete,
                        reason: None,
                    },
                );
                tracing::debug!(
                    world = world.as_str(),
                    stream = stream.label(),
                    pages_scanned = scan.pages_scanned,
                    "MoogleMail 恢复读取在最低待读 MID 处停止"
                );
                return Ok(RecoveryRows::Prefix(scan));
            }
            self.observe_scan(
                world,
                MailScanEvent::Prefix {
                    stream,
                    outcome: MailScanOutcome::Error,
                    reason: scan.issue,
                },
            );
        }
        let list = self
            .client
            .observe_incoming_list(cookie, round, self.config.maximum_pages, read_mail)
            .await?;
        if !list.complete {
            return Err(protocol_error(world));
        }
        self.observe_scan(
            world,
            MailScanEvent::Verification {
                stream,
                outcome: MailScanOutcome::Complete,
                reason: MailScanReason::RequestedFull,
            },
        );
        Ok(RecoveryRows::Full(list))
    }

    /// 优先从前缀构建窗口；每日校验、缺锚点或超限才完整枚举。
    ///
    /// 前缀只缩小观察范围：窗口内容仍需逐封详情解析、业务幂等与既有证明流程，
    /// 水位缺失或分页位移时立即回退完整扫描。
    async fn select_incoming_window(
        &self,
        world: MoogleMailWorld,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<VecDeque<MoogleMailSummary>, MoogleMailError> {
        let plan = self.scan_plan(world, MailScanStream::Inbox);
        self.observe_scan(
            world,
            MailScanEvent::Plan {
                stream: MailScanStream::Inbox,
                mode: plan.mode,
                reason: plan.reason,
            },
        );
        let anchor = cursor.and_then(MailScanAnchor::from_cursor);
        let mut prefix = None;
        if let Some(anchor) = anchor.as_ref() {
            let scan = self
                .client
                .observe_incoming_prefix(cookie, round, self.config.maximum_pages, false, anchor)
                .await?;
            if scan.anchor_found {
                self.observe_scan(
                    world,
                    MailScanEvent::Prefix {
                        stream: MailScanStream::Inbox,
                        outcome: MailScanOutcome::Complete,
                        reason: None,
                    },
                );
                tracing::debug!(
                    world = world.as_str(),
                    pages_scanned = scan.pages_scanned,
                    "MoogleMail 收件前缀扫描在锚点处停止"
                );
                prefix = Some(scan);
            } else {
                let reason = scan.issue.unwrap_or(MailScanReason::ReadUnavailable);
                self.observe_scan(
                    world,
                    MailScanEvent::Prefix {
                        stream: MailScanStream::Inbox,
                        outcome: MailScanOutcome::Error,
                        reason: Some(reason),
                    },
                );
            }
        }
        // `NoWatermark`（例如空列表或上次超限）仍可用持久游标做前缀；
        // 只有首次、检查点损坏、从未校验或每日到期才需要完整枚举。
        let verification_due = matches!(
            plan.reason,
            MailScanReason::NoCheckpoint
                | MailScanReason::InvalidCheckpoint
                | MailScanReason::NoFullVerification
                | MailScanReason::DailyVerification
                | MailScanReason::RequestedFull
        );
        if !verification_due && let Some(prefix) = prefix {
            self.record_scan_checkpoint(
                world,
                MailScanStream::Inbox,
                prefix.newest.clone(),
                round.observed_at(),
                false,
            );
            return Ok(incoming_window_rows(&prefix.summaries, cursor, limit));
        }
        let full = match self
            .client
            .observe_incoming_list(cookie, round, self.config.maximum_pages, false)
            .await
        {
            Ok(full) => full,
            Err(error) => {
                if let Some(prefix) = prefix {
                    // 每日校验读取失败只告警；前缀已经证明比游标更新的窗口完整。
                    self.record_scan_checkpoint(
                        world,
                        MailScanStream::Inbox,
                        None,
                        round.observed_at(),
                        true,
                    );
                    self.observe_scan(
                        world,
                        MailScanEvent::Verification {
                            stream: MailScanStream::Inbox,
                            outcome: MailScanOutcome::Error,
                            reason: plan.reason,
                        },
                    );
                    tracing::warn!(
                        world = world.as_str(),
                        "MoogleMail 完整校验读取失败；沿用已确认的前缀窗口"
                    );
                    return Ok(incoming_window_rows(&prefix.summaries, cursor, limit));
                }
                return Err(error);
            }
        };
        let outcome =
            if full.complete { MailScanOutcome::Complete } else { MailScanOutcome::PageLimit };
        // 计划外的完整枚举（前缀缺失回退）以 requested_full 记录，不冒充每日校验。
        let verification_reason =
            if verification_due { plan.reason } else { MailScanReason::RequestedFull };
        self.observe_scan(
            world,
            MailScanEvent::Verification {
                stream: MailScanStream::Inbox,
                outcome,
                reason: if full.complete { verification_reason } else { MailScanReason::PageLimit },
            },
        );
        let watermark = if full.complete {
            full.summaries.first().map(|summary| MailScanWatermark {
                mail_id: summary.external_mail_id.clone(),
                observed_at: summary.received_at,
            })
        } else {
            // 未到末页的枚举不能推进“已完整扫描”的水位；只更新校验时间。
            None
        };
        // 超限也算一次校验尝试；避免每个轮询轮都重扫同一超长列表。
        self.record_scan_checkpoint(
            world,
            MailScanStream::Inbox,
            watermark,
            round.observed_at(),
            true,
        );
        if !full.complete {
            // 超过页数上限只告警；若前缀已确认窗口完整则继续轮询，否则维持原失败关闭。
            if let Some(prefix) = &prefix {
                return Ok(incoming_window_rows(&prefix.summaries, cursor, limit));
            }
            return Err(protocol_error(world));
        }
        if let Some(prefix) = &prefix {
            self.compare_incoming_shadow(world, &full.summaries, &prefix.summaries, cursor);
        }
        Ok(incoming_window_rows(&full.summaries, cursor, limit))
    }

    fn compare_incoming_shadow(
        &self,
        world: MoogleMailWorld,
        legacy: &[MoogleMailSummary],
        candidate: &[MoogleMailSummary],
        cursor: Option<&str>,
    ) {
        let legacy = new_mail_ids(legacy, cursor);
        let candidate = new_mail_ids(candidate, cursor);
        let shadow = auction_application::compare_mail_scan_mids(&legacy, &candidate);
        self.observe_scan(
            world,
            MailScanEvent::Shadow {
                stream: MailScanStream::Inbox,
                outcome: MailScanOutcome::Complete,
                mid_sets_equal: shadow.is_match(),
            },
        );
        if !shadow.is_match() {
            tracing::warn!(
                world = world.as_str(),
                legacy_only = shadow.legacy_only,
                candidate_only = shadow.candidate_only,
                "MoogleMail 增量窗口与完整枚举的新增 MID 集合不一致"
            );
        }
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

    /// 按已知 MID 定位 Sent 列表行并核对详情；命中不扫无关尾页，判缺失必须到尾页。
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
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let result = tokio::time::timeout_at(deadline, async {
            let mut session = self.credentials.begin_read(world, MoogleMailOperation::Sent).await?;
            match self
                .read_sent_with_cookie(
                    world,
                    external_mail_id,
                    observed_at,
                    session.cookie(),
                    false,
                )
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
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_sent_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })?;
        // timeout_at 不抢占同步解析，也可能直接接受已就绪的缓存结果。
        if tokio::time::Instant::now() >= deadline {
            return Err(fetch_budget_exhausted(world));
        }
        result
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
        let mut session = tokio::time::timeout_at(
            deadline,
            self.credentials.begin_read(request.world, MoogleMailOperation::Sent),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "acquire_sent_session_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(request.world.as_str()),
                    ..Default::default()
                },
            );
            fetch_budget_exhausted(request.world)
        })??;
        tokio::time::timeout_at(deadline, async {
            match self.lookup_sent_with_cookie(&request, session.cookie(), deadline).await {
                Ok(report) => {
                    session.check_current().await?;
                    if tokio::time::Instant::now() >= deadline {
                        return Err(fetch_budget_exhausted(request.world));
                    }
                    Ok(report)
                }
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    Err(error)
                }
                Err(error) => Err(error),
            }
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "lookup_sent_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(request.world.as_str()),
                    ..Default::default()
                },
            );
            fetch_budget_exhausted(request.world)
        })?
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
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_sent_list_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(request.world.as_str()),
                    ..Default::default()
                },
            );
            fetch_budget_exhausted(request.world)
        })??;
        let mut candidates = Vec::new();
        for summary in list.summaries.iter().filter(|summary| {
            mail_recipient_is_list_projection(&summary.recipient, &request.recipient)
                && subject_is_list_projection(&summary.subject, &request.subject)
        }) {
            let detail_target =
                self.client.sent_detail(request.world, &summary.external_mail_id)?;
            let document = tokio::time::timeout_at(
                deadline,
                self.client.get_in_round(&detail_target, cookie, &round),
            )
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "read_sent_detail_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(request.world.as_str()),
                        ..Default::default()
                    },
                );
                fetch_budget_exhausted(request.world)
            })??;
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

    /// W10.9 恢复核对：枚举 `cutoff_at` 之后的 Sent 摘要；止于扫描预算与配置页数。
    /// 只读取列表，不读取详情，也不改变任何发送或领取事实。
    pub async fn scan_sent_since(
        &self,
        world: MoogleMailWorld,
        cutoff_at: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        maximum_pages: usize,
    ) -> Result<MoogleMailSentRecoveryScan, MoogleMailError> {
        if cutoff_at.timestamp_nanos_opt().is_none()
            || observed_at.timestamp_nanos_opt().is_none()
            || maximum_pages == 0
            || maximum_pages > self.config.maximum_pages
        {
            return Err(operation_error(
                MoogleMailOperation::Sent,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let mut session = tokio::time::timeout_at(
            deadline,
            self.credentials.begin_read(world, MoogleMailOperation::Sent),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "acquire_sent_recovery_session_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })??;
        let result: Result<MoogleMailSentRecoveryScan, MoogleMailError> =
            tokio::time::timeout_at(deadline, async {
                let round = self.client.read_round(session.cookie(), world, false)?;
                let list =
                    self.client.observe_sent_list(session.cookie(), &round, maximum_pages).await?;
                let observed_at = observed_at.min(round.observed_at());
                // 列表按新到旧排列：命中任何早于 cut-off 的行，即已覆盖 cut-off 之后的全部条目。
                let covers_cutoff = list.complete
                    || list.summaries.last().is_some_and(|row| row.sent_at < cutoff_at);
                let entries =
                    list.summaries.iter().filter(|row| row.sent_at >= cutoff_at).cloned().collect();
                self.client.check_read_round(session.cookie(), &round).await?;
                Ok(MoogleMailSentRecoveryScan {
                    entries,
                    covers_cutoff,
                    pages_scanned: list.pages_scanned,
                    observed_at,
                })
            })
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "scan_sent_recovery_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                fetch_budget_exhausted(world)
            })?;
        match result {
            Ok(scan) => {
                session.check_current().await?;
                Ok(scan)
            }
            Err(error) if error.is_session_expired() => {
                session.invalidate_current().await?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    /// W10.9 恢复核对：枚举 `cutoff_at` 之后的 Read Mail 摘要；不含正文，不改变已读状态。
    pub async fn scan_read_mail_since(
        &self,
        world: MoogleMailWorld,
        cutoff_at: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        maximum_pages: usize,
    ) -> Result<MoogleMailInboundRecoveryScan, MoogleMailError> {
        if cutoff_at.timestamp_nanos_opt().is_none()
            || observed_at.timestamp_nanos_opt().is_none()
            || maximum_pages == 0
            || maximum_pages > self.config.maximum_pages
        {
            return Err(operation_error(
                MoogleMailOperation::ReadMail,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let mut session = tokio::time::timeout_at(
            deadline,
            self.credentials.begin_read(world, MoogleMailOperation::ReadMail),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "acquire_read_mail_recovery_session_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })??;
        let result: Result<MoogleMailInboundRecoveryScan, MoogleMailError> =
            tokio::time::timeout_at(deadline, async {
                let round = self.client.read_round(session.cookie(), world, false)?;
                let list = self
                    .client
                    .observe_incoming_list(session.cookie(), &round, maximum_pages, true)
                    .await?;
                let observed_at = observed_at.min(round.observed_at());
                let covers_cutoff = list.complete
                    || list.summaries.last().is_some_and(|row| row.received_at < cutoff_at);
                let entries = list
                    .summaries
                    .iter()
                    .filter(|row| row.received_at >= cutoff_at)
                    .cloned()
                    .collect();
                self.client.check_read_round(session.cookie(), &round).await?;
                Ok(MoogleMailInboundRecoveryScan { entries, covers_cutoff, observed_at })
            })
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "scan_read_mail_recovery_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                fetch_budget_exhausted(world)
            })?;
        match result {
            Ok(scan) => {
                session.check_current().await?;
                Ok(scan)
            }
            Err(error) if error.is_session_expired() => {
                session.invalidate_current().await?;
                Err(error)
            }
            Err(error) => Err(error),
        }
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
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let result = tokio::time::timeout_at(deadline, async {
            let mut session =
                self.credentials.begin_read(world, MoogleMailOperation::Balance).await?;
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
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_balance_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })?;
        if tokio::time::Instant::now() >= deadline {
            return Err(fetch_budget_exhausted(world));
        }
        result
    }

    /// 只读读取一个 world、一个已批准分类的完整 Armory 装备身份集合。
    ///
    /// 与 `read_balance` 共用只读凭据会话与抓取预算；不执行 organize/解锁/寄送等任何写操作。
    pub async fn read_inventory_ids(
        &self,
        world: MoogleMailWorld,
        category: MoogleMailInventoryCategory,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailInventorySnapshot, MoogleMailError> {
        if observed_at.timestamp_nanos_opt().is_none() {
            return Err(operation_error(
                MoogleMailOperation::Inventory,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let result = tokio::time::timeout_at(deadline, async {
            let mut session =
                self.credentials.begin_read(world, MoogleMailOperation::Inventory).await?;
            let result = async {
                let target = self.client.inventory(world, category);
                let document = self.client.get(&target, session.cookie()).await?;
                self.client
                    .diagnose_parse(
                        &target,
                        &document,
                        self.parser.parse_inventory_ids(&document, world, category, observed_at),
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
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_inventory_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })?;
        if tokio::time::Instant::now() >= deadline {
            return Err(fetch_budget_exhausted(world));
        }
        result
    }

    pub(super) async fn read_sent_with_cookie(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        observed_at: DateTime<Utc>,
        cookie: &MoogleMailCookie,
        fresh: bool,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        // 履约证据不能早于 claim；普通已知 MID 读取使用新鲜私有轮次，不借用旧缓存时间。
        // 资金组合读取仍沿用原 fresh 写屏障及独占 session，不能把私有只读轮次当作资产授权。
        let round = if fresh {
            self.client.read_round(cookie, world, true)?
        } else {
            self.client.isolated_read_round(cookie, world)?
        };
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        tokio::time::timeout_at(deadline, async {
            let mut seen = HashSet::new();
            let mut summary = None;
            // 已知 MID 的正向列表身份证明不依赖无关尾页；每次履约观察保持独立新鲜。
            // 这不是新发送的基线/候选搜索，不能据此缩短后者的完整扫描或放宽资金证据期限。
            for page_index in 0..self.config.maximum_pages {
                let target = self.client.sent_page(world, page_index)?;
                let document = self.client.get_in_round(&target, cookie, &round).await?;
                let page = self
                    .client
                    .diagnose_parse(
                        &target,
                        &document,
                        self.parser.scan_sent_page(&document, world, page_index, &mut seen),
                    )
                    .await?;
                if tokio::time::Instant::now() >= deadline {
                    return Err(fetch_budget_exhausted(world));
                }
                if let Some(found) = page
                    .summaries
                    .into_iter()
                    .find(|entry| entry.external_mail_id == external_mail_id)
                {
                    summary = Some(found);
                    break;
                }
                if !page.has_next_page {
                    return Err(operation_error(
                        MoogleMailOperation::Sent,
                        world,
                        MoogleMailErrorCategory::NotObserved,
                    ));
                }
            }
            // 页数耗尽不是目标不存在，保持不完整错误；不得生成领取或结算成功证明。
            let summary = summary.ok_or_else(|| {
                operation_error(
                    MoogleMailOperation::Sent,
                    world,
                    MoogleMailErrorCategory::SentScanPageLimit,
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
                        &summary,
                        observed_at.min(round.observed_at()),
                    ),
                )
                .await?;
            if snapshot.external_mail_id != external_mail_id
                || snapshot.world != world
                || !mail_recipient_is_list_projection(&summary.recipient, &snapshot.recipient)
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
            if tokio::time::Instant::now() >= deadline {
                return Err(fetch_budget_exhausted(world));
            }
            Ok(snapshot)
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "build_sent_snapshot_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })?
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

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;
    use chrono::TimeZone;

    fn summary(id: &str, minute: u32) -> MoogleMailSummary {
        MoogleMailSummary {
            external_mail_id: id.to_owned(),
            world: MoogleMailWorld::Main,
            sender: "sender".to_owned(),
            subject: format!("subject-{id}"),
            received_at: Utc.with_ymd_and_hms(2026, 11, 4, 12, minute, 0).unwrap(),
        }
    }

    fn rows() -> Vec<MoogleMailSummary> {
        vec![
            summary("105", 9),
            summary("104", 8),
            summary("103", 7),
            summary("100", 6),
            summary("99", 5),
        ]
    }

    #[test]
    fn window_stops_at_known_cursor_without_reading_older_rows() {
        let window = incoming_window_rows(&rows(), Some("103"), 10);
        assert_eq!(
            window.iter().map(|summary| summary.external_mail_id.as_str()).collect::<Vec<_>>(),
            vec!["105", "104"],
            "游标之前的行不进入窗口"
        );
    }

    #[test]
    fn window_limits_keep_oldest_backlog_for_next_round() {
        let window = incoming_window_rows(&rows(), Some("99"), 2);
        assert_eq!(
            window.iter().map(|summary| summary.external_mail_id.as_str()).collect::<Vec<_>>(),
            vec!["103", "100"],
            "超过上限时保留最旧窗口；未进入窗口的新邮件仍大于游标，下一轮重新可见"
        );
    }

    #[test]
    fn new_mail_ids_exclude_cursor_and_older_rows() {
        let ids = new_mail_ids(&rows(), Some("100"));
        assert_eq!(ids, HashSet::from(["105".to_owned(), "104".to_owned(), "103".to_owned()]));
        let all = new_mail_ids(&rows(), None);
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn prefix_scan_early_stops_on_cursor() {
        let mut scan =
            auction_application::MailPrefixScan::new(MailScanAnchor::from_cursor("103").unwrap());
        let mut seen = Vec::new();
        for summary in rows() {
            match scan.observe(&summary.external_mail_id, summary.received_at) {
                Ok(auction_application::MailPrefixPage::Continue) => seen.push(summary),
                Ok(auction_application::MailPrefixPage::Done) => {
                    assert!(scan.anchor_found());
                    break;
                }
                Err(reason) => panic!("前缀不应失败：{reason:?}"),
            }
        }
        assert_eq!(seen.len(), 2, "只读取锚点之前的 105 与 104");
        assert_eq!(seen[0].external_mail_id, "105");
    }

    #[test]
    fn pending_anchor_uses_lowest_known_target() {
        let window = VecDeque::from(vec![summary("105", 9), summary("100", 6), summary("103", 7)]);
        assert_eq!(pending_anchor(&window).unwrap().mail_id, "100");
        assert_eq!(pending_anchor(&VecDeque::new()), None);
        let invalid = VecDeque::from(vec![summary("not-a-mid", 1)]);
        assert_eq!(pending_anchor(&invalid), None, "无效 MID 不能成为锚点");
    }

    #[test]
    fn prefix_scan_falls_back_when_cursor_disappears() {
        let mut scan =
            auction_application::MailPrefixScan::new(MailScanAnchor::from_cursor("101").unwrap());
        assert_eq!(
            scan.observe("105", Utc.with_ymd_and_hms(2026, 11, 4, 12, 9, 0).unwrap()),
            Ok(auction_application::MailPrefixPage::Continue)
        );
        assert_eq!(
            scan.observe("100", Utc.with_ymd_and_hms(2026, 11, 4, 12, 6, 0).unwrap()),
            Err(auction_application::MailScanReason::AnchorMissing),
            "跨过锚点却未见到锚点行时必须回退完整扫描"
        );
    }
}
