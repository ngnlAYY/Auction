//! Public/Admin 共用的低基数 HTTP、限流与 WebSocket 指标。

use std::{sync::Arc, time::Duration};

use prometheus::{
    HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
};

use super::types::MetricsError;

/// HTTP 频率限制的固定类别；容量、认证与业务拒绝不进入该计数。
#[derive(Clone, Copy)]
pub enum HttpRateLimitKind {
    Login,
    Mutation,
    Realtime,
    BrowserError,
}

impl HttpRateLimitKind {
    const ALL: [Self; 4] = [Self::Login, Self::Mutation, Self::Realtime, Self::BrowserError];

    const fn label(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Mutation => "mutation",
            Self::Realtime => "realtime",
            Self::BrowserError => "browser_error",
        }
    }
}

/// 浏览器错误信标的固定类别；只允许产品内定义的四类，原始错误正文不进入指标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserErrorClass {
    WasmPanic,
    DtoDecode,
    RealtimeProtocol,
    I18nFormat,
}

impl BrowserErrorClass {
    pub const fn label(self) -> &'static str {
        match self {
            Self::WasmPanic => "wasm_panic",
            Self::DtoDecode => "dto_decode",
            Self::RealtimeProtocol => "realtime_protocol",
            Self::I18nFormat => "i18n_format",
        }
    }
}

/// 可克隆的进程本地记录器；关闭指标或非 HTTP 角色使用无 collector 的 no-op。
#[derive(Clone, Default)]
pub struct HttpRecorder {
    inner: Option<Arc<PrometheusHttpRecorder>>,
}

struct PrometheusHttpRecorder {
    requests: IntCounterVec,
    durations: HistogramVec,
    rate_limit_rejections: [IntCounter; HttpRateLimitKind::ALL.len()],
    browser_errors: IntCounterVec,
    browser_error_build_mismatches: IntCounter,
    websocket: Option<WebSocketMetrics>,
}

struct WebSocketMetrics {
    connections: IntGauge,
    total: IntCounter,
}

impl HttpRecorder {
    pub(super) fn build(registry: &Registry, role: &'static str) -> Result<Self, MetricsError> {
        let role = match role {
            "public" => "public",
            "admin" => "admin",
            _ => return Ok(Self::default()),
        };
        let requests = IntCounterVec::new(
            Opts::new("auction_http_requests_total", "Total HTTP responses by route and status.")
                .const_label("role", role),
            &["route", "method", "status"],
        )
        .map_err(MetricsError::Registry)?;
        let durations = HistogramVec::new(
            HistogramOpts::new(
                "auction_http_request_duration_seconds",
                "HTTP response preparation duration in seconds.",
            )
            .const_label("role", role)
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 30.0]),
            &["route", "method"],
        )
        .map_err(MetricsError::Registry)?;
        let rejections = IntCounterVec::new(
            Opts::new(
                "auction_http_rate_limit_rejections_total",
                "Total HTTP frequency limit rejections by fixed kind.",
            )
            .const_label("role", role),
            &["kind"],
        )
        .map_err(MetricsError::Registry)?;
        let rate_limit_rejections =
            HttpRateLimitKind::ALL.map(|kind| rejections.with_label_values(&[kind.label()]));
        let browser_errors = IntCounterVec::new(
            Opts::new(
                "auction_browser_errors_total",
                "Total accepted browser error beacons by fixed class.",
            )
            .const_label("role", role),
            &["class"],
        )
        .map_err(MetricsError::Registry)?;
        let browser_error_build_mismatches = IntCounter::with_opts(
            Opts::new(
                "auction_browser_error_build_mismatches_total",
                "Total accepted browser error beacons whose build id is not the current process build.",
            )
            .const_label("role", role),
        )
        .map_err(MetricsError::Registry)?;
        registry.register(Box::new(requests.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(durations.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(rejections)).map_err(MetricsError::Registry)?;
        registry.register(Box::new(browser_errors.clone())).map_err(MetricsError::Registry)?;
        registry
            .register(Box::new(browser_error_build_mismatches.clone()))
            .map_err(MetricsError::Registry)?;
        let websocket = if role == "public" {
            let connections = IntGauge::with_opts(
                Opts::new(
                    "auction_websocket_connections",
                    "Current upgraded WebSocket connections.",
                )
                .const_label("role", role),
            )
            .map_err(MetricsError::Registry)?;
            let total = IntCounter::with_opts(
                Opts::new(
                    "auction_websocket_connections_total",
                    "Total successfully upgraded WebSocket connections.",
                )
                .const_label("role", role),
            )
            .map_err(MetricsError::Registry)?;
            registry.register(Box::new(connections.clone())).map_err(MetricsError::Registry)?;
            registry.register(Box::new(total.clone())).map_err(MetricsError::Registry)?;
            Some(WebSocketMetrics { connections, total })
        } else {
            None
        };
        Ok(Self {
            inner: Some(Arc::new(PrometheusHttpRecorder {
                requests,
                durations,
                rate_limit_rejections,
                browser_errors,
                browser_error_build_mismatches,
                websocket,
            })),
        })
    }

    /// 记录一次已生成的 HTTP 响应；route 只接受框架路由模板或固定缺省值，禁止传入原始 URI。
    pub fn observe_request(&self, route: &str, method: &str, status: u16, duration: Duration) {
        let Some(inner) = &self.inner else {
            return;
        };
        let method = match method {
            "GET" => "GET",
            "HEAD" => "HEAD",
            "POST" => "POST",
            "PUT" => "PUT",
            "PATCH" => "PATCH",
            "DELETE" => "DELETE",
            "OPTIONS" => "OPTIONS",
            "TRACE" => "TRACE",
            "CONNECT" => "CONNECT",
            _ => "OTHER",
        };
        // HTTP 状态码限定为协议的三位数字；非协议值聚合到固定占位，不能扩大标签空间。
        let status =
            if (100..=999).contains(&status) { status.to_string() } else { "OTHER".to_owned() };
        inner.requests.with_label_values(&[route, method, &status]).inc();
        inner.durations.with_label_values(&[route, method]).observe(duration.as_secs_f64());
    }

    /// 只在频率限制器实际拒绝时调用；该计数不参与额度或任何业务判定。
    pub fn observe_rate_limit_rejection(&self, kind: HttpRateLimitKind) {
        if let Some(inner) = &self.inner {
            inner.rate_limit_rejections[kind as usize].inc();
        }
    }

    /// 记录一条已通过格式与限流校验的浏览器错误信标；类别是闭集，不接收原始错误正文。
    pub fn observe_browser_error(&self, class: BrowserErrorClass) {
        if let Some(inner) = &self.inner {
            inner.browser_errors.with_label_values(&[class.label()]).inc();
        }
    }

    /// 构建号不一致只累加固定计数，不把上报的构建号写进标签，避免无界标签空间。
    pub fn observe_browser_error_build_mismatch(&self) {
        if let Some(inner) = &self.inner {
            inner.browser_error_build_mismatches.inc();
        }
    }

    /// 成功升级后登记连接；调用方必须让守卫跟随升级任务，覆盖取消与所有退出分支。
    pub fn websocket_connection(&self) -> WebSocketConnectionGuard {
        let connections =
            self.inner.as_ref().and_then(|inner| inner.websocket.as_ref()).map(|metrics| {
                metrics.connections.inc();
                metrics.total.inc();
                metrics.connections.clone()
            });
        WebSocketConnectionGuard { connections }
    }
}

/// 已升级连接的观测守卫；只维护指标，不持有或替代 Hub 配额与排空许可。
#[must_use = "连接指标守卫必须持有到 WebSocket 任务结束"]
pub struct WebSocketConnectionGuard {
    connections: Option<IntGauge>,
}

impl Drop for WebSocketConnectionGuard {
    fn drop(&mut self) {
        if let Some(connections) = &self.connections {
            connections.dec();
        }
    }
}
