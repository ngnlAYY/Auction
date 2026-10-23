//! Public/Admin Cookie、Basic Auth 和 CSRF 头的安全解析与生成。

use super::*;

pub(crate) fn read_public_session_cookie(headers: &HeaderMap) -> Option<String> {
    read_cookie(headers, PUBLIC_SESSION_COOKIE, false)
}

pub(crate) fn read_admin_basic_auth(headers: &HeaderMap) -> Option<(String, Vec<u8>)> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Basic") || encoded.is_empty() {
        return None;
    }
    let decoded = STANDARD.decode(encoded).ok()?;
    let separator = decoded.iter().position(|byte| *byte == b':')?;
    let actor = std::str::from_utf8(&decoded[..separator]).ok()?.to_owned();
    let secret = decoded.get(separator + 1..)?.to_vec();
    Some((actor, secret))
}

pub(crate) fn read_admin_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    read_cookie(headers, name, true)
}

fn read_cookie(headers: &HeaderMap, name: &str, skip_empty_pairs: bool) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        let value = value.to_str().ok()?;
        for pair in value.split(';') {
            let pair = pair.trim();
            if skip_empty_pairs && pair.is_empty() {
                continue;
            }
            let (cookie_name, cookie_value) = pair.split_once('=')?;
            if cookie_name != name {
                continue;
            }
            if found.is_some() || cookie_value.is_empty() {
                return None;
            }
            found = Some(cookie_value.to_owned());
        }
    }
    found
}

pub(crate) fn valid_admin_csrf_header(headers: &HeaderMap, cookie: &str) -> bool {
    let mut values = headers.get_all(ADMIN_CSRF_HEADER).iter();
    let Some(value) = values.next().and_then(|value| value.to_str().ok()) else {
        return false;
    };
    values.next().is_none() && constant_time_equal(cookie, value)
}

pub(crate) fn constant_time_equal(left: &str, right: &str) -> bool {
    let left_hash = Sha256::digest(left.as_bytes());
    let right_hash = Sha256::digest(right.as_bytes());
    left_hash.as_slice().ct_eq(right_hash.as_slice()).unwrap_u8() == 1
}

pub(crate) fn admin_cookie(
    name: &str,
    token: &str,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
    secure: bool,
    http_only: bool,
) -> Result<HeaderValue, ()> {
    let max_age = (expires_at - now).num_seconds().max(0);
    let mut value = format!(
        "{name}={token}; Path=/; Expires={}; Max-Age={max_age}; SameSite=Strict",
        expires_at.format("%a, %d %b %Y %H:%M:%S GMT"),
    );
    if http_only {
        value.push_str("; HttpOnly");
    }
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|_| ())
}

pub(crate) fn append_clear_cookie(headers: &mut HeaderMap, secure: bool) {
    let mut value = String::from(
        "auction_public_session=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; HttpOnly; SameSite=Lax",
    );
    if secure {
        value.push_str("; Secure");
    }
    if let Ok(value) = HeaderValue::from_str(&value) {
        headers.insert(header::SET_COOKIE, value);
    }
}

pub(crate) fn append_clear_admin_cookies(headers: &mut HeaderMap, secure: bool) {
    for (name, http_only) in [(ADMIN_SESSION_COOKIE, true), (ADMIN_CSRF_COOKIE, false)] {
        let mut value = format!(
            "{name}=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; SameSite=Strict"
        );
        if http_only {
            value.push_str("; HttpOnly");
        }
        if secure {
            value.push_str("; Secure");
        }
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.append(header::SET_COOKIE, value);
        }
    }
}

/// 浏览器持久期最多 400 天；有限会话再受服务器截止约束，只由显式串行化写请求同步。
pub(crate) fn public_session_cookie(
    token: &str,
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    secure: bool,
) -> Result<HeaderValue, ()> {
    let browser_limit = now.checked_add_signed(chrono::Duration::days(400)).ok_or(())?;
    let expires_at = expires_at.map_or(browser_limit, |value| value.min(browser_limit));
    let max_age = (expires_at - now).num_seconds().max(0);
    let mut value = format!(
        "{PUBLIC_SESSION_COOKIE}={token}; Path=/; Expires={}; Max-Age={max_age}; HttpOnly; SameSite=Lax",
        expires_at.format("%a, %d %b %Y %H:%M:%S GMT"),
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|_| ())
}
