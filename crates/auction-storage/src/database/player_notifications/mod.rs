//! 玩家通知事实扫描：卖方成交/流拍汇总与换季前 isekai 未完成义务。
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
            seller_summary_facts_postgres(self, settled_since, limit).await
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
            season_reminder_facts_postgres(self, now, lead_seconds, limit).await
        })
    }
}

/// 只返回窗口内已结算、且至少一个卖方尚无汇总的 lot；分组与正文由应用层完成。
#[cfg(feature = "postgres")]
const SELLER_SUMMARY_FACTS_SQL: &str = r#"WITH due AS (
           SELECT auction.id, auction.world_id, auction.sequence, auction.settled_at
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
       ),
       -- 限额只作用于 (auction, seller) 组；最终查询必须取回这些组的全部 lot，
       -- 否则按行截断会让应用层把不完整摘要冻结进稳定命令。
       selected_groups AS (
           SELECT DISTINCT due.id AS auction_id, submission.seller,
                           due.world_id, due.sequence, due.settled_at
           FROM due
           JOIN public.lots AS candidate ON candidate.auction_id = due.id
           JOIN public.auction_submissions AS submission
             ON submission.id = candidate.submission_id
           WHERE candidate.status IN ('sold', 'unsold')
             AND NOT EXISTS (
                 SELECT 1
                 FROM public.deliveries AS delivery
                 WHERE delivery.origin_kind = 'seller_summary'
                   AND delivery.origin_id = 'seller_summary:'
                       || due.id::text || ':' || submission.seller
             )
           ORDER BY due.settled_at DESC, due.id, submission.seller
           LIMIT $2
       )
       SELECT auction.id AS auction_id, auction.world_id, auction.sequence,
              submission.seller, lot.lot_number, lot.category_number, equipment.category,
              equipment.name AS equipment_name, lot.status, lot.current_price
       FROM selected_groups AS selected
       JOIN public.auctions AS auction ON auction.id = selected.auction_id
       JOIN public.lots AS lot ON lot.auction_id = selected.auction_id
       JOIN public.auction_submissions AS submission
         ON submission.id = lot.submission_id AND submission.seller = selected.seller
       JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
       WHERE lot.status IN ('sold', 'unsold')
       ORDER BY auction.settled_at DESC, auction.id, submission.seller, lot.lot_number"#;

#[cfg(feature = "postgres")]
async fn seller_summary_facts_postgres(
    store: &super::DatabaseStore,
    settled_since: DateTime<Utc>,
    limit: u32,
) -> Result<Vec<SellerSummaryLotFact>, ApplicationError> {
    use sqlx::Row;

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启卖方汇总扫描事务失败"))?;
    let rows = sqlx::query(SELLER_SUMMARY_FACTS_SQL)
        .bind(settled_since)
        .bind(i64::from(limit))
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
            category_number: u32::try_from(
                row.try_get::<i32, _>("category_number").map_err(map_row_error)?,
            )
            .map_err(|error| {
                let _ = error;
                storage_error("卖方汇总类别内编号无效")
            })?,
            category: row.try_get("category").map_err(map_row_error)?,
            equipment_name: row.try_get("equipment_name").map_err(map_row_error)?,
            sold_price,
        });
    }
    Ok(facts)
}

/// 限额只作用于 (冻结边界, 收件人) 组，并取回选中组的全部义务；未激活的 contingent
/// 退款不是当前欠付。退役与发送围栏使用同一义务口径。
#[cfg(feature = "postgres")]
const SEASON_REMINDER_FACTS_SQL: &str = r#"WITH boundary AS (
           SELECT min(auction.intake_opens_at) AS at
           FROM public.auctions AS auction
           WHERE auction.world_id = 'isekai'
             AND auction.status <> 'cancelled'
             AND auction.intake_opens_at IS NOT NULL
             AND auction.intake_opens_at > $1
             AND auction.intake_opens_at <= $1 + ($2::bigint * interval '1 second')
       ),
       due AS (
           SELECT boundary.at AS season_boundary,
                  COALESCE(obligation.recipient_game_identity, recipient.username) AS recipient,
                  obligation.obligation_kind, lot.id AS lot_id, lot.lot_number,
                  lot.category_number, equipment.category,
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
             -- 未经恢复显式激活的 contingent 退款不是当前欠付：正常成交会一直保留该义务。
             AND obligation.business_state <> 'contingent'
             AND COALESCE(obligation.recipient_game_identity, recipient.username) IS NOT NULL
             AND NOT EXISTS (
                 SELECT 1
                 FROM public.deliveries AS delivery
                 WHERE delivery.origin_kind = 'season_reminder'
                   AND delivery.origin_id = 'season_reminder:'
                       || (extract(epoch FROM boundary.at))::bigint::text
                       || ':' || COALESCE(obligation.recipient_game_identity, recipient.username)
             )
       ),
       -- 限额只作用于 (冻结边界, 收件人) 组；最终查询必须取回这些组的全部义务，
       -- 否则按行截断会让应用层把不完整提醒冻结进该组稳定命令，后续扫描又因投递已存在
       -- 而不再补寄缺失义务。
       selected_groups AS (
           SELECT DISTINCT due.season_boundary, due.recipient
           FROM due
           ORDER BY due.season_boundary, due.recipient
           LIMIT $3
       )
       SELECT due.season_boundary, due.recipient, due.obligation_kind, due.lot_id,
              due.lot_number, due.category_number, due.category, due.equipment_name, due.amount
       FROM due
       JOIN selected_groups AS selected
         ON selected.season_boundary = due.season_boundary
        AND selected.recipient = due.recipient
       ORDER BY due.recipient, due.lot_number, due.lot_id, due.obligation_kind"#;

/// 返回即将到来的冻结赛季边界之前、仍有未完成 isekai 义务的收件人事实。
///
/// 只使用已冻结且非 NULL 的 `auctions.intake_opens_at`；没有边界时返回空集，
/// 不从当前排期配置推测。边界本身随每行返回，正文与身份据此冻结。
#[cfg(feature = "postgres")]
async fn season_reminder_facts_postgres(
    store: &super::DatabaseStore,
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
    let rows = sqlx::query(SEASON_REMINDER_FACTS_SQL)
        .bind(now)
        .bind(lead_seconds)
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取换季提醒事实失败"))?;

    let mut facts = Vec::with_capacity(rows.len());
    for row in rows {
        facts.push(SeasonReminderFact {
            season_boundary: row.try_get("season_boundary").map_err(map_row_error)?,
            recipient: row.try_get("recipient").map_err(map_row_error)?,
            obligation_kind: row.try_get("obligation_kind").map_err(map_row_error)?,
            lot_id: row.try_get("lot_id").map_err(map_row_error)?,
            lot_number: u32::try_from(row.try_get::<i32, _>("lot_number").map_err(map_row_error)?)
                .map_err(|error| {
                    let _ = error;
                    storage_error("换季提醒 lot 编号无效")
                })?,
            category_number: u32::try_from(
                row.try_get::<i32, _>("category_number").map_err(map_row_error)?,
            )
            .map_err(|error| {
                let _ = error;
                storage_error("换季提醒类别内编号无效")
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

#[cfg(all(test, feature = "postgres"))]
mod tests {
    use super::{SEASON_REMINDER_FACTS_SQL, SELLER_SUMMARY_FACTS_SQL};
    use chrono::{Duration, Utc};
    use sqlx::{Connection, Executor, Row};

    /// 限额只限制组；选中一个卖方后必须取回该组全部 4001 条 lot，不能按行截断。
    #[tokio::test]
    #[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
    async fn postgres_seller_summary_returns_complete_selected_group()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
        let mut connection = sqlx::PgConnection::connect(&url).await?;
        let mut tx = connection.begin().await?;
        tx.execute(
            "CREATE TEMP TABLE auctions (
                id uuid, status text, settled_at timestamptz, world_id text, sequence bigint
             );
             CREATE TEMP TABLE auction_submissions (id uuid, seller text);
             CREATE TEMP TABLE lots (
                auction_id uuid, submission_id uuid, lot_number integer,
                category_number integer, status text, current_price bigint, equipment_id uuid
             );
             CREATE TEMP TABLE equipment (id uuid, category text, name text);
             CREATE TEMP TABLE deliveries (origin_kind text, origin_id text);
             INSERT INTO pg_temp.auctions
                VALUES ('00000000-0000-0000-0000-000000000001', 'settled', now(), 'isekai', 1);
             INSERT INTO pg_temp.auction_submissions
                VALUES ('00000000-0000-0000-0000-000000000002', 'alice');
             INSERT INTO pg_temp.equipment
                VALUES ('00000000-0000-0000-0000-000000000003', 'Claw', 'Item');
             INSERT INTO pg_temp.lots
                SELECT '00000000-0000-0000-0000-000000000001',
                       '00000000-0000-0000-0000-000000000002', number, number, 'sold', 1000,
                       '00000000-0000-0000-0000-000000000003'
                FROM generate_series(1, 4001) AS number;",
        )
        .await?;
        let rows = sqlx::query(sqlx::AssertSqlSafe(
            SELLER_SUMMARY_FACTS_SQL.replace("public.", "pg_temp."),
        ))
        .bind(Utc::now() - Duration::days(1))
        .bind(1_i64)
        .fetch_all(&mut *tx)
        .await?;
        assert_eq!(rows.len(), 4_001, "选中组的全部 lot 必须一次返回");
        assert!(rows.iter().all(|row| row.get::<String, _>("seller") == "alice"));
        tx.rollback().await?;
        Ok(())
    }

    /// 换季扫描与卖方汇总共用的最小来源表；义务由各用例显式插入。
    const SEASON_TABLES: &str = r#"
        CREATE TEMP TABLE auctions (
            id uuid, world_id text, status text, intake_opens_at timestamptz
        );
        CREATE TEMP TABLE lots (id uuid, lot_number integer, category_number integer);
        CREATE TEMP TABLE equipment (id uuid, category text, name text);
        CREATE TEMP TABLE settlement_obligations (
            id uuid, auction_id uuid, lot_id uuid, equipment_id uuid, obligation_kind text,
            amount bigint, business_state text, recipient_game_identity text,
            recipient_user_id uuid
        );
        CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text
        );
        CREATE TEMP TABLE users (id uuid, username text);
        CREATE TEMP TABLE deliveries (origin_kind text, origin_id text);
        INSERT INTO pg_temp.auctions VALUES
            ('00000000-0000-0000-0000-0000000000a1', 'isekai', 'draft',
             now() + interval '1 day');
        INSERT INTO pg_temp.equipment VALUES
            ('00000000-0000-0000-0000-0000000000b1', 'Claw', 'Item');
    "#;

    async fn season_rows(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        now: chrono::DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<sqlx::postgres::PgRow>, sqlx::Error> {
        sqlx::query(sqlx::AssertSqlSafe(SEASON_REMINDER_FACTS_SQL.replace("public.", "pg_temp.")))
            .bind(now)
            .bind(7 * 24 * 60 * 60_i64)
            .bind(limit)
            .fetch_all(&mut **tx)
            .await
    }

    /// 限额只限制 (冻结边界, 收件人) 组；选中一个收件人后必须取回该组全部 4001 条义务，
    /// 否则提醒会以不完整正文冻结进该组稳定命令，后续扫描又因投递已存在而不再补寄。
    #[tokio::test]
    #[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
    async fn postgres_season_reminder_returns_complete_selected_group()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
        let mut connection = sqlx::PgConnection::connect(&url).await?;
        let mut tx = connection.begin().await?;
        tx.execute(SEASON_TABLES).await?;
        tx.execute(
            "INSERT INTO pg_temp.lots
                SELECT ('00000000-0000-0000-0000-' || lpad(number::text, 12, '0'))::uuid,
                       number, number
                FROM generate_series(1, 4001) AS number;
             INSERT INTO pg_temp.lots
                VALUES ('00000000-0000-0000-0000-000000005000', 5000, 5000);
             INSERT INTO pg_temp.settlement_obligations
                SELECT lot.id, '00000000-0000-0000-0000-0000000000a1', lot.id,
                       '00000000-0000-0000-0000-0000000000b1', 'seller_payout', 1000, 'ready',
                       CASE WHEN lot.lot_number = 5000 THEN 'bob' ELSE 'alice' END, NULL
                FROM pg_temp.lots AS lot;
             INSERT INTO pg_temp.settlement_obligation_fulfillments
                SELECT obligation.id, 'pending'
                FROM pg_temp.settlement_obligations AS obligation;",
        )
        .await?;
        let rows = season_rows(&mut tx, Utc::now(), 1).await?;
        assert_eq!(rows.len(), 4_001, "选中组的全部义务必须一次返回");
        assert!(rows.iter().all(|row| row.get::<String, _>("recipient") == "alice"));
        tx.rollback().await?;
        Ok(())
    }

    /// 未经恢复显式激活的 contingent 退款不是当前欠付：不再产生提醒，也不阻挡退役。
    #[tokio::test]
    #[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
    async fn postgres_season_reminder_excludes_contingent_refund()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
        let mut connection = sqlx::PgConnection::connect(&url).await?;
        let mut tx = connection.begin().await?;
        tx.execute(SEASON_TABLES).await?;
        tx.execute(
            "INSERT INTO pg_temp.lots VALUES
                ('00000000-0000-0000-0000-0000000000c1', 1, 1),
                ('00000000-0000-0000-0000-0000000000c2', 2, 2);
             INSERT INTO pg_temp.settlement_obligations VALUES
                ('00000000-0000-0000-0000-0000000000d1',
                 '00000000-0000-0000-0000-0000000000a1',
                 '00000000-0000-0000-0000-0000000000c1',
                 '00000000-0000-0000-0000-0000000000b1', 'seller_payout', 1000, 'ready',
                 'alice', NULL),
                ('00000000-0000-0000-0000-0000000000d2',
                 '00000000-0000-0000-0000-0000000000a1',
                 '00000000-0000-0000-0000-0000000000c2',
                 '00000000-0000-0000-0000-0000000000b1', 'buyer_refund', 500, 'contingent',
                 'alice', NULL);
             INSERT INTO pg_temp.settlement_obligation_fulfillments VALUES
                ('00000000-0000-0000-0000-0000000000d1', 'pending'),
                ('00000000-0000-0000-0000-0000000000d2', 'pending');",
        )
        .await?;
        let rows = season_rows(&mut tx, Utc::now(), 512).await?;
        assert_eq!(rows.len(), 1, "只有已激活义务构成提醒");
        assert_eq!(rows[0].get::<String, _>("obligation_kind"), "seller_payout");
        tx.rollback().await?;
        Ok(())
    }
}
