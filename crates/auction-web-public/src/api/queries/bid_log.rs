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
    Ok(order_bid_log_auctions(auctions))
}

fn order_bid_log_auctions(
    mut auctions: Vec<super::super::HistoricalAuction>,
) -> Vec<super::super::HistoricalAuction> {
    // 各状态独立读取，场次在查询之间迁移时只采用已确认的最高版本。
    auctions.sort_by(|left, right| {
        left.id.cmp(&right.id).then_with(|| right.state_version.cmp(&left.state_version))
    });
    auctions.dedup_by(|left, right| left.id == right.id);
    auctions.sort_by(|left, right| {
        bid_log_world_rank(&left.world_id)
            .cmp(&bid_log_world_rank(&right.world_id))
            .then_with(|| right.sequence.cmp(&left.sequence))
            .then_with(|| right.id.cmp(&left.id))
    });
    auctions
}

fn bid_log_world_rank(world_id: &str) -> u8 {
    match world_id {
        "main" => 0,
        "isekai" => 1,
        _ => 2,
    }
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

#[cfg(test)]
mod tests {
    use super::order_bid_log_auctions;
    use crate::api::HistoricalAuction;

    fn auction(
        id: &str,
        world_id: &str,
        sequence: i64,
        status: &str,
        version: i64,
    ) -> HistoricalAuction {
        HistoricalAuction {
            id: id.to_owned(),
            world_id: world_id.to_owned(),
            sequence,
            status: status.to_owned(),
            scheduled_open_at: "2026-11-14T12:00:00Z".to_owned(),
            opened_at: None,
            final_phase_at: None,
            effective_end_at: None,
            settled_at: None,
            lot_count: 0,
            sold_count: 0,
            unsold_count: 0,
            sold_amount: 0,
            state_version: version,
        }
    }

    #[test]
    fn bid_log_directory_keeps_the_highest_version_in_either_input_order() {
        let live = auction("same", "main", 8, "live", 2);
        let final_phase = auction("same", "main", 8, "final", 3);
        for input in [vec![live.clone(), final_phase.clone()], vec![final_phase.clone(), live]] {
            assert_eq!(
                order_bid_log_auctions(input).as_slice(),
                std::slice::from_ref(&final_phase)
            );
        }
    }

    #[test]
    fn bid_log_directory_orders_worlds_then_descending_sequences() {
        let main_older = auction("main-older", "main", 7, "settled", 9);
        let main_newer = auction("main-newer", "main", 8, "live", 2);
        let isekai = auction("isekai", "isekai", 9, "live", 2);
        assert_eq!(
            order_bid_log_auctions(vec![isekai.clone(), main_older.clone(), main_newer.clone()]),
            [main_newer, main_older, isekai],
        );
    }
}
