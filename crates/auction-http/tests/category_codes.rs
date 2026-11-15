//! 浏览器共用的装备大类对照表必须与领域映射及公开分类键逐项一致。
//!
//! 前端不能依赖领域 crate，只能读取 `auction-contracts` 的常量表；本测试把该表与领域
//! 的规范名、公开键和 lot 代码前缀逐项比对，任何一侧漂移都会让网站代码与论坛可竞价代码分叉。

use auction_application::auction::PUBLIC_LOT_CATEGORIES;
use auction_contracts::EQUIPMENT_CATEGORY_CODES;
use auction_domain::equipment::taxonomy::EquipmentCategory;

#[test]
fn browser_category_codes_match_the_domain_mapping() {
    assert_eq!(EQUIPMENT_CATEGORY_CODES.len(), EquipmentCategory::ALL.len());
    for ((key, name, prefix), category) in
        EQUIPMENT_CATEGORY_CODES.into_iter().zip(EquipmentCategory::ALL)
    {
        assert_eq!(key, category.public_key(), "{name}");
        assert_eq!(name, category.as_str(), "{name}");
        assert_eq!(prefix, category.lot_code_prefix(), "{name}");
    }
}

#[test]
fn browser_category_keys_follow_the_public_count_order() {
    let keys = EQUIPMENT_CATEGORY_CODES.map(|(key, _, _)| key);
    assert_eq!(PUBLIC_LOT_CATEGORIES[0], "all");
    assert_eq!(PUBLIC_LOT_CATEGORIES[1..], keys);
}
