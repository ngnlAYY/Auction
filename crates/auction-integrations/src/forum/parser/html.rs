//! 论坛 HTML 的单次 DOM 解析、帖子抽取、作者识别和正文规整。

use std::sync::LazyLock;

use scraper::{ElementRef, Html, Node, Selector};
use url::Url;

use super::model::{ParsedHtmlPage, ParsedPost};

static POST_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("div.postcolor").map_err(|_| ()));
static EDIT_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse(".edit").map_err(|_| ()));
static AUTHOR_SELECTOR: LazyLock<Result<Selector, ()>> = LazyLock::new(|| {
    Selector::parse(".bigusername, .normalname, .author, .username, .post-author, .member_name")
        .map_err(|_| ())
});
static LINK_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("a[href]").map_err(|_| ()));

fn cached_selector(
    cache: &'static LazyLock<Result<Selector, ()>>,
) -> Result<&'static Selector, ()> {
    match LazyLock::force(cache) {
        Ok(selector) => Ok(selector),
        Err(()) => Err(()),
    }
}

pub fn parse_html(input: &str) -> Result<super::AdminForumPreviewResult, ()> {
    super::parse_thread(extract_html_page(input)?.into_posts())
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

    // 分页校验只消费用户正文区以外的主题链接；提前形成轻量事实后即可释放完整 DOM。
    let link_selector = cached_selector(&LINK_SELECTOR)?;
    let navigation_references = document
        .select(link_selector)
        .filter(|link| !link_is_user_content(*link))
        .filter_map(|link| link.attr("href"))
        .filter(|reference| topic_reference_candidate(reference))
        .map(str::to_owned)
        .collect();
    Ok(ParsedHtmlPage::new(posts, navigation_references))
}

pub(super) fn canonical_post_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.as_bytes()[0].is_ascii_digit()
        && value.as_bytes()[0] != b'0'
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn post_was_edited(element: ElementRef<'_>) -> bool {
    // 只信任论坛生成的 edit 元素；普通用户正文包含相同英文句子不能成为绕过命令解析的开关。
    cached_selector(&EDIT_SELECTOR)
        .ok()
        .is_some_and(|selector| element.select(selector).next().is_some())
}

fn render_post_text(element: ElementRef<'_>) -> String {
    let mut source = String::new();
    let mut previous_block = None;
    for node in element.descendants() {
        match node.value() {
            Node::Element(value) if value.name() == "br" => source.push('\n'),
            Node::Text(value) => {
                let block = node.ancestors().find_map(|ancestor| match ancestor.value() {
                    Node::Element(value)
                        if matches!(value.name(), "p" | "li" | "blockquote" | "div") =>
                    {
                        Some(ancestor.id())
                    }
                    _ => None,
                });
                if previous_block.is_some() && block != previous_block {
                    source.push('\n');
                }
                source.push_str(value);
                previous_block = block;
            }
            _ => {}
        }
    }
    super::text::normalize_text(&source)
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
    for _ in 0..16 {
        let Some(element) = current else {
            break;
        };
        if element.attr("class").is_some_and(|classes| {
            classes.split_ascii_whitespace().any(|class| matches!(class, "postcolor" | "signature"))
        }) {
            return true;
        }
        current = element.parent().and_then(ElementRef::wrap);
    }
    false
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
    (value.len() <= 32
        && value.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && value.as_bytes().first() != Some(&b'0')
        && value.bytes().all(|byte| byte.is_ascii_digit()))
    .then_some(value)
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

        let page = extract_html_page(html).expect("真实论坛结构应能形成受限页面事实");

        assert_eq!(page.post_ids().collect::<Vec<_>>(), ["6956799", "6956801"]);
        assert_eq!(page.navigation_references().count(), 2);
        let posts = page.into_posts();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].author_id.as_deref(), Some("5501029"));
        assert_eq!(posts[0].text, "One-Handed\n[One01] Test rapier (seller: seller) — Start: 50k");
        assert!(posts[1].edited);
    }
}
