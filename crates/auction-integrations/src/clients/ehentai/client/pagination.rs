//! E-Hentai 论坛主题分页发现与受限目标构造。

use std::collections::BTreeSet;

use crate::forum::ParsedHtmlPage;

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
    ) -> Result<Vec<(u64, Url)>, EhentaiError> {
        let first_target = self.index_url(&[("showtopic", thread_id)], operation)?;
        let identity = self.topic_page_identity(first_page, &first_target, thread_id, operation)?;
        let maximum_offset = identity.offsets.last().copied().unwrap_or(0);
        validate_page_post_count(0, maximum_offset, identity.post_count, operation)?;
        self.topic_page_targets_through(thread_id, maximum_offset, operation)
    }

    fn topic_page_targets_through(
        &self,
        thread_id: &str,
        maximum_offset: u64,
        operation: EhentaiOperation,
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
            return Err(EhentaiError::protocol(operation, false));
        }
        let first_target = self.index_url(&[("showtopic", thread_id)], operation)?;

        (0..page_count)
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
            .collect()
    }

    pub(super) fn validate_topic_page_identity(
        &self,
        page: &ParsedHtmlPage,
        page_target: &Url,
        thread_id: &str,
        expected_offset: u64,
        known_maximum_offset: u64,
        operation: EhentaiOperation,
    ) -> Result<(), EhentaiError> {
        let identity = self.topic_page_identity(page, page_target, thread_id, operation)?;
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
    ) -> Result<TopicPageIdentity, EhentaiError> {
        let mut post_ids = BTreeSet::new();
        for post_id in page.post_ids() {
            if !post_ids.insert(post_id.to_owned()) {
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
            let topic_values = query_values(&resolved, &["showtopic", "t"]);
            if topic_values.is_empty() {
                continue;
            }
            if topic_values.len() != 1 {
                return Err(EhentaiError::protocol(operation, false));
            }
            if topic_values[0] != thread_id {
                continue;
            }
            thread_identity_seen = true;
            let post_values = query_values(&resolved, &["p", "pid", "gopid"]);
            if post_values.len() > 1 {
                return Err(EhentaiError::protocol(operation, false));
            }
            if post_values.first().is_some_and(|post_id| post_ids.contains(post_id)) {
                page_post_identity_seen = true;
            }
            let offset_values = query_values(&resolved, &["st"]);
            if offset_values.is_empty() {
                continue;
            }
            if offset_values.len() != 1 {
                return Err(EhentaiError::protocol(operation, false));
            }
            // 论坛 Full Edit 等同主题操作链接会携带合法的空 `st=`；它没有声明分页目标，
            // 只能忽略，不能把空值降级成第 0 页，也不能因此拒绝整个主题。
            if offset_values[0].is_empty() {
                continue;
            }
            let offset = canonical_page_offset(&offset_values[0])
                .ok_or_else(|| EhentaiError::protocol(operation, false))?;
            if !offset.is_multiple_of(FORUM_POSTS_PER_PAGE) {
                return Err(EhentaiError::protocol(operation, false));
            }
            offsets.insert(offset);
        }
        if !thread_identity_seen || !page_post_identity_seen {
            return Err(EhentaiError::protocol(operation, false));
        }
        let post_count =
            u64::try_from(post_ids.len()).map_err(|_| EhentaiError::protocol(operation, false))?;
        Ok(TopicPageIdentity { offsets, post_count })
    }
}

fn query_values(url: &Url, names: &[&str]) -> Vec<String> {
    url.query_pairs()
        .filter(|(name, _)| names.iter().any(|wanted| name.as_ref() == *wanted))
        .map(|(_, value)| value.into_owned())
        .collect()
}

fn canonical_page_offset(value: &str) -> Option<u64> {
    if value == "0" {
        return Some(0);
    }
    (!value.is_empty()
        && value.len() <= 20
        && value.as_bytes()[0].is_ascii_digit()
        && value.as_bytes()[0] != b'0'
        && value.bytes().all(|byte| byte.is_ascii_digit()))
    .then(|| value.parse().ok())
    .flatten()
}

fn validate_target_offset(
    page_target: &Url,
    expected_offset: u64,
    operation: EhentaiOperation,
) -> Result<(), EhentaiError> {
    let values = query_values(page_target, &["st"]);
    let valid = if expected_offset == 0 {
        values.is_empty() || values.len() == 1 && values[0] == "0"
    } else {
        values.len() == 1 && canonical_page_offset(&values[0]) == Some(expected_offset)
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
            .expect("真实论坛主题页应能形成受限页面事实");
        let client = EhentaiClient::with_config(
            EhentaiClientConfig::production().expect("固定论坛配置应合法"),
        )
        .expect("安全论坛客户端应可构造");

        let targets = client
            .topic_page_targets(&page, "300433", EhentaiOperation::FetchTopic)
            .expect("编辑链接的空 st 不代表分页偏移");

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0, 0);
    }
}
