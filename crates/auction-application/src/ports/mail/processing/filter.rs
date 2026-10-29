//! 邮件装备准入策略及其配置边界。

use std::collections::HashSet;

use auction_domain::valid_text;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::super::{AdminIntakeSourceSnapshot, AuctionEarlyBirdCalendar};
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

/// 未配置品质允许列表时的硬性默认值：只接收 Legendary 与 Peerless。
pub const DEFAULT_ACCEPTED_QUALITIES: [&str; 2] = ["Legendary", "Peerless"];

/// 生成拥有独立所有权的默认品质列表，供配置快照使用。
pub fn default_accepted_qualities() -> Vec<String> {
    DEFAULT_ACCEPTED_QUALITIES.iter().map(|quality| (*quality).to_owned()).collect()
}

/// MoogleMail 入站装备过滤策略。
/// 品质允许列表是始终生效的硬性门槛，空列表回退 Legendary/Peerless；其余空 allowlist
/// 表示不限制对应维度，等级上下限分别用 0 表示不限制。
/// `enabled` 只控制附加维度；品质门槛不受其影响。
/// 早鸟日历存在且邮件收件时间落在参与世界的早鸟窗口内时，品质门槛改用早鸟品质列表，
/// 其中空列表表示不限品质；其余维度规则不变。
/// 策略只消费已由装备来源适配器生成的安全快照，不接触外部页面正文或装备 key。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailEquipmentFilter {
    pub enabled: bool,
    /// 硬性允许品质；空列表回退默认 Legendary/Peerless。
    pub qualities: Vec<String>,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub categories: Vec<String>,
    pub equipment_types: Vec<String>,
    pub level_min: i32,
    pub level_max: i32,
    /// 已启用的赛季早鸟日历；`None` 表示始终使用常规品质门槛。
    pub early_bird: Option<AuctionEarlyBirdCalendar>,
}

impl Default for MailEquipmentFilter {
    fn default() -> Self {
        Self {
            enabled: false,
            qualities: default_accepted_qualities(),
            prefixes: Vec::new(),
            suffixes: Vec::new(),
            categories: Vec::new(),
            equipment_types: Vec::new(),
            level_min: 0,
            level_max: 0,
            early_bird: None,
        }
    }
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
        for list in values {
            if list.len() > 256 {
                return Err(MailProcessingError::InvalidConfiguration);
            }
            let mut seen = HashSet::with_capacity(list.len());
            for value in list {
                if !valid_text(value, 128) || !seen.insert(value.as_str()) {
                    return Err(MailProcessingError::InvalidConfiguration);
                }
            }
        }
        Ok(())
    }

    /// 返回当前装备按常规品质门槛的稳定拒绝原因；空字符串表示通过。
    pub fn evaluate(&self, snapshot: &AdminIntakeSourceSnapshot) -> &'static str {
        self.evaluate_with(snapshot, None)
    }

    /// 按邮件世界和收件时间选择品质门槛：早鸟窗口内用早鸟品质，其余用常规品质。
    /// 收件时间来自外部邮件的冻结事实，同一封邮件重放时得到相同结果。
    pub fn evaluate_received(
        &self,
        snapshot: &AdminIntakeSourceSnapshot,
        world_id: &str,
        received_at: DateTime<Utc>,
    ) -> &'static str {
        let early_bird = self
            .early_bird
            .as_ref()
            .filter(|calendar| calendar.window_contains(world_id, received_at))
            .map(AuctionEarlyBirdCalendar::qualities);
        self.evaluate_with(snapshot, early_bird)
    }

    fn evaluate_with(
        &self,
        snapshot: &AdminIntakeSourceSnapshot,
        early_bird_qualities: Option<&[String]>,
    ) -> &'static str {
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
        // 品质是硬性接收门槛，不随 enabled 开关关闭；低于门槛的邮件由调用方直接退回。
        let quality_accepted = match early_bird_qualities {
            Some(allowed) => {
                allowed.is_empty() || allowed.iter().any(|item| item.eq_ignore_ascii_case(quality))
            }
            None => quality_allowed(&self.qualities, quality),
        };
        if !quality_accepted {
            return EQUIPMENT_QUALITY_NOT_ALLOWED;
        }
        if !self.enabled {
            return "";
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

fn quality_allowed(allowed: &[String], value: &str) -> bool {
    if allowed.is_empty() {
        return DEFAULT_ACCEPTED_QUALITIES
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(value));
    }
    allowed.iter().any(|candidate| candidate.eq_ignore_ascii_case(value))
}
