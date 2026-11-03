//! Public 补偿池批次、当前余额查询，以及单场批次摘要的共享读取。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::{
    ApplicationError, PublicCompensationCursor, PublicCompensationListRequest,
    PublicCompensationPage, PublicCompensationRound,
};
use auction_domain::SettlementMode;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::super::storage_error;
use crate::database::PostgresStore;

/// 在调用方的只读快照中读取单场已结算批次摘要；未结算或不存在时返回 `None`。
pub(super) async fn load_round(
    connection: &mut PgConnection,
    auction_id: Uuid,
) -> Result<Option<PublicCompensationRound>, ApplicationError> {
    sqlx::query(concat!(
        "SELECT ",
        round_columns!(),
        "
         FROM public.auction_settlements AS settlement
         JOIN public.auctions AS a
           ON a.id = settlement.auction_id AND a.status = 'settled'
         JOIN public.worlds AS world ON world.id = a.world_id
         ",
        round_totals!(),
        "
         WHERE settlement.auction_id = $1"
    ))
    .bind(auction_id)
    .fetch_optional(connection)
    .await
    .map_err(|error| storage_error_from(error, "读取公开补偿池批次失败"))?
    .map(|row| map_round(&row))
    .transpose()
}

fn map_round(row: &PgRow) -> Result<PublicCompensationRound, ApplicationError> {
    let policy_mode: String = row
        .try_get("policy_mode")
        .map_err(|error| storage_error_from(error, "读取补偿池结算策略失败"))?;
    Ok(PublicCompensationRound {
        auction_id: row
            .try_get("auction_id")
            .map_err(|error| storage_error_from(error, "读取补偿池拍卖标识失败"))?,
        world_id: row
            .try_get("world_id")
            .map_err(|error| storage_error_from(error, "读取补偿池世界标识失败"))?,
        auction_sequence: row
            .try_get("auction_sequence")
            .map_err(|error| storage_error_from(error, "读取补偿池拍卖序号失败"))?,
        policy_mode: SettlementMode::from_name(&policy_mode)
            .ok_or_else(|| storage_error("补偿池结算策略不在公开闭集内"))?,
        opening_balance: row
            .try_get("opening_balance")
            .map_err(|error| storage_error_from(error, "读取补偿池期初余额失败"))?,
        tax_income: row
            .try_get("tax_income")
            .map_err(|error| storage_error_from(error, "读取补偿池税收失败"))?,
        discount_demand: row
            .try_get("discount_demand")
            .map_err(|error| storage_error_from(error, "读取补偿池折扣需求失败"))?,
        compensation_out: row
            .try_get("compensation_out")
            .map_err(|error| storage_error_from(error, "读取补偿池支出失败"))?,
        closing_balance: row
            .try_get("closing_balance")
            .map_err(|error| storage_error_from(error, "读取补偿池期末余额失败"))?,
        movement_count: row
            .try_get("movement_count")
            .map_err(|error| storage_error_from(error, "读取补偿池账本笔数失败"))?,
        reversal_count: row
            .try_get("reversal_count")
            .map_err(|error| storage_error_from(error, "读取补偿池冲正笔数失败"))?,
        reversal_amount: row
            .try_get("reversal_amount")
            .map_err(|error| storage_error_from(error, "读取补偿池冲正金额失败"))?,
        settled_at: row
            .try_get("settled_at")
            .map_err(|error| storage_error_from(error, "读取补偿池结算时间失败"))?,
    })
}

/// 在同一只读快照中读取补偿批次分页、汇总和最新余额。
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
    // 仅额外探测行采用饱和加法；i64::MAX 已覆盖可表示行数，不改变请求及响应数量。
    let query_limit = if use_cursor { request.limit.saturating_add(1) } else { request.limit };

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启公开补偿池读取事务失败"))?;

    // 先按原游标或编号页选定批次，再读取账本合计，避免页外场次重复执行 LATERAL 聚合。
    let rows = sqlx::query(concat!(
        "WITH page_auctions AS MATERIALIZED (
            SELECT a.id
            FROM public.auction_settlements AS settlement
            JOIN public.auctions AS a
              ON a.id = settlement.auction_id AND a.status = 'settled'
            JOIN public.worlds AS world ON world.id = a.world_id
            WHERE (
                NOT $1::boolean
                OR a.sequence < $2::bigint
                OR (a.sequence = $2::bigint AND a.id < $3::uuid)
            )
            ORDER BY a.sequence DESC, a.id DESC
            LIMIT $4 OFFSET $5
         )
         SELECT ",
        round_columns!(),
        "
         FROM page_auctions
         JOIN public.auctions AS a ON a.id = page_auctions.id
         JOIN public.auction_settlements AS settlement ON settlement.auction_id = a.id
         ",
        round_totals!(),
        "
         ORDER BY a.sequence DESC, a.id DESC"
    ))
    .bind(use_boundary)
    .bind(boundary_sequence)
    .bind(boundary_id)
    .bind(query_limit)
    .bind(if use_cursor { 0 } else { request.offset })
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取公开补偿池批次失败"))?;

    let mut entries = rows.iter().map(map_round).collect::<Result<Vec<_>, ApplicationError>>()?;

    let entry_count =
        i64::try_from(entries.len()).map_err(|_| storage_error("补偿池页大小溢出"))?;
    let has_more = use_cursor && entry_count > request.limit;
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
                 JOIN public.worlds AS world ON world.id = a.world_id",
            )
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| storage_error_from(error, "统计公开补偿池批次失败"))?,
        )
    };

    // 两个世界共用一个补偿池；单行缺失说明 schema 不完整，读取失败关闭。
    let state = sqlx::query("SELECT balance, updated_at FROM public.compensation_pool")
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取公开补偿池余额失败"))?;
    let balance: i64 = state
        .try_get("balance")
        .map_err(|error| storage_error_from(error, "读取公开补偿池余额字段失败"))?;
    let updated_at: DateTime<Utc> = state
        .try_get("updated_at")
        .map_err(|error| storage_error_from(error, "读取公开补偿池更新时间失败"))?;

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
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交公开补偿池读取事务失败"))?;
    page.validate().map_err(storage_error)?;
    Ok(page)
}
