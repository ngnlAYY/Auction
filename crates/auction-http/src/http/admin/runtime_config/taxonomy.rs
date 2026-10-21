//! 统一装备词典到 Admin 只读 DTO 的映射；不复制词表或执行收件策略。

use std::{collections::BTreeMap, sync::OnceLock};

use auction_application::AdminRuntimeDocument;
use auction_contracts::{
    EquipmentBoundedKindOptionDto, EquipmentKindOptionDto, EquipmentMaterialOptionDto,
    EquipmentQualityOptionDto, EquipmentTaxonomyDto,
};
use auction_domain::equipment::taxonomy::{
    self, TAXONOMY_REVISION, TaxonomyStatus, prefixes_for_kind, resolve_kind, suffixes_for_kind,
};

// 主体只随程序词典版本变化；缓存只保存公开推荐值，不保存配置、命令或用户草稿。
static BASE_TAXONOMY: OnceLock<EquipmentTaxonomyDto> = OnceLock::new();

pub(super) fn equipment_taxonomy(document: &AdminRuntimeDocument) -> EquipmentTaxonomyDto {
    let mut result = BASE_TAXONOMY.get_or_init(base_taxonomy).clone();
    let mut resolved = BTreeMap::new();
    for value in
        document.equipment_colors.iter().flat_map(|colors| &colors.rules).flat_map(|rule| {
            std::iter::once(rule.equipment_type.as_str())
                .chain(rule.equipment_types.iter().map(String::as_str))
        })
    {
        let Ok(kind) = resolve_kind(value) else { continue };
        if kind.metadata.status != TaxonomyStatus::BoundedGrammar {
            continue;
        }
        let (Some(material), Some(slot)) = (kind.material, kind.slot) else { continue };
        // 仅为本次文档已有的合法完整名补推荐；大小写由领域规范化，实际闭集最多十六项。
        // 不修改主词典、配置值或指纹，也不把可能的组合展开进 77 项确证目录。
        resolved.entry(kind.name.clone()).or_insert_with(|| EquipmentBoundedKindOptionDto {
            prefixes: owned(prefixes_for_kind(&kind.name)),
            suffixes: owned(suffixes_for_kind(&kind.name)),
            name: kind.name,
            category: kind.category.as_str().to_owned(),
            material: material.to_owned(),
            slot: slot.as_str().to_owned(),
        });
    }
    result.resolved_kinds = resolved.into_values().collect();
    result
}

fn base_taxonomy() -> EquipmentTaxonomyDto {
    EquipmentTaxonomyDto {
        revision: TAXONOMY_REVISION.to_owned(),
        categories: taxonomy::categories().iter().map(|entry| entry.name.to_owned()).collect(),
        qualities: taxonomy::qualities()
            .iter()
            // 标色表单只推荐这三档品质；名称解析兼容和已有规则仍由各自 owner 保留。
            .filter(|entry| matches!(entry.name, "Peerless" | "Legendary" | "Magnificent"))
            .map(|entry| EquipmentQualityOptionDto {
                name: entry.name.to_owned(),
                legacy: entry.metadata.status == TaxonomyStatus::Legacy,
            })
            .collect(),
        kinds: taxonomy::kinds()
            .iter()
            .map(|entry| EquipmentKindOptionDto {
                name: entry.name.to_owned(),
                category: entry.category.as_str().to_owned(),
                material: entry.material.map(str::to_owned),
                slot: entry.slot.map(|slot| slot.as_str().to_owned()),
                legacy: entry.metadata.status == TaxonomyStatus::Legacy,
                prefixes: owned(prefixes_for_kind(entry.name)),
                suffixes: owned(suffixes_for_kind(entry.name)),
            })
            .collect(),
        materials: taxonomy::materials()
            .iter()
            .filter(|entry| entry.selectable_family)
            .map(|entry| EquipmentMaterialOptionDto {
                name: entry.name.to_owned(),
                category: entry.category.as_str().to_owned(),
                prefixes: owned(prefixes_for_kind(entry.name)),
                suffixes: owned(suffixes_for_kind(entry.name)),
            })
            .collect(),
        resolved_kinds: Vec::new(),
    }
}

fn owned(values: Vec<&str>) -> Vec<String> {
    values.into_iter().map(str::to_owned).collect()
}
