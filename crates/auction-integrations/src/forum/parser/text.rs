//! 论坛纯文本帖子切分与文本规整。

use super::model::ParsedPost;

pub fn parse_thread_text(input: &str) -> Result<super::AdminForumPreviewResult, ()> {
    super::parse_thread_with_identity(split_posts(input), false)
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

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;

    const THREAD: &str = "\
===== post-6956800 author=cataloger =====
One-Handed
[One01] Test rapier (seller: alice) — Start: 50k
===== post-6956801 author=bidder =====
One01 200k
===== post-6956802 author=bidder =====
One01 250k
This post has been edited by bidder
";

    #[test]
    fn text_thread_parses_catalog_and_bid_commands() {
        let result = parse_thread_text(THREAD).expect("完整目录与竞价行必须能形成预览");

        assert_eq!(result.catalog_post_id, "6956800");
        assert_eq!(result.catalog.lots.len(), 1);
        assert_eq!(result.catalog.lots[0].code, "One01");
        assert_eq!(result.posts.len(), 2);
        assert_eq!(result.posts[0].commands.len(), 1);
        assert_eq!(result.posts[0].commands[0].kind, "bid");
        assert_eq!(result.posts[0].commands[0].amount, Some(200_000));
        assert_eq!(result.posts[0].commands[0].actor.as_deref(), Some("bidder"));
        // 编辑过的帖子不能创建竞价或命令。
        assert!(result.posts[1].ignored);
        assert!(result.posts[1].commands.is_empty());
        assert!(result.warnings.iter().any(|warning| warning.code == "edited_post_ignored"));
    }

    #[test]
    fn text_thread_preserves_line_endings_and_rejects_missing_separators() {
        let posts = split_posts(
            "===== post-1 author=a =====\r\nOne-Handed\r\n[A01] x\r\n===== post-2 author=b =====\r\nA01 1000\r\n",
        );
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].id, "1");
        assert_eq!(posts[1].id, "2");
        assert_eq!(posts[0].author.as_deref(), Some("a"));

        // 没有 `post-` 前缀的分隔线只是正文，不产生帖子身份。
        assert!(split_posts("===== 6956800 =====\nOne01 200k\n").is_empty());
        assert!(parse_thread_text("One01 200k\n").is_err());
    }

    #[test]
    fn text_thread_reports_unknown_lot_without_creating_a_command() {
        let thread = "\
===== post-1 author=cataloger =====
One-Handed
[One01] Test rapier (seller: alice) — Start: 50k
===== post-2 author=bidder =====
Bad99 100k
";
        let result = parse_thread_text(thread).expect("未知拍品代码属于可定位警告而非整页失败");
        assert!(result.posts[0].commands.is_empty());
        assert!(result.warnings.iter().any(|warning| warning.code == "unknown_lot"));
    }

    #[test]
    fn text_thread_rejects_duplicate_catalog_codes() {
        let thread = "\
===== post-1 author=cataloger =====
One-Handed
[One01] First (seller: alice) — Start: 50k
[One01] Duplicate (seller: alice) — Start: 50k
";
        assert!(parse_thread_text(thread).is_err());
    }
}
