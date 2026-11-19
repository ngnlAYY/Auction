//! 世界列表、当前场次与按场次 ID 打开的历史场次读取。

use std::future::ready;

use auction_contracts::SettledAuctionSummaryResponse;
use futures_util::future::{Either, select};
use futures_util::pin_mut;
use futures_util::stream::{FuturesUnordered, StreamExt};
use gloo_net::http::Request;
use url::form_urlencoded;

use super::super::clock::send_response_observed;
#[cfg(test)]
use super::super::clock::{HttpFixture, HttpFixtureFuture};
use super::super::swr_cache;
use super::super::{
    CatalogLevelFilter, CurrentAuction, TransportError, Worlds, decoder, send_get, send_request,
};
use super::cached_get::cached_public_get;
use super::directory::{SUMMARY_PAGE_SIZE, fetch_auction_page};
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

/// 读取指定历史拍卖的公开投影；从目录或已结算摘要取得 lots 未提供的展示上下文。
pub(crate) async fn fetch_historical_auction(auction_id: String) -> Result<CurrentAuction, String> {
    // 复用结算目录第一页的缓存键，与 lots 并发读取，使目录命中时省去一次串行往返。
    let lots = fetch_current_lots(auction_id, None, None, CatalogLevelFilter::default(), 1, 20);
    let directory = fetch_auction_page(None, "settled".to_owned(), 1, SUMMARY_PAGE_SIZE);
    pin_mut!(lots, directory);
    let (envelope, settled_first_page) = match select(lots, directory).await {
        Either::Left((result, directory)) => (result?, Either::Left(directory)),
        Either::Right((result, lots)) => (lots.await?, Either::Right(ready(result))),
    };
    let status = envelope.auction_status.clone();
    // 未结算的场次（含仍可出价的 live/final）是所在世界唯一的活动场次，从当前场次接口取得
    // 完整上下文，其中的冻结代理默认模式决定代理新建表单的初始选项；已结算场次没有该值。
    if status != "settled" {
        return fetch_matching_current_auction(&envelope.auction_id, &status).await;
    }
    let first_page = settled_first_page.await?;
    let (world, number, scheduled_ends_at) =
        match first_page.auctions.into_iter().find(|auction| auction.id == envelope.auction_id) {
            Some(auction) => (auction.world_id, auction.sequence, auction.effective_end_at),
            None => {
                // lots 与目录有独立的新鲜期；目录缺席不能证明已结算场次不存在。
                let summary = fetch_settled_auction_summary(&envelope.auction_id).await?;
                (summary.world, summary.sequence, summary.effective_end_at)
            }
        };
    Ok(CurrentAuction {
        id: envelope.auction_id,
        world,
        number,
        status,
        opens_at: envelope.opens_at,
        scheduled_ends_at,
        presentation_mode: envelope.presentation_mode,
        bidding_open: envelope.bidding_open,
        preview_revision: envelope.preview_revision,
        proxy_default_mode: None,
    })
}

async fn fetch_settled_auction_summary(
    auction_id: &str,
) -> Result<SettledAuctionSummaryResponse, String> {
    let url = format!("/api/v1/auctions/{auction_id}/summary");
    // 成功摘要沿用 HTTP 缓存。只有收到 404 响应头才能确认缺席，其他失败仍可重试。
    let mut missing = false;
    let response = send_response_observed(
        |abort_signal| {
            Request::get(&url)
                .abort_signal(abort_signal)
                .build()
                .map_err(|_error| TransportError::Network(None))
        },
        #[cfg(test)]
        PageReadFixture::<String, HttpFixtureFuture>::response(url.clone())
            .map(HttpFixture::Buffered),
        |head| {
            head.observe_date();
            missing = head.status() == 404;
        },
    )
    .await;
    if missing {
        return Err("拍卖目录中不存在目标拍卖".to_owned());
    }
    let response = response.map_err(|error| error.code().to_owned())?;
    let status = response.status();
    let body = response.text().await;
    if status == 200 {
        return decoder::decode_settled_auction_summary(body.as_deref(), auction_id);
    }
    let error = body.as_deref().and_then(decoder::decode_error_body).map(|(code, _)| code);
    Err(error.filter(|code| code != "not_found").unwrap_or_else(|| "invalid_response".to_owned()))
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
    use std::collections::VecDeque;
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
        AuctionListPage, CurrentAuction, HistoricalAuction, HttpFixtureFuture, HttpResponseFixture,
        LotEnvelope, WorldStatus, Worlds,
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
            .with_root_context(summary_fixture(&requests, []))
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
        assert_eq!(requests.iter().filter(|url| url.as_str() == SUMMARY_URL).count(), 0);
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

    const DIRECTORY_URL: &str = "/api/v1/auctions?page=1&page_size=200&status=settled";
    const SUMMARY_URL: &str = "/api/v1/auctions/0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b/summary";
    const SUMMARY_BODY: &str =
        include_str!("../../../../auction-contracts/tests/fixtures/public/auction-summary.json");

    fn first_page(auctions: Vec<HistoricalAuction>) -> AuctionListPage {
        let count = auctions.len() as i64;
        AuctionListPage {
            page: Some(1),
            page_size: 200,
            total: Some(count + i64::from(count == 200)),
            auctions,
            cursor: None,
            next_cursor: None,
            has_more: None,
        }
    }

    fn summary_fixture(
        requests: &Rc<RefCell<Vec<String>>>,
        responses: impl IntoIterator<Item = (u16, Option<&'static str>)>,
    ) -> PageReadFixture<String, HttpFixtureFuture> {
        let requests = requests.clone();
        let responses = RefCell::new(responses.into_iter().collect::<VecDeque<_>>());
        PageReadFixture::new(move |url| -> HttpFixtureFuture {
            requests.borrow_mut().push(url);
            let (status, body) = responses.borrow_mut().pop_front().expect("多余的摘要请求");
            Box::pin(ready(Ok(HttpResponseFixture {
                status,
                etag: None,
                body: body.map(str::to_owned),
            })))
        })
    }

    fn read_historical(dom: &VirtualDom) -> Result<CurrentAuction, String> {
        dom.in_scope(ScopeId::APP, || {
            fetch_historical_auction(AUCTION_ID.to_owned()).now_or_never().unwrap()
        })
    }

    #[test]
    fn a_settled_round_missing_from_a_full_first_page_uses_only_its_summary() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let directory = PageReadFixture::new({
            let requests = requests.clone();
            move |url: String| {
                requests.borrow_mut().push(url.clone());
                let auctions = if url == DIRECTORY_URL {
                    vec![
                        HistoricalAuction {
                            id: "0190f2b4-1c2d-4e5f-8a6b-000000000000".to_owned(),
                            ..settled_summary()
                        };
                        200
                    ]
                } else {
                    Vec::new()
                };
                Ok::<_, String>(first_page(auctions))
            }
        });
        let mut dom = VirtualDom::new(|| rsx! {})
            .with_root_context(directory)
            .with_root_context(summary_fixture(&requests, [(200, Some(SUMMARY_BODY))]))
            .with_root_context(PageReadFixture::new(|_url: String| {
                Ok::<_, String>(lots("settled"))
            }));
        dom.rebuild_in_place();
        let result = read_historical(&dom);
        assert_eq!(*requests.borrow(), [DIRECTORY_URL, SUMMARY_URL]);
        let auction = result.unwrap();
        assert_eq!(auction.id, AUCTION_ID);
        assert_eq!(auction.world, "main");
        assert_eq!(auction.number, 5);
        assert_eq!(auction.scheduled_ends_at.as_deref(), Some("2025-11-17T09:46:40Z"));
        assert_eq!(auction.proxy_default_mode, None);
    }

    #[test]
    fn a_settled_round_on_the_cached_first_page_needs_no_summary_request() {
        let directory = first_page(vec![settled_summary()]);
        swr_cache::get_or_fetch(DIRECTORY_URL, 0, swr_cache::now_millis, |_| {
            ready(FetchOutcome::<_, String>::Changed {
                value: directory,
                etag: Some("\"settled-old\"".to_owned()),
            })
        })
        .now_or_never()
        .unwrap()
        .unwrap();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut dom = VirtualDom::new(|| rsx! {})
            .with_root_context(PageReadFixture::new(
                |request: (String, Option<String>)| -> ConditionalGetFailure {
                    panic!("意外重验目录 {request:?}")
                },
            ))
            .with_root_context(summary_fixture(&requests, []))
            .with_root_context(PageReadFixture::new(|_url: String| {
                Ok::<_, String>(lots("settled"))
            }));
        dom.rebuild_in_place();
        let auction = read_historical(&dom).unwrap();
        assert_eq!(
            (auction.id.as_str(), auction.world.as_str(), auction.number),
            (AUCTION_ID, "isekai", 18)
        );
        assert_eq!(auction.scheduled_ends_at, None);
        assert!(requests.borrow().is_empty());
    }

    fn settled_round_missing_from_directory(
        requests: &Rc<RefCell<Vec<String>>>,
        responses: impl IntoIterator<Item = (u16, Option<&'static str>)>,
    ) -> VirtualDom {
        let directory = PageReadFixture::new({
            let requests = requests.clone();
            move |url: String| {
                requests.borrow_mut().push(url);
                Ok::<_, String>(first_page(Vec::new()))
            }
        });
        let mut dom = VirtualDom::new(|| rsx! {})
            .with_root_context(directory)
            .with_root_context(summary_fixture(requests, responses))
            .with_root_context(PageReadFixture::new(|_url: String| {
                Ok::<_, String>(lots("settled"))
            }));
        dom.rebuild_in_place();
        dom
    }

    #[test]
    fn only_a_summary_404_confirms_the_round_is_missing() {
        let not_found = r#"{"code":"not_found","message":"场次不存在","data":null}"#;
        for (status, body, expected) in [
            (404, Some(not_found), "拍卖目录中不存在目标拍卖"),
            (404, None, "拍卖目录中不存在目标拍卖"),
            (503, Some(not_found), "invalid_response"),
        ] {
            let requests = Rc::new(RefCell::new(Vec::new()));
            let dom = settled_round_missing_from_directory(&requests, [(status, body)]);
            assert_eq!(read_historical(&dom), Err(expected.to_owned()));
            assert_eq!(*requests.borrow(), [DIRECTORY_URL, SUMMARY_URL]);
        }
    }

    #[test]
    fn a_summary_read_failure_can_be_retried() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let unavailable =
            r#"{"code":"storage_unavailable","message":"场次摘要暂不可用","data":null}"#;
        let dom = settled_round_missing_from_directory(
            &requests,
            [(503, Some(unavailable)), (200, Some(SUMMARY_BODY))],
        );
        assert_eq!(read_historical(&dom), Err("storage_unavailable".to_owned()));
        assert_eq!(*requests.borrow(), [DIRECTORY_URL, SUMMARY_URL]);

        assert_eq!(read_historical(&dom).unwrap().number, 5);
        assert_eq!(*requests.borrow(), [DIRECTORY_URL, SUMMARY_URL, DIRECTORY_URL, SUMMARY_URL]);
    }

    #[test]
    fn failed_lots_do_not_request_a_summary() {
        for error in ["not_found", "request_timeout"] {
            let requests = Rc::new(RefCell::new(Vec::new()));
            let mut dom = VirtualDom::new(|| rsx! {})
                .with_root_context(summary_fixture(&requests, []))
                .with_root_context(PageReadFixture::new(|()| false))
                .with_root_context(PageReadFixture::new(move |_url: String| {
                    Err::<LotEnvelope, _>(error.to_owned())
                }));
            dom.rebuild_in_place();
            assert_eq!(read_historical(&dom), Err(error.to_owned()));
            assert!(requests.borrow().is_empty());
        }
    }

    #[test]
    fn an_unreadable_first_directory_page_is_not_a_confirmed_miss() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut dom = VirtualDom::new(|| rsx! {})
            .with_root_context(summary_fixture(&requests, []))
            .with_root_context(PageReadFixture::new(|_url: String| {
                Ok::<_, String>(lots("settled"))
            }))
            .with_root_context(PageReadFixture::new(|_url: String| {
                Err::<AuctionListPage, _>("storage_unavailable".to_owned())
            }));
        dom.rebuild_in_place();
        assert_eq!(read_historical(&dom), Err("storage_unavailable".to_owned()));
        assert!(requests.borrow().is_empty());
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
