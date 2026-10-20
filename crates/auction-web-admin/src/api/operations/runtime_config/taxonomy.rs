//! 服务端装备词典投影的闭集结构校验；浏览器不再维护装备或词缀名称白名单。

use auction_contracts::EquipmentTaxonomyDto;

use super::valid_text;

/// 只验证投影的完整性、界限及引用，不把展示候选用作历史规则的准入条件。
pub(super) fn valid_equipment_taxonomy(value: &EquipmentTaxonomyDto) -> bool {
    valid_text(&value.revision, 128, false)
        && valid_names(value.categories.iter().map(String::as_str), 64, 64, false)
        && valid_names(value.qualities.iter().map(|option| option.name.as_str()), 64, 64, false)
        && valid_names(value.kinds.iter().map(|option| option.name.as_str()), 512, 64, false)
        && valid_names(value.materials.iter().map(|option| option.name.as_str()), 64, 64, false)
        && valid_names(value.resolved_kinds.iter().map(|option| option.name.as_str()), 16, 64, true)
        && value.materials.iter().all(|option| {
            value.categories.contains(&option.category)
                && valid_affixes(&option.prefixes)
                && valid_affixes(&option.suffixes)
        })
        && value.kinds.iter().all(|option| {
            value.categories.contains(&option.category)
                && value
                    .materials
                    .iter()
                    .all(|material| !material.name.eq_ignore_ascii_case(&option.name))
                && option.material.is_some() == option.slot.is_some()
                && option.material.as_deref().is_none_or(|name| valid_text(name, 64, false))
                && option
                    .slot
                    .as_deref()
                    .is_none_or(|slot| matches!(slot, "head" | "body" | "hands" | "legs" | "feet"))
                && option.material.as_ref().is_none_or(|material| {
                    // 旧完整种类的材质不一定提供族选项；若提供，所属大类必须一致。
                    value.materials.iter().all(|candidate| {
                        !candidate.name.eq_ignore_ascii_case(material)
                            || candidate.category == option.category
                    })
                })
                && valid_affixes(&option.prefixes)
                && valid_affixes(&option.suffixes)
        })
        && value.resolved_kinds.iter().all(|option| {
            value.categories.contains(&option.category)
                && value.kinds.iter().all(|kind| !kind.name.eq_ignore_ascii_case(&option.name))
                && value
                    .materials
                    .iter()
                    .all(|material| !material.name.eq_ignore_ascii_case(&option.name))
                && value.materials.iter().any(|material| {
                    material.name.eq_ignore_ascii_case(&option.material)
                        && material.category == option.category
                })
                && matches!(option.slot.as_str(), "head" | "body" | "hands" | "legs" | "feet")
                && valid_affixes(&option.prefixes)
                && valid_affixes(&option.suffixes)
        })
}

fn valid_affixes(values: &[String]) -> bool {
    valid_names(values.iter().map(String::as_str), 256, 128, true)
}

fn valid_names<'a>(
    values: impl Iterator<Item = &'a str>,
    maximum: usize,
    maximum_bytes: usize,
    allow_empty: bool,
) -> bool {
    let mut seen = Vec::<&str>::new();
    for value in values {
        if seen.len() == maximum
            || !valid_text(value, maximum_bytes, false)
            || seen.iter().any(|previous| previous.eq_ignore_ascii_case(value))
        {
            return false;
        }
        seen.push(value);
    }
    allow_empty || !seen.is_empty()
}
