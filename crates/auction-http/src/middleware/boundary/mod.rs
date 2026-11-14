//! Public 与 Admin 共用的 HTTP 请求边界。
//! 本模块处理协议级关联、可信代理、指标访问、安全响应头、超时、panic 隔离和访问观测。业务认证、
//! origin/CSRF 与命令幂等仍由各自 handler 和应用端口负责。

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};

use auction_application::core::{IpCidr, diagnostics::DiagnosticContext};
use auction_contracts::ApiErrorResponse;
use auction_observability::{HttpRecorder, METRICS_ROUTE};
use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::FutureExt;
use tracing::Instrument;
use uuid::Uuid;

use crate::diagnostics;

const REQUEST_ID_HEADER: &str = "x-request-id";
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";
const FORWARDED_PROTO_HEADER: &str = "x-forwarded-proto";

/// 一个 surface 的非业务 HTTP 边界配置。
#[derive(Clone)]
pub struct HttpBoundary {
    request_timeout: Duration,
    // 策略在启动后只读；middleware 和 surface state 克隆时只共享这一份已校验网段。
    trusted_proxy_cidrs: Arc<[IpCidr]>,
    trusted_unix_proxy: bool,
    metrics: HttpRecorder,
    runtime_switches: Option<auction_application::runtime_switches::RuntimeSwitches>,
}

impl Default for HttpBoundary {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(15),
            trusted_proxy_cidrs: Arc::default(),
            trusted_unix_proxy: false,
            metrics: HttpRecorder::default(),
            runtime_switches: None,
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
            let network = value.parse::<IpCidr>().map_err(|_| "trusted proxy CIDR 格式无效")?;
            if network.prefix_length() == 0 {
                return Err("trusted proxy CIDR 不能信任所有地址");
            }
            if network.to_string() != *value || networks.contains(&network) {
                return Err("trusted proxy CIDR 必须规范且不能重复");
            }
            networks.push(network);
        }
        Ok(Self {
            request_timeout,
            trusted_proxy_cidrs: networks.into(),
            trusted_unix_proxy,
            ..Self::default()
        })
    }

    /// 注入进程共享的运行时开关快照；指标访问策略在每次请求入口读取一次。
    pub fn with_runtime_switches(
        mut self,
        switches: auction_application::runtime_switches::RuntimeSwitches,
    ) -> Self {
        self.runtime_switches = Some(switches);
        self
    }

    pub(crate) fn with_metrics(mut self, metrics: HttpRecorder) -> Self {
        self.metrics = metrics;
        self
    }
}

/// 执行 Public/Admin 共用的协议边界。
pub(crate) async fn enforce(
    State(boundary): State<HttpBoundary>,
    request: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    // 只克隆框架模板的共享句柄；fallback 统一聚合，原始路径与 query 永不进入 label。
    let route = request.extensions().get::<MatchedPath>().cloned();
    let response = enforce_request(&boundary, request, next).await;
    boundary.metrics.observe_request(
        route.as_ref().map_or("__unmatched__", MatchedPath::as_str),
        method.as_str(),
        response.status().as_u16(),
        started.elapsed(),
    );
    response
}

async fn enforce_request(boundary: &HttpBoundary, mut request: Request, next: Next) -> Response {
    // 未注入快照时 fail closed；注入后每次请求只做一次目标化解析。
    let reject_forwarded_requests = boundary.runtime_switches.as_ref().is_none_or(
        auction_application::runtime_switches::RuntimeSwitches::metrics_reject_forwarded_requests,
    );
    let reject_metrics = reject_forwarded_requests
        && request.uri().path() == METRICS_ROUTE
        && crate::server::metrics::has_forwarding_headers(request.headers());
    // 浏览器侧协议只判定一次；拒绝、超时与 panic 出口也必须与成功响应使用同一结论。
    let hsts = request_is_https(&request, boundary);
    let request_id = normalize_request_id(request.headers_mut());
    // 访问收紧优先于 request-id 400；归一化失败仍只回传新生成的安全关联标识。
    if reject_metrics {
        let request_id = match request_id {
            Ok(request_id) | Err(request_id) => request_id,
        };
        return finish_response(StatusCode::NOT_FOUND.into_response(), &request_id, hsts);
    }
    let request_id = match request_id {
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
                hsts,
            );
        }
    };
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|value| value.0);
    let client_address = resolve_client_address(peer, request.headers(), boundary);
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(client_address, peer.map_or(0, |value| value.port()))));

    let method = request.method();
    let route_pattern = request.extensions().get::<MatchedPath>().map_or("", MatchedPath::as_str);
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

    let diagnostic_route = request.extensions().get::<MatchedPath>().cloned();
    let response_request_id = request_id.as_str();
    let body = async move {
        let run = AssertUnwindSafe(next.run(request)).catch_unwind();
        // 超时覆盖 WebSocket 握手；on_upgrade 已独立持有长连接，不受请求计时器约束。
        match tokio::time::timeout(boundary.request_timeout, run).await {
            Ok(Ok(response)) => {
                diagnostics::response_status(response.status());
                response
            }
            Ok(Err(diagnostic_cause)) => {
                diagnostics::error(
                    "http_boundary",
                    "catch_panic",
                    diagnostics::panic_kind(&diagnostic_cause),
                    DiagnosticContext::default(),
                );

                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "服务暂时不可用",
                    response_request_id,
                )
            }
            Err(diagnostic_cause) => {
                diagnostics::warn(
                    "http_boundary",
                    "request_timeout",
                    diagnostics::timeout_kind(&diagnostic_cause),
                    DiagnosticContext::default(),
                );

                error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "timeout",
                    "请求处理超时",
                    response_request_id,
                )
            }
        }
    };
    let response = diagnostics::scope(diagnostic_route, body).instrument(span.clone()).await;

    let status = response.status();
    span.record("status", status.as_u16());
    tracing::info!(
        parent: &span,
        status = status.as_u16(),
        duration_ms = started.elapsed().as_millis() as u64,
        "HTTP request completed"
    );
    finish_response(response, &request_id, hsts)
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
        (None, false) => Some(generated_request_id()),
        (Some(value), false) => {
            value.to_str().ok().filter(|value| valid_request_id(value)).map(str::to_owned)
        }
        _ => None,
    };
    let Some(request_id) = request_id else {
        return Err(generated_request_id());
    };
    // 合法值只含 ASCII 字母数字与 `-_.`，必然是有效头值。
    let value = HeaderValue::from_str(&request_id).map_err(|_| generated_request_id())?;
    headers.insert(REQUEST_ID_HEADER, value);
    Ok(request_id)
}

/// 32 位小写十六进制 UUID 恒满足 `valid_request_id`，无需再次扫描或回退占位值。
fn generated_request_id() -> String {
    Uuid::new_v4().simple().to_string()
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
    let fallback = peer.map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |value| canonical_ip(value.ip()));
    if !peer_is_trusted(peer, boundary) {
        return fallback;
    }
    forwarded_address(headers, boundary).unwrap_or(fallback)
}

/// 直接连接的来源是否是我们配置的上游代理；只有它能提供转发证明。
fn peer_is_trusted(peer: Option<SocketAddr>, boundary: &HttpBoundary) -> bool {
    match peer {
        None => boundary.trusted_unix_proxy,
        Some(value) => {
            let address = canonical_ip(value.ip());
            boundary.trusted_proxy_cidrs.iter().any(|network| network.contains(address))
        }
    }
}

/// 浏览器实际使用的协议，只用来决定是否发送 HSTS。
///
/// 本进程不自行终止 TLS，反向代理后的请求是明文 HTTP；因此只有来自可信上游的
/// `X-Forwarded-Proto` 才能证明浏览器走的是 HTTPS。直接以绝对形式请求 URI 且
/// scheme 为 `https` 时同样视为安全（前置 TLS 终止器可以直接改写请求行）。
///
/// 任何歧义都按明文处理：未受信来源、缺失、非法、重复或链中混有非 https 值一律不发送，
/// 因为明文响应上的 HSTS 本来会被浏览器忽略，少发只会退化为当前行为。
fn request_is_https(request: &Request, boundary: &HttpBoundary) -> bool {
    if request.uri().scheme_str() == Some("https") {
        return true;
    }
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|value| value.0);
    peer_is_trusted(peer, boundary) && forwarded_proto_is_https(request.headers())
}

fn forwarded_proto_is_https(headers: &HeaderMap) -> bool {
    let mut seen = false;
    for value in headers.get_all(FORWARDED_PROTO_HEADER).iter() {
        let Ok(value) = value.to_str() else {
            return false;
        };
        // 代理链可能以逗号追加；任一跳声明非 https 都不能代表整条链路是安全的。
        for token in value.split(',') {
            seen = true;
            if !token.trim().eq_ignore_ascii_case("https") {
                return false;
            }
        }
    }
    seen
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
                && !boundary.trusted_proxy_cidrs.iter().any(|network| network.contains(address))
            {
                client = Some(address);
            }
        }
    }
    client.or(first)
}

fn canonical_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address.to_ipv4_mapped().map_or(IpAddr::V6(address), IpAddr::V4),
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

fn finish_response(mut response: Response, request_id: &str, hsts: bool) -> Response {
    let cdn_policy = crate::assets::cdn_cache_policy(&response);
    // 共享缓存可以保存并重放的响应不带构建号：重放的副本只能说明生成它的那个进程，部署之后
    // 它会让已经是新版本的页面误判成“版本过期”。
    let replayable = cdn_policy != "no-store";
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
    if !replayable {
        // 部署后仍打开的旧页面靠这个响应头发现前后端构建号不一致；值已限定为 HTTP 头安全字符。
        // 会话、就绪、进行中场次的读取与全部命令响应都不可重放，足以覆盖偏差检测。
        headers.insert(
            crate::server::build_identity::BUILD_ID_HEADER,
            HeaderValue::from_static(crate::server::build_identity::build_id()),
        );
    }
    super::security::apply_browser_security_headers(headers, hsts);
    // `Date` 由 Hyper 的 HTTP/1 与 HTTP/2 编码器按秒缓存后统一补齐，这里无需逐响应格式化。
    crate::public::countdown::apply_security_headers(&mut response);
    response
}

#[cfg(test)]
mod tests;
