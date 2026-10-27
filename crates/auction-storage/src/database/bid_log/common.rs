//! Public 竞价日志共享的 cursor、结果校验和错误映射。

pub(super) use crate::database::storage_error;
use auction_application::{
    ApplicationError, PublicBidLogCursor, PublicBidLogEntry, PublicBidLogListRequest,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub(super) fn cursor_values(
    request: &PublicBidLogListRequest,
) -> (DateTime<Utc>, Uuid, i64, i32, Uuid) {
    match request.cursor.as_ref() {
        Some(PublicBidLogCursor::Boundary {
            created_at,
            lot_id,
            accepted_state_version,
            fact_index,
            id,
        }) => (*created_at, *lot_id, *accepted_state_version, *fact_index, *id),
        Some(PublicBidLogCursor::Start) | None => (Utc::now(), Uuid::nil(), 0, 0, Uuid::nil()),
    }
}

pub(super) fn entry_cursor(entry: &PublicBidLogEntry) -> PublicBidLogCursor {
    PublicBidLogCursor::Boundary {
        created_at: entry.created_at,
        lot_id: entry.lot_id,
        accepted_state_version: entry.accepted_state_version,
        fact_index: entry.fact_index,
        id: entry.id,
    }
}

pub(super) fn validate_entries(
    entries: &[PublicBidLogEntry],
    request: &PublicBidLogListRequest,
) -> Result<(), ApplicationError> {
    for entry in entries {
        if entry.id.is_nil()
            || entry.auction_id != request.auction_id
            || entry.lot_id.is_nil()
            || request.lot_id.is_some_and(|lot_id| entry.lot_id != lot_id)
            || request.lot_code.as_deref().is_some_and(|needle| {
                !lot_code(&entry.equipment_category, entry.lot_number)
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
            || entry.lot_number <= 0
            || entry.amount <= 0
            || entry.accepted_state_version <= 0
            || entry.fact_index <= 0
            || entry.equipment_name.is_empty()
            || entry.equipment_category.is_empty()
            || entry.bidder_name.is_empty()
            || !matches!(entry.mechanism.as_str(), "ordinary" | "proxy")
            || !valid_source(entry)
            || entry.created_at.timestamp_micros() == 0
        {
            return Err(storage_error("公开竞价日志包含无效条目"));
        }
    }
    Ok(())
}

fn lot_code(category: &str, lot_number: i64) -> String {
    let prefix = match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => "Lot",
    };
    format!("{prefix}{lot_number:02}")
}

fn valid_source(entry: &PublicBidLogEntry) -> bool {
    let forum_ids = match (entry.source_thread_id.as_deref(), entry.source_post_id.as_deref()) {
        (Some(thread_id), Some(post_id)) => {
            canonical_forum_id(thread_id) && canonical_forum_id(post_id)
        }
        (None, None) => false,
        _ => return false,
    };
    match (entry.mechanism.as_str(), entry.source.as_deref(), entry.source_post_number) {
        ("proxy", Some("proxy"), None) => !forum_ids,
        ("ordinary", Some("forum"), Some(post_number)) => post_number >= 3 && forum_ids,
        // 000028 之前的论坛事实没有可证明的楼层号，只展示论坛来源。
        ("ordinary", Some("forum"), None) => {
            forum_ids || entry.source_thread_id.is_none() && entry.source_post_id.is_none()
        }
        ("ordinary", None, None) => {
            entry.source_thread_id.is_none() && entry.source_post_id.is_none()
        }
        _ => false,
    }
}

fn canonical_forum_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
}
