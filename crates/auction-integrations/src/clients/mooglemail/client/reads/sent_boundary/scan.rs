//! 发送前后共用的 Sent 分钟组扫描，始终受原页数、字节、截止点与读取屏障约束。

use std::{collections::HashSet, mem::size_of};

use auction_application::SentBaselineReason;
use tokio::time::Instant;

use super::{SentBoundary, SentBoundaryScan, SentOrder};
use crate::clients::mooglemail::{
    MoogleMailClient, MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory,
    MoogleMailOperation, MoogleMailParser, MoogleMailSentSummary,
    client::{MoogleMailReadRound, reads::lists::validate_pages},
};

impl MoogleMailClient {
    /// None 读取完整最新分钟组；Some 读取锚点及其完整分钟组，Empty 必须读到末页。
    pub(crate) async fn observe_sent_boundary(
        &self,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        maximum_pages: usize,
        boundary: Option<&SentBoundary>,
        deadline: Instant,
    ) -> Result<SentBoundaryScan, MoogleMailError> {
        let world = round.key.world;
        self.check_read_round(cookie, round).await?;
        validate_pages(maximum_pages, world)?;
        let mut result = SentBoundaryScan {
            summaries: Vec::new(),
            pages_scanned: 0,
            complete: false,
            bounded: false,
            issue: None,
            bytes: self.retain_list_bytes(0, world)?,
        };
        let mut order = SentOrder::default();
        let mut minute = boundary.and_then(SentBoundary::sent_at);
        for page in 0..maximum_pages {
            if Instant::now() >= deadline {
                return Err(MoogleMailError::new(
                    MoogleMailOperation::Sent,
                    world,
                    MoogleMailErrorCategory::SentScanTimeout,
                    true,
                ));
            }
            let target = self.sent_page(world, page)?;
            let document = self.get_in_round(&target, cookie, round).await?;
            // 各页独立解析，再校验跨页重复；位移不能被去重悄悄解释为连续前缀。
            let parsed = self
                .diagnose_parse(
                    &target,
                    &document,
                    MoogleMailParser.scan_sent_page(&document, world, page, &mut HashSet::new()),
                )
                .await?;
            if boundary.is_none() && minute.is_none() {
                minute = parsed.summaries.first().map(|summary| summary.sent_at);
            }
            let previous_capacity = result.summaries.capacity();
            let payload = parsed
                .summaries
                .iter()
                .map(|summary| {
                    summary.external_mail_id.capacity()
                        + summary.recipient.capacity()
                        + summary.subject.capacity()
                })
                .sum::<usize>();
            for summary in &parsed.summaries {
                let checked = order.observe(summary).and_then(|()| match boundary {
                    Some(boundary) => boundary.check_row(summary),
                    None => Ok(()),
                });
                if let Err(reason) = checked {
                    // 分页重叠可补齐枚举；同页后续发现身份/时间前提失效时不能降级成可恢复问题。
                    if result.issue.is_none()
                        || result.issue == Some(SentBaselineReason::PageDiscontinuity)
                    {
                        result.issue = Some(reason);
                    }
                }
                if minute.is_some_and(|minute| summary.sent_at < minute) {
                    result.bounded = true;
                }
            }
            result.summaries.extend(parsed.summaries);
            result.bytes.merge(self.retain_list_bytes(
                payload
                    + (result.summaries.capacity() - previous_capacity)
                        * size_of::<MoogleMailSentSummary>(),
                world,
            )?);
            result.pages_scanned += 1;
            result.complete = !parsed.has_next_page;
            result.bounded |= result.complete;
            if result.bounded || result.issue.is_some() {
                break;
            }
        }
        self.check_read_round(cookie, round).await?;
        Ok(result)
    }
}
