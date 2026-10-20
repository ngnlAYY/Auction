//! 装备名称与分类的唯一纯领域词典。
//!
//! 来源版本、现行与历史词条、有限兼容语法分别记录；解析成功只说明名称可识别，
//! 收件品质、交易资格和页面真实性由各自应用用例及外部适配器校验。

mod affixes;
mod catalog;
mod model;
mod parse;

pub use affixes::{
    classify_prefix, classify_suffix, prefixes_for_kind, suffix_matches, suffixes_for_kind,
};
pub use catalog::{armor_material_category, categories, kinds, materials, qualities};
pub use model::{
    AffixCompatibility, CategoryEntry, EquipmentCategory, EquipmentIdentity, EquipmentSlot,
    KindEntry, MaterialEntry, QualityEntry, ResolvedKind, TaxonomyError, TaxonomyMetadata,
    TaxonomyStatus,
};
pub use parse::{category_from_header, parse_equipment_name, resolve_kind};

/// 词典审阅版本；三个数字依次为 Basics、Detailed 和 Obsolete 的 MediaWiki oldid。
pub const TAXONOMY_REVISION: &str = "ehwiki-65026-66094-65259-v1";
