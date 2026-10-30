//! MoogleMail 协议输入、endpoint 和敏感值校验。

use url::Url;

use super::{MAX_OPAQUE_ID_BYTES, PRODUCTION_HOST};

pub(crate) fn valid_opaque_id(value: &str) -> bool {
    valid_line(value, MAX_OPAQUE_ID_BYTES, false)
}

pub(crate) fn valid_line(value: &str, maximum: usize, allow_empty: bool) -> bool {
    (allow_empty && value.is_empty()) || auction_domain::valid_text(value, maximum)
}

pub(crate) fn valid_decimal_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn valid_hex_key(value: &str) -> bool {
    value.len() == 10 && value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
}

/// 换行同属控制字符，规范单行文本规则已拒绝 CR/LF 头注入。
pub(super) fn valid_secret(value: &str, maximum: usize) -> bool {
    auction_domain::valid_text(value, maximum)
}

pub(super) fn valid_endpoint(endpoint: &Url) -> bool {
    let path_is_root = endpoint.path().is_empty() || endpoint.path() == "/";
    let is_production = endpoint.scheme() == "https"
        && endpoint.host_str().is_some_and(|host| host.eq_ignore_ascii_case(PRODUCTION_HOST))
        && endpoint.port().is_none()
        && endpoint.username() == ""
        && endpoint.password().is_none()
        && path_is_root
        && endpoint.query().is_none()
        && endpoint.fragment().is_none();
    if is_production {
        return true;
    }
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || endpoint.username() != ""
        || endpoint.password().is_some()
        || !path_is_root
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return false;
    }
    // host_str 的 IPv6 表示含方括号，不能再交给 IpAddr 文本解析器。
    match endpoint.host() {
        Some(url::Host::Domain(hostname)) => hostname.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

pub(super) fn endpoint_origin(endpoint: &Url) -> String {
    let mut origin = format!("{}://{}", endpoint.scheme(), endpoint.host_str().unwrap_or_default());
    if let Some(port) = endpoint.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    origin
}
