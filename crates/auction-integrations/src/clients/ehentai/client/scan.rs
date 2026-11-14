//! 主题整页与增量读取：分页发现、并发受限的分页读取、楼层换算与扫描证明。

use auction_application::admin::AdminForumPreviewResult;
use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::external_metrics::ForumTopicScanMode;
use auction_application::forum::{ForumTopicCursor, ForumTopicScan};
use auction_domain::SettlementWorld;

use crate::clients::metrics::{ForumTopicScanGuard, forum_operation, forum_world};
use crate::safe_diagnostics;

use std::{collections::VecDeque, sync::Arc, sync::atomic::AtomicUsize};

use url::Url;

use super::super::forms::{valid_cookie, valid_topic_id};
use super::super::{
    EhentaiError, EhentaiOperation, HARD_MAXIMUM_TOPIC_BYTES, MAXIMUM_CONCURRENT_TOPIC_READS,
};
use super::pagination;
use super::{EhentaiClient, EhentaiCookie, ForumRequestScope, TopicPageReadContext};

impl EhentaiClient {
    /// 读取一个已发布主题，并复用论坛预览适配器解析帖子命令。
    pub async fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cookie: &EhentaiCookie,
    ) -> Result<AdminForumPreviewResult, EhentaiError> {
        let mut scan = ForumTopicScanGuard::start(self.external_metrics(), forum_world(world));
        let result = self.fetch_topic_inner(world, thread_id, cookie, &mut scan).await;
        if result.is_ok() {
            scan.finish_complete();
        } else {
            scan.finish_error();
        }
        result
    }

    /// 按游标读取主题：固定楼层页 + 游标页及其之后的分页。
    ///
    /// 先核对游标页仍包含游标帖子；删帖导致楼层前移或帖子缺失时回退完整读取，
    /// 因此增量只是减少分页请求，不改变任何帖子、命令或固定楼层的解释结果。
    pub async fn fetch_topic_incremental(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cookie: &EhentaiCookie,
        cursor: Option<&ForumTopicCursor>,
        fixed_through_post_number: u32,
    ) -> Result<ForumTopicScan, EhentaiError> {
        let mut scan = ForumTopicScanGuard::start(self.external_metrics(), forum_world(world));
        let result = self
            .fetch_topic_scan(
                world,
                thread_id,
                cookie,
                &mut scan,
                cursor,
                fixed_through_post_number,
            )
            .await;
        if result.is_ok() {
            scan.finish_complete();
        } else {
            scan.finish_error();
        }
        result
    }

    async fn fetch_topic_inner(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cookie: &EhentaiCookie,
        scan: &mut ForumTopicScanGuard,
    ) -> Result<AdminForumPreviewResult, EhentaiError> {
        self.fetch_topic_scan(world, thread_id, cookie, scan, None, 0)
            .await
            .map(|scan| scan.preview)
    }

    async fn fetch_topic_scan(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cookie: &EhentaiCookie,
        scan: &mut ForumTopicScanGuard,
        cursor: Option<&ForumTopicCursor>,
        fixed_through_post_number: u32,
    ) -> Result<ForumTopicScan, EhentaiError> {
        let operation = EhentaiOperation::FetchTopic;
        if !self.valid_for() || !valid_cookie(cookie) || !valid_topic_id(thread_id) {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world);
        if board_id <= 0 {
            return Err(EhentaiError::configuration(operation));
        }
        let target = self.index_url(&[("showtopic", thread_id)], operation)?;
        let (body, _) = self
            .get_page(
                operation,
                forum_operation(operation),
                ForumRequestScope::new(forum_world(world), cookie),
                target.clone(),
                Some(&target),
            )
            .await?;
        let html = String::from_utf8(body).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "decode_first_page",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let parser = crate::ForumPreviewAdapter::new();
        let first_page = parser.parse_html_page(&html).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "parse_first_page",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        scan.set_pages(1);
        let targets =
            self.topic_page_targets(&first_page, thread_id, operation, forum_world(world))?;
        // 目标页数已受扫描上限约束；usize 超出 u32 只可能来自损坏的页表，
        // 此时截断为 u32::MAX，仍会触发上层页数告警。
        let total_pages = u32::try_from(targets.len()).unwrap_or(u32::MAX);
        let maximum_offset = targets.last().map_or(0, |(offset, _)| *offset);
        let mut pages = Vec::with_capacity(targets.len());
        let mut total_bytes = html.len();
        drop(html);
        pages.push((0_u64, first_page));
        scan.set_pages(pages.len());
        let read_context = TopicPageReadContext {
            client: self.clone(),
            cookie: cookie.clone(),
            operation,
            metrics_world: forum_world(world),
            thread_id: thread_id.to_owned(),
            known_maximum_offset: maximum_offset,
            referer: target,
            // 在逐块读取时共享剩余额度，不能等并发整页下载、解析后才发现超限。
            remaining_bytes: Arc::new(AtomicUsize::new(HARD_MAXIMUM_TOPIC_BYTES - total_bytes)),
        };
        // 增量预算：只读固定楼层页与游标页之后；游标页先单独核对，失败即整场重读。
        let (selected_pages, probed_offset) = self
            .incremental_scan_plan(
                &targets,
                cursor,
                fixed_through_post_number,
                &read_context,
                pagination::IncrementalScanState {
                    pages: &mut pages,
                    total_bytes: &mut total_bytes,
                },
                operation,
            )
            .await?;
        let mode = if selected_pages.len() == targets.len() {
            ForumTopicScanMode::Full
        } else {
            scan.set_mode(ForumTopicScanMode::Incremental);
            ForumTopicScanMode::Incremental
        };
        let expected_pages = selected_pages.len();
        let mut remaining = targets
            .into_iter()
            .enumerate()
            .filter(|(index, _)| *index != 0)
            .filter(|(index, _)| selected_pages.binary_search(&(*index as u32)).is_ok())
            .filter(|(_, (offset, _))| Some(*offset) != probed_offset)
            .map(|(_, target)| target)
            .collect::<VecDeque<_>>();
        let mut reads = tokio::task::JoinSet::new();
        spawn_topic_page_reads(&mut reads, &mut remaining, &read_context);
        while let Some(read) = reads.join_next().await {
            let page = read.map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "forum_topic",
                    "read_page_join",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EhentaiError::remote_temporary(operation, false)
            })??;
            total_bytes = total_bytes
                .checked_add(page.1)
                .filter(|total| *total <= HARD_MAXIMUM_TOPIC_BYTES)
                .ok_or_else(|| EhentaiError::response_too_large(operation))?;
            pages.push((page.0, page.2));
            scan.set_pages(pages.len());
            spawn_topic_page_reads(&mut reads, &mut remaining, &read_context);
        }
        pages.sort_unstable_by_key(|(offset, _)| *offset);
        if pages.len() != expected_pages || pages.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(EhentaiError::protocol(operation, false));
        }
        // 绝对楼层由分页偏移与本页序号换算；目录首帖（楼层 1）不进入 preview.posts。
        let page_post_counts =
            pages.iter().map(|(offset, page)| (*offset, page.post_count())).collect::<Vec<_>>();
        let post_numbers = topic_post_numbers(&page_post_counts, operation)?;
        let parsed_pages = pages.into_iter().map(|(_, page)| page).collect();
        let preview = parser.parse_html_pages(parsed_pages).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "merge_pages",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let result = ForumTopicScan {
            preview,
            post_numbers,
            mode,
            page_count: Some(total_pages),
            page_limit: Some(u32::try_from(self.maximum_topic_pages).unwrap_or(u32::MAX)),
        };
        result.validate().ok().ok_or_else(|| EhentaiError::protocol(operation, false))?;
        Ok(result)
    }
}

pub(super) async fn read_topic_page(
    context: TopicPageReadContext,
    offset: u64,
    page_target: Url,
) -> Result<(u64, usize, crate::forum::ParsedHtmlPage), EhentaiError> {
    let identity_target = page_target.clone();
    let (body, _) = context
        .client
        .get_page_with_budget(
            context.operation,
            forum_operation(context.operation),
            ForumRequestScope::new(context.metrics_world, &context.cookie),
            page_target,
            Some(&context.referer),
            Some(&context.remaining_bytes),
        )
        .await?;
    let html = String::from_utf8(body).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "forum_topic",
            "decode_extra_page",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        EhentaiError::protocol(context.operation, false)
    })?;
    let page_bytes = html.len();
    let page =
        crate::ForumPreviewAdapter::new().parse_html_page(&html).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "parse_extra_page",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(context.operation, false)
        })?;
    context.client.validate_topic_page_identity(&page, &identity_target, offset, &context)?;
    Ok((offset, page_bytes, page))
}

/// 把分页覆盖换算成绝对楼层（1 基）。目录首帖占楼层 1，但不进入 `preview.posts`，
/// 因此结果从楼层 2 开始，与 fixture 中的行保持一一对应。
fn topic_post_numbers(
    pages: &[(u64, usize)],
    operation: EhentaiOperation,
) -> Result<Vec<u32>, EhentaiError> {
    let mut post_numbers = Vec::new();
    for (offset, count) in pages {
        for index in 0..*count {
            let floor = offset
                .checked_add(
                    u64::try_from(index)
                        .ok()
                        .ok_or_else(|| EhentaiError::protocol(operation, false))?,
                )
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| EhentaiError::protocol(operation, false))?;
            if floor == 1 {
                continue;
            }
            post_numbers.push(
                u32::try_from(floor)
                    .ok()
                    .ok_or_else(|| EhentaiError::protocol(operation, false))?,
            );
        }
    }
    Ok(post_numbers)
}

fn spawn_topic_page_reads(
    reads: &mut tokio::task::JoinSet<
        Result<(u64, usize, crate::forum::ParsedHtmlPage), EhentaiError>,
    >,
    remaining: &mut VecDeque<(u64, Url)>,
    context: &TopicPageReadContext,
) {
    while reads.len() < MAXIMUM_CONCURRENT_TOPIC_READS {
        let Some((offset, page_target)) = remaining.pop_front() else {
            break;
        };
        reads.spawn(read_topic_page(context.clone(), offset, page_target));
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::clients::ehentai::client::scan as parent;

    #[test]
    fn full_scan_floors_follow_page_offsets() {
        // 三页各 20 楼：楼层 1 是目录首帖，不进入 preview.posts。
        let numbers =
            topic_post_numbers(&[(0, 20), (20, 20), (40, 20)], EhentaiOperation::FetchTopic)
                .expect("楼层换算在预算内");
        assert_eq!(numbers.len(), 59);
        assert_eq!(numbers.first().copied(), Some(2));
        assert_eq!(numbers.last().copied(), Some(60));
    }

    #[test]
    fn incremental_scan_keeps_absolute_floors_after_skipped_pages() {
        // 固定首帖页 + 游标页（第 6 页，0 基 5）：中间分页被跳过，楼层仍为绝对楼层。
        let numbers =
            topic_post_numbers(&[(0, 20), (100, 20), (120, 7)], EhentaiOperation::FetchTopic)
                .expect("楼层换算在预算内");
        assert_eq!(numbers.len(), 19 + 20 + 7);
        assert_eq!(numbers[18], 20);
        assert_eq!(numbers[19], 101);
        assert_eq!(numbers.last().copied(), Some(127));
    }
}
