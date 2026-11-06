//! 首次设置的显式远程授权；只有指定代理可提供 HTTPS 转发证明。

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
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
    State(policy): State<Arc<FirstRunPolicy>>,
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
    // 远程模式已在 permitted 中要求可信代理与 `x-forwarded-proto: https`，浏览器经 HTTPS
    // 访问；本地回环模式是明文 HTTP，不发送 HSTS。
    auction_http::apply_browser_security_headers(response.headers_mut(), setup_is_https(&policy));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// 浏览器是否经 HTTPS 访问首次设置：只有显式远程代理模式在 TLS 之后，本地回环是明文。
fn setup_is_https(policy: &FirstRunPolicy) -> bool {
    policy.remote.is_some()
}

fn permitted(
    policy: &FirstRunPolicy,
    peer: SocketAddr,
    headers: &HeaderMap,
    method: &axum::http::Method,
) -> bool {
    let header = |name: &str| single_header(headers, name).ok().flatten();
    // 本地允许省略 Origin，但畸形值或重复头不能被折叠成“未提供”。
    let Ok(origin) = single_header(headers, "origin") else {
        return false;
    };
    if let Some(remote) = &policy.remote {
        peer.ip() == remote.proxy_ip
            && header("x-forwarded-proto") == Some("https")
            && header("host") == remote.origin.strip_prefix("https://")
            && (matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD)
                || origin == Some(remote.origin.as_str()))
    } else {
        // 本地模式也绑定 Host，防止任意域名解析到 loopback 后读取初始化页面。
        let default_port = policy.listen.port() == 80;
        let mut address = policy.listen.to_string();
        let mut localhost = format!("localhost:{}", policy.listen.port());
        // 浏览器省略 HTTP 默认端口；显式 :80 与省略形式仍绑定同一精确 Host/Origin。
        if default_port {
            address.truncate(address.len() - ":80".len());
            localhost.truncate(localhost.len() - ":80".len());
        }
        peer.ip().is_loopback()
            && header("host").is_some_and(|host| {
                let host =
                    if default_port { host.strip_suffix(":80").unwrap_or(host) } else { host };
                host == address || host == localhost
            })
            && origin.is_none_or(|origin| {
                let Some(origin) = origin.strip_prefix("http://") else {
                    return false;
                };
                let origin = if default_port {
                    origin.strip_suffix(":80").unwrap_or(origin)
                } else {
                    origin
                };
                origin == address || origin == localhost
            })
    }
}

/// 区分缺失与非法安全头；代理证明和令牌均不能接受歧义的首值。
pub(super) fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(());
    }
    value.to_str().map(Some).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;

    #[test]
    fn hsts_is_only_announced_for_remote_https_setup() {
        // 本地回环是明文 HTTP，浏览器会忽略 HSTS，因此不发送。
        assert!(!setup_is_https(&FirstRunPolicy::default()));
        let remote = FirstRunPolicy {
            remote: Some(RemoteSetupAccess {
                origin: "https://setup.example.test".to_owned(),
                proxy_ip: "127.0.0.1".parse().expect("proxy address"),
            }),
            ..FirstRunPolicy::default()
        };
        assert!(setup_is_https(&remote));
        // 远程模式必须能验证：origin 强制 HTTPS，代理地址不能是通配。
        assert!(remote.validate().is_ok());
    }

    #[test]
    fn remote_setup_rejects_plain_http_origin() {
        let remote = FirstRunPolicy {
            remote: Some(RemoteSetupAccess {
                origin: "http://setup.example.test".to_owned(),
                proxy_ip: "127.0.0.1".parse().expect("proxy address"),
            }),
            ..FirstRunPolicy::default()
        };
        assert!(remote.validate().is_err());
    }
}
