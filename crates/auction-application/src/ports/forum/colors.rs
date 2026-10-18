//! 论坛装备名称标色规则；只影响外部帖子渲染，不参与价格、资格或结算。

/// 一条按顺序匹配的论坛装备标色规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipmentColorRule {
    /// 装备类型，例如 `One-Handed`；空字符串表示不过滤。
    pub category: String,
    /// 装备种类，例如 `Rapier`；空字符串表示不过滤。
    pub equipment_type: String,
    /// 精确品质；空字符串表示不过滤。
    pub quality: String,
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

impl EquipmentColorRule {
    /// 按顺序命中第一条；类型、品质、等级、前缀、后缀全部满足才返回。
    pub fn matches(
        &self,
        category: &str,
        equipment_type: &str,
        quality: &str,
        level: Option<i64>,
        prefix: &str,
        suffix: &str,
    ) -> Option<EquipmentColorMatch> {
        if !self.category.is_empty() && !self.category.eq_ignore_ascii_case(category) {
            return None;
        }
        if !self.equipment_type.is_empty()
            && !self.equipment_type.eq_ignore_ascii_case(equipment_type)
        {
            return None;
        }
        if !self.quality.is_empty() && !self.quality.eq_ignore_ascii_case(quality) {
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
        if rule.category.len() > 64
            || rule.category != rule.category.trim()
            || rule.category.chars().any(char::is_control)
        {
            return Err(format!("equipment_colors.rules[{index}].category 无效"));
        }
        if rule.equipment_type.len() > 64
            || rule.equipment_type != rule.equipment_type.trim()
            || rule.equipment_type.chars().any(char::is_control)
        {
            return Err(format!("equipment_colors.rules[{index}].equipment_type 无效"));
        }
        if rule.quality.len() > 64
            || rule.quality != rule.quality.trim()
            || rule.quality.chars().any(char::is_control)
        {
            return Err(format!("equipment_colors.rules[{index}].quality 无效"));
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

/// 后缀匹配忽略大小写和可选的 `the ` 前缀；标色仍使用帖子中的完整后缀文本。
fn suffix_matches(rule: &str, actual: &str) -> bool {
    let actual = strip_the(actual);
    !actual.is_empty() && strip_the(rule).eq_ignore_ascii_case(actual)
}

fn strip_the(value: &str) -> &str {
    value
        .get(..4)
        .filter(|prefix| prefix.eq_ignore_ascii_case("the "))
        .map_or(value, |_| &value[4..])
}

fn valid_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}
