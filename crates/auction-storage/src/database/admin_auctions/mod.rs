//! Admin 拍卖列表与生命周期只读投影；查询和行映射由 PostgreSQL owner 负责。

#[cfg(feature = "postgres")]
use auction_application::AdminAuctionSummary;
use auction_application::{
    AdminAuctionListRequest, AdminAuctionPage, AdminAuctionReader, ApplicationError,
    ApplicationFuture,
};

use super::DatabaseStore;
use crate::database::storage_error;

impl AdminAuctionReader for DatabaseStore {
    fn list_admin_auctions(
        &self,
        request: AdminAuctionListRequest,
    ) -> ApplicationFuture<'_, AdminAuctionPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(storage_error("Admin 拍卖列表参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, &request).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
    request: &AdminAuctionListRequest,
) -> Result<AdminAuctionPage, ApplicationError> {
    use sqlx::Row;

    let mut transaction = store
        .pool()
        .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
        .await
        .map_err(|_| storage_error("开启 Admin 拍卖列表读取事务失败"))?;

    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.auctions AS a
         WHERE ($1::text IS NULL OR a.world_id = $1)
           AND ($2::text IS NULL OR a.status = $2)",
    )
    .bind(request.world_id.as_deref())
    .bind(request.status.as_deref())
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Admin 拍卖列表总数失败"))?;

    // 计数保持完整筛选集；lot 汇总只处理当前页，避免历史场次越多每页扫描越重。
    let rows = sqlx::query(
        "WITH page_auctions AS MATERIALIZED (
            SELECT a.id
            FROM public.auctions AS a
            WHERE ($1::text IS NULL OR a.world_id = $1)
              AND ($2::text IS NULL OR a.status = $2)
            ORDER BY a.created_at DESC, a.id DESC
            LIMIT $3 OFFSET $4
         )
         SELECT
            a.id, a.world_id, a.sequence, a.status, a.preview_scheduled_at,
            a.scheduled_open_at, a.preview_opened_at, a.preview_manual_open,
            a.preview_revision, a.opened_at, a.final_phase_at, a.settled_at,
            a.cancelled_at, a.cancelled_reason, a.cancelled_by, a.state_version,
            a.scheduled_end_at, a.effective_end_at, a.created_at, a.updated_at,
            COALESCE(
                MAX(l.effective_end_at) FILTER (
                    WHERE l.status IN ('live', 'sold', 'unsold')
                ),
                (a.policy_snapshot ->> 'scheduled_end_at')::timestamptz
            ) AS scheduled_ends_at,
            COUNT(l.id)::bigint AS lot_count
         FROM page_auctions
         JOIN public.auctions AS a ON a.id = page_auctions.id
         LEFT JOIN public.lots AS l ON l.auction_id = a.id
         GROUP BY a.id
         ORDER BY a.created_at DESC, a.id DESC",
    )
    .bind(request.world_id.as_deref())
    .bind(request.status.as_deref())
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Admin 拍卖列表失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            let auction = map_auction(&row)?;
            let summary = AdminAuctionSummary {
                auction,
                scheduled_ends_at: row
                    .try_get("scheduled_ends_at")
                    .map_err(|_| storage_error("读取 Admin 拍卖预计结束时间失败"))?,
                lot_count: row
                    .try_get("lot_count")
                    .map_err(|_| storage_error("读取 Admin 拍卖 lot 数量失败"))?,
            };
            if !summary.validate()
                || request
                    .world_id
                    .as_deref()
                    .is_some_and(|world| summary.auction.world_id != world)
                || request.status.as_deref().is_some_and(|status| summary.auction.status != status)
            {
                return Err(storage_error("Admin 拍卖列表返回筛选漂移"));
            }
            Ok(summary)
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;

    transaction.commit().await.map_err(|_| storage_error("提交 Admin 拍卖列表读取事务失败"))?;
    let page = AdminAuctionPage { entries, total, limit: request.limit, offset: request.offset };
    if !page.validate(request) {
        return Err(storage_error("Admin 拍卖列表存储返回不变量失败"));
    }
    Ok(page)
}

#[cfg(feature = "postgres")]
fn map_auction(
    row: &sqlx::postgres::PgRow,
) -> Result<auction_application::SettlementAuctionSnapshot, ApplicationError> {
    use auction_domain::AuctionId;
    use chrono::{DateTime, Utc};
    use sqlx::Row;

    Ok(auction_application::SettlementAuctionSnapshot {
        id: AuctionId::from_uuid(
            row.try_get("id").map_err(|_| storage_error("读取 Admin 拍卖标识失败"))?,
        ),
        world_id: row.try_get("world_id").map_err(|_| storage_error("读取 Admin 拍卖世界失败"))?,
        sequence: row.try_get("sequence").map_err(|_| storage_error("读取 Admin 拍卖序号失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取 Admin 拍卖状态失败"))?,
        preview_scheduled_at: row
            .try_get::<DateTime<Utc>, _>("preview_scheduled_at")
            .map_err(|_| storage_error("读取 Admin 拍卖预览计划时间失败"))?,
        scheduled_open_at: row
            .try_get::<DateTime<Utc>, _>("scheduled_open_at")
            .map_err(|_| storage_error("读取 Admin 拍卖开放计划时间失败"))?,
        preview_opened_at: row
            .try_get("preview_opened_at")
            .map_err(|_| storage_error("读取 Admin 拍卖预览开放时间失败"))?,
        preview_manual_open: row
            .try_get("preview_manual_open")
            .map_err(|_| storage_error("读取 Admin 拍卖手动开放标记失败"))?,
        preview_revision: row
            .try_get("preview_revision")
            .map_err(|_| storage_error("读取 Admin 拍卖预览版本失败"))?,
        opened_at: row
            .try_get("opened_at")
            .map_err(|_| storage_error("读取 Admin 拍卖开放时间失败"))?,
        final_phase_at: row
            .try_get("final_phase_at")
            .map_err(|_| storage_error("读取 Admin 拍卖最终阶段时间失败"))?,
        settled_at: row
            .try_get("settled_at")
            .map_err(|_| storage_error("读取 Admin 拍卖结算时间失败"))?,
        cancelled_at: row
            .try_get("cancelled_at")
            .map_err(|_| storage_error("读取 Admin 拍卖取消时间失败"))?,
        cancelled_reason: row
            .try_get("cancelled_reason")
            .map_err(|_| storage_error("读取 Admin 拍卖取消原因失败"))?,
        cancelled_by: row
            .try_get("cancelled_by")
            .map_err(|_| storage_error("读取 Admin 拍卖取消操作者失败"))?,
        state_version: row
            .try_get("state_version")
            .map_err(|_| storage_error("读取 Admin 拍卖状态版本失败"))?,
        scheduled_end_at: row
            .try_get("scheduled_end_at")
            .map_err(|_| storage_error("读取 Admin 拍卖计划结束时间失败"))?,
        effective_end_at: row
            .try_get("effective_end_at")
            .map_err(|_| storage_error("读取 Admin 拍卖有效结束时间失败"))?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| storage_error("读取 Admin 拍卖创建时间失败"))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| storage_error("读取 Admin 拍卖更新时间失败"))?,
    })
}
