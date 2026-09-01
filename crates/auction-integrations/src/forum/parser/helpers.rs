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

pub(super) fn normalize_lot_code(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_start_matches('[').trim_end_matches(']');
    if !raw.is_ascii() {
        return None;
    }
    let mut bytes = [0_u8; 5];
    let mut length = 0;
    for byte in raw.bytes().filter(|byte| *byte != b'-') {
        if length == bytes.len() {
            return None;
        }
        bytes[length] = byte.to_ascii_uppercase();
        length += 1;
    }
    if length != bytes.len()
        || !bytes[..3].iter().all(u8::is_ascii_uppercase)
        || !bytes[3..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    bytes[1] = bytes[1].to_ascii_lowercase();
    bytes[2] = bytes[2].to_ascii_lowercase();
    String::from_utf8(bytes.to_vec()).ok()
}
