//! 外部装备来源适配器交给应用层的规范化快照。

use super::*;

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
        if !valid_text(&self.adapter, 128)
            || self.source_fingerprint == [0; 32]
            || !decimal_identity(&self.external_equipment_id)
            || !valid_text(&self.name, 512)
            || !valid_text(&self.category, 128)
            || self.level.is_some_and(|level| !(0..=100_000).contains(&level))
            || self.collected_at.timestamp_micros() == 0
            || self.collected_at > now
            || !self.attributes.is_object()
        {
            return Err(AdminIntakeError::InvalidSource);
        }
        Ok(())
    }

    /// 校验适配器输出且拒绝未来时间、非 object attributes 和不可交易装备。
    pub fn validate(&self, now: DateTime<Utc>) -> Result<(), AdminIntakeError> {
        self.validate_for_mail(now)?;
        self.tradeable.then_some(()).ok_or(AdminIntakeError::InvalidSource)
    }

    /// 清零短生命周期 key。
    pub fn clear_secret(&mut self) {
        self.key.clear();
    }
}
