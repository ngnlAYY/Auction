//! PostgreSQL Public lot 详情查询与当前拍卖投影解析。

use super::super::*;
use super::mapping::{map_postgres_live, map_postgres_preview_detail, map_postgres_projection};

#[cfg(feature = "postgres")]
pub(in crate::database::lots) async fn detail_postgres(
    store: &PostgresStore,
    lot_id: Uuid,
    colors: &[EquipmentColorRule],
) -> Result<Option<PublicLotDetail>, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|_| storage_error("开启公开 lot 详情事务失败"))?;
    let Some(projection) = postgres_detail_projection(&mut transaction, lot_id).await? else {
        transaction.commit().await.map_err(|_| storage_error("提交公开 lot 详情事务失败"))?;
        return Ok(None);
    };
    let detail = if projection.projection_kind == "preview" {
        let row = sqlx::query(preview_lot_sql!("l.id = $1"))
            .bind(lot_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| storage_error("读取 PostgreSQL 预览 lot 详情失败"))?;
        let Some(row) = row else {
            transaction.commit().await.map_err(|_| storage_error("提交公开 lot 详情事务失败"))?;
            return Ok(None);
        };
        let equipment_id = EquipmentId::from_uuid(
            row.try_get("equipment_id")
                .map_err(|_| storage_error("读取 PostgreSQL 预览装备标识失败"))?,
        );
        let world_id = projection.world_id.clone();
        PublicLotDetail {
            equipment_id,
            projection,
            preview: Some(map_postgres_preview_detail(row, &world_id, colors)?),
            live: None,
        }
    } else {
        let row = sqlx::query(live_lot_sql!("l.id = $1"))
            .bind(lot_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| storage_error("读取 PostgreSQL 正式 lot 详情失败"))?;
        let Some(row) = row else {
            transaction.commit().await.map_err(|_| storage_error("提交公开 lot 详情事务失败"))?;
            return Ok(None);
        };
        let equipment_id = EquipmentId::from_uuid(
            row.try_get("equipment_id")
                .map_err(|_| storage_error("读取 PostgreSQL 正式装备标识失败"))?,
        );
        let summary = map_postgres_live(row, &projection.world_id, colors)?;
        PublicLotDetail {
            equipment_id,
            projection,
            preview: None,
            live: Some(PublicLiveLotDetail { summary }),
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
    let row = sqlx::query(concat!(
        "SELECT ",
        crate::database::auctions::public_auction_projection_sql!("current_lot"),
        "
         FROM public.lots AS target
         JOIN public.auctions AS a ON a.id = target.auction_id
         JOIN public.worlds AS w
           ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         LEFT JOIN public.lots AS current_lot ON current_lot.auction_id = a.id
         WHERE target.id = $1
           AND (
                (a.status = 'preview' AND target.status = 'pending')
                OR (a.status IN ('live', 'final', 'settling') AND target.status IN ('live', 'sold', 'unsold'))
                OR (a.status = 'settled' AND target.status IN ('sold', 'unsold'))
           )
           AND ",
        crate::database::auctions::preview_visibility_sql!(),
        "
         GROUP BY a.id"
    ))
    .bind(lot_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("解析 PostgreSQL lot projection 失败"))?;
    row.map(map_postgres_projection).transpose()
}
