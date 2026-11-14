//! 玩家支持查询的响应解码与参数编码。

use super::MAX_PLAYER_SUPPORT_ROWS;
use super::decode::{StrictDeliveryEnvelope, decode_delivery, parse_delivery_time};
use crate::api::{MAX_JSON_SAFE_INTEGER, PlayerSupportBundle, valid_safe_text, valid_uuid};

/// 只放行页面实际展示的闭集与有界数值；任何未知种类或越界 ID 都让整页失败关闭。
pub(super) fn decode_player_support(
    payload: StrictDeliveryEnvelope<PlayerSupportBundle>,
) -> Result<PlayerSupportBundle, ()> {
    if payload.code != "ok" || payload.message.is_empty() {
        return Err(());
    }
    let bundle = payload.data;
    if bundle.bids.len() > MAX_PLAYER_SUPPORT_ROWS
        || bundle.deals.len() > MAX_PLAYER_SUPPORT_ROWS
        || bundle.obligations.len() > MAX_PLAYER_SUPPORT_ROWS
        || bundle.deliveries.len() > MAX_PLAYER_SUPPORT_ROWS
        || bundle.notifications.len() > MAX_PLAYER_SUPPORT_ROWS
    {
        return Err(());
    }
    if let Some(user) = &bundle.user
        && (!valid_uuid(&user.id)
            || user.username.is_empty()
            || !matches!(user.status.as_str(), "active" | "blacklisted")
            || user.game_player_id.as_deref().is_some_and(str::is_empty))
    {
        return Err(());
    }
    for bid in &bundle.bids {
        if !valid_uuid(&bid.bid_id)
            || !valid_uuid(&bid.lot_id)
            || !valid_uuid(&bid.auction_id)
            || bid.lot_number < 0
            || !valid_world(&bid.world_id)
            || !(0..=MAX_JSON_SAFE_INTEGER).contains(&bid.amount)
            || !matches!(bid.mechanism.as_str(), "ordinary" | "proxy")
            || !matches!(
                bid.lot_status.as_str(),
                "pending" | "live" | "sold" | "unsold" | "withdrawn"
            )
            || parse_delivery_time(&bid.created_at).is_none()
        {
            return Err(());
        }
    }
    for deal in &bundle.deals {
        if !valid_uuid(&deal.lot_id)
            || !valid_uuid(&deal.auction_id)
            || deal.lot_number < 0
            || !valid_world(&deal.world_id)
            || deal.equipment_name.is_empty()
            || !(0..=MAX_JSON_SAFE_INTEGER).contains(&deal.final_price)
        {
            return Err(());
        }
    }
    for obligation in &bundle.obligations {
        if !valid_uuid(&obligation.obligation_id)
            || !valid_uuid(&obligation.auction_id)
            || !valid_uuid(&obligation.lot_id)
            || !valid_world(&obligation.world_id)
            || !valid_safe_text(&obligation.obligation_kind, 64)
            || !matches!(obligation.business_state.as_str(), "ready" | "blocked" | "contingent")
            || !(0..=MAX_JSON_SAFE_INTEGER).contains(&obligation.amount)
            || obligation.delivery_id.as_deref().is_some_and(|id| !valid_uuid(id))
        {
            return Err(());
        }
    }
    for delivery in &bundle.deliveries {
        decode_delivery(delivery.clone())?;
    }
    for notification in &bundle.notifications {
        if !valid_uuid(&notification.notification_id)
            || !matches!(notification.channel.as_str(), "mm" | "pm" | "pending")
            || !valid_safe_text(&notification.state, 64)
            || notification.lot_id.as_deref().is_some_and(|id| !valid_uuid(id))
            || notification.auction_id.as_deref().is_some_and(|id| !valid_uuid(id))
            || notification.delivery_id.as_deref().is_some_and(|id| !valid_uuid(id))
            || notification.state_reason.as_deref().is_some_and(|value| !valid_safe_text(value, 96))
            || parse_delivery_time(&notification.created_at).is_none()
            || parse_delivery_time(&notification.updated_at).is_none()
        {
            return Err(());
        }
    }
    for reference in &bundle.references {
        if !matches!(
            reference.kind.as_str(),
            "delivery" | "obligation" | "notification" | "lot" | "auction"
        ) || !valid_uuid(&reference.id)
        {
            return Err(());
        }
    }
    Ok(bundle)
}

pub(super) fn valid_world(world: &str) -> bool {
    matches!(world, "main" | "isekai")
}

pub(super) fn url_encode(value: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new()).append_pair("v", value).finish()[2..]
        .to_owned()
}
