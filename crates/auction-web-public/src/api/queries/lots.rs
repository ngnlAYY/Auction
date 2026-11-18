//! lot 目录读取（首次未得到权威值时的单次重读、首屏预取与唯一 URL 构造点）与 lot 详情读取。

use std::cell::Cell;
use std::future::Future;

use auction_web_shared::timer::TimeoutFuture;
use auction_web_shared::visibility::document_is_visible;
use url::form_urlencoded;

use super::super::clock::send_conditional_response;
use super::super::swr_cache::{self, CacheRead, FetchOutcome};
use super::super::{CatalogLevelFilter, LotEnvelope, decoder};
use super::cached_get::{
    cached_public_read_with_retry, failure_outcome, retry_now, transport_failure_outcome,
};

const PUBLIC_QUERY_RETRY_DELAY_MILLIS: u64 = 1_000;
const MAX_CATALOG_RETRY_DELAY_MILLIS: u64 = 60_000;

/// lot 详情读取失败，仅传递是否已由权威 HTTP 状态确认不存在。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LotDetailFetchError {
    Missing,
    Unavailable,
}

impl LotDetailFetchError {
    fn for_status(status: Option<u16>) -> Self {
        if matches!(status, Some(404 | 410)) { Self::Missing } else { Self::Unavailable }
    }

    pub(crate) fn not_found(self) -> bool {
        self == Self::Missing
    }
}

pub(crate) async fn fetch_current_lots(
    auction_id: String,
    category: Option<String>,
    equipment_group: Option<String>,
    levels: CatalogLevelFilter,
    page: i64,
    page_size: i64,
) -> Result<LotEnvelope, String> {
    let url = current_lots_url(
        &auction_id,
        category.as_deref(),
        equipment_group.as_deref(),
        levels,
        page,
        page_size,
    );
    let retry_at = Cell::new(None);
    retry_once_unless_authoritative(
        &url,
        |retry| {
            fetch_current_lots_once(
                &url,
                &auction_id,
                category.as_deref(),
                equipment_group.as_deref(),
                levels,
                &retry_at,
                retry,
            )
        },
        || {
            let now = retry_now();
            let shared =
                swr_cache::public_read_retry_after(&url, now).map(|window| window.not_before);
            let delay = shared
                .into_iter()
                .chain(retry_at.get())
                .max()
                .map_or(PUBLIC_QUERY_RETRY_DELAY_MILLIS, |deadline| deadline.saturating_sub(now));
            wait_to_retry(delay)
        },
    )
    .await
}

fn retry_visible() -> bool {
    #[cfg(test)]
    if let Some(visible) = crate::surface::PageReadFixture::<(), bool>::response(()) {
        return visible;
    }
    document_is_visible()
}

async fn wait_to_retry(delay: u64) -> bool {
    // 超出等待预算时交付首次结果，不能在服务端要求的时间之前发出补读。
    if delay > MAX_CATALOG_RETRY_DELAY_MILLIS || !retry_visible() {
        return false;
    }
    let delay = delay as u32;
    #[cfg(test)]
    if let Some(wait) = crate::surface::PageReadFixture::<
        u32,
        std::pin::Pin<Box<dyn Future<Output = ()>>>,
    >::response(delay)
    {
        wait.await;
        return retry_visible();
    }
    TimeoutFuture::new(delay).await;
    retry_visible()
}

/// 首次权威值立即交付。等待期间仍可补读才发起第二次 GET，否则交付首次错误或旧快照。
async fn retry_once_unless_authoritative<T, E, R, RF, W, WF>(
    url: &str,
    read: R,
    wait: W,
) -> Result<T, E>
where
    R: Fn(bool) -> RF,
    RF: Future<Output = Option<Result<CacheRead<T, E>, E>>>,
    W: FnOnce() -> WF,
    WF: Future<Output = bool>,
{
    let Some(first) = read(false).await else {
        unreachable!("首次读目录不受补读发送门禁限制");
    };
    if first.as_ref().is_ok_and(|read| read.stale_since().is_none()) || !wait().await {
        return first.and_then(|cached| cached.deliver(url));
    }
    // 等同 URL 在途请求时可见性仍会变化。未发送补读时，按此刻的回退下限重新交付首次结果。
    read(true).await.unwrap_or(first).and_then(|cached| cached.deliver(url))
}

/// 首屏预取：与 `/auctions/current` 并行读取上次记住的场次 lots，只单次尝试、丢弃结果。
/// 目录随后以相同参数读取时由缓存层复用这次在途或已写入的结果；场次已变化时只多一次请求。
pub(crate) async fn prefetch_current_lots(
    auction_id: String,
    category: Option<String>,
    equipment_group: Option<String>,
    levels: CatalogLevelFilter,
    page: i64,
    page_size: i64,
) {
    let url = current_lots_url(
        &auction_id,
        category.as_deref(),
        equipment_group.as_deref(),
        levels,
        page,
        page_size,
    );
    let _ = fetch_current_lots_once(
        &url,
        &auction_id,
        category.as_deref(),
        equipment_group.as_deref(),
        levels,
        &Cell::new(None),
        false,
    )
    .await;
}

async fn fetch_current_lots_once(
    url: &str,
    auction_id: &str,
    category: Option<&str>,
    equipment_group: Option<&str>,
    levels: CatalogLevelFilter,
    retry_at: &Cell<Option<u64>>,
    retry: bool,
) -> Option<Result<CacheRead<LotEnvelope, String>, String>> {
    let expected_auction = auction_id.to_owned();
    let expected_category = category.map(str::to_owned);
    let expected_equipment_group = equipment_group.map(str::to_owned);
    cached_public_read_with_retry(
        url,
        swr_cache::LIVE_TTL_MILLIS,
        move |response| async move {
            decoder::decode_lot_list(
                response,
                &expected_auction,
                expected_category.as_deref(),
                expected_equipment_group.as_deref(),
                levels,
            )
            .await
        },
        Some(retry_at),
        move || !retry || retry_visible(),
    )
    .await
}

/// lot 目录 URL 的唯一构造点：读取、首屏预取、标记页与历史场次都经这里，同一组参数得到逐字
/// 相同的 URL，缓存层才能按 URL 合并在途请求。目录行不展示装备属性，始终请求精简投影
/// （`fields=summary`）；属性明细只在打开详情抽屉时经 lot 详情读取。
fn current_lots_url(
    auction_id: &str,
    category: Option<&str>,
    equipment_group: Option<&str>,
    levels: CatalogLevelFilter,
    page: i64,
    page_size: i64,
) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.to_string()).append_pair("page_size", &page_size.to_string());
    if let Some(category) = category {
        query.append_pair("category", category);
    }
    if let Some(equipment_group) = equipment_group {
        query.append_pair("equipment_group", equipment_group);
    }
    if let Some(maximum) = levels.max_level {
        query.append_pair("max_level", &maximum.to_string());
    }
    if !levels.include_unleveled {
        query.append_pair("include_unleveled", "false");
    }
    query.append_pair("fields", "summary");
    format!("/api/v1/auctions/{auction_id}/lots?{}", query.finish())
}

pub(crate) async fn fetch_lot_detail(lot_id: String) -> Result<LotEnvelope, LotDetailFetchError> {
    let url = format!("/api/v1/lots/{lot_id}");
    let request_url = url.clone();
    swr_cache::get_or_fetch(
        &url,
        swr_cache::LIVE_TTL_MILLIS,
        swr_cache::now_millis,
        move |etag| async move {
            #[cfg(test)]
            let fixture =
                dioxus::prelude::try_consume_context::<crate::surface::LotDetailHttpFixture>()
                    .and_then(|transport| (transport.0)(&request_url, etag.as_deref()));
            let response = match send_conditional_response(
                &request_url,
                etag.as_deref(),
                #[cfg(test)]
                fixture,
            )
            .await
            {
                Ok(reply) => reply,
                Err((_, status)) => {
                    return transport_failure_outcome(
                        status,
                        LotDetailFetchError::for_status(status),
                    );
                }
            };
            match response.status() {
                304 if etag.is_some() => FetchOutcome::NotModified,
                200..=299 => {
                    let etag = response.etag();
                    let body = response.text().await;
                    match decoder::decode_lot_detail(body.as_deref(), None, &lot_id) {
                        Ok(value) => FetchOutcome::Changed { value, etag },
                        Err(_) => FetchOutcome::Rejected(LotDetailFetchError::Unavailable),
                    }
                }
                status => failure_outcome(status, LotDetailFetchError::for_status(Some(status))),
            }
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{Future, poll_fn, ready};
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::Poll;

    use dioxus::dioxus_core::NoOpMutations;
    use dioxus::prelude::*;
    use futures_util::FutureExt;
    use serde_json::json;

    use super::{
        current_lots_url, fetch_current_lots, fetch_lot_detail, prefetch_current_lots,
        retry_once_unless_authoritative,
    };
    use crate::api::clock::ConditionalGetFailure;
    use crate::api::swr_cache::{self, CacheRead, FetchOutcome};
    use crate::api::{
        CatalogLevelFilter, HttpResponseFixture, LotEnvelope, TransportError, invalidate_all,
    };
    use crate::surface::{LotDetailHttpFixture, PageReadFixture};

    const AUCTION_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
    const LOT_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";

    thread_local! {
        static DETAIL_READS: Cell<usize> = const { Cell::new(0) };
        static DETAIL_FINISHED: Cell<bool> = const { Cell::new(false) };
    }

    #[component]
    fn LotCacheFixture() -> Element {
        use_context_provider(|| {
            LotDetailHttpFixture(|url, etag| {
                assert_eq!(url, format!("/api/v1/lots/{LOT_ID}"));
                let read = DETAIL_READS.get();
                DETAIL_READS.set(read + 1);
                let reply = match read {
                    0 => {
                        assert_eq!(etag, None);
                        HttpResponseFixture {
                        status: 200,
                        etag: Some("\"present\"".to_owned()),
                        body: Some(json!({
                            "code": "ok", "message": "拍品详情",
                            "data": {
                                "auction_id": AUCTION_ID, "auction_status": "settled",
                                "projection_kind": "live", "presentation_mode": "live",
                                "bidding_open": false, "opens_at": "2026-11-18T12:00:00Z",
                                "preview_revision": 0,
                                "lot": {
                                    "id": LOT_ID, "auction_id": AUCTION_ID, "lot_number": 1,
                                    "status": "unsold", "start_price": 1000, "current_price": 1000,
                                    "next_bid": 1000, "has_leader": false, "seller_name": "S",
                                    "state_version": 1,
                                    "equipment": { "name": "Shield", "category": "Shield",
                                        "equipment_group": "rare", "game_equipment_url": "" }
                                }
                            }
                        }).to_string()),
                    }
                    }
                    1 => {
                        assert_eq!(etag, Some("\"present\""));
                        HttpResponseFixture { status: 404, etag: None, body: None }
                    }
                    2 => {
                        assert_eq!(etag, None, "404 必须驱逐拍品并清除 ETag");
                        HttpResponseFixture { status: 503, etag: None, body: None }
                    }
                    _ => panic!("多余的拍品请求"),
                };
                Some(Box::pin(ready(Ok(reply))))
            })
        });
        use_future(|| async {
            assert!(fetch_lot_detail(LOT_ID.to_owned()).await.is_ok());
            invalidate_all();
            assert!(fetch_lot_detail(LOT_ID.to_owned()).await.unwrap_err().not_found());
            assert!(!fetch_lot_detail(LOT_ID.to_owned()).await.unwrap_err().not_found());
            DETAIL_FINISHED.set(true);
        });
        rsx! {}
    }

    #[test]
    fn lot_detail_response_evicts_the_cached_etag_before_the_next_get() {
        DETAIL_READS.set(0);
        DETAIL_FINISHED.set(false);
        let mut dom = VirtualDom::new(LotCacheFixture);
        dom.rebuild_in_place();
        dom.process_events();
        dom.render_immediate(&mut NoOpMutations);
        assert!(DETAIL_FINISHED.get());
        assert_eq!(DETAIL_READS.get(), 3);
    }

    type CatalogRead = Result<CacheRead<&'static str, &'static str>, &'static str>;
    const STALE_URL: &str = "/api/v1/auctions/fixture/lots?page=1&page_size=20";

    fn authoritative(value: &'static str) -> CatalogRead {
        Ok(CacheRead::current(value))
    }

    fn stale(value: &'static str) -> CatalogRead {
        swr_cache::read_or_fetch(
            STALE_URL,
            0,
            || 0,
            |_| ready(FetchOutcome::<_, &'static str>::Changed { value, etag: None }),
        )
        .now_or_never()
        .unwrap()
        .unwrap();
        swr_cache::read_or_fetch(STALE_URL, 0, || 1_000, |_| ready(FetchOutcome::Busy("read_busy")))
            .now_or_never()
            .unwrap()
    }

    /// 依次给出每次读取的结果，返回最终结果、读取次数与等待次数；读取与等待都立即就绪。
    fn run_catalog_retry(
        reads: impl IntoIterator<Item = CatalogRead>,
    ) -> (Result<&'static str, &'static str>, u32, u32) {
        let reads = RefCell::new(reads.into_iter());
        let read_count = Cell::new(0);
        let wait_count = Cell::new(0);
        let result = retry_once_unless_authoritative(
            STALE_URL,
            |_| {
                read_count.set(read_count.get() + 1);
                std::future::ready(Some(
                    reads.borrow_mut().next().unwrap_or(Err("unexpected_read")),
                ))
            },
            || {
                wait_count.set(wait_count.get() + 1);
                std::future::ready(true)
            },
        )
        .now_or_never()
        .unwrap_or(Err("pending"));
        (result, read_count.get(), wait_count.get())
    }

    #[test]
    fn catalog_read_retries_once_after_a_stale_fallback() {
        // 首次读到权威值：直接返回，不等待、不重读。
        assert_eq!(run_catalog_retry([authoritative("v2")]), (Ok("v2"), 1, 0));
        // 首次只得到暂时性失败回退的旧快照：等待后重读，追上权威值。
        assert_eq!(run_catalog_retry([stale("v1"), authoritative("v2")]), (Ok("v2"), 2, 1));
        // 第二次仍是回退：接受旧快照，不再重试。
        assert_eq!(run_catalog_retry([stale("v1"), stale("v1")]), (Ok("v1"), 2, 1));
        // 第二次失败关闭：返回错误，不拿首次的旧快照兜底。
        assert_eq!(run_catalog_retry([stale("v1"), Err("not_found")]), (Err("not_found"), 2, 1));
        // 首次失败（没有可回退的快照）：照旧再读一次。
        assert_eq!(run_catalog_retry([Err("read_busy"), authoritative("v1")]), (Ok("v1"), 2, 1));
        let kept = retry_once_unless_authoritative(
            STALE_URL,
            |_| ready(Some(stale("v1"))),
            || ready(false),
        )
        .now_or_never()
        .unwrap();
        assert_eq!(kept, Ok("v1"));
    }

    #[test]
    fn a_skipped_retry_rejects_a_snapshot_invalidated_during_the_wait() {
        for invalidate in [swr_cache::invalidate_after_command, || {
            swr_cache::observe_response_status(STALE_URL, 404);
        }] {
            let result = retry_once_unless_authoritative(
                STALE_URL,
                |_| ready(Some(stale("v1"))),
                || {
                    invalidate();
                    ready(false)
                },
            )
            .now_or_never()
            .unwrap();
            assert_eq!(result, Err("read_busy"));
        }
    }

    #[derive(Clone, Copy)]
    enum CatalogStart {
        Direct,
        WaitingForPrefetch,
        AfterPrefetch,
        CancelledPrefetch,
    }

    struct CatalogRetryTrace {
        page: i64,
        start: CatalogStart,
        now: Cell<u64>,
        retry_after: &'static str,
        visible: Cell<bool>,
        visible_on_wake: bool,
        delays: RefCell<Vec<u32>>,
        requests: RefCell<Vec<String>>,
        finished: Cell<bool>,
    }

    fn retry_fixture() -> Element {
        let trace = consume_context::<Rc<CatalogRetryTrace>>();
        use_context_provider({
            let trace = trace.clone();
            move || {
                PageReadFixture::new(move |(url, _etag): (String, Option<String>)| {
                    let first = trace.requests.borrow().is_empty();
                    trace.requests.borrow_mut().push(url);
                    let mut pending = first && !matches!(trace.start, CatalogStart::Direct);
                    let read_body = Box::pin(poll_fn(move |cx| {
                        if std::mem::take(&mut pending) {
                            cx.waker().wake_by_ref();
                            Poll::Pending
                        } else {
                            Poll::Ready(())
                        }
                    }));
                    ConditionalGetFailure {
                        status: Some(429),
                        retry_after: Some(trace.retry_after),
                        date: Some("Wed, 18 Nov 2026 12:00:00 GMT"),
                        transport: TransportError::Timeout,
                        read_body: Some(read_body),
                    }
                })
            }
        });
        use_context_provider({
            let trace = trace.clone();
            move || PageReadFixture::new(move |()| trace.visible.get())
        });
        use_context_provider({
            let trace = trace.clone();
            move || PageReadFixture::new(move |()| trace.now.get())
        });
        use_context_provider({
            let trace = trace.clone();
            move || {
                PageReadFixture::new(move |delay: u32| -> Pin<Box<dyn Future<Output = ()>>> {
                    assert_eq!(trace.requests.borrow().len(), 1, "时限到期前只能有首次 GET");
                    trace.delays.borrow_mut().push(delay);
                    let trace = trace.clone();
                    let mut first_poll = true;
                    Box::pin(poll_fn(move |cx| {
                        if std::mem::take(&mut first_poll) {
                            cx.waker().wake_by_ref();
                            Poll::Pending
                        } else {
                            trace.now.set(trace.now.get() + u64::from(delay));
                            trace.visible.set(trace.visible_on_wake);
                            Poll::Ready(())
                        }
                    }))
                })
            }
        });
        use_future(move || {
            let trace = trace.clone();
            async move {
                let read = fetch_current_lots(
                    AUCTION_ID.to_owned(),
                    None,
                    None,
                    CatalogLevelFilter::default(),
                    trace.page,
                    20,
                );
                let warm = prefetch_current_lots(
                    AUCTION_ID.to_owned(),
                    None,
                    None,
                    CatalogLevelFilter::default(),
                    trace.page,
                    20,
                );
                let result = match trace.start {
                    CatalogStart::Direct => read.await,
                    CatalogStart::WaitingForPrefetch => futures_util::join!(warm, read).1,
                    CatalogStart::AfterPrefetch => {
                        warm.await;
                        read.await
                    }
                    CatalogStart::CancelledPrefetch => {
                        let mut warm = Box::pin(warm);
                        assert!(warm.as_mut().now_or_never().is_none());
                        drop(warm);
                        swr_cache::invalidate_after_command();
                        read.await
                    }
                };
                assert_eq!(result.unwrap_err(), "request_timeout");
                trace.finished.set(true);
            }
        });
        rsx! {}
    }

    #[test]
    fn catalog_rechecks_visibility_after_waiting_for_an_inflight_get() {
        for (visible, invalidated, expected_gets) in
            [(true, false, 3), (false, false, 2), (false, true, 2)]
        {
            let requests = Rc::new(Cell::new(0));
            let wake = Rc::new(Cell::new(false));
            let finish = Rc::new(Cell::new(false));
            let shown = Rc::new(Cell::new(true));
            let page = 27 + i64::from(visible) + 2 * i64::from(invalidated);
            let url =
                current_lots_url(AUCTION_ID, None, None, CatalogLevelFilter::default(), page, 20);
            swr_cache::get_or_fetch(&url, 0, swr_cache::now_millis, |_| {
                ready(FetchOutcome::<_, String>::Changed {
                    value: LotEnvelope {
                        auction_id: AUCTION_ID.to_owned(),
                        auction_status: "live".to_owned(),
                        projection_kind: "live".to_owned(),
                        presentation_mode: "live".to_owned(),
                        bidding_open: true,
                        opens_at: "2026-11-18T12:00:00Z".to_owned(),
                        preview_revision: 0,
                        final_restriction: None,
                        lots: None,
                        lot: None,
                        total: Some(0),
                        page: Some(page),
                        page_size: Some(20),
                        category_counts: None,
                    },
                    etag: Some("\"old\"".to_owned()),
                })
            })
            .now_or_never()
            .unwrap()
            .unwrap();
            swr_cache::invalidate_all();
            let wire = PageReadFixture::new({
                let requests = requests.clone();
                let finish = finish.clone();
                move |(_url, etag): (String, Option<String>)| {
                    assert_eq!(etag.as_deref(), Some("\"old\""));
                    let count = requests.get() + 1;
                    requests.set(count);
                    let read_body = (count == 2).then(|| {
                        let finish = finish.clone();
                        Box::pin(poll_fn(move |_| {
                            if finish.get() { Poll::Ready(()) } else { Poll::Pending }
                        })) as Pin<Box<dyn Future<Output = ()>>>
                    });
                    ConditionalGetFailure {
                        status: Some(503),
                        retry_after: None,
                        date: None,
                        read_body,
                        transport: TransportError::Timeout,
                    }
                }
            });
            let timer = PageReadFixture::new({
                let wake = wake.clone();
                move |delay: u32| -> Pin<Box<dyn Future<Output = ()>>> {
                    assert_eq!(delay, 1_000);
                    let wake = wake.clone();
                    Box::pin(poll_fn(
                        move |_| if wake.get() { Poll::Ready(()) } else { Poll::Pending },
                    ))
                }
            });
            let visibility = PageReadFixture::new({
                let shown = shown.clone();
                move |()| shown.get()
            });
            let mut dom = VirtualDom::new(|| rsx! {})
                .with_root_context(wire)
                .with_root_context(timer)
                .with_root_context(visibility);
            dom.rebuild_in_place();
            dom.in_scope(ScopeId::APP, || {
                let mut read = Box::pin(fetch_current_lots(
                    AUCTION_ID.to_owned(),
                    None,
                    None,
                    CatalogLevelFilter::default(),
                    page,
                    20,
                ));
                assert!(read.as_mut().now_or_never().is_none());
                assert_eq!(requests.get(), 1);
                let mut inflight = Box::pin(prefetch_current_lots(
                    AUCTION_ID.to_owned(),
                    None,
                    None,
                    CatalogLevelFilter::default(),
                    page,
                    20,
                ));
                assert!(inflight.as_mut().now_or_never().is_none());
                wake.set(true);
                assert!(read.as_mut().now_or_never().is_none(), "补读等单飞请求结束");
                assert_eq!(requests.get(), 2);
                shown.set(visible);
                if invalidated {
                    swr_cache::invalidate_after_command();
                }
                finish.set(true);
                inflight.now_or_never().unwrap();
                let result = read.now_or_never().unwrap();
                assert_eq!(requests.get(), expected_gets);
                if invalidated {
                    assert_eq!(result.unwrap_err(), "request_timeout");
                } else {
                    assert_eq!(result.unwrap().page, Some(page));
                }
            });
        }
    }

    #[test]
    fn catalog_uses_observed_retry_after_and_drops_hidden_retries() {
        for (index, (retry_after, visible, visible_on_wake, expected_delay, requests)) in [
            ("4", true, true, Some(4_000), 2),
            ("Wed, 18 Nov 2026 12:00:07 GMT", true, true, Some(7_000), 2),
            ("invalid", true, true, Some(1_000), 2),
            ("60", true, true, Some(60_000), 2),
            ("61", true, true, None, 1),
            ("999999999999999999999999", true, true, None, 1),
            ("4", false, true, None, 1),
            ("4", true, false, Some(4_000), 1),
        ]
        .into_iter()
        .enumerate()
        {
            let trace = Rc::new(CatalogRetryTrace {
                page: i64::try_from(index).unwrap() + 1,
                start: CatalogStart::Direct,
                now: Cell::new(0),
                retry_after,
                visible: Cell::new(visible),
                visible_on_wake,
                delays: RefCell::default(),
                requests: RefCell::default(),
                finished: Cell::default(),
            });
            run_retry_fixture(trace, expected_delay, requests);
        }
    }

    #[test]
    fn directory_waits_for_the_retry_window_even_when_the_header_came_from_prefetch() {
        for (page, start) in [
            (1, CatalogStart::WaitingForPrefetch),
            (2, CatalogStart::AfterPrefetch),
            (3, CatalogStart::CancelledPrefetch),
        ] {
            let trace = Rc::new(CatalogRetryTrace {
                page,
                start,
                now: Cell::new(0),
                retry_after: "4",
                visible: Cell::new(true),
                visible_on_wake: true,
                delays: RefCell::default(),
                requests: RefCell::default(),
                finished: Cell::default(),
            });
            run_retry_fixture(trace, Some(4_000), 2);
        }
    }

    fn run_retry_fixture(
        trace: Rc<CatalogRetryTrace>,
        expected_delay: Option<u32>,
        requests: usize,
    ) {
        let mut dom = VirtualDom::new(retry_fixture).with_root_context(trace.clone());
        dom.rebuild_in_place();
        for _ in 0..12 {
            dom.process_events();
            dom.render_immediate(&mut NoOpMutations);
        }
        assert!(trace.finished.get());
        assert_eq!(*trace.delays.borrow(), expected_delay.into_iter().collect::<Vec<_>>());
        assert_eq!(
            *trace.requests.borrow(),
            vec![
                format!(
                    "/api/v1/auctions/{AUCTION_ID}/lots?page={}&page_size=20&fields=summary",
                    trace.page
                );
                requests
            ]
        );
    }

    #[test]
    fn lot_list_url_always_requests_the_summary_projection() {
        // 读取、预取、标记页与历史场次共用这一构造点：同一组参数得到逐字相同的 URL。
        assert_eq!(
            current_lots_url(AUCTION_ID, None, None, CatalogLevelFilter::default(), 1, 20),
            format!("/api/v1/auctions/{AUCTION_ID}/lots?page=1&page_size=20&fields=summary")
        );
        let levels = CatalogLevelFilter { max_level: Some(450), include_unleveled: false };
        assert_eq!(
            current_lots_url(AUCTION_ID, Some("clo"), Some("rare"), levels, 3, 50),
            format!(
                "/api/v1/auctions/{AUCTION_ID}/lots?page=3&page_size=50&category=clo\
                 &equipment_group=rare&max_level=450&include_unleveled=false&fields=summary"
            )
        );
    }
}
