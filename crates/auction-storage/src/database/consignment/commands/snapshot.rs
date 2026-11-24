//! 合规观察的安全快照写入与独立摘要。

use auction_application::consignment::{
    LISTING_EQUIPMENT_SOURCE, ListingEquipmentLevel, ListingLookupSnapshot,
};
use auction_application::core::RequestHash;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::errors::WriteError;

/// 只写端口已核验的字段。持有人只供私有证据，摘要排除租约、时间、原文和凭证。不变量 3、7、8。
pub(super) async fn insert(
    conn: &mut PgConnection,
    snapshot_id: Uuid,
    listing_id: Uuid,
    generation: u64,
    snapshot: &ListingLookupSnapshot,
) -> Result<(), WriteError> {
    let details = snapshot.details();
    let label = details.label();
    let (level_kind, level_number) = match label.level {
        ListingEquipmentLevel::Assigned(level) => ("assigned", Some(level.cast_signed())),
        ListingEquipmentLevel::Unassigned => ("unassigned", None),
        ListingEquipmentLevel::Soulbound => ("soulbound", None),
    };
    let identity = snapshot.identity();
    let stat_groups: Vec<_> = details.stat_groups().iter().map(|group| {
        let stats: Vec<_> = group.stats.iter().map(|stat| {
            json!({"name": stat.name, "value": normalized_number(stat.value), "base": stat.base.map(normalized_number)})
        }).collect();
        json!({"name": group.name, "stats": stats})
    }).collect();
    let projection = Value::Array(stat_groups);
    // 快照摘要独立于命令指纹。使用白名单投影的定序 JSON 数组，属性按端口规范排序。
    let Ok(canonical) = serde_json::to_vec(&(
        LISTING_EQUIPMENT_SOURCE,
        identity.world_id(),
        identity.external_equipment_id(),
        &label.name,
        &label.alt_name,
        &label.category,
        level_kind,
        level_number,
        label.tradeable,
        snapshot.owner_name(),
        &projection,
    )) else {
        return Err(WriteError::invariant("consignment_snapshot_digest"));
    };
    let digest = RequestHash::from_canonical_bytes(&canonical).as_bytes();
    sqlx::query(
        "INSERT INTO public.consignment_snapshots
            (id, listing_id, generation, adapter, observed_at, digest, world_id,
             external_equipment_id, name, category, alt_name, level_kind, level_number,
             tradeable, owner_name, stat_groups)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(snapshot_id)
    .bind(listing_id)
    .bind(generation.cast_signed())
    .bind(LISTING_EQUIPMENT_SOURCE)
    .bind(details.observed_at())
    .bind(digest.as_slice())
    .bind(identity.world_id())
    .bind(identity.external_equipment_id())
    .bind(&label.name)
    .bind(&label.category)
    .bind(&label.alt_name)
    .bind(level_kind)
    .bind(level_number)
    .bind(label.tradeable)
    .bind(snapshot.owner_name())
    .bind(projection)
    .execute(conn)
    .await?;
    Ok(())
}

fn normalized_number(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}
