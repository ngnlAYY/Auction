//! 词缀推荐关系与历史识别；推荐集合不会成为名称解析或收件准入的白名单。

mod prefixes;
mod suffixes;

use super::{AffixCompatibility, EquipmentCategory, ResolvedKind, materials, resolve_kind};

/// 推荐集合借用最多三段静态词条；识别时直接遍历，公开候选 API 才收集为 Vec。
pub(super) struct Recommendations([&'static [&'static str]; 3]);

impl Recommendations {
    pub(super) const fn new(groups: [&'static [&'static str]; 3]) -> Self {
        Self(groups)
    }

    fn iter(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.iter().flat_map(|group| group.iter().copied())
    }
}

#[derive(Clone, Copy)]
struct AffixKind<'a> {
    name: &'a str,
    category: Option<EquipmentCategory>,
    recommends: bool,
}

impl<'a> From<&'a ResolvedKind> for AffixKind<'a> {
    fn from(kind: &'a ResolvedKind) -> Self {
        Self {
            name: &kind.name,
            category: Some(kind.category),
            // Chainmail 只有历史名称确证，没有当前词缀矩阵。
            recommends: kind.material != Some("Chainmail"),
        }
    }
}

/// 返回具体种类或十二个当前材质的前缀推荐并集，保留既有候选顺序。
pub fn prefixes_for_kind(kind: &str) -> Vec<&'static str> {
    with_known_kind(kind, |kind| {
        if kind.recommends { prefixes::for_kind(kind.name).iter().collect() } else { Vec::new() }
    })
    .unwrap_or_default()
}

/// 返回具体种类或十二个当前材质的后缀推荐并集；Wiki 总表与分项的明确差异取兼容并集。
pub fn suffixes_for_kind(kind: &str) -> Vec<&'static str> {
    with_known_kind(kind, |kind| {
        if kind.recommends { suffixes::for_kind(kind.name).iter().collect() } else { Vec::new() }
    })
    .unwrap_or_default()
}

/// 名称解析已识别种类，直接复用其规范名称和大类，不再解析或分配推荐集合。
pub(super) fn classify_resolved_prefix(kind: &ResolvedKind, value: &str) -> AffixCompatibility {
    classify_known_prefix(kind.into(), value)
}

fn classify_known_prefix(kind: AffixKind<'_>, value: &str) -> AffixCompatibility {
    if value.is_empty() {
        return AffixCompatibility::Empty;
    }
    if kind.recommends
        && prefixes::for_kind(kind.name)
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(value))
    {
        return AffixCompatibility::Recommended;
    }
    const LEGACY: &[&str] = &[
        "Bronze", "Iron", "Silver", "Steel", "Gold", "Platinum", "Titanium", "Emerald", "Sapphire",
        "Diamond", "Prism",
    ];
    let legacy = LEGACY.iter().any(|candidate| candidate.eq_ignore_ascii_case(value))
        || ["-trimmed", "-adorned", "-tipped"].iter().any(|amendment| {
            value.get(value.len().saturating_sub(amendment.len())..).is_some_and(|tail| {
                tail.eq_ignore_ascii_case(amendment)
                    && LEGACY.iter().any(|candidate| {
                        value
                            .get(..value.len() - amendment.len())
                            .is_some_and(|head| candidate.eq_ignore_ascii_case(head))
                    })
            })
        })
        // Wiki 将这两个历史元素前缀归于 Weapons；其他大类只保留原文，不推定历史关系。
        || matches!(
            kind.category,
            Some(EquipmentCategory::OneHanded | EquipmentCategory::TwoHanded | EquipmentCategory::Staff)
        ) && ["Astral", "Quintessential"].iter().any(|candidate| candidate.eq_ignore_ascii_case(value));
    if legacy { AffixCompatibility::Legacy } else { AffixCompatibility::Unclassified }
}

/// 与前缀相同，沿用本次名称识别结果和借用的静态推荐集合。
pub(super) fn classify_resolved_suffix(kind: &ResolvedKind, value: &str) -> AffixCompatibility {
    classify_known_suffix(kind.into(), value)
}

fn classify_known_suffix(kind: AffixKind<'_>, value: &str) -> AffixCompatibility {
    if value.is_empty() {
        return AffixCompatibility::Empty;
    }
    if kind.recommends
        && suffixes::for_kind(kind.name).iter().any(|candidate| suffix_matches(candidate, value))
    {
        return AffixCompatibility::Recommended;
    }
    let category = kind.category;
    let legacy = category == Some(EquipmentCategory::TwoHanded) && suffix_matches("Focus", value)
        || matches!(
            category,
            Some(EquipmentCategory::Cloth | EquipmentCategory::Light | EquipmentCategory::Heavy)
        ) && ["Hulk", "Shielding Aura", "Priestess", "Stone-Skinned"]
            .iter()
            .any(|candidate| suffix_matches(candidate, value));
    if legacy { AffixCompatibility::Legacy } else { AffixCompatibility::Unclassified }
}

/// 沿用标色规则的比较口径：双方只移除一次大小写无关的 the，实际值非空时作 ASCII 忽略大小写比较。
pub fn suffix_matches(rule: &str, actual: &str) -> bool {
    let rule = strip_the(rule);
    let actual = strip_the(actual);
    !actual.is_empty() && rule.eq_ignore_ascii_case(actual)
}

fn strip_the(value: &str) -> &str {
    if value.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("the ")) {
        &value[4..]
    } else {
        value
    }
}

/// 字符串 API 只为已知完整种类或十二个合法材质建立分类上下文；未知种类无历史推定。
fn with_known_kind<T>(value: &str, classify: impl FnOnce(AffixKind<'_>) -> T) -> Option<T> {
    if let Some(material) = materials()
        .iter()
        .find(|material| material.selectable_family && material.name.eq_ignore_ascii_case(value))
    {
        // 材质 API 沿用原推荐语义；需要完整种类才能推定大类历史词缀。
        return Some(classify(AffixKind { name: material.name, category: None, recommends: true }));
    }
    let kind = resolve_kind(value).ok()?;
    Some(classify((&kind).into()))
}
