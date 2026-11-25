//! 权利会话令牌 `S1-<43 字符无填充 base64url>`、会话截止与摘要。
//!
//! 令牌只放 `hv_right` cookie；库里只存 SHA-256 摘要。会话 10 分钟，轮换令牌不延寿。

use std::fmt;

use auction_time::{Duration, Timestamp};
use sha2::{Digest, Sha256};

use super::encoding::{RANDOM_BYTES, format_prefixed, parse_prefixed};
use super::errors::RightsError;

/// 会话令牌前缀。
pub const SESSION_TOKEN_PREFIX: &str = "S1-";
/// 单权利会话有效期。
pub const SESSION_TTL: Duration = Duration::minutes(10);

/// 32 字节会话令牌随机量；`Debug` 打码。
#[derive(Clone)]
pub struct RightSessionToken([u8; RANDOM_BYTES]);

impl RightSessionToken {
    /// 从密码适配器提供的随机量构造。
    pub const fn from_entropy(entropy: [u8; RANDOM_BYTES]) -> Self {
        Self(entropy)
    }

    /// 借用原始随机量。
    pub const fn as_bytes(&self) -> &[u8; RANDOM_BYTES] {
        &self.0
    }
}

impl fmt::Debug for RightSessionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 规范会话令牌文本；`Debug` 打码。
#[derive(Clone, PartialEq, Eq)]
pub struct SessionTokenText(String);

impl SessionTokenText {
    /// 借用完整文本；只用于写 cookie。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionTokenText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 会话令牌摘要：规范文本 ASCII 的 SHA-256。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionDigest([u8; 32]);

impl SessionDigest {
    /// 返回固定 32 字节摘要。
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

/// 把随机量编码为规范令牌文本。
pub fn format_session_token(token: &RightSessionToken) -> SessionTokenText {
    SessionTokenText(format_prefixed(SESSION_TOKEN_PREFIX, &token.0))
}

/// 严格解析规范令牌文本；错误不回显输入。
pub fn parse_session_token(text: &str) -> Result<RightSessionToken, RightsError> {
    parse_prefixed(SESSION_TOKEN_PREFIX, text)
        .map(RightSessionToken)
        .map_err(RightsError::SessionToken)
}

/// 计算令牌摘要。
pub fn session_digest(token: &RightSessionToken) -> SessionDigest {
    let text = format_prefixed(SESSION_TOKEN_PREFIX, &token.0);
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&Sha256::digest(text.as_bytes()));
    SessionDigest(bytes)
}

/// 轮换令牌时的截止：沿用原截止，绝不延寿；已过原截止失败关闭。
///
/// 保护的不变量：原浏览器同命令恢复同次授予只换令牌，会话总寿命不超过首次授予的 10 分钟。
pub fn rotated_session_expiry(
    original_expires_at: Timestamp,
    now: Timestamp,
) -> Result<Timestamp, RightsError> {
    if now >= original_expires_at {
        return Err(RightsError::SessionExpired);
    }
    Ok(original_expires_at)
}

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};

    use super::{
        RightSessionToken, SESSION_TTL, format_session_token, parse_session_token,
        rotated_session_expiry, session_digest,
    };
    use crate::consignment::rights::{RightsError, TextFault};

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_timestamp(seconds, 0).expect("时刻")
    }

    #[test]
    fn token_round_trips_and_rejects_foreign_prefix() {
        let token = RightSessionToken::from_entropy([3; 32]);
        let text = format_session_token(&token);
        let parsed = parse_session_token(text.as_str()).expect("解析");
        assert_eq!(session_digest(&parsed), session_digest(&token));
        let locator_like = text.as_str().replacen("S1-", "R1-", 1);
        assert_eq!(
            parse_session_token(&locator_like).map(|_| ()),
            Err(RightsError::SessionToken(TextFault::Prefix))
        );
        assert!(!format!("{token:?}{text:?}").contains(&text.as_str()[3..]));
    }

    #[test]
    fn rotation_keeps_original_deadline_and_never_extends() {
        let granted = at(1_000);
        let deadline = granted.checked_add_signed(SESSION_TTL).expect("截止");
        let rotated =
            rotated_session_expiry(deadline, at(1_000) + Duration::minutes(4)).expect("未过期");
        assert_eq!(rotated, deadline);
        assert_eq!(rotated_session_expiry(deadline, deadline), Err(RightsError::SessionExpired));
    }
}
