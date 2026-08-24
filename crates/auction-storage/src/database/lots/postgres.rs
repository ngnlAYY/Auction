//! PostgreSQL Public lot 读取与安全投影。
//!
//! 该模块只拥有 PostgreSQL 查询和行到应用投影的适配；共享字段校验与装备映射统一由 mapping 模块维护。

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn list_postgres(
    store: &super::PostgresStore,
    auction_id: Uuid,
    request: PublicLotListRequest,
) -> Result<Option<PublicLotPage>, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启公开 lot 读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置公开 lot 读取隔离级别失败"))?;
    let page = list_postgres_transaction(&mut transaction, auction_id, request).await?;
    transaction.commit().await.map_err(|_| storage_error("提交公开 lot 读取事务失败"))?;
    Ok(page)
}

#[cfg(feature = "postgres")]
async fn list_postgres_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
    request: PublicLotListRequest,
) -> Result<Option<PublicLotPage>, ApplicationError> {
    let Some(projection) = postgres_list_projection(transaction, auction_id).await? else {
        return Ok(None);
    };
    let counts = postgres_category_counts(transaction, auction_id, &projection).await?;
    let total = request
        .category
        .as_deref()
        .map(|category| counts.get(category).copied().unwrap_or_default())
        .unwrap_or_else(|| counts.get("all").copied().unwrap_or_default());

    let mut page = PublicLotPage {
        projection: projection.clone(),
        preview: Vec::new(),
        live: Vec::new(),
        total,
        limit: request.limit,
        offset: request.offset,
        category_counts: counts,
    };
    if projection.projection_kind == "preview" {
        let rows = sqlx::query(
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
             ORDER BY l.lot_number, l.id
             LIMIT $3 OFFSET $4",
        )
        .bind(auction_id)
        .bind(request.category.as_deref())
        .bind(request.limit)
        .bind(request.offset)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| storage_error("读取 PostgreSQL 预览 lot 列表失败"))?;
        page.preview = rows
            .into_iter()
            .map(|row| map_postgres_preview(row, &projection.world_id))
            .collect::<Result<_, _>>()?;
    } else {
        let rows = sqlx::query(
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
                COALESCE((a.policy_snapshot ->> 'bid_increment_ppm')::bigint, 0) AS policy_bid_increment_ppm,
                COALESCE((a.policy_snapshot ->> 'minimum_increment')::bigint, 30000) AS policy_minimum_increment,
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
             ORDER BY l.lot_number, l.id
             LIMIT $3 OFFSET $4",
        )
        .bind(auction_id)
        .bind(request.category.as_deref())
        .bind(request.limit)
        .bind(request.offset)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| storage_error("读取 PostgreSQL 正式 lot 列表失败"))?;
        page.live = rows
            .into_iter()
            .map(|row| map_postgres_live(row, &projection.world_id))
            .collect::<Result<_, _>>()?;
    }
    Ok(Some(page))
}

#[cfg(feature = "postgres")]
pub(super) async fn detail_postgres(
    store: &super::PostgresStore,
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
                COALESCE((a.policy_snapshot ->> 'bid_increment_ppm')::bigint, 0) AS policy_bid_increment_ppm,
                COALESCE((a.policy_snapshot ->> 'minimum_increment')::bigint, 30000) AS policy_minimum_increment,
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
    transaction.commit().await.map_err(|_| storage_error("提交公开 lot 详情事务失败"))?;
    Ok(Some(detail))
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
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
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
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
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

#[cfg(feature = "postgres")]
fn map_postgres_projection(
    row: sqlx::postgres::PgRow,
) -> Result<PublicAuctionProjection, ApplicationError> {
    let status: String =
        row.try_get("status").map_err(|_| storage_error("读取公开 lot 拍卖状态失败"))?;
    map_projection(
        row.try_get("id").map_err(|_| storage_error("读取公开 lot 拍卖标识失败"))?,
        row.try_get("world_id").map_err(|_| storage_error("读取公开 lot 世界标识失败"))?,
        row.try_get("sequence").map_err(|_| storage_error("读取公开 lot 拍卖序号失败"))?,
        status,
        row.try_get("scheduled_open_at").map_err(|_| storage_error("读取公开 lot 开始时间失败"))?,
        row.try_get("scheduled_ends_at").map_err(|_| storage_error("读取公开 lot 结束时间失败"))?,
        row.try_get("preview_revision").map_err(|_| storage_error("读取公开 lot 预览版本失败"))?,
        row.try_get("presentation_mode").map_err(|_| storage_error("读取公开 lot 展示模式失败"))?,
        row.try_get("bidding_open").map_err(|_| storage_error("读取公开 lot 竞价状态失败"))?,
    )
}

#[cfg(feature = "postgres")]
fn map_postgres_preview(
    row: sqlx::postgres::PgRow,
    world_id: &str,
) -> Result<PublicPreviewLotSummary, ApplicationError> {
    let attributes: sqlx::types::Json<Value> = row
        .try_get("equipment_attributes")
        .map_err(|_| storage_error("读取 PostgreSQL 预览装备属性失败"))?;
    map_preview_fields(
        world_id,
        row.try_get("id").map_err(|_| storage_error("读取 PostgreSQL 预览 lot 标识失败"))?,
        row.try_get("auction_id").map_err(|_| storage_error("读取 PostgreSQL 预览拍卖标识失败"))?,
        i64::from(
            row.try_get::<i32, _>("lot_number")
                .map_err(|_| storage_error("读取 PostgreSQL 预览 lot 序号失败"))?,
        ),
        row.try_get("status").map_err(|_| storage_error("读取 PostgreSQL 预览 lot 状态失败"))?,
        row.try_get("start_price").map_err(|_| storage_error("读取 PostgreSQL 预览起始价失败"))?,
        row.try_get("seller_name").map_err(|_| storage_error("读取 PostgreSQL 预览卖家失败"))?,
        row.try_get("external_equipment_id")
            .map_err(|_| storage_error("读取 PostgreSQL 预览装备编号失败"))?,
        row.try_get("equipment_name")
            .map_err(|_| storage_error("读取 PostgreSQL 预览装备名称失败"))?,
        row.try_get("equipment_category")
            .map_err(|_| storage_error("读取 PostgreSQL 预览装备分类失败"))?,
        attributes.0,
    )
}

#[cfg(feature = "postgres")]
fn map_postgres_preview_detail(
    row: sqlx::postgres::PgRow,
    world_id: &str,
) -> Result<PublicPreviewLotDetail, ApplicationError> {
    let attributes: sqlx::types::Json<Value> = row
        .try_get("equipment_attributes")
        .map_err(|_| storage_error("读取 PostgreSQL 预览详情装备属性失败"))?;
    let id: Uuid =
        row.try_get("id").map_err(|_| storage_error("读取 PostgreSQL 预览详情 lot 标识失败"))?;
    let auction_id: Uuid = row
        .try_get("auction_id")
        .map_err(|_| storage_error("读取 PostgreSQL 预览详情拍卖标识失败"))?;
    let lot_number = i64::from(
        row.try_get::<i32, _>("lot_number")
            .map_err(|_| storage_error("读取 PostgreSQL 预览详情 lot 序号失败"))?,
    );
    let status: String = row
        .try_get("status")
        .map_err(|_| storage_error("读取 PostgreSQL 预览详情 lot 状态失败"))?;
    let start_price: i64 = row
        .try_get("start_price")
        .map_err(|_| storage_error("读取 PostgreSQL 预览详情起始价失败"))?;
    let seller: String = row
        .try_get("seller_name")
        .map_err(|_| storage_error("读取 PostgreSQL 预览详情卖家失败"))?;
    let external_id: String = row
        .try_get("external_equipment_id")
        .map_err(|_| storage_error("读取 PostgreSQL 预览详情装备编号失败"))?;
    let name: String = row
        .try_get("equipment_name")
        .map_err(|_| storage_error("读取 PostgreSQL 预览详情装备名称失败"))?;
    let category: String = row
        .try_get("equipment_category")
        .map_err(|_| storage_error("读取 PostgreSQL 预览详情装备分类失败"))?;
    let equipment = map_equipment(world_id, &external_id, &name, &category, attributes.0)?;
    let summary = PublicPreviewLotSummary {
        id,
        auction_id,
        code: public_forum_lot_code(&category, lot_number),
        status,
        seller_name: seller,
        start_price,
        equipment_name: name,
        equipment_category: category,
        equipment_level: equipment.level,
        avg_roll: equipment.avg_roll,
        game_equipment_url: equipment.game_equipment_url.clone(),
    };
    Ok(PublicPreviewLotDetail { summary, equipment })
}

#[cfg(feature = "postgres")]
fn map_postgres_live(
    row: sqlx::postgres::PgRow,
    world_id: &str,
) -> Result<PublicLiveLotSummary, ApplicationError> {
    let attributes: sqlx::types::Json<Value> = row
        .try_get("equipment_attributes")
        .map_err(|_| storage_error("读取 PostgreSQL 正式装备属性失败"))?;
    map_live_fields(
        world_id,
        row.try_get("id").map_err(|_| storage_error("读取 PostgreSQL 正式 lot 标识失败"))?,
        row.try_get("auction_id").map_err(|_| storage_error("读取 PostgreSQL 正式拍卖标识失败"))?,
        i64::from(
            row.try_get::<i32, _>("lot_number")
                .map_err(|_| storage_error("读取 PostgreSQL 正式 lot 序号失败"))?,
        ),
        row.try_get("status").map_err(|_| storage_error("读取 PostgreSQL 正式 lot 状态失败"))?,
        row.try_get("start_price").map_err(|_| storage_error("读取 PostgreSQL 正式起始价失败"))?,
        row.try_get("current_price")
            .map_err(|_| storage_error("读取 PostgreSQL 正式当前价失败"))?,
        row.try_get("has_leader").map_err(|_| storage_error("读取 PostgreSQL 正式领先状态失败"))?,
        row.try_get("leader_proxy_last")
            .map_err(|_| storage_error("读取 PostgreSQL 正式代理状态失败"))?,
        row.try_get("material")
            .map_err(|_| storage_error("读取 PostgreSQL 正式装备材质状态失败"))?,
        row.try_get("policy_bid_increment_ppm")
            .map_err(|_| storage_error("读取 PostgreSQL 正式竞价增幅失败"))?,
        row.try_get("policy_minimum_increment")
            .map_err(|_| storage_error("读取 PostgreSQL 正式最低增量失败"))?,
        row.try_get("leader_name").map_err(|_| storage_error("读取 PostgreSQL 正式领先者失败"))?,
        row.try_get("seller_name").map_err(|_| storage_error("读取 PostgreSQL 正式卖家失败"))?,
        row.try_get("effective_end_at")
            .map_err(|_| storage_error("读取 PostgreSQL 正式结束时间失败"))?,
        row.try_get("state_version")
            .map_err(|_| storage_error("读取 PostgreSQL 正式状态版本失败"))?,
        row.try_get("external_equipment_id")
            .map_err(|_| storage_error("读取 PostgreSQL 正式装备编号失败"))?,
        row.try_get("equipment_name")
            .map_err(|_| storage_error("读取 PostgreSQL 正式装备名称失败"))?,
        row.try_get("equipment_category")
            .map_err(|_| storage_error("读取 PostgreSQL 正式装备分类失败"))?,
        attributes.0,
    )
}

#[cfg(feature = "postgres")]
fn map_postgres_counts(
    row: sqlx::postgres::PgRow,
) -> Result<BTreeMap<String, i64>, ApplicationError> {
    map_counts([
        row.try_get("all_count").map_err(|_| storage_error("读取 PostgreSQL lot 总数失败"))?,
        row.try_get("one_count").map_err(|_| storage_error("读取 PostgreSQL one 分类数量失败"))?,
        row.try_get("two_count").map_err(|_| storage_error("读取 PostgreSQL two 分类数量失败"))?,
        row.try_get("sta_count").map_err(|_| storage_error("读取 PostgreSQL sta 分类数量失败"))?,
        row.try_get("shd_count").map_err(|_| storage_error("读取 PostgreSQL shd 分类数量失败"))?,
        row.try_get("clo_count").map_err(|_| storage_error("读取 PostgreSQL clo 分类数量失败"))?,
        row.try_get("lig_count").map_err(|_| storage_error("读取 PostgreSQL lig 分类数量失败"))?,
        row.try_get("hea_count").map_err(|_| storage_error("读取 PostgreSQL hea 分类数量失败"))?,
    ])
}
