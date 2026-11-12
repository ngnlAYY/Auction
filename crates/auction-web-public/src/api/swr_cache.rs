//! Public 只读公开 GET 的内存 SWR/TTL 缓存。
//! 只有 `auctions/current`、拍卖目录、当前 lots、lot 详情、竞价日志、价格历史与历史场次
//! 可以进入缓存；会话、账号、watches、偏好、代理私有读取和命令响应永不缓存。条目保存的是
//! 经严格 decoder 校验后的成功正文投影、响应 ETag（若有）与写入时间；缓存不参与金额、资格、
//! 幂等、锁、会话或审计，命中只表示“同一完整 URL 的最近一次已验证成功投影”，清空或丢失
//! 缓存只会多读一次权威接口。
//!
//! 语义：TTL 内直接返回缓存；过期后携带 ETag 条件请求，304 只刷新写入时间；传输失败
//! （网络/超时）允许回退旧值（stale-while-revalidate），但 4xx/5xx 与正文校验失败失败关闭，
//! 既不写入也不回退。写入前提是本次请求开始后没有发生失效，避免在途旧响应重新灌回缓存。

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;

/// 实时视图的缓存有效期：当前拍卖、当前 lots、lot 详情与竞价日志。
/// 这些页面由实时事件与竞价合并窗口主动失效，短 TTL 只用于兜底同一帧内的重复挂载读取。
pub(crate) const LIVE_TTL_MILLIS: u64 = 2_000;
/// 目录视图的缓存有效期：拍卖列表、价格历史与历史场次；变化频率低，失效仍由事件触发。
pub(crate) const CATALOG_TTL_MILLIS: u64 = 15_000;
/// 单线程内存缓存条目上限；筛选与分页组合可能很多，超出时淘汰最早写入的条目。
const MAX_ENTRIES: usize = 128;

/// 单次缓存读取的请求结果分类；由调用方的 fetch 闭包在完整校验后构造。
pub(crate) enum FetchOutcome<T, E> {
    /// 2xx 且正文通过调用方严格校验；ETag 缺失时后续过期读取退回普通 GET。
    Changed { value: T, etag: Option<String> },
    /// 条件请求返回 304；只有存在缓存条目时才会出现。
    NotModified,
    /// 网络/超时等传输失败：允许用旧值继续，不写入缓存。
    Transport(E),
    /// 非 2xx 或正文校验失败：失败关闭，既不写入缓存也不回退旧值。
    Rejected(E),
}

struct Entry {
    value: Rc<dyn Any>,
    etag: Option<String>,
    stored_at_millis: u64,
}

/// 缓存条目与失效修订号共用一个线程本地状态。
#[derive(Default)]
struct Store {
    entries: HashMap<String, Entry>,
    /// 每次失效递增；请求返回时修订号已变化就丢弃写入，避免在途旧响应覆盖新快照。
    revision: u64,
}

thread_local! {
    // 浏览器与测试都在单线程内访问；缓存不跨标签页共享，也不承担任何正确性判断。
    static STORE: RefCell<Store> = RefCell::new(Store::default());
}

struct CachedEntry<T> {
    value: T,
    etag: Option<String>,
    stored_at_millis: u64,
}

/// 判断完整 URL 是否属于允许缓存的公开只读 GET。
/// 即使调用方误把私有读取接进来，也不会命中或写入缓存；路径按段精确匹配，查询参数不参与。
pub(crate) fn is_cacheable_url(url: &str) -> bool {
    let path = url.split_once('?').map_or(url, |(path, _)| path);
    let segments: Vec<&str> = path.split('/').collect();
    matches!(
        segments.as_slice(),
        ["", "api", "v1", "auctions", "current"]
            | ["", "api", "v1", "auctions"]
            | ["", "api", "v2", "auctions"]
            | ["", "api", "v1", "price-history"]
            | ["", "api", "v1", "auctions", _, "lots"]
            | ["", "api", "v1", "auctions", _, "bid-log"]
            | ["", "api", "v1", "lots", _]
    )
}

/// 返回可注入的当前时间毫秒；WASM 使用浏览器墙钟，宿主测试使用进程内单调时钟。
pub(crate) fn now_millis() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::now();
        if now.is_finite() && now > 0.0 {
            // f64 -> u64 在 Rust 中饱和转换，超出范围时取 u64::MAX。
            now as u64
        } else {
            0
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();
        u64::try_from(elapsed).unwrap_or(u64::MAX)
    }
}

/// 读取公开 URL：命中未过期条目时不调用 fetch；过期时把 ETag 交给 fetch 做条件请求。
/// `now_millis` 与 `fetch` 都是注入点，测试无需网络或浏览器即可覆盖全部缓存语义。
pub(crate) async fn get_or_fetch<T, E, C, F, Fut>(
    url: &str,
    ttl_millis: u64,
    now_millis: C,
    fetch: F,
) -> Result<T, E>
where
    T: Clone + 'static,
    C: Fn() -> u64,
    F: FnOnce(Option<String>) -> Fut,
    Fut: Future<Output = FetchOutcome<T, E>>,
{
    // 非白名单 URL 一律直连：不读取、不写入，也不改变失效修订号。
    if !is_cacheable_url(url) {
        return match fetch(None).await {
            FetchOutcome::Changed { value, .. } => Ok(value),
            FetchOutcome::Transport(error) | FetchOutcome::Rejected(error) => Err(error),
            // 调用方在未收到 ETag 时必须把 304 归为 Rejected；这里没有可回退的值。
            FetchOutcome::NotModified => unreachable!("未提供 ETag 的读取不应返回 304"),
        };
    }
    let revision = current_revision();
    if let Some(entry) = lookup::<T>(url) {
        if now_millis().saturating_sub(entry.stored_at_millis) < ttl_millis {
            return Ok(entry.value);
        }
        return match fetch(entry.etag.clone()).await {
            FetchOutcome::Changed { value, etag } => {
                store(url, &value, etag, now_millis(), revision);
                Ok(value)
            }
            FetchOutcome::NotModified => {
                // 304 表示正文未变：只把写入时间前移，避免同一份正文每秒重复传输。
                touch(url, now_millis(), revision);
                Ok(entry.value)
            }
            // 传输失败时旧值仍是最近一次已验证投影；继续服务并等待下次过期重试。
            FetchOutcome::Transport(_) => Ok(entry.value),
            FetchOutcome::Rejected(error) => Err(error),
        };
    }
    match fetch(None).await {
        FetchOutcome::Changed { value, etag } => {
            store(url, &value, etag, now_millis(), revision);
            Ok(value)
        }
        FetchOutcome::Transport(error) | FetchOutcome::Rejected(error) => Err(error),
        FetchOutcome::NotModified => unreachable!("未提供 ETag 的读取不应返回 304"),
    }
}

/// 清空全部公开读缓存；实时事件与成功命令在权威快照变化后调用。
pub(crate) fn invalidate_all() {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        store.entries.clear();
        store.revision = store.revision.wrapping_add(1);
    });
}

/// 按完整 URL 前缀失效；世界切换等只需清理同一接口族时使用，仍递增全局修订号。
#[allow(dead_code)] // 供后续按接口族失效复用；当前统一走 invalidate_all。
pub(crate) fn invalidate_prefix(prefix: &str) {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        store.entries.retain(|url, _| !url.starts_with(prefix));
        store.revision = store.revision.wrapping_add(1);
    });
}

fn current_revision() -> u64 {
    STORE.with(|store| store.borrow().revision)
}

fn lookup<T: Clone + 'static>(url: &str) -> Option<CachedEntry<T>> {
    STORE.with(|store| {
        let store = store.borrow();
        let entry = store.entries.get(url)?;
        let value = entry.value.downcast_ref::<T>()?.clone();
        Some(CachedEntry {
            value,
            etag: entry.etag.clone(),
            stored_at_millis: entry.stored_at_millis,
        })
    })
}

fn store<T: Clone + 'static>(
    url: &str,
    value: &T,
    etag: Option<String>,
    stored_at_millis: u64,
    revision: u64,
) {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        // 请求在途期间发生过失效：响应可能早于新快照，不得重新写入缓存。
        if store.revision != revision {
            return;
        }
        if !store.entries.contains_key(url) && store.entries.len() >= MAX_ENTRIES {
            // 淘汰最早写入的条目；过期条目仍可能用于 SWR 回退，因此只在超限时清理。
            if let Some(oldest) =
                store.entries.iter().min_by_key(|(_, entry)| entry.stored_at_millis)
            {
                let oldest = oldest.0.clone();
                store.entries.remove(&oldest);
            }
        }
        store.entries.insert(
            url.to_owned(),
            Entry { value: Rc::new(value.clone()), etag, stored_at_millis },
        );
    });
}

fn touch(url: &str, stored_at_millis: u64, revision: u64) {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        if store.revision != revision {
            return;
        }
        if let Some(entry) = store.entries.get_mut(url) {
            entry.stored_at_millis = stored_at_millis;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        CATALOG_TTL_MILLIS, FetchOutcome, LIVE_TTL_MILLIS, get_or_fetch, invalidate_all,
        invalidate_prefix, is_cacheable_url,
    };
    use std::cell::Cell;
    use std::future::Future;

    /// 只轮询立即就绪的 mock fetch；不为此引入异步运行时依赖。
    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::hint::spin_loop(),
            }
        }
    }

    /// 每个用例从干净缓存开始；线程本地状态在并行测试之间不共享。
    fn reset() {
        invalidate_all();
    }

    fn changed(
        value: &'static str,
        etag: Option<&str>,
    ) -> FetchOutcome<&'static str, &'static str> {
        FetchOutcome::Changed { value, etag: etag.map(str::to_owned) }
    }

    fn call<'a>(
        url: &'a str,
        now: &'a Cell<u64>,
        ttl_millis: u64,
        calls: &'a Cell<u32>,
        outcome: impl FnOnce(Option<String>) -> FetchOutcome<&'static str, &'static str> + 'a,
    ) -> Result<&'static str, &'static str> {
        block_on(get_or_fetch(
            url,
            ttl_millis,
            || now.get(),
            move |etag| {
                calls.set(calls.get() + 1);
                std::future::ready(outcome(etag))
            },
        ))
    }

    #[test]
    fn fresh_entry_is_served_without_second_request() {
        reset();
        let url = "/api/v1/auctions/current?world=main";
        let now = Cell::new(1_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                assert_eq!(etag, None);
                changed("v1", Some("etag-1"))
            }),
            Ok("v1")
        );
        now.set(1_000 + LIVE_TTL_MILLIS - 1);
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v2", None)), Ok("v1"));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn expired_entry_revalidates_with_etag_and_304_reuses_value() {
        reset();
        let url = "/api/v1/lots/00000000-0000-0000-0000-000000000001";
        let now = Cell::new(5_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| { changed("v1", Some("etag-1")) }),
            Ok("v1")
        );
        now.set(5_000 + LIVE_TTL_MILLIS);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                assert_eq!(etag.as_deref(), Some("etag-1"));
                FetchOutcome::NotModified
            }),
            Ok("v1")
        );
        // 304 已刷新写入时间：TTL 内再次读取不再发请求。
        now.set(5_000 + LIVE_TTL_MILLIS + LIVE_TTL_MILLIS - 1);
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v3", None)), Ok("v1"));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn transport_error_falls_back_to_stale_value_without_caching_failure() {
        reset();
        let url = "/api/v1/auctions/00000000-0000-0000-0000-000000000002/lots?page=1&page_size=20";
        let now = Cell::new(10_000);
        let calls = Cell::new(0);
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v1", None)), Ok("v1"));
        now.set(10_000 + LIVE_TTL_MILLIS);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| FetchOutcome::Transport("network_error")),
            Ok("v1")
        );
        assert_eq!(calls.get(), 2);
        // 失败没有刷新写入时间：下一次仍按过期处理并重新请求权威值。
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v2", None)), Ok("v2"));
        assert_eq!(calls.get(), 3);
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v3", None)), Ok("v2"));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn rejected_response_neither_replaces_nor_falls_back() {
        reset();
        let url = "/api/v1/price-history?page=1&page_size=50&sort=time_desc";
        let now = Cell::new(20_000);
        let calls = Cell::new(0);
        assert_eq!(call(url, &now, CATALOG_TTL_MILLIS, &calls, |_| changed("v1", None)), Ok("v1"));
        now.set(20_000 + CATALOG_TTL_MILLIS);
        assert_eq!(
            call(url, &now, CATALOG_TTL_MILLIS, &calls, |_| FetchOutcome::Rejected(
                "invalid_response"
            )),
            Err("invalid_response")
        );
        // 4xx/5xx 或正文校验失败不得回退旧值；旧条目仍在，但下一次仍会重新请求。
        assert_eq!(call(url, &now, CATALOG_TTL_MILLIS, &calls, |_| changed("v2", None)), Ok("v2"));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn invalidation_during_request_discards_the_inflight_response() {
        reset();
        let url = "/api/v1/auctions/current?world=main";
        let now = Cell::new(30_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| {
                // 模拟请求在途时实时事件完成失效：本次响应仍交给调用方，但不得写回缓存。
                invalidate_all();
                changed("v1", None)
            }),
            Ok("v1")
        );
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v2", None)), Ok("v2"));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn invalidate_all_clears_entries() {
        reset();
        let url =
            "/api/v1/auctions/00000000-0000-0000-0000-000000000003/bid-log?page=1&page_size=50";
        let now = Cell::new(40_000);
        let calls = Cell::new(0);
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v1", None)), Ok("v1"));
        invalidate_all();
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v2", None)), Ok("v2"));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn invalidate_prefix_clears_only_matching_urls() {
        reset();
        let bid_log = "/api/v1/auctions/00000000-0000-0000-0000-000000000004/bid-log?page=1";
        let current = "/api/v1/auctions/current?world=main";
        let now = Cell::new(50_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(bid_log, &now, LIVE_TTL_MILLIS, &calls, |_| changed("bid-1", None)),
            Ok("bid-1")
        );
        assert_eq!(
            call(current, &now, LIVE_TTL_MILLIS, &calls, |_| changed("current-1", None)),
            Ok("current-1")
        );
        invalidate_prefix("/api/v1/auctions/00000000-0000-0000-0000-000000000004/");
        assert_eq!(
            call(bid_log, &now, LIVE_TTL_MILLIS, &calls, |_| changed("bid-2", None)),
            Ok("bid-2")
        );
        assert_eq!(
            call(current, &now, LIVE_TTL_MILLIS, &calls, |_| changed("current-2", None)),
            Ok("current-1")
        );
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn only_public_read_paths_are_cacheable() {
        for url in [
            "/api/v1/auctions/current?world=main",
            "/api/v1/auctions?page=1&page_size=50&status=settled",
            "/api/v2/auctions?page=1&page_size=50&status=settled",
            "/api/v1/auctions/00000000-0000-0000-0000-000000000005/lots?page=1",
            "/api/v1/auctions/00000000-0000-0000-0000-000000000005/bid-log?page=1",
            "/api/v1/lots/00000000-0000-0000-0000-000000000006",
            "/api/v1/price-history?page=1",
        ] {
            assert!(is_cacheable_url(url), "{url} 应允许缓存");
        }
        for url in [
            "/api/v1/session",
            "/api/v1/worlds",
            "/api/v1/me/lot-watches?auction_id=00000000-0000-0000-0000-000000000007",
            "/api/v1/me/lot-watch-preference",
            "/api/v1/lots/00000000-0000-0000-0000-000000000008/watch",
            "/api/v1/lots/00000000-0000-0000-0000-000000000008/bids",
            "/api/v1/lots/00000000-0000-0000-0000-000000000008/proxy-authorizations",
            "/api/v1/auctions/00000000-0000-0000-0000-000000000009/lots/extra",
        ] {
            assert!(!is_cacheable_url(url), "{url} 不得进入缓存");
        }
    }

    #[test]
    fn non_cacheable_url_never_stores_a_value() {
        reset();
        let url = "/api/v1/session";
        let now = Cell::new(60_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("anonymous", None)),
            Ok("anonymous")
        );
        // 即使仍在 TTL 内也必须直连；私有读取不因重复调用而命中缓存。
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("authenticated", None)),
            Ok("authenticated")
        );
        assert_eq!(calls.get(), 2);
    }
}
