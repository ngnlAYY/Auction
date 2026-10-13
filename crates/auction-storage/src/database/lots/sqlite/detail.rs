//! SQLite Lite Public lot 详情查询与当前拍卖投影解析。
//!
//! 拍卖截止投影按实际时刻选择；毫秒并列时仅规范化已知 UTC RFC3339／空格格式，
//! 补齐小数以保留亚毫秒次序，未知偏移量不按 UTC 字符串解释。

use super::super::*;
use super::mapping::{map_sqlite_live, map_sqlite_preview_detail, map_sqlite_projection};

#[cfg(feature = "sqlite")]
pub(in crate::database::lots) async fn detail_sqlite(
    store: &SqliteStore,
    lot_id: Uuid,
) -> Result<Option<PublicLotDetail>, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启 Lite lot 详情事务失败"))?;
    let Some(projection) = sqlite_detail_projection(&mut transaction, lot_id).await? else {
        transaction.commit().await.map_err(|_| storage_error("提交 Lite lot 详情事务失败"))?;
        return Ok(None);
    };
    let detail = if projection.projection_kind == "preview" {
        let row = sqlx::query(
            "SELECT
                l.id, l.auction_id, l.equipment_id, l.category_number AS lot_number, l.status, l.start_price,
                submission.seller AS seller_name,
                e.external_equipment_id, e.name AS equipment_name,
                e.category AS equipment_category, e.attributes AS equipment_attributes
             FROM lots AS l
             JOIN auctions AS a ON a.id = l.auction_id
             JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
             JOIN equipment AS e ON e.id = l.equipment_id
             JOIN auction_submissions AS submission ON submission.id = l.submission_id
             WHERE l.id = ? AND a.status = 'preview' AND l.status = 'pending'",
        )
        .bind(lot_id.as_bytes().to_vec())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 Lite 预览 lot 详情失败"))?;
        let Some(row) = row else {
            transaction.commit().await.map_err(|_| storage_error("提交 Lite lot 详情事务失败"))?;
            return Ok(None);
        };
        let equipment_id = EquipmentId::from_uuid(uuid_from_blob(
            row.try_get("equipment_id").map_err(|_| storage_error("读取 Lite 预览装备标识失败"))?,
            "Lite 预览装备标识",
        )?);
        let world_id = projection.world_id.clone();
        PublicLotDetail {
            equipment_id,
            projection,
            preview: Some(map_sqlite_preview_detail(row, &world_id)?),
            live: None,
        }
    } else {
        let row = sqlx::query(
            "SELECT
                l.id, l.auction_id, l.equipment_id, l.category_number AS lot_number, l.status, l.start_price, l.current_price,
                CASE WHEN l.leader_user_id IS NULL THEN 0 ELSE 1 END AS has_leader,
                l.material,
                CAST(json_extract(a.policy_snapshot, '$.bid_increment_ppm') AS INTEGER) AS policy_bid_increment_ppm,
                CAST(json_extract(a.policy_snapshot, '$.minimum_increment') AS INTEGER) AS policy_minimum_increment,
                leader.username AS leader_name,
                submission.seller AS seller_name, l.effective_end_at, l.state_version,
                e.external_equipment_id, e.name AS equipment_name,
                e.category AS equipment_category, e.attributes AS equipment_attributes
             FROM lots AS l
             JOIN auctions AS a ON a.id = l.auction_id
             JOIN worlds AS w
               ON w.id = a.world_id AND (w.enabled = 1 OR a.status = 'settled')
             JOIN equipment AS e ON e.id = l.equipment_id
             LEFT JOIN auction_submissions AS submission ON submission.id = l.submission_id
             LEFT JOIN users AS leader ON leader.id = l.leader_user_id
             WHERE l.id = ?
               AND ((a.status IN ('live', 'final', 'settling') AND l.status IN ('live', 'sold', 'unsold'))
                    OR (a.status = 'settled' AND l.status IN ('sold', 'unsold')))",
        )
        .bind(lot_id.as_bytes().to_vec())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 Lite 正式 lot 详情失败"))?;
        let Some(row) = row else {
            transaction.commit().await.map_err(|_| storage_error("提交 Lite lot 详情事务失败"))?;
            return Ok(None);
        };
        let equipment_id = EquipmentId::from_uuid(uuid_from_blob(
            row.try_get("equipment_id").map_err(|_| storage_error("读取 Lite 正式装备标识失败"))?,
            "Lite 正式装备标识",
        )?);
        let summary = map_sqlite_live(row, &projection.world_id)?;
        let attributes = summary.equipment.attributes.clone();
        PublicLotDetail {
            equipment_id,
            projection,
            preview: None,
            live: Some(PublicLiveLotDetail { summary, equipment_attributes: attributes }),
        }
    };
    detail.validate(lot_id).map_err(storage_error)?;
    transaction.commit().await.map_err(|_| storage_error("提交 Lite lot 详情事务失败"))?;
    Ok(Some(detail))
}

#[cfg(feature = "sqlite")]
async fn sqlite_detail_projection(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    lot_id: Uuid,
) -> Result<Option<PublicAuctionProjection>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            a.id, a.world_id, a.sequence, a.status, a.scheduled_open_at,
            (SELECT deadline.effective_end_at FROM lots AS deadline
             WHERE deadline.auction_id = a.id AND deadline.status IN ('live', 'sold', 'unsold')
               AND deadline.effective_end_at IS NOT NULL
             ORDER BY julianday(deadline.effective_end_at) DESC,
                 CASE WHEN substr(deadline.effective_end_at, -6) = '+00:00'
                           OR substr(deadline.effective_end_at, -1) = 'Z'
                           OR (substr(deadline.effective_end_at, 11, 1) = ' '
                               AND substr(deadline.effective_end_at, 20) NOT GLOB '*[+-]*')
                     THEN substr(replace(replace(replace(deadline.effective_end_at,
                         'T', ' '), '+00:00', ''), 'Z', '')
                         || CASE WHEN substr(deadline.effective_end_at, 20, 1) = '.'
                             THEN '000000000' ELSE '.000000000' END, 1, 29)
                 END DESC,
                 deadline.id DESC LIMIT 1) AS scheduled_ends_at,
            a.preview_revision,
            CASE WHEN a.status = 'preview' AND julianday('now') < julianday(a.scheduled_open_at) THEN 'preview'
                 WHEN a.status = 'preview' THEN 'opening' ELSE 'live' END AS presentation_mode,
            CASE WHEN a.status IN ('live', 'final') THEN 1 ELSE 0 END AS bidding_open
         FROM lots AS target
         JOIN auctions AS a ON a.id = target.auction_id
         JOIN worlds AS w
           ON w.id = a.world_id AND (w.enabled = 1 OR a.status = 'settled')
         WHERE target.id = ?
           AND ((a.status = 'preview' AND target.status = 'pending')
                OR (a.status IN ('live', 'final', 'settling') AND target.status IN ('live', 'sold', 'unsold'))
                OR (a.status = 'settled' AND target.status IN ('sold', 'unsold')))
           AND (a.status IN ('live', 'final', 'settling', 'settled') OR NOT EXISTS (
                SELECT 1 FROM auctions AS active
                WHERE active.world_id = a.world_id
                  AND active.status IN ('live', 'final', 'settling')
           ))",
    )
    .bind(lot_id.as_bytes().to_vec())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("解析 Lite lot projection 失败"))?;
    row.map(map_sqlite_projection).transpose()
}
