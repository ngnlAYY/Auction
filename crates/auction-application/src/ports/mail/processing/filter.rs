//! 邮件装备准入策略及其配置边界。

use std::collections::HashSet;

use auction_domain::AuctionEarlyBirdCalendar;
use auction_domain::valid_text;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::errors::MailProcessingError;
use super::rejection::EquipmentRejectionReason;
use crate::ports::admin::AdminIntakeSourceSnapshot;

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

/// 装备过滤配置越界的类别；各调用方映射为自己的错误类型与文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentFilterBoundsError {
    /// 等级不在 0 至 1000，或非零上限小于下限。
    Level,
    /// 某个 token 列表超过 256 项。
    TooManyTokens,
    /// token 重复，或不是 1 至 128 字节的规范单行文本。
    InvalidToken,
}

/// 装备过滤边界的唯一规则：Admin 保存 runtime 文档、Worker 加载配置与邮件处理共用，
/// 避免出现“Admin 接受、Worker 启动拒绝”的分歧。等级 0 表示不限制；token 必须满足
/// [`valid_text`]（拒绝任何控制字符，含 Tab），且在各自列表内唯一。
pub fn validate_equipment_filter_bounds(
    level_min: i32,
    level_max: i32,
    token_lists: [&[String]; 5],
) -> Result<(), EquipmentFilterBoundsError> {
    if !(0..=1000).contains(&level_min)
        || !(0..=1000).contains(&level_max)
        || level_max != 0 && level_max < level_min
    {
        return Err(EquipmentFilterBoundsError::Level);
    }
    for list in token_lists {
        if list.len() > 256 {
            return Err(EquipmentFilterBoundsError::TooManyTokens);
        }
        let mut seen = HashSet::with_capacity(list.len());
        for value in list {
            if !valid_text(value, 128) || !seen.insert(value.as_str()) {
                return Err(EquipmentFilterBoundsError::InvalidToken);
            }
        }
    }
    Ok(())
}

impl MailEquipmentFilter {
    /// 校验配置边界，避免运行时用未审计的 token 或空策略拦截所有邮件。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        let token_lists = [
            &self.qualities,
            &self.prefixes,
            &self.suffixes,
            &self.categories,
            &self.equipment_types,
        ]
        .map(Vec::as_slice);
        if validate_equipment_filter_bounds(self.level_min, self.level_max, token_lists).is_err() {
            return Err(MailProcessingError::InvalidConfiguration);
        }
        Ok(())
    }

    /// 按邮件世界和收件时间选择品质门槛：早鸟窗口内用早鸟品质，其余用常规品质。
    /// 收件时间来自外部邮件的冻结事实，同一封邮件重放时得到相同结果。
    pub(crate) fn received_qualities(
        &self,
        world_id: &str,
        received_at: DateTime<Utc>,
    ) -> Option<&[String]> {
        self.early_bird
            .as_ref()
            .filter(|calendar| calendar.window_contains(world_id, received_at))
            .map(AuctionEarlyBirdCalendar::qualities)
    }

    /// 复用已选定的早鸟品质，逐件保留可交易性、属性和附加维度的原拒绝顺序；`None` 表示接受。
    pub(crate) fn evaluate_with(
        &self,
        snapshot: &AdminIntakeSourceSnapshot,
        early_bird_qualities: Option<&[String]>,
    ) -> Option<EquipmentRejectionReason> {
        if !snapshot.tradeable {
            return Some(EquipmentRejectionReason::TradeableRequired);
        }
        let Some(attributes) = snapshot.attributes.as_object() else {
            return Some(EquipmentRejectionReason::AttributesInvalid);
        };
        let text = |name: &str| match attributes.get(name) {
            None => Some(""),
            Some(Value::String(value)) => Some(value.as_str()),
            Some(_) => None,
        };
        let Some(equipment_type) = text("type") else {
            return Some(EquipmentRejectionReason::AttributesInvalid);
        };
        let Some(quality) = text("quality") else {
            return Some(EquipmentRejectionReason::AttributesInvalid);
        };
        let Some(prefix) = text("prefix") else {
            return Some(EquipmentRejectionReason::AttributesInvalid);
        };
        let Some(suffix) = text("suffix") else {
            return Some(EquipmentRejectionReason::AttributesInvalid);
        };
        match attributes.get("tradeable") {
            Some(Value::Bool(true)) => {}
            Some(Value::Bool(false)) | None | Some(Value::Null) => {
                return Some(EquipmentRejectionReason::TradeableRequired);
            }
            Some(_) => return Some(EquipmentRejectionReason::AttributesInvalid),
        }
        let level = match attributes.get("level") {
            None | Some(Value::Null) => None,
            Some(Value::Number(value)) => {
                match value.as_i64().and_then(|value| i32::try_from(value).ok()) {
                    Some(value) => Some(value),
                    None => return Some(EquipmentRejectionReason::AttributesInvalid),
                }
            }
            Some(_) => return Some(EquipmentRejectionReason::AttributesInvalid),
        };
        // 品质是硬性接收门槛，不随 enabled 开关关闭；低于门槛的邮件由调用方直接退回。
        let quality_accepted = match early_bird_qualities {
            Some(allowed) => {
                allowed.is_empty() || allowed.iter().any(|item| item.eq_ignore_ascii_case(quality))
            }
            None => quality_allowed(&self.qualities, quality),
        };
        if !quality_accepted {
            return Some(EquipmentRejectionReason::QualityNotAllowed);
        }
        if !self.enabled {
            return None;
        }
        if !allowed_value(&self.prefixes, prefix) {
            return Some(EquipmentRejectionReason::PrefixNotAllowed);
        }
        if !allowed_value(&self.suffixes, suffix) {
            return Some(EquipmentRejectionReason::SuffixNotAllowed);
        }
        if !allowed_value(&self.categories, &snapshot.category) {
            return Some(EquipmentRejectionReason::CategoryNotAllowed);
        }
        if !allowed_value(&self.equipment_types, equipment_type) {
            return Some(EquipmentRejectionReason::EquipmentTypeNotAllowed);
        }
        if (self.level_min != 0 || self.level_max != 0)
            && level.is_none_or(|value| {
                value < self.level_min || self.level_max != 0 && value > self.level_max
            })
        {
            return Some(EquipmentRejectionReason::LevelNotAllowed);
        }
        None
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

#[cfg(test)]
mod tests {
    use auction_domain::EquipmentKey;
    use chrono::{DateTime, Utc};
    use serde_json::{Value, json};

    use super::MailEquipmentFilter;
    use crate::ports::admin::AdminIntakeSourceSnapshot;
    use crate::ports::mail::{
        EquipmentRejectionReason, MailEquipmentDiagnosticCategory, MailEquipmentDiagnosticSnapshot,
        MailEquipmentDiagnosticStage, MailEquipmentEvidenceStatus, MailNotificationFields,
        MailNotificationTemplate,
    };

    fn attributes() -> Value {
        json!({
            "type": "Two-Handed",
            "quality": "Legendary",
            "prefix": "Ethereal",
            "suffix": "of the Vampire",
            "tradeable": true,
            "level": 100,
        })
    }

    fn snapshot(attributes: Value) -> AdminIntakeSourceSnapshot {
        AdminIntakeSourceSnapshot {
            adapter: "test".to_owned(),
            source_fingerprint: [7; 32],
            external_equipment_id: "900000001".to_owned(),
            name: "Legendary Ethereal Great Mace of the Vampire".to_owned(),
            category: "Two-Handed".to_owned(),
            level: Some(100),
            tradeable: true,
            attributes,
            collected_at: DateTime::<Utc>::UNIX_EPOCH,
            key: EquipmentKey::new("0123456789").expect("固定装备 key"),
        }
    }

    fn with(name: &str, value: Value) -> AdminIntakeSourceSnapshot {
        let mut attributes = attributes();
        attributes[name] = value;
        snapshot(attributes)
    }

    /// 过滤器的每条拒收分支都产出模板表闭集内的原因，说明信能够入队，诊断投影也接受同一原因；
    /// 从任一闭集删掉原因，这里失败，而不是让生产环境的入站邮件反复重试。
    #[test]
    fn filter_rejections_produce_bound_reasons() {
        let filter = MailEquipmentFilter {
            enabled: true,
            prefixes: vec!["Ethereal".to_owned()],
            suffixes: vec!["of the Vampire".to_owned()],
            categories: vec!["Two-Handed".to_owned()],
            equipment_types: vec!["Two-Handed".to_owned()],
            level_min: 100,
            level_max: 200,
            ..MailEquipmentFilter::default()
        };
        assert_eq!(filter.evaluate_with(&snapshot(attributes()), None), None, "全部维度命中时接受");

        let mut untradeable = snapshot(attributes());
        untradeable.tradeable = false;
        let mut other_category = snapshot(attributes());
        other_category.category = "One-Handed".to_owned();
        let cases = [
            (untradeable, EquipmentRejectionReason::TradeableRequired),
            (snapshot(json!([])), EquipmentRejectionReason::AttributesInvalid),
            (with("quality", json!(1)), EquipmentRejectionReason::AttributesInvalid),
            (with("tradeable", json!(false)), EquipmentRejectionReason::TradeableRequired),
            (with("quality", json!("Superior")), EquipmentRejectionReason::QualityNotAllowed),
            (with("prefix", json!("Charged")), EquipmentRejectionReason::PrefixNotAllowed),
            (with("suffix", json!("of Slaughter")), EquipmentRejectionReason::SuffixNotAllowed),
            (other_category, EquipmentRejectionReason::CategoryNotAllowed),
            (with("type", json!("Staff")), EquipmentRejectionReason::EquipmentTypeNotAllowed),
            (with("level", json!(50)), EquipmentRejectionReason::LevelNotAllowed),
        ];
        let binding = MailNotificationTemplate::AuctionEquipmentRejected.binding();
        for (snapshot, expected) in cases {
            assert_eq!(filter.evaluate_with(&snapshot, None), Some(expected));
            assert!(
                binding.admits(MailNotificationFields {
                    origin: binding.origin.as_str(),
                    world: "main",
                    result_code: Some(expected.as_str()),
                    user_key_user_id: None,
                    body: None,
                }),
                "{expected:?} 不在拒收说明的结果码闭集内"
            );
            // 处理器把同一原因写成过滤拒收诊断；诊断闭集与数据库 CHECK 同口径，漏登记同样卡住游标。
            let diagnostic = MailEquipmentDiagnosticSnapshot {
                schema_version: 1,
                equipment_id: "900000001".to_owned(),
                attachment_position: 1,
                stage: MailEquipmentDiagnosticStage::Validate,
                category: MailEquipmentDiagnosticCategory::FilterRejected,
                detail_code: Some(expected.as_str().to_owned()),
                http_status: None,
                elapsed_millis: None,
                evidence_id: None,
                evidence_status: MailEquipmentEvidenceStatus::Disabled,
                retention_error: None,
            };
            assert!(diagnostic.validate(), "{expected:?} 不在过滤拒收诊断的闭集内");
        }
    }
}
