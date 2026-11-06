//! Admin 已结算拍卖选择目录适配器。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

#[cfg(feature = "postgres")]
use auction_application::admin::SettlementCatalogEntry;
use auction_application::admin::{
    SettlementCatalogPage, SettlementCatalogReader, SettlementCatalogRequest,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;

use super::DatabaseStore;
use crate::database::storage_error;

impl SettlementCatalogReader for DatabaseStore {
    fn list_settlement_catalog(
        &self,
        request: SettlementCatalogRequest,
    ) -> ApplicationFuture<'_, SettlementCatalogPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(storage_error("结算报表目录参数无效"));
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
    request: &SettlementCatalogRequest,
) -> Result<SettlementCatalogPage, ApplicationError> {
    use auction_domain::AuctionId;
    use sqlx::Row;

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启结算报表目录读取事务失败"))?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)::bigint
         FROM public.auctions AS auction
         WHERE auction.status = 'settled'
           AND auction.settled_at IS NOT NULL
           AND ($1::text IS NULL OR auction.world_id = $1)
           AND ($2::bigint IS NULL OR auction.sequence = $2)",
    )
    .bind(request.world_id.as_deref())
    .bind(request.sequence)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "统计结算报表目录失败"))?;
    let rows = sqlx::query(
        "SELECT auction.id, auction.world_id, auction.sequence, auction.settled_at
         FROM public.auctions AS auction
         WHERE auction.status = 'settled'
           AND auction.settled_at IS NOT NULL
           AND ($1::text IS NULL OR auction.world_id = $1)
           AND ($2::bigint IS NULL OR auction.sequence = $2)
         ORDER BY
             CASE WHEN $1::text IS NOT NULL THEN auction.sequence END DESC,
             auction.settled_at DESC,
             auction.world_id ASC,
             auction.sequence DESC,
             auction.id DESC
         LIMIT $3 OFFSET $4",
    )
    .bind(request.world_id.as_deref())
    .bind(request.sequence)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取结算报表目录失败"))?;
    let entries = rows
        .into_iter()
        .map(|row| {
            Ok(SettlementCatalogEntry {
                auction_id: AuctionId::from_uuid(
                    row.try_get("id")
                        .map_err(|error| storage_error_from(error, "读取结算目录拍卖标识失败"))?,
                ),
                world_id: row
                    .try_get("world_id")
                    .map_err(|error| storage_error_from(error, "读取结算目录世界失败"))?,
                sequence: row
                    .try_get("sequence")
                    .map_err(|error| storage_error_from(error, "读取结算目录期号失败"))?,
                settled_at: row
                    .try_get("settled_at")
                    .map_err(|error| storage_error_from(error, "读取结算目录结算时间失败"))?,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交结算报表目录读取事务失败"))?;
    let page =
        SettlementCatalogPage { entries, total, limit: request.limit, offset: request.offset };
    if !page.validate(request) {
        return Err(storage_error("结算报表目录返回不变量失败"));
    }
    Ok(page)
}
