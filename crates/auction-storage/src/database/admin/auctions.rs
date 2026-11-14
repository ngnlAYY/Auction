//! Admin 拍卖列表与生命周期只读投影；查询和行映射由 PostgreSQL owner 负责。

use auction_application::admin::AdminAuctionSummary;
use auction_application::admin::{AdminAuctionListRequest, AdminAuctionPage, AdminAuctionReader};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;

use crate::database::DatabaseStore;
use crate::database::auction::snapshot::{auction_snapshot_columns, decode_auction_snapshot};
use crate::database::storage_error;
use crate::database::storage_error_from;

impl AdminAuctionReader for DatabaseStore {
    fn list_admin_auctions(
        &self,
        request: AdminAuctionListRequest,
    ) -> ApplicationFuture<'_, AdminAuctionPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(storage_error("Admin 拍卖列表参数无效"));
            }
            list_postgres(self, &request).await
        })
    }
}

async fn list_postgres(
    store: &crate::database::DatabaseStore,
    request: &AdminAuctionListRequest,
) -> Result<AdminAuctionPage, ApplicationError> {
    use sqlx::Row;

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启 Admin 拍卖列表读取事务失败"))?;

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
    .map_err(|error| storage_error_from(error, "读取 Admin 拍卖列表总数失败"))?;

    // 计数保持完整筛选集；lot 汇总只处理当前页，避免历史场次越多每页扫描越重。
    let rows = sqlx::query(concat!(
        "WITH page_auctions AS MATERIALIZED (
            SELECT a.id
            FROM public.auctions AS a
            WHERE ($1::text IS NULL OR a.world_id = $1)
              AND ($2::text IS NULL OR a.status = $2)
            ORDER BY a.created_at DESC, a.id DESC
            LIMIT $3 OFFSET $4
         )
         SELECT ",
        auction_snapshot_columns!(),
        ",
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
         ORDER BY a.created_at DESC, a.id DESC"
    ))
    .bind(request.world_id.as_deref())
    .bind(request.status.as_deref())
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 Admin 拍卖列表失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            let auction = decode_auction_snapshot(&row)
                .map_err(|(context, error)| storage_error_from(error, context))?;
            let summary = AdminAuctionSummary {
                auction,
                scheduled_ends_at: row.try_get("scheduled_ends_at").map_err(|error| {
                    storage_error_from(error, "读取 Admin 拍卖预计结束时间失败")
                })?,
                lot_count: row
                    .try_get("lot_count")
                    .map_err(|error| storage_error_from(error, "读取 Admin 拍卖 lot 数量失败"))?,
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

    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 Admin 拍卖列表读取事务失败"))?;
    let page = AdminAuctionPage { entries, total, limit: request.limit, offset: request.offset };
    if !page.validate(request) {
        return Err(storage_error("Admin 拍卖列表存储返回不变量失败"));
    }
    Ok(page)
}
