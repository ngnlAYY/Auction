//! 固定楼层身份校验与系统日志条目构造（含安全英文投影清洗）。

use std::collections::HashMap;

use auction_application::core::ApplicationError;
use auction_application::forum::{
    ForumSystemLogEntry, forum_catalog_marker, forum_system_log_marker,
};

use super::super::completion::repository_error;
use super::super::rejection::warning_message;

pub(super) fn validate_fixed_layout(
    topic: &auction_application::forum::ForumReplyTopic,
    preview: &auction_application::admin::AdminForumPreviewResult,
) -> Result<(), ApplicationError> {
    let catalog_marker = forum_catalog_marker(topic.auction_id);
    let system_marker = forum_system_log_marker(topic.auction_id);
    let system_index = topic.catalog_continuation.len();
    let Some(system) = preview.posts.get(system_index) else {
        return Err(repository_error("论坛系统更新楼已丢失"));
    };
    if preview.catalog_post_id != topic.catalog_post_id
        || !preview.catalog_post_text.contains(&catalog_marker)
        || system.post.id != topic.system_log_post_id
        || !system.post.text.contains(&system_marker)
        || preview.catalog.lots.len().div_ceil(topic.catalog_layout.page_size()) != system_index + 1
    {
        return Err(repository_error("论坛固定楼层或目录页数与持久化证明不一致"));
    }
    for (index, page) in topic.catalog_continuation.iter().enumerate() {
        let Some(post) = preview.posts.get(index) else {
            return Err(repository_error("论坛目录续页已丢失"));
        };
        if post.post.id != page.post_id
            || !post.post.text.contains(&auction_application::forum::forum_catalog_page_marker(
                topic.auction_id,
                index + 2,
            ))
        {
            return Err(repository_error("论坛目录续页身份与持久化证明不一致"));
        }
    }
    Ok(())
}

pub(super) fn warning_entries(
    topic: &auction_application::forum::ForumReplyTopic,
    preview: &auction_application::admin::AdminForumPreviewResult,
    post_numbers: &HashMap<&str, usize>,
) -> Vec<ForumSystemLogEntry> {
    preview
        .warnings
        .iter()
        .filter_map(|warning| {
            let (post_number, post_id) = match warning.post_id.as_ref() {
                Some(post_id) => (post_numbers.get(post_id.as_str()).copied()?, post_id.as_str()),
                None => (1, preview.catalog_post_id.as_str()),
            };
            let catalog_warning = post_number < topic.system_log_post_number()
                && matches!(warning.code.as_str(), "invalid_catalog_row" | "invalid_start_price");
            (catalog_warning || post_number > topic.system_log_post_number()).then(|| {
                let (code, message) = warning_message(&warning.code);
                system_entry(
                    post_number,
                    post_id,
                    (warning.line > 0).then_some(warning.line),
                    code,
                    message,
                    warning.code == "edited_post_ignored",
                )
            })
        })
        .collect()
}

pub(super) fn runtime_warning(
    post_number: usize,
    post_id: &str,
    line_number: Option<usize>,
    code: &str,
) -> ForumSystemLogEntry {
    let (code, message) = warning_message(code);
    system_entry(post_number, post_id, line_number, code, message, false)
}

pub(super) fn system_entry(
    post_number: usize,
    post_id: &str,
    line_number: Option<usize>,
    code: &str,
    message: &str,
    ignored: bool,
) -> ForumSystemLogEntry {
    ForumSystemLogEntry {
        post_number,
        post_id: post_id.to_owned(),
        line_number,
        code: clean_system_message(code),
        message: clean_system_message(message),
        ignored,
    }
}

pub(super) fn clean_system_message(value: &str) -> String {
    value
        .split_whitespace()
        .enumerate()
        .flat_map(|(index, word)| (index > 0).then_some(' ').into_iter().chain(word.chars()))
        .take(512)
        .collect()
}
