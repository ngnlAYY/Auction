//! Public 拍卖、lot、竞价日志和价格历史查询。

use super::{
    ApiResponse, AuctionListPage, BidLogPage, CurrentAuction, LotEnvelope, PriceHistoryPage,
    PriceHistoryQuery, Worlds, send_get,
};
use auction_contracts::CurrentAuctionResponse;
use url::form_urlencoded;

pub(crate) async fn fetch_worlds() -> Result<Worlds, String> {
    let response = send_get("/api/v1/worlds").await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<Worlds> = response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_current_auction(
    world_id: String,
) -> Result<Option<CurrentAuction>, String> {
    let world_query = if world_id == "main" {
        String::new()
    } else {
        let encoded = form_urlencoded::Serializer::new(String::new())
            .append_pair("world", &world_id)
            .finish();
        format!("?{encoded}")
    };
    let url = format!("/api/v1/auctions/current{world_query}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<CurrentAuctionResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.auction)
}

/// 读取指定历史拍卖的公开投影；lot 端点同时提供展示所需的拍卖上下文。
pub(crate) async fn fetch_historical_auction(auction_id: String) -> Result<CurrentAuction, String> {
    let envelope = fetch_current_lots(auction_id, None, 1).await?;
    Ok(CurrentAuction {
        id: envelope.auction_id,
        world: envelope.world,
        number: envelope.number,
        status: envelope.auction_status,
        opens_at: envelope.opens_at,
        scheduled_ends_at: envelope.scheduled_ends_at,
        presentation_mode: envelope.presentation_mode,
        bidding_open: envelope.bidding_open,
        preview_revision: envelope.preview_revision,
    })
}

pub(crate) async fn fetch_bid_log_page(
    auction_id: String,
    page: i64,
    world_id: String,
    lot_id: Option<String>,
) -> Result<BidLogPage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string()).append_pair("page_size", "50");
    if !world_id.is_empty() {
        query.append_pair("world", &world_id);
    }
    if let Some(lot_id) = lot_id {
        query.append_pair("lot", &lot_id);
    }
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<BidLogPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

/// 读取可供公开竞价日志选择的拍卖；旧版只展示 live、final 和 settled 拍卖。
pub(crate) async fn fetch_bid_log_auctions(
    world_id: String,
) -> Result<Vec<super::HistoricalAuction>, String> {
    let mut auctions = Vec::new();
    for status in ["live", "final", "settled"] {
        let page = fetch_auction_page(Some(world_id.clone()), status.to_owned(), 1, 200).await?;
        auctions.extend(page.auctions);
    }
    auctions.sort_by(|left, right| {
        right.sequence.cmp(&left.sequence).then_with(|| right.id.cmp(&left.id))
    });
    auctions.dedup_by(|left, right| left.id == right.id);
    Ok(auctions)
}

/// 读取单个 lot 的编号竞价历史；分页参数沿用公开竞价日志契约。
pub(crate) async fn fetch_lot_bid_log(
    auction_id: String,
    lot_id: String,
    page: i64,
) -> Result<BidLogPage, String> {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("lot", &lot_id)
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", "20")
        .finish();
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{query}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<BidLogPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_auction_page(
    world_id: Option<String>,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AuctionListPage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.to_string())
        .append_pair("page_size", &page_size.to_string())
        .append_pair("status", &status);
    if let Some(world_id) = world_id.filter(|world| !world.is_empty() && world != "all") {
        query.append_pair("world", &world_id);
    }
    let url = format!("/api/v1/auctions?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AuctionListPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_auction_list() -> Result<Vec<super::HistoricalAuction>, String> {
    Ok(fetch_auction_page(None, "settled".to_owned(), 1, 200).await?.auctions)
}

pub(crate) async fn fetch_price_history(
    query: PriceHistoryQuery,
) -> Result<PriceHistoryPage, String> {
    let mut params = form_urlencoded::Serializer::new(String::new());
    for (key, value) in [
        ("world", query.world.as_deref()),
        ("name", query.name.as_deref()),
        ("from", query.from.as_deref()),
        ("to", query.to.as_deref()),
        ("min_price", query.min_price.as_deref()),
        ("max_price", query.max_price.as_deref()),
        ("buyer", query.buyer.as_deref()),
        ("seller", query.seller.as_deref()),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            params.append_pair(key, value);
        }
    }
    params.append_pair("page", &query.page.max(1).to_string()).append_pair("page_size", "50");
    let url = format!("/api/v1/price-history?{}", params.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<PriceHistoryPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_current_lots(
    auction_id: String,
    category: Option<String>,
    page: i64,
) -> Result<LotEnvelope, String> {
    let category_query = category
        .map(|value| {
            let encoded = form_urlencoded::Serializer::new(String::new())
                .append_pair("category", &value)
                .finish();
            format!("&{encoded}")
        })
        .unwrap_or_default();
    let url =
        format!("/api/v1/auctions/{auction_id}/lots?page={page}&page_size=20{category_query}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<LotEnvelope> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_lot_detail(lot_id: String) -> Result<LotEnvelope, String> {
    let url = format!("/api/v1/lots/{lot_id}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<LotEnvelope> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
