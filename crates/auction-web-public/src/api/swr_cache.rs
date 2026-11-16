//! Public 只读公开 GET 的内存 SWR/TTL 缓存。
//! 只有 `auctions/current`、拍卖目录、当前 lots、lot 详情、竞价日志、价格历史与历史场次
//! 可以进入缓存；会话、账号、watches、偏好、代理私有读取和命令响应永不缓存。条目保存的是
//! 经严格 decoder 校验后的成功正文投影、响应 ETag（若有）与写入时间；缓存不参与金额、资格、
//! 幂等、锁、会话或审计，命中只表示“同一完整 URL 的最近一次已验证成功投影”，清空或丢失
//! 缓存只会多读一次权威接口。
//!
//! 语义：TTL 内且未被失效的条目直接返回；过期或被失效后携带 ETag 条件请求，304 刷新写入时间
//! 并恢复新鲜。传输失败（网络/超时）与服务端暂时拒绝（429 读闸繁忙或来源超额、503 依赖暂不
//! 可用、源站不可达时的网关 502/504 与 CDN 520–524）在已有条目时回退旧值（stale-while-revalidate）并标明这是回退，没有条目时照常返回
//! 错误；其他 4xx/5xx 与正文校验失败失败关闭，既不写入也不回退。旧快照只用于读视图展示：命令
//! 仍由服务端权威校验，过期的下一口价只会被服务端拒绝。失效只把全部条目标为过期（保留正文与
//! ETag）并推进修订号。
//! 同一 URL 的读取经在途登记串行：已有请求在途时，后到的读取等它结束再按上述规则查缓存，不并发
//! 发出相同请求，因此成功响应不会比已有条目旧，总是替换其正文与 ETag。请求期间发生过失效时，
//! 响应只作为过期条目保留，供条件请求与暂时性失败回退，不作为新鲜命中；在途请求失败、被取消或
//! 期间失效时，等待方自行请求。
//!
//! 本层是这些读取唯一的客户端新鲜度来源：未命中（或旧条目没有 ETag）时的请求要求浏览器向服务端
//! 再验证，不接受 HTTP 缓存按 `max-age`/`stale-while-revalidate` 给出的旧副本，否则旧正文会
//! 被当作新快照保留到 TTL 结束；带 `If-None-Match` 的条件请求按 Fetch 规范同样绕过 HTTP 缓存。
//! 失效时机是实时事件、开场迁移、外壳权威轮询（非 Live 每 60 秒，Live 约每 5 分钟兜底）、页面
//! 恢复可见、看板对齐重读与错误重试、会话变化，以及竞价与代理命令得到服务端答复（成功或被拒绝）；
//! 只推进页面刷新代号而不失效时，TTL 内的读取直接命中新鲜条目，不会回到服务端。命令答复后的失效另把回退下限推进到
//! 新修订号：用户刚提交的命令之后，读视图只显示权威结果或错误，不退回命令之前的价格与领先者；
//! 他人竞价等事件触发的普通失效仍允许繁忙时先显示最后已知快照。

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

/// 实时视图的缓存有效期：当前拍卖、当前 lots、lot 详情与竞价日志。
/// 这些页面由实时事件与竞价合并窗口主动失效；60 秒 TTL 覆盖关闭详情后回到列表的常见路径，
/// 避免返回时因短 TTL 过期而重新显示加载态。外壳权威轮询（非 Live 每 60 秒，Live 约每 5 分钟
/// 兜底）会使全部条目过期（保留正文与 ETag，随后的读取带 `If-None-Match` 再验证）。
pub(crate) const LIVE_TTL_MILLIS: u64 = 60_000;
/// 目录视图的缓存有效期：拍卖列表、价格历史与历史场次；变化频率低，失效仍由事件触发。
pub(crate) const CATALOG_TTL_MILLIS: u64 = 15_000;
/// 单线程内存缓存条目上限；筛选与分页组合可能很多，失效只标记过期、不删除条目，超出上限时
/// 淘汰最早写入的条目。
const MAX_ENTRIES: usize = 128;

/// 单次缓存读取的请求结果分类；由调用方的 fetch 闭包在完整校验后构造。
pub(crate) enum FetchOutcome<T, E> {
    /// 2xx 且正文通过调用方严格校验；ETag 缺失时后续过期读取退回普通 GET。
    Changed { value: T, etag: Option<String> },
    /// 条件请求返回 304；只有存在缓存条目时才会出现。
    NotModified,
    /// 网络/超时等传输失败：允许用旧值继续，不写入缓存。
    Transport(E),
    /// 服务端暂时拒绝（429、503 与网关 502/504、CDN 520–524）：与传输失败相同，有可回退的条目
    /// （含过期或已被失效的）时回退旧值，没有条目时返回错误；不写入、不刷新写入时间。
    Busy(E),
    /// 其他非 2xx 或正文校验失败：失败关闭，既不写入缓存也不回退旧值。
    Rejected(E),
}

/// 一次缓存读取得到的值及其来源。
pub(crate) struct CacheRead<T> {
    pub(crate) value: T,
    /// 重读遇到传输失败或服务端暂时拒绝，返回的是已有的过期或已被失效快照；需要尽快追上权威值
    /// 的调用方可据此稍后再读。新鲜命中、304 确认与新响应都为 false。
    pub(crate) stale_fallback: bool,
}

struct Entry {
    value: Rc<dyn Any>,
    etag: Option<String>,
    stored_at_millis: u64,
    /// 写入或最近一次 304 确认该条目的请求开始时的失效修订号；落后于当前修订号即已被失效，
    /// 只能用于条件请求与暂时性失败回退，不能当作新鲜命中。
    revision: u64,
}

/// 缓存条目与失效修订号共用一个线程本地状态。
#[derive(Default)]
struct Store {
    entries: HashMap<String, Entry>,
    /// 每次失效递增，已有条目随之全部过期；请求开始后发生过失效时，响应仍写入但保留请求开始时
    /// 的修订号，只作为过期条目。
    revision: u64,
    /// 同一 URL 正在进行的请求。
    pending: HashMap<String, Rc<Pending>>,
    /// 暂时性失败回退的修订号下限：命令答复后推进到当时的修订号。修订号低于下限的条目（命令前
    /// 写入，或命令前已在途、之后才写回）仍可带 ETag 条件请求，但不再用于回退。修订号按 u64
    /// 逐次递增，实际不会回绕。
    fallback_floor: u64,
}

/// 在途请求的完成通知；只表示“该请求已结束”，不携带结果。等待方结束后仍按缓存规则重新读取：
/// 首个请求失败、被取消或期间发生失效（只留下过期条目）时，等待方自行请求；是否新鲜完全由
/// 条目记录的修订号决定。
#[derive(Default)]
struct Pending {
    done: Cell<bool>,
    wakers: RefCell<Vec<Waker>>,
}

impl Pending {
    fn finish(&self) {
        self.done.set(true);
        for waker in self.wakers.borrow_mut().drain(..) {
            waker.wake();
        }
    }
}

struct WaitPending(Rc<Pending>);

impl Future for WaitPending {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.0.done.get() {
            return Poll::Ready(());
        }
        self.0.wakers.borrow_mut().push(context.waker().clone());
        Poll::Pending
    }
}

/// 登记在途请求；正常结束、出错或调用方被丢弃（组件卸载）时都会撤销登记并唤醒等待方。
struct PendingGuard {
    url: String,
    pending: Rc<Pending>,
}

impl PendingGuard {
    fn register(url: &str) -> Self {
        let pending = Rc::new(Pending::default());
        STORE.with(|store| {
            store.borrow_mut().pending.insert(url.to_owned(), Rc::clone(&pending));
        });
        Self { url: url.to_owned(), pending }
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        STORE.with(|store| {
            let mut store = store.borrow_mut();
            if store
                .pending
                .get(&self.url)
                .is_some_and(|current| Rc::ptr_eq(current, &self.pending))
            {
                store.pending.remove(&self.url);
            }
        });
        self.pending.finish();
    }
}

fn pending_for(url: &str) -> Option<Rc<Pending>> {
    STORE.with(|store| store.borrow().pending.get(url).cloned())
}

thread_local! {
    // 浏览器与测试都在单线程内访问；缓存不跨标签页共享，也不承担任何正确性判断。
    static STORE: RefCell<Store> = RefCell::new(Store::default());
}

struct CachedEntry<T> {
    value: T,
    etag: Option<String>,
    stored_at_millis: u64,
    /// 条目在当前修订号下写入或确认；失效后为 false。
    current: bool,
    /// 条目不早于最近一次命令答复，暂时性失败时可作为回退值。
    fallback_allowed: bool,
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

/// 与 [`read_or_fetch`] 相同，只取值，不区分是否为暂时性失败后的旧快照回退。
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
    read_or_fetch(url, ttl_millis, now_millis, fetch).await.map(|read| read.value)
}

/// 读取公开 URL：命中未过期且未被失效的条目时不调用 fetch；否则把旧条目的 ETag 交给 fetch
/// 做条件请求。`now_millis` 与 `fetch` 都是注入点，测试无需网络或浏览器即可覆盖全部缓存语义。
pub(crate) async fn read_or_fetch<T, E, C, F, Fut>(
    url: &str,
    ttl_millis: u64,
    now_millis: C,
    fetch: F,
) -> Result<CacheRead<T>, E>
where
    T: Clone + 'static,
    C: Fn() -> u64,
    F: FnOnce(Option<String>) -> Fut,
    Fut: Future<Output = FetchOutcome<T, E>>,
{
    let current = |value| CacheRead { value, stale_fallback: false };
    // 非白名单 URL 一律直连：不读取、不写入，也不改变失效修订号。
    if !is_cacheable_url(url) {
        return match fetch(None).await {
            FetchOutcome::Changed { value, .. } => Ok(current(value)),
            FetchOutcome::Transport(error)
            | FetchOutcome::Busy(error)
            | FetchOutcome::Rejected(error) => Err(error),
            // 调用方在未收到 ETag 时必须把 304 归为 Rejected；这里没有可回退的值。
            FetchOutcome::NotModified => unreachable!("未提供 ETag 的读取不应返回 304"),
        };
    }
    let fresh = |entry: &CachedEntry<T>| {
        entry.current && now_millis().saturating_sub(entry.stored_at_millis) < ttl_millis
    };
    // 同一 URL 已有请求在途（例如启动预取）：等它结束再按同一规则读缓存。单线程内“检查—登记”
    // 之间没有 await，因此不会有两个请求同时登记同一 URL。
    while let Some(pending) = pending_for(url) {
        WaitPending(pending).await;
        if let Some(entry) = lookup::<T>(url).filter(fresh) {
            return Ok(current(entry.value));
        }
    }
    let _pending = PendingGuard::register(url);
    let revision = current_revision();
    if let Some(entry) = lookup::<T>(url) {
        if fresh(&entry) {
            return Ok(current(entry.value));
        }
        // 过期与已被失效的条目同样带旧 ETag 再验证：正文未变时 304 直接复用，不重传整页。
        return match fetch(entry.etag.clone()).await {
            FetchOutcome::Changed { value, etag } => {
                store(url, &value, etag, now_millis(), revision);
                Ok(current(value))
            }
            FetchOutcome::NotModified => {
                // 304 表示正文在本次请求时仍未变：期间没有再发生失效时，前移写入时间并标回
                // 当前修订号。
                touch(url, now_millis(), revision);
                Ok(current(entry.value))
            }
            // 传输失败或服务端暂时拒绝时，旧值仍是最近一次已验证投影：继续展示并标明是回退，
            // 由调用方决定是否稍后再读；不刷新写入时间，也不把条目标回新鲜。条目早于最近一次
            // 命令答复时不回退，按失败返回，避免命令之后显示命令之前的快照。
            FetchOutcome::Transport(error) | FetchOutcome::Busy(error) => {
                if entry.fallback_allowed {
                    Ok(CacheRead { value: entry.value, stale_fallback: true })
                } else {
                    Err(error)
                }
            }
            FetchOutcome::Rejected(error) => Err(error),
        };
    }
    match fetch(None).await {
        FetchOutcome::Changed { value, etag } => {
            store(url, &value, etag, now_millis(), revision);
            Ok(current(value))
        }
        FetchOutcome::Transport(error)
        | FetchOutcome::Busy(error)
        | FetchOutcome::Rejected(error) => Err(error),
        FetchOutcome::NotModified => unreachable!("未提供 ETag 的读取不应返回 304"),
    }
}

/// 使全部公开读缓存过期；实时事件与命令结果等在权威快照可能变化后调用。
/// 条目保留正文与 ETag，只供随后的条件请求与暂时性失败回退；失效前发出的在途请求随后写回的
/// 响应同样只算过期条目，不成为新鲜命中。
pub(crate) fn invalidate_all() {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        store.revision = store.revision.wrapping_add(1);
    });
}

/// 用户命令（竞价、代理创建与降额）得到服务端答复后调用：与 [`invalidate_all`] 一样使全部条目
/// 过期，另把回退下限推进到新修订号。之前的条目仍可带 ETag 做条件请求，但暂时性失败时不再回退
/// 到命令之前的快照，读视图只显示权威结果或错误。
pub(crate) fn invalidate_after_command() {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        store.revision = store.revision.wrapping_add(1);
        store.fallback_floor = store.revision;
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
            current: entry.revision == store.revision,
            fallback_allowed: entry.revision >= store.fallback_floor,
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
        // 同一 URL 的请求经在途登记串行，响应不会比已有条目旧，因此总是替换正文与 ETag：页面已
        // 展示这份响应，之后的条件请求与暂时性失败回退都不能退回更早的快照。`revision` 是请求
        // 开始时的修订号，期间发生过失效时条目随即落后于当前修订号，只作过期条目，不成为新鲜命中。
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
            Entry { value: Rc::new(value.clone()), etag, stored_at_millis, revision },
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
            entry.revision = revision;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Poll};

    use super::{
        CATALOG_TTL_MILLIS, FetchOutcome, LIVE_TTL_MILLIS, MAX_ENTRIES, STORE, Store, get_or_fetch,
        invalidate_after_command, invalidate_all, is_cacheable_url, read_or_fetch,
    };

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

    /// 每个用例从干净缓存开始；失效不删除条目，这里直接重建线程本地状态。
    fn reset() {
        STORE.with(|store| *store.borrow_mut() = Store::default());
    }

    fn entry_count() -> usize {
        STORE.with(|store| store.borrow().entries.len())
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
    fn invalidation_during_request_keeps_the_inflight_response_stale() {
        reset();
        let url = "/api/v1/auctions/current?world=main";
        let now = Cell::new(30_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| {
                // 模拟请求在途时实时事件完成失效：本次响应照常交给调用方，期间发生失效时只作为
                // 过期条目保留，供条件请求与暂时性失败回退，不作为新鲜命中。
                invalidate_all();
                changed("v1", None)
            }),
            Ok("v1")
        );
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v2", None)), Ok("v2"));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn response_after_inflight_invalidation_backs_revalidation_and_fallback() {
        reset();
        let url = "/api/v1/lots/00000000-0000-0000-0000-00000000000e";
        let now = Cell::new(35_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v1", Some("etag-1"))),
            Ok("v1")
        );
        invalidate_all();
        // 重读在途时又来一次失效：v2 已交给页面展示，缓存随之前进到 v2，只是仍按过期处理。
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                assert_eq!(etag.as_deref(), Some("etag-1"));
                invalidate_all();
                changed("v2", Some("etag-2"))
            }),
            Ok("v2")
        );
        // 下一次读取带 v2 的 ETag 再验证；读闸繁忙时回退 v2，不倒退到页面已替换掉的 v1。
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                assert_eq!(etag.as_deref(), Some("etag-2"));
                FetchOutcome::Busy("read_busy")
            }),
            Ok("v2")
        );
        assert_eq!(calls.get(), 3);
    }

    /// 由测试手动放行的 fetch：结果写入前一直挂起，模拟在途请求。
    struct Gate(Rc<RefCell<Option<FetchOutcome<&'static str, &'static str>>>>);

    impl Future for Gate {
        type Output = FetchOutcome<&'static str, &'static str>;

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            self.0.borrow_mut().take().map_or(Poll::Pending, Poll::Ready)
        }
    }

    fn poll_once<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(std::task::Waker::noop()))
    }

    #[test]
    fn concurrent_reads_share_one_inflight_request() {
        reset();
        let url = "/api/v1/auctions/00000000-0000-0000-0000-00000000000a/lots?page=1&page_size=20";
        let calls = Cell::new(0);
        let gate = Rc::new(RefCell::new(None));
        let first_gate = Rc::clone(&gate);
        let mut first = Box::pin(get_or_fetch(
            url,
            LIVE_TTL_MILLIS,
            || 1_000,
            |_| {
                calls.set(calls.get() + 1);
                Gate(first_gate)
            },
        ));
        assert!(poll_once(first.as_mut()).is_pending());
        let mut second = Box::pin(get_or_fetch(
            url,
            LIVE_TTL_MILLIS,
            || 1_000,
            |_| {
                calls.set(calls.get() + 1);
                std::future::ready(changed("second", None))
            },
        ));
        // 第二个读取只等待，不发出第二个相同请求。
        assert!(poll_once(second.as_mut()).is_pending());
        *gate.borrow_mut() = Some(changed("v1", None));
        assert_eq!(poll_once(first.as_mut()), Poll::Ready(Ok("v1")));
        assert_eq!(poll_once(second.as_mut()), Poll::Ready(Ok("v1")));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn waiter_requests_again_when_the_inflight_response_was_invalidated() {
        reset();
        let url = "/api/v1/auctions/current?world=isekai";
        let calls = Cell::new(0);
        let gate = Rc::new(RefCell::new(None));
        let first_gate = Rc::clone(&gate);
        let mut first = Box::pin(get_or_fetch(
            url,
            LIVE_TTL_MILLIS,
            || 1_000,
            |_| {
                calls.set(calls.get() + 1);
                Gate(first_gate)
            },
        ));
        assert!(poll_once(first.as_mut()).is_pending());
        let mut second = Box::pin(get_or_fetch(
            url,
            LIVE_TTL_MILLIS,
            || 1_000,
            |etag| {
                // 等待方不把首个响应当作新鲜命中，而是带它的 ETag 自行再验证。
                assert_eq!(etag.as_deref(), Some("etag-stale"));
                calls.set(calls.get() + 1);
                std::future::ready(changed("fresh", None))
            },
        ));
        assert!(poll_once(second.as_mut()).is_pending());
        // 在途期间失效：首个响应照常交给其调用方，只作为过期条目保留，等待方必须自己读取新快照。
        invalidate_all();
        *gate.borrow_mut() = Some(changed("stale", Some("etag-stale")));
        assert_eq!(poll_once(first.as_mut()), Poll::Ready(Ok("stale")));
        assert_eq!(poll_once(second.as_mut()), Poll::Ready(Ok("fresh")));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn dropped_inflight_request_releases_waiters() {
        reset();
        let url = "/api/v1/lots/00000000-0000-0000-0000-00000000000b";
        let calls = Cell::new(0);
        let mut first = Box::pin(get_or_fetch(
            url,
            LIVE_TTL_MILLIS,
            || 1_000,
            |_| {
                calls.set(calls.get() + 1);
                Gate(Rc::new(RefCell::new(None)))
            },
        ));
        assert!(poll_once(first.as_mut()).is_pending());
        let mut second = Box::pin(get_or_fetch(
            url,
            LIVE_TTL_MILLIS,
            || 1_000,
            |_| {
                calls.set(calls.get() + 1);
                std::future::ready(changed("own", None))
            },
        ));
        assert!(poll_once(second.as_mut()).is_pending());
        // 发起方随组件卸载被丢弃：登记撤销，等待方不能永远挂起。
        drop(first);
        assert_eq!(poll_once(second.as_mut()), Poll::Ready(Ok("own")));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn invalidated_entry_revalidates_with_its_etag_and_304_restores_freshness() {
        reset();
        let url =
            "/api/v1/auctions/00000000-0000-0000-0000-000000000003/bid-log?page=1&page_size=50";
        let now = Cell::new(40_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v1", Some("etag-1"))),
            Ok("v1")
        );
        // 仍在 TTL 内，但失效后不得当作新鲜命中：必须带旧 ETag 向服务端再验证。
        invalidate_all();
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                assert_eq!(etag.as_deref(), Some("etag-1"));
                FetchOutcome::NotModified
            }),
            Ok("v1")
        );
        // 304 已在当前修订号下确认正文：TTL 内再次读取不再发请求。
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v3", None)), Ok("v1"));
        assert_eq!(calls.get(), 2);
        // 再次失效后正文变化时照常写入新值。
        invalidate_all();
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v4", Some("etag-4"))),
            Ok("v4")
        );
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn busy_rejection_serves_the_invalidated_snapshot_and_keeps_it_stale() {
        reset();
        let url = "/api/v1/auctions/00000000-0000-0000-0000-00000000000c/lots?page=1&page_size=20";
        let now = Cell::new(50_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v1", Some("etag-1"))),
            Ok("v1")
        );
        invalidate_all();
        // 429/503 类暂时拒绝：已有快照继续展示，不把错误面板换上整张目录。
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| FetchOutcome::Busy("read_busy")),
            Ok("v1")
        );
        // 回退不刷新条目：下一次读取仍带旧 ETag 再验证，而不是命中旧值。
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                assert_eq!(etag.as_deref(), Some("etag-1"));
                changed("v2", Some("etag-2"))
            }),
            Ok("v2")
        );
        assert_eq!(calls.get(), 3);
        // 传输失败对已失效条目同样回退旧值。
        invalidate_all();
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| FetchOutcome::Transport("network_error")),
            Ok("v2")
        );
    }

    #[test]
    fn busy_rejection_without_snapshot_still_fails() {
        reset();
        let url = "/api/v1/lots/00000000-0000-0000-0000-00000000000d";
        let now = Cell::new(60_000);
        let calls = Cell::new(0);
        // 首屏没有可回退的快照：照常返回错误，由页面显示错误与重试。
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| FetchOutcome::Busy("read_busy")),
            Err("read_busy")
        );
        assert_eq!(entry_count(), 0);
        // 暂时拒绝不留条目：下一次读取不带 ETag。
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                assert_eq!(etag, None);
                changed("v1", None)
            }),
            Ok("v1")
        );
    }

    #[test]
    fn other_rejections_do_not_fall_back_to_the_invalidated_snapshot() {
        reset();
        let url = "/api/v1/auctions/current?world=main";
        let now = Cell::new(70_000);
        let calls = Cell::new(0);
        assert_eq!(call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v1", None)), Ok("v1"));
        invalidate_all();
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| FetchOutcome::Rejected("not_found")),
            Err("not_found")
        );
    }

    #[test]
    fn only_transient_failure_fallbacks_are_marked_stale() {
        reset();
        let url = "/api/v1/auctions/00000000-0000-0000-0000-00000000000f/lots?page=1&page_size=20";
        let read = |outcome: FetchOutcome<&'static str, &'static str>| {
            block_on(read_or_fetch(
                url,
                LIVE_TTL_MILLIS,
                || 90_000,
                |_| std::future::ready(outcome),
            ))
            .map(|cached| (cached.value, cached.stale_fallback))
        };
        // 新响应与 TTL 内的新鲜命中（不会调用 fetch）都是权威值。
        assert_eq!(read(changed("v1", Some("etag-1"))), Ok(("v1", false)));
        assert_eq!(read(FetchOutcome::Rejected("unused")), Ok(("v1", false)));
        invalidate_all();
        // 暂时拒绝与传输失败回退的旧快照带标记，调用方据此决定是否稍后再读。
        assert_eq!(read(FetchOutcome::Busy("read_busy")), Ok(("v1", true)));
        assert_eq!(read(FetchOutcome::Transport("network_error")), Ok(("v1", true)));
        // 304 确认的正文同样是权威值。
        assert_eq!(read(FetchOutcome::NotModified), Ok(("v1", false)));
    }

    /// 命令答复后，命令之前的快照只能用于条件请求：暂时性失败按错误返回；304 确认后重新允许回退。
    #[test]
    fn command_invalidation_forbids_falling_back_to_pre_command_snapshots() {
        reset();
        let url = "/api/v1/lots/00000000-0000-0000-0000-00000000000c";
        let now = Cell::new(90_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v1", Some("etag-1"))),
            Ok("v1")
        );
        invalidate_after_command();
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                assert_eq!(etag.as_deref(), Some("etag-1"));
                FetchOutcome::Busy("read_busy")
            }),
            Err("read_busy")
        );
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| FetchOutcome::Transport("network")),
            Err("network")
        );
        // 命令之后的 304 是权威确认：条目回到当前修订号，此后的暂时性失败可以回退。
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| FetchOutcome::NotModified),
            Ok("v1")
        );
        invalidate_all();
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| FetchOutcome::Busy("read_busy")),
            Ok("v1")
        );
        assert_eq!(calls.get(), 5);
    }

    /// 命令前已在途、命令答复后才写回的响应早于回退下限：仍可条件请求，但不作为回退值。
    #[test]
    fn inflight_response_from_before_a_command_is_not_a_fallback() {
        reset();
        let url = "/api/v1/auctions/00000000-0000-0000-0000-00000000000d/lots?page=1&page_size=20";
        let now = Cell::new(95_000);
        let calls = Cell::new(0);
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |_| {
                invalidate_after_command();
                changed("v-before-command", Some("etag-before"))
            }),
            Ok("v-before-command")
        );
        assert_eq!(
            call(url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                assert_eq!(etag.as_deref(), Some("etag-before"));
                FetchOutcome::Busy("read_busy")
            }),
            Err("read_busy")
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn entry_cap_evicts_the_oldest_write() {
        reset();
        let calls = Cell::new(0);
        let url = |index: usize| format!("/api/v1/lots/00000000-0000-0000-0000-{index:012}");
        for index in 0..=MAX_ENTRIES {
            let now = Cell::new(1_000 + index as u64);
            let url = url(index);
            assert_eq!(call(&url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("v", None)), Ok("v"));
            // 失效不删除条目，上限是会话内条目数的唯一约束。
            invalidate_all();
        }
        assert_eq!(entry_count(), MAX_ENTRIES);
        let present =
            |index: usize| STORE.with(|store| store.borrow().entries.contains_key(&url(index)));
        assert!(!present(0), "最早写入的条目应被淘汰");
        assert!(present(1));
        assert!(present(MAX_ENTRIES));
    }

    /// 看板对齐重读、错误重试与恢复可见补读的缓存前提：TTL 内只推进刷新代号的重读命中新鲜条目，
    /// 当前场次与目录原样返回同一对不一致的快照、不发请求；经共用刷新入口先失效后，两侧都带各自
    /// ETag 回到服务端，滞后的一侧追上权威值，未变的一侧 304 复用。
    #[test]
    fn misaligned_current_and_lots_reach_the_server_only_after_invalidation() {
        let current = "/api/v1/auctions/current?world=main&include=proxy_default_mode";
        let lots = "/api/v1/auctions/00000000-0000-0000-0000-000000000010/lots?page=1&page_size=20";
        // 旧场次配新目录（开场前后各持一边），以及相反的旧目录配新场次。
        for (lagging_url, lagging_etag, fresh_url) in
            [(current, "etag-current", lots), (lots, "etag-lots", current)]
        {
            reset();
            let now = Cell::new(100_000);
            let calls = Cell::new(0);
            let (lagging_value, fresh_value) = ("preview", "live");
            assert_eq!(
                call(lagging_url, &now, LIVE_TTL_MILLIS, &calls, |_| {
                    changed(lagging_value, Some(lagging_etag))
                }),
                Ok(lagging_value)
            );
            assert_eq!(
                call(fresh_url, &now, LIVE_TTL_MILLIS, &calls, |_| {
                    changed(fresh_value, Some("etag-fresh"))
                }),
                Ok(fresh_value)
            );
            // 只推进代号的重读（旧的对齐重读与恢复可见补读）：两侧都命中新鲜条目，不发请求。
            now.set(100_000 + 1_000);
            assert_eq!(
                call(lagging_url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("live", None)),
                Ok(lagging_value)
            );
            assert_eq!(
                call(fresh_url, &now, LIVE_TTL_MILLIS, &calls, |_| changed("other", None)),
                Ok(fresh_value)
            );
            assert_eq!(calls.get(), 2);
            // 先失效再重读：滞后的一侧带 ETag 取得新快照，另一侧 304 确认，两侧重新对齐。
            invalidate_all();
            assert_eq!(
                call(lagging_url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                    assert_eq!(etag.as_deref(), Some(lagging_etag));
                    changed("live", Some("etag-new"))
                }),
                Ok("live")
            );
            assert_eq!(
                call(fresh_url, &now, LIVE_TTL_MILLIS, &calls, |etag| {
                    assert_eq!(etag.as_deref(), Some("etag-fresh"));
                    FetchOutcome::NotModified
                }),
                Ok(fresh_value)
            );
            assert_eq!(calls.get(), 4);
        }
    }

    /// 回退标记取默认值即权威读取；只判断是否为暂时性失败后的回退。
    fn is_fallback<M: Default + PartialEq>(marker: M) -> bool {
        marker != M::default()
    }

    /// 历史深链由 lot 外壳与往期目录拼出场次：两者同样只在失效后重新验证；重读遇到 503 时回退
    /// 并标明旧快照，看板据此仍判不对齐，有界用尽后显示可重试的错误。
    #[test]
    fn historical_deep_link_rereads_revalidate_and_busy_falls_back_marked_stale() {
        reset();
        let directory = "/api/v1/auctions?page=1&page_size=200&status=settled";
        let lots = "/api/v1/auctions/00000000-0000-0000-0000-000000000011/lots?page=1&page_size=20";
        let read = |url: &str, ttl: u64, outcome: FetchOutcome<&'static str, &'static str>| {
            block_on(read_or_fetch(url, ttl, || 200_000, |_| std::future::ready(outcome)))
                .map(|cached| (cached.value, is_fallback(cached.stale_fallback)))
        };
        assert_eq!(
            read(directory, CATALOG_TTL_MILLIS, changed("final", Some("d1"))),
            Ok(("final", false))
        );
        assert_eq!(read(lots, LIVE_TTL_MILLIS, changed("final", Some("l1"))), Ok(("final", false)));
        // TTL 内未失效：新鲜命中，不会调用 fetch（这里给出的拒绝不会被使用）。
        assert_eq!(
            read(directory, CATALOG_TTL_MILLIS, FetchOutcome::Rejected("unused")),
            Ok(("final", false))
        );
        invalidate_all();
        // 失效后真实再验证：结算后的目录返回新快照。
        assert_eq!(
            read(directory, CATALOG_TTL_MILLIS, changed("settled", Some("d2"))),
            Ok(("settled", false))
        );
        // lot 外壳重读遇到 503：回退并标明旧快照，不当作已对齐的权威值。
        assert_eq!(
            read(lots, LIVE_TTL_MILLIS, FetchOutcome::Busy("read_busy")),
            Ok(("final", true))
        );
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
