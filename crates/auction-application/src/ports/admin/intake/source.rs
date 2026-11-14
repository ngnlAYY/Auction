//! 外部装备来源适配器交给应用层的规范化快照。

use std::fmt;

use auction_domain::EquipmentKey;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::boundary::{AdminIntakeError, AdminIntakePolicy, decimal_identity, valid_text};

/// 外部来源适配器交给应用 owner 的规范化快照。
pub struct AdminIntakeSourceSnapshot {
    pub adapter: String,
    pub source_fingerprint: [u8; 32],
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub level: Option<i32>,
    pub tradeable: bool,
    pub attributes: Value,
    pub collected_at: DateTime<Utc>,
    pub key: EquipmentKey,
}

impl fmt::Debug for AdminIntakeSourceSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminIntakeSourceSnapshot")
            .field("adapter", &self.adapter)
            .field("source_fingerprint", &self.source_fingerprint)
            .field("external_equipment_id", &self.external_equipment_id)
            .field("name", &self.name)
            .field("category", &self.category)
            .field("level", &self.level)
            .field("tradeable", &self.tradeable)
            .field("attributes", &self.attributes)
            .field("collected_at", &self.collected_at)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl AdminIntakeSourceSnapshot {
    /// 校验适配器输出的结构、时间和属性形状，但保留 `tradeable` 供邮件策略给出稳定拒绝原因。
    pub fn validate_for_mail(&self, now: DateTime<Utc>) -> Result<(), AdminIntakeError> {
        if self.mail_validation_issue(now).is_some() {
            return Err(AdminIntakeError::InvalidSource);
        }
        Ok(())
    }

    /// 返回不含字段原文的首个失败分类；与原有快照校验共用唯一规则。
    pub fn mail_validation_issue(&self, now: DateTime<Utc>) -> Option<&'static str> {
        if !valid_text(&self.adapter, 128) {
            Some("adapter_invalid")
        } else if self.source_fingerprint == [0; 32] {
            Some("fingerprint_invalid")
        } else if !decimal_identity(&self.external_equipment_id) {
            Some("equipment_id_invalid")
        } else if !valid_text(&self.name, 512) {
            Some("name_invalid")
        } else if !valid_text(&self.category, 128) {
            Some("category_invalid")
        } else if self.level.is_some_and(|level| !(0..=100_000).contains(&level)) {
            Some("level_invalid")
        } else if self.collected_at.timestamp_micros() == 0 {
            Some("collected_at_invalid")
        } else if self.collected_at > now {
            Some("collected_at_future")
        } else if !self.attributes.is_object() {
            Some("attributes_invalid")
        } else {
            None
        }
    }

    /// 按 Admin 手工录入策略校验适配器输出；策略关闭时保留不可交易事实并继续录入。
    pub fn validate_for_admin(
        &self,
        now: DateTime<Utc>,
        policy: AdminIntakePolicy,
    ) -> Result<(), AdminIntakeError> {
        self.validate_for_mail(now)?;
        if policy.requires_tradeable() && !self.tradeable {
            return Err(AdminIntakeError::InvalidSource);
        }
        Ok(())
    }

    /// 清零短生命周期 key。
    pub fn clear_secret(&mut self) {
        self.key.clear();
    }
}
