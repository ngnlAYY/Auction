//! 回环源站上的逐文件 GET：状态、表示与大小都按“失败关闭”判定。

use std::net::IpAddr;

use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_SECURITY_POLICY};
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use url::{Host, Url};

use super::policy::{WebFetchPolicy, WebRepresentation};

/// 取回失败的原因；文本只含路径、状态与限额，不含响应内容。
#[derive(Debug, thiserror::Error)]
pub enum WebFetchError {
    #[error("源站地址无效：{0}")]
    InvalidOrigin(&'static str),
    #[error("取回限额无效：{0}")]
    InvalidPolicy(&'static str),
    #[error("请求路径不安全：{0}")]
    InvalidPath(String),
    #[error("创建 HTTP 客户端失败")]
    Build(#[source] reqwest::Error),
    #[error("请求失败：{path}")]
    Transport {
        path: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("源站对 {path} 返回了非 200 状态：{status}")]
    Status { path: String, status: u16 },
    #[error("{path} 的内容编码与所请求的表示不符：{actual}")]
    Encoding { path: String, actual: String },
    #[error("{path} 的响应超过大小上限 {limit} 字节")]
    TooLarge { path: String, limit: u64 },
}

/// 一次成功取回：原始实体字节与响应声明的页面脚本策略（仅供调用方核对源站身份）。
#[derive(Debug)]
pub struct WebFetched {
    pub body: Vec<u8>,
    pub content_security_policy: Option<String>,
}

/// 固定在一个回环 origin 上的客户端。
pub struct WebFetchClient {
    http: Client,
    origin: Url,
    policy: WebFetchPolicy,
}

impl WebFetchClient {
    /// `origin` 必须是只含 scheme、主机与端口的回环地址：取旧字节必须绕过 CDN 与代理，
    /// 直连旧二进制本机监听，不能指向任何公网主机。
    pub fn new(origin: &Url, policy: WebFetchPolicy) -> Result<Self, WebFetchError> {
        validate_origin(origin)?;
        policy.validate().map_err(WebFetchError::InvalidPolicy)?;
        let _ = rustls::crypto::ring::default_provider().install_default();
        // 未启用任何解压 feature，响应体保持源站发出的实体字节；重定向与代理一并关闭。
        let http = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(policy.connect_timeout)
            .timeout(policy.request_timeout)
            .build()
            .map_err(WebFetchError::Build)?;
        Ok(Self { http, origin: origin.clone(), policy })
    }

    /// 取回 `relative_path`（不含前导 `/`）的指定表示。
    ///
    /// 只接受 `200`；Brotli 要求 `Content-Encoding: br`，原文要求没有内容编码。
    pub async fn get(
        &self,
        relative_path: &str,
        representation: WebRepresentation,
    ) -> Result<WebFetched, WebFetchError> {
        validate_path(relative_path)?;
        let Ok(url) = self.origin.join(relative_path) else {
            return Err(WebFetchError::InvalidPath(relative_path.to_owned()));
        };
        let transport =
            |source| WebFetchError::Transport { path: relative_path.to_owned(), source };
        let mut response = self
            .http
            .get(url)
            .header(ACCEPT_ENCODING, representation.accept_encoding())
            .send()
            .await
            .map_err(transport)?;
        if response.status() != StatusCode::OK {
            return Err(WebFetchError::Status {
                path: relative_path.to_owned(),
                status: response.status().as_u16(),
            });
        }
        let encoding = response
            .headers()
            .get(CONTENT_ENCODING)
            .map(|value| value.to_str().unwrap_or("<非文本>").trim().to_ascii_lowercase());
        let encoding_matches = matches!(
            (representation, encoding.as_deref()),
            (WebRepresentation::Identity, None | Some("identity"))
                | (WebRepresentation::Brotli, Some("br"))
        );
        if !encoding_matches {
            return Err(WebFetchError::Encoding {
                path: relative_path.to_owned(),
                actual: encoding.unwrap_or_else(|| "无".to_owned()),
            });
        }
        let content_security_policy = response
            .headers()
            .get(CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let too_large = || WebFetchError::TooLarge {
            path: relative_path.to_owned(),
            limit: self.policy.max_file_bytes,
        };
        let declared = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        if declared.is_some_and(|length| length > self.policy.max_file_bytes) {
            return Err(too_large());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport)? {
            if body.len() as u64 + chunk.len() as u64 > self.policy.max_file_bytes {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(WebFetched { body, content_security_policy })
    }
}

fn validate_origin(origin: &Url) -> Result<(), WebFetchError> {
    if !matches!(origin.scheme(), "http" | "https") {
        return Err(WebFetchError::InvalidOrigin("只支持 http 与 https"));
    }
    if !origin.username().is_empty() || origin.password().is_some() {
        return Err(WebFetchError::InvalidOrigin("不得携带用户信息"));
    }
    if origin.query().is_some() || origin.fragment().is_some() || origin.path() != "/" {
        return Err(WebFetchError::InvalidOrigin("只允许 scheme、主机与端口"));
    }
    let loopback = match origin.host() {
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if loopback { Ok(()) } else { Err(WebFetchError::InvalidOrigin("必须是回环地址")) }
}

/// 请求路径只允许清单里会出现的安全字符：不含 `?`、`#`、`%`、反斜线、控制字符、空段与 `..`，
/// 且不以 `.br` 结尾（Brotli 实体经 `Accept-Encoding` 取得，不按 `.br` 路径请求）。
fn validate_path(path: &str) -> Result<(), WebFetchError> {
    let safe_bytes =
        path.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-~@+/".contains(&byte));
    let safe_segments =
        path.split('/').all(|segment| !segment.is_empty() && segment != "." && segment != "..");
    if path.is_empty() || !safe_bytes || !safe_segments || path.ends_with(".br") {
        return Err(WebFetchError::InvalidPath(path.to_owned()));
    }
    Ok(())
}
