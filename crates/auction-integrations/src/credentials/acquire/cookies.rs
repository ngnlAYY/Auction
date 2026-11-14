//! Cookie header 与 Set-Cookie 分开解析；游戏凭据只保存两个认证字段，不把论坛会话带到游戏。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::safe_diagnostics;

use auction_application::admin::{
    AdminCredentialError, AdminCredentialKind, AdminCredentialSecret,
    MAX_ADMIN_CREDENTIAL_SECRET_BYTES,
};
use auction_application::core::ApplicationError;
use reqwest::header::{self, HeaderMap, HeaderValue};
use url::Url;

use super::{SecretBuffer, unavailable};

const MAXIMUM_COOKIE_FIELDS: usize = 64;
const MAXIMUM_SET_COOKIE_BYTES: usize = 4096;
const COOKIE_NAMES: [&str; 4] = ["ipb_member_id", "ipb_pass_hash", "ipb_session_id", "ipb_coppa"];

pub(super) fn normalize(
    kind: AdminCredentialKind,
    secret: AdminCredentialSecret,
) -> Result<AdminCredentialSecret, ApplicationError> {
    let invalid = || ApplicationError::AdminCredential(AdminCredentialError::InvalidCookie);
    let header = std::str::from_utf8(secret.as_bytes()).map_err(|_| invalid())?;
    if header.len() > MAX_ADMIN_CREDENTIAL_SECRET_BYTES || !visible_ascii(header) {
        return Err(invalid());
    }
    let mut names = Vec::new();
    let mut member_id = None;
    let mut pass_hash = None;
    for pair in header.split(';') {
        let (name, value) = cookie_pair(pair.trim_matches(' ')).ok_or_else(invalid)?;
        if names.len() >= MAXIMUM_COOKIE_FIELDS || names.contains(&name) {
            return Err(invalid());
        }
        names.push(name);
        if let Some(index) = COOKIE_NAMES.iter().position(|candidate| *candidate == name) {
            if !valid_auth_value(index, value) {
                return Err(invalid());
            }
            match index {
                0 => member_id = Some(value),
                1 => pass_hash = Some(value),
                _ => {}
            }
        }
    }
    let member_id = member_id.ok_or_else(invalid)?;
    let pass_hash = pass_hash.ok_or_else(invalid)?;
    match kind {
        // 论坛保留通过严格 header 校验的原始字段；不替用户删除站点实际使用的其他 Cookie。
        AdminCredentialKind::ForumCookie => Ok(secret),
        AdminCredentialKind::MailCookie => {
            let mut value = String::with_capacity(29 + member_id.len() + pass_hash.len());
            value.push_str("ipb_member_id=");
            value.push_str(member_id);
            value.push_str("; ipb_pass_hash=");
            value.push_str(pass_hash);
            AdminCredentialSecret::new(value).map_err(|_| invalid())
        }
    }
}

/// 只保留服务器发出的四个已知 Cookie；每次更新立即释放并清零被替换的值。
#[derive(Default)]
pub(super) struct LoginCookies {
    values: [Option<SecretBuffer>; 4],
    updated: u8,
}

impl LoginCookies {
    pub(super) fn from_headers(
        headers: &HeaderMap,
        endpoint: &Url,
    ) -> Result<Self, ApplicationError> {
        let mut result = Self::default();
        let mut total = 0usize;
        for (index, header) in headers.get_all(header::SET_COOKIE).iter().enumerate() {
            total = total.saturating_add(header.as_bytes().len());
            if index >= MAXIMUM_COOKIE_FIELDS
                || header.as_bytes().len() > MAXIMUM_SET_COOKIE_BYTES
                || total > MAX_ADMIN_CREDENTIAL_SECRET_BYTES
            {
                return Err(unavailable());
            }
            let value = header.to_str().map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_login",
                    "decode_cookie_header",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            })?;
            if !visible_ascii(value) {
                return Err(unavailable());
            }
            let mut parts = value.split(';');
            let (name, value) = parts.next().and_then(cookie_pair).ok_or_else(unavailable)?;
            let Some(index) = COOKIE_NAMES.iter().position(|candidate| *candidate == name) else {
                // 不建立通用 Cookie jar，也不让其他 Cookie 成为认证成功的证据。
                continue;
            };
            result.updated |= 1 << index;
            let deleted = validate_attributes(parts, endpoint)?;
            if deleted {
                result.values[index] = None;
                continue;
            }
            if !valid_auth_value(index, value) {
                return Err(unavailable());
            }
            // 真实论坛可重复设置 session；Set-Cookie 按顺序更新，不套用 Cookie header 的重复拒绝规则。
            result.values[index] = Some(SecretBuffer(value.as_bytes().to_vec()));
        }
        Ok(result)
    }

    pub(super) fn has_authentication(&self) -> bool {
        self.values[0].is_some() && self.values[1].is_some()
    }

    pub(super) fn inherit_session(&mut self, mut initial: Self) {
        for index in [2, 3] {
            // 明确删除同样是更新，不能从登录前页面复活已被服务器清除的 session。
            if self.updated & (1 << index) == 0 {
                self.values[index] = initial.values[index].take();
            }
        }
    }

    pub(super) fn header(&self) -> Result<Option<HeaderValue>, ApplicationError> {
        if self.values.iter().all(Option::is_none) {
            return Ok(None);
        }
        let value = self.render(COOKIE_NAMES.len());
        let mut header = HeaderValue::from_bytes(&value.0).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "credential_login",
                "encode_cookie_header",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })?;
        header.set_sensitive(true);
        Ok(Some(header))
    }

    pub(super) fn into_secret(self) -> Result<AdminCredentialSecret, ApplicationError> {
        self.secret(COOKIE_NAMES.len())
    }

    pub(super) fn authentication_secret(&self) -> Result<AdminCredentialSecret, ApplicationError> {
        self.secret(2)
    }

    fn secret(&self, field_count: usize) -> Result<AdminCredentialSecret, ApplicationError> {
        if !self.has_authentication() {
            return Err(unavailable());
        }
        let mut value = self.render(field_count);
        let text = String::from_utf8(std::mem::take(&mut value.0)).map_err(|error| {
            let mut bytes = error.into_bytes();
            bytes.fill(0);
            unavailable()
        })?;
        AdminCredentialSecret::new(text).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "credential_login",
                "seal_cookie_secret",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })
    }

    fn render(&self, field_count: usize) -> SecretBuffer {
        let capacity = self.values.iter().zip(COOKIE_NAMES).take(field_count).fold(
            0,
            |length, (value, name)| {
                length + value.as_ref().map_or(0, |value| name.len() + value.0.len() + 3)
            },
        );
        let mut result = SecretBuffer(Vec::with_capacity(capacity));
        for (name, value) in COOKIE_NAMES.iter().zip(&self.values).take(field_count) {
            if let Some(value) = value {
                if !result.0.is_empty() {
                    result.0.extend_from_slice(b"; ");
                }
                result.0.extend_from_slice(name.as_bytes());
                result.0.push(b'=');
                result.0.extend_from_slice(&value.0);
            }
        }
        result
    }
}

fn validate_attributes<'a>(
    parts: impl Iterator<Item = &'a str>,
    endpoint: &Url,
) -> Result<bool, ApplicationError> {
    let mut domain_seen = false;
    let mut path_seen = false;
    let mut max_age_seen = false;
    let mut deleted = false;
    for part in parts {
        let part = part.trim_matches(' ');
        if part.is_empty() {
            return Err(unavailable());
        }
        let (name, value) =
            part.split_once('=').map_or((part, None), |(name, value)| (name, Some(value)));
        if !valid_name(name) {
            return Err(unavailable());
        }
        if name.eq_ignore_ascii_case("domain") {
            if domain_seen || !value.is_some_and(|value| valid_domain(value, endpoint)) {
                return Err(unavailable());
            }
            domain_seen = true;
        } else if name.eq_ignore_ascii_case("path") {
            if path_seen || !matches!(value, Some("/" | "/index.php")) {
                return Err(unavailable());
            }
            path_seen = true;
        } else if name.eq_ignore_ascii_case("max-age") {
            if max_age_seen {
                return Err(unavailable());
            }
            let age = value.and_then(|value| value.parse::<i64>().ok()).ok_or_else(unavailable)?;
            deleted = age <= 0;
            max_age_seen = true;
        } else if (name.eq_ignore_ascii_case("secure") || name.eq_ignore_ascii_case("httponly"))
            && value.is_some()
        {
            return Err(unavailable());
        }
    }
    Ok(deleted)
}

fn valid_domain(value: &str, endpoint: &Url) -> bool {
    let value = value.strip_prefix('.').unwrap_or(value);
    endpoint.host_str().is_some_and(|host| {
        value.eq_ignore_ascii_case(host)
            || (host == "forums.e-hentai.org" && value.eq_ignore_ascii_case("e-hentai.org"))
    })
}

fn cookie_pair(pair: &str) -> Option<(&str, &str)> {
    let (name, value) = pair.split_once('=')?;
    (valid_name(name)
        && value.bytes().all(
            |byte| matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e),
        ))
    .then_some((name, value))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_auth_value(index: usize, value: &str) -> bool {
    match index {
        0 => {
            // 20 位十进制可能超过 u64，规范文本之外仍需解析确认范围。
            auction_domain::valid_positive_decimal(value, 20) && value.parse::<u64>().is_ok()
        }
        1 | 2 => value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        3 => matches!(value, "0" | "1"),
        _ => false,
    }
}

fn visible_ascii(value: &str) -> bool {
    value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
}
