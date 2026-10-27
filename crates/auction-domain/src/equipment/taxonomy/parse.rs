//! 名称分解、页面大类与有限重甲语法；不读取 HTML，也不决定收件准入。

use super::{
    EquipmentCategory, EquipmentIdentity, EquipmentSlot, ResolvedKind, TaxonomyError,
    affixes::{classify_resolved_prefix, classify_resolved_suffix},
    catalog::{BOUNDED_METADATA, name_matching_kinds},
    kinds, qualities,
};

/// 将页面 header 大类标签映射到原七个规范值；级别与交易状态由页面适配器另行读取。
pub fn category_from_header(value: &str) -> Result<EquipmentCategory, TaxonomyError> {
    // 直接比较借用的 token，保留 Unicode 空白、分隔符与 ASCII 大小写兼容，不建立规范化副本。
    let mut fields = value
        .split(|character: char| character.is_whitespace() || matches!(character, '-' | '_'))
        .filter(|field| !field.is_empty());
    match [fields.next(), fields.next(), fields.next(), fields.next()] {
        [Some(first), Some(second), Some(third), None]
            if second.eq_ignore_ascii_case("handed") && third.eq_ignore_ascii_case("weapon") =>
        {
            if first.eq_ignore_ascii_case("one") {
                Ok(EquipmentCategory::OneHanded)
            } else if first.eq_ignore_ascii_case("two") {
                Ok(EquipmentCategory::TwoHanded)
            } else {
                Err(TaxonomyError::UnknownCategory)
            }
        }
        [Some(first), Some(second), None, None] if second.eq_ignore_ascii_case("armor") => {
            if first.eq_ignore_ascii_case("cloth") {
                Ok(EquipmentCategory::Cloth)
            } else if first.eq_ignore_ascii_case("light") {
                Ok(EquipmentCategory::Light)
            } else if first.eq_ignore_ascii_case("heavy") {
                Ok(EquipmentCategory::Heavy)
            } else {
                Err(TaxonomyError::UnknownCategory)
            }
        }
        [Some(first), None, None, None] if first.eq_ignore_ascii_case("staff") => {
            Ok(EquipmentCategory::Staff)
        }
        [Some(first), None, None, None] if first.eq_ignore_ascii_case("shield") => {
            Ok(EquipmentCategory::Shield)
        }
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

/// 按品质、前缀、种类、护甲部位和后缀分解名称；Legendary 必须有前后缀。
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
    // 先匹配 Great Mace 等完整基础名，再从护甲基础名拆出种类和部位，避免误切长种类。
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
    // Legendary 的前后缀是名称结构必填项；孤立冠词不构成后缀，其余品质保留兼容识别。
    if quality.name == "Legendary"
        && (prefix.is_empty() || suffix.is_empty() || suffix.eq_ignore_ascii_case("the"))
    {
        return Err(TaxonomyError::InvalidName);
    }
    Ok(EquipmentIdentity {
        quality: quality.name.to_owned(),
        prefix_compatibility: classify_resolved_prefix(&kind, &prefix),
        suffix_compatibility: classify_resolved_suffix(&kind, &suffix),
        prefix,
        equipment_type: kind.kind_name().to_owned(),
        slot_name: kind.slot_name().map(str::to_owned),
        slot: kind.slot,
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
    let kind = bounded_heavy_kind_parts(material, slot)?;
    Some((kind, prefix.to_owned()))
}

fn bounded_heavy_kind(value: &str) -> Option<ResolvedKind> {
    let (material, raw_slot) = value.split_once(' ')?;
    bounded_heavy_kind_parts(material, raw_slot)
}

// 名称解析已取得两个 token；直接借用，避免为验证重建临时完整名称。
fn bounded_heavy_kind_parts(material: &str, raw_slot: &str) -> Option<ResolvedKind> {
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
