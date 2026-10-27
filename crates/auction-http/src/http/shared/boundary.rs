//! Public 与 Admin 共用的 HTTP 请求边界。
//! 本模块只处理协议级关联、可信代理、安全响应头、超时、panic 隔离和访问日志。业务认证、
//! origin/CSRF 与命令幂等仍由各自 handler 和应用端口负责。

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};

use auction_contracts::ApiErrorResponse;
use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::FutureExt;
use ipnet::IpNet;
use tracing::Instrument;
use uuid::Uuid;

const REQUEST_ID_HEADER: &str = "x-request-id";
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// 一个 surface 的非业务 HTTP 边界配置。
#[derive(Clone)]
pub struct HttpBoundary {
    request_timeout: Duration,
    // 策略在启动后只读；middleware 和 surface state 克隆时只共享这一份已校验网段。
    trusted_proxy_cidrs: Arc<[IpNet]>,
    trusted_unix_proxy: bool,
}

impl Default for HttpBoundary {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(15),
            trusted_proxy_cidrs: Arc::default(),
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
        Ok(Self { request_timeout, trusted_proxy_cidrs: networks.into(), trusted_unix_proxy })
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

    let method = request.method();
    let route_pattern =
        request.extensions().get::<MatchedPath>().map(MatchedPath::as_str).unwrap_or("");
    let traceparent = single_header(request.headers(), "traceparent");
    let tracestate = single_header(request.headers(), "tracestate");
    let started = Instant::now();
    let span = tracing::info_span!(
        "http.request",
        request_id = %request_id,
        method = %method,
        route_pattern = %route_pattern,
        client_address = %client_address,
        status = tracing::field::Empty,
    );
    auction_observability::attach_remote_trace_context(&span, traceparent, tracestate);

    let response_request_id = request_id.as_str();
    let response = async move {
        let run = AssertUnwindSafe(next.run(request)).catch_unwind();
        // 超时覆盖 WebSocket 握手；on_upgrade 已独立持有长连接，不受请求计时器约束。
        match tokio::time::timeout(boundary.request_timeout, run).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                tracing::error!("HTTP handler panic");
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "服务暂时不可用",
                    response_request_id,
                )
            }
            Err(_) => {
                tracing::warn!(
                    timeout_seconds = boundary.request_timeout.as_secs(),
                    "HTTP request timed out"
                );
                error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "timeout",
                    "请求处理超时",
                    response_request_id,
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

/// 仅业务 API 路由使用：把 Path/Bytes 等默认纯文本拒绝收敛为稳定 JSON 外壳。
/// 不提前提取参数或正文，保留原有认证、限流、状态码及已经映射的 JSON 错误语义。
pub(crate) async fn normalize_api_rejection(request: Request, next: Next) -> Response {
    let is_head = request.method() == Method::HEAD;
    let request_id = request_id(request.headers());
    let response = next.run(request).await;
    let plain_text = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/plain"));
    if !plain_text {
        return response;
    }
    let (code, message) = match response.status() {
        StatusCode::BAD_REQUEST => ("invalid_query", "请求参数无效"),
        StatusCode::PAYLOAD_TOO_LARGE => ("body_too_large", "请求体过大"),
        StatusCode::INTERNAL_SERVER_ERROR => ("internal_error", "服务内部错误"),
        _ => return response,
    };
    let (mut parts, _) = response.into_parts();
    // 原始框架正文可能包含参数或实现细节，直接丢弃，不能复制其长度、编码或实体校验值。
    for name in [
        "content-length",
        "content-encoding",
        "transfer-encoding",
        "content-range",
        "content-md5",
        "digest",
        "etag",
        "last-modified",
    ] {
        parts.headers.remove(name);
    }
    parts.headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let body = if is_head {
        Body::empty()
    } else {
        Json(ApiErrorResponse {
            code: code.to_owned(),
            message: message.to_owned(),
            data: None,
            request_id,
        })
        .into_response()
        .into_body()
    };
    Response::from_parts(parts, body)
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
    let peer_is_trusted = match peer {
        None => boundary.trusted_unix_proxy,
        Some(_) => boundary.trusted_proxy_cidrs.iter().any(|network| network.contains(&fallback)),
    };
    if !peer_is_trusted {
        return fallback;
    }
    forwarded_address(headers, boundary).unwrap_or(fallback)
}

fn forwarded_address(headers: &HeaderMap, boundary: &HttpBoundary) -> Option<IpAddr> {
    let mut first = None;
    let mut client = None;
    // 直接倒序借用各头与地址段，不分配整条地址链；找到客户端后仍须校验剩余左侧，
    // 保留任意位置非法时整链回退、全部受信时使用最左地址的既有语义。
    for value in headers.get_all(FORWARDED_FOR_HEADER).iter().rev() {
        let value = value.to_str().ok()?;
        for raw in value.rsplit(',') {
            let address = raw.trim().parse::<IpAddr>().ok().map(canonical_ip)?;
            first = Some(address);
            if client.is_none()
                && !boundary.trusted_proxy_cidrs.iter().any(|network| network.contains(&address))
            {
                client = Some(address);
            }
        }
    }
    client.or(first)
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
    let cdn_policy = crate::assets::cdn_cache_policy(&response);
    let headers = response.headers_mut();
    // CDN 策略独立于浏览器 no-cache 契约；默认拒绝缓存，只有静态 handler 的私有标记可放行。
    headers.insert("cdn-cache-control", cdn_policy.clone());
    headers.insert("cloudflare-cdn-cache-control", cdn_policy);
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if let Ok(value) = HeaderValue::from_str(request_id) {
        headers.insert(REQUEST_ID_HEADER, value);
    }
    super::security::apply_browser_security_headers(headers);
    // `Date` 由 Hyper 的 HTTP/1 与 HTTP/2 编码器按秒缓存后统一补齐，这里不再逐响应格式化。
    super::countdown::apply_security_headers(&mut response);
    response
}
