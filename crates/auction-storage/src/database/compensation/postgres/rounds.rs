//! Public 补偿池批次与余额查询。

use auction_application::{
    ApplicationError, PublicCompensationCursor, PublicCompensationListRequest,
    PublicCompensationPage, PublicCompensationRound,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::super::storage_error;
use crate::database::PostgresStore;

/// 在同一 Serializable 快照中读取补偿批次分页、汇总和最新余额。
#[cfg(feature = "postgres")]
pub(crate) async fn list(
    store: &PostgresStore,
    request: PublicCompensationListRequest,
) -> Result<PublicCompensationPage, ApplicationError> {
    let use_cursor = request.cursor.is_some();
    let (use_boundary, boundary_sequence, boundary_id) = match request.cursor.as_ref() {
        Some(PublicCompensationCursor::Boundary { sequence, id }) => (true, *sequence, *id),
        Some(PublicCompensationCursor::Start) | None => (false, 0, Uuid::nil()),
    };
    let query_limit = if use_cursor {
        request.limit.checked_add(1).ok_or_else(|| storage_error("公开补偿池分页上限溢出"))?
    } else {
        request.limit
    };

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启公开补偿池读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置公开补偿池读取隔离级别失败"))?;

    let rows = sqlx::query(
        "SELECT
            a.id AS auction_id,
            a.world_id,
            a.sequence AS auction_sequence,
            settlement.opening_pool_balance AS opening_balance,
            totals.tax_income,
            settlement.discount_demand,
            totals.compensation_out,
            settlement.closing_pool_balance AS closing_balance,
            totals.movement_count,
            settlement.settled_at
         FROM public.auction_settlements AS settlement
         JOIN public.auctions AS a
           ON a.id = settlement.auction_id AND a.status = 'settled'
         JOIN public.worlds AS world ON world.id = a.world_id AND world.enabled
         JOIN LATERAL (
             SELECT
                 count(*)::bigint AS movement_count,
                 coalesce(sum(ledger.amount) FILTER (WHERE ledger.movement_kind = 'tax'), 0)::bigint
                     AS tax_income,
                 coalesce(
                     -(sum(ledger.amount) FILTER (WHERE ledger.movement_kind = 'compensation')),
                     0
                 )::bigint AS compensation_out
             FROM public.compensation_ledger AS ledger
             WHERE ledger.auction_id = settlement.auction_id
         ) AS totals ON true
         WHERE (
             NOT $1::boolean
             OR a.sequence < $2::bigint
             OR (a.sequence = $2::bigint AND a.id < $3::uuid)
         )
         ORDER BY a.sequence DESC, a.id DESC
         LIMIT $4 OFFSET $5",
    )
    .bind(use_boundary)
    .bind(boundary_sequence)
    .bind(boundary_id)
    .bind(query_limit)
    .bind(if use_cursor { 0 } else { request.offset })
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取公开补偿池批次失败"))?;

    let mut entries = rows
        .into_iter()
        .map(|row| {
            Ok(PublicCompensationRound {
                auction_id: row
                    .try_get("auction_id")
                    .map_err(|_| storage_error("读取补偿池拍卖标识失败"))?,
                world_id: row
                    .try_get("world_id")
                    .map_err(|_| storage_error("读取补偿池世界标识失败"))?,
                auction_sequence: row
                    .try_get("auction_sequence")
                    .map_err(|_| storage_error("读取补偿池拍卖序号失败"))?,
                opening_balance: row
                    .try_get("opening_balance")
                    .map_err(|_| storage_error("读取补偿池期初余额失败"))?,
                tax_income: row
                    .try_get("tax_income")
                    .map_err(|_| storage_error("读取补偿池税收失败"))?,
                discount_demand: row
                    .try_get("discount_demand")
                    .map_err(|_| storage_error("读取补偿池折扣需求失败"))?,
                compensation_out: row
                    .try_get("compensation_out")
                    .map_err(|_| storage_error("读取补偿池支出失败"))?,
                closing_balance: row
                    .try_get("closing_balance")
                    .map_err(|_| storage_error("读取补偿池期末余额失败"))?,
                movement_count: row
                    .try_get("movement_count")
                    .map_err(|_| storage_error("读取补偿池账本笔数失败"))?,
                settled_at: row
                    .try_get("settled_at")
                    .map_err(|_| storage_error("读取补偿池结算时间失败"))?,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;

    let has_more = use_cursor && entries.len() as i64 > request.limit;
    if has_more {
        entries.truncate(request.limit as usize);
    }
    let next_cursor = has_more
        .then(|| {
            entries.last().map(|entry| PublicCompensationCursor::Boundary {
                sequence: entry.auction_sequence,
                id: entry.auction_id,
            })
        })
        .flatten();

    let total = if use_cursor {
        None
    } else {
        Some(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*)::bigint
                 FROM public.auction_settlements AS settlement
                 JOIN public.auctions AS a
                   ON a.id = settlement.auction_id AND a.status = 'settled'
                 JOIN public.worlds AS world ON world.id = a.world_id AND world.enabled",
            )
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| storage_error("统计公开补偿池批次失败"))?,
        )
    };

    let state = sqlx::query(
        "SELECT
            coalesce(latest.closing_balance, 0)::bigint AS balance,
            latest.settled_at AS updated_at
         FROM (SELECT true AS singleton) AS one
         LEFT JOIN LATERAL (
             SELECT
                 settlement.closing_pool_balance AS closing_balance,
                 settlement.settled_at
             FROM public.auction_settlements AS settlement
             JOIN public.auctions AS a
               ON a.id = settlement.auction_id AND a.status = 'settled'
             JOIN public.worlds AS world ON world.id = a.world_id AND world.enabled
             ORDER BY settlement.settled_at DESC, a.id DESC
             LIMIT 1
         ) AS latest ON one.singleton",
    )
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取公开补偿池余额失败"))?;
    let balance: i64 =
        state.try_get("balance").map_err(|_| storage_error("读取公开补偿池余额字段失败"))?;
    let updated_at: Option<DateTime<Utc>> =
        state.try_get("updated_at").map_err(|_| storage_error("读取公开补偿池更新时间失败"))?;

    let page = PublicCompensationPage {
        entries,
        balance,
        updated_at,
        total,
        limit: request.limit,
        offset: if use_cursor { 0 } else { request.offset },
        next_cursor,
        has_more,
    };
    transaction.commit().await.map_err(|_| storage_error("提交公开补偿池读取事务失败"))?;
    page.validate().map_err(storage_error)?;
    Ok(page)
}
