//! 每周拍卖轮次的数据库时间、幂等 bootstrap 和安全投影。

use auction_application::{ScheduledRoundBootstrap, ScheduledRoundSnapshot};
use chrono::{DateTime, Datelike, Duration, Utc};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::database::TrackedPool;

use super::mapping::{map_storage_error, repository_error_owned};

#[cfg(feature = "sqlite")]
use super::mapping::{sqlite_timestamp, uuid_blob};

#[cfg(feature = "postgres")]
use sqlx::Postgres;
#[cfg(feature = "sqlite")]
use sqlx::{Sqlite, SqliteConnection};

#[cfg(feature = "postgres")]
pub(super) async fn database_now_postgres(
    pool: &TrackedPool<Postgres>,
) -> Result<DateTime<Utc>, auction_application::RepositoryError> {
    sqlx::query_scalar("SELECT CURRENT_TIMESTAMP").fetch_one(pool).await.map_err(map_storage_error)
}

#[cfg(feature = "sqlite")]
pub(super) async fn database_now_sqlite(
    pool: &TrackedPool<Sqlite>,
) -> Result<DateTime<Utc>, auction_application::RepositoryError> {
    let value: String = sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
        .fetch_one(pool)
        .await
        .map_err(map_storage_error)?;
    DateTime::parse_from_str(&format!("{value} UTC"), "%Y-%m-%d %H:%M:%S %Z")
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| repository_error_owned("SQLite 数据库时间格式无效"))
}

#[cfg(feature = "postgres")]
pub(super) async fn ensure_postgres(
    pool: &TrackedPool<Postgres>,
    request: ScheduledRoundBootstrap,
) -> Result<ScheduledRoundSnapshot, auction_application::RepositoryError> {
    validate_request(&request)?;
    for attempt in 0..3_u8 {
        match ensure_postgres_once(pool, request.clone()).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(PostgresBootstrapError::Storage(error))
                if is_serialization_error(&error) && attempt < 2 =>
            {
                continue;
            }
            Err(PostgresBootstrapError::Storage(error)) => return Err(map_storage_error(error)),
            Err(PostgresBootstrapError::Repository(error)) => return Err(error),
        }
    }
    Err(repository_error_owned("周计划 bootstrap 序列化重试耗尽"))
}

#[cfg(feature = "postgres")]
enum PostgresBootstrapError {
    Storage(sqlx::Error),
    Repository(auction_application::RepositoryError),
}

#[cfg(feature = "postgres")]
impl From<auction_application::RepositoryError> for PostgresBootstrapError {
    fn from(error: auction_application::RepositoryError) -> Self {
        Self::Repository(error)
    }
}

#[cfg(feature = "postgres")]
fn is_serialization_error(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.code().as_deref() == Some("40001"))
}

#[cfg(feature = "postgres")]
async fn ensure_postgres_once(
    pool: &TrackedPool<Postgres>,
    request: ScheduledRoundBootstrap,
) -> Result<ScheduledRoundSnapshot, PostgresBootstrapError> {
    let mut transaction = pool.begin().await.map_err(PostgresBootstrapError::Storage)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(PostgresBootstrapError::Storage)?;
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM public.worlds WHERE id = $1 FOR UPDATE")
            .bind(&request.world_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(PostgresBootstrapError::Storage)?;
    match enabled {
        None => return Err(repository_error_owned("周计划 world 不存在").into()),
        Some(false) => return Err(repository_error_owned("周计划 world 已禁用").into()),
        Some(true) => {}
    }
    let db_now: DateTime<Utc> = sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
        .fetch_one(&mut *transaction)
        .await
        .map_err(PostgresBootstrapError::Storage)?;
    if let Some(row) = sqlx::query(
        "SELECT id, world_id, local_week, auction_id, status, preview_due_at, open_due_at,
                scheduled_end_at, create_command_id, preview_completed_at, open_completed_at
         FROM public.scheduled_rounds WHERE world_id = $1 AND local_week = $2 FOR UPDATE",
    )
    .bind(&request.world_id)
    .bind(request.local_week)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(PostgresBootstrapError::Storage)?
    {
        let result = snapshot_postgres(&row)?;
        verify_existing(&result, &request)?;
        if result.active {
            let auction = sqlx::query("SELECT policy_snapshot FROM public.auctions WHERE id = $1")
                .bind(request.auction_id.as_uuid())
                .fetch_one(&mut *transaction)
                .await
                .map_err(PostgresBootstrapError::Storage)?;
            let policy_snapshot: serde_json::Value =
                auction.try_get("policy_snapshot").map_err(map_storage_error)?;
            if policy_snapshot != request.policy.snapshot(request.scheduled_end_at) {
                return Err(repository_error_owned("周计划策略快照冲突").into());
            }
        }
        transaction.commit().await.map_err(PostgresBootstrapError::Storage)?;
        return Ok(result);
    }

    let missed = request.scheduled_end_at <= db_now;
    let sequence: i64 = if missed {
        0
    } else {
        sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM public.auctions WHERE world_id = $1",
        )
        .bind(&request.world_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(PostgresBootstrapError::Storage)?
    };
    let policy_snapshot = request.policy.snapshot(request.scheduled_end_at);
    let rules =
        request.policy.auction_rules().map_err(|_| repository_error_owned("周计划策略无效"))?;
    if !missed {
        sqlx::query(
            "INSERT INTO public.auctions
                (id, world_id, sequence, title, status, default_start_price,
                 soft_close_window_seconds, soft_close_extension_seconds,
                 cancel_cutoff_seconds, proxy_slow_interval_seconds, policy_snapshot,
                 preview_scheduled_at, scheduled_open_at, scheduled_end_at, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'draft', $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14)",
        )
        .bind(request.auction_id.as_uuid())
        .bind(&request.world_id)
        .bind(sequence)
        .bind(format!("{} #{}", request.world_id, sequence))
        .bind(rules.default_start_price.value())
        .bind(rules.soft_close_window.num_seconds())
        .bind(rules.soft_close_extension.num_seconds())
        .bind(rules.cancel_cutoff.num_seconds())
        .bind(rules.proxy_slow_interval.num_seconds())
        .bind(policy_snapshot)
        .bind(request.preview_due_at)
        .bind(request.open_due_at)
        .bind(request.scheduled_end_at)
        .bind(db_now)
        .execute(&mut *transaction)
        .await
        .map_err(PostgresBootstrapError::Storage)?;
        sqlx::query(
            "INSERT INTO public.auction_intake_targets
                (world_id, auction_id, target_status, state_version, updated_at)
             VALUES ($1, $2, 'draft', 1, $3)",
        )
        .bind(&request.world_id)
        .bind(request.auction_id.as_uuid())
        .bind(db_now)
        .execute(&mut *transaction)
        .await
        .map_err(PostgresBootstrapError::Storage)?;
    }
    insert_command_postgres(&mut transaction, &request, missed, db_now).await?;
    sqlx::query(
        "INSERT INTO public.scheduled_rounds
            (id, world_id, local_week, auction_id, status, preview_due_at, open_due_at,
             scheduled_end_at, create_command_id, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)",
    )
    .bind(request.round_id)
    .bind(&request.world_id)
    .bind(request.local_week)
    .bind((!missed).then_some(request.auction_id.as_uuid()))
    .bind(if missed { "missed" } else { "active" })
    .bind(request.preview_due_at)
    .bind(request.open_due_at)
    .bind(request.scheduled_end_at)
    .bind(request.create_command_id.as_uuid())
    .bind(db_now)
    .execute(&mut *transaction)
    .await
    .map_err(PostgresBootstrapError::Storage)?;
    append_audit_postgres(&mut transaction, &request, missed, db_now).await?;
    transaction.commit().await.map_err(PostgresBootstrapError::Storage)?;
    Ok(ScheduledRoundSnapshot {
        round_id: request.round_id,
        auction_id: (!missed).then_some(request.auction_id),
        active: !missed,
        world_id: request.world_id,
        local_week: request.local_week,
        preview_due_at: request.preview_due_at,
        open_due_at: request.open_due_at,
        scheduled_end_at: request.scheduled_end_at,
        create_command_id: request.create_command_id.as_uuid().to_owned(),
        preview_completed_at: None,
        open_completed_at: None,
    })
}

#[cfg(feature = "postgres")]
async fn insert_command_postgres(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    request: &ScheduledRoundBootstrap,
    missed: bool,
    now: DateTime<Utc>,
) -> Result<(), PostgresBootstrapError> {
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload, created_at)
         VALUES ($1, 'scheduled_round_create', $2, $3, 'accepted', $4, $5)",
    )
    .bind(request.create_command_id.as_uuid())
    .bind(request.round_id)
    .bind(request.request_hash.as_bytes())
    .bind(json!({
        "round_id": request.round_id,
        "auction_id": (!missed).then_some(request.auction_id.as_uuid()),
        "world_id": request.world_id,
        "local_week": request.local_week,
        "missed": missed,
    }))
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(PostgresBootstrapError::Storage)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn append_audit_postgres(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    request: &ScheduledRoundBootstrap,
    missed: bool,
    now: DateTime<Utc>,
) -> Result<(), PostgresBootstrapError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES ($1, $2, 'worker', 'auction-worker', 'auction.schedule_round',
                 'scheduled_round', $3, 'success', 'worker', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(request.round_id.to_string())
    .bind(json!({"world_id": request.world_id, "local_week": request.local_week, "missed": missed}))
    .execute(&mut **transaction)
    .await
    .map_err(PostgresBootstrapError::Storage)?;
    Ok(())
}

#[cfg(feature = "sqlite")]
pub(super) async fn ensure_sqlite(
    pool: &TrackedPool<Sqlite>,
    request: ScheduledRoundBootstrap,
) -> Result<ScheduledRoundSnapshot, auction_application::RepositoryError> {
    validate_request(&request)?;
    let mut connection = pool.acquire().await.map_err(map_storage_error)?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *connection).await.map_err(map_storage_error)?;
    let result = ensure_sqlite_inner(&mut connection, request).await;
    match result {
        Ok(result) => {
            sqlx::query("COMMIT").execute(&mut *connection).await.map_err(map_storage_error)?;
            Ok(result)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

#[cfg(feature = "sqlite")]
async fn ensure_sqlite_inner(
    connection: &mut SqliteConnection,
    request: ScheduledRoundBootstrap,
) -> Result<ScheduledRoundSnapshot, auction_application::RepositoryError> {
    let enabled: Option<i64> = sqlx::query_scalar("SELECT enabled FROM worlds WHERE id = ?")
        .bind(&request.world_id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_storage_error)?;
    if enabled != Some(1) {
        return Err(repository_error_owned(if enabled.is_none() {
            "周计划 world 不存在"
        } else {
            "周计划 world 已禁用"
        }));
    }
    if let Some(row) = sqlx::query(
        "SELECT id, world_id, local_week, auction_id, status, preview_due_at, open_due_at,
                scheduled_end_at, create_command_id, preview_completed_at, open_completed_at
         FROM scheduled_rounds WHERE world_id = ? AND local_week = ?",
    )
    .bind(&request.world_id)
    .bind(request.local_week.format("%Y-%m-%d").to_string())
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_storage_error)?
    {
        let result = snapshot_sqlite(&row)?;
        verify_existing(&result, &request)?;
        if result.active {
            let policy_snapshot: String =
                sqlx::query_scalar("SELECT policy_snapshot FROM auctions WHERE id = ?")
                    .bind(uuid_blob(request.auction_id.as_uuid()))
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(map_storage_error)?;
            let policy_snapshot: serde_json::Value = serde_json::from_str(&policy_snapshot)
                .map_err(|_| repository_error_owned("SQLite 周计划策略快照无效"))?;
            if policy_snapshot != request.policy.snapshot(request.scheduled_end_at) {
                return Err(repository_error_owned("周计划策略快照冲突"));
            }
        }
        return Ok(result);
    }
    let db_now = sqlite_now(connection).await?;
    let missed = request.scheduled_end_at <= db_now;
    let sequence: i64 = if missed {
        0
    } else {
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) + 1 FROM auctions WHERE world_id = ?")
            .bind(&request.world_id)
            .fetch_one(&mut *connection)
            .await
            .map_err(map_storage_error)?
    };
    let policy_snapshot = request.policy.snapshot(request.scheduled_end_at).to_string();
    let rules =
        request.policy.auction_rules().map_err(|_| repository_error_owned("周计划策略无效"))?;
    if !missed {
        sqlx::query(
            "INSERT INTO auctions
                (id, world_id, sequence, title, status, default_start_price,
                 soft_close_window_seconds, soft_close_extension_seconds, cancel_cutoff_seconds,
                 proxy_slow_interval_seconds, policy_snapshot, preview_scheduled_at,
                 scheduled_open_at, scheduled_end_at, state_version, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'draft', ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(uuid_blob(request.auction_id.as_uuid()))
        .bind(&request.world_id)
        .bind(sequence)
        .bind(format!("{} #{}", request.world_id, sequence))
        .bind(rules.default_start_price.value())
        .bind(rules.soft_close_window.num_seconds())
        .bind(rules.soft_close_extension.num_seconds())
        .bind(rules.cancel_cutoff.num_seconds())
        .bind(rules.proxy_slow_interval.num_seconds())
        .bind(policy_snapshot)
        .bind(sqlite_timestamp(request.preview_due_at))
        .bind(sqlite_timestamp(request.open_due_at))
        .bind(sqlite_timestamp(request.scheduled_end_at))
        .bind(sqlite_timestamp(db_now))
        .bind(sqlite_timestamp(db_now))
        .execute(&mut *connection)
        .await
        .map_err(map_storage_error)?;
    }
    sqlx::query(
        "INSERT INTO auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload, created_at)
         VALUES (?, 'scheduled_round_create', ?, ?, 'accepted', ?, ?)",
    )
    .bind(uuid_blob(request.create_command_id.as_uuid()))
    .bind(uuid_blob(request.round_id))
    .bind(request.request_hash.as_bytes().to_vec())
    .bind(json!({"round_id": request.round_id, "missed": missed}).to_string())
    .bind(sqlite_timestamp(db_now))
    .execute(&mut *connection)
    .await
    .map_err(map_storage_error)?;
    sqlx::query(
        "INSERT INTO scheduled_rounds
            (id, world_id, local_week, auction_id, status, preview_due_at, open_due_at,
             scheduled_end_at, create_command_id, state_version, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
    )
    .bind(uuid_blob(request.round_id))
    .bind(&request.world_id)
    .bind(request.local_week.format("%Y-%m-%d").to_string())
    .bind((!missed).then(|| uuid_blob(request.auction_id.as_uuid())))
    .bind(if missed { "missed" } else { "active" })
    .bind(sqlite_timestamp(request.preview_due_at))
    .bind(sqlite_timestamp(request.open_due_at))
    .bind(sqlite_timestamp(request.scheduled_end_at))
    .bind(uuid_blob(request.create_command_id.as_uuid()))
    .bind(sqlite_timestamp(db_now))
    .bind(sqlite_timestamp(db_now))
    .execute(&mut *connection)
    .await
    .map_err(map_storage_error)?;
    sqlx::query(
        "INSERT INTO audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES (?, ?, 'worker', 'auction-worker', 'auction.schedule_round',
                 'scheduled_round', ?, 'success', 'worker', ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(sqlite_timestamp(db_now))
    .bind(request.round_id.to_string())
    .bind(json!({"world_id": request.world_id, "local_week": request.local_week, "missed": missed}).to_string())
    .execute(&mut *connection)
    .await
    .map_err(map_storage_error)?;
    Ok(ScheduledRoundSnapshot {
        round_id: request.round_id,
        auction_id: (!missed).then_some(request.auction_id),
        active: !missed,
        world_id: request.world_id,
        local_week: request.local_week,
        preview_due_at: request.preview_due_at,
        open_due_at: request.open_due_at,
        scheduled_end_at: request.scheduled_end_at,
        create_command_id: request.create_command_id.as_uuid().to_owned(),
        preview_completed_at: None,
        open_completed_at: None,
    })
}

#[cfg(feature = "sqlite")]
async fn sqlite_now(
    connection: &mut SqliteConnection,
) -> Result<DateTime<Utc>, auction_application::RepositoryError> {
    let value: String = sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
        .fetch_one(&mut *connection)
        .await
        .map_err(map_storage_error)?;
    DateTime::parse_from_str(&format!("{value} UTC"), "%Y-%m-%d %H:%M:%S %Z")
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| repository_error_owned("SQLite 数据库时间格式无效"))
}

fn validate_request(
    request: &ScheduledRoundBootstrap,
) -> Result<(), auction_application::RepositoryError> {
    if request.round_id.is_nil()
        || request.auction_id.is_nil()
        || request.create_command_id.is_nil()
        || !matches!(request.world_id.as_str(), "main" | "isekai")
        || request.local_week.weekday().num_days_from_monday() != 0
        || request.preview_due_at >= request.open_due_at
        || request.open_due_at - request.preview_due_at < Duration::hours(2)
        || request.open_due_at >= request.scheduled_end_at
        || request.request_hash.as_bytes() == [0; 32]
    {
        return Err(repository_error_owned("周计划 bootstrap 参数无效"));
    }
    request.policy.validate().map_err(|_| repository_error_owned("周计划策略无效"))
}

fn verify_existing(
    snapshot: &ScheduledRoundSnapshot,
    request: &ScheduledRoundBootstrap,
) -> Result<(), auction_application::RepositoryError> {
    if snapshot.round_id != request.round_id
        || snapshot.world_id != request.world_id
        || snapshot.local_week != request.local_week
        || snapshot.preview_due_at != request.preview_due_at
        || snapshot.open_due_at != request.open_due_at
        || snapshot.scheduled_end_at != request.scheduled_end_at
        || snapshot.create_command_id != request.create_command_id.as_uuid()
        || snapshot.active != snapshot.auction_id.is_some()
        || (!snapshot.active && snapshot.auction_id.is_some())
        || (snapshot.active && snapshot.auction_id != Some(request.auction_id))
    {
        return Err(repository_error_owned("周计划稳定 identity 冲突"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) fn snapshot_postgres(
    row: &sqlx::postgres::PgRow,
) -> Result<ScheduledRoundSnapshot, auction_application::RepositoryError> {
    let active = scheduled_round_active(row.try_get("status").map_err(map_storage_error)?)?;
    Ok(ScheduledRoundSnapshot {
        round_id: row.try_get("id").map_err(map_storage_error)?,
        auction_id: row
            .try_get::<Option<Uuid>, _>("auction_id")
            .map_err(map_storage_error)?
            .map(auction_domain::AuctionId::from_uuid),
        active,
        world_id: row.try_get("world_id").map_err(map_storage_error)?,
        local_week: row.try_get("local_week").map_err(map_storage_error)?,
        preview_due_at: row.try_get("preview_due_at").map_err(map_storage_error)?,
        open_due_at: row.try_get("open_due_at").map_err(map_storage_error)?,
        scheduled_end_at: row.try_get("scheduled_end_at").map_err(map_storage_error)?,
        create_command_id: row.try_get("create_command_id").map_err(map_storage_error)?,
        preview_completed_at: row.try_get("preview_completed_at").map_err(map_storage_error)?,
        open_completed_at: row.try_get("open_completed_at").map_err(map_storage_error)?,
    })
}

#[cfg(feature = "sqlite")]
pub(super) fn snapshot_sqlite(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ScheduledRoundSnapshot, auction_application::RepositoryError> {
    let round_id = Uuid::from_slice(&row.try_get::<Vec<u8>, _>("id").map_err(map_storage_error)?)
        .map_err(|_| repository_error_owned("周计划 round UUID 无效"))?;
    let local_week = chrono::NaiveDate::parse_from_str(
        &row.try_get::<String, _>("local_week").map_err(map_storage_error)?,
        "%Y-%m-%d",
    )
    .map_err(|_| repository_error_owned("周计划 local_week 无效"))?;
    let auction_id = row
        .try_get::<Option<Vec<u8>>, _>("auction_id")
        .map_err(map_storage_error)?
        .map(|value| {
            Uuid::from_slice(&value)
                .map(auction_domain::AuctionId::from_uuid)
                .map_err(|_| repository_error_owned("周计划 auction UUID 无效"))
        })
        .transpose()?;
    let active = scheduled_round_active(row.try_get("status").map_err(map_storage_error)?)?;
    Ok(ScheduledRoundSnapshot {
        round_id,
        auction_id,
        active,
        world_id: row.try_get("world_id").map_err(map_storage_error)?,
        local_week,
        preview_due_at: parse_required_sqlite_timestamp(
            &row.try_get::<String, _>("preview_due_at").map_err(map_storage_error)?,
        )?,
        open_due_at: parse_required_sqlite_timestamp(
            &row.try_get::<String, _>("open_due_at").map_err(map_storage_error)?,
        )?,
        scheduled_end_at: parse_required_sqlite_timestamp(
            &row.try_get::<String, _>("scheduled_end_at").map_err(map_storage_error)?,
        )?,
        create_command_id: Uuid::from_slice(
            &row.try_get::<Vec<u8>, _>("create_command_id").map_err(map_storage_error)?,
        )
        .map_err(|_| repository_error_owned("周计划 create command UUID 无效"))?,
        preview_completed_at: row
            .try_get::<Option<String>, _>("preview_completed_at")
            .map_err(map_storage_error)?
            .and_then(|value| parse_sqlite_timestamp(&value)),
        open_completed_at: row
            .try_get::<Option<String>, _>("open_completed_at")
            .map_err(map_storage_error)?
            .and_then(|value| parse_sqlite_timestamp(&value)),
    })
}

fn scheduled_round_active(status: String) -> Result<bool, auction_application::RepositoryError> {
    match status.as_str() {
        "active" => Ok(true),
        "missed" => Ok(false),
        _ => Err(repository_error_owned("周计划轮次状态无效")),
    }
}

#[cfg(feature = "sqlite")]
fn parse_sqlite_timestamp(value: &str) -> Option<DateTime<Utc>> {
    ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"]
        .iter()
        .find_map(|format| chrono::NaiveDateTime::parse_from_str(value, format).ok())
        .map(|value| value.and_utc())
}

#[cfg(feature = "sqlite")]
fn parse_required_sqlite_timestamp(
    value: &str,
) -> Result<DateTime<Utc>, auction_application::RepositoryError> {
    parse_sqlite_timestamp(value).ok_or_else(|| repository_error_owned("SQLite 周计划时间格式无效"))
}
