//! 论坛纯文本帖子切分与文本规整。

use super::model::ParsedPost;

pub fn parse_thread_text(input: &str) -> Result<super::AdminForumPreviewResult, ()> {
    super::parse_thread(split_posts(input))
}

pub(super) fn split_posts(input: &str) -> Vec<ParsedPost> {
    let mut posts = Vec::new();
    let mut current = None::<ParsedPost>;
    for line in normalized_lines(input) {
        if let Some((id, author)) = post_separator(line) {
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
            post.text.push_str(line);
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

pub(super) fn normalized_lines(value: &str) -> NormalizedLines<'_> {
    NormalizedLines { remaining: Some(value) }
}

pub(super) fn normalize_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    for line in normalized_lines(value) {
        let mut words = line.split_whitespace();
        let Some(first) = words.next() else {
            continue;
        };
        if !normalized.is_empty() {
            normalized.push('\n');
        }
        normalized.push_str(first);
        for word in words {
            normalized.push(' ');
            normalized.push_str(word);
        }
    }
    normalized
}

/// 同时兼容 LF、CRLF 与旧纯文本夹具中的单独 CR，不为每行分配 `String`。
pub(super) struct NormalizedLines<'a> {
    remaining: Option<&'a str>,
}

impl<'a> Iterator for NormalizedLines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.remaining.take()?;
        let Some(index) = remaining.bytes().position(|byte| matches!(byte, b'\r' | b'\n')) else {
            return Some(remaining);
        };
        let delimiter_bytes = if remaining.as_bytes()[index] == b'\r'
            && remaining.as_bytes().get(index + 1) == Some(&b'\n')
        {
            2
        } else {
            1
        };
        self.remaining = Some(&remaining[index + delimiter_bytes..]);
        Some(&remaining[..index])
    }
}
