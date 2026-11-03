//! 服务心跳的 PostgreSQL 完整档写入与读取。

use auction_observability::{HealthError, Heartbeat, HeartbeatCommand, TracingHealth};

use super::super::PostgresStore;
use super::mapping::{
    parse_role, parse_service, parse_status, parse_tracing, storage_error, storage_error_from,
};

pub(super) async fn publish_postgres(
    store: &PostgresStore,
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
    .bind(Json(&command.details))
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "写入 PostgreSQL 服务心跳失败"))?;

    if returned.is_some() { Ok(()) } else { Err(HealthError::StaleObservation) }
}

pub(super) async fn list_postgres(store: &PostgresStore) -> Result<Vec<Heartbeat>, HealthError> {
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
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 服务心跳失败"))?;

    rows.into_iter()
        .map(|row| {
            let service_name = parse_service(
                row.try_get("service_name")
                    .map_err(|error| storage_error_from(error, "读取服务心跳名称失败"))?,
            )?;
            let process_role = parse_role(
                row.try_get("process_role")
                    .map_err(|error| storage_error_from(error, "读取服务心跳职责失败"))?,
            )?;
            let status = parse_status(
                row.try_get("status")
                    .map_err(|error| storage_error_from(error, "读取服务心跳状态失败"))?,
            )?;
            let tracing = TracingHealth {
                status: parse_tracing(
                    row.try_get("tracing_status")
                        .map_err(|error| storage_error_from(error, "读取 tracing 状态失败"))?,
                )?,
                dropped_span_count: row
                    .try_get("tracing_dropped_span_count")
                    .map_err(|error| storage_error_from(error, "读取 tracing 丢弃计数失败"))?,
                export_failure_count: row
                    .try_get("tracing_export_failure_count")
                    .map_err(|error| storage_error_from(error, "读取 tracing 失败计数失败"))?,
                last_success_at: row
                    .try_get("tracing_last_success_at")
                    .map_err(|error| storage_error_from(error, "读取 tracing 成功时间失败"))?,
                last_failure_at: row
                    .try_get("tracing_last_failure_at")
                    .map_err(|error| storage_error_from(error, "读取 tracing 失败时间失败"))?,
                last_error_code: row
                    .try_get("tracing_last_error_code")
                    .map_err(|error| storage_error_from(error, "读取 tracing 错误码失败"))?,
            };
            let heartbeat = Heartbeat {
                service_name,
                process_role,
                instance_id: row
                    .try_get("instance_id")
                    .map_err(|error| storage_error_from(error, "读取服务心跳实例失败"))?,
                version: row
                    .try_get("version")
                    .map_err(|error| storage_error_from(error, "读取服务心跳版本失败"))?,
                started_at: row
                    .try_get("started_at")
                    .map_err(|error| storage_error_from(error, "读取服务心跳启动时间失败"))?,
                status,
                readiness: row
                    .try_get("readiness")
                    .map_err(|error| storage_error_from(error, "读取服务心跳 readiness 失败"))?,
                audit_failure_count: row
                    .try_get("audit_failure_count")
                    .map_err(|error| storage_error_from(error, "读取服务心跳审计计数失败"))?,
                tracing,
                observation_sequence: row
                    .try_get("observation_sequence")
                    .map_err(|error| storage_error_from(error, "读取服务心跳序列失败"))?,
                observed_at: row
                    .try_get("observed_at")
                    .map_err(|error| storage_error_from(error, "读取服务心跳观察时间失败"))?,
                expires_at: row
                    .try_get("expires_at")
                    .map_err(|error| storage_error_from(error, "读取服务心跳过期时间失败"))?,
            };
            heartbeat
                .validate()
                .then_some(heartbeat)
                .ok_or_else(|| storage_error("PostgreSQL 服务心跳安全投影无效"))
        })
        .collect()
}
