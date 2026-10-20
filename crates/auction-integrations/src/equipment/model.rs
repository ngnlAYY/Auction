//! 装备来源归一化模型与稳定指纹；模型不携带原始 HTML，装备 key 不写入 fingerprint 或属性 JSON。

use std::collections::BTreeMap;

use auction_application::{AdminIntakeError, CanonicalEquipment};
use serde::Serialize;

use super::parser::{EquipmentLevel, EquipmentParseError, normalize_stat_value, parse_identity};

pub(super) struct EquipmentFacts {
    pub(super) external_equipment_id: String,
    pub(super) name: String,
    pub(super) category: String,
    pub(super) equipment_type: String,
    pub(super) quality: String,
    pub(super) prefix: String,
    pub(super) suffix: String,
    pub(super) level: EquipmentLevel,
    pub(super) tradeable: bool,
    pub(super) stats: BTreeMap<String, EquipmentStat>,
}

#[derive(Clone, Serialize)]
pub(super) struct EquipmentStat {
    pub(super) value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) base: Option<f64>,
}

impl EquipmentFacts {
    pub(super) fn attributes(&self) -> Result<serde_json::Value, AdminIntakeError> {
        serde_json::to_value(serde_json::json!({
            "type": self.equipment_type,
            "quality": self.quality,
            "prefix": self.prefix,
            "suffix": self.suffix,
            "level": self.level.legacy_level(),
            "tradeable": self.tradeable,
            "stats": self.stats,
        }))
        .map_err(|_| AdminIntakeError::InvalidSource)
    }
}

pub(super) fn canonical_facts(
    input: &CanonicalEquipment,
) -> Result<EquipmentFacts, EquipmentParseError> {
    let identity = parse_identity(&input.name)?;
    if identity.category.as_str() != input.category {
        return Err(EquipmentParseError::CategoryMismatch);
    }
    if identity.quality != input.quality
        || identity.prefix != input.prefix
        || identity.equipment_type != input.equipment_type
        || identity.suffix != input.suffix
    {
        return Err(EquipmentParseError::CanonicalMismatch);
    }
    let stats = input
        .stats
        .iter()
        .map(|(name, stat)| {
            let value =
                normalize_stat_value(&stat.value).ok_or(EquipmentParseError::InvalidAttributes)?;
            if stat
                .base
                .is_some_and(|base| !base.is_finite() || !(0.0..=1_000_000.0).contains(&base))
            {
                return Err(EquipmentParseError::InvalidAttributes);
            }
            Ok((name.clone(), EquipmentStat { value, base: stat.base }))
        })
        .collect::<Result<BTreeMap<_, _>, EquipmentParseError>>()?;
    Ok(EquipmentFacts {
        external_equipment_id: input.external_equipment_id.clone(),
        name: input.name.clone(),
        category: input.category.clone(),
        equipment_type: identity.equipment_type,
        quality: identity.quality,
        prefix: identity.prefix,
        suffix: identity.suffix,
        level: EquipmentLevel::from_legacy(input.level),
        tradeable: input.tradeable,
        stats,
    })
}

#[derive(Serialize)]
pub(super) struct CanonicalFingerprint<'a> {
    pub(super) world: &'a str,
    pub(super) external_equipment_id: &'a str,
    pub(super) name: &'a str,
    pub(super) category: &'a str,
    pub(super) equipment_type: &'a str,
    pub(super) quality: &'a str,
    pub(super) prefix: &'a str,
    pub(super) suffix: &'a str,
    pub(super) level: Option<i32>,
    pub(super) tradeable: bool,
    pub(super) stats: &'a BTreeMap<String, EquipmentStat>,
}

pub(super) fn canonical_fingerprint(
    world_id: &str,
    facts: &EquipmentFacts,
) -> Result<[u8; 32], AdminIntakeError> {
    let encoded = serde_json::to_vec(&CanonicalFingerprint {
        world: world_id,
        external_equipment_id: &facts.external_equipment_id,
        name: &facts.name,
        category: &facts.category,
        equipment_type: &facts.equipment_type,
        quality: &facts.quality,
        prefix: &facts.prefix,
        suffix: &facts.suffix,
        level: facts.level.legacy_level(),
        tradeable: facts.tradeable,
        stats: &facts.stats,
    })
    .map_err(|_| AdminIntakeError::InvalidSource)?;
    let mut input = b"manual-json-v1\0".to_vec();
    input.extend_from_slice(&encoded);
    Ok(sha256(input))
}

pub(super) fn sha256(bytes: Vec<u8>) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}
