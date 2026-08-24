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
            edited: text.to_ascii_lowercase().contains("this post has been edited by"),
            text,
        });
    }
    super::parse_thread(posts)
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
    let selector =
        Selector::parse(".normalname, .author, .username, .post-author, .member_name").ok();
    let mut current = Some(element);
    for _ in 0..8 {
        let Some(node) = current else {
            break;
        };
        let author_id = ["data-author-id", "data-user-id", "data-player-id", "data-game-player-id"]
            .iter()
            .find_map(|key| {
                node.attr(key).map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned)
            });
        let author_element = selector.as_ref().and_then(|selector| node.select(selector).next());
        let author = author_element.map(render_post_text).filter(|value| !value.is_empty());
        let author_id = author_id.or_else(|| {
            author_element.and_then(|value| {
                let links = Selector::parse("a").ok()?;
                value.select(&links).find_map(|link| {
                    let href = link.attr("href")?;
                    let url = Url::parse(href)
                        .or_else(|_| Url::parse(&format!("https://invalid.local{href}")))
                        .ok()?;
                    ["game_player_id", "player_id"].iter().find_map(|key| {
                        url.query_pairs()
                            .find(|(name, _)| name == key)
                            .map(|(_, value)| value.into_owned())
                    })
                })
            })
        });
        if author_id.is_some() || author.is_some() {
            return (author_id, author);
        }
        current = node.parent().and_then(ElementRef::wrap);
    }
    (None, None)
}
