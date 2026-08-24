//! Public 与 Admin 共用的 HTTP 请求边界。
//!
//! 本模块只处理协议级关联、可信代理、安全响应头、超时、panic 隔离和访问日志。业务认证、
//! origin/CSRF 与命令幂等仍由各自 handler 和应用端口负责。

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    panic::AssertUnwindSafe,
    time::{Duration, Instant},
};

use auction_contracts::ApiErrorResponse;
use axum::{
    Json,
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use futures_util::FutureExt;
use ipnet::IpNet;
use tracing::Instrument;
use uuid::Uuid;

const REQUEST_ID_HEADER: &str = "x-request-id";
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";
const WEBSOCKET_PATH: &str = "/api/v1/ws";

/// 一个 surface 的非业务 HTTP 边界配置。
#[derive(Clone)]
pub struct HttpBoundary {
    request_timeout: Duration,
    trusted_proxy_cidrs: Vec<IpNet>,
    trusted_unix_proxy: bool,
}

impl Default for HttpBoundary {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(15),
            trusted_proxy_cidrs: Vec::new(),
            trusted_unix_proxy: false,
        }
    }
}

impl HttpBoundary {
    /// 建立经过二次防御性校验的请求边界；配置层已校验的值在这里转成运行时网络对象。
    pub(crate) fn new(
        request_timeout: Duration,
        trusted_proxy_cidrs: &[String],
        trusted_unix_proxy: bool,
    ) -> Result<Self, &'static str> {
        if request_timeout.is_zero() || request_timeout > Duration::from_secs(120) {
            return Err("HTTP request timeout 配置无效");
        }
        if trusted_proxy_cidrs.len() > 64 {
            return Err("trusted proxy CIDR 数量无效");
        }
        let mut networks = Vec::with_capacity(trusted_proxy_cidrs.len());
        for value in trusted_proxy_cidrs {
            let network = value.parse::<IpNet>().map_err(|_| "trusted proxy CIDR 格式无效")?;
            if network.trunc().to_string() != *value || networks.contains(&network) {
                return Err("trusted proxy CIDR 必须规范且不能重复");
            }
            networks.push(network);
        }
        Ok(Self { request_timeout, trusted_proxy_cidrs: networks, trusted_unix_proxy })
    }
}

/// 执行 Public/Admin 共用的协议边界。
pub(crate) async fn enforce(
    State(boundary): State<HttpBoundary>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = match normalize_request_id(request.headers_mut()) {
        Ok(request_id) => request_id,
        Err(generated) => {
            return finish_response(
                error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_request_id",
                    "请求标识无效",
                    &generated,
                ),
                &generated,
            );
        }
    };
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|value| value.0);
    let client_address = resolve_client_address(peer, request.headers(), &boundary);
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(client_address, peer.map_or(0, |value| value.port()))));

    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let route_pattern =
        request.extensions().get::<MatchedPath>().map(MatchedPath::as_str).unwrap_or("").to_owned();
    let traceparent = single_header(request.headers(), "traceparent").map(str::to_owned);
    let tracestate = single_header(request.headers(), "tracestate").map(str::to_owned);
    let started = Instant::now();
    let span = tracing::info_span!(
        "http.request",
        request_id = %request_id,
        method = %method,
        route_pattern = %route_pattern,
        client_address = %client_address,
        status = tracing::field::Empty,
    );
    auction_observability::attach_remote_trace_context(
        &span,
        traceparent.as_deref(),
        tracestate.as_deref(),
    );

    let response_request_id = request_id.clone();
    let response = async move {
        let run = AssertUnwindSafe(next.run(request)).catch_unwind();
        if path == WEBSOCKET_PATH {
            return match run.await {
                Ok(response) => response,
                Err(_) => {
                    tracing::error!("HTTP handler panic");
                    error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal_error",
                        "服务暂时不可用",
                        &response_request_id,
                    )
                }
            };
        }
        match tokio::time::timeout(boundary.request_timeout, run).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                tracing::error!("HTTP handler panic");
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "服务暂时不可用",
                    &response_request_id,
                )
            }
            Err(_) => {
                tracing::warn!(
                    timeout_seconds = boundary.request_timeout.as_secs(),
                    "HTTP request timed out"
                );
                error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "request_timeout",
                    "请求处理超时",
                    &response_request_id,
                )
            }
        }
    }
    .instrument(span.clone())
    .await;

    let status = response.status();
    span.record("status", status.as_u16());
    tracing::info!(
        parent: &span,
        status = status.as_u16(),
        duration_ms = started.elapsed().as_millis() as u64,
        "HTTP request completed"
    );
    finish_response(response, &request_id)
}

/// 返回请求边界规范化后的关联标识；handler 只使用该值写入命令审计。
pub(crate) fn request_id(headers: &HeaderMap) -> Option<String> {
    single_header(headers, REQUEST_ID_HEADER)
        .filter(|value| valid_request_id(value))
        .map(str::to_owned)
}

fn normalize_request_id(headers: &mut HeaderMap) -> Result<String, String> {
    let values = headers.get_all(REQUEST_ID_HEADER);
    let mut values = values.iter();
    let first = values.next();
    let duplicate = values.next().is_some();
    let request_id = match (first, duplicate) {
        (None, false) => generated_request_id(),
        (Some(value), false) => {
            value.to_str().ok().filter(|value| valid_request_id(value)).map(str::to_owned)
        }
        _ => None,
    };
    let Some(request_id) = request_id else {
        return Err(generated_request_id().unwrap_or_else(|| "request-id-unavailable".to_owned()));
    };
    let value = HeaderValue::from_str(&request_id).map_err(|_| {
        generated_request_id().unwrap_or_else(|| "request-id-unavailable".to_owned())
    })?;
    headers.insert(REQUEST_ID_HEADER, value);
    Ok(request_id)
}

fn generated_request_id() -> Option<String> {
    let value = Uuid::new_v4().simple().to_string();
    valid_request_id(&value).then_some(value)
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_' | b'.'))
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let values = headers.get_all(name);
    let mut values = values.iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn resolve_client_address(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    boundary: &HttpBoundary,
) -> IpAddr {
    let fallback =
        peer.map(|value| canonical_ip(value.ip())).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let peer_is_trusted = boundary.trusted_unix_proxy
        || boundary.trusted_proxy_cidrs.iter().any(|network| network.contains(&fallback));
    if !peer_is_trusted {
        return fallback;
    }
    let Some(forwarded) = forwarded_addresses(headers) else {
        return fallback;
    };
    for candidate in forwarded.iter().rev() {
        if !boundary.trusted_proxy_cidrs.iter().any(|network| network.contains(candidate)) {
            return *candidate;
        }
    }
    forwarded.first().copied().unwrap_or(fallback)
}

fn forwarded_addresses(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let mut result = Vec::new();
    for value in headers.get_all(FORWARDED_FOR_HEADER) {
        let value = value.to_str().ok()?;
        for raw in value.split(',') {
            let address = raw.trim().parse::<IpAddr>().ok().map(canonical_ip)?;
            result.push(address);
        }
    }
    Some(result)
}

fn canonical_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => {
            address.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(address))
        }
        address => address,
    }
}

fn error_response(
    status: StatusCode,
    code: &'static str,
    message: &str,
    request_id: &str,
) -> Response {
    (
        status,
        Json(ApiErrorResponse {
            code: code.to_owned(),
            message: message.to_owned(),
            data: None,
            request_id: Some(request_id.to_owned()),
        }),
    )
        .into_response()
}

fn finish_response(mut response: Response, request_id: &str) -> Response {
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(request_id) {
        headers.insert(REQUEST_ID_HEADER, value);
    }
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        // Dioxus 的 WASM 启动需要 `wasm-unsafe-eval`；两端 index 中的首屏防闪样式和组件
        // style 属性需要 `style-src 'unsafe-inline'`，脚本仍只允许同源外部资源。
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), geolocation=(), microphone=()"),
    );
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    if !headers.contains_key(header::DATE) {
        let date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        if let Ok(value) = HeaderValue::from_str(&date) {
            headers.insert(header::DATE, value);
        }
    }
    response
}
