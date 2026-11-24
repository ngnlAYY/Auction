//! 稳定原信键：认证 Bot UID、世界与规范正十进制 MID。
//!
//! 拍卖、寄售、通用拒件、投递、RTS、轮询与人工核对共享该键。配置名、显示名、Cookie 版本
//! 或游标都不能生成新的身份；同名换 UID 是不同原信，同 UID 改名续办原决定。

use std::cmp::Ordering;

use thiserror::Error;

use crate::SettlementWorld;

/// 返回数据库、HTTP 与规范编码共用的世界代码。
pub const fn settlement_world_code(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}

/// 只接受闭集内的世界代码；未知值失败关闭。
pub fn parse_settlement_world(value: &str) -> Option<SettlementWorld> {
    match value {
        "main" => Some(SettlementWorld::Main),
        "isekai" => Some(SettlementWorld::Isekai),
        _ => None,
    }
}

/// 原信键构造失败的安全分类，不回显原输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum OriginalMailKeyError {
    /// 认证 Bot UID 不是规范正十进制。
    #[error("认证 Bot UID 无效")]
    InvalidBotUid,
    /// MID 不是规范正十进制。
    #[error("原信 MID 无效")]
    InvalidMid,
}

/// 稳定原信键 `K=(authenticated_bot_uid,world,external_mid)`。
///
/// 字段保持私有以保护构造校验；排序与编码按 `UID、世界代码、MID` 的规范文本顺序进行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginalMailKey {
    authenticated_bot_uid: String,
    world: SettlementWorld,
    external_mid: String,
}

impl OriginalMailKey {
    /// 从已认证的 Bot UID、来源世界与规范 MID 构造稳定键。
    ///
    /// UID 与 MID 均沿 `valid_external_positive_id` 的口径：非空 ASCII 十进制、无前导零、
    /// 解析为正 `i64`。MID 保留规范文本，不从整数反写。
    pub fn new(
        authenticated_bot_uid: &str,
        world: SettlementWorld,
        external_mid: &str,
    ) -> Result<Self, OriginalMailKeyError> {
        if !valid_positive_i64_decimal(authenticated_bot_uid) {
            return Err(OriginalMailKeyError::InvalidBotUid);
        }
        if !valid_positive_i64_decimal(external_mid) {
            return Err(OriginalMailKeyError::InvalidMid);
        }
        Ok(Self {
            authenticated_bot_uid: authenticated_bot_uid.to_owned(),
            world,
            external_mid: external_mid.to_owned(),
        })
    }

    /// 借用认证 Bot UID。
    pub fn authenticated_bot_uid(&self) -> &str {
        &self.authenticated_bot_uid
    }

    /// 返回来源世界。
    pub const fn world(&self) -> SettlementWorld {
        self.world
    }

    /// 借用规范 MID 文本。
    pub fn external_mid(&self) -> &str {
        &self.external_mid
    }
}

impl Ord for OriginalMailKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.authenticated_bot_uid
            .cmp(&other.authenticated_bot_uid)
            .then_with(|| settlement_world_code(self.world).cmp(settlement_world_code(other.world)))
            .then_with(|| self.external_mid.cmp(&other.external_mid))
    }
}

impl PartialOrd for OriginalMailKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::hash::Hash for OriginalMailKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.authenticated_bot_uid.hash(state);
        settlement_world_code(self.world).hash(state);
        self.external_mid.hash(state);
    }
}

fn valid_positive_i64_decimal(value: &str) -> bool {
    // 与 `auction-application` 的 `valid_external_positive_id` 同口径；领域不能依赖应用层。
    !value.starts_with('0')
        && !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|number| number > 0)
}

#[cfg(test)]
mod tests {
    use crate::SettlementWorld;
    use crate::delivery::original::{
        OriginalMailKey, OriginalMailKeyError, parse_settlement_world, settlement_world_code,
    };

    #[test]
    fn canonical_key_preserves_uid_world_and_mid() -> Result<(), OriginalMailKeyError> {
        let key = OriginalMailKey::new("42", SettlementWorld::Main, "9007199254740993")?;
        assert_eq!(key.authenticated_bot_uid(), "42");
        assert_eq!(key.world(), SettlementWorld::Main);
        assert_eq!(key.external_mid(), "9007199254740993");
        assert_eq!(settlement_world_code(key.world()), "main");
        assert_eq!(parse_settlement_world("main"), Some(SettlementWorld::Main));
        assert_eq!(parse_settlement_world("isekai"), Some(SettlementWorld::Isekai));
        assert_eq!(parse_settlement_world("Main"), None);
        Ok(())
    }

    #[test]
    fn uid_and_mid_require_canonical_positive_i64_decimal() {
        let maximum = i64::MAX.to_string();
        for value in ["1", "42", maximum.as_str()] {
            assert!(OriginalMailKey::new(value, SettlementWorld::Main, "42").is_ok());
            assert!(OriginalMailKey::new("42", SettlementWorld::Main, value).is_ok());
        }
        for value in
            ["", "0", "00", "042", "+42", "-42", " 42", "42 ", "4.2", "４２", "9223372036854775808"]
        {
            assert_eq!(
                OriginalMailKey::new(value, SettlementWorld::Isekai, "42"),
                Err(OriginalMailKeyError::InvalidBotUid),
                "uid {value:?}"
            );
            assert_eq!(
                OriginalMailKey::new("42", SettlementWorld::Isekai, value),
                Err(OriginalMailKeyError::InvalidMid),
                "mid {value:?}"
            );
        }
    }

    #[test]
    fn same_uid_and_mid_but_other_world_is_a_different_key() -> Result<(), OriginalMailKeyError> {
        let main = OriginalMailKey::new("42", SettlementWorld::Main, "7")?;
        let isekai = OriginalMailKey::new("42", SettlementWorld::Isekai, "7")?;
        assert_ne!(main, isekai);
        let other_uid = OriginalMailKey::new("43", SettlementWorld::Main, "7")?;
        assert_ne!(main, other_uid);
        assert_eq!(OriginalMailKey::new("42", SettlementWorld::Main, "7")?, main);
        Ok(())
    }

    #[test]
    fn ordering_uses_canonical_text_not_enum_variant_order() -> Result<(), OriginalMailKeyError> {
        let isekai = OriginalMailKey::new("42", SettlementWorld::Isekai, "9")?;
        let main = OriginalMailKey::new("42", SettlementWorld::Main, "1")?;
        assert!(isekai < main);
        Ok(())
    }
}
