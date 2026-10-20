//! 名称分解、页面大类与有限重甲语法；不读取 HTML，也不决定收件准入。

use super::{
    EquipmentCategory, EquipmentIdentity, EquipmentSlot, ResolvedKind, TaxonomyError,
    affixes::{classify_resolved_prefix, classify_resolved_suffix},
    catalog::{BOUNDED_METADATA, name_matching_kinds},
    kinds, qualities,
};

/// 将页面 header 大类标签映射到原七个规范值；级别与交易状态由页面适配器另行读取。
pub fn category_from_header(value: &str) -> Result<EquipmentCategory, TaxonomyError> {
    let normalized = value
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match normalized.as_str() {
        "one handed weapon" => Ok(EquipmentCategory::OneHanded),
        "two handed weapon" => Ok(EquipmentCategory::TwoHanded),
        "staff" => Ok(EquipmentCategory::Staff),
        "shield" => Ok(EquipmentCategory::Shield),
        "cloth armor" => Ok(EquipmentCategory::Cloth),
        "light armor" => Ok(EquipmentCategory::Light),
        "heavy armor" => Ok(EquipmentCategory::Heavy),
        _ => Err(TaxonomyError::UnknownCategory),
    }
}

/// 识别完整种类并返回规范大小写；不接受自由前缀，不以未知字符串猜测大类。
pub fn resolve_kind(value: &str) -> Result<ResolvedKind, TaxonomyError> {
    if let Some(kind) = kinds().iter().find(|kind| kind.name.eq_ignore_ascii_case(value)) {
        return Ok(kind.into());
    }
    bounded_heavy_kind(value).ok_or(TaxonomyError::UnknownKind)
}

/// 分解完整装备名称，优先最长完整种类；自由词缀保留原文并单独标记识别状态。
pub fn parse_equipment_name(name: &str) -> Result<EquipmentIdentity, TaxonomyError> {
    if name.len() > 512 || name.chars().any(char::is_control) {
        return Err(TaxonomyError::InvalidName);
    }
    let fields: Vec<&str> = name.split_whitespace().collect();
    if fields.len() < 2 {
        return Err(TaxonomyError::InvalidName);
    }
    let quality = qualities()
        .iter()
        .find(|quality| quality.name.eq_ignore_ascii_case(fields[0]))
        .ok_or(TaxonomyError::UnknownQuality)?;
    let body = &fields[1..];
    let separator = body.iter().position(|field| field.eq_ignore_ascii_case("of"));
    let (type_fields, suffix) = match separator {
        Some(index) if index > 0 && index + 1 < body.len() => {
            (&body[..index], body[index + 1..].join(" "))
        }
        Some(_) => return Err(TaxonomyError::InvalidName),
        None => (body, String::new()),
    };
    let joined = type_fields.join(" ");
    let mut selected = None;
    // Great Mace 和带材质的完整种类先匹配，沿用旧解析器的规范字段和自由前缀拆分。
    for &candidate in name_matching_kinds() {
        if let Some(prefix) = preceding_prefix(&joined, candidate.name) {
            selected = Some((ResolvedKind::from(candidate), prefix.to_owned()));
            break;
        }
    }
    let (kind, prefix) =
        selected.or_else(|| bounded_heavy_identity(&joined)).ok_or(TaxonomyError::UnknownKind)?;
    if !valid_optional_affix(&prefix) || !valid_optional_affix(&suffix) {
        return Err(TaxonomyError::InvalidName);
    }
    Ok(EquipmentIdentity {
        quality: quality.name.to_owned(),
        prefix_compatibility: classify_resolved_prefix(&kind, &prefix),
        suffix_compatibility: classify_resolved_suffix(&kind, &suffix),
        prefix,
        equipment_type: kind.name.clone(),
        suffix,
        category: kind.category,
        kind,
        quality_metadata: quality.metadata,
    })
}

fn preceding_prefix<'a>(value: &'a str, kind: &str) -> Option<&'a str> {
    if value.eq_ignore_ascii_case(kind) {
        return Some("");
    }
    let start = value.len().checked_sub(kind.len())?;
    if value.get(start..)?.eq_ignore_ascii_case(kind)
        && start > 0
        && value.as_bytes().get(start - 1) == Some(&b' ')
    {
        value.get(..start - 1).map(str::trim)
    } else {
        None
    }
}

fn bounded_heavy_identity(value: &str) -> Option<(ResolvedKind, String)> {
    let (leading, slot) = value.rsplit_once(' ')?;
    let (prefix, material) = leading.rsplit_once(' ').unwrap_or(("", leading));
    let kind = bounded_heavy_kind(&format!("{material} {slot}"))?;
    Some((kind, prefix.to_owned()))
}

fn bounded_heavy_kind(value: &str) -> Option<ResolvedKind> {
    let (material, raw_slot) = value.split_once(' ')?;
    let material = ["Chain", "Reactive"]
        .into_iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(material))?;
    // Wiki 仅给材质与 Heavy 公共部位；这八个 token 只允许兼容读取，不生成确证种类。
    let (slot_name, slot) = [
        ("Helmet", EquipmentSlot::Head),
        ("Cuirass", EquipmentSlot::Body),
        ("Armor", EquipmentSlot::Body),
        ("Gauntlets", EquipmentSlot::Hands),
        ("Greaves", EquipmentSlot::Legs),
        ("Leggings", EquipmentSlot::Legs),
        ("Sabatons", EquipmentSlot::Feet),
        ("Boots", EquipmentSlot::Feet),
    ]
    .into_iter()
    .find(|(candidate, _)| candidate.eq_ignore_ascii_case(raw_slot))?;
    Some(ResolvedKind {
        name: format!("{material} {slot_name}"),
        category: EquipmentCategory::Heavy,
        material: Some(material),
        slot: Some(slot),
        metadata: BOUNDED_METADATA,
    })
}

fn valid_optional_affix(value: &str) -> bool {
    value.len() <= 128 && !value.chars().any(char::is_control)
}
