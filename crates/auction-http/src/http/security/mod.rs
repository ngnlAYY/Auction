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

mod headers;
pub use headers::apply_browser_security_headers;

const MAX_RATE_LIMIT_ENTRIES: usize = 4096;

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
            rate_limit_window,
            request_timeout,
            trusted_proxy_cidrs,
            trusted_unix_proxy,
        } = options;
        let origins = OriginPolicy::new(origins_enabled, allowed_origins)?;
        let boundary = HttpBoundary::new(request_timeout, trusted_proxy_cidrs, trusted_unix_proxy)?;
        if login_limit == 0 || mutation_limit == 0 || rate_limit_window.is_zero() {
            return Err("HTTP 速率限制配置无效");
        }
        Ok(Self {
            boundary,
            cookie_secure,
            origins,
            login_limiter: RateLimiter::new(login_limit, rate_limit_window),
            mutation_limiter: RateLimiter::new(mutation_limit, rate_limit_window),
            realtime_connection_limiter: RateLimiter::new(mutation_limit, rate_limit_window),
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

    pub(crate) fn allow_realtime_connection(&self, address: IpAddr) -> Option<u64> {
        self.realtime_connection_limiter.allow(address)
    }
}

/// Admin surface 的 HTTP 安全策略。
/// Admin 与 Public 使用相同的基础 origin/速率限制实现，但保持独立类型，避免管理路由误用
/// Public session 或把两个 surface 的策略状态混在同一个 state 中。
#[derive(Clone, Default)]
pub struct AdminHttpSecurity {
    inner: PublicHttpSecurity,
}

impl AdminHttpSecurity {
    /// 从配置建立 Admin HTTP 安全策略；校验失败时在启动期拒绝。
    pub fn new(options: HttpSecurityOptions<'_>) -> Result<Self, &'static str> {
        Ok(Self { inner: PublicHttpSecurity::new(options)? })
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
        self.inner.allow_login(address)
    }

    pub(crate) fn allow_mutation(&self, address: IpAddr) -> Option<u64> {
        self.inner.allow_mutation(address)
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
            let parsed = Url::parse(value).map_err(|_| "allowed_origins 格式无效")?;
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
            // 满表时一次清理即可摊还后续新地址的插入，只有全部活跃时才逐个淘汰最久未见者。
            entries.retain(|_, value| now.duration_since(value.started) < self.window);
        }
        if entries.len() >= self.max_entries
            && let Some(oldest) = entries
                .iter()
                .min_by(|(left_address, left), (right_address, right)| {
                    left.last_seen
                        .cmp(&right.last_seen)
                        .then_with(|| left_address.cmp(right_address))
                })
                .map(|(address, _)| *address)
        {
            entries.remove(&oldest);
        }
        entries.insert(address, RateWindow { started: now, last_seen: now, count: 1 });
        None
    }
}
