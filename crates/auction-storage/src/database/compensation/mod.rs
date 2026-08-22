//! Public 补偿池 PostgreSQL 读取适配器。
//!
//! PostgreSQL 完整档在一个 Serializable 快照中读取结算批次、补偿账本和最新池余额；
//! SQLite Lite 没有结算表，因此显式报告能力缺失，避免把空集合误当成真实补偿池。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicCompensationListRequest,
    PublicCompensationMovementPage, PublicCompensationMovementRequest, PublicCompensationPage,
    PublicCompensationReader,
};
#[cfg(feature = "postgres")]
use auction_application::{
    PublicCompensationCursor, PublicCompensationMovement, PublicCompensationMovementKind,
    PublicCompensationRound,
};
use uuid::Uuid;

use super::DatabaseStore;

impl PublicCompensationReader for DatabaseStore {
    fn list_public_compensation(
        &self,
        request: PublicCompensationListRequest,
    ) -> ApplicationFuture<'_, PublicCompensationPage> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "公开补偿池仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn list_public_compensation_movements(
        &self,
        auction_id: Uuid,
        request: PublicCompensationMovementRequest,
    ) -> ApplicationFuture<'_, Option<PublicCompensationMovementPage>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("公开补偿账本拍卖标识无效"));
            }
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    movements_postgres(store, auction_id, request).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "公开补偿账本仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
    request: PublicCompensationListRequest,
) -> Result<PublicCompensationPage, ApplicationError> {
    use chrono::{DateTime, Utc};
    use sqlx::Row;

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

#[cfg(feature = "postgres")]
async fn movements_postgres(
    store: &super::PostgresStore,
    auction_id: Uuid,
    request: PublicCompensationMovementRequest,
) -> Result<Option<PublicCompensationMovementPage>, ApplicationError> {
    use chrono::DateTime;
    use sqlx::Row;

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启公开补偿账本读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置公开补偿账本读取隔离级别失败"))?;

    let round_row = sqlx::query(
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
         WHERE settlement.auction_id = $1",
    )
    .bind(auction_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取公开补偿池批次失败"))?;

    let Some(round_row) = round_row else {
        transaction.commit().await.map_err(|_| storage_error("提交公开补偿账本空结果事务失败"))?;
        return Ok(None);
    };

    let round = PublicCompensationRound {
        auction_id: round_row
            .try_get("auction_id")
            .map_err(|_| storage_error("读取补偿账本拍卖标识失败"))?,
        world_id: round_row
            .try_get("world_id")
            .map_err(|_| storage_error("读取补偿账本世界标识失败"))?,
        auction_sequence: round_row
            .try_get("auction_sequence")
            .map_err(|_| storage_error("读取补偿账本拍卖序号失败"))?,
        opening_balance: round_row
            .try_get("opening_balance")
            .map_err(|_| storage_error("读取补偿账本期初余额失败"))?,
        tax_income: round_row
            .try_get("tax_income")
            .map_err(|_| storage_error("读取补偿账本税收失败"))?,
        discount_demand: round_row
            .try_get("discount_demand")
            .map_err(|_| storage_error("读取补偿账本折扣需求失败"))?,
        compensation_out: round_row
            .try_get("compensation_out")
            .map_err(|_| storage_error("读取补偿账本支出失败"))?,
        closing_balance: round_row
            .try_get("closing_balance")
            .map_err(|_| storage_error("读取补偿账本期末余额失败"))?,
        movement_count: round_row
            .try_get("movement_count")
            .map_err(|_| storage_error("读取补偿账本笔数失败"))?,
        settled_at: round_row
            .try_get("settled_at")
            .map_err(|_| storage_error("读取补偿账本结算时间失败"))?,
    };

    let rows = sqlx::query(
        "SELECT
            ledger.auction_id,
            ledger.entry_sequence,
            ledger.lot_id,
            ledger.movement_kind,
            ledger.amount,
            ledger.balance_after,
            ledger.created_at,
            lot.lot_number,
            equipment.name AS equipment_name,
            equipment.category AS equipment_category
         FROM public.compensation_ledger AS ledger
         LEFT JOIN public.lots AS lot ON lot.id = ledger.lot_id
         LEFT JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
         WHERE ledger.auction_id = $1
         ORDER BY ledger.entry_sequence
         LIMIT $2 OFFSET $3",
    )
    .bind(auction_id)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取公开补偿账本条目失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            let kind: String =
                row.try_get("movement_kind").map_err(|_| storage_error("读取补偿账本类型失败"))?;
            let kind = PublicCompensationMovementKind::parse(&kind)
                .ok_or_else(|| storage_error("补偿账本类型不在公开闭集内"))?;
            let created_at: DateTime<_> =
                row.try_get("created_at").map_err(|_| storage_error("读取补偿账本时间失败"))?;
            Ok(PublicCompensationMovement {
                auction_id: row
                    .try_get("auction_id")
                    .map_err(|_| storage_error("读取补偿账本条目拍卖标识失败"))?,
                sequence: row
                    .try_get("entry_sequence")
                    .map_err(|_| storage_error("读取补偿账本序号失败"))?,
                lot_id: row
                    .try_get("lot_id")
                    .map_err(|_| storage_error("读取补偿账本 lot 标识失败"))?,
                lot_number: row
                    .try_get("lot_number")
                    .map_err(|_| storage_error("读取补偿账本 lot 序号失败"))?,
                equipment_name: row
                    .try_get("equipment_name")
                    .map_err(|_| storage_error("读取补偿账本装备名称失败"))?,
                equipment_category: row
                    .try_get("equipment_category")
                    .map_err(|_| storage_error("读取补偿账本装备分类失败"))?,
                kind,
                amount: row.try_get("amount").map_err(|_| storage_error("读取补偿账本金额失败"))?,
                balance_after: row
                    .try_get("balance_after")
                    .map_err(|_| storage_error("读取补偿账本余额失败"))?,
                created_at,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;

    let page = PublicCompensationMovementPage {
        total: round.movement_count,
        round,
        entries,
        limit: request.limit,
        offset: request.offset,
    };
    transaction.commit().await.map_err(|_| storage_error("提交公开补偿账本读取事务失败"))?;
    page.validate().map_err(storage_error)?;
    Ok(Some(page))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
