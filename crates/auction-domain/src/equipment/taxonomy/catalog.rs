//! 当前与历史的确证清单；有限语法不枚举为完整种类。

use std::sync::LazyLock;

use EquipmentCategory::*;
use EquipmentSlot::*;

use super::model::{
    CategoryEntry, EquipmentCategory, EquipmentSlot, KindEntry, MaterialEntry, QualityEntry,
    TaxonomyMetadata, TaxonomyStatus,
};

pub(super) const BASIC_METADATA: TaxonomyMetadata = TaxonomyMetadata {
    status: TaxonomyStatus::Current,
    source: "https://ehwiki.org/wiki/Equipment_Basics",
    revision: "65026",
};
pub(super) const DETAIL_METADATA: TaxonomyMetadata = TaxonomyMetadata {
    status: TaxonomyStatus::Current,
    source: "https://ehwiki.org/wiki/Detailed_Equip_Characteristics",
    revision: "66094",
};
pub(super) const LEGACY_METADATA: TaxonomyMetadata = TaxonomyMetadata {
    status: TaxonomyStatus::Legacy,
    source: "https://ehwiki.org/wiki/Obsolete_Items",
    revision: "65259",
};
pub(super) const BOUNDED_METADATA: TaxonomyMetadata =
    TaxonomyMetadata { status: TaxonomyStatus::BoundedGrammar, ..BASIC_METADATA };

const CATEGORIES: &[CategoryEntry] = &[
    CategoryEntry { name: "One-Handed", category: OneHanded, metadata: BASIC_METADATA },
    CategoryEntry { name: "Two-Handed", category: TwoHanded, metadata: BASIC_METADATA },
    CategoryEntry { name: "Staff", category: Staff, metadata: BASIC_METADATA },
    CategoryEntry { name: "Shield", category: Shield, metadata: BASIC_METADATA },
    CategoryEntry { name: "Cloth", category: Cloth, metadata: BASIC_METADATA },
    CategoryEntry { name: "Light", category: Light, metadata: BASIC_METADATA },
    CategoryEntry { name: "Heavy", category: Heavy, metadata: BASIC_METADATA },
];

const QUALITIES: &[QualityEntry] = &[
    QualityEntry { name: "Ultimate", metadata: DETAIL_METADATA },
    QualityEntry { name: "Peerless+", metadata: DETAIL_METADATA },
    QualityEntry { name: "Peerless", metadata: DETAIL_METADATA },
    QualityEntry { name: "Legendary", metadata: DETAIL_METADATA },
    QualityEntry { name: "Magnificent", metadata: DETAIL_METADATA },
    QualityEntry { name: "Exquisite", metadata: DETAIL_METADATA },
    QualityEntry { name: "Superior", metadata: DETAIL_METADATA },
    QualityEntry { name: "Average", metadata: DETAIL_METADATA },
    QualityEntry { name: "Fair", metadata: DETAIL_METADATA },
    QualityEntry { name: "Crude", metadata: DETAIL_METADATA },
    QualityEntry { name: "Fine", metadata: LEGACY_METADATA },
    QualityEntry { name: "Flimsy", metadata: LEGACY_METADATA },
];

const MATERIALS: &[MaterialEntry] = &[
    MaterialEntry {
        name: "Cotton",
        category: Cloth,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Gossamer",
        category: Cloth,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Ironsilk",
        category: Cloth,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Phase",
        category: Cloth,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Leather",
        category: Light,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Drakehide",
        category: Light,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Kevlar",
        category: Light,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Shade",
        category: Light,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Plate",
        category: Heavy,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Chain",
        category: Heavy,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Reactive",
        category: Heavy,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Power",
        category: Heavy,
        selectable_family: true,
        metadata: BASIC_METADATA,
    },
    MaterialEntry {
        name: "Chainmail",
        category: Heavy,
        selectable_family: false,
        metadata: LEGACY_METADATA,
    },
];

const fn kind(
    name: &'static str,
    category: EquipmentCategory,
    material: Option<&'static str>,
    slot: Option<EquipmentSlot>,
    metadata: TaxonomyMetadata,
) -> KindEntry {
    KindEntry { name, category, material, slot, metadata }
}

// 22 个武器/盾牌、50 个当前护甲完整名，以及 5 个历史 Chainmail；顺序不参与解析优先级。
const KINDS: &[KindEntry] = &[
    kind("Axe", OneHanded, None, None, BASIC_METADATA),
    kind("Club", OneHanded, None, None, BASIC_METADATA),
    kind("Dagger", OneHanded, None, None, BASIC_METADATA),
    kind("Rapier", OneHanded, None, None, BASIC_METADATA),
    kind("Shortsword", OneHanded, None, None, BASIC_METADATA),
    kind("Wakizashi", OneHanded, None, None, BASIC_METADATA),
    kind("Estoc", TwoHanded, None, None, BASIC_METADATA),
    kind("Longsword", TwoHanded, None, None, BASIC_METADATA),
    kind("Mace", TwoHanded, None, None, BASIC_METADATA),
    kind("Great Mace", TwoHanded, None, None, DETAIL_METADATA),
    kind("Katana", TwoHanded, None, None, BASIC_METADATA),
    kind("Scythe", TwoHanded, None, None, BASIC_METADATA),
    kind("Swordchucks", TwoHanded, None, None, BASIC_METADATA),
    kind("Ebony Staff", Staff, None, None, BASIC_METADATA),
    kind("Oak Staff", Staff, None, None, BASIC_METADATA),
    kind("Willow Staff", Staff, None, None, BASIC_METADATA),
    kind("Redwood Staff", Staff, None, None, BASIC_METADATA),
    kind("Katalox Staff", Staff, None, None, BASIC_METADATA),
    kind("Buckler", Shield, None, None, BASIC_METADATA),
    kind("Kite Shield", Shield, None, None, BASIC_METADATA),
    kind("Tower Shield", Shield, None, None, BASIC_METADATA),
    kind("Force Shield", Shield, None, None, BASIC_METADATA),
    kind("Cotton Cap", Cloth, Some("Cotton"), Some(Head), BASIC_METADATA),
    kind("Cotton Robe", Cloth, Some("Cotton"), Some(Body), BASIC_METADATA),
    kind("Cotton Gloves", Cloth, Some("Cotton"), Some(Hands), BASIC_METADATA),
    kind("Cotton Pants", Cloth, Some("Cotton"), Some(Legs), BASIC_METADATA),
    kind("Cotton Shoes", Cloth, Some("Cotton"), Some(Feet), BASIC_METADATA),
    kind("Gossamer Cap", Cloth, Some("Gossamer"), Some(Head), BASIC_METADATA),
    kind("Gossamer Robe", Cloth, Some("Gossamer"), Some(Body), BASIC_METADATA),
    kind("Gossamer Gloves", Cloth, Some("Gossamer"), Some(Hands), BASIC_METADATA),
    kind("Gossamer Pants", Cloth, Some("Gossamer"), Some(Legs), BASIC_METADATA),
    kind("Gossamer Shoes", Cloth, Some("Gossamer"), Some(Feet), BASIC_METADATA),
    kind("Ironsilk Cap", Cloth, Some("Ironsilk"), Some(Head), BASIC_METADATA),
    kind("Ironsilk Robe", Cloth, Some("Ironsilk"), Some(Body), BASIC_METADATA),
    kind("Ironsilk Gloves", Cloth, Some("Ironsilk"), Some(Hands), BASIC_METADATA),
    kind("Ironsilk Pants", Cloth, Some("Ironsilk"), Some(Legs), BASIC_METADATA),
    kind("Ironsilk Shoes", Cloth, Some("Ironsilk"), Some(Feet), BASIC_METADATA),
    kind("Phase Cap", Cloth, Some("Phase"), Some(Head), BASIC_METADATA),
    kind("Phase Robe", Cloth, Some("Phase"), Some(Body), BASIC_METADATA),
    kind("Phase Gloves", Cloth, Some("Phase"), Some(Hands), BASIC_METADATA),
    kind("Phase Pants", Cloth, Some("Phase"), Some(Legs), BASIC_METADATA),
    kind("Phase Shoes", Cloth, Some("Phase"), Some(Feet), BASIC_METADATA),
    kind("Leather Helmet", Light, Some("Leather"), Some(Head), BASIC_METADATA),
    kind("Leather Breastplate", Light, Some("Leather"), Some(Body), BASIC_METADATA),
    kind("Leather Gauntlets", Light, Some("Leather"), Some(Hands), BASIC_METADATA),
    kind("Leather Leggings", Light, Some("Leather"), Some(Legs), BASIC_METADATA),
    kind("Leather Boots", Light, Some("Leather"), Some(Feet), BASIC_METADATA),
    kind("Drakehide Helmet", Light, Some("Drakehide"), Some(Head), BASIC_METADATA),
    kind("Drakehide Breastplate", Light, Some("Drakehide"), Some(Body), BASIC_METADATA),
    kind("Drakehide Gauntlets", Light, Some("Drakehide"), Some(Hands), BASIC_METADATA),
    kind("Drakehide Leggings", Light, Some("Drakehide"), Some(Legs), BASIC_METADATA),
    kind("Drakehide Boots", Light, Some("Drakehide"), Some(Feet), BASIC_METADATA),
    kind("Kevlar Helmet", Light, Some("Kevlar"), Some(Head), BASIC_METADATA),
    kind("Kevlar Breastplate", Light, Some("Kevlar"), Some(Body), BASIC_METADATA),
    kind("Kevlar Gauntlets", Light, Some("Kevlar"), Some(Hands), BASIC_METADATA),
    kind("Kevlar Leggings", Light, Some("Kevlar"), Some(Legs), BASIC_METADATA),
    kind("Kevlar Boots", Light, Some("Kevlar"), Some(Feet), BASIC_METADATA),
    kind("Shade Helmet", Light, Some("Shade"), Some(Head), BASIC_METADATA),
    kind("Shade Breastplate", Light, Some("Shade"), Some(Body), BASIC_METADATA),
    kind("Shade Gauntlets", Light, Some("Shade"), Some(Hands), BASIC_METADATA),
    kind("Shade Leggings", Light, Some("Shade"), Some(Legs), BASIC_METADATA),
    kind("Shade Boots", Light, Some("Shade"), Some(Feet), BASIC_METADATA),
    kind("Plate Helmet", Heavy, Some("Plate"), Some(Head), BASIC_METADATA),
    kind("Plate Cuirass", Heavy, Some("Plate"), Some(Body), BASIC_METADATA),
    kind("Plate Gauntlets", Heavy, Some("Plate"), Some(Hands), BASIC_METADATA),
    kind("Plate Greaves", Heavy, Some("Plate"), Some(Legs), BASIC_METADATA),
    kind("Plate Sabatons", Heavy, Some("Plate"), Some(Feet), BASIC_METADATA),
    kind("Power Helmet", Heavy, Some("Power"), Some(Head), BASIC_METADATA),
    kind("Power Armor", Heavy, Some("Power"), Some(Body), BASIC_METADATA),
    kind("Power Gauntlets", Heavy, Some("Power"), Some(Hands), BASIC_METADATA),
    kind("Power Leggings", Heavy, Some("Power"), Some(Legs), BASIC_METADATA),
    kind("Power Boots", Heavy, Some("Power"), Some(Feet), BASIC_METADATA),
    kind("Chainmail Coif", Heavy, Some("Chainmail"), Some(Head), LEGACY_METADATA),
    kind("Chainmail Hauberk", Heavy, Some("Chainmail"), Some(Body), LEGACY_METADATA),
    kind("Chainmail Mitons", Heavy, Some("Chainmail"), Some(Hands), LEGACY_METADATA),
    kind("Chainmail Chausses", Heavy, Some("Chainmail"), Some(Legs), LEGACY_METADATA),
    kind("Chainmail Boots", Heavy, Some("Chainmail"), Some(Feet), LEGACY_METADATA),
];

/// 返回七个稳定大类及来源。
pub fn categories() -> &'static [CategoryEntry] {
    CATEGORIES
}

/// 返回十个当前品质及 Fine、Flimsy 两个历史品质；顺序不代表收件准入或品质范围。
pub fn qualities() -> &'static [QualityEntry] {
    QUALITIES
}

/// 返回 77 个确证完整种类，不展开 Chain/Reactive 的有限兼容语法。
pub fn kinds() -> &'static [KindEntry] {
    KINDS
}

/// 名称匹配顺序只初始化一次；稳定排序保留同长度词条的原先优先级。
pub(super) fn name_matching_kinds() -> &'static [&'static KindEntry] {
    static ORDERED: LazyLock<Vec<&'static KindEntry>> = LazyLock::new(|| {
        let mut entries: Vec<_> = KINDS.iter().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.name.split_whitespace().count()));
        entries
    });
    &ORDERED
}

/// 返回十二个当前材质及历史 Chainmail；整族匹配资格由 selectable_family 指示。
pub fn materials() -> &'static [MaterialEntry] {
    MATERIALS
}

/// 仅将原十二个当前材质映射为新多选标色规则的整族大类；历史 Chainmail 保持精确名称语义。
pub fn armor_material_category(value: &str) -> Option<EquipmentCategory> {
    MATERIALS
        .iter()
        .find(|entry| entry.selectable_family && entry.name.eq_ignore_ascii_case(value))
        .map(|entry| entry.category)
}
