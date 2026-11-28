//! 匿名非寄售登记时由登记人自填的游戏角色名。

use crate::consignment::ListingError;
use crate::valid_text;

/// 自填游戏角色名的最大 UTF-8 字节数；与解析快照的持有人名同属有界安全文本。
pub const MAX_SELLER_GAME_NAME_BYTES: usize = 128;

/// 匿名非寄售挂单的卖家名，即 HentaiVerse 游戏角色名。
///
/// 它是登记人自填、未经站内账户验证的标识，只用于展示、额度计数与持有人比对；
/// 比较一律由数据库 `lower` 等值完成，这里保留原样大小写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingSellerName(String);

impl ListingSellerName {
    /// 校验单行、不带首尾空白（有则拒绝，不做修剪）、无控制字符与行分隔符，并拒绝装备链接与 key 片段。
    pub fn new(value: &str) -> Result<Self, ListingError> {
        // 不变量 6、7：与数据库 consignment_safe_observation_text 同口径，凭据形状不能借名字进入公开 DTO。
        if !valid_text(value, MAX_SELLER_GAME_NAME_BYTES)
            || value.contains(['\u{2028}', '\u{2029}'])
        {
            return Err(ListingError::InvalidSellerName);
        }
        // 只对有界文本做大小写折叠，避免超长外部输入在拒绝前产生整份副本。
        let lowered = value.to_ascii_lowercase();
        if ["/equip/", "/equip?", "key="].iter().any(|fragment| lowered.contains(fragment)) {
            return Err(ListingError::InvalidSellerName);
        }
        Ok(Self(value.to_owned()))
    }

    /// 借用原样文本，供持久化、请求摘要与展示使用。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{ListingSellerName, MAX_SELLER_GAME_NAME_BYTES};
    use crate::consignment::ListingError;

    #[test]
    fn seller_name_accepts_a_plain_game_name_and_keeps_its_case() -> Result<(), ListingError> {
        assert_eq!(ListingSellerName::new("Alice Zero")?.as_str(), "Alice Zero");
        assert_eq!(ListingSellerName::new("角色名")?.as_str(), "角色名");
        let longest = "a".repeat(MAX_SELLER_GAME_NAME_BYTES);
        assert_eq!(ListingSellerName::new(&longest)?.as_str(), longest);
        Ok(())
    }

    #[test]
    fn seller_name_rejects_blank_padded_control_oversized_and_credential_shapes() {
        let oversized = "a".repeat(MAX_SELLER_GAME_NAME_BYTES + 1);
        for value in [
            "",
            " Alice",
            "Alice ",
            "Al\nice",
            "Al\u{0085}ice",
            "Al\u{2028}ice",
            oversized.as_str(),
            "https://hentaiverse.org/equip/1/abcdef0123",
            "x/EQUIP?eid=1",
            "KEY=abcdef0123",
        ] {
            assert_eq!(
                ListingSellerName::new(value),
                Err(ListingError::InvalidSellerName),
                "{value:?}"
            );
        }
    }
}
