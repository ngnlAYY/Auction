//! 论坛 HTML 帖子抽取、作者识别和正文规整。

use scraper::{ElementRef, Html, Selector};
use url::Url;

use super::model::ParsedPost;

pub fn parse_html(input: &str) -> Result<super::AdminForumPreviewResult, ()> {
    let document = Html::parse_document(input);
    let selector = Selector::parse("div.postcolor").map_err(|_| ())?;
    let mut posts = Vec::new();
    for element in document.select(&selector) {
        let Some(raw_id) = element.attr("id") else {
            continue;
        };
        let Some(id) = raw_id.strip_prefix("post-").filter(|value| !value.is_empty()) else {
            continue;
        };
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
    super::parse_thread(posts)
}

fn post_was_edited(element: ElementRef<'_>) -> bool {
    // 只信任论坛生成的 edit 元素；普通用户正文包含相同英文句子不能成为绕过命令解析的开关。
    Selector::parse(".edit").ok().is_some_and(|selector| element.select(&selector).next().is_some())
}

fn render_post_text(element: ElementRef<'_>) -> String {
    let mut source = element.inner_html();
    for tag in ["<br>", "<br/>", "<br />", "</p>", "</li>", "</blockquote>", "</div>"] {
        source = source.replace(tag, "\n");
    }
    let fragment = Html::parse_fragment(&source);
    super::text::normalize_text(&fragment.root_element().text().collect::<String>())
}

fn extract_author(element: ElementRef<'_>) -> (Option<String>, Option<String>) {
    let selector = Selector::parse(
        ".bigusername, .normalname, .author, .username, .post-author, .member_name",
    )
    .ok();
    let mut current = Some(element);
    for _ in 0..8 {
        let Some(node) = current else {
            break;
        };
        let author_id = ["data-author-id", "data-user-id", "data-player-id", "data-game-player-id"]
            .iter()
            .find_map(|key| {
                node.attr(key).map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned)
            })
            .and_then(canonical_forum_player_id);
        let author_element = selector.as_ref().and_then(|selector| node.select(selector).next());
        let author = author_element.map(render_post_text).filter(|value| !value.is_empty());
        let author_id = author_id.or_else(|| {
            author_element.and_then(|value| {
                let links = Selector::parse("a[href]").ok()?;
                value.select(&links).find_map(|link| forum_player_id_from_href(link.attr("href")?))
            })
        });
        if author_id.is_some() || author.is_some() {
            return (author_id, author);
        }
        current = node.parent().and_then(ElementRef::wrap);
    }
    (None, None)
}

fn forum_player_id_from_href(href: &str) -> Option<String> {
    let base = Url::parse("https://forums.e-hentai.org/").ok()?;
    let url = base.join(href).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("forums.e-hentai.org")
        || url.path() != "/index.php"
    {
        return None;
    }

    let mut player_id = None;
    for (name, value) in url.query_pairs() {
        if !matches!(name.as_ref(), "showuser" | "game_player_id" | "player_id") {
            continue;
        }
        let value = canonical_forum_player_id(value.into_owned())?;
        if player_id.as_ref().is_some_and(|current| current != &value) {
            return None;
        }
        player_id = Some(value);
    }
    player_id
}

fn canonical_forum_player_id(value: String) -> Option<String> {
    (value.len() <= 32
        && value.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && value.as_bytes().first() != Some(&b'0')
        && value.bytes().all(|byte| byte.is_ascii_digit()))
    .then_some(value)
}
