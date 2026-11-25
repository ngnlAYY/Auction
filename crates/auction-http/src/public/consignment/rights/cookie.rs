//! 受限权利的两枚 Cookie：`hv_right`（10 分钟单权利会话）与 `hv_right_browser`（长期浏览器绑定）。
//!
//! 两者都限定在权利路径下，`HttpOnly; Secure; SameSite=Strict`。`hv_right_browser` 的值是随机量，
//! 服务端只保留它的摘要作为浏览器绑定；两枚 Cookie 的值都不得进入日志、DTO 或错误（不变量 7）。
//! 只借用 `session/cookies.rs` 的 Cookie 读取，序列化因路径与属性不同在此单独生成。

use auction_application::consignment::BrowserBinding;
use auction_domain::consignment::rights::{RightSessionToken, format_session_token};
use axum::http::{HeaderMap, HeaderValue};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::session::read_admin_cookie;

/// 单权利会话 Cookie 名。
pub(super) const RIGHT_SESSION_COOKIE: &str = "hv_right";
/// 浏览器绑定 Cookie 名。
pub(super) const RIGHT_BROWSER_COOKIE: &str = "hv_right_browser";
/// 两枚 Cookie 的路径：只随权利接口发送。
const COOKIE_PATH: &str = "/api/v2/consignment/rights";
/// 会话 Cookie 的有效期：固定 10 分钟，服务端另按原截止判定，轮换不延寿。
const SESSION_MAX_AGE_SECONDS: u32 = 600;
/// 浏览器绑定 Cookie 的有效期：浏览器对持久 Cookie 的 400 天上限。
const BROWSER_MAX_AGE_SECONDS: u32 = 400 * 24 * 60 * 60;
/// 绑定摘要的域前缀，使它不会与其他用途的 SHA-256 混用。
const BINDING_DOMAIN: &[u8] = b"auction.consignment.right.browser.v1\0";
/// 随机量字节数。
const RANDOM_BYTES: usize = 32;

/// 本次请求使用的浏览器绑定与其 Cookie 文本；不实现 `Debug`，避免随机量外泄。
pub(super) struct BrowserCookie {
    value: String,
    binding: BrowserBinding,
}

impl BrowserCookie {
    /// 读取请求里已有且规范的绑定 Cookie；缺失、重复、空值或非规范编码都视为没有。
    pub(super) fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let value = read_admin_cookie(headers, RIGHT_BROWSER_COOKIE)?;
        let decoded = URL_SAFE_NO_PAD.decode(value.as_bytes()).ok()?;
        (decoded.len() == RANDOM_BYTES).then(|| Self::from_value(value))
    }

    /// 为首次访问的浏览器签发新随机量。
    pub(super) fn mint() -> Self {
        Self::from_value(URL_SAFE_NO_PAD.encode(random_bytes()))
    }

    fn from_value(value: String) -> Self {
        let mut hash = Sha256::new();
        hash.update(BINDING_DOMAIN);
        hash.update(value.as_bytes());
        Self { binding: BrowserBinding(hash.finalize().into()), value }
    }

    /// 服务端保留的浏览器绑定摘要。
    pub(super) const fn binding(&self) -> BrowserBinding {
        self.binding
    }

    /// `Set-Cookie` 头；值不可编码时为空，调用方失败关闭。
    pub(super) fn set_cookie(&self) -> Option<HeaderValue> {
        encode(RIGHT_BROWSER_COOKIE, &self.value, BROWSER_MAX_AGE_SECONDS)
    }
}

/// 核验成功后写入会话令牌的 `Set-Cookie` 头。
pub(super) fn session_set_cookie(token: &RightSessionToken) -> Option<HeaderValue> {
    encode(RIGHT_SESSION_COOKIE, format_session_token(token).as_str(), SESSION_MAX_AGE_SECONDS)
}

/// 请求里的会话 Cookie 文本；缺失或重复为 `None`，格式由用例严格解析。
pub(super) fn read_session_cookie(headers: &HeaderMap) -> Option<String> {
    read_admin_cookie(headers, RIGHT_SESSION_COOKIE)
}

fn encode(name: &str, value: &str, max_age: u32) -> Option<HeaderValue> {
    HeaderValue::from_str(&format!(
        "{name}={value}; Path={COOKIE_PATH}; Max-Age={max_age}; HttpOnly; Secure; SameSite=Strict"
    ))
    .ok()
}

/// 全熵随机字节：由 v4 UUID 的 CSPRNG 取得，丢弃每个 UUID 里被版本与变体占用的两个字节。
fn random_bytes() -> [u8; RANDOM_BYTES] {
    let mut output = [0_u8; RANDOM_BYTES];
    let mut filled = 0;
    while filled < RANDOM_BYTES {
        for (index, byte) in Uuid::new_v4().as_bytes().iter().enumerate() {
            if matches!(index, 6 | 8) || filled == RANDOM_BYTES {
                continue;
            }
            output[filled] = *byte;
            filled += 1;
        }
    }
    output
}
