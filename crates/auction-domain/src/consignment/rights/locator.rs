//! 权利定位符 `R1-<43 字符无填充 base64url>` 的编码、解析与摘要。
//!
//! 随机量固定 32 字节；库里只存 SHA-256 摘要。定位符只定位权利，不授予任何动作。

use std::fmt;

use sha2::{Digest, Sha256};

use super::encoding::{RANDOM_BYTES, TEXT_LEN, format_prefixed, parse_prefixed};
use super::errors::RightsError;

/// 定位符前缀。
pub const LOCATOR_PREFIX: &str = "R1-";
/// 随机量字节数。
pub const LOCATOR_RANDOM_BYTES: usize = RANDOM_BYTES;
/// 完整定位符文本的 ASCII 字节数。
pub const LOCATOR_TEXT_LEN: usize = TEXT_LEN;

/// 32 字节定位符随机量；`Debug` 打码。
#[derive(Clone)]
pub struct RightLocator([u8; LOCATOR_RANDOM_BYTES]);

impl RightLocator {
    /// 从密码适配器提供的随机量构造。
    pub const fn from_entropy(entropy: [u8; LOCATOR_RANDOM_BYTES]) -> Self {
        Self(entropy)
    }

    /// 借用原始随机量；只用于摘要或持久化适配器。
    pub const fn as_bytes(&self) -> &[u8; LOCATOR_RANDOM_BYTES] {
        &self.0
    }
}

impl fmt::Debug for RightLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 规范定位符文本；`Debug` 打码，不序列化。
#[derive(Clone, PartialEq, Eq)]
pub struct LocatorText(String);

impl LocatorText {
    /// 借用完整文本；调用方只在向本人展示或寄送时使用。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LocatorText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 定位符摘要：规范文本 ASCII 的 SHA-256。256 位随机量的摘要不可逆，可入库与日志。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocatorDigest([u8; 32]);

impl LocatorDigest {
    /// 返回固定 32 字节摘要。
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

/// 把随机量编码为规范定位符文本。
pub fn format_locator(entropy: [u8; LOCATOR_RANDOM_BYTES]) -> LocatorText {
    LocatorText(format_prefixed(LOCATOR_PREFIX, &entropy))
}

/// 严格解析规范定位符；错误不回显输入。
pub fn parse_locator(text: &str) -> Result<RightLocator, RightsError> {
    parse_prefixed(LOCATOR_PREFIX, text).map(RightLocator).map_err(RightsError::Locator)
}

/// 计算定位符摘要。
pub fn locator_digest(locator: &RightLocator) -> LocatorDigest {
    let text = format_prefixed(LOCATOR_PREFIX, &locator.0);
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&Sha256::digest(text.as_bytes()));
    LocatorDigest(bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        LOCATOR_RANDOM_BYTES, LOCATOR_TEXT_LEN, RightLocator, format_locator, locator_digest,
        parse_locator,
    };
    use crate::consignment::rights::{RightsError, TextFault};

    #[test]
    fn locator_round_trips_with_fixed_shape_and_digest() {
        let entropy = [7_u8; LOCATOR_RANDOM_BYTES];
        let text = format_locator(entropy);
        assert_eq!(text.as_str().len(), LOCATOR_TEXT_LEN);
        assert!(text.as_str().starts_with("R1-"));
        let parsed = parse_locator(text.as_str()).expect("规范定位符应可解析");
        assert_eq!(parsed.as_bytes(), &entropy);
        let other = RightLocator::from_entropy([8_u8; LOCATOR_RANDOM_BYTES]);
        assert_ne!(locator_digest(&parsed), locator_digest(&other));
        assert_eq!(
            format_locator([0; 32]).as_str(),
            format!("R1-{}", "A".repeat(43)),
            "零向量固定"
        );
    }

    #[test]
    fn parser_rejects_every_noncanonical_shape() {
        let body = "A".repeat(43);
        assert_eq!(
            parse_locator(&format!("R1-{body}=")).map(|_| ()),
            Err(RightsError::Locator(TextFault::Alphabet))
        );
        assert_eq!(
            parse_locator(&format!("R1-{body}A")).map(|_| ()),
            Err(RightsError::Locator(TextFault::Length))
        );
        assert_eq!(
            parse_locator(&format!("C1-{body}")).map(|_| ()),
            Err(RightsError::Locator(TextFault::Prefix))
        );
        assert_eq!(
            parse_locator(&format!("R1-{}B", "A".repeat(42))).map(|_| ()),
            Err(RightsError::Locator(TextFault::NonCanonical))
        );
        assert_eq!(
            parse_locator(&format!("R1-{}+", "A".repeat(42))).map(|_| ()),
            Err(RightsError::Locator(TextFault::Alphabet))
        );
    }

    /// 安全边界：秘密不进 Debug 与错误文本。
    #[test]
    fn locator_secret_never_appears_in_debug_or_error() {
        let entropy = [0x5a_u8; LOCATOR_RANDOM_BYTES];
        let text = format_locator(entropy);
        let parsed = parse_locator(text.as_str()).expect("解析");
        assert!(!format!("{parsed:?}").contains(&text.as_str()[3..]));
        assert!(!format!("{text:?}").contains(&text.as_str()[3..]));
        let bad = format!("{}X", text.as_str());
        let error = parse_locator(&bad).expect_err("长度错误");
        assert!(!format!("{error} {error:?}").contains(&text.as_str()[3..]));
        // 反例对照：显式取文本才能看到原文。
        assert!(text.as_str().contains(&text.as_str()[3..]));
    }
}
