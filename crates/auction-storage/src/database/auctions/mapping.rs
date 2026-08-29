//! Public 拍卖行到应用投影的跨后端映射与请求校验。

use auction_application::{
    ApplicationError, CurrentAuctionSnapshot, PublicAuctionListRequest, PublicAuctionSummary,
    PublicRealtimeAuctionStatus, RepositoryError,
};
#[cfg(feature = "sqlite")]
use chrono::{DateTime, NaiveDateTime, Utc};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use sqlx::Row;
#[cfg(feature = "sqlite")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
pub(super) fn map_postgres_summary(
    row: sqlx::postgres::PgRow,
) -> Result<PublicAuctionSummary, ApplicationError> {
    Ok(PublicAuctionSummary {
        id: row.try_get("id").map_err(|_| storage_error("读取公开拍卖标识失败"))?,
        world_id: row.try_get("world_id").map_err(|_| storage_error("读取公开拍卖世界失败"))?,
        sequence: row.try_get("sequence").map_err(|_| storage_error("读取公开拍卖序号失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取公开拍卖状态失败"))?,
        scheduled_open_at: row
            .try_get("scheduled_open_at")
            .map_err(|_| storage_error("读取公开拍卖开始时间失败"))?,
        opened_at: row
            .try_get("opened_at")
            .map_err(|_| storage_error("读取公开拍卖开放时间失败"))?,
        final_phase_at: row
            .try_get("final_phase_at")
            .map_err(|_| storage_error("读取公开拍卖终阶段时间失败"))?,
        effective_end_at: row
            .try_get("effective_end_at")
            .map_err(|_| storage_error("读取公开拍卖有效结束时间失败"))?,
        settled_at: row
            .try_get("settled_at")
            .map_err(|_| storage_error("读取公开拍卖结算时间失败"))?,
        lot_count: row
            .try_get("lot_count")
            .map_err(|_| storage_error("读取公开拍卖 lot 数量失败"))?,
        sold_count: row
            .try_get("sold_count")
            .map_err(|_| storage_error("读取公开拍卖售出数量失败"))?,
        unsold_count: row
            .try_get("unsold_count")
            .map_err(|_| storage_error("读取公开拍卖未售数量失败"))?,
        sold_amount: row
            .try_get("sold_amount")
            .map_err(|_| storage_error("读取公开拍卖售出金额失败"))?,
        state_version: row
            .try_get("state_version")
            .map_err(|_| storage_error("读取公开拍卖状态版本失败"))?,
    })
}

#[cfg(feature = "postgres")]
pub(super) fn map_postgres_current(
    row: sqlx::postgres::PgRow,
) -> Result<CurrentAuctionSnapshot, ApplicationError> {
    let snapshot = CurrentAuctionSnapshot {
        id: row.try_get("id").map_err(|_| storage_error("读取当前拍卖标识失败"))?,
        world_id: row.try_get("world_id").map_err(|_| storage_error("读取当前拍卖世界失败"))?,
        sequence: row.try_get("sequence").map_err(|_| storage_error("读取当前拍卖序号失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取当前拍卖状态失败"))?,
        opens_at: row
            .try_get("scheduled_open_at")
            .map_err(|_| storage_error("读取当前拍卖开始时间失败"))?,
        scheduled_ends_at: row
            .try_get("scheduled_ends_at")
            .map_err(|_| storage_error("读取当前拍卖结束时间失败"))?,
        presentation_mode: row
            .try_get("presentation_mode")
            .map_err(|_| storage_error("读取当前拍卖展示模式失败"))?,
        bidding_open: row
            .try_get("bidding_open")
            .map_err(|_| storage_error("读取当前拍卖竞价状态失败"))?,
        preview_revision: row
            .try_get("preview_revision")
            .map_err(|_| storage_error("读取当前拍卖预览版本失败"))?,
    };
    snapshot.validate().map_err(storage_error)?;
    Ok(snapshot)
}

#[cfg(feature = "sqlite")]
pub(super) fn map_sqlite_summary(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PublicAuctionSummary, ApplicationError> {
    Ok(PublicAuctionSummary {
        id: uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite 公开拍卖标识失败"))?,
            "公开拍卖标识",
        )?,
        world_id: row
            .try_get("world_id")
            .map_err(|_| storage_error("读取 Lite 公开拍卖世界失败"))?,
        sequence: row
            .try_get("sequence")
            .map_err(|_| storage_error("读取 Lite 公开拍卖序号失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取 Lite 公开拍卖状态失败"))?,
        scheduled_open_at: parse_timestamp(
            row.try_get("scheduled_open_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖开始时间失败"))?,
            "公开拍卖开始时间",
        )?,
        opened_at: parse_optional_timestamp(
            row.try_get("opened_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖开放时间失败"))?,
            "公开拍卖开放时间",
        )?,
        final_phase_at: parse_optional_timestamp(
            row.try_get("final_phase_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖终阶段时间失败"))?,
            "公开拍卖终阶段时间",
        )?,
        effective_end_at: parse_optional_timestamp(
            row.try_get("effective_end_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖有效结束时间失败"))?,
            "公开拍卖有效结束时间",
        )?,
        settled_at: parse_optional_timestamp(
            row.try_get("settled_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖结算时间失败"))?,
            "公开拍卖结算时间",
        )?,
        lot_count: row
            .try_get("lot_count")
            .map_err(|_| storage_error("读取 Lite 公开拍卖 lot 数量失败"))?,
        sold_count: row
            .try_get("sold_count")
            .map_err(|_| storage_error("读取 Lite 公开拍卖售出数量失败"))?,
        unsold_count: row
            .try_get("unsold_count")
            .map_err(|_| storage_error("读取 Lite 公开拍卖未售数量失败"))?,
        sold_amount: row
            .try_get("sold_amount")
            .map_err(|_| storage_error("读取 Lite 公开拍卖售出金额失败"))?,
        state_version: row
            .try_get("state_version")
            .map_err(|_| storage_error("读取 Lite 公开拍卖状态版本失败"))?,
    })
}

#[cfg(feature = "sqlite")]
pub(super) fn map_sqlite_current(
    row: sqlx::sqlite::SqliteRow,
) -> Result<CurrentAuctionSnapshot, ApplicationError> {
    let snapshot = CurrentAuctionSnapshot {
        id: uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite 当前拍卖标识失败"))?,
            "当前拍卖标识",
        )?,
        world_id: row
            .try_get("world_id")
            .map_err(|_| storage_error("读取 Lite 当前拍卖世界失败"))?,
        sequence: row
            .try_get("sequence")
            .map_err(|_| storage_error("读取 Lite 当前拍卖序号失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取 Lite 当前拍卖状态失败"))?,
        opens_at: parse_timestamp(
            row.try_get("scheduled_open_at")
                .map_err(|_| storage_error("读取 Lite 当前拍卖开始时间失败"))?,
            "当前拍卖开始时间",
        )?,
        scheduled_ends_at: parse_optional_timestamp(
            row.try_get("scheduled_ends_at")
                .map_err(|_| storage_error("读取 Lite 当前拍卖结束时间失败"))?,
            "当前拍卖结束时间",
        )?,
        presentation_mode: row
            .try_get("presentation_mode")
            .map_err(|_| storage_error("读取 Lite 当前拍卖展示模式失败"))?,
        bidding_open: row
            .try_get::<i64, _>("bidding_open")
            .map_err(|_| storage_error("读取 Lite 当前拍卖竞价状态失败"))?
            != 0,
        preview_revision: row
            .try_get("preview_revision")
            .map_err(|_| storage_error("读取 Lite 当前拍卖预览版本失败"))?,
    };
    snapshot.validate().map_err(storage_error)?;
    Ok(snapshot)
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn parse_optional_timestamp(
    value: Option<String>,
    label: &str,
) -> Result<Option<DateTime<Utc>>, ApplicationError> {
    value.map(|value| parse_timestamp(value, label)).transpose()
}

#[cfg(feature = "sqlite")]
fn parse_timestamp(value: String, label: &str) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error(label))
}

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

pub(super) fn validate_list_request(
    request: &PublicAuctionListRequest,
) -> Result<(), ApplicationError> {
    if request.status.trim().is_empty()
        || request.status != request.status.trim()
        || request.limit < 1
        || request.offset < 0
        || request.cursor.is_some() && request.offset != 0
    {
        return Err(storage_error("公开拍卖列表参数无效"));
    }
    if request
        .world_id
        .as_deref()
        .is_some_and(|world| world.is_empty() || world.len() > 64 || world != world.trim())
    {
        return Err(storage_error("公开拍卖世界参数无效"));
    }
    Ok(())
}

pub(super) fn map_realtime_status(
    status: &str,
) -> Result<PublicRealtimeAuctionStatus, ApplicationError> {
    match status {
        "preview" => Ok(PublicRealtimeAuctionStatus::Preview),
        "live" => Ok(PublicRealtimeAuctionStatus::Live),
        "final" => Ok(PublicRealtimeAuctionStatus::Final),
        "settling" => Ok(PublicRealtimeAuctionStatus::Settling),
        "settled" => Ok(PublicRealtimeAuctionStatus::Settled),
        _ => Err(storage_error("读取 Public realtime 拍卖状态失败")),
    }
}
