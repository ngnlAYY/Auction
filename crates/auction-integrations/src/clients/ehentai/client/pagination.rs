//! E-Hentai 论坛主题分页发现与受限目标构造。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::clients::metrics::topic_scan_discovered;
use crate::safe_diagnostics;

use auction_application::external_metrics::{ForumTopicScanOutcome, ForumWorld};

use std::collections::BTreeSet;

use crate::forum::ParsedHtmlPage;

use super::scan::read_topic_page;
use super::*;

struct TopicPageIdentity {
    offsets: BTreeSet<u64>,
    post_count: u64,
}

impl EhentaiClient {
    pub(super) fn topic_page_targets(
        &self,
        first_page: &ParsedHtmlPage,
        thread_id: &str,
        operation: EhentaiOperation,
        metrics_world: ForumWorld,
    ) -> Result<Vec<(u64, Url)>, EhentaiError> {
        let first_target = self.index_url(&[("showtopic", thread_id)], operation)?;
        let identity = self.topic_page_identity(
            first_page,
            &first_target,
            thread_id,
            operation,
            metrics_world,
        )?;
        let maximum_offset = identity.offsets.last().copied().unwrap_or(0);
        validate_page_post_count(0, maximum_offset, identity.post_count, operation)?;
        self.topic_page_targets_through(thread_id, maximum_offset, operation, metrics_world)
    }

    /// 计算本轮的增量读取预算，并核对游标页仍包含游标帖子。
    ///
    /// 返回需要读取的页号（0 基）与已经单独读过的游标页偏移；游标缺失、无效、
    /// 落在固定楼层之内，或游标页不再包含该帖子（删帖使楼层前移）时返回全部页，
    /// 也就是回退完整扫描。
    pub(super) async fn incremental_scan_plan(
        &self,
        targets: &[(u64, Url)],
        cursor: Option<&ForumTopicCursor>,
        fixed_through_post_number: u32,
        read_context: &TopicPageReadContext,
        state: IncrementalScanState<'_>,
        operation: EhentaiOperation,
    ) -> Result<(Vec<u32>, Option<u64>), EhentaiError> {
        let IncrementalScanState { pages, total_bytes } = state;
        let page_count = u32::try_from(targets.len())
            .map_err(|_: std::num::TryFromIntError| EhentaiError::protocol(operation, false))?;
        let all_pages = (0..page_count).collect::<Vec<_>>();
        let Some(cursor) = cursor.filter(|cursor| cursor.validate().is_ok()) else {
            return Ok((all_pages, None));
        };
        let fixed_page_count =
            auction_application::forum::forum_topic_fixed_page_count(fixed_through_post_number);
        let selected_pages = auction_application::forum::forum_topic_scan_pages(
            Some(cursor.page()),
            fixed_page_count,
            page_count,
        );
        if selected_pages.len() == targets.len() {
            return Ok((all_pages, None));
        }
        let Some((offset, page_target)) = targets.get(cursor.page() as usize) else {
            return Ok((all_pages, None));
        };
        let (offset, page_bytes, page) =
            read_topic_page(read_context.clone(), *offset, page_target.clone()).await?;
        *total_bytes = total_bytes
            .checked_add(page_bytes)
            .filter(|total| *total <= HARD_MAXIMUM_TOPIC_BYTES)
            .ok_or_else(|| EhentaiError::response_too_large(operation))?;
        let cursor_page_contains_post = page.post_ids().any(|post_id| post_id == cursor.post_id);
        let plan = confirm_incremental_pages(selected_pages, page_count, cursor_page_contains_post);
        if !cursor_page_contains_post {
            // 删帖或楼层前移使游标页不再包含该帖子：整场重读，绝不跳过无法证明的楼层。
            return Ok((plan, None));
        }
        pages.push((offset, page));
        Ok((plan, Some(offset)))
    }

    fn topic_page_targets_through(
        &self,
        thread_id: &str,
        maximum_offset: u64,
        operation: EhentaiOperation,
        metrics_world: ForumWorld,
    ) -> Result<Vec<(u64, Url)>, EhentaiError> {
        if !maximum_offset.is_multiple_of(FORUM_POSTS_PER_PAGE) {
            return Err(EhentaiError::protocol(operation, false));
        }
        let page_count = maximum_offset
            .checked_div(FORUM_POSTS_PER_PAGE)
            .and_then(|value| value.checked_add(1))
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| EhentaiError::protocol(operation, false))?;
        if page_count > self.maximum_topic_pages {
            topic_scan_discovered(
                self.external_metrics().as_ref(),
                metrics_world,
                ForumTopicScanOutcome::PageLimit,
                page_count,
            );
            return Err(EhentaiError::topic_page_limit(operation)
                .with_page_count(u32::try_from(page_count).unwrap_or(u32::MAX))
                .with_page_limit(u32::try_from(self.maximum_topic_pages).unwrap_or(u32::MAX)));
        }
        let first_target = self.index_url(&[("showtopic", thread_id)], operation)?;

        let targets = (0..page_count)
            .map(|page| {
                let offset = u64::try_from(page)
                    .ok()
                    .and_then(|value| value.checked_mul(FORUM_POSTS_PER_PAGE))
                    .ok_or_else(|| EhentaiError::protocol(operation, false))?;
                let target = if offset == 0 {
                    first_target.clone()
                } else {
                    let offset_value = offset.to_string();
                    self.index_url(
                        &[("showtopic", thread_id), ("st", offset_value.as_str())],
                        operation,
                    )?
                };
                Ok((offset, target))
            })
            .collect::<Result<Vec<_>, EhentaiError>>()?;
        topic_scan_discovered(
            self.external_metrics().as_ref(),
            metrics_world,
            ForumTopicScanOutcome::Complete,
            targets.len(),
        );
        Ok(targets)
    }

    pub(super) fn validate_topic_page_identity(
        &self,
        page: &ParsedHtmlPage,
        page_target: &Url,
        expected_offset: u64,
        context: &TopicPageReadContext,
    ) -> Result<(), EhentaiError> {
        let operation = context.operation;
        let known_maximum_offset = context.known_maximum_offset;
        let identity = self.topic_page_identity(
            page,
            page_target,
            &context.thread_id,
            operation,
            context.metrics_world,
        )?;
        let discovered_maximum_offset = identity.offsets.last().copied().unwrap_or(0);
        if discovered_maximum_offset > known_maximum_offset {
            return Err(EhentaiError::protocol(operation, false));
        }
        validate_target_offset(page_target, expected_offset, operation)?;
        validate_page_post_count(
            expected_offset,
            known_maximum_offset,
            identity.post_count,
            operation,
        )
    }

    fn topic_page_identity(
        &self,
        page: &ParsedHtmlPage,
        page_target: &Url,
        thread_id: &str,
        operation: EhentaiOperation,
        metrics_world: ForumWorld,
    ) -> Result<TopicPageIdentity, EhentaiError> {
        let mut post_ids = BTreeSet::new();
        for post_id in page.post_ids() {
            if !post_ids.insert(post_id) {
                return Err(EhentaiError::protocol(operation, false));
            }
        }
        if post_ids.is_empty() {
            return Err(EhentaiError::protocol(operation, false));
        }

        let mut offsets = BTreeSet::from([0_u64]);
        let mut thread_identity_seen = false;
        let mut page_post_identity_seen = false;

        for reference in page.navigation_references() {
            let Ok(mut resolved) = page_target.join(reference) else {
                continue;
            };
            if !same_origin(&resolved, &self.base_url)
                || !resolved.username().is_empty()
                || resolved.password().is_some()
                || resolved.path() != "/index.php"
            {
                continue;
            }
            resolved.set_fragment(None);
            let Some(topic_id) = optional_query_identity(&resolved, &["showtopic", "t"]).map_err(
                |diagnostic_cause| {
                    safe_diagnostics::warn(
                        "forum_topic",
                        "parse_topic_identity",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    EhentaiError::protocol(operation, false)
                },
            )?
            else {
                continue;
            };
            if topic_id != thread_id {
                continue;
            }
            // 真实 IPB 用引用链接证明帖子身份；只接受当前世界板块的受限引用动作。
            let query_identity = |aliases: &[&str]| {
                optional_query_identity(&resolved, aliases)
                    .map_err(|()| EhentaiError::protocol(operation, false))
            };
            if query_identity(&["qpid"])?.is_some() {
                let board = match metrics_world {
                    ForumWorld::Main => self.board_for_world(SettlementWorld::Main),
                    ForumWorld::Isekai => self.board_for_world(SettlementWorld::Isekai),
                    ForumWorld::Shared => return Err(EhentaiError::protocol(operation, false)),
                };
                let expected_board = board.to_string();
                if query_identity(&["f"])?.as_deref() != Some(expected_board.as_str())
                    || query_identity(&["act"])?.as_deref() != Some("Post")
                    || query_identity(&["CODE"])?.as_deref() != Some("02")
                {
                    return Err(EhentaiError::protocol(operation, false));
                }
            }
            thread_identity_seen = true;
            let post_id = optional_query_identity(&resolved, &["p", "pid", "gopid", "qpid"])
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "forum_topic",
                        "parse_post_identity",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    EhentaiError::protocol(operation, false)
                })?;
            if post_id.as_deref().is_some_and(|post_id| post_ids.contains(post_id)) {
                page_post_identity_seen = true;
            }
            let Some(offset_value) =
                optional_query_identity(&resolved, &["st"]).map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "forum_topic",
                        "parse_page_offset",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    EhentaiError::protocol(operation, false)
                })?
            else {
                continue;
            };
            // 论坛 Full Edit 等同主题操作链接会携带合法的空 `st=`；它没有声明分页目标，
            // 只能忽略，不能把空值降级成第 0 页，也不能因此拒绝整个主题。
            if offset_value.is_empty() {
                continue;
            }
            let offset = canonical_page_offset(&offset_value)
                .ok_or_else(|| EhentaiError::protocol(operation, false))?;
            if !offset.is_multiple_of(FORUM_POSTS_PER_PAGE) {
                return Err(EhentaiError::protocol(operation, false));
            }
            offsets.insert(offset);
        }
        if !thread_identity_seen || !page_post_identity_seen {
            return Err(EhentaiError::protocol(operation, false));
        }
        let post_count = u64::try_from(post_ids.len()).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "convert_post_count",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        Ok(TopicPageIdentity { offsets, post_count })
    }
}

/// 增量扫描的写入目标：已读页面与累计字节。游标页探测结果先写入这些状态，再继续并发读取。
pub(super) struct IncrementalScanState<'a> {
    pub pages: &'a mut Vec<(u64, ParsedHtmlPage)>,
    pub total_bytes: &'a mut usize,
}

/// 游标页核对结果决定分页预算：只有确认该帖子仍在游标页时才跳过更早的分页。
///
/// 删帖、楼层前移或帖子被移动到别的页都会让核对失败，此时退回完整读取。
fn confirm_incremental_pages(
    selected_pages: Vec<u32>,
    page_count: u32,
    cursor_page_contains_post: bool,
) -> Vec<u32> {
    if cursor_page_contains_post { selected_pages } else { (0..page_count).collect() }
}

fn canonical_page_offset(value: &str) -> Option<u64> {
    if value == "0" {
        return Some(0);
    }
    auction_domain::valid_positive_decimal(value, 20).then(|| value.parse().ok()).flatten()
}

fn validate_target_offset(
    page_target: &Url,
    expected_offset: u64,
    operation: EhentaiOperation,
) -> Result<(), EhentaiError> {
    let value = optional_query_identity(page_target, &["st"]).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "forum_topic",
            "validate_page_offset",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        EhentaiError::protocol(operation, false)
    })?;
    let valid = if expected_offset == 0 {
        value.as_deref().is_none_or(|value| value == "0")
    } else {
        value.as_deref().and_then(canonical_page_offset) == Some(expected_offset)
    };
    if !valid {
        return Err(EhentaiError::protocol(operation, false));
    }
    Ok(())
}

fn validate_page_post_count(
    offset: u64,
    maximum_offset: u64,
    post_count: u64,
    operation: EhentaiOperation,
) -> Result<(), EhentaiError> {
    let valid = offset <= maximum_offset
        && post_count > 0
        && post_count <= FORUM_POSTS_PER_PAGE
        && (offset == maximum_offset || post_count == FORUM_POSTS_PER_PAGE);
    if !valid {
        return Err(EhentaiError::protocol(operation, false));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_edit_offset_is_not_treated_as_topic_pagination() {
        let html = r#"
            <html><body>
              <a href="index.php?showtopic=300433&amp;p=6956799#entry6956799">#1</a>
              <a href="index.php?act=post&amp;do=edit_post&amp;f=4&amp;t=300433&amp;p=6956799&amp;st=">Full Edit</a>
              <div class="postcolor" id="post-6956799">Catalog</div>
              <a href="index.php?showtopic=300433&amp;p=6956800#entry6956800">#2</a>
              <div class="postcolor" id="post-6956800">System log</div>
            </body></html>
        "#;
        let page = crate::ForumPreviewAdapter::new()
            .parse_html_page(html)
            .unwrap_or_else(|error| panic!("真实论坛主题页应能形成受限页面事实: {error:?}"));
        let config = EhentaiClientConfig::production()
            .unwrap_or_else(|error| panic!("固定论坛配置应合法: {error:?}"));
        let client = EhentaiClient::with_config(config)
            .unwrap_or_else(|error| panic!("安全论坛客户端应可构造: {error:?}"));

        let targets = client
            .topic_page_targets(&page, "300433", EhentaiOperation::FetchTopic, ForumWorld::Main)
            .unwrap_or_else(|error| panic!("编辑链接的空 st 不代表分页偏移: {error:?}"));

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0, 0);
    }

    #[test]
    fn captured_quote_identity_stays_bound_to_origin_thread_board_and_post() {
        let mut config = EhentaiClientConfig::production().unwrap();
        config.test_board_mode = true;
        let client = EhentaiClient::with_config(config).unwrap();
        let quote = "index.php?act=Post&amp;CODE=02&amp;f=4&amp;t=302133&amp;qpid=6987908";
        let valid = format!(
            "<a href=\"{quote}\">Quote</a><div class=\"postcolor\" id=\"post-6987908\">Catalog</div>"
        );
        let targets = |html: &str| {
            let page = crate::ForumPreviewAdapter::new().parse_html_page(html).unwrap();
            client.topic_page_targets(
                &page,
                "302133",
                EhentaiOperation::FetchTopic,
                ForumWorld::Main,
            )
        };
        assert!(targets(&valid).is_ok());
        for (old, new) in [
            ("index.php?", "https://example.invalid/index.php?"),
            ("t=302133", "t=999"),
            ("f=4", "f=77"),
            ("qpid=6987908", "qpid=999"),
            ("qpid=6987908", "qpid=6987908&amp;qpid=6987908"),
            ("qpid=6987908", "qpid=6987908&amp;p=6987908"),
            ("CODE=02", "CODE=03"),
        ] {
            assert!(targets(&valid.replace(old, new)).is_err(), "{new}");
        }
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::confirm_incremental_pages;

    #[test]
    fn confirmed_cursor_page_keeps_incremental_window() {
        assert_eq!(confirm_incremental_pages(vec![0, 3, 4, 5], 6, true), vec![0, 3, 4, 5]);
    }

    #[test]
    fn deleted_or_shifted_cursor_post_falls_back_to_full_scan() {
        // 游标页不再包含该帖子：删帖、楼层前移或被移动到其他页，都必须重读全部页。
        assert_eq!(confirm_incremental_pages(vec![0, 3, 4, 5], 6, false), vec![0, 1, 2, 3, 4, 5]);
    }
}
