//! W14.3 玩家通知事实扫描：卖方成交/流拍汇总与换季前 isekai 未完成义务。
//!
//! 只读投影，不写业务事实、不改写冻结结算或投递载荷。聚合、正文与稳定身份由
//! `auction-application` 负责；本模块只提供 SQL 事实。

#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::player_notifications::{
    PlayerNotificationScanner, SeasonReminderFact, SellerSummaryLotFact,
};

use super::DatabaseStore;
use crate::database::storage_error;

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

/// 一次扫描返回的事实行硬上限；超出的部分由下一轮继续。
const MAX_SCAN_ROWS: i64 = 4_000;

impl PlayerNotificationScanner for DatabaseStore {
    fn seller_summary_facts(
        &self,
        settled_since: chrono::DateTime<chrono::Utc>,
        limit: u32,
    ) -> ApplicationFuture<'_, Vec<SellerSummaryLotFact>> {
        Box::pin(async move {
            if limit == 0 || limit > 1_000 {
                return Err(storage_error("卖方汇总扫描批量无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    seller_summary_facts_postgres(store, settled_since, limit).await
                }
                #[cfg(not(feature = "postgres"))]
                _ => Err(storage_error("卖方汇总扫描需要 PostgreSQL 后端")),
            }
        })
    }

    fn season_reminder_facts(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        lead_seconds: i64,
        limit: u32,
    ) -> ApplicationFuture<'_, Vec<SeasonReminderFact>> {
        Box::pin(async move {
            if limit == 0 || limit > 1_000 || !(1..=30 * 24 * 60 * 60).contains(&lead_seconds) {
                return Err(storage_error("换季提醒扫描参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    season_reminder_facts_postgres(store, now, lead_seconds, limit).await
                }
                #[cfg(not(feature = "postgres"))]
                _ => Err(storage_error("换季提醒扫描需要 PostgreSQL 后端")),
            }
        })
    }
}

/// 只返回窗口内已结算、且至少一个卖方尚无汇总的 lot；分组与正文由应用层完成。
#[cfg(feature = "postgres")]
async fn seller_summary_facts_postgres(
    store: &super::PostgresStore,
    settled_since: DateTime<Utc>,
    limit: u32,
) -> Result<Vec<SellerSummaryLotFact>, ApplicationError> {
    use sqlx::Row;

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启卖方汇总扫描事务失败"))?;
    let rows = sqlx::query(
        r#"WITH due AS (
               SELECT auction.id
               FROM public.auctions AS auction
               WHERE auction.status = 'settled'
                 AND auction.settled_at IS NOT NULL
                 AND auction.settled_at >= $1
                 AND EXISTS (
                     SELECT 1
                     FROM public.lots AS candidate
                     JOIN public.auction_submissions AS submission
                       ON submission.id = candidate.submission_id
                     WHERE candidate.auction_id = auction.id
                       AND candidate.status IN ('sold', 'unsold')
                       AND NOT EXISTS (
                           SELECT 1
                           FROM public.deliveries AS delivery
                           WHERE delivery.origin_kind = 'seller_summary'
                             AND delivery.origin_id = 'seller_summary:'
                                 || auction.id::text || ':' || submission.seller
                       )
                 )
               ORDER BY auction.settled_at DESC, auction.id
               LIMIT $2
           )
           SELECT auction.id AS auction_id, auction.world_id, auction.sequence,
                  submission.seller, lot.lot_number, equipment.category,
                  equipment.name AS equipment_name, lot.status, lot.current_price
           FROM due
           JOIN public.auctions AS auction ON auction.id = due.id
           JOIN public.lots AS lot ON lot.auction_id = auction.id
           JOIN public.auction_submissions AS submission ON submission.id = lot.submission_id
           JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
           WHERE lot.status IN ('sold', 'unsold')
           ORDER BY auction.settled_at DESC, auction.id, submission.seller, lot.lot_number
           LIMIT $3"#,
    )
    .bind(settled_since)
    .bind(i64::from(limit))
    .bind(MAX_SCAN_ROWS)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取卖方汇总事实失败"))?;

    let mut facts = Vec::with_capacity(rows.len());
    for row in rows {
        let sold_price = if row.try_get::<String, _>("status").map_err(map_row_error)? == "sold" {
            row.try_get::<Option<i64>, _>("current_price").map_err(map_row_error)?
        } else {
            None
        };
        facts.push(SellerSummaryLotFact {
            auction_id: row.try_get("auction_id").map_err(map_row_error)?,
            world: row.try_get("world_id").map_err(map_row_error)?,
            sequence: row.try_get("sequence").map_err(map_row_error)?,
            seller: row.try_get("seller").map_err(map_row_error)?,
            lot_number: u32::try_from(row.try_get::<i32, _>("lot_number").map_err(map_row_error)?)
                .map_err(|error| {
                    let _ = error;
                    storage_error("卖方汇总 lot 编号无效")
                })?,
            category: row.try_get("category").map_err(map_row_error)?,
            equipment_name: row.try_get("equipment_name").map_err(map_row_error)?,
            sold_price,
        });
    }
    Ok(facts)
}

/// 返回即将到来的冻结赛季边界之前、仍有未完成 isekai 义务的收件人事实。
///
/// 只使用已冻结且非 NULL 的 `auctions.intake_opens_at`；没有边界时返回空集，
/// 不从当前排期配置推测。边界本身随每行返回，正文与身份据此冻结。
#[cfg(feature = "postgres")]
async fn season_reminder_facts_postgres(
    store: &super::PostgresStore,
    now: DateTime<Utc>,
    lead_seconds: i64,
    limit: u32,
) -> Result<Vec<SeasonReminderFact>, ApplicationError> {
    use sqlx::Row;

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启换季提醒扫描事务失败"))?;
    let rows = sqlx::query(
        r#"WITH boundary AS (
               SELECT min(auction.intake_opens_at) AS at
               FROM public.auctions AS auction
               WHERE auction.world_id = 'isekai'
                 AND auction.status <> 'cancelled'
                 AND auction.intake_opens_at IS NOT NULL
                 AND auction.intake_opens_at > $1
                 AND auction.intake_opens_at <= $1 + ($2::bigint * interval '1 second')
           )
           SELECT boundary.at AS season_boundary,
                  COALESCE(obligation.recipient_game_identity, recipient.username) AS recipient,
                  obligation.obligation_kind, lot.lot_number, equipment.category,
                  equipment.name AS equipment_name, obligation.amount
           FROM boundary,
                public.settlement_obligations AS obligation
           JOIN public.auctions AS auction ON auction.id = obligation.auction_id
           JOIN public.lots AS lot ON lot.id = obligation.lot_id
           JOIN public.equipment AS equipment ON equipment.id = obligation.equipment_id
           JOIN public.settlement_obligation_fulfillments AS fulfillment
             ON fulfillment.obligation_id = obligation.id
           LEFT JOIN public.users AS recipient ON recipient.id = obligation.recipient_user_id
           WHERE boundary.at IS NOT NULL
             AND auction.world_id = 'isekai'
             AND fulfillment.execution_state IN ('pending', 'prepared', 'enqueued')
             AND COALESCE(obligation.recipient_game_identity, recipient.username) IS NOT NULL
             AND NOT EXISTS (
                 SELECT 1
                 FROM public.deliveries AS delivery
                 WHERE delivery.origin_kind = 'season_reminder'
                   AND delivery.origin_id = 'season_reminder:'
                       || (extract(epoch FROM boundary.at))::bigint::text
                       || ':' || COALESCE(obligation.recipient_game_identity, recipient.username)
             )
           ORDER BY recipient, lot.lot_number, obligation.obligation_kind
           LIMIT $3"#,
    )
    .bind(now)
    .bind(lead_seconds)
    .bind(i64::from(limit) * 64)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取换季提醒事实失败"))?;

    let mut facts = Vec::with_capacity(rows.len());
    for row in rows {
        facts.push(SeasonReminderFact {
            season_boundary: row.try_get("season_boundary").map_err(map_row_error)?,
            recipient: row.try_get("recipient").map_err(map_row_error)?,
            obligation_kind: row.try_get("obligation_kind").map_err(map_row_error)?,
            lot_number: u32::try_from(row.try_get::<i32, _>("lot_number").map_err(map_row_error)?)
                .map_err(|error| {
                    let _ = error;
                    storage_error("换季提醒 lot 编号无效")
                })?,
            category: row.try_get("category").map_err(map_row_error)?,
            equipment_name: row.try_get("equipment_name").map_err(map_row_error)?,
            amount: row.try_get("amount").map_err(map_row_error)?,
        });
    }
    Ok(facts)
}

#[cfg(feature = "postgres")]
fn map_row_error(error: sqlx::Error) -> ApplicationError {
    storage_error_from(error, "读取玩家通知事实失败")
}
