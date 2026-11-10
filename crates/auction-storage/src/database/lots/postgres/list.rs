//! PostgreSQL Public lot 列表查询与分类统计。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use std::collections::BTreeMap;

use super::mapping::{
    map_postgres_counts, map_postgres_live, map_postgres_preview, map_postgres_projection,
};
use crate::database::DatabaseStore;
use crate::database::lots::storage_error;
use auction_application::auction::{PublicAuctionProjection, PublicLotListRequest, PublicLotPage};
use auction_application::core::ApplicationError;
use auction_application::forum::EquipmentColorRule;
use uuid::Uuid;

/// 列表页过滤：$2 分类代码、$5 预筛选 lot 集合，按全局 lot 序号稳定排序后以 $3/$4 分页。
macro_rules! lot_page_filter_sql {
    () => {
        "l.auction_id = $1
               AND ($2::text IS NULL OR e.category = CASE $2::text
                   WHEN 'one' THEN 'One-Handed'
                   WHEN 'two' THEN 'Two-Handed'
                   WHEN 'sta' THEN 'Staff'
                   WHEN 'shd' THEN 'Shield'
                   WHEN 'clo' THEN 'Cloth'
                   WHEN 'lig' THEN 'Light'
                   WHEN 'hea' THEN 'Heavy'
               END)
               AND ($5::uuid[] IS NULL OR l.id = ANY($5))
             ORDER BY l.lot_number, l.id
             LIMIT $3 OFFSET $4"
    };
}

#[cfg(feature = "postgres")]
pub(in crate::database::lots) async fn list_postgres(
    store: &DatabaseStore,
    auction_id: Uuid,
    request: PublicLotListRequest,
    colors: &[EquipmentColorRule],
) -> Result<Option<PublicLotPage>, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启公开 lot 读取事务失败"))?;
    let page = list_postgres_transaction(&mut transaction, auction_id, request, colors).await?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交公开 lot 读取事务失败"))?;
    Ok(page)
}

#[cfg(feature = "postgres")]
async fn list_postgres_transaction(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
    request: PublicLotListRequest,
    colors: &[EquipmentColorRule],
) -> Result<Option<PublicLotPage>, ApplicationError> {
    let Some(projection) = postgres_list_projection(transaction, auction_id).await? else {
        return Ok(None);
    };
    // 无分组或等级条件时保留原 SQL 快路；有条件才流式扫描，不在分页后丢行。
    let (counts, page_ids) = if request.equipment_group.is_some()
        || request.max_level.is_some()
        || !request.include_unleveled
    {
        let selection =
            super::selection::select_filtered_page(transaction, auction_id, &request, colors)
                .await?;
        (selection.category_counts, Some(selection.lot_ids))
    } else {
        (postgres_category_counts(transaction, auction_id, &projection).await?, None)
    };
    let query_offset = if page_ids.is_some() { 0 } else { request.offset };
    let total = request
        .category
        .as_deref()
        .map(|category| counts.get(category).copied().unwrap_or_default())
        .unwrap_or_else(|| counts.get("all").copied().unwrap_or_default());

    let mut page = PublicLotPage {
        projection,
        preview: Vec::new(),
        live: Vec::new(),
        total,
        limit: request.limit,
        offset: request.offset,
        category_counts: counts,
    };
    if page.projection.projection_kind == "preview" {
        let rows = sqlx::query(preview_lot_sql!(lot_page_filter_sql!()))
            .bind(auction_id)
            .bind(request.category.as_deref())
            .bind(request.limit)
            .bind(query_offset)
            .bind(page_ids.as_deref())
            .fetch_all(&mut **transaction)
            .await
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览 lot 列表失败"))?;
        page.preview = rows
            .into_iter()
            .map(|row| map_postgres_preview(row, &page.projection.world_id, colors))
            .collect::<Result<_, _>>()?;
    } else {
        let rows = sqlx::query(live_lot_sql!(lot_page_filter_sql!()))
            .bind(auction_id)
            .bind(request.category.as_deref())
            .bind(request.limit)
            .bind(query_offset)
            .bind(page_ids.as_deref())
            .fetch_all(&mut **transaction)
            .await
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式 lot 列表失败"))?;
        page.live = rows
            .into_iter()
            .map(|row| map_postgres_live(row, &page.projection.world_id, colors))
            .collect::<Result<_, _>>()?;
    }
    page.validate(auction_id, request.category.as_deref()).map_err(storage_error)?;
    page.validate_filters(&request).map_err(storage_error)?;
    Ok(Some(page))
}

#[cfg(feature = "postgres")]
async fn postgres_list_projection(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
) -> Result<Option<PublicAuctionProjection>, ApplicationError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        crate::database::auctions::public_auction_projection_sql!("l"),
        "
         FROM public.auctions AS a
         JOIN public.worlds AS w
           ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         LEFT JOIN public.lots AS l ON l.auction_id = a.id
         WHERE a.id = $1
           AND a.status IN ('preview', 'live', 'final', 'settling', 'settled')
           AND ",
        crate::database::auctions::preview_visibility_sql!(),
        "
         GROUP BY a.id"
    ))
    .bind(auction_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL lot projection 失败"))?;
    row.map(map_postgres_projection).transpose()
}

#[cfg(feature = "postgres")]
async fn postgres_category_counts(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
    projection: &PublicAuctionProjection,
) -> Result<BTreeMap<String, i64>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            COUNT(*)::bigint AS all_count,
            COUNT(*) FILTER (WHERE e.category = 'One-Handed')::bigint AS one_count,
            COUNT(*) FILTER (WHERE e.category = 'Two-Handed')::bigint AS two_count,
            COUNT(*) FILTER (WHERE e.category = 'Staff')::bigint AS sta_count,
            COUNT(*) FILTER (WHERE e.category = 'Shield')::bigint AS shd_count,
            COUNT(*) FILTER (WHERE e.category = 'Cloth')::bigint AS clo_count,
            COUNT(*) FILTER (WHERE e.category = 'Light')::bigint AS lig_count,
            COUNT(*) FILTER (WHERE e.category = 'Heavy')::bigint AS hea_count
         FROM public.lots AS l
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w
           ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         JOIN public.equipment AS e ON e.id = l.equipment_id
         WHERE l.auction_id = $1
           AND (
                ($2 = 'preview' AND a.status = 'preview' AND l.status = 'pending')
                OR ($2 <> 'preview' AND (
                    (a.status IN ('live', 'final', 'settling') AND l.status IN ('live', 'sold', 'unsold'))
                    OR (a.status = 'settled' AND l.status IN ('sold', 'unsold'))
                ))
           )",
    )
    .bind(auction_id)
    .bind(&projection.projection_kind)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "统计 PostgreSQL lot 分类失败"))?;
    map_postgres_counts(row)
}
