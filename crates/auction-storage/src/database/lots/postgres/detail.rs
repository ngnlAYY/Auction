//! PostgreSQL Public lot 详情查询与当前拍卖投影解析。

use super::super::*;
use super::mapping::{map_postgres_live, map_postgres_preview_detail, map_postgres_projection};

#[cfg(feature = "postgres")]
pub(in crate::database::lots) async fn detail_postgres(
    store: &PostgresStore,
    lot_id: Uuid,
) -> Result<Option<PublicLotDetail>, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启公开 lot 详情事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置公开 lot 详情隔离级别失败"))?;
    let Some(projection) = postgres_detail_projection(&mut transaction, lot_id).await? else {
        transaction.commit().await.map_err(|_| storage_error("提交公开 lot 详情事务失败"))?;
        return Ok(None);
    };
    let detail = if projection.projection_kind == "preview" {
        let row = sqlx::query(
            "SELECT
                l.id,
                l.auction_id,
                l.lot_number,
                l.status,
                l.start_price,
                submission.seller AS seller_name,
                e.external_equipment_id,
                e.name AS equipment_name,
                e.category AS equipment_category,
                e.attributes AS equipment_attributes
             FROM public.lots AS l
             JOIN public.auctions AS a ON a.id = l.auction_id
             JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
             JOIN public.equipment AS e ON e.id = l.equipment_id
             JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
             WHERE l.id = $1 AND a.status = 'preview' AND l.status = 'pending'",
        )
        .bind(lot_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 PostgreSQL 预览 lot 详情失败"))?;
        let Some(row) = row else {
            transaction.commit().await.map_err(|_| storage_error("提交公开 lot 详情事务失败"))?;
            return Ok(None);
        };
        let world_id = projection.world_id.clone();
        PublicLotDetail {
            projection,
            preview: Some(map_postgres_preview_detail(row, &world_id)?),
            live: None,
        }
    } else {
        let row = sqlx::query(
            "SELECT
                l.id,
                l.auction_id,
                l.lot_number,
                l.status,
                l.start_price,
                l.current_price,
                (l.leader_user_id IS NOT NULL)::boolean AS has_leader,
                l.leader_proxy_last,
                l.material,
                (a.policy_snapshot ->> 'bid_increment_ppm')::bigint AS policy_bid_increment_ppm,
                (a.policy_snapshot ->> 'minimum_increment')::bigint AS policy_minimum_increment,
                leader.username AS leader_name,
                submission.seller AS seller_name,
                l.effective_end_at,
                l.state_version,
                e.external_equipment_id,
                e.name AS equipment_name,
                e.category AS equipment_category,
                e.attributes AS equipment_attributes
             FROM public.lots AS l
             JOIN public.auctions AS a ON a.id = l.auction_id
             JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
             JOIN public.equipment AS e ON e.id = l.equipment_id
             LEFT JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
             LEFT JOIN public.users AS leader ON leader.id = l.leader_user_id
             WHERE l.id = $1
               AND ((a.status IN ('live', 'final', 'settling') AND l.status IN ('live', 'sold', 'unsold'))
                    OR (a.status = 'settled' AND l.status IN ('sold', 'unsold')))",
        )
        .bind(lot_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 PostgreSQL 正式 lot 详情失败"))?;
        let Some(row) = row else {
            transaction.commit().await.map_err(|_| storage_error("提交公开 lot 详情事务失败"))?;
            return Ok(None);
        };
        let summary = map_postgres_live(row, &projection.world_id)?;
        let attributes = summary.equipment.attributes.clone();
        PublicLotDetail {
            projection,
            preview: None,
            live: Some(PublicLiveLotDetail { summary, equipment_attributes: attributes }),
        }
    };
    detail.validate(lot_id).map_err(storage_error)?;
    transaction.commit().await.map_err(|_| storage_error("提交公开 lot 详情事务失败"))?;
    Ok(Some(detail))
}

#[cfg(feature = "postgres")]
async fn postgres_detail_projection(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    lot_id: Uuid,
) -> Result<Option<PublicAuctionProjection>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            a.id,
            a.world_id,
            a.sequence,
            a.status,
            a.scheduled_open_at,
            MAX(current_lot.effective_end_at) FILTER (WHERE current_lot.status IN ('live', 'sold', 'unsold')) AS scheduled_ends_at,
            a.preview_revision,
            CASE
                WHEN a.status = 'preview' AND CURRENT_TIMESTAMP < a.scheduled_open_at THEN 'preview'
                WHEN a.status = 'preview' THEN 'opening'
                ELSE 'live'
            END AS presentation_mode,
            (a.status IN ('live', 'final')) AS bidding_open
         FROM public.lots AS target
         JOIN public.auctions AS a ON a.id = target.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
         LEFT JOIN public.lots AS current_lot ON current_lot.auction_id = a.id
         WHERE target.id = $1
           AND (
                (a.status = 'preview' AND target.status = 'pending')
                OR (a.status IN ('live', 'final', 'settling') AND target.status IN ('live', 'sold', 'unsold'))
                OR (a.status = 'settled' AND target.status IN ('sold', 'unsold'))
           )
           AND (
                a.status IN ('live', 'final', 'settling', 'settled')
                OR NOT EXISTS (
                    SELECT 1 FROM public.auctions AS active
                    WHERE active.world_id = a.world_id
                      AND active.status IN ('live', 'final', 'settling')
                )
           )
         GROUP BY a.id",
    )
    .bind(lot_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("解析 PostgreSQL lot projection 失败"))?;
    row.map(map_postgres_projection).transpose()
}
