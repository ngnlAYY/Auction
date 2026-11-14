//! 论坛 HTML 的单次 DOM 解析、帖子抽取、作者识别和正文规整。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::sync::LazyLock;

use scraper::{ElementRef, Html, Node, Selector};
use url::Url;

use super::model::{ParsedHtmlPage, ParsedPost};

static POST_SELECTOR: LazyLock<Result<Selector, ()>> = LazyLock::new(|| {
    Selector::parse("div.postcolor").map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "forum_topic",
            "build_preview_post_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
    })
});
static EDIT_SELECTOR: LazyLock<Result<Selector, ()>> = LazyLock::new(|| {
    Selector::parse(".edit").map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "forum_topic",
            "build_preview_edit_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
    })
});
static AUTHOR_SELECTOR: LazyLock<Result<Selector, ()>> = LazyLock::new(|| {
    Selector::parse(".bigusername, .normalname, .author, .username, .post-author, .member_name")
        .map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_topic",
                "build_preview_author_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
        })
});
static LINK_SELECTOR: LazyLock<Result<Selector, ()>> = LazyLock::new(|| {
    Selector::parse("a[href]").map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "forum_topic",
            "build_preview_link_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
    })
});

fn cached_selector(
    cache: &'static LazyLock<Result<Selector, ()>>,
) -> Result<&'static Selector, ()> {
    LazyLock::force(cache).as_ref().map_err(|_| ())
}

pub(super) fn extract_html_page(input: &str) -> Result<ParsedHtmlPage, ()> {
    let document = Html::parse_document(input);
    let post_selector = cached_selector(&POST_SELECTOR)?;
    let mut posts = Vec::new();
    for element in document.select(post_selector) {
        let id = element
            .attr("id")
            .and_then(|value| value.strip_prefix("post-"))
            .filter(|value| canonical_post_id(value))
            .ok_or(())?;
        let text = render_post_text(element);
        let (author_id, author) = extract_author(element);
        posts.push(ParsedPost {
            id: id.to_owned(),
            author_id,
            author,
            edited: post_was_edited(element),
            text,
        });
    }
    if posts.is_empty() {
        return Err(());
    }

    // 分页校验只消费用户正文区以外的主题链接；沿 DOM 深度维护状态，避免每个链接重扫祖先链。
    let mut navigation_references = Vec::new();
    let mut ancestors = Vec::new();
    for element in document.root_element().descendants().filter_map(ElementRef::wrap) {
        let parent = element.parent().map(|parent| parent.id());
        while ancestors.last().is_some_and(|(id, _)| Some(*id) != parent) {
            ancestors.pop();
        }
        let user_content = ancestors.last().is_some_and(|(_, user_content)| *user_content)
            || is_user_content(element);
        if !user_content
            && element.value().name() == "a"
            && let Some(reference) =
                element.attr("href").filter(|value| topic_reference_candidate(value))
        {
            navigation_references.push(reference.to_owned());
        }
        ancestors.push((element.id(), user_content));
    }
    Ok(ParsedHtmlPage::new(posts, navigation_references))
}

pub(super) fn canonical_post_id(value: &str) -> bool {
    auction_domain::valid_positive_decimal(value, 32)
}

fn post_was_edited(element: ElementRef<'_>) -> bool {
    // 只信任论坛生成的 edit 元素；普通用户正文包含相同英文句子不能成为绕过命令解析的开关。
    cached_selector(&EDIT_SELECTOR)
        .ok()
        .is_some_and(|selector| element.select(selector).next().is_some())
}

fn render_post_text(element: ElementRef<'_>) -> String {
    // IPB 的引用标题/正文和 blockquote 属于被引用作者，不能归到当前回帖者的竞价命令。
    if element.ancestors().filter_map(ElementRef::wrap).any(|node| is_quote(node.value())) {
        return String::new();
    }
    let inherited_block = element.ancestors().find_map(|node| match node.value() {
        Node::Element(value) if is_text_block(value) => Some(node.id()),
        _ => None,
    });
    let mut source = String::new();
    let mut previous_block = None;
    // 深度优先遍历中每个元素只入栈、出栈一次；深层 BBCode 无需为每个节点重扫祖先链。
    let mut ancestors = Vec::new();
    for node in element.descendants() {
        let parent = node.parent().map(|parent| parent.id());
        while ancestors.last().is_some_and(|(id, _, _)| Some(*id) != parent) {
            ancestors.pop();
        }
        let (block, quoted) = ancestors
            .last()
            .map_or((inherited_block, false), |(_, block, quoted)| (*block, *quoted));
        if quoted {
            source.push('\n');
        } else {
            match node.value() {
                Node::Element(value) if value.name() == "br" => source.push('\n'),
                Node::Text(value) => {
                    if previous_block.is_some() && block != previous_block {
                        source.push('\n');
                    }
                    source.push_str(value);
                    previous_block = block;
                }
                _ => {}
            }
        }
        if let Node::Element(value) = node.value() {
            ancestors.push((
                node.id(),
                if is_text_block(value) { Some(node.id()) } else { block },
                quoted || is_quote(value),
            ));
        }
    }
    super::text::normalize_text(&source)
}

fn is_quote(value: &scraper::node::Element) -> bool {
    value.name() == "blockquote"
        || value
            .classes()
            .any(|class| matches!(class, "quotetop" | "quotemain" | "quote" | "blockquote"))
}

fn is_text_block(value: &scraper::node::Element) -> bool {
    matches!(value.name(), "p" | "li" | "blockquote" | "div")
}

fn extract_author(element: ElementRef<'_>) -> (Option<String>, Option<String>) {
    let Some(row) =
        element.ancestors().filter_map(ElementRef::wrap).find(|node| node.value().name() == "tr")
    else {
        return (None, None);
    };
    let Ok(authors) = cached_selector(&AUTHOR_SELECTOR) else {
        return (None, None);
    };
    let Ok(links) = cached_selector(&LINK_SELECTOR) else {
        return (None, None);
    };
    let mut identity = None;
    for author in row.select(authors) {
        for link in std::iter::once(author)
            .chain(author.select(links))
            .filter(|candidate| candidate.value().name() == "a")
            .filter(|candidate| !link_is_user_content(*candidate))
        {
            let Some(account_id) = link.attr("href").and_then(forum_account_id_from_href) else {
                continue;
            };
            let player_name = render_post_text(link);
            if player_name.is_empty() {
                continue;
            }
            let candidate = (account_id, player_name);
            if identity.as_ref().is_some_and(|current| current != &candidate) {
                // 同一帖子行出现互相矛盾的论坛生成作者链接时失败关闭，不能拼接数字键与名称。
                return (None, None);
            }
            identity = Some(candidate);
        }
    }
    identity.map_or((None, None), |(account_id, player_name)| (Some(account_id), Some(player_name)))
}

fn link_is_user_content(link: ElementRef<'_>) -> bool {
    let mut current = Some(link);
    // 用户可以任意嵌套 BBCode；固定祖先层数会把深层正文链接误当成系统作者或分页。
    while let Some(element) = current {
        if is_user_content(element) {
            return true;
        }
        current = element.parent().and_then(ElementRef::wrap);
    }
    false
}

fn is_user_content(element: ElementRef<'_>) -> bool {
    element.attr("class").is_some_and(|classes| {
        classes.split_ascii_whitespace().any(|class| matches!(class, "postcolor" | "signature"))
    })
}

fn topic_reference_candidate(reference: &str) -> bool {
    reference.len() <= 4 * 1024
        && !reference.chars().any(char::is_control)
        && (reference.contains("showtopic=")
            || reference.contains("?t=")
            || reference.contains("&t="))
}

fn forum_account_id_from_href(href: &str) -> Option<String> {
    let base = Url::parse("https://forums.e-hentai.org/").ok()?;
    let url = base.join(href).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("forums.e-hentai.org")
        || url.path() != "/index.php"
    {
        return None;
    }

    let mut account_id = None;
    for (name, value) in url.query_pairs() {
        if name != "showuser" {
            continue;
        }
        let value = canonical_forum_account_id(value.into_owned())?;
        if account_id.as_ref().is_some_and(|current| current != &value) {
            return None;
        }
        account_id = Some(value);
    }
    account_id
}

fn canonical_forum_account_id(value: String) -> Option<String> {
    auction_domain::valid_positive_decimal(&value, 32).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_live_forum_posts_and_navigation_in_one_pass() {
        let html = r#"
            <html><body>
              <a href="index.php?showtopic=300433&amp;p=6956799#entry6956799">#1</a>
              <table><tr><td>
                <span class="bigusername"><a href="index.php?showuser=5501029">ngnlAYY</a></span>
                <div class="postcolor" id="post-6956799">
                  One-Handed<br>[One01] Test rapier (seller: seller) — Start: 50k
                </div>
              </td></tr></table>
              <a href="index.php?showtopic=300433&amp;p=6956801#entry6956801">#2</a>
              <table><tr><td>
                <span class="bigusername"><a href="index.php?showuser=5501029">ngnlAYY</a></span>
                <div class="postcolor" id="post-6956801">
                  One01 200k<br><span class="edit">This post has been edited by ngnlAYY</span>
                </div>
              </td></tr></table>
            </body></html>
        "#;

        let page = extract_html_page(html)
            .unwrap_or_else(|error| panic!("真实论坛结构应能形成受限页面事实: {error:?}"));

        assert_eq!(page.post_ids().collect::<Vec<_>>(), ["6956799", "6956801"]);
        assert_eq!(page.navigation_references().count(), 2);
        let posts = page.into_posts();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].author_id.as_deref(), Some("5501029"));
        assert_eq!(posts[0].text, "One-Handed\n[One01] Test rapier (seller: seller) — Start: 50k");
        assert!(posts[1].edited);
    }
}
