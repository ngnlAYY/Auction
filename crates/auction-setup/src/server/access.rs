//! 首次设置的显式远程授权；只有指定代理可提供 HTTPS 转发证明。

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 初始化期间的监听与后续行为，不读取业务角色配置。
#[derive(Clone)]
pub struct FirstRunPolicy {
    pub listen: SocketAddr,
    pub remote: Option<RemoteSetupAccess>,
    pub initialize_and_start: bool,
}

impl Default for FirstRunPolicy {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 8091)),
            remote: None,
            initialize_and_start: false,
        }
    }
}

/// 显式信任的 HTTPS 反向代理；不允许任意来源自报转发协议。
#[derive(Clone)]
pub struct RemoteSetupAccess {
    pub origin: String,
    pub proxy_ip: IpAddr,
}

impl FirstRunPolicy {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.listen.port() == 0 {
            return Err("setup listener must use a fixed port");
        }
        if let Some(remote) = &self.remote {
            let url = url::Url::parse(&remote.origin).map_err(|_| "invalid setup HTTPS origin")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/"
                || url.origin().ascii_serialization() != remote.origin
                || remote.proxy_ip.is_unspecified()
                || remote.proxy_ip.is_multicast()
            {
                return Err("remote setup requires an exact HTTPS origin and a trusted proxy IP");
            }
        } else if !self.listen.ip().is_loopback() {
            return Err("non-loopback setup requires explicit remote HTTPS proxy configuration");
        }
        Ok(())
    }
}

pub(super) async fn enforce(
    State(policy): State<FirstRunPolicy>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = if !permitted(&policy, peer, request.headers(), request.method()) {
        super::error(StatusCode::FORBIDDEN, "setup_origin").into_response()
    } else {
        // 同时限制正文读取与异步处理；complete 的发布与终态确认之间没有可取消的 await。
        match tokio::time::timeout(REQUEST_TIMEOUT, next.run(request)).await {
            Ok(response) => response,
            Err(_) => super::error(StatusCode::GATEWAY_TIMEOUT, "request_timeout").into_response(),
        }
    };
    // 门禁拒绝、框架错误和超时也必须经过同一出口，不能跳过禁止缓存与防嵌入策略。
    auction_http::apply_browser_security_headers(response.headers_mut());
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn permitted(
    policy: &FirstRunPolicy,
    peer: SocketAddr,
    headers: &axum::http::HeaderMap,
    method: &axum::http::Method,
) -> bool {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    if let Some(remote) = &policy.remote {
        peer.ip() == remote.proxy_ip
            && header("x-forwarded-proto") == Some("https")
            && header("host") == remote.origin.strip_prefix("https://")
            && (matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD)
                || header("origin") == Some(remote.origin.as_str()))
    } else {
        // 本地模式也绑定 Host，防止任意域名解析到 loopback 后读取初始化页面。
        let address = policy.listen.to_string();
        let localhost = format!("localhost:{}", policy.listen.port());
        peer.ip().is_loopback()
            && matches!(header("host"), Some(host) if host == address || host == localhost)
            && header("origin").is_none_or(|origin| {
                origin == format!("http://{address}") || origin == format!("http://{localhost}")
            })
    }
}
