//! Public 看板的实时连接样式和刷新事件判定。

use super::*;

pub(crate) fn live_connection_class(state: LiveConnectionState) -> &'static str {
    match state {
        LiveConnectionState::Checking => "public-connection-badge--checking",
        LiveConnectionState::Connecting => "public-connection-badge--connecting",
        LiveConnectionState::Connected => "public-connection-badge--connected",
        LiveConnectionState::Anonymous => "public-connection-badge--anonymous",
        LiveConnectionState::Stopped => "public-connection-badge--stopped",
        LiveConnectionState::Error => "public-connection-badge--error",
    }
}

pub(crate) fn should_refresh_auction_board(event: &RealtimeEvent) -> bool {
    matches!(
        event.kind.as_str(),
        "auction.created"
            | "auction.preview_updated"
            | "preview_catalog_changed"
            | "auction.opened"
            | "auction.final_phase_entered"
            | "auction.cancelled"
            | "auction.settling"
            | "auction.settled"
            | "lot.bid_accepted"
    )
}
