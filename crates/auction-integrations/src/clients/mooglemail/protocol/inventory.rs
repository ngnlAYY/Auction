//! Armory 已批准分类闭集；没有默认分类、salvaged 或任意查询字符串。

use auction_domain::equipment::taxonomy::{EquipmentCategory, parse_equipment_name};

/// 当前持有证据仅扫描已观察的七类 organize 列表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoogleMailInventoryCategory {
    OneHanded,
    TwoHanded,
    Staff,
    Shield,
    Cloth,
    Light,
    Heavy,
}

impl MoogleMailInventoryCategory {
    pub const ALL: [Self; 7] = [
        Self::OneHanded,
        Self::TwoHanded,
        Self::Staff,
        Self::Shield,
        Self::Cloth,
        Self::Light,
        Self::Heavy,
    ];

    /// 域装备大类到 Armory 分类的固定映射；只作为读取提示，不参与资格或身份证明。
    pub(crate) const fn from_domain(category: EquipmentCategory) -> Self {
        match category {
            EquipmentCategory::OneHanded => Self::OneHanded,
            EquipmentCategory::TwoHanded => Self::TwoHanded,
            EquipmentCategory::Staff => Self::Staff,
            EquipmentCategory::Shield => Self::Shield,
            EquipmentCategory::Cloth => Self::Cloth,
            EquipmentCategory::Light => Self::Light,
            EquipmentCategory::Heavy => Self::Heavy,
        }
    }

    /// 从已冻结装备名推导读取提示；名称无法识别时不提示，回退完整七类扫描。
    pub(crate) fn hint_from_name(name: &str) -> Option<Self> {
        parse_equipment_name(name).ok().map(|identity| Self::from_domain(identity.category))
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneHanded => "weapon_1handed",
            Self::TwoHanded => "weapon_2handed",
            Self::Staff => "weapon_staff",
            Self::Shield => "shield",
            Self::Cloth => "armor_cloth",
            Self::Light => "armor_light",
            Self::Heavy => "armor_heavy",
        }
    }
}
