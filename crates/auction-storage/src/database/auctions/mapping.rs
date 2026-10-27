//! Public 拍卖行到应用投影的跨后端映射与请求校验。

use auction_application::{
    ApplicationError, CurrentAuctionSnapshot, PublicAuctionListRequest, PublicAuctionSummary,
    PublicForumLink, PublicRealtimeAuctionStatus, RepositoryError,
};
#[cfg(feature = "postgres")]
use sqlx::Row;

#[cfg(feature = "postgres")]
pub(super) fn map_postgres_summary(
    row: sqlx::postgres::PgRow,
) -> Result<PublicAuctionSummary, ApplicationError> {
    let auction_id = row.try_get("id").map_err(|_| storage_error("读取公开拍卖标识失败"))?;
    let forum_link = map_postgres_forum_link(&row, auction_id)?;
    Ok(PublicAuctionSummary {
        id: auction_id,
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
        forum_link,
    })
}

/// 未对账历史记录由关联条件省略；已声明内容证明的记录必须构造已校验链接，损坏数据返回错误。
#[cfg(feature = "postgres")]
fn map_postgres_forum_link(
    row: &sqlx::postgres::PgRow,
    auction_id: uuid::Uuid,
) -> Result<Option<PublicForumLink>, ApplicationError> {
    let forum_auction_id: Option<uuid::Uuid> =
        row.try_get("forum_auction_id").map_err(|_| storage_error("读取公开论坛所属拍卖失败"))?;
    let Some(forum_auction_id) = forum_auction_id else { return Ok(None) };
    if forum_auction_id != auction_id {
        return Err(storage_error("公开论坛所属拍卖不一致"));
    }
    let url =
        row.try_get("forum_thread_url").map_err(|_| storage_error("读取公开拍卖论坛地址失败"))?;
    let content_revision = row
        .try_get("forum_content_revision")
        .map_err(|_| storage_error("读取公开论坛内容版本失败"))?;
    let content_synced_at = row
        .try_get("forum_content_synced_at")
        .map_err(|_| storage_error("读取公开论坛内容同步时间失败"))?;
    PublicForumLink::new(url, content_revision, content_synced_at).map(Some).map_err(storage_error)
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
