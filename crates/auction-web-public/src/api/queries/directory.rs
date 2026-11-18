//! 场次目录分页读取：目录 URL 构造、响应与请求身份绑定，以及内置摘要目录的读取页长。

use url::form_urlencoded;

use super::super::swr_cache;
use super::super::{AuctionListPage, decoder};
use super::cached_get::cached_public_get;

/// 场次摘要目录的内置读取页长（Public 页长上限）；与并行预取的第一页同 URL，命中同一缓存条目。
pub(super) const SUMMARY_PAGE_SIZE: i64 = auction_contracts::MAX_PUBLIC_PAGE_SIZE;

pub(super) async fn fetch_auction_page(
    world_id: Option<String>,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AuctionListPage, String> {
    fetch_auction_page_with_ttl(world_id, status, page, page_size, swr_cache::CATALOG_TTL_MILLIS)
        .await
}

/// lot 已结算但目录未见时，跳过浏览器端 SWR 新鲜期再核对近期结算场次。
pub(super) async fn revalidate_recent_settled() -> Result<AuctionListPage, String> {
    fetch_auction_page_with_ttl(None, "settled".to_owned(), 1, SUMMARY_PAGE_SIZE, 0).await
}

async fn fetch_auction_page_with_ttl(
    world_id: Option<String>,
    status: String,
    page: i64,
    page_size: i64,
    ttl_millis: u64,
) -> Result<AuctionListPage, String> {
    let world =
        world_id.as_deref().filter(|world| !world.is_empty() && *world != "all").map(str::to_owned);
    let url = auction_page_url("/api/v1/auctions", world.as_deref(), &status, page, page_size);
    cached_public_get(&url, ttl_millis, move |response| async move {
        let decoded = decoder::decode_auction_list(response, world.as_deref(), &status).await?;
        bind_auction_page(decoded, page, page_size)
    })
    .await
}

/// 历史页用 v2 一次取得本页摘要与论坛地址，不按行加载完整论坛镜像。
pub(crate) async fn fetch_auction_history_page(
    world_id: Option<String>,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AuctionListPage<super::super::HistoricalAuctionWithForum>, String> {
    let world =
        world_id.as_deref().filter(|world| !world.is_empty() && *world != "all").map(str::to_owned);
    let url = auction_page_url("/api/v2/auctions", world.as_deref(), &status, page, page_size);
    cached_public_get(&url, swr_cache::CATALOG_TTL_MILLIS, move |response| async move {
        let decoded = decoder::decode_auction_list_v2(response, world.as_deref(), &status).await?;
        bind_auction_page(decoded, page, page_size)
    })
    .await
}

/// 页面和跨页查找共用请求身份校验；先完成严格解码，不能把错页响应拼进目录。
fn bind_auction_page<T>(
    response: AuctionListPage<T>,
    page: i64,
    page_size: i64,
) -> Result<AuctionListPage<T>, String> {
    if response.page != Some(page) || response.page_size != page_size {
        return Err("invalid_response".to_owned());
    }
    Ok(response)
}

fn auction_page_url(
    endpoint: &str,
    world: Option<&str>,
    status: &str,
    page: i64,
    page_size: i64,
) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.to_string())
        .append_pair("page_size", &page_size.to_string())
        .append_pair("status", status);
    if let Some(world) = world {
        query.append_pair("world", world);
    }
    format!("{endpoint}?{}", query.finish())
}

/// 预览切换只消费同一世界的近期结算场次，避免被其他世界的场次挤出第一页目录。
pub(crate) async fn fetch_auction_list(
    world_id: String,
) -> Result<Vec<super::super::HistoricalAuction>, String> {
    let page = fetch_auction_page(Some(world_id), "settled".to_owned(), 1, SUMMARY_PAGE_SIZE);
    Ok(page.await?.auctions)
}

#[cfg(test)]
mod tests {
    use auction_contracts::MAX_PUBLIC_PAGE_SIZE;

    use super::SUMMARY_PAGE_SIZE;

    /// 内置场次目录遍历按 Public 页长上限逐页读取，后端接受这个页长（见 HTTP 路由测试）。
    #[test]
    fn summary_directory_reads_use_the_public_page_size_maximum() {
        assert_eq!(SUMMARY_PAGE_SIZE, MAX_PUBLIC_PAGE_SIZE);
        assert_eq!(SUMMARY_PAGE_SIZE, 200);
    }
}
