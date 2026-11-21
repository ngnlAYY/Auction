//! Public 履约进度的 HTTP DTO 与请求：已结算场次目录与逐件履约状态页。

pub(crate) use auction_contracts::{
    PublicFulfillmentLotResponse as FulfillmentLot,
    PublicFulfillmentPageResponse as FulfillmentPage,
};
use auction_time::{SecondsFormat, Timestamp};
use url::form_urlencoded;
use uuid::Uuid;

use super::queries::cached_get::cached_public_read;
use super::queries::fetch_bid_log_auctions;
use super::{HistoricalAuction, decoder, swr_cache};

/// 履约进度接口的每页数量上限；超过时服务端返回 400。页面按上限请求，自定义输入超出时就地提示。
pub(crate) const MAX_FULFILLMENT_PAGE_SIZE: i64 = 100;

/// 一次履约进度读取：编号页，以及它是否为暂时性失败后回退的旧快照。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FulfillmentRead {
    pub(crate) page: FulfillmentPage,
    /// 重读遇到网络失败或服务端暂时拒绝、展示的是旧快照时，给出该快照最近一次被服务端确认的
    /// 时刻（RFC 3339 UTC，秒精度），页面据此标明数据时点，不静默显示旧状态；权威结果为 `None`。
    pub(crate) stale_since: Option<String>,
}

/// 读取一个世界的已结算场次目录页；只有已结算场次才有履约进度。
pub(crate) async fn fetch_fulfillment_auctions(
    world_id: String,
    page: i64,
) -> Result<(Vec<HistoricalAuction>, bool), String> {
    fetch_bid_log_auctions(world_id, "settled", page).await
}

/// 读取单场的履约进度编号页；响应必须与请求的场次、世界与分页一致，否则按无效响应处理。
pub(crate) async fn fetch_fulfillment_page(
    auction_id: String,
    world_id: String,
    lot: Option<String>,
    page: i64,
    page_size: i64,
) -> Result<FulfillmentRead, String> {
    let auction_id =
        Uuid::try_parse(&auction_id).map_err(|_| "invalid_request".to_owned())?.to_string();
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
    let read =
        cached_public_read(&url, swr_cache::CATALOG_TTL_MILLIS, move |response| async move {
            decoder::decode_fulfillment_page(response, &auction_id, &world_id, page, page_size)
                .await
        })
        .await?;
    // 确认时刻无法表示时不展示旧快照，按读取失败处理：页面显示错误与重试，不静默显示旧状态。
    let stale_since = read
        .stale_since()
        .map(|confirmed_at| {
            confirmation_time(confirmed_at).ok_or_else(|| "invalid_response".to_owned())
        })
        .transpose()?;
    Ok(FulfillmentRead { page: read.value, stale_since })
}

/// 把缓存层记录的确认时刻（浏览器墙钟毫秒）写成 RFC 3339 UTC，交给页面统一的时间格式显示。
fn confirmation_time(millis: u64) -> Option<String> {
    let millis = i64::try_from(millis).ok()?;
    Timestamp::from_timestamp_millis(millis)
        .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::confirmation_time;

    #[test]
    fn confirmation_time_is_whole_seconds_in_utc() {
        assert_eq!(confirmation_time(1_763_200_000_750).as_deref(), Some("2025-11-15T09:46:40Z"));
        assert_eq!(confirmation_time(0).as_deref(), Some("1970-01-01T00:00:00Z"));
        // 超出可表示范围时不猜测时刻，由调用方按读取失败处理。
        assert_eq!(confirmation_time(u64::MAX), None);
        assert_eq!(confirmation_time(i64::MAX as u64), None);
    }
}
