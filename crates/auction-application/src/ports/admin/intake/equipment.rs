//! Admin canonical 装备事实、来源类型和短生命周期密钥清理。

use super::*;

/// Admin 装备来源的闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminIntakeSourceKind {
    Url,
    Json,
}

impl AdminIntakeSourceKind {
    /// 返回冻结的外部契约名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Json => "json",
        }
    }
}

/// canonical JSON 中单个属性的公开值。
#[derive(Clone, Deserialize, Serialize)]
pub struct CanonicalEquipmentStat {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<f64>,
}

impl fmt::Debug for CanonicalEquipmentStat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalEquipmentStat")
            .field("value", &self.value)
            .field("base", &self.base)
            .finish()
    }
}

/// Admin canonical 装备输入。
/// `key` 只用于来源适配器即时生成密文信封，禁止进入持久化命令、日志或 HTTP 响应。
#[derive(Clone, Deserialize, Serialize)]
pub struct CanonicalEquipment {
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    /// 历史 canonical 字段保存完整基础名（如 `Phase Robe`）；来源适配器与领域种类/部位分开映射。
    pub equipment_type: String,
    pub quality: String,
    pub prefix: String,
    pub suffix: String,
    pub level: Option<i32>,
    pub tradeable: bool,
    pub stats: BTreeMap<String, CanonicalEquipmentStat>,
    #[serde(skip_serializing)]
    pub key: String,
}

impl fmt::Debug for CanonicalEquipment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalEquipment")
            .field("external_equipment_id", &self.external_equipment_id)
            .field("name", &self.name)
            .field("category", &self.category)
            .field("equipment_type", &self.equipment_type)
            .field("quality", &self.quality)
            .field("prefix", &self.prefix)
            .field("suffix", &self.suffix)
            .field("level", &self.level)
            .field("tradeable", &self.tradeable)
            .field("stats", &self.stats)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl Drop for CanonicalEquipment {
    fn drop(&mut self) {
        self.clear_secret();
    }
}

impl CanonicalEquipment {
    /// 清零 key 的 UTF-8 缓冲区，供请求生命周期结束前调用。
    pub fn clear_secret(&mut self) {
        let mut bytes = std::mem::take(&mut self.key).into_bytes();
        bytes.fill(0);
    }

    /// 校验 canonical 输入的边界和封闭字段。
    pub fn validate(&self) -> Result<(), AdminIntakeError> {
        if !decimal_identity(&self.external_equipment_id)
            || !valid_text(&self.name, 512)
            || !valid_text(&self.category, 128)
            || !valid_text(&self.equipment_type, 128)
            || !valid_text(&self.quality, 128)
            || !valid_optional_text(&self.prefix, 128)
            || !valid_optional_text(&self.suffix, 128)
            || self.level.is_some_and(|level| !(1..=100_000).contains(&level))
            || self.stats.len() > 256
            || EquipmentKey::new(&self.key).is_err()
        {
            return Err(AdminIntakeError::InvalidCommand);
        }
        for (name, stat) in &self.stats {
            if !valid_text(name, 128)
                || !valid_text(&stat.value, 256)
                || stat
                    .base
                    .is_some_and(|base| !base.is_finite() || !(0.0..=1_000_000.0).contains(&base))
            {
                return Err(AdminIntakeError::InvalidCommand);
            }
        }
        Ok(())
    }

    pub(super) fn digest_view(&self) -> CanonicalEquipmentDigest<'_> {
        let mut hasher = Sha256::new();
        hasher.update(self.key.as_bytes());
        let mut key_digest = [0_u8; 32];
        key_digest.copy_from_slice(&hasher.finalize());
        CanonicalEquipmentDigest {
            external_equipment_id: &self.external_equipment_id,
            name: &self.name,
            category: &self.category,
            equipment_type: &self.equipment_type,
            quality: &self.quality,
            prefix: &self.prefix,
            suffix: &self.suffix,
            level: self.level,
            tradeable: self.tradeable,
            stats: &self.stats,
            key_digest,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct CanonicalEquipmentDigest<'a> {
    pub(crate) external_equipment_id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) category: &'a str,
    pub(crate) equipment_type: &'a str,
    pub(crate) quality: &'a str,
    pub(crate) prefix: &'a str,
    pub(crate) suffix: &'a str,
    pub(crate) level: Option<i32>,
    pub(crate) tradeable: bool,
    pub(crate) stats: &'a BTreeMap<String, CanonicalEquipmentStat>,
    pub(crate) key_digest: [u8; 32],
}
