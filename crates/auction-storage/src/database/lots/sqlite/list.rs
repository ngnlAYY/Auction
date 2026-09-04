//! SQLite Lite Public lot 列表查询与分类统计。

use std::collections::BTreeMap;

use super::super::*;
use super::mapping::{map_sqlite_live, map_sqlite_preview, map_sqlite_projection};

#[cfg(feature = "sqlite")]
pub(in crate::database::lots) async fn list_sqlite(
    store: &SqliteStore,
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
                l.equipment_id,
                l.auction_id,
                l.category_number AS lot_number,
                l.status,
                l.start_price,
                l.current_price,
                CASE WHEN l.leader_user_id IS NULL THEN 0 ELSE 1 END AS has_leader,
                l.material,
                CAST(json_extract(a.policy_snapshot, '$.bid_increment_ppm') AS INTEGER) AS policy_bid_increment_ppm,
                CAST(json_extract(a.policy_snapshot, '$.minimum_increment') AS INTEGER) AS policy_minimum_increment,
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
             JOIN worlds AS w
               ON w.id = a.world_id AND (w.enabled = 1 OR a.status = 'settled')
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
    page.validate(auction_id, request.category.as_deref()).map_err(storage_error)?;
    Ok(Some(page))
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
         JOIN worlds AS w
           ON w.id = a.world_id AND (w.enabled = 1 OR a.status = 'settled')
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
         JOIN worlds AS w
           ON w.id = a.world_id AND (w.enabled = 1 OR a.status = 'settled')
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
