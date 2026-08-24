//! 持久化 Worker 调度适配器。
//!
//! interval 只负责唤醒 Worker；due 条件、租约、尝试次数、完成游标和失败原因都由数据库
//! 在明确事务或条件更新中保存。PostgreSQL 使用 `FOR UPDATE SKIP LOCKED`，SQLite 使用
//! `BEGIN IMMEDIATE` 串行领取。

use auction_application::{
    ApplicationError, ScheduledRoundClaim, ScheduledRoundPhase, ScheduledRoundStore,
};
use auction_domain::{AuctionId, CommandId, deterministic_uuid};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
use sqlx::Postgres;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use sqlx::Row;
#[cfg(feature = "sqlite")]
use sqlx::{Sqlite, SqliteConnection};

impl ScheduledRoundStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> auction_application::ApplicationFuture<'_, Vec<ScheduledRoundClaim>> {
        Box::pin(async move {
            if worker_id.is_empty() || worker_id != worker_id.trim() || limit == 0 {
                return Err(repository_error("Worker 调度领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim_postgres(store.pool(), &worker_id, now, lease_until, limit)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => {
                    claim_sqlite(store.pool(), &worker_id, now, lease_until, limit)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn complete(
        &self,
        claim: ScheduledRoundClaim,
        completed_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => complete_postgres(store.pool(), &claim, completed_at)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => complete_sqlite(store.pool(), &claim, completed_at)
                    .await
                    .map_err(ApplicationError::from),
            }
        })
    }

    fn fail(
        &self,
        claim: ScheduledRoundClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 128);
            let safe_detail = bounded_text(safe_detail, 512);
            if failure_code.is_empty() || safe_detail.is_empty() {
                return Err(repository_error("Worker 失败记录不能为空"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    fail_postgres(store.pool(), &claim, retry_at, &failure_code, &safe_detail)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => {
                    fail_sqlite(store.pool(), &claim, retry_at, &failure_code, &safe_detail)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }
}

fn repository_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn command_id(create_command_id: Uuid, phase: ScheduledRoundPhase) -> CommandId {
    let label = match phase {
        ScheduledRoundPhase::Preview => "scheduled-preview",
        ScheduledRoundPhase::Open => "scheduled-open",
        ScheduledRoundPhase::Final => "scheduled-final",
    };
    CommandId::from_uuid(deterministic_uuid(CommandId::from_uuid(create_command_id), label, 1))
}

fn phase_from_completion(preview_completed: bool, open_completed: bool) -> ScheduledRoundPhase {
    if !preview_completed {
        ScheduledRoundPhase::Preview
    } else if !open_completed {
        ScheduledRoundPhase::Open
    } else {
        ScheduledRoundPhase::Final
    }
}

#[cfg(feature = "postgres")]
async fn claim_postgres(
    pool: &super::TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<ScheduledRoundClaim>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let rows = sqlx::query(
        "SELECT round.id, round.auction_id, round.create_command_id,
                round.preview_completed_at, round.open_completed_at, round.attempt_count
         FROM public.scheduled_rounds AS round
         INNER JOIN public.auctions AS auction ON auction.id = round.auction_id
         LEFT JOIN LATERAL (
             SELECT MIN(lot.effective_end_at) AS earliest_end_at
             FROM public.lots AS lot
             WHERE lot.auction_id = round.auction_id AND lot.status = 'live'
         ) AS deadline ON true
         WHERE round.status = 'active'
           AND round.final_completed_at IS NULL
           AND (round.lease_until IS NULL OR round.lease_until <= $1)
           AND (
               (round.preview_completed_at IS NULL AND round.preview_due_at <= $1)
               OR (
                   round.preview_completed_at IS NOT NULL
                   AND round.open_completed_at IS NULL
                   AND round.open_due_at <= $1
               )
               OR (
                   round.open_completed_at IS NOT NULL
                   AND auction.status = 'live'
                   AND deadline.earliest_end_at IS NOT NULL
                   AND deadline.earliest_end_at
                       - ((auction.policy_snapshot ->> 'final_phase_lead_seconds')::double precision
                          * interval '1 second') <= $1
               )
           )
         ORDER BY CASE
             WHEN round.preview_completed_at IS NULL THEN round.preview_due_at
             WHEN round.open_completed_at IS NULL THEN round.open_due_at
             ELSE deadline.earliest_end_at
                 - ((auction.policy_snapshot ->> 'final_phase_lead_seconds')::double precision
                    * interval '1 second')
         END, round.id
         LIMIT $2
         FOR UPDATE OF round SKIP LOCKED",
    )
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let round_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
        let auction_id =
            AuctionId::from_uuid(row.try_get("auction_id").map_err(map_storage_error)?);
        let create_command_id: Uuid =
            row.try_get("create_command_id").map_err(map_storage_error)?;
        let preview_completed_at: Option<DateTime<Utc>> =
            row.try_get("preview_completed_at").map_err(map_storage_error)?;
        let open_completed_at: Option<DateTime<Utc>> =
            row.try_get("open_completed_at").map_err(map_storage_error)?;
        let phase =
            phase_from_completion(preview_completed_at.is_some(), open_completed_at.is_some());
        let old_attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
        let attempt_count = old_attempt_count
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| repository_error_owned("Worker 尝试次数溢出"))?;
        let updated = sqlx::query(
            "UPDATE public.scheduled_rounds
             SET lease_owner = $2, lease_until = $3, attempt_count = $4,
                 state_version = state_version + 1,
                 last_failure_code = NULL, last_failure_detail = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND status = 'active' AND final_completed_at IS NULL",
        )
        .bind(round_id)
        .bind(worker_id)
        .bind(lease_until)
        .bind(
            i32::try_from(attempt_count)
                .map_err(|_| repository_error_owned("Worker 尝试次数超出数据库范围"))?,
        )
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(repository_error_owned("Worker 领取轮次时状态已变化"));
        }
        claims.push(ScheduledRoundClaim {
            round_id,
            auction_id,
            phase,
            command_id: command_id(create_command_id, phase),
            worker_id: worker_id.to_owned(),
            lease_until,
            attempt_count,
        });
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}

#[cfg(feature = "postgres")]
async fn complete_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &ScheduledRoundClaim,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let (sql, phase) = match claim.phase {
        ScheduledRoundPhase::Preview => (
            "UPDATE public.scheduled_rounds
             SET preview_command_id = $3, preview_completed_at = $4,
                 state_version = state_version + 1,
                 lease_owner = NULL, lease_until = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND status = 'active' AND lease_owner = $2
               AND preview_completed_at IS NULL AND final_completed_at IS NULL",
            "preview",
        ),
        ScheduledRoundPhase::Open => (
            "UPDATE public.scheduled_rounds
             SET open_command_id = $3, open_completed_at = $4,
                 state_version = state_version + 1,
                 lease_owner = NULL, lease_until = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL
                   AND final_completed_at IS NULL",
            "open",
        ),
        ScheduledRoundPhase::Final => (
            "UPDATE public.scheduled_rounds
             SET final_command_id = $3, final_completed_at = $4,
                 state_version = state_version + 1,
                 lease_owner = NULL, lease_until = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NOT NULL
                   AND final_completed_at IS NULL",
            "final",
        ),
    };
    let result = sqlx::query(sql)
        .bind(claim.round_id)
        .bind(&claim.worker_id)
        .bind(claim.command_id.as_uuid())
        .bind(completed_at)
        .execute(pool)
        .await
        .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned(&format!("Worker {phase} 阶段完成租约已失效")));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn fail_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &ScheduledRoundClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    safe_detail: &str,
) -> Result<(), auction_application::RepositoryError> {
    let result = match claim.phase {
        ScheduledRoundPhase::Preview => {
            sqlx::query(
                "UPDATE public.scheduled_rounds
                 SET lease_owner = NULL, lease_until = $3, last_failure_code = $4,
                     state_version = state_version + 1,
                     last_failure_detail = $5, updated_at = CURRENT_TIMESTAMP
                 WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND preview_completed_at IS NULL AND final_completed_at IS NULL",
            )
            .bind(claim.round_id)
            .bind(&claim.worker_id)
            .bind(retry_at)
            .bind(failure_code)
            .bind(safe_detail)
            .execute(pool)
            .await
        }
        ScheduledRoundPhase::Open => {
            sqlx::query(
                "UPDATE public.scheduled_rounds
                 SET lease_owner = NULL, lease_until = $3, last_failure_code = $4,
                     state_version = state_version + 1,
                     last_failure_detail = $5, updated_at = CURRENT_TIMESTAMP
                 WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL
                   AND final_completed_at IS NULL",
            )
            .bind(claim.round_id)
            .bind(&claim.worker_id)
            .bind(retry_at)
            .bind(failure_code)
            .bind(safe_detail)
            .execute(pool)
            .await
        }
        ScheduledRoundPhase::Final => {
            sqlx::query(
                "UPDATE public.scheduled_rounds
                 SET lease_owner = NULL, lease_until = $3, last_failure_code = $4,
                     state_version = state_version + 1,
                     last_failure_detail = $5, updated_at = CURRENT_TIMESTAMP
                 WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NOT NULL
                   AND final_completed_at IS NULL",
            )
            .bind(claim.round_id)
            .bind(&claim.worker_id)
            .bind(retry_at)
            .bind(failure_code)
            .bind(safe_detail)
            .execute(pool)
            .await
        }
    }
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("Worker 失败记录租约已失效"));
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn claim_sqlite(
    pool: &super::TrackedPool<Sqlite>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<ScheduledRoundClaim>, auction_application::RepositoryError> {
    let mut connection = pool.acquire().await.map_err(map_storage_error)?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *connection).await.map_err(map_storage_error)?;
    let result = claim_sqlite_inner(&mut connection, worker_id, now, lease_until, limit).await;
    match result {
        Ok(claims) => {
            sqlx::query("COMMIT").execute(&mut *connection).await.map_err(map_storage_error)?;
            Ok(claims)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

#[cfg(feature = "sqlite")]
async fn claim_sqlite_inner(
    connection: &mut SqliteConnection,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<ScheduledRoundClaim>, auction_application::RepositoryError> {
    let now_text = sqlite_timestamp(now);
    let lease_text = sqlite_timestamp(lease_until);
    let rows = sqlx::query(
        "SELECT round.id, round.auction_id, round.create_command_id,
                round.preview_completed_at, round.open_completed_at, round.attempt_count
         FROM scheduled_rounds AS round
         INNER JOIN auctions AS auction ON auction.id = round.auction_id
         WHERE round.status = 'active'
           AND round.final_completed_at IS NULL
           AND (round.lease_until IS NULL OR round.lease_until <= ?)
           AND (
               (round.preview_completed_at IS NULL AND round.preview_due_at <= ?)
               OR (
                   round.preview_completed_at IS NOT NULL
                   AND round.open_completed_at IS NULL
                   AND round.open_due_at <= ?
               )
               OR (
                   round.open_completed_at IS NOT NULL
                   AND auction.status = 'live'
                   AND (
                       SELECT MIN(julianday(lot.effective_end_at))
                       FROM lots AS lot
                       WHERE lot.auction_id = round.auction_id AND lot.status = 'live'
                   ) - CAST(json_extract(
                       auction.policy_snapshot, '$.final_phase_lead_seconds'
                   ) AS REAL) / 86400.0 <= julianday(?)
               )
           )
         ORDER BY CASE
             WHEN round.preview_completed_at IS NULL THEN julianday(round.preview_due_at)
             WHEN round.open_completed_at IS NULL THEN julianday(round.open_due_at)
             ELSE (
                 SELECT MIN(julianday(lot.effective_end_at))
                 FROM lots AS lot
                 WHERE lot.auction_id = round.auction_id AND lot.status = 'live'
             ) - CAST(json_extract(
                 auction.policy_snapshot, '$.final_phase_lead_seconds'
             ) AS REAL) / 86400.0
         END, round.id
         LIMIT ?",
    )
    .bind(&now_text)
    .bind(&now_text)
    .bind(&now_text)
    .bind(&now_text)
    .bind(i64::from(limit))
    .fetch_all(&mut *connection)
    .await
    .map_err(map_storage_error)?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let round_id = uuid_from_blob(row.try_get("id").map_err(map_storage_error)?)?;
        let auction_id = AuctionId::from_uuid(uuid_from_blob(
            row.try_get("auction_id").map_err(map_storage_error)?,
        )?);
        let create_command_id =
            uuid_from_blob(row.try_get("create_command_id").map_err(map_storage_error)?)?;
        let preview_completed_at: Option<String> =
            row.try_get("preview_completed_at").map_err(map_storage_error)?;
        let open_completed_at: Option<String> =
            row.try_get("open_completed_at").map_err(map_storage_error)?;
        let phase =
            phase_from_completion(preview_completed_at.is_some(), open_completed_at.is_some());
        let old_attempt_count: i64 = row.try_get("attempt_count").map_err(map_storage_error)?;
        let attempt_count = old_attempt_count
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| repository_error_owned("Worker 尝试次数溢出"))?;
        let updated = sqlx::query(
            "UPDATE scheduled_rounds
             SET lease_owner = ?, lease_until = ?, attempt_count = ?,
                 state_version = state_version + 1,
                 last_failure_code = NULL, last_failure_detail = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ? AND status = 'active' AND final_completed_at IS NULL",
        )
        .bind(worker_id)
        .bind(&lease_text)
        .bind(i64::from(attempt_count))
        .bind(uuid_blob(round_id))
        .execute(&mut *connection)
        .await
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(repository_error_owned("Worker 领取轮次时状态已变化"));
        }
        claims.push(ScheduledRoundClaim {
            round_id,
            auction_id,
            phase,
            command_id: command_id(create_command_id, phase),
            worker_id: worker_id.to_owned(),
            lease_until,
            attempt_count,
        });
    }
    Ok(claims)
}

#[cfg(feature = "sqlite")]
async fn complete_sqlite(
    pool: &super::TrackedPool<Sqlite>,
    claim: &ScheduledRoundClaim,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let (sql, phase) = match claim.phase {
        ScheduledRoundPhase::Preview => (
            "UPDATE scheduled_rounds
             SET preview_command_id = ?, preview_completed_at = ?,
                 state_version = state_version + 1,
                 lease_owner = NULL, lease_until = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE id = ? AND status = 'active' AND lease_owner = ?
               AND preview_completed_at IS NULL AND final_completed_at IS NULL",
            "preview",
        ),
        ScheduledRoundPhase::Open => (
            "UPDATE scheduled_rounds
             SET open_command_id = ?, open_completed_at = ?,
                 state_version = state_version + 1,
                 lease_owner = NULL, lease_until = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE id = ? AND status = 'active' AND lease_owner = ?
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL
                   AND final_completed_at IS NULL",
            "open",
        ),
        ScheduledRoundPhase::Final => (
            "UPDATE scheduled_rounds
             SET final_command_id = ?, final_completed_at = ?,
                 state_version = state_version + 1,
                 lease_owner = NULL, lease_until = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE id = ? AND status = 'active' AND lease_owner = ?
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NOT NULL
                   AND final_completed_at IS NULL",
            "final",
        ),
    };
    let result = sqlx::query(sql)
        .bind(uuid_blob(claim.command_id.as_uuid()))
        .bind(sqlite_timestamp(completed_at))
        .bind(uuid_blob(claim.round_id))
        .bind(&claim.worker_id)
        .execute(pool)
        .await
        .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned(&format!("Worker {phase} 阶段完成租约已失效")));
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn fail_sqlite(
    pool: &super::TrackedPool<Sqlite>,
    claim: &ScheduledRoundClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    safe_detail: &str,
) -> Result<(), auction_application::RepositoryError> {
    let (sql, phase) = match claim.phase {
        ScheduledRoundPhase::Preview => (
            "UPDATE scheduled_rounds
             SET lease_owner = NULL, lease_until = ?, last_failure_code = ?,
                 state_version = state_version + 1,
                 last_failure_detail = ?, updated_at = CURRENT_TIMESTAMP
             WHERE id = ? AND status = 'active' AND lease_owner = ?
               AND preview_completed_at IS NULL AND final_completed_at IS NULL",
            "preview",
        ),
        ScheduledRoundPhase::Open => (
            "UPDATE scheduled_rounds
             SET lease_owner = NULL, lease_until = ?, last_failure_code = ?,
                 state_version = state_version + 1,
                 last_failure_detail = ?, updated_at = CURRENT_TIMESTAMP
             WHERE id = ? AND status = 'active' AND lease_owner = ?
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL
                   AND final_completed_at IS NULL",
            "open",
        ),
        ScheduledRoundPhase::Final => (
            "UPDATE scheduled_rounds
             SET lease_owner = NULL, lease_until = ?, last_failure_code = ?,
                 state_version = state_version + 1,
                 last_failure_detail = ?, updated_at = CURRENT_TIMESTAMP
             WHERE id = ? AND status = 'active' AND lease_owner = ?
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NOT NULL
                   AND final_completed_at IS NULL",
            "final",
        ),
    };
    let result = sqlx::query(sql)
        .bind(sqlite_timestamp(retry_at))
        .bind(failure_code)
        .bind(safe_detail)
        .bind(uuid_blob(claim.round_id))
        .bind(&claim.worker_id)
        .execute(pool)
        .await
        .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned(&format!("Worker 失败记录 {phase} 租约已失效")));
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
fn sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string()
}

#[cfg(feature = "sqlite")]
fn uuid_blob(value: Uuid) -> Vec<u8> {
    value.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(bytes: Vec<u8>) -> Result<Uuid, auction_application::RepositoryError> {
    Uuid::from_slice(&bytes).map_err(|_| repository_error_owned("Worker 调度 UUID 格式无效"))
}

fn repository_error_owned(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::RowNotFound => "Worker 调度记录不存在".to_owned(),
        _ => "Worker 调度数据库操作失败".to_owned(),
    })
}
