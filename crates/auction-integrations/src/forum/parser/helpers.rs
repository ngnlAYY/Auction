//! 论坛预览的结果映射、警告和排序辅助。

use auction_application::{AdminForumPost, AdminForumWarning};

use super::model::ParsedPost;

pub(super) fn map_post(post: ParsedPost) -> AdminForumPost {
    AdminForumPost {
        id: post.id,
        author_id: post.author_id,
        author: post.author,
        text: post.text,
        edited: post.edited,
    }
}

pub(super) fn is_audit_post(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("warning log:") || value.contains("manually ignored posts")
}

pub(super) fn warning(
    line: usize,
    code: &str,
    message: &str,
    text: Option<&str>,
    post_id: Option<String>,
) -> AdminForumWarning {
    AdminForumWarning {
        line,
        code: code.to_owned(),
        message: message.to_owned(),
        text: text.map(str::to_owned),
        post_id,
    }
}

pub(super) fn sort_warnings(warnings: &mut [AdminForumWarning]) {
    warnings.sort_by(|left, right| {
        left.post_id.cmp(&right.post_id).then_with(|| left.line.cmp(&right.line))
    });
}
