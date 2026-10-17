//! Armory 已批准分类闭集；没有默认分类、salvaged 或任意查询字符串。

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
