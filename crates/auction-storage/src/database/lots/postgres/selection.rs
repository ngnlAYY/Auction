//! 装备分组与等级的分页前筛选：只流式扫描身份字段，规则仍由应用层统一判断。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use futures_util::TryStreamExt;

use crate::database::lots::{CATEGORY_KEYS, map_counts, matches_equipment_filters, storage_error};
use auction_application::{ApplicationError, EquipmentColorRule, PublicLotListRequest};
use serde_json::Value;
use sqlx::Row;
use std::collections::BTreeMap;
use uuid::Uuid;

pub(super) struct FilteredPageSelection {
    pub category_counts: BTreeMap<String, i64>,
    pub lot_ids: Vec<Uuid>,
}

/// 统计选定分组及等级范围内的各类型，只保留当前页 UUID，不缓存全场装备。
/// 与完整页查询共用同一只读快照，筛选始终先于计数和 offset/limit。
pub(super) async fn select_filtered_page(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
    request: &PublicLotListRequest,
    colors: &[EquipmentColorRule],
) -> Result<FilteredPageSelection, ApplicationError> {
    let mut rows = sqlx::query(
        "SELECT l.id, e.name, e.category, e.attributes - 'stats' AS identity_attributes
         FROM public.lots AS l
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         JOIN public.equipment AS e ON e.id = l.equipment_id
         WHERE l.auction_id = $1
           AND ((a.status = 'preview' AND l.status = 'pending')
             OR (a.status IN ('live', 'final', 'settling') AND l.status IN ('live', 'sold', 'unsold'))
             OR (a.status = 'settled' AND l.status IN ('sold', 'unsold')))
         ORDER BY l.lot_number, l.id",
    )
    .bind(auction_id)
    .fetch(&mut **transaction);
    let mut counts = [0_i64; 8];
    let mut matched_count = 0_i64;
    let mut lot_ids = Vec::new();
    while let Some(row) = rows
        .try_next()
        .await
        .map_err(|error| storage_error_from(error, "读取公开装备分组候选失败"))?
    {
        let name: &str = row
            .try_get("name")
            .map_err(|error| storage_error_from(error, "读取分组装备名称失败"))?;
        let category: &str = row
            .try_get("category")
            .map_err(|error| storage_error_from(error, "读取分组装备类型失败"))?;
        let attributes: sqlx::types::Json<Value> = row
            .try_get("identity_attributes")
            .map_err(|error| storage_error_from(error, "读取分组装备身份失败"))?;
        if !matches_equipment_filters(name, category, attributes.0, colors, request)? {
            continue;
        }
        counts[0] = increment_count(counts[0])?;
        let category_index = match category {
            "One-Handed" => Some(1),
            "Two-Handed" => Some(2),
            "Staff" => Some(3),
            "Shield" => Some(4),
            "Cloth" => Some(5),
            "Light" => Some(6),
            "Heavy" => Some(7),
            _ => None,
        };
        if let Some(index) = category_index {
            counts[index] = increment_count(counts[index])?;
        }
        if request.category.as_deref().is_some_and(|requested| {
            category_index.is_none_or(|index| CATEGORY_KEYS[index] != requested)
        }) {
            continue;
        }
        if matched_count >= request.offset && (lot_ids.len() as u64) < request.limit as u64 {
            lot_ids.push(
                row.try_get("id")
                    .map_err(|error| storage_error_from(error, "读取分组 lot 标识失败"))?,
            );
        }
        matched_count = increment_count(matched_count)?;
    }
    Ok(FilteredPageSelection { category_counts: map_counts(counts)?, lot_ids })
}

fn increment_count(count: i64) -> Result<i64, ApplicationError> {
    count.checked_add(1).ok_or_else(|| storage_error("公开 lot 分组数量溢出"))
}
