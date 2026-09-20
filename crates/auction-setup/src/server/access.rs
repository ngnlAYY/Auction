//! 首次设置的显式远程授权；只有指定代理可提供 HTTPS 转发证明。

use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

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
    let permitted = permitted(&policy, peer, request.headers(), request.method());
    if !permitted {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(auction_contracts::SetupErrorResponse { code: "setup_origin".to_owned() }),
        )
            .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", axum::http::HeaderValue::from_static("no-referrer"));
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
