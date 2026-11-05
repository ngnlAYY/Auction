//! 三种真实邮箱列表的有界完整扫描；列表身份与页数一起缓存，缺页不会伪装为完整。

use auction_application::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use std::{collections::HashSet, mem::size_of};

use tokio::sync::OnceCell;

use auction_application::SegmentPlan;

use super::*;
use crate::clients::metrics::MailLockWaitGuard;
use crate::clients::mooglemail::parser::{
    MoogleMailParser, MoogleMailSentSummary, MoogleMailSummary,
};

/// 每次持有成员关系读锁的最大页数；段间必须释放，详情 GET 才有机会推进。
const INCOMING_SCAN_SEGMENT_PAGES: usize = 4;

type ListCell<T> = Arc<OnceCell<Arc<T>>>;

#[derive(Default)]
pub(super) struct ListCells {
    sent: StdMutex<HashMap<usize, ListCell<MoogleMailSentList>>>,
    incoming: StdMutex<HashMap<(bool, usize, u64), ListCell<MoogleMailIncomingList>>>,
}

pub(crate) struct MoogleMailSentList {
    pub(crate) summaries: Vec<MoogleMailSentSummary>,
    pub(crate) pages_scanned: usize,
    pub(crate) complete: bool,
    _bytes: OwnedSemaphorePermit,
}

/// 只枚举比基线边界更新的 Sent 前缀；`bounded` 表示更旧的邮件全部已在基线中。
pub(crate) struct MoogleMailSentPrefix {
    pub(crate) summaries: Vec<MoogleMailSentSummary>,
    pub(crate) bounded: bool,
    _bytes: OwnedSemaphorePermit,
}

pub(crate) struct MoogleMailIncomingList {
    pub(crate) summaries: Vec<MoogleMailSummary>,
    pub(crate) complete: bool,
    _bytes: OwnedSemaphorePermit,
}

/// 从持久水位继续的有界前缀；`anchor_found` 才表示“更新的邮件已全部枚举”。
/// 本结果只用于缩小观察窗口，永远不是收取、已读或发送证明。
pub(crate) struct MoogleMailIncomingPrefix {
    pub(crate) summaries: Vec<MoogleMailSummary>,
    pub(crate) anchor_found: bool,
    pub(crate) newest: Option<auction_application::MailScanWatermark>,
    /// 回退原因；调用方据此记录指标并完整扫描。
    pub(crate) issue: Option<auction_application::MailScanReason>,
    pub(crate) pages_scanned: usize,
    _bytes: OwnedSemaphorePermit,
}

impl MoogleMailClient {
    /// 同一轮次完整 Sent 摘要只解析一次；履约逐 MID 读取不重复扫描全网列表。
    pub(crate) async fn observe_sent_list(
        &self,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        maximum_pages: usize,
    ) -> Result<Arc<MoogleMailSentList>, MoogleMailError> {
        let world = round.key.world;
        self.check_read_round(cookie, round).await?;
        validate_pages(maximum_pages, world)?;
        let cell = {
            let mut cells = round.epoch.lists.sent.lock().map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "lock_sent_cache",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                read_unavailable(world)
            })?;
            if cells.len() >= 8 && !cells.contains_key(&maximum_pages) {
                return Err(read_unavailable(world));
            }
            Arc::clone(cells.entry(maximum_pages).or_default())
        };
        let list = cell
            .get_or_try_init(|| async {
                let mut summaries = Vec::new();
                let mut seen = HashSet::new();
                let mut pages_scanned = 0;
                let mut complete = false;
                let mut bytes = self.retain_list_bytes(0, world)?;
                for page in 0..maximum_pages {
                    let target = self.sent_page(world, page)?;
                    let document = self.get_in_round(&target, cookie, round).await?;
                    let parsed = self
                        .diagnose_parse(
                            &target,
                            &document,
                            MoogleMailParser.scan_sent_page(&document, world, page, &mut seen),
                        )
                        .await?;
                    let previous_capacity = summaries.capacity();
                    let payload_bytes = parsed
                        .summaries
                        .iter()
                        .map(|summary| {
                            summary.external_mail_id.capacity()
                                + summary.recipient.capacity()
                                + summary.subject.capacity()
                        })
                        .sum::<usize>();
                    summaries.extend(parsed.summaries);
                    // 逐页计费，不能先积累整箱摘要后才检查内存上限。
                    bytes.merge(self.retain_list_bytes(
                        payload_bytes
                            + (summaries.capacity() - previous_capacity)
                                * size_of::<MoogleMailSentSummary>(),
                        world,
                    )?);
                    pages_scanned += 1;
                    if !parsed.has_next_page {
                        complete = true;
                        break;
                    }
                }
                self.check_read_round(cookie, round).await?;
                Ok::<_, MoogleMailError>(Arc::new(MoogleMailSentList {
                    summaries,
                    pages_scanned,
                    complete,
                    _bytes: bytes,
                }))
            })
            .await?;
        self.check_read_round(cookie, round).await?;
        Ok(Arc::clone(list))
    }

    /// 从最新页向下扫描，直到越过基线边界或末页。
    ///
    /// 列表按 `sent_at` 降序（实测连续 60 行不递增）；任一基线邮件 R 在捕获基线时已存在，
    /// 之后创建的邮件必然排在 R 之前，因此读到第一行比 R 更旧时即可停止。同分钟并列的行
    /// 全部读完才停止，避免并列顺序丢失新邮件。结果不写入轮次缓存，仍受页数、字节与
    /// 读取屏障约束。
    pub(crate) async fn observe_sent_prefix(
        &self,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        maximum_pages: usize,
        baseline: &HashSet<String>,
    ) -> Result<Arc<MoogleMailSentPrefix>, MoogleMailError> {
        let world = round.key.world;
        self.check_read_round(cookie, round).await?;
        validate_pages(maximum_pages, world)?;
        let mut summaries = Vec::new();
        let mut seen = HashSet::new();
        let mut bytes = self.retain_list_bytes(0, world)?;
        let mut boundary = None;
        let mut bounded = false;
        for page in 0..maximum_pages {
            let target = self.sent_page(world, page)?;
            let document = self.get_in_round(&target, cookie, round).await?;
            let parsed = self
                .diagnose_parse(
                    &target,
                    &document,
                    MoogleMailParser.scan_sent_page(&document, world, page, &mut seen),
                )
                .await?;
            let previous_capacity = summaries.capacity();
            let payload_bytes = parsed
                .summaries
                .iter()
                .map(|summary| {
                    summary.external_mail_id.capacity()
                        + summary.recipient.capacity()
                        + summary.subject.capacity()
                })
                .sum::<usize>();
            let mut stopped = false;
            for summary in parsed.summaries {
                if boundary.is_some_and(|stop_at| summary.sent_at < stop_at) {
                    stopped = true;
                    break;
                }
                if boundary.is_none() && baseline.contains(&summary.external_mail_id) {
                    boundary = Some(summary.sent_at);
                }
                summaries.push(summary);
            }
            bytes.merge(self.retain_list_bytes(
                payload_bytes
                    + (summaries.capacity() - previous_capacity)
                        * size_of::<MoogleMailSentSummary>(),
                world,
            )?);
            if stopped {
                bounded = true;
                break;
            }
            if !parsed.has_next_page {
                bounded = true;
                break;
            }
        }
        self.check_read_round(cookie, round).await?;
        Ok(Arc::new(MoogleMailSentPrefix { summaries, bounded, _bytes: bytes }))
    }

    /// Read Mail 只返回历史摘要；新入站业务必须来自 Inbox 窗口或既有持久待读账本。
    /// 整个分页过程与改变列表成员关系的详情 GET 互斥，而不占用 compose/资产 guard。
    pub(crate) async fn observe_incoming_list(
        &self,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        maximum_pages: usize,
        read_mail: bool,
    ) -> Result<Arc<MoogleMailIncomingList>, MoogleMailError> {
        let world = round.key.world;
        self.check_read_round(cookie, round).await?;
        validate_pages(maximum_pages, world)?;
        let operation =
            if read_mail { MoogleMailOperation::ReadMail } else { MoogleMailOperation::Inbox };
        let membership = round.scope.membership_revision.load(Ordering::Acquire);
        let key = (read_mail, maximum_pages, membership);
        let cell = {
            let mut cells = round.epoch.lists.incoming.lock().map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "lock_inbox_cache",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                read_unavailable(world)
            })?;
            cells.retain(|(_, _, revision), _| *revision == membership);
            if cells.len() >= 8 && !cells.contains_key(&key) {
                return Err(read_unavailable(world));
            }
            Arc::clone(cells.entry(key).or_default())
        };
        let list = cell
            .get_or_try_init(|| async {
                let mut summaries = Vec::new();
                let mut seen = HashSet::new();
                let mut complete = false;
                let mut bytes = self.retain_list_bytes(0, world)?;
                // 长列表按段释放成员关系读锁：详情 GET 可在段间推进，下一次取锁发现
                // revision 改变即放弃本次扫描，避免整轮持锁拖住写入。
                let mut plan = SegmentPlan::new(maximum_pages, INCOMING_SCAN_SEGMENT_PAGES);
                'segments: while let Some(pages) = plan.next_segment() {
                    let lock_wait = MailLockWaitGuard::start(
                        Arc::clone(&self.external_metrics),
                        world,
                        operation,
                        auction_application::MailLock::IncomingRead,
                    );
                    let segment = lock_incoming_segment(
                        &round.scope.incoming,
                        &round.scope.membership_revision,
                        membership,
                        world,
                    )
                    .await?;
                    lock_wait.acquired();
                    for page in pages {
                        let target = if read_mail {
                            self.read_mail_page(world, page)?
                        } else {
                            self.inbox_page(world, page)?
                        };
                        let document = self.get_in_round(&target, cookie, round).await?;
                        let parsed = if read_mail {
                            MoogleMailParser.scan_read_mail_page(&document, world, page, &mut seen)
                        } else {
                            MoogleMailParser.scan_inbox_page(&document, world, page, &mut seen)
                        };
                        let parsed = self.diagnose_parse(&target, &document, parsed).await?;
                        let previous_capacity = summaries.capacity();
                        let payload_bytes = parsed
                            .summaries
                            .iter()
                            .map(|summary| {
                                summary.external_mail_id.capacity()
                                    + summary.sender.capacity()
                                    + summary.subject.capacity()
                            })
                            .sum::<usize>();
                        summaries.extend(parsed.summaries);
                        bytes.merge(self.retain_list_bytes(
                            payload_bytes
                                + (summaries.capacity() - previous_capacity)
                                    * size_of::<MoogleMailSummary>(),
                            world,
                        )?);
                        if !parsed.has_next_page {
                            complete = true;
                            break 'segments;
                        }
                    }
                    drop(segment);
                }
                self.check_read_round(cookie, round).await?;
                Ok::<_, MoogleMailError>(Arc::new(MoogleMailIncomingList {
                    summaries,
                    complete,
                    _bytes: bytes,
                }))
            })
            .await?;
        self.check_read_round(cookie, round).await?;
        Ok(Arc::clone(list))
    }

    /// 从已知 MID 锚点向下读取，遇到锚点行即停；缺锚点、重复或超限由状态机回退。
    ///
    /// 与完整列表共用成员关系读锁、分段释放、字节预算与读取屏障；每页使用独立去重集，
    /// 跨页重复交给前缀状态机判定，避免 parser 静默跳行被误认为连续前缀。
    pub(crate) async fn observe_incoming_prefix(
        &self,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        maximum_pages: usize,
        read_mail: bool,
        anchor: &auction_application::MailScanAnchor,
    ) -> Result<MoogleMailIncomingPrefix, MoogleMailError> {
        let world = round.key.world;
        self.check_read_round(cookie, round).await?;
        validate_pages(maximum_pages, world)?;
        if !anchor.validate() {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                world,
                MoogleMailErrorCategory::Configuration,
                false,
            ));
        }
        let operation =
            if read_mail { MoogleMailOperation::ReadMail } else { MoogleMailOperation::Inbox };
        let membership = round.scope.membership_revision.load(Ordering::Acquire);
        let mut scan = auction_application::MailPrefixScan::new(anchor.clone());
        let mut summaries = Vec::new();
        let mut issue = None;
        let mut pages_scanned = 0;
        let mut list_complete = false;
        let mut anchor_found = false;
        let mut bytes = self.retain_list_bytes(0, world)?;
        let mut plan = SegmentPlan::new(maximum_pages, INCOMING_SCAN_SEGMENT_PAGES);
        'segments: while let Some(pages) = plan.next_segment() {
            let lock_wait = MailLockWaitGuard::start(
                Arc::clone(&self.external_metrics),
                world,
                operation,
                auction_application::MailLock::IncomingRead,
            );
            let segment = lock_incoming_segment(
                &round.scope.incoming,
                &round.scope.membership_revision,
                membership,
                world,
            )
            .await?;
            lock_wait.acquired();
            for page in pages {
                let target = if read_mail {
                    self.read_mail_page(world, page)?
                } else {
                    self.inbox_page(world, page)?
                };
                let document = self.get_in_round(&target, cookie, round).await?;
                // 每页独立去重；跨页重复由 MailPrefixScan 判定为 PageDiscontinuity。
                let mut page_seen = HashSet::new();
                let parsed = if read_mail {
                    MoogleMailParser.scan_read_mail_page(&document, world, page, &mut page_seen)
                } else {
                    MoogleMailParser.scan_inbox_page(&document, world, page, &mut page_seen)
                };
                let parsed = self.diagnose_parse(&target, &document, parsed).await?;
                let previous_capacity = summaries.capacity();
                let payload_bytes = parsed
                    .summaries
                    .iter()
                    .map(|summary| {
                        summary.external_mail_id.capacity()
                            + summary.sender.capacity()
                            + summary.subject.capacity()
                    })
                    .sum::<usize>();
                let mut stop = false;
                for summary in &parsed.summaries {
                    match scan.observe(&summary.external_mail_id, summary.received_at) {
                        Ok(auction_application::MailPrefixPage::Continue) => {}
                        Ok(auction_application::MailPrefixPage::Done) => {
                            anchor_found = true;
                            stop = true;
                        }
                        Err(reason) => {
                            issue = Some(reason);
                            stop = true;
                            break;
                        }
                    }
                }
                if issue.is_none() {
                    summaries.extend(parsed.summaries);
                }
                bytes.merge(self.retain_list_bytes(
                    payload_bytes
                        + (summaries.capacity() - previous_capacity)
                            * size_of::<MoogleMailSummary>(),
                    world,
                )?);
                pages_scanned += 1;
                if stop {
                    break 'segments;
                }
                if !parsed.has_next_page {
                    list_complete = true;
                    break 'segments;
                }
            }
            drop(segment);
        }
        self.check_read_round(cookie, round).await?;
        let page_limit = !anchor_found && !list_complete && issue.is_none();
        let outcome = if let Some(reason) = issue {
            auction_application::MailPrefixOutcome::Fallback(reason)
        } else {
            scan.finish(page_limit, list_complete)
        };
        let (anchor_found, newest, issue) = match outcome {
            auction_application::MailPrefixOutcome::Complete { newest } => (true, newest, None),
            auction_application::MailPrefixOutcome::Fallback(reason) => {
                tracing::debug!(
                    world = world.as_str(),
                    reason = reason.label(),
                    "MoogleMail 前缀扫描回退完整扫描"
                );
                summaries.clear();
                (false, None, Some(reason))
            }
        };
        Ok(MoogleMailIncomingPrefix {
            summaries,
            anchor_found,
            newest,
            issue,
            pages_scanned,
            _bytes: bytes,
        })
    }

    pub(super) fn retain_list_bytes(
        &self,
        bytes: usize,
        world: MoogleMailWorld,
    ) -> Result<OwnedSemaphorePermit, MoogleMailError> {
        let bytes = u32::try_from(bytes).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "convert_body_budget",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            read_unavailable(world)
        })?;
        Arc::clone(&self.reads.bytes).try_acquire_many_owned(bytes).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "reserve_body_budget",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            read_unavailable(world)
        })
    }
}

pub(super) fn validate_pages(
    maximum_pages: usize,
    world: MoogleMailWorld,
) -> Result<(), MoogleMailError> {
    if !(1..=100).contains(&maximum_pages) {
        return Err(MoogleMailError::new(
            MoogleMailOperation::Inbox,
            world,
            MoogleMailErrorCategory::Configuration,
            false,
        ));
    }
    Ok(())
}
