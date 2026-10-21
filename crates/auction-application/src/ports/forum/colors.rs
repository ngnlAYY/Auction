//! 论坛装备名称标色与公开页装备分组共用的规则；不参与价格、资格或结算。

use auction_domain::equipment::taxonomy::{armor_material_category, suffix_matches};

/// 一条按顺序匹配的论坛装备标色规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipmentColorRule {
    /// 装备类型，例如 `One-Handed`；空字符串表示不过滤。
    pub category: String,
    /// 装备种类，例如 `Rapier`；空字符串表示不过滤。
    pub equipment_type: String,
    /// 精确品质；空字符串表示不过滤。
    pub quality: String,
    /// 装备类型多选；非空时同维任一匹配，且旧 `category` 必须为空。
    pub categories: Vec<String>,
    /// 装备种类多选；支持十二种护甲材质族或精确完整种类，且旧 `equipment_type` 必须为空。
    pub equipment_types: Vec<String>,
    /// 品质多选；非空时同维任一匹配，且旧 `quality` 必须为空。
    pub qualities: Vec<String>,
    /// 等级下限；0 表示不限制。
    pub level_min: i32,
    /// 等级上限；0 表示不限制。
    pub level_max: i32,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub prefix_color: String,
    pub suffix_color: String,
    pub combined_color: String,
}

/// 单条规则的命中部分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EquipmentColorMatch {
    pub prefix: bool,
    pub suffix: bool,
}

/// 公开页的装备分组；按同一规则是否实际渲染论坛颜色确定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentGroup {
    Rare,
    Fusion,
}

impl EquipmentGroup {
    /// 返回稳定的公开 JSON 和查询参数值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rare => "rare",
            Self::Fusion => "fusion",
        }
    }

    /// 只接受已定义分组；全部分组由查询参数省略表示。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "rare" => Some(Self::Rare),
            "fusion" => Some(Self::Fusion),
            _ => None,
        }
    }
}

/// 论坛和公开页使用的同一份装备身份；种类保留存储中的完整基础名称。
#[derive(Debug, Clone, Copy)]
pub struct EquipmentColorFacts<'a> {
    pub name: &'a str,
    pub category: &'a str,
    pub equipment_type: &'a str,
    pub quality: &'a str,
    pub level: Option<i64>,
    pub prefix: &'a str,
    pub suffix: &'a str,
}

/// 一次匹配同时给出论坛渲染方案和公开页分组，保持首条规则优先级一致。
#[derive(Debug, Clone, Copy)]
pub struct EquipmentColorDecision<'a> {
    pub peerless: bool,
    pub matched: Option<(&'a EquipmentColorRule, EquipmentColorMatch)>,
    pub equipment_group: EquipmentGroup,
}

/// Peerless 固定彩虹视为稀有；普通装备只有首条命中规则实际给名称片段上色才是稀有。
/// 等级 0 按论坛快照视为未知等级；历史属性与名称不一致时，单边词缀在名称中找不到仍保持融合，与论坛的纯文本回退一致。
pub fn equipment_color_decision<'a>(
    rules: &'a [EquipmentColorRule],
    facts: EquipmentColorFacts<'_>,
) -> EquipmentColorDecision<'a> {
    let peerless = facts.quality.eq_ignore_ascii_case("Peerless")
        && facts.name.get(..8).is_some_and(|value| value.eq_ignore_ascii_case("Peerless"));
    let matched = rules.iter().find_map(|rule| {
        rule.matches(
            facts.category,
            facts.equipment_type,
            facts.quality,
            facts.level.filter(|level| *level > 0),
            facts.prefix,
            facts.suffix,
        )
        .map(|matched| (rule, matched))
    });
    let colored = peerless
        || matched.is_some_and(|(_, matched)| {
            matched.prefix && matched.suffix
                || matched.prefix
                    && facts.name.to_ascii_lowercase().contains(&facts.prefix.to_ascii_lowercase())
                || matched.suffix
                    && facts.name.to_ascii_lowercase().contains(&facts.suffix.to_ascii_lowercase())
        });
    EquipmentColorDecision {
        peerless,
        matched,
        equipment_group: if colored { EquipmentGroup::Rare } else { EquipmentGroup::Fusion },
    }
}

impl EquipmentColorRule {
    /// 类型、种类、品质各维度内任一匹配、维度间同时满足，再校验等级和前后缀。
    /// 新旧选择同维并存时失败关闭；规则顺序及前后缀任一命中的行为保持不变。
    pub fn matches(
        &self,
        category: &str,
        equipment_type: &str,
        quality: &str,
        level: Option<i64>,
        prefix: &str,
        suffix: &str,
    ) -> Option<EquipmentColorMatch> {
        if !selection_matches(&self.category, &self.categories, category)
            || !equipment_type_matches(
                &self.equipment_type,
                &self.equipment_types,
                category,
                equipment_type,
            )
            || !selection_matches(&self.quality, &self.qualities, quality)
        {
            return None;
        }
        if self.level_min > 0 || self.level_max > 0 {
            let level = i32::try_from(level?).ok()?;
            if self.level_min > 0 && level < self.level_min {
                return None;
            }
            if self.level_max > 0 && level > self.level_max {
                return None;
            }
        }
        let prefix = self
            .prefixes
            .iter()
            .any(|value| !prefix.is_empty() && value.eq_ignore_ascii_case(prefix));
        let suffix = self.suffixes.iter().any(|value| suffix_matches(value, suffix));
        (prefix || suffix).then_some(EquipmentColorMatch { prefix, suffix })
    }
}

/// Peerless 品质词的固定彩虹颜色，顺序对应 P/e/e/r/l/e/s/s。
pub const PEERLESS_COLORS: [&str; 8] =
    ["#ff0000", "#ff7f00", "#c3c300", "#00ff00", "#00ffff", "#7171ff", "#8b00ff", "#ff0000"];

/// 校验规则集合的闭集边界；颜色必须是 `#rrggbb`。
pub fn validate_equipment_color_rules(rules: &[EquipmentColorRule]) -> Result<(), String> {
    if rules.len() > 200 {
        return Err("equipment_colors.rules 最多 200 条".to_owned());
    }
    for (index, rule) in rules.iter().enumerate() {
        for (field, single, values) in [
            ("categories", &rule.category, &rule.categories),
            ("equipment_types", &rule.equipment_type, &rule.equipment_types),
            ("qualities", &rule.quality, &rule.qualities),
        ] {
            if !valid_selection(single, values) {
                return Err(format!(
                    "equipment_colors.rules[{index}].{field} 无效：单值与多选不能并存，条目须规范且不重复"
                ));
            }
        }
        if !(0..=1000).contains(&rule.level_min)
            || !(0..=1000).contains(&rule.level_max)
            || rule.level_max != 0 && rule.level_max < rule.level_min
        {
            return Err(format!("equipment_colors.rules[{index}].level 无效"));
        }
        if rule.prefixes.len() > 256 || rule.suffixes.len() > 256 {
            return Err(format!("equipment_colors.rules[{index}] 前后缀列表过长"));
        }
        for values in [&rule.prefixes, &rule.suffixes] {
            for (value_index, value) in values.iter().enumerate() {
                if value.is_empty()
                    || value.len() > 128
                    || value != value.trim()
                    || value.chars().any(char::is_control)
                    || values[..value_index].iter().any(|previous| previous == value)
                {
                    return Err(format!("equipment_colors.rules[{index}] 前后缀条目重复或无效"));
                }
            }
        }
        if rule.prefixes.is_empty() && rule.suffixes.is_empty() {
            return Err(format!("equipment_colors.rules[{index}] 至少需要一个前缀或后缀"));
        }
        for color in [&rule.prefix_color, &rule.suffix_color, &rule.combined_color] {
            if !valid_hex_color(color) {
                return Err(format!("equipment_colors.rules[{index}] 颜色必须是 #rrggbb"));
            }
        }
    }
    Ok(())
}

fn selection_matches(single: &str, values: &[String], actual: &str) -> bool {
    if values.is_empty() {
        single.is_empty() || single.eq_ignore_ascii_case(actual)
    } else {
        single.is_empty() && values.iter().any(|value| value.eq_ignore_ascii_case(actual))
    }
}

// 材质族只赋予新多选数组语义；旧单值继续精确匹配，不能因升级扩大历史规则。
// 同时核对实际大类与完整种类的词边界，避免前缀子串或跨类别名称被视为护甲。
fn equipment_type_matches(single: &str, values: &[String], category: &str, actual: &str) -> bool {
    if values.is_empty() {
        return single.is_empty() || single.eq_ignore_ascii_case(actual);
    }
    if !single.is_empty() {
        return false;
    }
    values.iter().any(|value| match armor_material_category(value) {
        Some(expected_category) => {
            category.eq_ignore_ascii_case(expected_category.as_str())
                && actual.split_once(' ').is_some_and(|(material, slot)| {
                    material.eq_ignore_ascii_case(value) && !slot.trim().is_empty()
                })
        }
        None => value.eq_ignore_ascii_case(actual),
    })
}

// 接受历史自定义名称，不把 Admin 候选词表变成采集白名单；保存值须已去首尾空白，
// 大小写不敏感的重复条目会产生同一匹配，统一拒绝，避免文档与执行语义分叉。
fn valid_selection(single: &str, values: &[String]) -> bool {
    let valid_text = |value: &str| {
        value.len() <= 64 && value == value.trim() && !value.chars().any(char::is_control)
    };
    valid_text(single)
        && (single.is_empty() || values.is_empty())
        && values.len() <= 256
        && values.iter().enumerate().all(|(index, value)| {
            !value.is_empty()
                && valid_text(value)
                && !values[..index].iter().any(|previous| previous.eq_ignore_ascii_case(value))
        })
}

fn valid_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}
