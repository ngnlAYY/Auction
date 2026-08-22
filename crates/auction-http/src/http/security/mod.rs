//! Public 与 Admin HTTP 的 origin、Cookie 和进程内速率限制边界。

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::http::HeaderMap;
use url::Url;

#[derive(Clone)]
pub struct PublicHttpSecurity {
    cookie_secure: bool,
    origins: OriginPolicy,
    login_limiter: RateLimiter,
    mutation_limiter: RateLimiter,
}

impl Default for PublicHttpSecurity {
    fn default() -> Self {
        Self {
            cookie_secure: false,
            origins: OriginPolicy::disabled(),
            login_limiter: RateLimiter::new(10, Duration::from_secs(60)),
            mutation_limiter: RateLimiter::new(60, Duration::from_secs(60)),
        }
    }
}

impl PublicHttpSecurity {
    /// 从配置建立 Public HTTP 安全策略；origin 校验失败时在启动期直接拒绝。
    pub fn new(
        cookie_secure: bool,
        origins_enabled: bool,
        allowed_origins: &[String],
        login_limit: u32,
        mutation_limit: u32,
        rate_limit_window: Duration,
    ) -> Result<Self, &'static str> {
        let origins = OriginPolicy::new(origins_enabled, allowed_origins)?;
        if login_limit == 0 || mutation_limit == 0 || rate_limit_window.is_zero() {
            return Err("HTTP 速率限制配置无效");
        }
        Ok(Self {
            cookie_secure,
            origins,
            login_limiter: RateLimiter::new(login_limit, rate_limit_window),
            mutation_limiter: RateLimiter::new(mutation_limit, rate_limit_window),
        })
    }

    pub(crate) fn cookie_secure(&self) -> bool {
        self.cookie_secure
    }

    pub(crate) fn allows_origin(&self, headers: &HeaderMap) -> bool {
        self.origins.allows(headers)
    }

    pub(crate) fn allow_login(&self, address: IpAddr) -> bool {
        self.login_limiter.allow(address)
    }

    pub(crate) fn allow_mutation(&self, address: IpAddr) -> bool {
        self.mutation_limiter.allow(address)
    }

    pub(crate) fn retry_after_seconds(&self) -> u64 {
        self.login_limiter.window.as_secs().max(1)
    }
}

/// Admin surface 的 HTTP 安全策略。
///
/// Admin 与 Public 使用相同的基础 origin/速率限制实现，但保持独立类型，避免管理路由误用
/// Public session 或把两个 surface 的策略状态混在同一个 state 中。
#[derive(Clone, Default)]
pub struct AdminHttpSecurity {
    inner: PublicHttpSecurity,
}

impl AdminHttpSecurity {
    /// 从配置建立 Admin HTTP 安全策略；校验失败时在启动期拒绝。
    pub fn new(
        cookie_secure: bool,
        origins_enabled: bool,
        allowed_origins: &[String],
        login_limit: u32,
        mutation_limit: u32,
        rate_limit_window: Duration,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            inner: PublicHttpSecurity::new(
                cookie_secure,
                origins_enabled,
                allowed_origins,
                login_limit,
                mutation_limit,
                rate_limit_window,
            )?,
        })
    }

    pub(crate) fn cookie_secure(&self) -> bool {
        self.inner.cookie_secure()
    }

    pub(crate) fn allows_origin(&self, headers: &HeaderMap) -> bool {
        self.inner.allows_origin(headers)
    }

    pub(crate) fn allow_login(&self, address: IpAddr) -> bool {
        self.inner.allow_login(address)
    }

    pub(crate) fn allow_mutation(&self, address: IpAddr) -> bool {
        self.inner.allow_mutation(address)
    }

    pub(crate) fn retry_after_seconds(&self) -> u64 {
        self.inner.retry_after_seconds()
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
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || parsed.username() != ""
                || parsed.password().is_some()
                || parsed.path() != ""
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.as_str() != value
                || allowed.iter().any(|item| item == value)
            {
                return Err("allowed_origins 必须是唯一的精确 scheme+host");
            }
            allowed.push(value.clone());
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
}

#[derive(Clone)]
struct RateLimiter {
    limit: u32,
    window: Duration,
    entries: Arc<Mutex<HashMap<IpAddr, RateWindow>>>,
}

#[derive(Clone, Copy)]
struct RateWindow {
    started: Instant,
    count: u32,
}

impl RateLimiter {
    fn new(limit: u32, window: Duration) -> Self {
        Self { limit, window, entries: Arc::new(Mutex::new(HashMap::new())) }
    }

    fn allow(&self, address: IpAddr) -> bool {
        let now = Instant::now();
        let Ok(mut entries) = self.entries.lock() else {
            return false;
        };
        entries.retain(|_, value| now.duration_since(value.started) < self.window);
        let value = entries.entry(address).or_insert(RateWindow { started: now, count: 0 });
        if now.duration_since(value.started) >= self.window {
            *value = RateWindow { started: now, count: 0 };
        }
        if value.count >= self.limit {
            return false;
        }
        value.count += 1;
        true
    }
}
