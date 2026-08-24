//! 服务心跳的 SQLite Lite 写入、读取与时间格式映射。

use auction_observability::{HealthError, Heartbeat, HeartbeatCommand, TracingHealth};

use super::super::SqliteStore;
use super::mapping::{parse_role, parse_service, parse_status, parse_tracing, storage_error};

pub(super) async fn publish_sqlite(
    store: &SqliteStore,
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

pub(super) async fn list_sqlite(store: &SqliteStore) -> Result<Vec<Heartbeat>, HealthError> {
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

fn sqlite_timestamp(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn parse_optional_sqlite_timestamp(
    value: Option<String>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, HealthError> {
    value.filter(|value| !value.is_empty()).map(|value| parse_sqlite_timestamp(&value)).transpose()
}

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
