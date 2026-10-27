//! Public 价格历史的共享分页、筛选、统计与安全映射。

use auction_application::{
    ApplicationError, PublicPriceHistoryCursor, PublicPriceHistoryEntry,
    PublicPriceHistoryListRequest, PublicPriceHistoryStats, RepositoryError,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

pub(crate) fn cursor_values(
    request: &PublicPriceHistoryListRequest,
) -> (DateTime<Utc>, i64, i64, Uuid) {
    match request.cursor.as_ref() {
        Some(PublicPriceHistoryCursor::Boundary {
            closed_at,
            auction_sequence,
            lot_number,
            lot_id,
        }) => (*closed_at, *auction_sequence, *lot_number, *lot_id),
        Some(PublicPriceHistoryCursor::Start) | None => (Utc::now(), 0, 0, Uuid::nil()),
    }
}

pub(crate) fn entry_cursor(entry: &PublicPriceHistoryEntry) -> PublicPriceHistoryCursor {
    PublicPriceHistoryCursor::Boundary {
        closed_at: entry.closed_at,
        auction_sequence: entry.auction_sequence,
        lot_number: entry.ordering_lot_number,
        lot_id: entry.lot_id,
    }
}

pub(crate) fn history_name_patterns(
    request: &PublicPriceHistoryListRequest,
) -> Option<Vec<String>> {
    if request.name_terms.is_empty() {
        return None;
    }
    Some(
        request
            .name_terms
            .iter()
            .map(|term| {
                format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
            })
            .collect(),
    )
}

pub(crate) fn validate_entries(
    entries: &[PublicPriceHistoryEntry],
) -> Result<(), ApplicationError> {
    if entries.iter().any(|entry| {
        entry.lot_id.is_nil()
            || entry.auction_id.is_nil()
            || entry.world_id.is_empty()
            || entry.world_id != entry.world_id.trim()
            || entry.auction_sequence <= 0
            || entry.ordering_lot_number <= 0
            || entry.lot_number <= 0
            || entry.equipment_name.is_empty()
            || entry.equipment_category.is_empty()
            || entry.equipment_level.is_some_and(|level| level < 0)
            || entry.buyer_name.is_empty()
            || entry.seller_name.is_empty()
            || entry.price <= 0
            || entry.closed_at.timestamp_micros() == 0
    }) {
        return Err(storage_error("公开价格历史包含无效条目"));
    }
    Ok(())
}

pub(crate) fn equipment_level(level: Value) -> Option<i64> {
    level.as_i64().filter(|level| *level >= 0)
}

pub(crate) fn validate_stats(
    stats: PublicPriceHistoryStats,
) -> Result<PublicPriceHistoryStats, ApplicationError> {
    stats.validate().map_err(storage_error)?;
    Ok(stats)
}

pub(crate) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
