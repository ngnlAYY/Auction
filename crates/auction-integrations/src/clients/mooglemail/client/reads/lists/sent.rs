//! Sent 列表的完整扫描、增量前缀与严格前缀；前缀按分钟降序与唯一锚点校验。

use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::super::{MoogleMailReadRound, read_unavailable};
use super::{MoogleMailSentList, MoogleMailSentPrefix, validate_pages};
use crate::clients::mooglemail::client::MoogleMailClient;
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation,
    MoogleMailWorld,
};

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::safe_diagnostics;

use std::{collections::HashSet, mem::size_of};

use auction_application::mail::{MailScanAnchor, MailScanReason};

use crate::clients::mooglemail::parser::{MoogleMailParser, MoogleMailSentSummary, protocol};

/// Sent 前缀的严格顺序状态：分钟降序，同分钟 MID 可任意排列；锚点必须唯一且时间一致。
struct SentPrefixState {
    anchor_mail_id: String,
    anchor_at: DateTime<Utc>,
    seen: HashSet<String>,
    previous_at: Option<DateTime<Utc>>,
    anchor_found: bool,
    older_seen: bool,
    anchor_sent_at: Option<DateTime<Utc>>,
}

impl SentPrefixState {
    fn new(anchor: &MailScanAnchor) -> Result<Self, MailScanReason> {
        let anchor_at = anchor.observed_at.ok_or(MailScanReason::InvalidMid)?;
        Ok(Self {
            anchor_mail_id: anchor.mail_id.clone(),
            anchor_at,
            seen: HashSet::new(),
            previous_at: None,
            anchor_found: false,
            older_seen: false,
            anchor_sent_at: None,
        })
    }

    fn observe(&mut self, mail_id: &str, sent_at: DateTime<Utc>) -> Result<(), MailScanReason> {
        if sent_at.timestamp_nanos_opt().is_none() {
            return Err(MailScanReason::InvalidMid);
        }
        if !self.seen.insert(mail_id.to_owned()) {
            return Err(MailScanReason::PageDiscontinuity);
        }
        if self.previous_at.is_some_and(|previous| sent_at > previous) {
            return Err(MailScanReason::OrderViolation);
        }
        self.previous_at = Some(sent_at);
        if self.anchor_found {
            if sent_at < self.anchor_at {
                self.older_seen = true;
            }
            return Ok(());
        }
        if mail_id == self.anchor_mail_id {
            if sent_at != self.anchor_at {
                return Err(MailScanReason::AnchorChanged);
            }
            self.anchor_found = true;
            self.anchor_sent_at = Some(sent_at);
            return Ok(());
        }
        // 锚点之前只允许同一分钟的更小/更大 MID；遇到更旧分钟即证明锚点缺失。
        if sent_at < self.anchor_at {
            return Err(MailScanReason::AnchorMissing);
        }
        Ok(())
    }
}

fn strict_prefix_error(world: MoogleMailWorld, reason: MailScanReason) -> MoogleMailError {
    let category = if reason == MailScanReason::PageLimit {
        MoogleMailErrorCategory::SentScanPageLimit
    } else {
        MoogleMailErrorCategory::ProtocolDrift
    };
    tracing::warn!(
        world = world.as_str(),
        reason = reason.label(),
        "MoogleMail Sent 严格前缀异常，回退完整扫描"
    );
    MoogleMailError::new(MoogleMailOperation::Sent, world, category, false)
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
        let mut anchor_sent_at = None;
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
                    anchor_sent_at = Some(summary.sent_at);
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
        Ok(Arc::new(MoogleMailSentPrefix { summaries, bounded, anchor_sent_at, _bytes: bytes }))
    }

    /// 维护专用严格前缀：校验锚点、整页/跨页时间与 MID 连续；任何异常返回错误，由调用方回退完整扫描。
    pub(crate) async fn observe_sent_prefix_strict(
        &self,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        maximum_pages: usize,
        anchor: &MailScanAnchor,
    ) -> Result<Arc<MoogleMailSentPrefix>, MoogleMailError> {
        let world = round.key.world;
        self.check_read_round(cookie, round).await?;
        validate_pages(maximum_pages, world)?;
        if !anchor.validate() || anchor.observed_at.is_none() {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }
        let mut scan =
            SentPrefixState::new(anchor).map_err(|reason| strict_prefix_error(world, reason))?;
        let mut summaries = Vec::new();
        let mut seen = HashSet::new();
        let mut bytes = self.retain_list_bytes(0, world)?;
        let mut page_limit_reached = false;
        for page in 0..maximum_pages {
            let target = self.sent_page(world, page)?;
            let document = self.get_in_round(&target, cookie, round).await?;
            let parsed = self
                .diagnose_parse(
                    &target,
                    &document,
                    MoogleMailParser.scan_sent_page_strict(&document, world, page, &mut seen),
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
            for summary in &parsed.summaries {
                scan.observe(&summary.external_mail_id, summary.sent_at)
                    .map_err(|reason| strict_prefix_error(world, reason))?;
                // 锚点后只保留同分钟行；更旧行仍用于顺序校验但不进入覆盖结果。
                if !scan.anchor_found || summary.sent_at >= scan.anchor_at {
                    summaries.push(summary.clone());
                }
            }
            bytes.merge(self.retain_list_bytes(
                payload_bytes
                    + (summaries.capacity() - previous_capacity)
                        * size_of::<MoogleMailSentSummary>(),
                world,
            )?);
            if scan.anchor_found {
                // 锚点后同分钟的行可能跨页；必须继续到看到严格更早分钟或真实末页。
                if scan.older_seen || !parsed.has_next_page {
                    break;
                }
            } else if !parsed.has_next_page {
                break;
            }
            if page + 1 == maximum_pages {
                page_limit_reached = true;
            }
        }
        if !scan.anchor_found {
            return Err(strict_prefix_error(world, MailScanReason::AnchorMissing));
        }
        if page_limit_reached && !scan.older_seen {
            return Err(strict_prefix_error(world, MailScanReason::PageLimit));
        }
        self.check_read_round(cookie, round).await?;
        Ok(Arc::new(MoogleMailSentPrefix {
            summaries,
            bounded: true,
            anchor_sent_at: scan.anchor_sent_at,
            _bytes: bytes,
        }))
    }
}

#[cfg(test)]
mod sent_prefix_tests {
    use super::SentPrefixState;
    use auction_application::mail::MailScanAnchor;
    use chrono::{TimeZone, Utc};

    #[test]
    fn same_minute_lower_mid_is_allowed_but_time_reversal_is_not() {
        let at = Utc.with_ymd_and_hms(2026, 11, 7, 6, 10, 0).single().expect("time");
        let mut state = SentPrefixState::new(&MailScanAnchor {
            mail_id: "100".to_owned(),
            observed_at: Some(at),
        })
        .expect("anchor time");
        assert!(state.observe("99", at).is_ok(), "同分钟更小 MID 可以出现在锚点前");
        assert!(state.observe("100", at).is_ok());
        assert!(state.observe("98", at - chrono::Duration::minutes(10)).is_ok());
        assert!(state.older_seen);
        assert!(state.observe("101", at).is_err(), "时间上升必须回退");
    }

    #[test]
    fn older_minute_before_anchor_is_anchor_missing() {
        let at = Utc.with_ymd_and_hms(2026, 11, 7, 6, 10, 0).single().expect("time");
        let mut state = SentPrefixState::new(&MailScanAnchor {
            mail_id: "100".to_owned(),
            observed_at: Some(at),
        })
        .expect("anchor time");
        assert!(state.observe("99", at - chrono::Duration::minutes(10)).is_err());
    }
}
