//! IPB 确认页与主题/帖子引用解析；普通页面链接不构成写入证明。

use self::parent::*;
use crate::clients::ehentai::client::topic as parent;

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::sync::LazyLock;

static ACKNOWLEDGEMENT_TITLE_SELECTOR: LazyLock<Result<Selector, ()>> = LazyLock::new(|| {
    Selector::parse("title").map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "forum_topic",
            "build_receipt_title_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
    })
});
static ACKNOWLEDGEMENT_REFRESH_SELECTOR: LazyLock<Result<Selector, ()>> = LazyLock::new(|| {
    Selector::parse("meta[http-equiv][content]").map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "forum_topic",
            "build_receipt_refresh_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
    })
});

impl EhentaiClient {
    pub(super) fn post_from_acknowledgement_page(
        &self,
        base: &Url,
        body: &[u8],
        board_id: i32,
        thread_id: &str,
        operation: EhentaiOperation,
    ) -> Option<ForumPublishedPost> {
        let html = std::str::from_utf8(body).ok()?;
        let document = Html::parse_document(html);
        let refresh_reference = acknowledgement_reference(&document)?;
        let refresh_proof =
            self.post_from_reference(base, refresh_reference, board_id, thread_id, operation)?;

        let links = link_selector().ok()?;
        let link_proofs = document
            .select(links)
            .filter_map(|link| link.value().attr("href"))
            .filter_map(|reference| {
                self.post_from_reference(base, reference, board_id, thread_id, operation)
            });
        let link_proof = consistent_post_proof(link_proofs)?;
        (refresh_proof == link_proof).then_some(refresh_proof)
    }

    pub(super) fn topic_from_redirect_page(
        &self,
        base: &Url,
        body: &[u8],
        board_id: i32,
        operation: EhentaiOperation,
    ) -> Option<auction_application::forum::ForumPublishedThread> {
        let html = std::str::from_utf8(body).ok()?;
        let document = Html::parse_document(html);
        // HTTP 200 的普通页面可能链接到旧主题；只有现有确认页协议才能授予 Created。
        let refresh_proof = self.topic_from_reference(
            base,
            acknowledgement_reference(&document)?,
            board_id,
            operation,
        )?;
        let links = link_selector().ok()?;
        let mut proofs =
            document.select(links).filter_map(|link| link.value().attr("href")).filter_map(
                |reference| self.topic_from_reference(base, reference, board_id, operation),
            );
        let first = proofs.next()?;
        (first == refresh_proof && proofs.all(|candidate| candidate == refresh_proof))
            .then_some(refresh_proof)
    }

    pub(super) fn topic_from_reference(
        &self,
        base: &Url,
        reference: &str,
        board_id: i32,
        operation: EhentaiOperation,
    ) -> Option<auction_application::forum::ForumPublishedThread> {
        let resolved = self.resolve_same_origin(base, reference, operation).ok()?;
        let topic_id = optional_query_identity(&resolved, &["showtopic", "t"]).ok()??;
        if resolved.path() != "/index.php"
            || !valid_topic_id(&topic_id)
            || !posting_action_matches_board(&resolved, board_id)
        {
            return None;
        }
        let canonical = self.index_url(&[("showtopic", topic_id.as_str())], operation).ok()?;
        Some(auction_application::forum::ForumPublishedThread {
            board_id,
            thread_id: topic_id,
            thread_url: canonical.to_string(),
        })
    }

    pub(super) fn post_from_reference(
        &self,
        base: &Url,
        reference: &str,
        board_id: i32,
        thread_id: &str,
        operation: EhentaiOperation,
    ) -> Option<ForumPublishedPost> {
        let resolved = self.resolve_same_origin(base, reference, operation).ok()?;
        let observed_thread = optional_query_identity(&resolved, &["showtopic", "t"]).ok()??;
        let query_post = optional_query_identity(&resolved, &["p", "pid", "gopid"]).ok()?;
        let fragment_post = resolved.fragment().and_then(|fragment| {
            fragment
                .strip_prefix("entry")
                .or_else(|| fragment.strip_prefix("post-"))
                .map(str::to_owned)
        });
        let observed_post = match (query_post, fragment_post) {
            (Some(query), Some(fragment)) if query != fragment => return None,
            (Some(query), _) => query,
            (None, Some(fragment)) => fragment,
            (None, None) => return None,
        };
        if resolved.path() != "/index.php"
            || !posting_action_matches_board(&resolved, board_id)
            || observed_thread != thread_id
            || !valid_topic_id(&observed_thread)
            || !valid_post_id(&observed_post)
        {
            return None;
        }
        Some(ForumPublishedPost { thread_id: observed_thread, post_id: observed_post })
    }
}

/// 创建与固定帖写入共用已有 IPB 确认页结构；错误页中的普通主题链接不构成写入证明。
fn acknowledgement_reference(document: &Html) -> Option<&str> {
    let title_selector = LazyLock::force(&ACKNOWLEDGEMENT_TITLE_SELECTOR).as_ref().ok()?;
    let mut titles = document.select(title_selector);
    let title = clean_line(&titles.next()?.text().collect::<String>());
    if title != "Please stand by..." || titles.next().is_some() {
        return None;
    }

    let refresh_selector = LazyLock::force(&ACKNOWLEDGEMENT_REFRESH_SELECTOR).as_ref().ok()?;
    let mut refreshes = document.select(refresh_selector).filter(|element| {
        element
            .value()
            .attr("http-equiv")
            .is_some_and(|value| value.eq_ignore_ascii_case("refresh"))
    });
    let refresh = refreshes.next()?;
    if refreshes.next().is_some() {
        return None;
    }
    refresh_reference(refresh.value().attr("content")?)
}

fn refresh_reference(content: &str) -> Option<&str> {
    if content.is_empty() || content.len() > 4 * 1024 || content.chars().any(char::is_control) {
        return None;
    }
    let (_, target) = content.split_once(';')?;
    let (name, value) = target.trim().split_once('=')?;
    if !name.trim().eq_ignore_ascii_case("url") {
        return None;
    }
    let value = value.trim();
    let value = if value.len() >= 2
        && matches!(value.as_bytes()[0], b'\'' | b'"')
        && value.as_bytes().last() == value.as_bytes().first()
    {
        &value[1..value.len() - 1]
    } else {
        value
    };
    (!value.is_empty()).then_some(value)
}

fn consistent_post_proof(
    mut proofs: impl Iterator<Item = ForumPublishedPost>,
) -> Option<ForumPublishedPost> {
    let proof = proofs.next()?;
    proofs.all(|candidate| candidate == proof).then_some(proof)
}
