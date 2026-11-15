//! 装备大类 ↔ 公开分类键 ↔ lot 代码前缀的冻结对照。
//!
//! 论坛回帖按 lot 代码定位竞价目标，网站、邮件与竞价日志显示同一代码；公开分类键是已发布
//! 的 JSON 键。任一映射漂移都会让玩家看到的代码与论坛可竞价代码不一致，因此逐项冻结。

use auction_domain::equipment::taxonomy::EquipmentCategory;

const FROZEN: [(EquipmentCategory, &str, &str, &str); 7] = [
    (EquipmentCategory::OneHanded, "One-Handed", "one", "One"),
    (EquipmentCategory::TwoHanded, "Two-Handed", "two", "Two"),
    (EquipmentCategory::Staff, "Staff", "sta", "Sta"),
    (EquipmentCategory::Shield, "Shield", "shd", "Shd"),
    (EquipmentCategory::Cloth, "Cloth", "clo", "Clo"),
    (EquipmentCategory::Light, "Light", "lig", "Lig"),
    (EquipmentCategory::Heavy, "Heavy", "hea", "Hea"),
];

#[test]
fn category_names_keys_and_prefixes_are_frozen_in_order() {
    assert_eq!(EquipmentCategory::ALL.map(EquipmentCategory::as_str), FROZEN.map(|row| row.1));
    for (category, name, key, prefix) in FROZEN {
        assert_eq!(category.as_str(), name);
        assert_eq!(category.public_key(), key);
        assert_eq!(category.lot_code_prefix(), prefix);
        assert_eq!(EquipmentCategory::from_name(name), Some(category));
        assert_eq!(EquipmentCategory::from_public_key(key), Some(category));
    }
}

#[test]
fn unknown_names_and_keys_are_not_categories() {
    // 只接受精确规范名与公开键；大小写变体、`all` 与 header 标签都不是规范值。
    for value in ["", "all", "one-handed", "Cloth ", "Cloth Armor", "Lot", "weapon"] {
        assert_eq!(EquipmentCategory::from_name(value), None, "{value:?}");
        assert_eq!(EquipmentCategory::from_public_key(value), None, "{value:?}");
    }
}

#[test]
fn lot_codes_pad_the_category_number_to_two_digits() {
    assert_eq!(EquipmentCategory::OneHanded.lot_code(1), "One01");
    assert_eq!(EquipmentCategory::Cloth.lot_code(16), "Clo16");
    assert_eq!(EquipmentCategory::Heavy.lot_code(120), "Hea120");
}
