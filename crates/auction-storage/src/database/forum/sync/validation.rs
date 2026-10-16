//! 论坛主题同步的输入闭集、编码约束和后端能力错误。

use auction_application::{
    ApplicationError, ForumPublishedLayout, ForumPublishedThread, ForumRenderedPost,
    ForumRenderedThread, ForumSyncClaim, RepositoryError,
};
#[cfg(feature = "postgres")]
use auction_domain::SettlementWorld;

pub(super) const MAX_CLAIM_BATCH_SIZE: u16 = 100;
pub(super) const MAX_WORKER_ID_BYTES: usize = 256;
pub(super) const MAX_FAILURE_CODE_BYTES: usize = 128;
pub(super) const MAX_FAILURE_DETAIL_BYTES: usize = 2048;
pub(super) const MAX_THREAD_ID_BYTES: usize = 64;
pub(super) const MAX_FIXED_POST_ID_BYTES: usize = 32;
pub(super) const MAX_THREAD_URL_BYTES: usize = 2048;
pub(super) const MAX_RENDERED_TITLE_BYTES: usize = 512;
pub(super) const MAX_RENDERED_DESCRIPTION_BYTES: usize = 512;
pub(super) const MAX_RENDERED_BODY_BYTES: usize = 65_531;

pub(super) fn valid_claim(claim: &ForumSyncClaim) -> bool {
    !claim.auction_id.is_nil()
        && !claim.claim_token.is_nil()
        && valid_text(&claim.worker_id, MAX_WORKER_ID_BYTES)
        && claim.claim_fencing > 0
        && claim.source_revision > 0
        && claim
            .existing_thread_id
            .as_ref()
            .is_none_or(|value| valid_decimal_id(value, MAX_THREAD_ID_BYTES))
        && claim
            .previous_failure_code
            .as_ref()
            .is_none_or(|code| valid_text(code, MAX_FAILURE_CODE_BYTES))
        && claim.claimed_at.timestamp_nanos_opt().is_some()
        && claim.lease_until > claim.claimed_at
}

pub(super) fn valid_rendered(rendered: &ForumRenderedThread) -> bool {
    !rendered.title.is_empty()
        && rendered.title == rendered.title.trim()
        && rendered.title.len() <= MAX_RENDERED_TITLE_BYTES
        && !rendered.title.chars().any(char::is_control)
        && valid_text(&rendered.description, MAX_RENDERED_DESCRIPTION_BYTES)
        && rendered
            .legacy_title
            .as_deref()
            .is_none_or(|value| valid_text(value, MAX_RENDERED_TITLE_BYTES))
        && rendered.catalog_continuation.len() < auction_application::MAX_FORUM_CATALOG_PAGES
        && rendered.catalog_continuation.iter().all(valid_rendered_post)
        && !rendered.body.trim().is_empty()
        && rendered.body.len() <= MAX_RENDERED_BODY_BYTES
        && !rendered
            .body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

pub(super) fn valid_rendered_post(rendered: &ForumRenderedPost) -> bool {
    !rendered.body.trim().is_empty()
        && rendered.body.len() <= MAX_RENDERED_BODY_BYTES
        && !rendered
            .body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

pub(super) fn valid_published(published: &ForumPublishedThread) -> bool {
    published.board_id > 0
        && valid_decimal_id(&published.thread_id, MAX_THREAD_ID_BYTES)
        && valid_text(&published.thread_url, MAX_THREAD_URL_BYTES)
        && published.thread_url.starts_with("https://")
}

pub(super) fn valid_layout(published: &ForumPublishedLayout) -> bool {
    valid_published(&published.thread)
        && valid_decimal_id(&published.catalog_post_id, MAX_FIXED_POST_ID_BYTES)
        && valid_decimal_id(&published.system_log_post_id, MAX_FIXED_POST_ID_BYTES)
        && super::super::catalog::valid_page_ids(
            &published.catalog_post_id,
            &published.system_log_post_id,
            published.catalog_continuation_post_ids.iter().map(String::as_str),
        )
}

pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_decimal_id(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.as_bytes()[0].is_ascii_digit()
        && value.as_bytes()[0] != b'0'
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(feature = "postgres")]
pub(super) fn parse_world(value: &str) -> Result<SettlementWorld, ApplicationError> {
    match value {
        "main" => Ok(SettlementWorld::Main),
        "isekai" => Ok(SettlementWorld::Isekai),
        _ => Err(storage_error("论坛快照世界不在闭集内")),
    }
}

#[cfg(feature = "postgres")]
pub(super) fn lot_code(category: &str, lot_number: i32) -> Result<String, ApplicationError> {
    let prefix = match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => return Err(storage_error("论坛快照装备分类不在闭集内")),
    };
    if !(1..=99).contains(&lot_number) {
        return Err(storage_error("论坛快照 lot 序号超出论坛编码范围"));
    }
    Ok(format!("{prefix}{lot_number:02}"))
}

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
