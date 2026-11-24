//! 持久准入、Worker 心跳与事务内暂停事实。

use auction_application::consignment::{ListingAdmission, LookupPermitStatus};
use auction_application::runtime_switches::RuntimeSwitchName;
use auction_time::Timestamp;
use sqlx::PgConnection;

use super::control::{lock_control, read_admission};
use super::protocol::{LEASE_GRACE, WORKER_READY_TTL};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;

pub(super) async fn permit_status(store: &DatabaseStore) -> Result<LookupPermitStatus, WriteError> {
    let mut conn = store.pool().acquire().await?;
    Ok(read_admission(&mut conn).await?.permit)
}

pub(super) async fn sync(
    conn: &mut PgConnection,
    enabled: bool,
) -> Result<ListingAdmission, WriteError> {
    let control = lock_control(conn).await?;
    let epoch = if control.admitting == enabled {
        control.enabled_epoch
    } else {
        // 不变量 2、5、6：关闭、换代和失效心跳同提交；原许可仍隔离可能在途的请求。
        sqlx::query_scalar(
            "UPDATE public.consignment_lookup_control c
             SET admitting = $1, enabled_epoch = enabled_epoch + CASE WHEN $1 THEN 0 ELSE 1 END,
                 worker_ready_until = NULL,
                 permit_until = CASE WHEN $1 THEN c.permit_until ELSE GREATEST(c.permit_until, (
                     SELECT w.network_deadline + make_interval(secs => $2)
                     FROM public.consignment_lookup_work w WHERE w.id = c.holder_work_id
                         AND w.fence = c.holder_fence AND w.token = c.holder_token
                 )) END
             WHERE c.id = 1 RETURNING enabled_epoch",
        )
        .bind(enabled)
        .bind(LEASE_GRACE.as_secs_f64())
        .fetch_one(conn)
        .await?
    };
    Ok(ListingAdmission { service_epoch: epoch.cast_unsigned(), admitting: enabled })
}

pub(super) async fn record_ready(
    conn: &mut PgConnection,
    service_epoch: i64,
) -> Result<(), WriteError> {
    let control = lock_control(conn).await?;
    // 不变量 5、6：旧代和关闭状态均不能续心跳，锁定之后才取墙钟。
    if control.admitting && control.enabled_epoch == service_epoch {
        sqlx::query(
            "UPDATE public.consignment_lookup_control
             SET worker_ready_until = clock_timestamp() + make_interval(secs => $1) WHERE id = 1",
        )
        .bind(WORKER_READY_TTL.as_secs_f64())
        .execute(conn)
        .await?;
    }
    Ok(())
}

pub(super) async fn lock_pause(conn: &mut PgConnection) -> Result<(), WriteError> {
    // 不变量 2、5：与 runtime_switches::transaction::write 获取同一名称锁；
    // 历史事实是追加的，对最近一行取普通行锁无法串行化下一次写入。
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
        .bind(format!("runtime_switch:{}", RuntimeSwitchName::ExternalAccessPaused.label()))
        .execute(conn)
        .await?;
    Ok(())
}

pub(super) async fn paused_at(conn: &mut PgConnection, at: Timestamp) -> Result<bool, WriteError> {
    let latest: Option<(String, Option<serde_json::Value>, Option<Timestamp>)> = sqlx::query_as(
        "SELECT operation, value, expires_at FROM public.runtime_switch_events
         WHERE switch_name = $1 ORDER BY revision DESC LIMIT 1",
    )
    .bind(RuntimeSwitchName::ExternalAccessPaused.label())
    .fetch_optional(conn)
    .await?;
    match latest {
        None => Ok(false),
        Some((operation, value, expires)) => match (operation.as_str(), value) {
            ("set", Some(serde_json::Value::Bool(value))) => {
                Ok(value && expires.is_none_or(|expires| expires > at))
            }
            ("clear" | "expire", None) => Ok(false),
            _ => Err(WriteError::invariant("consignment_lookup_pause_shape")),
        },
    }
}

pub(super) async fn db_now(conn: &mut PgConnection) -> Result<Timestamp, WriteError> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(conn).await?)
}
