//! 公开竞价日志读取，以及竞价日志与履约进度共用的按状态场次目录遍历。

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
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, |response| async move {
        decoder::decode_bid_log_page(response).await
    })
    .await
}

/// 读取可供公开竞价日志选择的 live、final 和 settled 拍卖，保留已结算场次的历史深链。
pub(crate) async fn fetch_bid_log_auctions(
    world_id: String,
) -> Result<Vec<super::super::HistoricalAuction>, String> {
    // 状态目录相互独立，最多并发三个读取；各状态内部仍按编号顺序逐页读取。
    let (mut auctions, final_auctions, settled_auctions) = futures_util::try_join!(
        fetch_bid_log_auction_status(&world_id, "live"),
        fetch_bid_log_auction_status(&world_id, "final"),
        fetch_bid_log_auction_status(&world_id, "settled"),
    )?;
    auctions.extend(final_auctions);
    auctions.extend(settled_auctions);
    auctions.sort_by(|left, right| {
        right.sequence.cmp(&left.sequence).then_with(|| right.id.cmp(&left.id))
    });
    auctions.dedup_by(|left, right| left.id == right.id);
    Ok(auctions)
}

/// 按编号顺序逐页读完单一状态的公开拍卖目录；竞价日志与履约进度的场次选择共用。
pub(in crate::api) async fn fetch_bid_log_auction_status(
    world_id: &str,
    status: &str,
) -> Result<Vec<super::super::HistoricalAuction>, String> {
    let mut auctions = Vec::new();
    let mut page_number = 1_i64;
    loop {
        let world = Some(world_id.to_owned());
        let page =
            fetch_auction_page(world, status.to_owned(), page_number, SUMMARY_PAGE_SIZE).await?;
        let row_count =
            i64::try_from(page.auctions.len()).map_err(|_| "拍卖目录页过大".to_owned())?;
        let page_size = page.page_size;
        if page_size <= 0 {
            return Err("拍卖目录分页大小无效".to_owned());
        }
        let total = page.total;
        auctions.extend(page.auctions);
        // 满页且缺 total 时继续读取，避免选择器静默截断到第一页。
        if row_count < page_size
            || total.is_some_and(|total| page_number.saturating_mul(page_size) >= total)
        {
            return Ok(auctions);
        }
        page_number = page_number.checked_add(1).ok_or_else(|| "拍卖目录页码溢出".to_owned())?;
    }
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
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, |response| async move {
        decoder::decode_bid_log_page(response).await
    })
    .await
}
