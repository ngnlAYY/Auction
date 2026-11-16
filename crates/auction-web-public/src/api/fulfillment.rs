//! Public 履约进度的 HTTP DTO 与请求：已结算场次目录与逐件履约状态页。

pub(crate) use auction_contracts::{
    PublicFulfillmentLotResponse as FulfillmentLot,
    PublicFulfillmentPageResponse as FulfillmentPage,
};
use url::form_urlencoded;

use super::queries::{cached_public_get, fetch_bid_log_auction_status};
use super::{HistoricalAuction, decoder, swr_cache};

/// 履约进度接口的每页数量上限；超过时服务端返回 400，页面按上限请求。
pub(crate) const MAX_FULFILLMENT_PAGE_SIZE: i64 = 100;

/// 读取一个世界全部已结算场次，按期号倒序；只有已结算场次才有履约进度。
pub(crate) async fn fetch_fulfillment_auctions(
    world_id: String,
) -> Result<Vec<HistoricalAuction>, String> {
    let mut auctions = fetch_bid_log_auction_status(&world_id, "settled").await?;
    // 目录逐页读取期间新结算的场次会让后页重复出现前页的末尾行。
    auctions.sort_by(|left, right| {
        right.sequence.cmp(&left.sequence).then_with(|| right.id.cmp(&left.id))
    });
    auctions.dedup_by(|left, right| left.id == right.id);
    Ok(auctions)
}

/// 读取单场的履约进度编号页；响应必须与请求的场次、世界与分页一致，否则按无效响应处理。
pub(crate) async fn fetch_fulfillment_page(
    auction_id: String,
    world_id: String,
    lot: Option<String>,
    page: i64,
    page_size: i64,
) -> Result<FulfillmentPage, String> {
    let page = page.max(1);
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.to_string())
        .append_pair("page_size", &page_size.to_string())
        .append_pair("world", &world_id);
    if let Some(lot) = lot.as_deref() {
        query.append_pair("lot", lot);
    }
    let url = format!("/api/v1/auctions/{auction_id}/fulfillment?{}", query.finish());
    cached_public_get(&url, swr_cache::CATALOG_TTL_MILLIS, move |response| async move {
        decoder::decode_fulfillment_page(response, &auction_id, &world_id, page, page_size).await
    })
    .await
}
