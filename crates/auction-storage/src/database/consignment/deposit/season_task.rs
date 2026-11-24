//! 季窗首次冻结与季前通知投递的 PostgreSQL 写入。

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, SeasonNoticeDelivery, SeasonNoticeDeliveryCommand,
    SeasonNoticeTarget, SeasonTaskStore,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::{CalendarVersion, FrozenSeason};
use auction_observability::DatabaseTransactionOwner;
use sqlx::Row;
use uuid::Uuid;

use super::primitives::database_now;
use super::repository::DepositRepository;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::delivery::MailDeliveryTransactionError;
use crate::database::mail::delivery::enqueue_in_transaction_with_id;

impl SeasonTaskStore for DepositRepository {
    fn freeze_season_window<'a>(
        &'a self,
        season: FrozenSeason,
        timezone: &'a str,
    ) -> ConsignmentFuture<'a, FrozenSeason> {
        Box::pin(async move {
            // 不变量 10：同一 (world, season_id) 以首次提交为准；已有行原样返回，不因
            // calendar_version 或预停天数变化改写界值。
            if season.world != SettlementWorld::Isekai
                || !auction_domain::valid_text(&season.season_id, 128)
                || !auction_domain::valid_text(timezone, 64)
            {
                return Err(ConsignmentError::Unavailable);
            }
            sqlx::query(
                "INSERT INTO public.consignment_season_windows
                    (id, season_id, world_id, timezone, start_utc, end_utc,
                     admission_cutoff_utc, calendar_version, preclose_days)
                 VALUES ($1, $2, 'isekai', $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (world_id, season_id) DO NOTHING",
            )
            .bind(Uuid::new_v4())
            .bind(&season.season_id)
            .bind(timezone)
            .bind(season.start_utc)
            .bind(season.end_utc)
            .bind(season.admission_cutoff_utc)
            .bind(season.calendar_version.as_bytes().as_slice())
            .bind(i64::from(season.preclose_days))
            .execute(self.store().pool())
            .await
            .map_err(storage_error)?;
            let row = sqlx::query(
                "SELECT season_id, start_utc, end_utc, admission_cutoff_utc,
                        calendar_version, preclose_days
                 FROM public.consignment_season_windows
                 WHERE world_id = 'isekai' AND season_id = $1",
            )
            .bind(&season.season_id)
            .fetch_one(self.store().pool())
            .await
            .map_err(storage_error)?;
            let stored = season_from_row(row).map_err(WriteError::into_application)?;
            Ok(stored)
        })
    }

    fn frozen_seasons(&self) -> ConsignmentFuture<'_, Vec<FrozenSeason>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT season_id, start_utc, end_utc, admission_cutoff_utc,
                        calendar_version, preclose_days
                 FROM public.consignment_season_windows
                 WHERE world_id = 'isekai'
                 ORDER BY end_utc, season_id",
            )
            .fetch_all(self.store().pool())
            .await
            .map_err(storage_error)?;
            rows.into_iter()
                .map(season_from_row)
                .collect::<Result<Vec<_>, WriteError>>()
                .map_err(WriteError::into_application)
        })
    }

    fn season_notice_targets<'a>(
        &'a self,
        season: &'a FrozenSeason,
    ) -> ConsignmentFuture<'a, Vec<SeasonNoticeTarget>> {
        Box::pin(async move {
            if season.world != SettlementWorld::Isekai {
                return Ok(Vec::new());
            }
            // 不变量 10：到期判定读数据库墙钟与冻结行，内存定时器只负责唤醒。
            let rows = sqlx::query(
                "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
                 SELECT position.id, position.seller_user_id
                 FROM public.consignment_custody_positions AS position
                 JOIN public.consignment_season_windows AS season_window
                   ON season_window.id = position.season_window_id
                 CROSS JOIN sql_clock
                 WHERE season_window.world_id = 'isekai'
                   AND season_window.season_id = $1
                   AND season_window.admission_cutoff_utc <= sql_clock.at
                   AND sql_clock.at < season_window.end_utc
                   AND position.state NOT IN ('handed_over', 'season_ended', 'lost')
                   AND NOT EXISTS (
                       SELECT 1 FROM public.consignment_season_notices AS notice
                       WHERE notice.position_id = position.id
                         AND notice.seller_user_id = position.seller_user_id
                         AND notice.kind = 'preclose_withdrawal')
                 ORDER BY position.id",
            )
            .bind(&season.season_id)
            .fetch_all(self.store().pool())
            .await
            .map_err(storage_error)?;
            rows.into_iter()
                .map(|row| {
                    Ok(SeasonNoticeTarget {
                        position_id: row.try_get("id")?,
                        seller_user_id: auction_domain::UserId::from_uuid(
                            row.try_get("seller_user_id")?,
                        ),
                    })
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()
                .map_err(WriteError::from)
                .map_err(WriteError::into_application)
        })
    }

    fn season_notice_deliveries<'a>(
        &'a self,
        season: &'a FrozenSeason,
    ) -> ConsignmentFuture<'a, Vec<SeasonNoticeDelivery>> {
        Box::pin(async move {
            if season.world != SettlementWorld::Isekai {
                return Ok(Vec::new());
            }
            let rows = sqlx::query(
                "SELECT notice.id, notice.position_id, notice.seller_user_id, users.username
                 FROM public.consignment_season_notices AS notice
                 JOIN public.consignment_season_windows AS season_window
                   ON season_window.id = notice.season_window_id
                 JOIN public.users AS users ON users.id = notice.seller_user_id
                 WHERE season_window.world_id = 'isekai'
                   AND season_window.season_id = $1
                   AND notice.kind = 'preclose_withdrawal'
                   AND NOT EXISTS (
                       SELECT 1 FROM public.mail_disposition_work AS work
                       WHERE work.notice_id = notice.id)
                 ORDER BY notice.id",
            )
            .bind(&season.season_id)
            .fetch_all(self.store().pool())
            .await
            .map_err(storage_error)?;
            rows.into_iter()
                .map(|row| {
                    Ok(SeasonNoticeDelivery {
                        notice_id: row.try_get("id")?,
                        position_id: row.try_get("position_id")?,
                        seller_user_id: auction_domain::UserId::from_uuid(
                            row.try_get("seller_user_id")?,
                        ),
                        seller_name: row.try_get("username")?,
                    })
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()
                .map_err(WriteError::from)
                .map_err(WriteError::into_application)
        })
    }

    fn create_season_notice_delivery(
        &self,
        command: SeasonNoticeDeliveryCommand,
    ) -> ConsignmentFuture<'_, ()> {
        Box::pin(async move {
            let SeasonNoticeDeliveryCommand { delivery, notice_id, work_id } = command;
            if notice_id.is_nil()
                || delivery.command_id.as_uuid()
                    != auction_application::consignment::season_notice_delivery_id(notice_id)
                || work_id != auction_application::consignment::season_notice_work_id(notice_id)
            {
                return Err(ConsignmentError::Unavailable);
            }
            let delivery_id = delivery.command_id.as_uuid();
            let pool = self.store().pool();
            // 不变量 3、5：通知、工作与投递的关联只在本地同一顶层事务内提交；重复调用按
            // 派生 ID 幂等，通知已提交而投递失败时下一轮可安全重试。
            pool.observe_transaction(DatabaseTransactionOwner::DeliveryOutcome, async {
                let mut transaction = pool.begin().await?;
                let stored = enqueue_in_transaction_with_id(
                    &mut transaction,
                    &delivery,
                    None,
                    Some(self.store().notification_metrics()),
                    delivery_id,
                )
                .await
                .map_err(delivery_error)?;
                if stored.delivery_id != delivery_id {
                    return Err(WriteError::invariant("consignment_notice_delivery_id"));
                }
                let now = database_now(&mut transaction).await?;
                sqlx::query(
                    "INSERT INTO public.mail_disposition_work
                        (id, notice_id, action, delivery_id, created_at)
                     VALUES ($1, $2, 'send', $3, $4)
                     ON CONFLICT DO NOTHING",
                )
                .bind(work_id)
                .bind(notice_id)
                .bind(delivery_id)
                .bind(now)
                .execute(&mut *transaction)
                .await?;
                let stored_work: Option<(Uuid, String, Uuid)> = sqlx::query_as(
                    "SELECT id, action, delivery_id
                     FROM public.mail_disposition_work WHERE notice_id = $1",
                )
                .bind(notice_id)
                .fetch_optional(&mut *transaction)
                .await?;
                let Some((stored_work_id, action, stored_delivery_id)) = stored_work else {
                    return Err(WriteError::invariant("consignment_notice_work_missing"));
                };
                if stored_work_id != work_id
                    || action != "send"
                    || stored_delivery_id != delivery_id
                {
                    return Err(WriteError::invariant("consignment_notice_work_mismatch"));
                }
                transaction.commit().await?;
                Ok(())
            })
            .await
            .map_err(WriteError::into_application)
        })
    }

    fn season_close_pending<'a>(&'a self, season: &'a FrozenSeason) -> ConsignmentFuture<'a, bool> {
        Box::pin(async move {
            if season.world != SettlementWorld::Isekai {
                return Ok(false);
            }
            let pending: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1
                    FROM public.consignment_listings AS listing
                    JOIN public.consignment_season_windows AS season_window
                      ON season_window.world_id = listing.world_id
                    WHERE season_window.world_id = 'isekai'
                      AND season_window.season_id = $1
                      AND season_window.end_utc <= clock_timestamp()
                      AND listing.created_at < season_window.end_utc
                      AND listing.state IN (
                          'resolving', 'displayed', 'awaiting_deposit', 'deposit_accepted',
                          'available', 'held_off_market', 'withdrawal_pending'))",
            )
            .bind(&season.season_id)
            .fetch_one(self.store().pool())
            .await
            .map_err(storage_error)?;
            Ok(pending)
        })
    }
}

fn storage_error(error: sqlx::Error) -> ConsignmentError {
    WriteError::from(error).into_application()
}

fn season_from_row(row: sqlx::postgres::PgRow) -> Result<FrozenSeason, WriteError> {
    let calendar: Vec<u8> = row.try_get("calendar_version")?;
    let calendar: [u8; 32] = match calendar.try_into() {
        Ok(calendar) => calendar,
        Err(error) => {
            tracing::warn!(length = error.len(), "寄售季窗日历摘要长度损坏");
            return Err(WriteError::invariant("consignment_season_calendar"));
        }
    };
    let preclose_days: i16 = row.try_get("preclose_days")?;
    let preclose_days = u8::try_from(preclose_days)?;
    Ok(FrozenSeason {
        world: SettlementWorld::Isekai,
        season_id: row.try_get("season_id")?,
        start_utc: row.try_get("start_utc")?,
        end_utc: row.try_get("end_utc")?,
        admission_cutoff_utc: row.try_get("admission_cutoff_utc")?,
        calendar_version: CalendarVersion::from_bytes(calendar),
        preclose_days,
    })
}

fn delivery_error(error: MailDeliveryTransactionError) -> WriteError {
    match error {
        MailDeliveryTransactionError::Sqlx(error) => WriteError::from(error),
        MailDeliveryTransactionError::Rejected(_) => {
            WriteError::invariant("consignment_notice_delivery_rejected")
        }
    }
}
