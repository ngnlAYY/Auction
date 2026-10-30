//! 每周拍卖轮次与赛季早鸟轮次的数据库时间、幂等 bootstrap 和安全投影。

use auction_application::{ScheduledRoundBootstrap, ScheduledRoundSnapshot};
use chrono::{DateTime, Datelike, Utc};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::database::TrackedPool;

use super::mapping::{map_storage_error, repository_error_owned};

#[cfg(feature = "postgres")]
use sqlx::Postgres;

#[cfg(feature = "postgres")]
pub(super) async fn database_now_postgres(
    pool: &TrackedPool<Postgres>,
) -> Result<DateTime<Utc>, auction_application::RepositoryError> {
    sqlx::query_scalar("SELECT CURRENT_TIMESTAMP").fetch_one(pool).await.map_err(map_storage_error)
}

#[cfg(feature = "postgres")]
pub(super) async fn ensure_postgres(
    pool: &TrackedPool<Postgres>,
    request: ScheduledRoundBootstrap,
) -> Result<ScheduledRoundSnapshot, auction_application::RepositoryError> {
    validate_request(&request)?;
    for attempt in 0..3_u8 {
        match ensure_postgres_once(pool, &request).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(PostgresBootstrapError::Storage(error))
                if crate::database::is_serialization_conflict(&error) && attempt < 2 =>
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
async fn ensure_postgres_once(
    pool: &TrackedPool<Postgres>,
    request: &ScheduledRoundBootstrap,
) -> Result<ScheduledRoundSnapshot, PostgresBootstrapError> {
    let mut transaction = pool
        .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
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
    // world 行锁等待可能跨过整周窗口；事务起始时间不能据此创建已经过期的 Draft。
    let db_now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *transaction)
        .await
        .map_err(PostgresBootstrapError::Storage)?;
    // 分开两种身份谓词，让预处理通用计划也能识别 000065 的部分索引条件。
    let existing = if let Some(slot_key) = request.slot_key.as_deref() {
        sqlx::query(
            "SELECT id, world_id, local_week, slot_key, auction_id, status, preview_due_at,
                    open_due_at, scheduled_end_at, create_command_id, preview_completed_at,
                    open_completed_at, schedule_snapshot
             FROM public.scheduled_rounds
             WHERE world_id = $1 AND slot_key = $2
             FOR UPDATE",
        )
        .bind(&request.world_id)
        .bind(slot_key)
    } else {
        sqlx::query(
            "SELECT id, world_id, local_week, slot_key, auction_id, status, preview_due_at,
                    open_due_at, scheduled_end_at, create_command_id, preview_completed_at,
                    open_completed_at, schedule_snapshot
             FROM public.scheduled_rounds
             WHERE world_id = $1 AND local_week = $2 AND slot_key IS NULL
             FOR UPDATE",
        )
        .bind(&request.world_id)
        .bind(request.local_week)
    };
    if let Some(row) =
        existing.fetch_optional(&mut *transaction).await.map_err(PostgresBootstrapError::Storage)?
    {
        let result = snapshot_postgres(&row)?;
        verify_existing(&result, request)?;
        if let Some(source) = row
            .try_get::<Option<serde_json::Value>, _>("schedule_snapshot")
            .map_err(map_storage_error)?
        {
            let source: auction_application::AuctionScheduleSource = serde_json::from_value(source)
                .map_err(|_| repository_error_owned("周计划来源快照无效"))?;
            if source != request.schedule_source {
                return Err(repository_error_owned("周计划来源快照冲突").into());
            }
        }
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
                 preview_scheduled_at, scheduled_open_at, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'draft', $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)",
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
        // Draft 的结束计划只保存在策略与轮次中，开拍事务才物化运行期结束时间。
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
    insert_command_postgres(&mut transaction, request, missed, db_now).await?;
    sqlx::query(
        "INSERT INTO public.scheduled_rounds
            (id, world_id, local_week, auction_id, status, preview_due_at, open_due_at,
             scheduled_end_at, create_command_id, created_at, updated_at, schedule_snapshot,
             slot_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10, $11, $12)",
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
    .bind(
        serde_json::to_value(&request.schedule_source)
            .map_err(|_| repository_error_owned("周计划来源无法编码"))?,
    )
    .bind(request.slot_key.as_deref())
    .execute(&mut *transaction)
    .await
    .map_err(PostgresBootstrapError::Storage)?;
    append_audit_postgres(&mut transaction, request, missed, db_now).await?;
    transaction.commit().await.map_err(PostgresBootstrapError::Storage)?;
    Ok(ScheduledRoundSnapshot {
        round_id: request.round_id,
        auction_id: (!missed).then_some(request.auction_id),
        active: !missed,
        world_id: request.world_id.clone(),
        local_week: request.local_week,
        slot_key: request.slot_key.clone(),
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
    .bind(with_slot_key(
        json!({
            "round_id": request.round_id,
            "auction_id": (!missed).then_some(request.auction_id.as_uuid()),
            "world_id": request.world_id,
            "local_week": request.local_week,
            "missed": missed,
        }),
        request.slot_key.as_deref(),
    ))
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
    .bind(with_slot_key(
        json!({"world_id": request.world_id, "local_week": request.local_week, "missed": missed}),
        request.slot_key.as_deref(),
    ))
    .execute(&mut **transaction)
    .await
    .map_err(PostgresBootstrapError::Storage)?;
    Ok(())
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
        || request.open_due_at >= request.scheduled_end_at
        || request.request_hash.as_bytes() == [0; 32]
        || request.slot_key.as_deref().is_some_and(|key| !valid_slot_key(key))
        || request.slot_key != request.schedule_source.early_bird.map(|slot| slot.key())
    {
        return Err(repository_error_owned("周计划 bootstrap 参数无效"));
    }
    let times =
        request.schedule_source.resolve().map_err(|_| repository_error_owned("周计划来源无效"))?;
    if times.preview_due_at != request.preview_due_at
        || times.open_due_at != request.open_due_at
        || times.scheduled_end_at != request.scheduled_end_at
    {
        return Err(repository_error_owned("周计划时间与来源不一致"));
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
        || snapshot.slot_key != request.slot_key
        || snapshot.preview_due_at != request.preview_due_at
        || snapshot.open_due_at != request.open_due_at
        || snapshot.scheduled_end_at != request.scheduled_end_at
        || snapshot.create_command_id != request.create_command_id.as_uuid()
        || snapshot.active != snapshot.auction_id.is_some()
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
        slot_key: row.try_get("slot_key").map_err(map_storage_error)?,
        preview_due_at: row.try_get("preview_due_at").map_err(map_storage_error)?,
        open_due_at: row.try_get("open_due_at").map_err(map_storage_error)?,
        scheduled_end_at: row.try_get("scheduled_end_at").map_err(map_storage_error)?,
        create_command_id: row.try_get("create_command_id").map_err(map_storage_error)?,
        preview_completed_at: row.try_get("preview_completed_at").map_err(map_storage_error)?,
        open_completed_at: row.try_get("open_completed_at").map_err(map_storage_error)?,
    })
}

/// 早鸟时段键形状：`early-bird:YYYY-MM-DD:N`，N 为 1–99；与数据库 CHECK 一致。
pub(super) fn valid_slot_key(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("early-bird:") else {
        return false;
    };
    let Some((date, session)) = rest.split_once(':') else {
        return false;
    };
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
        && date.len() == 10
        && auction_domain::valid_positive_decimal(session, 2)
}

/// 早鸟轮次在命令结果与审计元数据中追加时段键；常规周保持原有字段集合。
fn with_slot_key(mut value: serde_json::Value, slot_key: Option<&str>) -> serde_json::Value {
    if let (Some(slot_key), Some(object)) = (slot_key, value.as_object_mut()) {
        object.insert("slot_key".to_owned(), serde_json::Value::String(slot_key.to_owned()));
    }
    value
}

fn scheduled_round_active(status: String) -> Result<bool, auction_application::RepositoryError> {
    match status.as_str() {
        "active" => Ok(true),
        "missed" => Ok(false),
        _ => Err(repository_error_owned("周计划轮次状态无效")),
    }
}
