//! Inbox 与 Read Mail 的完整扫描和增量前缀；成员关系读锁按段释放，详情读取得以推进。

use std::sync::{Arc, atomic::Ordering};

use super::super::{MoogleMailReadRound, lock_incoming_segment, read_unavailable};
use super::{
    INCOMING_SCAN_SEGMENT_PAGES, MoogleMailIncomingList, MoogleMailIncomingPrefix, validate_pages,
};
use crate::clients::mooglemail::client::MoogleMailClient;
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation,
};

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::safe_diagnostics;

use std::{collections::HashSet, mem::size_of};

use auction_application::mail::SegmentPlan;

use crate::clients::metrics::MailLockWaitGuard;
use crate::clients::mooglemail::parser::{MoogleMailParser, MoogleMailSummary};

impl MoogleMailClient {
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
                        auction_application::external_metrics::MailLock::IncomingRead,
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
        anchor: &auction_application::mail::MailScanAnchor,
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
        let mut scan = auction_application::mail::MailPrefixScan::new(anchor.clone());
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
                auction_application::external_metrics::MailLock::IncomingRead,
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
                        Ok(auction_application::mail::MailPrefixPage::Continue) => {}
                        Ok(auction_application::mail::MailPrefixPage::Done) => {
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
            auction_application::mail::MailPrefixOutcome::Fallback(reason)
        } else {
            scan.finish(page_limit, list_complete)
        };
        let (anchor_found, newest, issue) = match outcome {
            auction_application::mail::MailPrefixOutcome::Complete { newest } => {
                (true, newest, None)
            }
            auction_application::mail::MailPrefixOutcome::Fallback(reason) => {
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
}
