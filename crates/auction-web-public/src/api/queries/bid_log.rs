//! 公开竞价日志读取，以及竞价日志与履约进度共用的按状态场次目录分页。

use url::form_urlencoded;

use super::super::swr_cache;
use super::super::{BidLogPage, decoder};
use super::cached_get::cached_public_get;
use super::directory::{SUMMARY_PAGE_SIZE, fetch_auction_page};

/// 竞价日志的附加字段：代理竞争中落败的授权。缺省响应不含它，旧页面按原字段闭集解码。
const BID_LOG_INCLUDE: &str = "defeated_proxy";

pub(crate) async fn fetch_bid_log_page(
    auction_id: String,
    page: i64,
    world_id: String,
    lot_id: Option<String>,
    page_size: i64,
) -> Result<BidLogPage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", &page_size.to_string())
        .append_pair("include", BID_LOG_INCLUDE);
    if !world_id.is_empty() {
        query.append_pair("world", &world_id);
    }
    if let Some(lot_id) = lot_id {
        query.append_pair("lot", &lot_id);
    }
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{}", query.finish());
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, decoder::decode_bid_log_page).await
}

/// 读取场次选择器某个状态的一页，并告诉页面是否已到目录末尾。
pub(crate) async fn fetch_bid_log_auctions(
    world_id: String,
    status: &str,
    page_number: i64,
) -> Result<(Vec<super::super::HistoricalAuction>, bool), String> {
    let page =
        fetch_auction_page(Some(world_id), status.to_owned(), page_number, SUMMARY_PAGE_SIZE)
            .await?;
    let row_count = i64::try_from(page.auctions.len()).map_err(|_| "拍卖目录页过大".to_owned())?;
    if page.page_size <= 0 {
        return Err("拍卖目录分页大小无效".to_owned());
    }
    // 满页且缺 total 时继续读取，不能据此判定指定场次不可见。
    let complete = row_count < page.page_size
        || page.total.is_some_and(|total| page_number.saturating_mul(page.page_size) >= total);
    Ok((page.auctions, complete))
}

/// 读取单个 lot 的编号竞价历史；分页参数沿用公开竞价日志契约。
pub(crate) async fn fetch_lot_bid_log(
    auction_id: String,
    lot_id: String,
    page: i64,
    page_size: i64,
) -> Result<BidLogPage, String> {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("lot", &lot_id)
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", &page_size.to_string())
        .append_pair("include", BID_LOG_INCLUDE)
        .finish();
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{query}");
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, decoder::decode_bid_log_page).await
}
