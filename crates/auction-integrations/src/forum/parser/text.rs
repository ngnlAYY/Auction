//! 论坛纯文本帖子切分与文本规整。

use super::model::ParsedPost;

pub fn parse_thread_text(input: &str) -> Result<super::AdminForumPreviewResult, ()> {
    super::parse_thread(split_posts(input))
}

pub(super) fn split_posts(input: &str) -> Vec<ParsedPost> {
    let mut posts = Vec::new();
    let mut current = None::<ParsedPost>;
    for line in normalized_lines(input) {
        if let Some((id, author)) = post_separator(&line) {
            if let Some(mut post) = current.take() {
                post.text = post.text.trim().to_owned();
                post.edited =
                    post.text.to_ascii_lowercase().contains("this post has been edited by");
                posts.push(post);
            }
            current = Some(ParsedPost {
                id,
                author_id: None,
                author,
                text: String::new(),
                edited: false,
            });
            continue;
        }
        let Some(post) = current.as_mut() else {
            continue;
        };
        if post.text.is_empty() && line.trim_start().starts_with("Author:") {
            post.author = Some(line.trim_start()["Author:".len()..].trim().to_owned());
        } else {
            post.text.push_str(&line);
            post.text.push('\n');
        }
    }
    if let Some(mut post) = current {
        post.text = post.text.trim().to_owned();
        post.edited = post.text.to_ascii_lowercase().contains("this post has been edited by");
        posts.push(post);
    }
    posts
}

fn post_separator(line: &str) -> Option<(String, Option<String>)> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix("=====")?.strip_suffix("=====")?.trim();
    let mut fields = inner.split_whitespace();
    let id = fields.next()?.strip_prefix("post-")?.to_owned();
    if id.is_empty() {
        return None;
    }
    let author = fields
        .find_map(|field| field.strip_prefix("author=").map(str::to_owned))
        .filter(|value| !value.is_empty());
    Some((id, author))
}

pub(super) fn normalized_lines(value: &str) -> Vec<String> {
    value.replace("\r\n", "\n").replace('\r', "\n").split('\n').map(str::to_owned).collect()
}

pub(super) fn normalize_text(value: &str) -> String {
    normalized_lines(value)
        .into_iter()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
