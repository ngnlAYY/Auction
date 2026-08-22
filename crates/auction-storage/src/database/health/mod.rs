//! service_heartbeats 的 PostgreSQL 完整档与 SQLite Lite 适配器。
//!
//! 心跳是可过期的安全投影。PostgreSQL 使用数据库时钟和 observation sequence 条件 upsert；
//! SQLite Lite 使用本地数据库时钟、单主机写锁和同一序列 fence。两个后端都只返回 dashboard
//! 所需字段，不读取或转发 details 原文。

use auction_observability::{
    HealthError, HealthFuture, Heartbeat, HeartbeatCommand, HeartbeatStatus, HeartbeatStore,
    ProcessRole, ServiceName, TracingHealth, TracingStatus,
};

use super::DatabaseStore;

impl HeartbeatStore for DatabaseStore {
    fn publish(&self, command: HeartbeatCommand) -> HealthFuture<'_, ()> {
        Box::pin(async move {
            if !command.validate() {
                return Err(HealthError::InvalidCommand);
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => publish_postgres(store, &command).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => publish_sqlite(store, &command).await,
            }
        })
    }

    fn list_live(&self) -> HealthFuture<'_, Vec<Heartbeat>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_sqlite(store).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn publish_postgres(
    store: &super::PostgresStore,
    command: &HeartbeatCommand,
) -> Result<(), HealthError> {
    use sqlx::types::Json;

    let ttl_micros = command.ttl.num_microseconds().ok_or(HealthError::InvalidCommand)?;
    let returned: Option<i64> = sqlx::query_scalar(
        "WITH observation AS (SELECT clock_timestamp() AS observed_at)
         INSERT INTO public.service_heartbeats AS current_heartbeat (
             service_name, process_role, instance_id, version, started_at, status, readiness,
             audit_failure_count, tracing_status, tracing_dropped_span_count,
             tracing_export_failure_count, tracing_last_success_at, tracing_last_failure_at,
             tracing_last_error_code, observation_sequence, observed_at, expires_at, details
         )
         SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                observation.observed_at,
                observation.observed_at + ($16::bigint * interval '1 microsecond'),
                $17
         FROM observation
         ON CONFLICT (service_name, instance_id) DO UPDATE SET
             process_role = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.process_role ELSE current_heartbeat.process_role END,
             version = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.version ELSE current_heartbeat.version END,
             started_at = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.started_at ELSE current_heartbeat.started_at END,
             status = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.status ELSE current_heartbeat.status END,
             readiness = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.readiness ELSE current_heartbeat.readiness END,
             audit_failure_count = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.audit_failure_count ELSE current_heartbeat.audit_failure_count END,
             tracing_status = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.tracing_status ELSE current_heartbeat.tracing_status END,
             tracing_dropped_span_count = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.tracing_dropped_span_count ELSE current_heartbeat.tracing_dropped_span_count END,
             tracing_export_failure_count = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.tracing_export_failure_count ELSE current_heartbeat.tracing_export_failure_count END,
             tracing_last_success_at = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.tracing_last_success_at ELSE current_heartbeat.tracing_last_success_at END,
             tracing_last_failure_at = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.tracing_last_failure_at ELSE current_heartbeat.tracing_last_failure_at END,
             tracing_last_error_code = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.tracing_last_error_code ELSE current_heartbeat.tracing_last_error_code END,
             observation_sequence = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.observation_sequence ELSE current_heartbeat.observation_sequence END,
             observed_at = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.observed_at ELSE current_heartbeat.observed_at END,
             expires_at = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.expires_at ELSE current_heartbeat.expires_at END,
             details = CASE WHEN current_heartbeat.observation_sequence < EXCLUDED.observation_sequence
                 THEN EXCLUDED.details ELSE current_heartbeat.details END
         WHERE current_heartbeat.observation_sequence <= EXCLUDED.observation_sequence
         RETURNING observation_sequence",
    )
    .bind(command.service_name.as_str())
    .bind(command.process_role.as_str())
    .bind(&command.instance_id)
    .bind(&command.version)
    .bind(command.started_at)
    .bind(command.status.as_str())
    .bind(command.readiness)
    .bind(command.audit_failure_count)
    .bind(command.tracing.status.as_str())
    .bind(command.tracing.dropped_span_count)
    .bind(command.tracing.export_failure_count)
    .bind(command.tracing.last_success_at)
    .bind(command.tracing.last_failure_at)
    .bind(&command.tracing.last_error_code)
    .bind(command.observation_sequence)
    .bind(ttl_micros)
    .bind(Json(command.details.clone()))
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("写入 PostgreSQL 服务心跳失败"))?;

    if returned.is_some() { Ok(()) } else { Err(HealthError::StaleObservation) }
}

#[cfg(feature = "postgres")]
async fn list_postgres(store: &super::PostgresStore) -> Result<Vec<Heartbeat>, HealthError> {
    use sqlx::Row;

    let rows = sqlx::query(
        "SELECT service_name, process_role, instance_id, version, started_at, status, readiness,
                audit_failure_count, tracing_status, tracing_dropped_span_count,
                tracing_export_failure_count, tracing_last_success_at, tracing_last_failure_at,
                tracing_last_error_code, observation_sequence, observed_at, expires_at
         FROM public.service_heartbeats
         WHERE expires_at > clock_timestamp()
         ORDER BY service_name, instance_id",
    )
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 服务心跳失败"))?;

    rows.into_iter()
        .map(|row| {
            let service_name = parse_service(
                row.try_get("service_name").map_err(|_| storage_error("读取服务心跳名称失败"))?,
            )?;
            let process_role = parse_role(
                row.try_get("process_role").map_err(|_| storage_error("读取服务心跳职责失败"))?,
            )?;
            let status = parse_status(
                row.try_get("status").map_err(|_| storage_error("读取服务心跳状态失败"))?,
            )?;
            let tracing = TracingHealth {
                status: parse_tracing(
                    row.try_get("tracing_status")
                        .map_err(|_| storage_error("读取 tracing 状态失败"))?,
                )?,
                dropped_span_count: row
                    .try_get("tracing_dropped_span_count")
                    .map_err(|_| storage_error("读取 tracing 丢弃计数失败"))?,
                export_failure_count: row
                    .try_get("tracing_export_failure_count")
                    .map_err(|_| storage_error("读取 tracing 失败计数失败"))?,
                last_success_at: row
                    .try_get("tracing_last_success_at")
                    .map_err(|_| storage_error("读取 tracing 成功时间失败"))?,
                last_failure_at: row
                    .try_get("tracing_last_failure_at")
                    .map_err(|_| storage_error("读取 tracing 失败时间失败"))?,
                last_error_code: row
                    .try_get("tracing_last_error_code")
                    .map_err(|_| storage_error("读取 tracing 错误码失败"))?,
            };
            let heartbeat = Heartbeat {
                service_name,
                process_role,
                instance_id: row
                    .try_get("instance_id")
                    .map_err(|_| storage_error("读取服务心跳实例失败"))?,
                version: row
                    .try_get("version")
                    .map_err(|_| storage_error("读取服务心跳版本失败"))?,
                started_at: row
                    .try_get("started_at")
                    .map_err(|_| storage_error("读取服务心跳启动时间失败"))?,
                status,
                readiness: row
                    .try_get("readiness")
                    .map_err(|_| storage_error("读取服务心跳 readiness 失败"))?,
                audit_failure_count: row
                    .try_get("audit_failure_count")
                    .map_err(|_| storage_error("读取服务心跳审计计数失败"))?,
                tracing,
                observation_sequence: row
                    .try_get("observation_sequence")
                    .map_err(|_| storage_error("读取服务心跳序列失败"))?,
                observed_at: row
                    .try_get("observed_at")
                    .map_err(|_| storage_error("读取服务心跳观察时间失败"))?,
                expires_at: row
                    .try_get("expires_at")
                    .map_err(|_| storage_error("读取服务心跳过期时间失败"))?,
            };
            heartbeat
                .validate()
                .then_some(heartbeat)
                .ok_or_else(|| storage_error("PostgreSQL 服务心跳安全投影无效"))
        })
        .collect()
}

#[cfg(feature = "sqlite")]
async fn publish_sqlite(
    store: &super::SqliteStore,
    command: &HeartbeatCommand,
) -> Result<(), HealthError> {
    let ttl_seconds = command.ttl.num_seconds().max(1);
    let returned: Option<i64> = sqlx::query_scalar(
        "INSERT INTO service_heartbeats (
             service_name, process_role, instance_id, version, started_at, status, readiness,
             audit_failure_count, observation_sequence, observed_at, expires_at, details,
             tracing_status, tracing_dropped_span_count, tracing_export_failure_count,
             tracing_last_success_at, tracing_last_failure_at, tracing_last_error_code
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?,
             strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+' || ? || ' seconds'), ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (service_name, instance_id) DO UPDATE SET
             process_role = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.process_role ELSE service_heartbeats.process_role END,
             version = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.version ELSE service_heartbeats.version END,
             started_at = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.started_at ELSE service_heartbeats.started_at END,
             status = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.status ELSE service_heartbeats.status END,
             readiness = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.readiness ELSE service_heartbeats.readiness END,
             audit_failure_count = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.audit_failure_count ELSE service_heartbeats.audit_failure_count END,
             observation_sequence = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.observation_sequence ELSE service_heartbeats.observation_sequence END,
             observed_at = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.observed_at ELSE service_heartbeats.observed_at END,
             expires_at = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.expires_at ELSE service_heartbeats.expires_at END,
             details = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.details ELSE service_heartbeats.details END,
             tracing_status = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.tracing_status ELSE service_heartbeats.tracing_status END,
             tracing_dropped_span_count = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.tracing_dropped_span_count ELSE service_heartbeats.tracing_dropped_span_count END,
             tracing_export_failure_count = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.tracing_export_failure_count ELSE service_heartbeats.tracing_export_failure_count END,
             tracing_last_success_at = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.tracing_last_success_at ELSE service_heartbeats.tracing_last_success_at END,
             tracing_last_failure_at = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.tracing_last_failure_at ELSE service_heartbeats.tracing_last_failure_at END,
             tracing_last_error_code = CASE WHEN service_heartbeats.observation_sequence < excluded.observation_sequence
                 THEN excluded.tracing_last_error_code ELSE service_heartbeats.tracing_last_error_code END
         WHERE service_heartbeats.observation_sequence <= excluded.observation_sequence
         RETURNING observation_sequence",
    )
    .bind(command.service_name.as_str())
    .bind(command.process_role.as_str())
    .bind(&command.instance_id)
    .bind(&command.version)
    .bind(sqlite_timestamp(command.started_at))
    .bind(command.status.as_str())
    .bind(i64::from(command.readiness))
    .bind(command.audit_failure_count)
    .bind(command.observation_sequence)
    .bind(ttl_seconds)
    .bind(command.details.to_string())
    .bind(command.tracing.status.as_str())
    .bind(command.tracing.dropped_span_count)
    .bind(command.tracing.export_failure_count)
    .bind(command.tracing.last_success_at.map(sqlite_timestamp))
    .bind(command.tracing.last_failure_at.map(sqlite_timestamp))
    .bind(&command.tracing.last_error_code)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("写入 SQLite 服务心跳失败"))?;

    if returned.is_some() { Ok(()) } else { Err(HealthError::StaleObservation) }
}

#[cfg(feature = "sqlite")]
async fn list_sqlite(store: &super::SqliteStore) -> Result<Vec<Heartbeat>, HealthError> {
    let rows = sqlx::query(
        "SELECT service_name, process_role, instance_id, version, started_at, status, readiness,
                audit_failure_count, tracing_status, tracing_dropped_span_count,
                tracing_export_failure_count, tracing_last_success_at, tracing_last_failure_at,
                tracing_last_error_code, observation_sequence, observed_at, expires_at
         FROM service_heartbeats
         WHERE datetime(expires_at) > datetime('now')
         ORDER BY service_name, instance_id",
    )
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 SQLite 服务心跳失败"))?;

    rows.into_iter().map(map_sqlite_heartbeat).collect()
}

#[cfg(feature = "sqlite")]
fn map_sqlite_heartbeat(row: sqlx::sqlite::SqliteRow) -> Result<Heartbeat, HealthError> {
    use sqlx::Row;

    let service_name = parse_service(
        row.try_get("service_name").map_err(|_| storage_error("读取服务心跳名称失败"))?,
    )?;
    let process_role = parse_role(
        row.try_get("process_role").map_err(|_| storage_error("读取服务心跳职责失败"))?,
    )?;
    let tracing = TracingHealth {
        status: parse_tracing(
            row.try_get("tracing_status").map_err(|_| storage_error("读取 tracing 状态失败"))?,
        )?,
        dropped_span_count: row
            .try_get("tracing_dropped_span_count")
            .map_err(|_| storage_error("读取 tracing 丢弃计数失败"))?,
        export_failure_count: row
            .try_get("tracing_export_failure_count")
            .map_err(|_| storage_error("读取 tracing 失败计数失败"))?,
        last_success_at: parse_optional_sqlite_timestamp(
            row.try_get("tracing_last_success_at")
                .map_err(|_| storage_error("读取 tracing 成功时间失败"))?,
        )?,
        last_failure_at: parse_optional_sqlite_timestamp(
            row.try_get("tracing_last_failure_at")
                .map_err(|_| storage_error("读取 tracing 失败时间失败"))?,
        )?,
        last_error_code: row
            .try_get("tracing_last_error_code")
            .map_err(|_| storage_error("读取 tracing 错误码失败"))?,
    };
    let heartbeat = Heartbeat {
        service_name,
        process_role,
        instance_id: row
            .try_get("instance_id")
            .map_err(|_| storage_error("读取服务心跳实例失败"))?,
        version: row.try_get("version").map_err(|_| storage_error("读取服务心跳版本失败"))?,
        started_at: parse_sqlite_timestamp(
            &row.try_get::<String, _>("started_at")
                .map_err(|_| storage_error("读取服务心跳启动时间失败"))?,
        )?,
        status: parse_status(
            row.try_get("status").map_err(|_| storage_error("读取服务心跳状态失败"))?,
        )?,
        readiness: row
            .try_get::<i64, _>("readiness")
            .map_err(|_| storage_error("读取服务心跳 readiness 失败"))?
            != 0,
        audit_failure_count: row
            .try_get("audit_failure_count")
            .map_err(|_| storage_error("读取服务心跳审计计数失败"))?,
        tracing,
        observation_sequence: row
            .try_get("observation_sequence")
            .map_err(|_| storage_error("读取服务心跳序列失败"))?,
        observed_at: parse_sqlite_timestamp(
            &row.try_get::<String, _>("observed_at")
                .map_err(|_| storage_error("读取服务心跳观察时间失败"))?,
        )?,
        expires_at: parse_sqlite_timestamp(
            &row.try_get::<String, _>("expires_at")
                .map_err(|_| storage_error("读取服务心跳过期时间失败"))?,
        )?,
    };
    heartbeat
        .validate()
        .then_some(heartbeat)
        .ok_or_else(|| storage_error("SQLite 服务心跳安全投影无效"))
}

fn parse_service(value: String) -> Result<ServiceName, HealthError> {
    ServiceName::parse(&value).ok_or_else(|| storage_error("服务心跳名称无效"))
}

fn parse_role(value: String) -> Result<ProcessRole, HealthError> {
    ProcessRole::parse(&value).ok_or_else(|| storage_error("服务心跳职责无效"))
}

fn parse_status(value: String) -> Result<HeartbeatStatus, HealthError> {
    HeartbeatStatus::parse(&value).ok_or_else(|| storage_error("服务心跳状态无效"))
}

fn parse_tracing(value: String) -> Result<TracingStatus, HealthError> {
    TracingStatus::parse(&value).ok_or_else(|| storage_error("tracing 状态无效"))
}

#[cfg(feature = "sqlite")]
fn sqlite_timestamp(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(feature = "sqlite")]
fn parse_optional_sqlite_timestamp(
    value: Option<String>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, HealthError> {
    value.filter(|value| !value.is_empty()).map(|value| parse_sqlite_timestamp(&value)).transpose()
}

#[cfg(feature = "sqlite")]
fn parse_sqlite_timestamp(value: &str) -> Result<chrono::DateTime<chrono::Utc>, HealthError> {
    use chrono::{DateTime, NaiveDateTime, Utc};

    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(DateTime::from_naive_utc_and_offset(parsed, Utc));
        }
    }
    Err(storage_error("SQLite 服务心跳时间格式无效"))
}

fn storage_error(message: &str) -> HealthError {
    HealthError::Storage { message: message.to_owned() }
}
