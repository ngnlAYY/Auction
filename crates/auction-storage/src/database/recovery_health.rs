//! PostgreSQL recovery health 持久化 owner。
//!
//! health 行不是外部余额的权威来源，而是带时间戳的安全投影。每次刷新同时重算 recovery
//! backlog，避免运维页面把旧计数与新余额状态拼接成不一致快照。

use auction_application::{
    ApplicationError, ApplicationFuture, RecoveryHealthStore, RecoveryHealthUpdate, RepositoryError,
};

use super::DatabaseStore;

impl RecoveryHealthStore for DatabaseStore {
    fn refresh_world_health(&self, update: RecoveryHealthUpdate) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            update.validate().map_err(|message| {
                ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
            })?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => refresh_postgres(store.pool(), &update).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery health 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn refresh_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    update: &RecoveryHealthUpdate,
) -> Result<(), ApplicationError> {
    let world = match update.world {
        auction_domain::SettlementWorld::Main => "main",
        auction_domain::SettlementWorld::Isekai => "isekai",
    };
    let row = sqlx::query(
        r#"WITH recovery_stats AS (
               SELECT
                   count(*) FILTER (WHERE stage = 'payment_overdue')::bigint
                       AS payment_overdue_count,
                   count(*) FILTER (WHERE stage = 'relist_pending')::bigint
                       AS relist_pending_count,
                   coalesce(
                       max(
                           greatest(
                               0,
                               floor(extract(epoch FROM $6::timestamptz - payment_deadline_at))
                           )
                       ) FILTER (WHERE stage = 'payment_overdue'),
                       0
                   )::bigint AS oldest_overdue_age_seconds
               FROM public.settlement_recoveries
               WHERE world_id = $7
           ),
           action_stats AS (
               SELECT
                   count(*) FILTER (WHERE action.execution_state = 'failed')::bigint
                       AS failed_action_count,
                   count(*) FILTER (WHERE action.execution_state = 'manual_reconcile')::bigint
                       AS manual_reconcile_count
               FROM public.settlement_recovery_actions AS action
               JOIN public.settlement_recoveries AS recovery
                 ON recovery.id = action.recovery_id
               WHERE recovery.world_id = $7
           )
           UPDATE public.settlement_recovery_health AS health
           SET balance_credits = coalesce($1, health.balance_credits),
               balance_observed_at = coalesce($2, health.balance_observed_at),
               balance_status = $3,
               evidence_last_success_at = coalesce($4, health.evidence_last_success_at),
               evidence_status = $5,
               payment_overdue_count = recovery_stats.payment_overdue_count,
               relist_pending_count = recovery_stats.relist_pending_count,
               failed_action_count = action_stats.failed_action_count,
               manual_reconcile_count = action_stats.manual_reconcile_count,
               oldest_overdue_age_seconds = recovery_stats.oldest_overdue_age_seconds,
               updated_at = $6
           FROM recovery_stats, action_stats
           WHERE health.world_id = $7
           RETURNING health.world_id"#,
    )
    .bind(update.balance_credits)
    .bind(update.balance_observed_at)
    .bind(update.balance_status.as_str())
    .bind(update.evidence_last_success_at)
    .bind(update.evidence_status.as_str())
    .bind(update.updated_at)
    .bind(world)
    .fetch_optional(pool)
    .await
    .map_err(|_| storage_error("刷新 recovery health 失败"))?;

    if row.is_none() {
        return Err(storage_error("recovery health world 行不存在"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
