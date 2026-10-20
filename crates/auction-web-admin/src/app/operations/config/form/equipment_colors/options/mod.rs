//! 只从服务端统一装备词典投影计算展示候选；历史已选值由复选框组保留。

use auction_contracts::EquipmentTaxonomyDto;

pub(super) fn equipment_kinds_for(taxonomy: &EquipmentTaxonomyDto, category: &str) -> Vec<String> {
    let categories = selections(category);
    taxonomy
        .kinds
        .iter()
        .filter(|option| selected_category(&categories, &option.category))
        .map(|option| option.name.clone())
        .chain(
            taxonomy
                .resolved_kinds
                .iter()
                .filter(|option| selected_category(&categories, &option.category))
                .map(|option| option.name.clone()),
        )
        .chain(
            taxonomy
                .materials
                .iter()
                .filter(|option| selected_category(&categories, &option.category))
                .map(|option| option.name.clone()),
        )
        .collect()
}

pub(super) fn prefixes_for(
    taxonomy: &EquipmentTaxonomyDto,
    category: &str,
    kind: &str,
) -> Vec<String> {
    affixes_for(taxonomy, category, kind, true)
}

pub(super) fn suffixes_for(
    taxonomy: &EquipmentTaxonomyDto,
    category: &str,
    kind: &str,
) -> Vec<String> {
    affixes_for(taxonomy, category, kind, false)
}

/// 多选维度取并集；只按投影中的精确名称关联，未知名称不会猜测成某个材质族。
fn affixes_for(
    taxonomy: &EquipmentTaxonomyDto,
    category: &str,
    kind: &str,
    prefixes: bool,
) -> Vec<String> {
    let categories = selections(category);
    let kinds = selections(kind);
    let mut result = Vec::<String>::new();
    let candidates =
        taxonomy
            .kinds
            .iter()
            .map(|option| (&option.name, &option.category, &option.prefixes, &option.suffixes))
            .chain(
                taxonomy.resolved_kinds.iter().map(|option| {
                    (&option.name, &option.category, &option.prefixes, &option.suffixes)
                }),
            )
            .chain(taxonomy.materials.iter().map(|option| {
                (&option.name, &option.category, &option.prefixes, &option.suffixes)
            }));
    for (name, category, prefix_options, suffix_options) in candidates {
        if !selected_category(&categories, category)
            || (!kinds.is_empty()
                && !kinds.iter().any(|selected| selected.eq_ignore_ascii_case(name)))
        {
            continue;
        }
        for option in if prefixes { prefix_options } else { suffix_options } {
            if !result.iter().any(|previous| previous.eq_ignore_ascii_case(option)) {
                result.push(option.clone());
            }
        }
    }
    result
}

fn selections(value: &str) -> Vec<&str> {
    value.lines().map(str::trim).filter(|value| !value.is_empty()).collect()
}

fn selected_category(categories: &[&str], category: &str) -> bool {
    categories.is_empty()
        || categories.iter().any(|selected| selected.eq_ignore_ascii_case(category))
}
