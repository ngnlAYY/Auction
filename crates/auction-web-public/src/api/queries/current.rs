//! 世界列表、当前场次与按场次 ID 打开的历史场次读取。

use std::future::ready;

use futures_util::future::{Either, select};
use futures_util::pin_mut;
use futures_util::stream::{FuturesUnordered, StreamExt};
use gloo_net::http::Request;
use url::form_urlencoded;

use super::super::swr_cache;
use super::super::{
    AuctionListPage, CatalogLevelFilter, CurrentAuction, TransportError, Worlds, decoder, send_get,
    send_request,
};
use super::cached_get::cached_public_get;
use super::directory::{SUMMARY_PAGE_SIZE, fetch_auction_page, revalidate_recent_settled};
use super::lots::fetch_current_lots;
#[cfg(test)]
use crate::surface::PageReadFixture;

/// 默认读取沿用浏览器缓存。
pub(crate) async fn fetch_worlds() -> Result<Worlds, String> {
    #[cfg(test)]
    if let Some(result) = PageReadFixture::response(()) {
        return result;
    }
    read_worlds(false).await
}

/// 低频目录允许缓存返回旧快照，周期再验证须等待当次响应。
pub(crate) async fn revalidate_worlds() -> Result<Worlds, String> {
    read_worlds(true).await
}

async fn read_worlds(revalidate: bool) -> Result<Worlds, String> {
    const PATH: &str = "/api/v1/worlds";
    let response = if revalidate {
        send_request(|abort_signal| {
            Request::get(PATH)
                .cache(web_sys::RequestCache::NoCache)
                .abort_signal(abort_signal)
                .build()
                .map_err(|_error| TransportError::Network(None))
        })
        .await
    } else {
        send_get(PATH).await
    }
    .map_err(|error| error.code().to_owned())?;
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
    let lots = fetch_current_lots(auction_id, None, None, CatalogLevelFilter::default(), 1, 20);
    let directory = fetch_auction_page(None, "settled".to_owned(), 1, SUMMARY_PAGE_SIZE);
    pin_mut!(lots, directory);
    let (envelope, settled_first_page) = match select(lots, directory).await {
        Either::Left((result, directory)) => (result?, Either::Left(directory)),
        Either::Right((result, lots)) => (lots.await?, Either::Right(ready(result))),
    };
    let status = envelope.auction_status.clone();
    // 未结算的场次（含仍可出价的 live/final）是所在世界唯一的活动场次，从当前场次接口取得
    // 完整上下文，其中的冻结代理默认模式决定代理新建表单的初始选项；已结算场次不再竞价，
    // 只从往期目录拼装，没有该值。
    if status != "settled" {
        return fetch_matching_current_auction(&envelope.auction_id, &status).await;
    }
    let summary =
        match fetch_auction_summary(&envelope.auction_id, settled_first_page.await?).await? {
            Some(summary) => summary,
            // lots 与目录有独立的新鲜期。缺席只允许一次近期目录再验证，不能据旧列表猜场次身份。
            None => revalidate_recent_settled()
                .await?
                .auctions
                .into_iter()
                .find(|auction| auction.id == envelope.auction_id)
                .ok_or_else(|| "拍卖目录中不存在目标拍卖".to_owned())?,
        };
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

/// 在结算场次目录中查找摘要；`first_page` 是调用方已并行取得的第一页。
async fn fetch_auction_summary(
    auction_id: &str,
    first_page: AuctionListPage,
) -> Result<Option<super::super::HistoricalAuction>, String> {
    const PAGE_SIZE: i64 = SUMMARY_PAGE_SIZE;
    let mut page = first_page;
    let mut page_number = 1_i64;
    loop {
        let row_count =
            i64::try_from(page.auctions.len()).map_err(|_| "拍卖目录页过大".to_owned())?;
        let total = page.total;
        if let Some(auction) = page.auctions.into_iter().find(|auction| auction.id == auction_id) {
            return Ok(Some(auction));
        }
        if row_count < PAGE_SIZE
            || total.is_some_and(|total| page_number.saturating_mul(PAGE_SIZE) >= total)
        {
            return Ok(None);
        }
        page_number = page_number.checked_add(1).ok_or_else(|| "拍卖目录页码溢出".to_owned())?;
        page = fetch_auction_page(None, "settled".to_owned(), page_number, PAGE_SIZE).await?;
    }
}

async fn fetch_matching_current_auction(
    auction_id: &str,
    expected_status: &str,
) -> Result<CurrentAuction, String> {
    let worlds = fetch_worlds().await?;
    // 各世界的当前场次互不依赖，并行读取：任一世界命中即返回；都未命中时先报读取失败，
    // 不把读不到的世界当作“不存在目标拍卖”。
    let mut currents: FuturesUnordered<_> =
        worlds.worlds.into_keys().map(fetch_current_auction).collect();
    let mut first_error = None;
    while let Some(current) = currents.next().await {
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
    use std::cell::RefCell;
    use std::future::{Future, pending, poll_fn, ready};
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::Poll;

    use dioxus::prelude::*;
    use futures_util::FutureExt;

    use super::{current_auction_url, fetch_historical_auction, is_target_current};
    use crate::api::clock::ConditionalGetFailure;
    use crate::api::swr_cache::{self, FetchOutcome};
    use crate::api::{
        AuctionListPage, CurrentAuction, HistoricalAuction, LotEnvelope, TransportError,
        WorldStatus, Worlds,
    };
    use crate::surface::PageReadFixture;

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

    fn lots(status: &str) -> LotEnvelope {
        LotEnvelope {
            auction_id: AUCTION_ID.to_owned(),
            auction_status: status.to_owned(),
            projection_kind: "live".to_owned(),
            presentation_mode: "live".to_owned(),
            bidding_open: status == "live",
            opens_at: "2026-11-01T00:00:00Z".to_owned(),
            preview_revision: 0,
            final_restriction: None,
            lots: None,
            lot: None,
            total: Some(0),
            page: Some(1),
            page_size: Some(20),
            category_counts: None,
        }
    }

    type Reply<T> = Pin<Box<dyn Future<Output = Result<T, String>>>>;

    #[test]
    fn a_matched_active_round_leaves_slow_settled_and_other_world_reads_behind() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let directory = PageReadFixture::new({
            let requests = requests.clone();
            move |url: String| -> Reply<AuctionListPage> {
                requests.borrow_mut().push(url);
                Box::pin(pending())
            }
        });
        let current = PageReadFixture::new({
            let requests = requests.clone();
            move |url: String| -> Reply<Option<CurrentAuction>> {
                let matched = url.contains("world=main");
                requests.borrow_mut().push(url);
                if matched {
                    Box::pin(ready(Ok(Some(current_with("live")))))
                } else {
                    Box::pin(pending())
                }
            }
        });
        let lots = PageReadFixture::new({
            let requests = requests.clone();
            move |url: String| -> Reply<LotEnvelope> {
                requests.borrow_mut().push(url);
                let mut first_poll = true;
                Box::pin(poll_fn(move |context| {
                    if std::mem::take(&mut first_poll) {
                        context.waker().wake_by_ref();
                        Poll::Pending
                    } else {
                        Poll::Ready(Ok(lots("live")))
                    }
                }))
            }
        });
        let mut dom = VirtualDom::new(|| rsx! {})
            .with_root_context(directory)
            .with_root_context(current)
            .with_root_context(lots)
            .with_root_context(worlds_fixture());
        dom.rebuild_in_place();
        let result = dom.in_scope(ScopeId::APP, || {
            let mut find = Box::pin(fetch_historical_auction(AUCTION_ID.to_owned()));
            assert!(find.as_mut().now_or_never().is_none());
            find.now_or_never().expect("活动场次命中后不能等另两个查询").unwrap()
        });
        assert_eq!(result, current_with("live"));
        let requests = requests.borrow();
        assert!(requests.iter().any(|url| url.contains("world=isekai")));
        assert!(requests.iter().any(|url| url.contains("status=settled")));
    }

    fn worlds_fixture() -> PageReadFixture<(), Result<Worlds, String>> {
        PageReadFixture::new(|()| {
            Ok(Worlds {
                worlds: ["main", "isekai"]
                    .into_iter()
                    .map(|world| (world.to_owned(), WorldStatus { enabled: true }))
                    .collect(),
            })
        })
    }

    #[test]
    fn an_active_round_miss_preserves_a_world_read_failure() {
        let mut dom = VirtualDom::new(|| rsx! {})
            .with_root_context(PageReadFixture::new(|_url: String| Ok::<_, String>(lots("live"))))
            .with_root_context(PageReadFixture::new(|url: String| {
                if url.contains("world=main") {
                    Ok(None::<CurrentAuction>)
                } else {
                    Err("request_timeout".to_owned())
                }
            }))
            .with_root_context(worlds_fixture());
        dom.rebuild_in_place();
        assert_eq!(
            dom.in_scope(ScopeId::APP, || {
                fetch_historical_auction(AUCTION_ID.to_owned()).now_or_never().unwrap()
            }),
            Err("request_timeout".to_owned())
        );
    }

    fn settled_summary() -> HistoricalAuction {
        HistoricalAuction {
            id: AUCTION_ID.to_owned(),
            world_id: "isekai".to_owned(),
            sequence: 18,
            status: "settled".to_owned(),
            scheduled_open_at: "2026-11-01T00:00:00Z".to_owned(),
            opened_at: None,
            final_phase_at: None,
            effective_end_at: None,
            settled_at: None,
            lot_count: 0,
            sold_count: 0,
            unsold_count: 0,
            sold_amount: 0,
            state_version: 1,
        }
    }

    #[test]
    fn a_settled_round_missing_from_the_first_directory_gets_one_revalidation() {
        for found_on in [Some(1), Some(2), None] {
            let requests = Rc::new(RefCell::new(Vec::new()));
            let directory = PageReadFixture::new({
                let requests = requests.clone();
                move |url: String| {
                    let mut requests = requests.borrow_mut();
                    requests.push(url);
                    let auctions = (Some(requests.len()) == found_on)
                        .then(settled_summary)
                        .into_iter()
                        .collect::<Vec<_>>();
                    Ok::<_, String>(AuctionListPage {
                        page: Some(1),
                        page_size: 200,
                        total: Some(auctions.len() as i64),
                        auctions,
                        cursor: None,
                        next_cursor: None,
                        has_more: None,
                    })
                }
            });
            let mut dom =
                VirtualDom::new(|| rsx! {}).with_root_context(directory).with_root_context(
                    PageReadFixture::new(|_url: String| Ok::<_, String>(lots("settled"))),
                );
            dom.rebuild_in_place();
            let result = dom.in_scope(ScopeId::APP, || {
                fetch_historical_auction(AUCTION_ID.to_owned()).now_or_never().unwrap()
            });
            if found_on.is_some() {
                let auction = result.unwrap();
                assert_eq!(
                    (auction.id.as_str(), auction.world.as_str(), auction.number),
                    (AUCTION_ID, "isekai", 18)
                );
            } else {
                assert!(result.is_err());
            }
            assert_eq!(
                *requests.borrow(),
                vec![
                    "/api/v1/auctions?page=1&page_size=200&status=settled".to_owned();
                    if found_on == Some(1) { 1 } else { 2 }
                ]
            );
        }
    }

    #[test]
    fn settled_round_revalidation_sends_a_conditional_get_against_a_fresh_directory() {
        const URL: &str = "/api/v1/auctions?page=1&page_size=200&status=settled";
        for cached_target in [true, false] {
            let auctions: Vec<_> = cached_target.then(settled_summary).into_iter().collect();
            let old_directory = AuctionListPage {
                page: Some(1),
                page_size: 200,
                total: Some(auctions.len() as i64),
                auctions,
                cursor: None,
                next_cursor: None,
                has_more: None,
            };
            swr_cache::get_or_fetch(URL, 0, swr_cache::now_millis, |_| {
                ready(FetchOutcome::<_, String>::Changed {
                    value: old_directory,
                    etag: Some("\"settled-old\"".to_owned()),
                })
            })
            .now_or_never()
            .unwrap()
            .unwrap();
            let requests = Rc::new(RefCell::new(Vec::new()));
            let wire = PageReadFixture::new({
                let requests = requests.clone();
                move |request: (String, Option<String>)| {
                    requests.borrow_mut().push(request);
                    ConditionalGetFailure {
                        status: Some(503),
                        retry_after: None,
                        date: None,
                        read_body: None,
                        transport: TransportError::Timeout,
                    }
                }
            });
            let mut dom = VirtualDom::new(|| rsx! {}).with_root_context(wire).with_root_context(
                PageReadFixture::new(|_url: String| Ok::<_, String>(lots("settled"))),
            );
            dom.rebuild_in_place();
            let result = dom.in_scope(ScopeId::APP, || {
                fetch_historical_auction(AUCTION_ID.to_owned()).now_or_never().unwrap()
            });
            if cached_target {
                assert_eq!(result.unwrap().id, AUCTION_ID);
                assert!(requests.borrow().is_empty());
            } else {
                assert_eq!(result.unwrap_err(), "拍卖目录中不存在目标拍卖");
                assert_eq!(
                    *requests.borrow(),
                    [(URL.to_owned(), Some("\"settled-old\"".to_owned()))]
                );
            }
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
