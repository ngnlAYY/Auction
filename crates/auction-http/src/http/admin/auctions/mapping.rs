use auction_application::AdminAuctionSummary;
use auction_contracts::AdminAuctionResponse;

/// 将拍卖应用摘要转换为管理端安全 DTO。
pub(crate) fn map_auction(summary: AdminAuctionSummary) -> AdminAuctionResponse {
    let auction = summary.auction;
    AdminAuctionResponse {
        id: auction.id.to_string(),
        world_id: auction.world_id,
        sequence: auction.sequence,
        status: auction.status,
        preview_scheduled_at: super::timestamp(auction.preview_scheduled_at),
        scheduled_open_at: super::timestamp(auction.scheduled_open_at),
        preview_opened_at: auction.preview_opened_at.map(super::timestamp),
        preview_manual_open: auction.preview_manual_open,
        preview_revision: auction.preview_revision,
        opened_at: auction.opened_at.map(super::timestamp),
        final_phase_at: auction.final_phase_at.map(super::timestamp),
        settled_at: auction.settled_at.map(super::timestamp),
        cancelled_at: auction.cancelled_at.map(super::timestamp),
        cancelled_reason: auction.cancelled_reason,
        cancelled_by: auction.cancelled_by,
        state_version: auction.state_version,
        scheduled_end_at: auction.scheduled_end_at.map(super::timestamp),
        effective_end_at: auction.effective_end_at.map(super::timestamp),
        scheduled_ends_at: summary.scheduled_ends_at.map(super::timestamp),
        lot_count: summary.lot_count,
        created_at: super::timestamp(auction.created_at),
        updated_at: super::timestamp(auction.updated_at),
    }
}
