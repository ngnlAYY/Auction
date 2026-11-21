//! 失效租约的回收候选、让出执行权与原围栏清理。

use std::time::Duration;

use auction_application::consignment::{
    LookupAbandonment, LookupFence, LookupLeaseUpdate, LookupTransientReason,
};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::admission::db_now;
use super::control::{LookupControl, lock_control};
use super::protocol::{LEASE_GRACE, MAX_LISTING_LOOKUP_ATTEMPTS, retry_delay};
use super::store::{ReapCommands, transact};
use super::terminal;
use super::work::{LookupFenceCheck, check_lookup_fence};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::eligibility::predicates::listing_sql;

const REAP_LIMIT: i64 = 16;

#[derive(Clone, Copy)]
pub(super) enum ReclaimMode {
    Expired,
    Abandon(LookupAbandonment),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalEvidence {
    Deadline(Timestamp),
    Exhausted,
}

pub(super) enum WorkCompletion {
    Queued(Duration),
    Cancelled,
    Done,
}

enum Reclaimed {
    Lost,
    Applied,
    Terminal(TerminalEvidence),
}

pub(super) async fn reap_expired(
    store: &DatabaseStore,
    commands: &ReapCommands,
) -> Result<u32, WriteError> {
    // 停用、碰撞和失资格由公平补扫清退。领取/自动清退可以在本查询与行锁之间推进工作。
    let rows: Vec<(Uuid, Uuid, i64, i64, Uuid)> = sqlx::query_as(concat!(
        "WITH db_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         SELECT w.id, w.listing_id, w.generation, w.fence, w.token
         FROM public.consignment_lookup_work w JOIN public.consignment_listings l ON l.id = w.listing_id
         CROSS JOIN db_clock",
        listing_sql!(eligibility_joins),
        "WHERE w.state = 'leased' AND w.lease_until <= at AND l.state = 'resolving'
           AND ", listing_sql!(admitting_epoch),
        " AND ", listing_sql!(listing_seller_eligible),
        " AND NOT ", listing_sql!(collision),
        " ORDER BY w.lease_until, w.id LIMIT $1",
    ))
    .bind(REAP_LIMIT)
    .fetch_all(store.pool())
    .await?;
    let mut processed = 0;
    for (work_id, listing_id, generation, fence, token) in rows {
        let fence =
            LookupFence::new(work_id, listing_id, generation.cast_unsigned(), fence, token)?;
        if reclaim(store, commands, fence, ReclaimMode::Expired).await? {
            processed += 1;
        }
    }
    Ok(processed)
}

pub(super) async fn abandon(
    store: &DatabaseStore,
    commands: &ReapCommands,
    fence: LookupFence,
    status: LookupAbandonment,
) -> Result<LookupLeaseUpdate<()>, WriteError> {
    Ok(if reclaim(store, commands, fence, ReclaimMode::Abandon(status)).await? {
        LookupLeaseUpdate::Applied(())
    } else {
        LookupLeaseUpdate::Lost
    })
}

async fn reclaim(
    store: &DatabaseStore,
    commands: &ReapCommands,
    fence: LookupFence,
    mode: ReclaimMode,
) -> Result<bool, WriteError> {
    match transact(store, &(fence, mode), |conn, &(fence, mode)| {
        Box::pin(reclaim_work(conn, fence, mode))
    })
    .await?
    {
        Reclaimed::Lost => Ok(false),
        Reclaimed::Applied => Ok(true),
        Reclaimed::Terminal(evidence) => {
            terminal::finish(store, commands, fence, mode, evidence).await
        }
    }
}

async fn reclaim_work(
    conn: &mut PgConnection,
    fence: LookupFence,
    mode: ReclaimMode,
) -> Result<Reclaimed, WriteError> {
    let control = lock_control(conn).await?;
    let Some(work) = lock_reclaimable(conn, fence, mode).await? else {
        return Ok(Reclaimed::Lost);
    };
    let at = db_now(conn).await?;
    let (forced, expires_at) = read_resolution(conn, fence, at).await?;
    if let Some(evidence) = terminal_evidence(work.attempt, at, forced.as_deref(), expires_at) {
        // 不变量 2、3：释放短事务锁后，终结入口按资格、挂单、单例、工作顺序重新验证。
        return Ok(Reclaimed::Terminal(evidence));
    }
    let completion = if forced.is_some() {
        match mode {
            // 到期回收不能抢先另一条更高优先级的自动清退。
            ReclaimMode::Expired => return Ok(Reclaimed::Lost),
            ReclaimMode::Abandon(_) => WorkCompletion::Cancelled,
        }
    } else {
        let delay = retry_delay(work.attempt)
            .ok_or_else(|| WriteError::invariant("consignment_lookup_reclaim_attempt"))?;
        let delay = if matches!(mode, ReclaimMode::Abandon(LookupAbandonment::NotSent)) {
            // 确认未发送时只让出许可，仍保留领取计次和全局起始间隔。
            Duration::ZERO
        } else {
            delay
        };
        WorkCompletion::Queued(delay)
    };
    finish_work(conn, &control, fence, mode, work, at, completion).await?;
    Ok(Reclaimed::Applied)
}

pub(super) fn terminal_evidence(
    attempt: i16,
    at: Timestamp,
    forced: Option<&str>,
    expires_at: Timestamp,
) -> Option<TerminalEvidence> {
    match forced {
        Some("lookup_deadline") if at >= expires_at => Some(TerminalEvidence::Deadline(expires_at)),
        None if attempt == i16::from(MAX_LISTING_LOOKUP_ATTEMPTS) => {
            Some(TerminalEvidence::Exhausted)
        }
        _ => None,
    }
}

pub(super) async fn read_resolution(
    conn: &mut PgConnection,
    fence: LookupFence,
    at: Timestamp,
) -> Result<(Option<String>, Timestamp), WriteError> {
    Ok(sqlx::query_as(concat!(
        "SELECT CASE WHEN l.state <> 'resolving' THEN 'not_resolving' ELSE ",
        listing_sql!(forced_reason, "$2"),
        " END, l.expires_at FROM public.consignment_listings l",
        listing_sql!(eligibility_joins),
        "WHERE l.id = $1",
    ))
    .bind(fence.listing_id())
    .bind(at)
    .fetch_one(conn)
    .await?)
}

pub(super) struct ReclaimableWork {
    pub(super) attempt: i16,
    deadline: Timestamp,
}

pub(super) async fn lock_reclaimable(
    conn: &mut PgConnection,
    fence: LookupFence,
    mode: ReclaimMode,
) -> Result<Option<ReclaimableWork>, WriteError> {
    // 对让出执行权，仍要求活租约和原许可。到期回收可以在全局许可已换手后收拾旧工作。
    if matches!(mode, ReclaimMode::Abandon(_))
        && !matches!(check_lookup_fence(conn, fence).await?, LookupFenceCheck::Matched(_))
    {
        return Ok(None);
    }
    let row: Option<(i16, Timestamp)> = sqlx::query_as(
        "SELECT attempt_count, network_deadline FROM public.consignment_lookup_work
         WHERE id = $1 AND listing_id = $2 AND generation = $3 AND fence = $4 AND token = $5
           AND state = 'leased' AND (lease_until <= clock_timestamp()) = $6
         FOR UPDATE",
    )
    .bind(fence.work_id())
    .bind(fence.listing_id())
    .bind(fence.generation().cast_signed())
    .bind(fence.fence())
    .bind(fence.token())
    .bind(matches!(mode, ReclaimMode::Expired))
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(attempt, deadline)| ReclaimableWork { attempt, deadline }))
}

/// 在单例和原工作行锁内结束本次租约，同时保留仍可能在途的时间栅栏。
pub(super) async fn finish_work(
    conn: &mut PgConnection,
    control: &LookupControl,
    fence: LookupFence,
    mode: ReclaimMode,
    work: ReclaimableWork,
    at: Timestamp,
    completion: WorkCompletion,
) -> Result<(), WriteError> {
    // 不变量 2、5、6：仅更新原围栏；已用次数与未来 fence 不变。回收无上游响应，不能写冷却。
    let reason = matches!(mode, ReclaimMode::Expired)
        .then_some(LookupTransientReason::SourceTimeout.as_str());
    let (state, delay) = match completion {
        WorkCompletion::Queued(delay) => ("queued", Some(delay.as_secs_f64())),
        WorkCompletion::Cancelled => ("cancelled", None),
        WorkCompletion::Done => ("done", None),
    };
    let changed = sqlx::query(
        "UPDATE public.consignment_lookup_work
         SET state = $6, next_attempt_at = $7 + make_interval(secs => $8),
             lease_owner = NULL, token = NULL, lease_until = NULL, network_deadline = NULL,
             last_reason = COALESCE($9, last_reason),
             completed_at = CASE WHEN $6 <> 'queued' THEN $7 END, updated_at = $7
         WHERE id = $1 AND listing_id = $2 AND generation = $3 AND fence = $4 AND token = $5
           AND state = 'leased'",
    )
    .bind(fence.work_id())
    .bind(fence.listing_id())
    .bind(fence.generation().cast_signed())
    .bind(fence.fence())
    .bind(fence.token())
    .bind(state)
    .bind(at)
    .bind(delay)
    .bind(reason)
    .execute(&mut *conn)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_lookup_reclaim_fence"));
    }
    if control.holder_work_id != Some(fence.work_id())
        || control.holder_fence != Some(fence.fence())
        || control.holder_token != Some(fence.token())
    {
        return Ok(());
    }
    let keep_time_fence = matches!(
        mode,
        ReclaimMode::Expired | ReclaimMode::Abandon(LookupAbandonment::MayBeInFlight)
    );
    sqlx::query(
        "UPDATE public.consignment_lookup_control
         SET holder_work_id = CASE WHEN keep THEN holder_work_id END,
             holder_fence = CASE WHEN keep THEN holder_fence END,
             holder_token = CASE WHEN keep THEN holder_token END,
             permit_until = CASE WHEN keep THEN until_at END
         FROM (
             SELECT $1 AND until_at > $2 AS keep, until_at FROM (
                 SELECT GREATEST(permit_until, $3 + make_interval(secs => $4)) AS until_at
                 FROM public.consignment_lookup_control WHERE id = 1
             ) deadline
         ) retained
         WHERE id = 1",
    )
    .bind(keep_time_fence)
    .bind(at)
    .bind(work.deadline)
    .bind(LEASE_GRACE.as_secs_f64())
    .execute(conn)
    .await?;
    Ok(())
}
