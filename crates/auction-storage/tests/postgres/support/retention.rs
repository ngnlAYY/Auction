//! 保留清理演练与执行用例共用的运行态插行夹具：心跳、实时事件、推送失败与两类会话。
//!
//! 只写夹具行，执行器由调用方给出（连接池或单条连接），不经过被测端口。
#![allow(clippy::expect_used)]

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

pub async fn insert_heartbeat(
    executor: impl PgExecutor<'_>,
    instance: &str,
    expires_at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO public.service_heartbeats
            (service_name, process_role, instance_id, version, started_at, status, readiness,
             audit_failure_count, observation_sequence, observed_at, expires_at, details)
         VALUES ('auction-worker', 'worker', $1, 'test', $2, 'ready', true, 0, 1, $2, $3, '{}')",
    )
    .bind(instance)
    .bind(expires_at - TimeDelta::minutes(1))
    .bind(expires_at)
    .execute(executor)
    .await
    .expect("插入心跳");
}

pub async fn insert_event(
    executor: impl PgExecutor<'_>,
    stream: &str,
    sequence: i64,
    created_at: DateTime<Utc>,
    replay_of: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             created_at, replay_of_event_id)
         VALUES ($1, $2, $3, 'auction.list_changed', 1, '{}', $4, $5)",
    )
    .bind(id)
    .bind(stream)
    .bind(sequence)
    .bind(created_at)
    .bind(replay_of)
    .execute(executor)
    .await
    .expect("插入实时事件");
    id
}

pub async fn insert_failure(
    executor: impl PgExecutor<'_>,
    consumer: &str,
    event_id: Uuid,
    failed_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO public.realtime_failures
            (id, consumer_key, event_id, failure_code, safe_detail, failed_at, resolved_at)
         VALUES ($1, $2, $3, 'delivery_failed', 'test', $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(consumer)
    .bind(event_id)
    .bind(failed_at)
    .bind(resolved_at)
    .execute(executor)
    .await
    .expect("插入推送失败");
}

pub async fn insert_public_session(
    executor: impl PgExecutor<'_>,
    user_id: Uuid,
    token: u8,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO public.public_sessions
            (token_hash, user_id, created_at, last_activity_at, expires_at, absolute_expires_at,
             revoked_at)
         VALUES ($1, $2, $3, $3, $4, $4, $5)",
    )
    .bind(vec![token; 32])
    .bind(user_id)
    .bind(created_at)
    .bind(expires_at)
    .bind(revoked_at)
    .execute(executor)
    .await
    .expect("插入 Public 会话");
}

pub async fn insert_admin_session(
    executor: impl PgExecutor<'_>,
    token: u8,
    actor: &str,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO public.admin_sessions
            (token_hash, actor, csrf_hash, created_at, last_activity_at, expires_at,
             absolute_expires_at, revoked_at)
         VALUES ($1, $6, $2, $3, $3, $4, $4, $5)",
    )
    .bind(vec![token; 32])
    .bind(vec![token.wrapping_add(1); 32])
    .bind(created_at)
    .bind(expires_at)
    .bind(revoked_at)
    .bind(actor)
    .execute(executor)
    .await
    .expect("插入 Admin 会话");
}
