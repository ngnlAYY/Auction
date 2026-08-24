//! Public lot 只读查询适配器。
//!
//! lot 列表和详情必须与当前公开 projection 使用同一数据库快照，并且只返回当前公开
//! 状态允许展示的 preview/live 事实。数据库行、内部策略和装备原始字段在这里映射为
//! 应用层安全投影，HTTP 层不直接接触 SQL 类型。

use std::collections::BTreeMap;

use auction_application::{
    ApplicationError, ApplicationFuture, PublicAuctionProjection, PublicEquipment,
    PublicEquipmentStat, PublicFinalRestriction, PublicLiveLotDetail, PublicLiveLotSummary,
    PublicLotDetail, PublicLotListRequest, PublicLotPage, PublicLotReader, PublicPreviewLotDetail,
    PublicPreviewLotSummary, RepositoryError,
};
use auction_domain::{AuctionRules, BidAmount};
#[cfg(feature = "sqlite")]
use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;

const CATEGORY_KEYS: [&str; 8] = ["all", "one", "two", "sta", "shd", "clo", "lig", "hea"];

impl PublicLotReader for DatabaseStore {
    fn list_public_lots(
        &self,
        auction_id: Uuid,
        request: PublicLotListRequest,
    ) -> ApplicationFuture<'_, Option<PublicLotPage>> {
        Box::pin(async move {
            validate_lot_request(auction_id, &request)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, auction_id, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_sqlite(store, auction_id, request).await,
            }
        })
    }

    fn public_lot(&self, lot_id: Uuid) -> ApplicationFuture<'_, Option<PublicLotDetail>> {
        Box::pin(async move {
            if lot_id.is_nil() {
                return Err(storage_error("公开 lot 标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => detail_postgres(store, lot_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => detail_sqlite(store, lot_id).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
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
async fn detail_postgres(
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

#[cfg(feature = "sqlite")]
async fn list_sqlite(
    store: &super::SqliteStore,
    auction_id: Uuid,
    request: PublicLotListRequest,
) -> Result<Option<PublicLotPage>, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启 Lite lot 读取事务失败"))?;
    let page = list_sqlite_transaction(&mut transaction, auction_id, request).await?;
    transaction.commit().await.map_err(|_| storage_error("提交 Lite lot 读取事务失败"))?;
    Ok(page)
}

#[cfg(feature = "sqlite")]
async fn list_sqlite_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    auction_id: Uuid,
    request: PublicLotListRequest,
) -> Result<Option<PublicLotPage>, ApplicationError> {
    let Some(projection) = sqlite_list_projection(transaction, auction_id).await? else {
        return Ok(None);
    };
    let counts = sqlite_category_counts(transaction, auction_id, &projection).await?;
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
             FROM lots AS l
             JOIN auctions AS a ON a.id = l.auction_id
             JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
             JOIN equipment AS e ON e.id = l.equipment_id
             JOIN auction_submissions AS submission ON submission.id = l.submission_id
             WHERE l.auction_id = ? AND a.status = 'preview' AND l.status = 'pending'
               AND (? IS NULL OR e.category = CASE ?
                   WHEN 'one' THEN 'One-Handed' WHEN 'two' THEN 'Two-Handed'
                   WHEN 'sta' THEN 'Staff' WHEN 'shd' THEN 'Shield'
                   WHEN 'clo' THEN 'Cloth' WHEN 'lig' THEN 'Light' WHEN 'hea' THEN 'Heavy'
               END)
             ORDER BY l.lot_number, l.id LIMIT ? OFFSET ?",
        )
        .bind(auction_id.as_bytes().to_vec())
        .bind(request.category.as_deref())
        .bind(request.category.as_deref())
        .bind(request.limit)
        .bind(request.offset)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| storage_error("读取 Lite 预览 lot 列表失败"))?;
        page.preview = rows
            .into_iter()
            .map(|row| map_sqlite_preview(row, &projection.world_id))
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
                CASE WHEN l.leader_user_id IS NULL THEN 0 ELSE 1 END AS has_leader,
                l.leader_proxy_last,
                l.material,
                COALESCE(CAST(json_extract(a.policy_snapshot, '$.bid_increment_ppm') AS INTEGER), 0) AS policy_bid_increment_ppm,
                COALESCE(CAST(json_extract(a.policy_snapshot, '$.minimum_increment') AS INTEGER), 30000) AS policy_minimum_increment,
                leader.username AS leader_name,
                submission.seller AS seller_name,
                l.effective_end_at,
                l.state_version,
                e.external_equipment_id,
                e.name AS equipment_name,
                e.category AS equipment_category,
                e.attributes AS equipment_attributes
             FROM lots AS l
             JOIN auctions AS a ON a.id = l.auction_id
             JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
             JOIN equipment AS e ON e.id = l.equipment_id
             LEFT JOIN auction_submissions AS submission ON submission.id = l.submission_id
             LEFT JOIN users AS leader ON leader.id = l.leader_user_id
             WHERE l.auction_id = ?
               AND ((a.status IN ('live', 'final', 'settling') AND l.status IN ('live', 'sold', 'unsold'))
                    OR (a.status = 'settled' AND l.status IN ('sold', 'unsold')))
               AND (? IS NULL OR e.category = CASE ?
                   WHEN 'one' THEN 'One-Handed' WHEN 'two' THEN 'Two-Handed'
                   WHEN 'sta' THEN 'Staff' WHEN 'shd' THEN 'Shield'
                   WHEN 'clo' THEN 'Cloth' WHEN 'lig' THEN 'Light' WHEN 'hea' THEN 'Heavy'
               END)
             ORDER BY l.lot_number, l.id LIMIT ? OFFSET ?",
        )
        .bind(auction_id.as_bytes().to_vec())
        .bind(request.category.as_deref())
        .bind(request.category.as_deref())
        .bind(request.limit)
        .bind(request.offset)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| storage_error("读取 Lite 正式 lot 列表失败"))?;
        page.live = rows
            .into_iter()
            .map(|row| map_sqlite_live(row, &projection.world_id))
            .collect::<Result<_, _>>()?;
    }
    Ok(Some(page))
}

#[cfg(feature = "sqlite")]
async fn detail_sqlite(
    store: &super::SqliteStore,
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
                l.id, l.auction_id, l.lot_number, l.status, l.start_price,
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
        let world_id = projection.world_id.clone();
        PublicLotDetail {
            projection,
            preview: Some(map_sqlite_preview_detail(row, &world_id)?),
            live: None,
        }
    } else {
        let row = sqlx::query(
            "SELECT
                l.id, l.auction_id, l.lot_number, l.status, l.start_price, l.current_price,
                CASE WHEN l.leader_user_id IS NULL THEN 0 ELSE 1 END AS has_leader,
                l.leader_proxy_last, l.material,
                COALESCE(CAST(json_extract(a.policy_snapshot, '$.bid_increment_ppm') AS INTEGER), 0) AS policy_bid_increment_ppm,
                COALESCE(CAST(json_extract(a.policy_snapshot, '$.minimum_increment') AS INTEGER), 30000) AS policy_minimum_increment,
                leader.username AS leader_name,
                submission.seller AS seller_name, l.effective_end_at, l.state_version,
                e.external_equipment_id, e.name AS equipment_name,
                e.category AS equipment_category, e.attributes AS equipment_attributes
             FROM lots AS l
             JOIN auctions AS a ON a.id = l.auction_id
             JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
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
        let summary = map_sqlite_live(row, &projection.world_id)?;
        let attributes = summary.equipment.attributes.clone();
        PublicLotDetail {
            projection,
            preview: None,
            live: Some(PublicLiveLotDetail { summary, equipment_attributes: attributes }),
        }
    };
    transaction.commit().await.map_err(|_| storage_error("提交 Lite lot 详情事务失败"))?;
    Ok(Some(detail))
}

#[cfg(feature = "sqlite")]
async fn sqlite_list_projection(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    auction_id: Uuid,
) -> Result<Option<PublicAuctionProjection>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            a.id, a.world_id, a.sequence, a.status, a.scheduled_open_at,
            MAX(CASE WHEN l.status IN ('live', 'sold', 'unsold') THEN l.effective_end_at END) AS scheduled_ends_at,
            a.preview_revision,
            CASE WHEN a.status = 'preview' AND CURRENT_TIMESTAMP < a.scheduled_open_at THEN 'preview'
                 WHEN a.status = 'preview' THEN 'opening' ELSE 'live' END AS presentation_mode,
            CASE WHEN a.status IN ('live', 'final') THEN 1 ELSE 0 END AS bidding_open
         FROM auctions AS a
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         LEFT JOIN lots AS l ON l.auction_id = a.id
         WHERE a.id = ? AND a.status IN ('preview', 'live', 'final', 'settling', 'settled')
           AND (a.status IN ('live', 'final', 'settling', 'settled') OR NOT EXISTS (
                SELECT 1 FROM auctions AS active
                WHERE active.world_id = a.world_id
                  AND active.status IN ('live', 'final', 'settling')
           ))
         GROUP BY a.id",
    )
    .bind(auction_id.as_bytes().to_vec())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 Lite lot projection 失败"))?;
    row.map(map_sqlite_projection).transpose()
}

#[cfg(feature = "sqlite")]
async fn sqlite_detail_projection(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    lot_id: Uuid,
) -> Result<Option<PublicAuctionProjection>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            a.id, a.world_id, a.sequence, a.status, a.scheduled_open_at,
            MAX(CASE WHEN current_lot.status IN ('live', 'sold', 'unsold') THEN current_lot.effective_end_at END) AS scheduled_ends_at,
            a.preview_revision,
            CASE WHEN a.status = 'preview' AND CURRENT_TIMESTAMP < a.scheduled_open_at THEN 'preview'
                 WHEN a.status = 'preview' THEN 'opening' ELSE 'live' END AS presentation_mode,
            CASE WHEN a.status IN ('live', 'final') THEN 1 ELSE 0 END AS bidding_open
         FROM lots AS target
         JOIN auctions AS a ON a.id = target.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         LEFT JOIN lots AS current_lot ON current_lot.auction_id = a.id
         WHERE target.id = ?
           AND ((a.status = 'preview' AND target.status = 'pending')
                OR (a.status IN ('live', 'final', 'settling') AND target.status IN ('live', 'sold', 'unsold'))
                OR (a.status = 'settled' AND target.status IN ('sold', 'unsold')))
           AND (a.status IN ('live', 'final', 'settling', 'settled') OR NOT EXISTS (
                SELECT 1 FROM auctions AS active
                WHERE active.world_id = a.world_id
                  AND active.status IN ('live', 'final', 'settling')
           ))
         GROUP BY a.id",
    )
    .bind(lot_id.as_bytes().to_vec())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("解析 Lite lot projection 失败"))?;
    row.map(map_sqlite_projection).transpose()
}

#[cfg(feature = "sqlite")]
async fn sqlite_category_counts(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    auction_id: Uuid,
    projection: &PublicAuctionProjection,
) -> Result<BTreeMap<String, i64>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            COUNT(*) AS all_count,
            COUNT(CASE WHEN e.category = 'One-Handed' THEN 1 END) AS one_count,
            COUNT(CASE WHEN e.category = 'Two-Handed' THEN 1 END) AS two_count,
            COUNT(CASE WHEN e.category = 'Staff' THEN 1 END) AS sta_count,
            COUNT(CASE WHEN e.category = 'Shield' THEN 1 END) AS shd_count,
            COUNT(CASE WHEN e.category = 'Cloth' THEN 1 END) AS clo_count,
            COUNT(CASE WHEN e.category = 'Light' THEN 1 END) AS lig_count,
            COUNT(CASE WHEN e.category = 'Heavy' THEN 1 END) AS hea_count
         FROM lots AS l
         JOIN auctions AS a ON a.id = l.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         JOIN equipment AS e ON e.id = l.equipment_id
         WHERE l.auction_id = ?
           AND ((? = 'preview' AND a.status = 'preview' AND l.status = 'pending')
                OR (? <> 'preview' AND (
                    (a.status IN ('live', 'final', 'settling') AND l.status IN ('live', 'sold', 'unsold'))
                    OR (a.status = 'settled' AND l.status IN ('sold', 'unsold'))
                )))",
    )
    .bind(auction_id.as_bytes().to_vec())
    .bind(&projection.projection_kind)
    .bind(&projection.projection_kind)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|_| storage_error("统计 Lite lot 分类失败"))?;
    map_counts([
        row.try_get("all_count").map_err(|_| storage_error("读取 Lite lot 总数失败"))?,
        row.try_get("one_count").map_err(|_| storage_error("读取 Lite one 分类数量失败"))?,
        row.try_get("two_count").map_err(|_| storage_error("读取 Lite two 分类数量失败"))?,
        row.try_get("sta_count").map_err(|_| storage_error("读取 Lite sta 分类数量失败"))?,
        row.try_get("shd_count").map_err(|_| storage_error("读取 Lite shd 分类数量失败"))?,
        row.try_get("clo_count").map_err(|_| storage_error("读取 Lite clo 分类数量失败"))?,
        row.try_get("lig_count").map_err(|_| storage_error("读取 Lite lig 分类数量失败"))?,
        row.try_get("hea_count").map_err(|_| storage_error("读取 Lite hea 分类数量失败"))?,
    ])
}

#[cfg(feature = "sqlite")]
fn map_sqlite_projection(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PublicAuctionProjection, ApplicationError> {
    let status: String =
        row.try_get("status").map_err(|_| storage_error("读取 Lite lot 拍卖状态失败"))?;
    map_projection(
        uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite lot 拍卖标识失败"))?,
            "Lite lot 拍卖标识",
        )?,
        row.try_get("world_id").map_err(|_| storage_error("读取 Lite lot 世界标识失败"))?,
        row.try_get("sequence").map_err(|_| storage_error("读取 Lite lot 拍卖序号失败"))?,
        status,
        parse_timestamp(
            row.try_get("scheduled_open_at")
                .map_err(|_| storage_error("读取 Lite lot 开始时间失败"))?,
            "Lite lot 开始时间",
        )?,
        parse_optional_timestamp(
            row.try_get("scheduled_ends_at")
                .map_err(|_| storage_error("读取 Lite lot 结束时间失败"))?,
            "Lite lot 结束时间",
        )?,
        row.try_get("preview_revision").map_err(|_| storage_error("读取 Lite lot 预览版本失败"))?,
        row.try_get("presentation_mode")
            .map_err(|_| storage_error("读取 Lite lot 展示模式失败"))?,
        row.try_get::<i64, _>("bidding_open")
            .map_err(|_| storage_error("读取 Lite lot 竞价状态失败"))?
            != 0,
    )
}

#[cfg(feature = "sqlite")]
fn map_sqlite_preview(
    row: sqlx::sqlite::SqliteRow,
    world_id: &str,
) -> Result<PublicPreviewLotSummary, ApplicationError> {
    let attributes = parse_attributes(
        row.try_get("equipment_attributes")
            .map_err(|_| storage_error("读取 Lite 预览装备属性失败"))?,
    )?;
    map_preview_fields(
        world_id,
        uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite 预览 lot 标识失败"))?,
            "Lite 预览 lot 标识",
        )?,
        uuid_from_blob(
            row.try_get("auction_id").map_err(|_| storage_error("读取 Lite 预览拍卖标识失败"))?,
            "Lite 预览拍卖标识",
        )?,
        row.try_get("lot_number").map_err(|_| storage_error("读取 Lite 预览 lot 序号失败"))?,
        row.try_get("status").map_err(|_| storage_error("读取 Lite 预览 lot 状态失败"))?,
        row.try_get("start_price").map_err(|_| storage_error("读取 Lite 预览起始价失败"))?,
        row.try_get("seller_name").map_err(|_| storage_error("读取 Lite 预览卖家失败"))?,
        row.try_get("external_equipment_id")
            .map_err(|_| storage_error("读取 Lite 预览装备编号失败"))?,
        row.try_get("equipment_name").map_err(|_| storage_error("读取 Lite 预览装备名称失败"))?,
        row.try_get("equipment_category")
            .map_err(|_| storage_error("读取 Lite 预览装备分类失败"))?,
        attributes,
    )
}

#[cfg(feature = "sqlite")]
fn map_sqlite_preview_detail(
    row: sqlx::sqlite::SqliteRow,
    world_id: &str,
) -> Result<PublicPreviewLotDetail, ApplicationError> {
    let attributes = parse_attributes(
        row.try_get("equipment_attributes")
            .map_err(|_| storage_error("读取 Lite 预览详情装备属性失败"))?,
    )?;
    let id = uuid_from_blob(
        row.try_get("id").map_err(|_| storage_error("读取 Lite 预览详情 lot 标识失败"))?,
        "Lite 预览详情 lot 标识",
    )?;
    let auction_id = uuid_from_blob(
        row.try_get("auction_id").map_err(|_| storage_error("读取 Lite 预览详情拍卖标识失败"))?,
        "Lite 预览详情拍卖标识",
    )?;
    let lot_number: i64 =
        row.try_get("lot_number").map_err(|_| storage_error("读取 Lite 预览详情 lot 序号失败"))?;
    let status: String =
        row.try_get("status").map_err(|_| storage_error("读取 Lite 预览详情 lot 状态失败"))?;
    let start_price: i64 =
        row.try_get("start_price").map_err(|_| storage_error("读取 Lite 预览详情起始价失败"))?;
    let seller: String =
        row.try_get("seller_name").map_err(|_| storage_error("读取 Lite 预览详情卖家失败"))?;
    let external_id: String = row
        .try_get("external_equipment_id")
        .map_err(|_| storage_error("读取 Lite 预览详情装备编号失败"))?;
    let name: String = row
        .try_get("equipment_name")
        .map_err(|_| storage_error("读取 Lite 预览详情装备名称失败"))?;
    let category: String = row
        .try_get("equipment_category")
        .map_err(|_| storage_error("读取 Lite 预览详情装备分类失败"))?;
    let equipment = map_equipment(world_id, &external_id, &name, &category, attributes)?;
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

#[cfg(feature = "sqlite")]
fn map_sqlite_live(
    row: sqlx::sqlite::SqliteRow,
    world_id: &str,
) -> Result<PublicLiveLotSummary, ApplicationError> {
    let attributes = parse_attributes(
        row.try_get("equipment_attributes")
            .map_err(|_| storage_error("读取 Lite 正式装备属性失败"))?,
    )?;
    map_live_fields(
        world_id,
        uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite 正式 lot 标识失败"))?,
            "Lite 正式 lot 标识",
        )?,
        uuid_from_blob(
            row.try_get("auction_id").map_err(|_| storage_error("读取 Lite 正式拍卖标识失败"))?,
            "Lite 正式拍卖标识",
        )?,
        row.try_get("lot_number").map_err(|_| storage_error("读取 Lite 正式 lot 序号失败"))?,
        row.try_get("status").map_err(|_| storage_error("读取 Lite 正式 lot 状态失败"))?,
        row.try_get("start_price").map_err(|_| storage_error("读取 Lite 正式起始价失败"))?,
        row.try_get("current_price").map_err(|_| storage_error("读取 Lite 正式当前价失败"))?,
        row.try_get::<i64, _>("has_leader")
            .map_err(|_| storage_error("读取 Lite 正式领先状态失败"))?
            != 0,
        row.try_get::<i64, _>("leader_proxy_last")
            .map_err(|_| storage_error("读取 Lite 正式代理状态失败"))?
            != 0,
        row.try_get::<i64, _>("material")
            .map_err(|_| storage_error("读取 Lite 正式材质状态失败"))?
            != 0,
        row.try_get("policy_bid_increment_ppm")
            .map_err(|_| storage_error("读取 Lite 正式竞价增幅失败"))?,
        row.try_get("policy_minimum_increment")
            .map_err(|_| storage_error("读取 Lite 正式最低增量失败"))?,
        row.try_get("leader_name").map_err(|_| storage_error("读取 Lite 正式领先者失败"))?,
        row.try_get("seller_name").map_err(|_| storage_error("读取 Lite 正式卖家失败"))?,
        parse_optional_timestamp(
            row.try_get("effective_end_at")
                .map_err(|_| storage_error("读取 Lite 正式结束时间失败"))?,
            "Lite 正式结束时间",
        )?,
        row.try_get("state_version").map_err(|_| storage_error("读取 Lite 正式状态版本失败"))?,
        row.try_get("external_equipment_id")
            .map_err(|_| storage_error("读取 Lite 正式装备编号失败"))?,
        row.try_get("equipment_name").map_err(|_| storage_error("读取 Lite 正式装备名称失败"))?,
        row.try_get("equipment_category")
            .map_err(|_| storage_error("读取 Lite 正式装备分类失败"))?,
        attributes,
    )
}

// 这里把两个后端的列名映射集中收口；参数分别对应公开契约字段，拆成多个临时结构反而会
// 让后端行映射跨越适配器边界。
#[allow(clippy::too_many_arguments)]
fn map_projection(
    auction_id: Uuid,
    world_id: String,
    sequence: i64,
    auction_status: String,
    opens_at: DateTime<Utc>,
    scheduled_ends_at: Option<DateTime<Utc>>,
    preview_revision: i64,
    presentation_mode: String,
    bidding_open: bool,
) -> Result<PublicAuctionProjection, ApplicationError> {
    if !matches!(auction_status.as_str(), "preview" | "live" | "final" | "settling" | "settled") {
        return Err(storage_error("公开 lot 拍卖状态无效"));
    }
    Ok(PublicAuctionProjection {
        auction_id,
        world_id,
        sequence,
        projection_kind: if auction_status == "preview" { "preview" } else { "live" }.to_owned(),
        final_restriction: (auction_status == "final").then_some(PublicFinalRestriction {
            new_slow_proxy_allowed: false,
            existing_slow_proxy_converted: true,
        }),
        auction_status,
        presentation_mode,
        bidding_open,
        opens_at,
        scheduled_ends_at,
        preview_revision,
    })
}

#[allow(clippy::too_many_arguments)]
fn map_preview_fields(
    world_id: &str,
    id: Uuid,
    auction_id: Uuid,
    lot_number: i64,
    status: String,
    start_price: i64,
    seller_name: Option<String>,
    external_id: String,
    equipment_name: String,
    equipment_category: String,
    attributes: Value,
) -> Result<PublicPreviewLotSummary, ApplicationError> {
    let equipment =
        map_equipment(world_id, &external_id, &equipment_name, &equipment_category, attributes)?;
    Ok(PublicPreviewLotSummary {
        id,
        auction_id,
        code: public_forum_lot_code(&equipment_category, lot_number),
        status,
        seller_name: seller_name.unwrap_or_else(|| "—".to_owned()),
        start_price,
        equipment_name,
        equipment_category,
        equipment_level: equipment.level,
        avg_roll: equipment.avg_roll,
        game_equipment_url: equipment.game_equipment_url,
    })
}

#[allow(clippy::too_many_arguments)]
fn map_live_fields(
    world_id: &str,
    id: Uuid,
    auction_id: Uuid,
    lot_number: i64,
    status: String,
    start_price: i64,
    current_price: Option<i64>,
    has_leader: bool,
    proxy_last: bool,
    material: bool,
    policy_bid_increment_ppm: i64,
    policy_minimum_increment: i64,
    leader_name: Option<String>,
    seller_name: Option<String>,
    effective_end_at: Option<DateTime<Utc>>,
    state_version: i64,
    external_id: String,
    equipment_name: String,
    equipment_category: String,
    attributes: Value,
) -> Result<PublicLiveLotSummary, ApplicationError> {
    let start =
        BidAmount::new(start_price).map_err(|_| storage_error("公开 lot 起始价不满足金额网格"))?;
    let current_value = current_price.unwrap_or(start_price);
    let current = BidAmount::new(current_value)
        .map_err(|_| storage_error("公开 lot 当前价不满足金额网格"))?;
    if current < start {
        return Err(storage_error("公开 lot 当前价低于起始价"));
    }
    let rules = AuctionRules {
        bid_increment_ppm: policy_bid_increment_ppm,
        minimum_increment: policy_minimum_increment,
        ..AuctionRules::default()
    };
    rules.validate().map_err(|_| storage_error("公开 lot 拍卖策略无效"))?;
    let next_bid = rules
        .next_minimum_bid(Some(start), has_leader.then_some(current), material, proxy_last)
        .value();
    let equipment =
        map_equipment(world_id, &external_id, &equipment_name, &equipment_category, attributes)?;
    Ok(PublicLiveLotSummary {
        id,
        auction_id,
        lot_number,
        status,
        seller_name: seller_name.unwrap_or_else(|| "—".to_owned()),
        start_price,
        current_price: current.value(),
        next_bid,
        has_leader,
        leader_name,
        effective_end_at,
        state_version,
        equipment,
    })
}

fn map_counts(values: [i64; 8]) -> Result<BTreeMap<String, i64>, ApplicationError> {
    if values.iter().any(|value| *value < 0) {
        return Err(storage_error("公开 lot 分类统计为负数"));
    }
    Ok(CATEGORY_KEYS.into_iter().zip(values).map(|(key, value)| (key.to_owned(), value)).collect())
}

fn map_equipment(
    world_id: &str,
    external_id: &str,
    name: &str,
    category: &str,
    attributes: Value,
) -> Result<PublicEquipment, ApplicationError> {
    if !matches!(world_id, "main" | "isekai")
        || external_id.is_empty()
        || external_id.len() > 32
        || !external_id.bytes().all(|byte| byte.is_ascii_digit())
        || !public_text(name, 512)
        || !public_text(category, 128)
        || !attributes.is_object()
    {
        return Err(storage_error("公开装备字段无效"));
    }
    let stored: StoredEquipmentAttributes = serde_json::from_value(attributes.clone())
        .map_err(|_| storage_error("公开装备属性无效"))?;
    let info_summary =
        stored.type_name.filter(|value| !value.is_empty()).unwrap_or_else(|| category.to_owned());
    let info_summary =
        stored.level.map_or(info_summary.clone(), |level| format!("{info_summary} Lv{level}"));
    if !public_text(&info_summary, 512) || stored.level.is_some_and(|level| level < 0) {
        return Err(storage_error("公开装备摘要无效"));
    }
    let quality = stored.quality.as_deref().and_then(quality_range);
    let mut stats = BTreeMap::new();
    let mut rolls = Vec::new();
    for (stat_name, stat) in stored.stats {
        if !public_text(&stat_name, 128)
            || !public_text(&stat.value, 256)
            || stat.base.is_some_and(|base| !base.is_finite())
        {
            return Err(storage_error("公开装备 stat 无效"));
        }
        let roll =
            quality.and_then(|range| stat.base.map(|base| public_equipment_roll(base, range)));
        if let Some(roll) = roll {
            rolls.push(roll);
        }
        stats.insert(stat_name, PublicEquipmentStat { value: stat.value, base: stat.base, roll });
    }
    let avg_roll = (!rolls.is_empty()).then(|| {
        let total: f64 = rolls.iter().sum();
        (total / rolls.len() as f64 * 10.0).round() / 10.0
    });
    let path = if world_id == "isekai" {
        format!("/isekai/equip/{external_id}")
    } else {
        format!("/equip/{external_id}")
    };
    Ok(PublicEquipment {
        name: name.to_owned(),
        category: category.to_owned(),
        level: stored.level,
        info_summary,
        stats,
        avg_roll,
        game_equipment_url: format!("https://hentaiverse.org{path}"),
        attributes,
    })
}

fn public_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn public_equipment_roll(base: f64, range: EquipmentQualityRange) -> f64 {
    ((base - range.minimum) / (range.maximum - range.minimum) * 100.0 * 10.0).round() / 10.0
}

#[derive(Debug, Clone, Copy)]
struct EquipmentQualityRange {
    minimum: f64,
    maximum: f64,
}

fn quality_range(value: &str) -> Option<EquipmentQualityRange> {
    let name = value.split_once(' ').map_or(value, |(name, _)| name);
    match name {
        "Peerless" | "Legendary" => Some(EquipmentQualityRange { minimum: 170.0, maximum: 200.0 }),
        "Magnificent" => Some(EquipmentQualityRange { minimum: 150.0, maximum: 180.0 }),
        "Exquisite" => Some(EquipmentQualityRange { minimum: 120.0, maximum: 160.0 }),
        "Superior" => Some(EquipmentQualityRange { minimum: 90.0, maximum: 130.0 }),
        "Average" => Some(EquipmentQualityRange { minimum: 60.0, maximum: 100.0 }),
        "Fair" => Some(EquipmentQualityRange { minimum: 30.0, maximum: 70.0 }),
        "Crude" => Some(EquipmentQualityRange { minimum: 0.0, maximum: 40.0 }),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct StoredEquipmentAttributes {
    #[serde(rename = "type")]
    type_name: Option<String>,
    quality: Option<String>,
    level: Option<i64>,
    #[serde(default)]
    stats: BTreeMap<String, StoredEquipmentStat>,
}

#[derive(Debug, Deserialize)]
struct StoredEquipmentStat {
    value: String,
    base: Option<f64>,
}

fn public_forum_lot_code(category: &str, number: i64) -> String {
    let prefix = match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => "Lot",
    };
    format!("{prefix}{number:02}")
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn parse_attributes(value: String) -> Result<Value, ApplicationError> {
    serde_json::from_str(&value).map_err(|_| storage_error("Lite 公开装备属性不是有效 JSON"))
}

#[cfg(feature = "sqlite")]
fn parse_optional_timestamp(
    value: Option<String>,
    label: &str,
) -> Result<Option<DateTime<Utc>>, ApplicationError> {
    value.map(|value| parse_timestamp(value, label)).transpose()
}

#[cfg(feature = "sqlite")]
fn parse_timestamp(value: String, label: &str) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error(label))
}

fn validate_lot_request(
    auction_id: Uuid,
    request: &PublicLotListRequest,
) -> Result<(), ApplicationError> {
    if auction_id.is_nil() {
        return Err(storage_error("公开 lot 拍卖标识无效"));
    }
    request.validate().map_err(storage_error)
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
