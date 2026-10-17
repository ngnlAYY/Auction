use serde_json::Value;

use super::super::AdminIntakeSourceSnapshot;
use super::common::MailProcessingError;

/// 装备过滤策略返回的稳定拒绝原因。
pub const EQUIPMENT_QUALITY_NOT_ALLOWED: &str = "quality_not_allowed";
pub const EQUIPMENT_PREFIX_NOT_ALLOWED: &str = "prefix_not_allowed";
pub const EQUIPMENT_SUFFIX_NOT_ALLOWED: &str = "suffix_not_allowed";
pub const EQUIPMENT_CATEGORY_NOT_ALLOWED: &str = "category_not_allowed";
pub const EQUIPMENT_TYPE_NOT_ALLOWED: &str = "equipment_type_not_allowed";
pub const EQUIPMENT_LEVEL_NOT_ALLOWED: &str = "level_not_allowed";
pub const EQUIPMENT_ATTRIBUTES_INVALID: &str = "attributes_invalid";
pub const EQUIPMENT_TRADEABLE_REQUIRED: &str = "tradeable_required";

/// MoogleMail 入站装备过滤策略。
/// 空的 allowlist 表示不限制对应维度；等级上下限分别用 0 表示不限制。
/// 启用策略时至少要配置一个维度或等级范围。
/// 策略只消费已由装备来源适配器生成的安全快照，不接触外部页面正文或装备 key。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MailEquipmentFilter {
    pub enabled: bool,
    pub qualities: Vec<String>,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub categories: Vec<String>,
    pub equipment_types: Vec<String>,
    pub level_min: i32,
    pub level_max: i32,
}

impl MailEquipmentFilter {
    /// 校验配置边界，避免运行时用未审计的 token 或空策略拦截所有邮件。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        if !(0..=1000).contains(&self.level_min)
            || !(0..=1000).contains(&self.level_max)
            || self.level_max != 0 && self.level_max < self.level_min
        {
            return Err(MailProcessingError::InvalidConfiguration);
        }
        let values = [
            &self.qualities,
            &self.prefixes,
            &self.suffixes,
            &self.categories,
            &self.equipment_types,
        ];
        let mut filter_count = 0;
        for list in values {
            if list.len() > 256 {
                return Err(MailProcessingError::InvalidConfiguration);
            }
            filter_count += list.len();
            for (index, value) in list.iter().enumerate() {
                if value.is_empty()
                    || value.len() > 128
                    || value != value.trim()
                    || value.chars().any(char::is_control)
                    || list[..index].iter().any(|previous| previous == value)
                {
                    return Err(MailProcessingError::InvalidConfiguration);
                }
            }
        }
        if self.enabled && filter_count == 0 && self.level_min == 0 && self.level_max == 0 {
            return Err(MailProcessingError::InvalidConfiguration);
        }
        Ok(())
    }

    /// 返回当前装备的稳定拒绝原因；空字符串表示通过。
    pub fn evaluate(&self, snapshot: &AdminIntakeSourceSnapshot) -> &'static str {
        if !snapshot.tradeable {
            return EQUIPMENT_TRADEABLE_REQUIRED;
        }
        let Some(attributes) = snapshot.attributes.as_object() else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        let text = |name: &str| match attributes.get(name) {
            None => Some(""),
            Some(Value::String(value)) => Some(value.as_str()),
            Some(_) => None,
        };
        let Some(equipment_type) = text("type") else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        let Some(quality) = text("quality") else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        let Some(prefix) = text("prefix") else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        let Some(suffix) = text("suffix") else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        match attributes.get("tradeable") {
            Some(Value::Bool(true)) => {}
            Some(Value::Bool(false)) | None | Some(Value::Null) => {
                return EQUIPMENT_TRADEABLE_REQUIRED;
            }
            Some(_) => return EQUIPMENT_ATTRIBUTES_INVALID,
        }
        let level = match attributes.get("level") {
            None | Some(Value::Null) => None,
            Some(Value::Number(value)) => {
                match value.as_i64().and_then(|value| i32::try_from(value).ok()) {
                    Some(value) => Some(value),
                    None => return EQUIPMENT_ATTRIBUTES_INVALID,
                }
            }
            Some(_) => return EQUIPMENT_ATTRIBUTES_INVALID,
        };
        if !self.enabled {
            return "";
        }
        if !allowed_value(&self.qualities, quality) {
            return EQUIPMENT_QUALITY_NOT_ALLOWED;
        }
        if !allowed_value(&self.prefixes, prefix) {
            return EQUIPMENT_PREFIX_NOT_ALLOWED;
        }
        if !allowed_value(&self.suffixes, suffix) {
            return EQUIPMENT_SUFFIX_NOT_ALLOWED;
        }
        if !allowed_value(&self.categories, &snapshot.category) {
            return EQUIPMENT_CATEGORY_NOT_ALLOWED;
        }
        if !allowed_value(&self.equipment_types, equipment_type) {
            return EQUIPMENT_TYPE_NOT_ALLOWED;
        }
        if (self.level_min != 0 || self.level_max != 0)
            && level.is_none_or(|value| {
                value < self.level_min || self.level_max != 0 && value > self.level_max
            })
        {
            return EQUIPMENT_LEVEL_NOT_ALLOWED;
        }
        ""
    }
}

fn allowed_value(allowed: &[String], value: &str) -> bool {
    allowed.is_empty() || allowed.iter().any(|candidate| candidate == value)
}
