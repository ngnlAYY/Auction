//! Public 与 Admin HTTP 的 origin、Cookie 和进程内速率限制边界。

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::http::HeaderMap;
use url::Url;

use super::boundary::HttpBoundary;

mod admin_login;
mod headers;
#[cfg(test)]
mod rotation_tests;
use admin_login::AdminLoginDefense;
pub use headers::apply_browser_security_headers;

const MAX_RATE_LIMIT_ENTRIES: usize = 4096;

/// 浏览器错误信标在任一限流窗口内的每来源上限；与登录、普通 mutation 使用独立账本。
const BROWSER_ERROR_LIMIT_PER_WINDOW: u32 = 30;

/// 未显式配置时的匿名读每来源窗口上限与并发上限；并发上限由装配层按服务池进一步校正。
const DEFAULT_ANONYMOUS_READ_LIMIT: u32 = 300;
const DEFAULT_ANONYMOUS_READ_CONCURRENCY: usize = 8;

/// 装配根传入 HTTP 安全适配器的非敏感参数。
pub struct HttpSecurityOptions<'a> {
    /// 是否为浏览器 Cookie 设置 `Secure`。
    pub cookie_secure: bool,
    /// 是否启用精确 origin allowlist。
    pub origins_enabled: bool,
    /// 允许的精确 HTTP(S) origin。
    pub allowed_origins: &'a [String],
    /// 登录请求在固定窗口内的最大次数。
    pub login_limit: u32,
    /// 状态变更请求在固定窗口内的最大次数；Public realtime 连接尝试复用该数值，
    /// 但使用独立账本，不能消耗 mutation 配额。
    pub mutation_limit: u32,
    /// 匿名公开读取在固定窗口内的每来源最大次数；与登录、mutation 使用独立账本，
    /// 且 IPv6 按 /64 聚合。
    pub anonymous_read_limit: u32,
    /// 匿名公开读取的进程内并发上限；出价等写请求不占用该额度。
    pub anonymous_read_concurrency: u32,
    pub rate_limit_window: Duration,
    /// 普通 HTTP 请求的最长处理时间。
    pub request_timeout: Duration,
    /// 可以提供 `X-Forwarded-For` 的可信 TCP 代理网段。
    pub trusted_proxy_cidrs: &'a [String],
    /// 是否信任 Unix socket 上游提供的 `X-Forwarded-For`。
    pub trusted_unix_proxy: bool,
}

#[derive(Clone)]
pub struct PublicHttpSecurity {
    boundary: HttpBoundary,
    cookie_secure: bool,
    origins: OriginPolicy,
    login_limiter: RateLimiter,
    mutation_limiter: RateLimiter,
    realtime_connection_limiter: RateLimiter,
    browser_error_limiter: RateLimiter,
    // 读闸门与写路径完全独立：匿名读突发只能等待自己的许可，不会占用出价连接。
    anonymous_read_gate: AnonymousReadGate,
}

/// 匿名读闸门的可克隆句柄；只共享来源额度与并发许可，不携带其他 HTTP 策略。
#[derive(Clone)]
pub struct AnonymousReadGate {
    limiter: RateLimiter,
    permits: Arc<tokio::sync::Semaphore>,
}

impl AnonymousReadGate {
    fn new(limit: u32, window: Duration, concurrency: usize) -> Self {
        Self {
            limiter: RateLimiter::new(limit, window),
            permits: Arc::new(tokio::sync::Semaphore::new(concurrency)),
        }
    }

    /// `None` 表示允许，`Some` 是拒绝后的最短重试秒数；IPv6 按 /64 聚合。
    pub(crate) fn allow(&self, address: IpAddr) -> Option<u64> {
        self.limiter.allow(address)
    }

    /// 非阻塞取得一个并发许可；无许可时调用方立即拒绝，不排入等待队列。
    pub(crate) fn try_acquire(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(&self.permits).try_acquire_owned().ok()
    }
}

impl Default for PublicHttpSecurity {
    fn default() -> Self {
        Self {
            boundary: HttpBoundary::default(),
            cookie_secure: false,
            origins: OriginPolicy::disabled(),
            login_limiter: RateLimiter::new(10, Duration::from_secs(60)),
            mutation_limiter: RateLimiter::new(60, Duration::from_secs(60)),
            realtime_connection_limiter: RateLimiter::new(60, Duration::from_secs(60)),
            browser_error_limiter: RateLimiter::new(
                BROWSER_ERROR_LIMIT_PER_WINDOW,
                Duration::from_secs(60),
            ),
            anonymous_read_gate: AnonymousReadGate::new(
                DEFAULT_ANONYMOUS_READ_LIMIT,
                Duration::from_secs(60),
                DEFAULT_ANONYMOUS_READ_CONCURRENCY,
            ),
        }
    }
}

impl PublicHttpSecurity {
    /// 从配置建立 Public HTTP 安全策略；origin 校验失败时在启动期直接拒绝。
    pub fn new(options: HttpSecurityOptions<'_>) -> Result<Self, &'static str> {
        let HttpSecurityOptions {
            cookie_secure,
            origins_enabled,
            allowed_origins,
            login_limit,
            mutation_limit,
            anonymous_read_limit,
            anonymous_read_concurrency,
            rate_limit_window,
            request_timeout,
            trusted_proxy_cidrs,
            trusted_unix_proxy,
        } = options;
        let origins = OriginPolicy::new(origins_enabled, allowed_origins)?;
        let boundary = HttpBoundary::new(request_timeout, trusted_proxy_cidrs, trusted_unix_proxy)?;
        if login_limit == 0
            || mutation_limit == 0
            || anonymous_read_limit == 0
            || anonymous_read_concurrency == 0
            || rate_limit_window.is_zero()
        {
            return Err("HTTP 速率限制配置无效");
        }
        let read_concurrency = usize::try_from(anonymous_read_concurrency)
            .map_err(|_: std::num::TryFromIntError| "HTTP 速率限制配置无效")?;
        Ok(Self {
            boundary,
            cookie_secure,
            origins,
            login_limiter: RateLimiter::new(login_limit, rate_limit_window),
            mutation_limiter: RateLimiter::new(mutation_limit, rate_limit_window),
            realtime_connection_limiter: RateLimiter::new(mutation_limit, rate_limit_window),
            browser_error_limiter: RateLimiter::new(
                BROWSER_ERROR_LIMIT_PER_WINDOW,
                rate_limit_window,
            ),
            anonymous_read_gate: AnonymousReadGate::new(
                anonymous_read_limit,
                rate_limit_window,
                read_concurrency,
            ),
        })
    }

    pub(crate) fn cookie_secure(&self) -> bool {
        self.cookie_secure
    }

    pub(crate) fn allows_origin(&self, headers: &HeaderMap) -> bool {
        self.origins.allows(headers)
    }

    pub(crate) fn allows_websocket_origin(&self, headers: &HeaderMap) -> bool {
        self.origins.allows_websocket(headers)
    }

    pub(crate) fn boundary(&self) -> HttpBoundary {
        self.boundary.clone()
    }

    pub(crate) fn allow_login(&self, address: IpAddr) -> Option<u64> {
        self.login_limiter.allow(address)
    }

    pub(crate) fn allow_mutation(&self, address: IpAddr) -> Option<u64> {
        self.mutation_limiter.allow(address)
    }

    /// 路由装配使用的闸门句柄；不暴露其余安全策略。
    pub(crate) fn anonymous_read_gate(&self) -> AnonymousReadGate {
        self.anonymous_read_gate.clone()
    }

    pub(crate) fn allow_realtime_connection(&self, address: IpAddr) -> Option<u64> {
        self.realtime_connection_limiter.allow(address)
    }

    /// 信标使用独立账本，不能消耗登录或业务 mutation 配额。
    pub(crate) fn allow_browser_error(&self, address: IpAddr) -> Option<u64> {
        self.browser_error_limiter.allow(address)
    }
}

/// Admin surface 的 HTTP 安全策略。
/// Admin 与 Public 使用相同的基础 origin/速率限制实现，但保持独立类型，避免管理路由误用
/// Public session 或把两个 surface 的策略状态混在同一个 state 中。
#[derive(Clone, Default)]
pub struct AdminHttpSecurity {
    inner: PublicHttpSecurity,
    login: AdminLoginDefense,
}

impl AdminHttpSecurity {
    /// 从配置建立 Admin HTTP 安全策略；校验失败时在启动期拒绝。
    pub fn new(options: HttpSecurityOptions<'_>) -> Result<Self, &'static str> {
        let login = AdminLoginDefense::new(options.login_limit, options.rate_limit_window);
        Ok(Self { inner: PublicHttpSecurity::new(options)?, login })
    }

    pub(crate) fn cookie_secure(&self) -> bool {
        self.inner.cookie_secure()
    }

    pub(crate) fn allows_origin(&self, headers: &HeaderMap) -> bool {
        self.inner.allows_origin(headers)
    }

    pub(crate) fn boundary(&self) -> HttpBoundary {
        self.inner.boundary()
    }

    pub(crate) fn allow_login(&self, address: IpAddr) -> Option<u64> {
        self.login.allow().or_else(|| self.inner.allow_login(address))
    }

    /// 无效凭据按共享账号计数，不允许更换来源绕过退避。
    pub(crate) fn login_failed(&self) {
        self.login.failed();
    }

    pub(crate) fn login_succeeded(&self) {
        self.login.succeeded();
    }

    pub(crate) fn allow_mutation(&self, address: IpAddr) -> Option<u64> {
        self.inner.allow_mutation(address)
    }

    pub(crate) fn allow_browser_error(&self, address: IpAddr) -> Option<u64> {
        self.inner.allow_browser_error(address)
    }
}

#[derive(Clone)]
struct OriginPolicy {
    disabled: bool,
    allowed: Arc<Vec<String>>,
}

impl OriginPolicy {
    fn disabled() -> Self {
        Self { disabled: true, allowed: Arc::new(Vec::new()) }
    }

    fn new(enabled: bool, values: &[String]) -> Result<Self, &'static str> {
        if !enabled {
            return Ok(Self::disabled());
        }
        if values.is_empty() || values.len() > 16 {
            return Err("allowed_origins 数量无效");
        }
        let mut allowed = Vec::with_capacity(values.len());
        for value in values {
            let parsed =
                Url::parse(value).map_err(|_: url::ParseError| "allowed_origins 格式无效")?;
            let normalized_value = value.strip_suffix('/').unwrap_or(value);
            let serialized_value = parsed.as_str();
            let serialized_value = serialized_value.strip_suffix('/').unwrap_or(serialized_value);
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || parsed.username() != ""
                || parsed.password().is_some()
                || !(parsed.path().is_empty() || parsed.path() == "/")
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || serialized_value != normalized_value
                || allowed.iter().any(|item| item == normalized_value)
            {
                return Err("allowed_origins 必须是唯一的精确 scheme+host");
            }
            allowed.push(normalized_value.to_owned());
        }
        Ok(Self { disabled: false, allowed: Arc::new(allowed) })
    }

    fn allows(&self, headers: &HeaderMap) -> bool {
        if self.disabled {
            return true;
        }
        let origins = headers.get_all("origin");
        let mut values = origins.iter();
        let Some(origin) = values.next() else {
            return true;
        };
        if values.next().is_some() {
            return false;
        }
        origin.to_str().is_ok_and(|value| self.allowed.iter().any(|item| item == value))
    }

    /// 旧 v1 WebSocket 在启用 allowlist 时要求请求必须且只能携带一个可信 Origin。
    fn allows_websocket(&self, headers: &HeaderMap) -> bool {
        if self.disabled {
            return true;
        }
        let origins = headers.get_all("origin");
        let mut values = origins.iter();
        let Some(origin) = values.next() else {
            return false;
        };
        if values.next().is_some() {
            return false;
        }
        origin.to_str().is_ok_and(|value| self.allowed.iter().any(|item| item == value))
    }
}

#[derive(Clone)]
struct RateLimiter {
    limit: u32,
    window: Duration,
    entries: Arc<Mutex<HashMap<IpAddr, RateWindow>>>,
    max_entries: usize,
}

#[derive(Clone, Copy)]
struct RateWindow {
    started: Instant,
    last_seen: Instant,
    count: u32,
}

impl RateLimiter {
    fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            entries: Arc::new(Mutex::new(HashMap::new())),
            max_entries: MAX_RATE_LIMIT_ENTRIES,
        }
    }

    /// 消耗一个地址的固定窗口额度；`None` 表示允许，`Some` 是拒绝后的最短重试秒数。
    fn allow(&self, address: IpAddr) -> Option<u64> {
        let address = rate_limit_key(address);
        let Ok(mut entries) = self.entries.lock() else {
            return Some(self.window.as_secs().max(1));
        };
        // 以锁内顺序取时间，防止并发请求按旧时刻覆盖较新的 last_seen 或窗口起点。
        let now = Instant::now();
        if let Some(value) = entries.get_mut(&address) {
            if now.duration_since(value.started) >= self.window {
                *value = RateWindow { started: now, last_seen: now, count: 1 };
                return None;
            }
            value.last_seen = now;
            if value.count >= self.limit {
                let remaining = self.window.saturating_sub(now.duration_since(value.started));
                return Some(remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0));
            }
            value.count += 1;
            return None;
        }
        if entries.len() >= self.max_entries {
            // 窗口已过期的记录与缺失记录等价（下次访问会重置），可整批移除而不改变限流结果；
            // 同一次遍历记录最久未见者，全部活跃时无需二次扫描满表。时间相同时仍按 IP 排序。
            let mut oldest: Option<(Instant, IpAddr)> = None;
            entries.retain(|address, value| {
                if now.duration_since(value.started) >= self.window {
                    return false;
                }
                let candidate = (value.last_seen, *address);
                oldest = Some(oldest.map_or(candidate, |previous| previous.min(candidate)));
                true
            });
            if entries.len() >= self.max_entries
                && let Some((_, address)) = oldest
            {
                entries.remove(&address);
            }
        }
        entries.insert(address, RateWindow { started: now, last_seen: now, count: 1 });
        None
    }
}

// IPv4-mapped IPv6 必须与 IPv4 共用键；原生 IPv6 在 /64 内共用额度。
fn rate_limit_key(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or_else(|| IpAddr::V6((u128::from(address) & (u128::MAX << 64)).into())),
        address => address,
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::middleware::security as parent;

    #[test]
    fn ipv6_addresses_share_a_64_bit_rate_limit_key() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        assert_eq!(limiter.allow("2001:db8:1:2::1".parse().unwrap()), None);
        assert!(limiter.allow("2001:db8:1:2:ffff::2".parse().unwrap()).is_some());
        assert_eq!(limiter.allow("2001:db8:1:3::1".parse().unwrap()), None);
    }

    #[test]
    fn mapped_ipv6_cannot_bypass_ipv4_rate_limit() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        assert_eq!(limiter.allow("192.0.2.1".parse().unwrap()), None);
        assert!(limiter.allow("::ffff:192.0.2.1".parse().unwrap()).is_some());
    }

    #[test]
    fn admin_login_quota_and_failure_backoff_ignore_rotating_sources() {
        let security = AdminHttpSecurity::default();
        for suffix in 1..=10 {
            assert_eq!(security.allow_login(IpAddr::from([192, 0, 2, suffix])), None);
        }
        assert!(security.allow_login("198.51.100.1".parse().unwrap()).is_some());
        let failures = AdminHttpSecurity::default();
        for _ in 0..5 {
            failures.login_failed();
        }
        assert!(failures.allow_login("198.51.100.2".parse().unwrap()).is_some());
        failures.login_succeeded();
        assert_eq!(failures.allow_login("198.51.100.3".parse().unwrap()), None);
    }
}
