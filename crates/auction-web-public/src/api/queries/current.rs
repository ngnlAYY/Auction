//! 世界列表、当前场次与按场次 ID 打开的历史场次读取。

use url::form_urlencoded;

use super::super::swr_cache;
use super::super::{
    AuctionListPage, CatalogLevelFilter, CurrentAuction, Worlds, decoder, send_get,
};
use super::cached_get::cached_public_get;
use super::directory::{SUMMARY_PAGE_SIZE, fetch_auction_page};
use super::lots::fetch_current_lots;
#[cfg(test)]
use crate::surface::PageReadFixture;

pub(crate) async fn fetch_worlds() -> Result<Worlds, String> {
    #[cfg(test)]
    if let Some(result) = PageReadFixture::response(()) {
        return result;
    }
    let response = send_get("/api/v1/worlds").await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_worlds(response).await
}

pub(crate) async fn fetch_current_auction(
    world_id: String,
) -> Result<Option<CurrentAuction>, String> {
    let url = current_auction_url(&world_id);
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, move |response| async move {
        Ok(decoder::decode_current_auction(response, &world_id).await?.auction)
    })
    .await
}

/// 当前场次的唯一 URL 构造点：始终用 `include=proxy_default_mode` 显式请求冻结策略的代理默认
/// 模式，解码器据此要求该键。服务端只对显式请求追加该键，旧页面的缺省请求仍得到原字段集合。
fn current_auction_url(world_id: &str) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("world", world_id).append_pair("include", "proxy_default_mode");
    format!("/api/v1/auctions/current?{}", query.finish())
}

/// 读取指定历史拍卖的公开投影。
/// 旧 v1 lot wire 不包含世界、场次号和计划结束时间；这些展示上下文必须从拍卖目录
/// 读取，不能通过扩张 lot 响应破坏旧客户端的严格字段闭集。
pub(crate) async fn fetch_historical_auction(auction_id: String) -> Result<CurrentAuction, String> {
    // 目录与看板默认目录共用同一 URL（page_size=20），命中 SWR 缓存，避免点击详情时
    // 额外触发一次 10 条目录请求并在返回时闪现加载态。按场次 ID 打开的页面绝大多数是已结算
    // 场次：已结算目录第一页与 lot 目录同时发出，省去“等状态再查摘要”的一轮串行往返；
    // 该 URL 是公开低频缓存档，状态不是 settled 时丢弃预取结果、按实际状态查找。
    let (envelope, settled_first_page) = futures_util::join!(
        fetch_current_lots(auction_id, None, None, CatalogLevelFilter::default(), 1, 20),
        fetch_auction_page(None, "settled".to_owned(), 1, SUMMARY_PAGE_SIZE),
    );
    let envelope = envelope?;
    let status = envelope.auction_status.clone();
    // 未结算的场次（含仍可出价的 live/final）是所在世界唯一的活动场次，从当前场次接口取得
    // 完整上下文，其中的冻结代理默认模式决定代理新建表单的初始选项；已结算场次不再竞价，
    // 只从往期目录拼装，没有该值。
    if status != "settled" {
        return fetch_matching_current_auction(&envelope.auction_id, &status).await;
    }
    let summary =
        fetch_auction_summary(&envelope.auction_id, status.clone(), Some(settled_first_page))
            .await?;
    Ok(CurrentAuction {
        id: envelope.auction_id,
        world: summary.world_id,
        number: summary.sequence,
        status,
        opens_at: envelope.opens_at,
        scheduled_ends_at: summary.effective_end_at,
        presentation_mode: envelope.presentation_mode,
        bidding_open: envelope.bidding_open,
        preview_revision: envelope.preview_revision,
        proxy_default_mode: None,
    })
}

/// 在指定状态的场次目录中查找摘要；`first_page` 是调用方已并行取得的第一页。
async fn fetch_auction_summary(
    auction_id: &str,
    status: String,
    mut first_page: Option<Result<AuctionListPage, String>>,
) -> Result<super::super::HistoricalAuction, String> {
    const PAGE_SIZE: i64 = SUMMARY_PAGE_SIZE;
    let mut page_number = 1_i64;
    loop {
        let page = match first_page.take() {
            Some(page) => page?,
            None => fetch_auction_page(None, status.clone(), page_number, PAGE_SIZE).await?,
        };
        let row_count =
            i64::try_from(page.auctions.len()).map_err(|_| "拍卖目录页过大".to_owned())?;
        let total = page.total;
        if let Some(auction) = page.auctions.into_iter().find(|auction| auction.id == auction_id) {
            return Ok(auction);
        }
        if row_count < PAGE_SIZE
            || total.is_some_and(|total| page_number.saturating_mul(PAGE_SIZE) >= total)
        {
            return Err("拍卖目录中不存在目标拍卖".to_owned());
        }
        page_number = page_number.checked_add(1).ok_or_else(|| "拍卖目录页码溢出".to_owned())?;
    }
}

async fn fetch_matching_current_auction(
    auction_id: &str,
    expected_status: &str,
) -> Result<CurrentAuction, String> {
    let worlds = fetch_worlds().await?;
    // 各世界的当前场次互不依赖，并行读取：任一世界命中即返回；都未命中时先报读取失败，
    // 不把读不到的世界当作“不存在目标拍卖”。
    let currents =
        futures_util::future::join_all(worlds.worlds.into_keys().map(fetch_current_auction)).await;
    let mut first_error = None;
    for current in currents {
        match current {
            Ok(Some(auction)) if is_target_current(&auction, auction_id, expected_status) => {
                return Ok(auction);
            }
            Ok(_) => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "当前拍卖目录中不存在目标拍卖".to_owned()))
}

/// 当前场次条目是否就是按 ID 打开的目标场次。lot 外壳与当前场次各有缓存 TTL，live→final
/// 切换前后两者可能短暂各持一边；同属可出价阶段时以当前场次条目为准，其余状态必须一致。
fn is_target_current(auction: &CurrentAuction, auction_id: &str, expected_status: &str) -> bool {
    auction.id == auction_id
        && (auction.status == expected_status
            || matches!(
                (auction.status.as_str(), expected_status),
                ("live" | "final", "live" | "final")
            ))
}

#[cfg(test)]
mod tests {
    use super::{current_auction_url, is_target_current};
    use crate::api::CurrentAuction;

    const AUCTION_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

    #[test]
    fn current_auction_url_always_requests_the_proxy_default_mode() {
        // 看板、外壳、标记页与按场次查找共用这一构造点；解码器要求的键必须由请求显式选择，
        // 不带参数的缺省响应没有该键。
        assert_eq!(
            current_auction_url("main"),
            "/api/v1/auctions/current?world=main&include=proxy_default_mode"
        );
        assert_eq!(
            current_auction_url("isekai"),
            "/api/v1/auctions/current?world=isekai&include=proxy_default_mode"
        );
    }

    fn current_with(status: &str) -> CurrentAuction {
        CurrentAuction {
            id: AUCTION_ID.to_owned(),
            world: "main".to_owned(),
            number: 12,
            status: status.to_owned(),
            opens_at: "2026-11-01T00:00:00Z".to_owned(),
            scheduled_ends_at: Some("2026-11-08T00:00:00Z".to_owned()),
            presentation_mode: "live".to_owned(),
            bidding_open: true,
            preview_revision: 1,
            proxy_default_mode: Some("fast".to_owned()),
        }
    }

    /// 按场次 ID 打开活动场次时，只容忍 lot 外壳与当前场次缓存之间的 live↔final 切换差；其他
    /// 状态差与不同场次都不算命中。
    #[test]
    fn target_current_tolerates_only_the_live_final_switch() {
        for (current, expected) in
            [("live", "live"), ("final", "final"), ("live", "final"), ("final", "live")]
        {
            assert!(
                is_target_current(&current_with(current), AUCTION_ID, expected),
                "{current} vs {expected}"
            );
        }
        for (current, expected) in [("preview", "live"), ("live", "preview"), ("final", "settling")]
        {
            assert!(
                !is_target_current(&current_with(current), AUCTION_ID, expected),
                "{current} vs {expected}"
            );
        }
        assert!(is_target_current(&current_with("preview"), AUCTION_ID, "preview"));
        let other = "0190f2b4-1c2d-4e5f-8a6b-000000000000";
        assert!(!is_target_current(&current_with("live"), other, "live"));
    }
}
