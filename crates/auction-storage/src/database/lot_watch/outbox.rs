//! W12.1 残留：被超提醒与自动标记的持久化提交后 outbox。
//!
//! 出价 Serializable 事务只向 `lot_watch_events` 追加一条身份确定的事件行；自动标记、
//! 领先周期、待定提醒与提醒入队全部移到提交后的独立事务，由 [`process_committed_events`]
//! 或 Worker 的 [`LotWatchReminderStore::process_due`] 应用。事件与完成标记同一事务提交，
//! 崩溃只会重试；同一 lot 的竞价事件按 `state_version` 顺序领取，因此仍由既有追加式事实
//! 决定领先周期和提醒身份，不建立第二套业务状态机。

use auction_application::auction::ApplicationFuture;
use auction_application::lot_watch_reminders::{LotWatchReminderBatch, LotWatchReminderStore};
use auction_domain::{AcceptedBid, BidAmount, BidId, BidMechanism, CommandId, LotId, UserId};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use self::parent::*;
use super as parent;
use crate::database::TrackedTransaction as Transaction;
use crate::database::{DatabaseStore, PostgresStore, storage_error, storage_error_from};

/// 领取租约的上下限；只决定崩溃后多久可回收，不改变事件顺序。
pub(crate) const MIN_LEASE_SECONDS: u32 = 5;
pub(crate) const MAX_LEASE_SECONDS: u32 = 600;
/// 单次处理的硬上限；调用方按自身批量预算收缩。
const MAX_BATCH: u16 = 1_000;

fn bid_event_id(bid_id: Uuid, lot_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/lot-watch-event-v1/bid/{lot_id}/{bid_id}").as_bytes(),
    )
}

fn proxy_event_id(authorization_id: Uuid, lot_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/lot-watch-event-v1/proxy/{lot_id}/{authorization_id}").as_bytes(),
    )
}

/// 返回竞价事实对应的事件 ID；调用方在同一事务内用它插入事件行并登记提交后处理。
pub(crate) fn bid_event(bid: &AcceptedBid) -> Uuid {
    bid_event_id(bid.id.as_uuid(), bid.lot_id.as_uuid())
}

/// 返回新建代理对应的事件 ID；同一授权重复创建不会产生第二条事件。
pub(crate) fn proxy_event(authorization_id: Uuid, lot_id: Uuid) -> Uuid {
    proxy_event_id(authorization_id, lot_id)
}

/// 出价事务内只追加事件：不读标记、不写领先周期、不触发投递唤醒。
pub(crate) async fn insert_bid_event(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    bid: &AcceptedBid,
    occurred_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    sqlx::query(
        "INSERT INTO public.lot_watch_events
            (event_id, event_kind, lot_id, user_id, bid_id, state_version,
             occurred_at, next_attempt_at)
         VALUES ($1, 'bid_accepted', $2, $3, $4, $5, $6, $6)
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(event_id)
    .bind(bid.lot_id.as_uuid())
    .bind(bid.bidder_id.as_uuid())
    .bind(bid.id.as_uuid())
    .bind(i64::try_from(bid.state_version).map_err(|error| {
        let _ = error;
        CompetitionTransactionError::Failed("标记事件状态版本超出范围".to_owned())
    })?)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?;
    Ok(())
}

/// 代理创建事务内只追加标记事件；真正的自动标记由提交后处理完成。
pub(crate) async fn insert_proxy_event(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    authorization_id: Uuid,
    user_id: Uuid,
    lot_id: Uuid,
    state_version: u64,
    occurred_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    sqlx::query(
        "INSERT INTO public.lot_watch_events
            (event_id, event_kind, lot_id, user_id, authorization_id, state_version,
             occurred_at, next_attempt_at)
         VALUES ($1, 'proxy_created', $2, $3, $4, $5, $6, $6)
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(event_id)
    .bind(lot_id)
    .bind(user_id)
    .bind(authorization_id)
    .bind(i64::try_from(state_version).map_err(|error| {
        let _ = error;
        CompetitionTransactionError::Failed("标记事件状态版本超出范围".to_owned())
    })?)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?;
    Ok(())
}

/// 提交后的同进程处理：只处理本次提交事件所属 lot 的最早待处理事件。
///
/// 这是尽力而为的及时性路径；失败不会改变已经提交的出价结果，事件仍由 Worker 扫描重试。
pub(crate) async fn process_committed_events(store: &PostgresStore, event_ids: &[Uuid]) {
    if event_ids.is_empty() {
        return;
    }
    let lots: Result<Vec<Uuid>, _> = sqlx::query_scalar(
        "SELECT DISTINCT lot_id FROM public.lot_watch_events WHERE event_id = ANY($1)",
    )
    .bind(event_ids)
    .fetch_all(store.pool())
    .await;
    let lots = match lots {
        Ok(lots) => lots,
        Err(error) => {
            let context = crate::database::sql_error_context(&error, "读取标记事件 lot 失败");
            tracing::warn!(error = %context, "lot watch post-commit drain skipped");
            return;
        }
    };
    if lots.is_empty() {
        return;
    }
    // 预算包含少量余量，用于消化本 lot 更早的待处理事件；其余由 Worker 继续。
    let limit =
        u16::try_from(event_ids.len()).unwrap_or(MAX_BATCH).saturating_add(4).min(MAX_BATCH);
    // 每次提交后处理使用独立租约身份，便于区分并发请求与崩溃遗留的 claim。
    let owner = format!("bid-post-commit-{}", Uuid::new_v4());
    if let Err(error) = process_batch(store, &owner, 60, Some(&lots), limit).await {
        tracing::warn!(error = %error, "lot watch post-commit drain failed; worker will retry");
    }
}

impl LotWatchReminderStore for DatabaseStore {
    fn process_due(
        &self,
        worker_id: String,
        lease_seconds: u32,
        limit: u16,
    ) -> ApplicationFuture<'_, LotWatchReminderBatch> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id.len() > 128
                || worker_id.trim() != worker_id
                || worker_id.chars().any(char::is_control)
                || !(MIN_LEASE_SECONDS..=MAX_LEASE_SECONDS).contains(&lease_seconds)
                || !(1..=MAX_BATCH).contains(&limit)
            {
                return Err(storage_error("标记事件处理参数无效"));
            }
            let Self::Postgres(store) = self;
            process_batch(store, &worker_id, lease_seconds, None, limit).await
        })
    }
}

#[derive(Debug)]
struct ClaimedEvent {
    event_id: Uuid,
    event_kind: String,
    lot_id: Uuid,
    user_id: Uuid,
    bid_id: Option<Uuid>,
    occurred_at: DateTime<Utc>,
}

async fn process_batch(
    store: &PostgresStore,
    worker_id: &str,
    lease_seconds: u32,
    lots: Option<&[Uuid]>,
    limit: u16,
) -> Result<LotWatchReminderBatch, auction_application::core::ApplicationError> {
    let lot_filter = lots.map(<[Uuid]>::to_vec);
    let mut batch = LotWatchReminderBatch::default();
    // 每次领取对每个 lot 至多一条（顺序屏障），因此循环领取直到用完预算或没有到期事件；
    // 停机期间同一 lot 的多笔出价不会退化成每轮只推进一笔。
    while batch.claimed < limit {
        let remaining = limit - batch.claimed;
        let claimed =
            claim_events(store, worker_id, lease_seconds, lot_filter.as_deref(), remaining).await?;
        if claimed.is_empty() {
            break;
        }
        batch.claimed =
            batch.claimed.saturating_add(u16::try_from(claimed.len()).unwrap_or(u16::MAX));
        for event in claimed {
            match process_claimed(store, &event, worker_id).await {
                Ok(queued_delivery) => {
                    batch.completed = batch.completed.saturating_add(1);
                    // 通知在投递入队事务提交之后单独发送，不延长提交的 NOTIFY 锁；
                    // 发送失败只降低及时性，Worker 的轮询扫描仍会领取已提交投递。
                    if queued_delivery
                        && let Err(error) =
                            crate::database::delivery_scheduler::publish_after_commit(store.pool())
                                .await
                    {
                        tracing::warn!(
                            event_id = %event.event_id,
                            lot_id = %event.lot_id,
                            error = %error,
                            "delivery wakeup after commit failed; polling remains active"
                        );
                    }
                }
                Err(error) => {
                    batch.retried = batch.retried.saturating_add(1);
                    release_claim(store, &event.event_id, worker_id, error).await;
                    tracing::warn!(
                        event_id = %event.event_id,
                        lot_id = %event.lot_id,
                        error_class = error,
                        "lot watch event processing failed; scheduled for retry"
                    );
                }
            }
        }
    }
    Ok(batch)
}

async fn claim_events(
    store: &PostgresStore,
    worker_id: &str,
    lease_seconds: u32,
    lots: Option<&[Uuid]>,
    limit: u16,
) -> Result<Vec<ClaimedEvent>, auction_application::core::ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
        .await
        .map_err(|error| storage_error_from(error, "开启标记事件领取事务失败"))?;
    let rows = sqlx::query(
        r#"UPDATE public.lot_watch_events AS event
           SET status = 'processing',
               lease_owner = $1,
               lease_expires_at = clock_timestamp() + make_interval(secs => $2),
               attempts = event.attempts + 1,
               last_error_code = NULL
           FROM (
               SELECT candidate.event_id
               FROM public.lot_watch_events AS candidate
               WHERE (
                       (
                           candidate.status = 'pending'
                           AND candidate.next_attempt_at <= clock_timestamp()
                       ) OR (
                           candidate.status = 'processing'
                           AND candidate.lease_expires_at <= clock_timestamp()
                       )
                     )
                 AND ($3::uuid[] IS NULL OR candidate.lot_id = ANY($3::uuid[]))
                 AND (
                       candidate.event_kind = 'proxy_created'
                       OR NOT EXISTS (
                           SELECT 1
                           FROM public.lot_watch_events AS prior
                           WHERE prior.lot_id = candidate.lot_id
                             AND prior.event_kind = 'bid_accepted'
                             AND prior.status <> 'done'
                             AND (prior.state_version, prior.event_id)
                                 < (candidate.state_version, candidate.event_id)
                       )
                 )
               ORDER BY candidate.next_attempt_at, candidate.occurred_at, candidate.event_id
               LIMIT $4
               FOR UPDATE SKIP LOCKED
           ) AS due
           WHERE event.event_id = due.event_id
           RETURNING event.event_id, event.event_kind, event.lot_id, event.user_id,
                     event.bid_id, event.occurred_at"#,
    )
    .bind(worker_id)
    .bind(f64::from(lease_seconds))
    .bind(lots.map(<[Uuid]>::to_vec))
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "领取标记事件失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交标记事件领取失败"))?;

    rows.into_iter()
        .map(|row| {
            Ok(ClaimedEvent {
                event_id: row.try_get("event_id").map_err(storage_row_error)?,
                event_kind: row.try_get("event_kind").map_err(storage_row_error)?,
                lot_id: row.try_get("lot_id").map_err(storage_row_error)?,
                user_id: row.try_get("user_id").map_err(storage_row_error)?,
                bid_id: row.try_get("bid_id").map_err(storage_row_error)?,
                occurred_at: row.try_get("occurred_at").map_err(storage_row_error)?,
            })
        })
        .collect()
}

fn storage_row_error(error: sqlx::Error) -> auction_application::core::ApplicationError {
    storage_error_from(error, "读取标记事件字段失败")
}

/// 处理单条事件；处理事务与 `status = 'done'` 同一事务提交。
/// 返回提交事务是否入队了新的 `lot_watch` 投递；调用方在提交后发送唤醒。
async fn process_claimed(
    store: &PostgresStore,
    event: &ClaimedEvent,
    worker_id: &str,
) -> Result<bool, &'static str> {
    let mut transaction =
        store.pool().begin_with("BEGIN ISOLATION LEVEL READ COMMITTED").await.map_err(|error| {
            let _ = error;
            "begin_failed"
        })?;
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.lot_watch_events
         WHERE event_id = $1 AND status = 'processing' AND lease_owner = $2)",
    )
    .bind(event.event_id)
    .bind(worker_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| {
        let _ = error;
        "claim_check_failed"
    })?;
    if !owned {
        let _ = transaction.rollback().await;
        return Ok(false);
    }
    // 即使租约被并发回收，同一 lot 的处理仍按 advisory 锁串行，保证领先周期顺序。
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(event.lot_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            let _ = error;
            "lot_lock_failed"
        })?;

    let notifications = store.notification_metrics();
    let public_base_url = store.public_base_url.clone();
    let mail_templates = store.mail_templates();
    let queued_delivery = match event.event_kind.as_str() {
        "bid_accepted" => {
            let bid_id = event.bid_id.ok_or("missing_bid_identity")?;
            let bid = load_bid(&mut transaction, bid_id, event.lot_id).await?;
            if bid.mechanism == BidMechanism::Ordinary {
                super::facts::auto_watch(
                    &mut transaction,
                    bid.bidder_id.as_uuid(),
                    bid.lot_id.as_uuid(),
                    "lot_watch.bid_created",
                )
                .await
                .map_err(error_class)?;
            }
            super::facts::apply_bid(
                &mut transaction,
                &bid,
                event.occurred_at,
                notifications,
                public_base_url.as_deref(),
                mail_templates,
            )
            .await
            .map_err(error_class)?
        }
        "proxy_created" => {
            super::facts::auto_watch(
                &mut transaction,
                event.user_id,
                event.lot_id,
                "lot_watch.proxy_created",
            )
            .await
            .map_err(error_class)?;
            false
        }
        _ => return Err("unknown_event_kind"),
    };

    let updated = sqlx::query(
        "UPDATE public.lot_watch_events
         SET status = 'done', completed_at = clock_timestamp(),
             lease_owner = NULL, lease_expires_at = NULL, last_error_code = NULL
         WHERE event_id = $1 AND status = 'processing' AND lease_owner = $2",
    )
    .bind(event.event_id)
    .bind(worker_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| {
        let _ = error;
        "complete_failed"
    })?;
    if updated.rows_affected() != 1 {
        let _ = transaction.rollback().await;
        return Err("claim_lost");
    }
    transaction.commit().await.map_err(|error| {
        let _ = error;
        "commit_failed"
    })?;
    Ok(queued_delivery)
}

async fn load_bid(
    transaction: &mut Transaction<'_, Postgres>,
    bid_id: Uuid,
    lot_id: Uuid,
) -> Result<AcceptedBid, &'static str> {
    let row = sqlx::query(
        "SELECT id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
                effective_end_at, accepted_state_version
         FROM public.bids WHERE id = $1 AND lot_id = $2",
    )
    .bind(bid_id)
    .bind(lot_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| {
        let _ = error;
        "load_bid_failed"
    })?
    .ok_or("bid_missing")?;
    let fact_index: i32 = row.try_get("fact_index").map_err(|error| {
        let _ = error;
        "bid_field_invalid"
    })?;
    let state_version: i64 = row.try_get("accepted_state_version").map_err(|error| {
        let _ = error;
        "bid_field_invalid"
    })?;
    let amount: i64 = row.try_get("amount").map_err(|error| {
        let _ = error;
        "bid_field_invalid"
    })?;
    let mechanism: String = row.try_get("mechanism").map_err(|error| {
        let _ = error;
        "bid_field_invalid"
    })?;
    Ok(AcceptedBid {
        id: BidId::from_uuid(row.try_get("id").map_err(|error| {
            let _ = error;
            "bid_field_invalid"
        })?),
        command_id: CommandId::from_uuid(row.try_get("command_id").map_err(|error| {
            let _ = error;
            "bid_field_invalid"
        })?),
        lot_id: LotId::from_uuid(row.try_get("lot_id").map_err(|error| {
            let _ = error;
            "bid_field_invalid"
        })?),
        bidder_id: UserId::from_uuid(row.try_get("bidder_user_id").map_err(|error| {
            let _ = error;
            "bid_field_invalid"
        })?),
        fact_index: u32::try_from(fact_index).map_err(|error| {
            let _ = error;
            "bid_field_invalid"
        })?,
        amount: BidAmount::from_persisted_positive(amount).map_err(|error| {
            let _ = error;
            "bid_field_invalid"
        })?,
        effective_end_at: row.try_get("effective_end_at").map_err(|error| {
            let _ = error;
            "bid_field_invalid"
        })?,
        state_version: u64::try_from(state_version).map_err(|error| {
            let _ = error;
            "bid_field_invalid"
        })?,
        mechanism: BidMechanism::from_name(&mechanism).ok_or("bid_field_invalid")?,
    })
}

fn error_class(error: CompetitionTransactionError) -> &'static str {
    match error {
        CompetitionTransactionError::SerializationConflict => "serialization_conflict",
        CompetitionTransactionError::Conflict => "conflict",
        CompetitionTransactionError::CommandLedger(_) => "command_ledger",
        _ => "transaction_failed",
    }
}

/// 释放失败事件的租约并写入有界退避；事件永不删除。
async fn release_claim(
    store: &PostgresStore,
    event_id: &Uuid,
    worker_id: &str,
    error_class: &'static str,
) {
    if let Err(error) = sqlx::query(
        "UPDATE public.lot_watch_events
         SET status = 'pending', lease_owner = NULL, lease_expires_at = NULL,
             last_error_code = $3,
             next_attempt_at = clock_timestamp()
                 + least(make_interval(secs => power(2, attempts)::int), interval '60 seconds')
         WHERE event_id = $1 AND status = 'processing' AND lease_owner = $2",
    )
    .bind(event_id)
    .bind(worker_id)
    .bind(error_class)
    .execute(store.pool())
    .await
    {
        let context = crate::database::sql_error_context(&error, "释放标记事件租约失败");
        tracing::warn!(error = %context, "lot watch event lease release failed");
    }
}
