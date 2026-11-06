//! 装备来源快照归一：把 canonical JSON 与解析后的页面事实折叠成同一份脱敏快照。

use auction_application::admin::{AdminIntakeError, AdminIntakeSourceSnapshot, CanonicalEquipment};
use auction_domain::EquipmentKey;
use chrono::Utc;

use super::super::model::{EquipmentFacts, canonical_facts, canonical_fingerprint, sha256};
use super::diagnostics;

/// canonical JSON 录入不经网络：W1.10 后 Admin 仍在本进程完成该分支，
/// URL 分支则改为经 Worker control 执行。
pub fn canonical_json_snapshot(
    world_id: &str,
    input: CanonicalEquipment,
) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
    if !matches!(world_id, "main" | "isekai") {
        return Err(AdminIntakeError::InvalidCommand);
    }
    input.validate()?;
    let facts = canonical_facts(&input)
        .map_err(|reason| diagnostics::source_parse_error("canonical_json", reason))?;
    let key = EquipmentKey::new(&input.key).map_err(|_| AdminIntakeError::InvalidSource)?;
    let fingerprint = canonical_fingerprint(world_id, &facts)?;
    let attributes = facts.attributes();
    Ok(AdminIntakeSourceSnapshot {
        adapter: "manual-json-v1".to_owned(),
        source_fingerprint: fingerprint,
        external_equipment_id: facts.external_equipment_id,
        name: facts.name,
        category: facts.category,
        level: facts.level.legacy_level(),
        tradeable: facts.tradeable,
        attributes,
        collected_at: Utc::now(),
        key,
    })
}

pub(super) fn facts_to_snapshot(
    world_id: &str,
    facts: EquipmentFacts,
    key: EquipmentKey,
) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
    let attributes = facts.attributes();
    let mut fingerprint_input = Vec::new();
    fingerprint_input.extend_from_slice(b"hentaiverse-equipment-v1\0");
    fingerprint_input.extend_from_slice(world_id.as_bytes());
    fingerprint_input.push(0);
    fingerprint_input.extend_from_slice(facts.external_equipment_id.as_bytes());
    Ok(AdminIntakeSourceSnapshot {
        adapter: "hentaiverse-equipment-v1".to_owned(),
        source_fingerprint: sha256(fingerprint_input),
        external_equipment_id: facts.external_equipment_id,
        name: facts.name,
        category: facts.category,
        level: facts.level.legacy_level(),
        tradeable: facts.tradeable,
        attributes,
        collected_at: Utc::now(),
        key,
    })
}
