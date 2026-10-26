//! PostgreSQL Public lot 列表查询与分类统计。

use std::collections::BTreeMap;

use super::super::*;
use super::mapping::{
    map_postgres_counts, map_postgres_live, map_postgres_preview, map_postgres_projection,
};

#[cfg(feature = "postgres")]
pub(in crate::database::lots) async fn list_postgres(
    store: &PostgresStore,
    auction_id: Uuid,
    request: PublicLotListRequest,
    colors: &[EquipmentColorRule],
) -> Result<Option<PublicLotPage>, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启公开 lot 读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置公开 lot 读取隔离级别失败"))?;
    let page = list_postgres_transaction(&mut transaction, auction_id, request, colors).await?;
    transaction.commit().await.map_err(|_| storage_error("提交公开 lot 读取事务失败"))?;
    Ok(page)
}

#[cfg(feature = "postgres")]
async fn list_postgres_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
        let rows = sqlx::query(
            "SELECT
                l.id,
                l.equipment_id,
                l.auction_id,
                l.category_number AS lot_number,
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
             WHERE l.auction_id = $1
               AND a.status = 'preview'
               AND l.status = 'pending'
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
             LIMIT $3 OFFSET $4",
        )
        .bind(auction_id)
        .bind(request.category.as_deref())
        .bind(request.limit)
        .bind(query_offset)
        .bind(page_ids.as_deref())
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| storage_error("读取 PostgreSQL 预览 lot 列表失败"))?;
        page.preview = rows
            .into_iter()
            .map(|row| map_postgres_preview(row, &page.projection.world_id, colors))
            .collect::<Result<_, _>>()?;
    } else {
        let rows = sqlx::query(
            "SELECT
                l.id,
                l.equipment_id,
                l.auction_id,
                l.category_number AS lot_number,
                l.status,
                l.start_price,
                l.current_price,
                (l.leader_user_id IS NOT NULL)::boolean AS has_leader,
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
             JOIN public.worlds AS w
               ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
             JOIN public.equipment AS e ON e.id = l.equipment_id
             LEFT JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
             LEFT JOIN public.users AS leader ON leader.id = l.leader_user_id
             WHERE l.auction_id = $1
               AND ((a.status IN ('live', 'final', 'settling') AND l.status IN ('live', 'sold', 'unsold'))
                    OR (a.status = 'settled' AND l.status IN ('sold', 'unsold')))
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
             LIMIT $3 OFFSET $4",
        )
        .bind(auction_id)
        .bind(request.category.as_deref())
        .bind(request.limit)
        .bind(query_offset)
        .bind(page_ids.as_deref())
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| storage_error("读取 PostgreSQL 正式 lot 列表失败"))?;
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
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
) -> Result<Option<PublicAuctionProjection>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            a.id,
            a.world_id,
            a.sequence,
            a.status,
            a.scheduled_open_at,
            MAX(l.effective_end_at) FILTER (WHERE l.status IN ('live', 'sold', 'unsold')) AS scheduled_ends_at,
            a.preview_revision,
            CASE
                WHEN a.status = 'preview' AND CURRENT_TIMESTAMP < a.scheduled_open_at THEN 'preview'
                WHEN a.status = 'preview' THEN 'opening'
                ELSE 'live'
            END AS presentation_mode,
            (a.status IN ('live', 'final')) AS bidding_open
         FROM public.auctions AS a
         JOIN public.worlds AS w
           ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         LEFT JOIN public.lots AS l ON l.auction_id = a.id
         WHERE a.id = $1
           AND a.status IN ('preview', 'live', 'final', 'settling', 'settled')
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
    .bind(auction_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL lot projection 失败"))?;
    row.map(map_postgres_projection).transpose()
}

#[cfg(feature = "postgres")]
async fn postgres_category_counts(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .map_err(|_| storage_error("统计 PostgreSQL lot 分类失败"))?;
    map_postgres_counts(row)
}
