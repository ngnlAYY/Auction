//! 定义挂单的世界内装备身份与历史装备编号的匹配规则。

use crate::consignment::ListingError;
use crate::valid_positive_decimal;

/// 挂单的装备身份，由世界和规范外部编号组成。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListingEquipmentIdentity {
    world_id: &'static str,
    external_equipment_id: String,
}

impl ListingEquipmentIdentity {
    /// 从已通过 URL 解析的世界和原样提取的编号建立身份。
    ///
    /// 读取凭证由调用方用 [`crate::EquipmentKey`] 保管。
    pub fn new(world_id: &str, external_equipment_id: &str) -> Result<Self, ListingError> {
        // 不变量 6：新登记必须提交规范正十进制，兼容去零只用于查历史。
        let world_id = match world_id {
            "main" => "main",
            "isekai" => "isekai",
            _ => return Err(ListingError::InvalidIdentity),
        };
        if !valid_positive_decimal(external_equipment_id, 32) {
            return Err(ListingError::InvalidIdentity);
        }
        Ok(Self { world_id, external_equipment_id: external_equipment_id.to_owned() })
    }

    /// 返回数据库世界代码。
    pub fn world_id(&self) -> &'static str {
        self.world_id
    }

    /// 借用规范的外部装备编号。
    pub fn external_equipment_id(&self) -> &str {
        &self.external_equipment_id
    }
}

/// 判断历史行的原始编号是否指向该世界内的同一装备。
///
/// 与合同第 4.2 节的 `public.consignment_eid_alias(text)` 加同世界等值比较一致。
pub fn matches_historical_equipment_alias(
    identity: &ListingEquipmentIdentity,
    historical_world_id: &str,
    raw_equipment_id: &str,
) -> bool {
    // 先校验整串 ASCII，再按文本去零，避免截断旧编号或丢失精度。
    historical_world_id == identity.world_id()
        && raw_equipment_id.bytes().all(|byte| byte.is_ascii_digit())
        && raw_equipment_id.trim_start_matches('0') == identity.external_equipment_id()
}

#[cfg(test)]
mod tests {
    use super::{ListingEquipmentIdentity, matches_historical_equipment_alias};
    use crate::consignment::ListingError;

    #[test]
    fn identity_requires_a_supported_world() -> Result<(), ListingError> {
        let main = ListingEquipmentIdentity::new("main", "42")?;
        let isekai = ListingEquipmentIdentity::new("isekai", "42")?;
        assert_eq!(main.world_id(), "main");
        assert_eq!(isekai.world_id(), "isekai");
        assert_ne!(main, isekai);
        for world_id in ["", "Main", "main ", "persistent"] {
            assert_eq!(
                ListingEquipmentIdentity::new(world_id, "42"),
                Err(ListingError::InvalidIdentity)
            );
        }
        Ok(())
    }

    #[test]
    fn identity_requires_canonical_positive_decimal_up_to_32_digits() -> Result<(), ListingError> {
        let maximum = "12345678901234567890123456789012";
        for eid in ["1", "42", maximum] {
            assert_eq!(ListingEquipmentIdentity::new("main", eid)?.external_equipment_id(), eid);
        }
        for eid in [
            "",
            "0",
            "00",
            "00042",
            "+42",
            "-42",
            "４２",
            "42 ",
            "4.2",
            "4\n2",
            "123456789012345678901234567890123",
        ] {
            assert_eq!(
                ListingEquipmentIdentity::new("main", eid),
                Err(ListingError::InvalidIdentity)
            );
        }
        Ok(())
    }

    #[test]
    fn historical_alias_matches_only_in_the_same_world() -> Result<(), ListingError> {
        let main = ListingEquipmentIdentity::new("main", "42")?;
        let isekai = ListingEquipmentIdentity::new("isekai", "42")?;
        for (identity, other_world_id) in [(&main, "isekai"), (&isekai, "main")] {
            assert!(matches_historical_equipment_alias(identity, identity.world_id(), "42"));
            assert!(matches_historical_equipment_alias(identity, identity.world_id(), "00042"));
            assert!(!matches_historical_equipment_alias(identity, other_world_id, "00042"));
            assert!(!matches_historical_equipment_alias(identity, identity.world_id(), "00043"));
        }
        Ok(())
    }

    #[test]
    fn long_historical_alias_is_compared_without_integer_range_or_precision_loss()
    -> Result<(), ListingError> {
        let identity = ListingEquipmentIdentity::new("main", "12345678901234567890123456789012")?;
        let historical = "0000000000000000000000000000000012345678901234567890123456789012";
        let neighboring = "0000000000000000000000000000000012345678901234567890123456789013";
        assert_eq!(historical.len(), 64);
        assert!(matches_historical_equipment_alias(&identity, "main", historical));
        assert!(!matches_historical_equipment_alias(&identity, "main", neighboring));
        assert!(!matches_historical_equipment_alias(&identity, "main", &"9".repeat(64)));
        Ok(())
    }

    #[test]
    fn historical_zero_and_malformed_ids_never_match() -> Result<(), ListingError> {
        let identity = ListingEquipmentIdentity::new("main", "42")?;
        assert!(matches_historical_equipment_alias(&identity, "main", "00042"));
        for raw in [
            "",
            "0",
            "000",
            "+00042",
            "-00042",
            " 00042",
            "00042 ",
            "00042\n",
            "00\t042",
            "000４２",
            "0004x",
            "0.42",
            "42e0",
        ] {
            assert!(!matches_historical_equipment_alias(&identity, "main", raw));
        }
        assert!(!matches_historical_equipment_alias(&identity, "main", &"0".repeat(64)));
        Ok(())
    }
}
