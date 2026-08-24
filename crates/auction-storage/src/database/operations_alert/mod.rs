//! PostgreSQL 完整档 operations alert 观察、批次和租约适配器。
//!
//! 告警只读取各 owner 已提交的安全水位，不参与竞价、结算或 delivery 的正确性决策。
//! signal、批次和 delivery 在同一 PostgreSQL 事务中建立；Worker 之后仍通过正常的
//! `MailDeliveryEnqueuer` 做幂等确认，数据库状态才是唯一权威。

use std::time::Duration;

use auction_application::{
    ApplicationError, ApplicationFuture, OperationsAlertBatchClaim, OperationsAlertClaimRequest,
    OperationsAlertDeliveryStatus, OperationsAlertDispatchErrorCode, OperationsAlertEnqueuedBatch,
    OperationsAlertError, OperationsAlertHealth, OperationsAlertObserveCommand,
    OperationsAlertSnapshot, OperationsAlertSourceReader, OperationsAlertStore,
};
#[cfg(feature = "postgres")]
use auction_application::{
    OperationsAlertBatchItem, OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot,
    OperationsAlertKind, OperationsAlertProcessSnapshot, OperationsAlertProcessState,
    OperationsAlertRecoverySnapshot, OperationsAlertRecoveryWorld, OperationsAlertSignal,
    OperationsAlertSource, RequestHash, body_digest, canonical_summary, request_hash,
};
#[cfg(feature = "postgres")]
use auction_observability::{HeartbeatStatus, HeartbeatStore, ServiceName, TracingStatus};
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::json;
#[cfg(feature = "postgres")]
use sqlx::Row;
#[cfg(feature = "postgres")]
use uuid::Uuid;

use super::DatabaseStore;
#[cfg(feature = "postgres")]
use super::delivery_audit::{ACTIVATE, DeliveryAudit, PREPARE, append as append_delivery_audit};

#[cfg(feature = "postgres")]
const MAX_ATTEMPTS: u32 = 3;

impl OperationsAlertSourceReader for DatabaseStore {
    fn snapshot(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, OperationsAlertSnapshot> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(storage_error("operations alert 观察时间无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => snapshot_postgres(store, now).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}

impl OperationsAlertStore for DatabaseStore {
    fn observe(&self, command: OperationsAlertObserveCommand) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            command.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => observe_postgres(store.pool(), &command).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn claim_due_batch(
        &self,
        request: OperationsAlertClaimRequest,
    ) -> ApplicationFuture<'_, Option<OperationsAlertBatchClaim>> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => claim_postgres(store.pool(), &request).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn mark_enqueued(
        &self,
        claim: OperationsAlertBatchClaim,
        cooldown: Duration,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            claim
                .validate(auction_application::MAX_OPERATIONS_ALERT_BODY_BYTES)
                .map_err(ApplicationError::from)?;
            if cooldown < Duration::from_secs(1) || cooldown > Duration::from_secs(7 * 24 * 3_600) {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    mark_enqueued_postgres(store.pool(), &claim, cooldown).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn mark_claim_failed(
        &self,
        claim: OperationsAlertBatchClaim,
        error_code: OperationsAlertDispatchErrorCode,
        retry_delay: Duration,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            #[cfg(not(feature = "postgres"))]
            let _ = error_code;
            claim
                .validate(auction_application::MAX_OPERATIONS_ALERT_BODY_BYTES)
                .map_err(ApplicationError::from)?;
            if retry_delay < Duration::from_secs(1) || retry_delay > Duration::from_secs(24 * 3_600)
            {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    mark_claim_failed_postgres(store.pool(), &claim, error_code, retry_delay).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn list_enqueued(
        &self,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<OperationsAlertEnqueuedBatch>> {
        Box::pin(async move {
            if limit == 0 || limit > 64 {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => list_enqueued_postgres(store.pool(), limit).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn mark_terminal(
        &self,
        batch: OperationsAlertEnqueuedBatch,
        status: OperationsAlertDeliveryStatus,
        completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            #[cfg(not(feature = "postgres"))]
            let _ = status;
            if batch.batch_id.is_nil()
                || batch.delivery_id.is_nil()
                || completed_at.timestamp_nanos_opt().is_none()
            {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    mark_terminal_postgres(store.pool(), &batch, status, completed_at).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn health(&self) -> ApplicationFuture<'_, OperationsAlertHealth> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => health_postgres(store.pool()).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Ok(OperationsAlertHealth::disabled()),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn snapshot_postgres(
    store: &super::PostgresStore,
    now: DateTime<Utc>,
) -> Result<OperationsAlertSnapshot, ApplicationError> {
    // 四个 source 独立读取；单个 owner 的查询失败只生成该 source 的 unavailable signal，
    // 不让一个损坏的履约/恢复投影掩盖 process 或 delivery 的告警。
    let heartbeat_store = DatabaseStore::Postgres(store.clone());
    let process = process_snapshot(&heartbeat_store).await.ok();
    let delivery = delivery_snapshot(store.pool(), now).await.ok();
    let fulfillment = fulfillment_snapshot(store.pool(), now).await.ok();
    let recovery = recovery_snapshot(store.pool(), now).await.ok();
    Ok(OperationsAlertSnapshot { process, delivery, fulfillment, recovery })
}

#[cfg(feature = "postgres")]
async fn process_snapshot(
    heartbeat_store: &DatabaseStore,
) -> Result<OperationsAlertProcessSnapshot, ApplicationError> {
    // 复用 health owner 的安全心跳读取；这里不读取 heartbeat details。
    let heartbeats = HeartbeatStore::list_live(heartbeat_store)
        .await
        .map_err(|_| storage_error("读取 operations alert 进程健康快照失败"))?;
    let mut process_states: Vec<Option<OperationsAlertProcessState>> = vec![None, None, None];
    let mut audit_failure_count = 0_i64;
    for heartbeat in heartbeats {
        if !heartbeat.validate() {
            return Err(storage_error("operations alert 进程心跳安全投影无效"));
        }
        audit_failure_count = audit_failure_count
            .checked_add(heartbeat.audit_failure_count)
            .ok_or_else(|| storage_error("operations alert 审计计数溢出"))?;
        let index = match heartbeat.service_name {
            ServiceName::Public => 0,
            ServiceName::Admin => 1,
            ServiceName::Worker => 2,
        };
        let role = heartbeat.process_role.as_str().to_owned();
        let status = if heartbeat.status == HeartbeatStatus::Ready
            && heartbeat.readiness
            && heartbeat.tracing.status != TracingStatus::Degraded
        {
            "ready"
        } else {
            "degraded"
        };
        let replace = process_states[index]
            .as_ref()
            .is_none_or(|current| current.status == "ready" && status == "degraded");
        if replace {
            process_states[index] =
                Some(OperationsAlertProcessState { role, status: status.to_owned() });
        }
    }
    let roles = ["public", "admin", "worker"];
    let processes = process_states
        .into_iter()
        .enumerate()
        .map(|(index, state)| {
            state.unwrap_or_else(|| OperationsAlertProcessState {
                role: roles[index].to_owned(),
                status: "missing".to_owned(),
            })
        })
        .collect();
    Ok(OperationsAlertProcessSnapshot { processes, audit_failure_count })
}

#[cfg(feature = "postgres")]
async fn delivery_snapshot(
    pool: &super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertDeliverySnapshot, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status IN ('prepared', 'pending', 'claimed', 'sending')) AS pending,
            COUNT(*) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status = 'failed') AS failed,
            COUNT(*) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status = 'manual_reconcile') AS manual_reconcile,
            MIN(created_at) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status IN ('prepared', 'pending', 'claimed', 'sending')) AS oldest_pending,
            MIN(created_at) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status = 'manual_reconcile') AS oldest_manual,
            MAX(updated_at) FILTER (WHERE origin_kind <> 'operations_alert') AS last_update",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| storage_error("读取 operations alert delivery 水位失败"))?;
    let pending: i64 =
        row.try_get("pending").map_err(|_| storage_error("读取 delivery 积压计数失败"))?;
    let failed: i64 =
        row.try_get("failed").map_err(|_| storage_error("读取 delivery 失败计数失败"))?;
    let manual_reconcile: i64 = row
        .try_get("manual_reconcile")
        .map_err(|_| storage_error("读取 delivery 人工对账计数失败"))?;
    Ok(OperationsAlertDeliverySnapshot {
        pending,
        failed,
        manual_reconcile,
        oldest_pending_age_seconds: age_seconds(
            now,
            row.try_get("oldest_pending")
                .map_err(|_| storage_error("读取 delivery 积压时间失败"))?,
        ),
        oldest_manual_reconcile_age_seconds: age_seconds(
            now,
            row.try_get("oldest_manual")
                .map_err(|_| storage_error("读取 delivery 人工对账时间失败"))?,
        ),
        last_update_at: row
            .try_get("last_update")
            .map_err(|_| storage_error("读取 delivery 更新时间失败"))?,
    })
}

#[cfg(feature = "postgres")]
async fn fulfillment_snapshot(
    pool: &super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertFulfillmentSnapshot, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE fulfillment.execution_state IN
                ('pending', 'prepared', 'enqueued')) AS pending,
            COUNT(*) FILTER (WHERE obligation.business_state = 'blocked') AS blocked,
            COUNT(*) FILTER (WHERE fulfillment.execution_state = 'failed') AS failed,
            COUNT(*) FILTER (WHERE fulfillment.execution_state = 'manual_reconcile')
                AS manual_reconcile,
            MIN(CASE WHEN fulfillment.execution_state IN ('pending', 'prepared', 'enqueued')
                     OR obligation.business_state = 'blocked'
                THEN COALESCE(fulfillment.created_at, obligation.created_at) END) AS oldest_backlog
         FROM public.settlement_obligations AS obligation
         LEFT JOIN public.settlement_obligation_fulfillments AS fulfillment
           ON fulfillment.obligation_id = obligation.id",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| storage_error("读取 operations alert 履约水位失败"))?;
    Ok(OperationsAlertFulfillmentSnapshot {
        pending: row.try_get("pending").map_err(|_| storage_error("读取履约待处理计数失败"))?,
        blocked: row.try_get("blocked").map_err(|_| storage_error("读取履约阻塞计数失败"))?,
        failed: row.try_get("failed").map_err(|_| storage_error("读取履约失败计数失败"))?,
        manual_reconcile: row
            .try_get("manual_reconcile")
            .map_err(|_| storage_error("读取履约人工对账计数失败"))?,
        oldest_backlog_age_seconds: age_seconds(
            now,
            row.try_get("oldest_backlog").map_err(|_| storage_error("读取履约积压时间失败"))?,
        ),
    })
}

#[cfg(feature = "postgres")]
async fn recovery_snapshot(
    pool: &super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertRecoverySnapshot, ApplicationError> {
    let rows = sqlx::query(
        "SELECT world_id, balance_credits, balance_status, evidence_status,
                balance_observed_at, evidence_last_success_at, updated_at,
                payment_overdue_count, relist_pending_count, failed_action_count,
                manual_reconcile_count, oldest_overdue_age_seconds
         FROM public.settlement_recovery_health
         WHERE world_id IN ('main', 'isekai')
         ORDER BY world_id",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| storage_error("读取 operations alert recovery 水位失败"))?;
    let mut snapshot = OperationsAlertRecoverySnapshot {
        payment_overdue_count: 0,
        relist_pending_count: 0,
        failed_action_count: 0,
        manual_reconcile_count: 0,
        oldest_overdue_age_seconds: 0,
        worlds: Vec::with_capacity(rows.len()),
    };
    for row in rows {
        let world: String =
            row.try_get("world_id").map_err(|_| storage_error("读取 recovery 世界失败"))?;
        let payment_overdue_count: i64 = row
            .try_get("payment_overdue_count")
            .map_err(|_| storage_error("读取 recovery 逾期计数失败"))?;
        let relist_pending_count: i64 = row
            .try_get("relist_pending_count")
            .map_err(|_| storage_error("读取 recovery 重挂计数失败"))?;
        let failed_action_count: i64 = row
            .try_get("failed_action_count")
            .map_err(|_| storage_error("读取 recovery 失败计数失败"))?;
        let manual_reconcile_count: i64 = row
            .try_get("manual_reconcile_count")
            .map_err(|_| storage_error("读取 recovery 人工对账计数失败"))?;
        snapshot.payment_overdue_count = snapshot
            .payment_overdue_count
            .checked_add(payment_overdue_count)
            .ok_or_else(|| storage_error("recovery 逾期计数溢出"))?;
        snapshot.relist_pending_count = snapshot
            .relist_pending_count
            .checked_add(relist_pending_count)
            .ok_or_else(|| storage_error("recovery 重挂计数溢出"))?;
        snapshot.failed_action_count = snapshot
            .failed_action_count
            .checked_add(failed_action_count)
            .ok_or_else(|| storage_error("recovery 失败计数溢出"))?;
        snapshot.manual_reconcile_count = snapshot
            .manual_reconcile_count
            .checked_add(manual_reconcile_count)
            .ok_or_else(|| storage_error("recovery 人工对账计数溢出"))?;
        let oldest_overdue_age_seconds: i64 = row
            .try_get("oldest_overdue_age_seconds")
            .map_err(|_| storage_error("读取 recovery 逾期时间失败"))?;
        snapshot.oldest_overdue_age_seconds =
            snapshot.oldest_overdue_age_seconds.max(oldest_overdue_age_seconds);
        let balance_observed_at: Option<DateTime<Utc>> = row
            .try_get("balance_observed_at")
            .map_err(|_| storage_error("读取 recovery 余额时间失败"))?;
        let evidence_last_success_at: Option<DateTime<Utc>> = row
            .try_get("evidence_last_success_at")
            .map_err(|_| storage_error("读取 recovery 证据时间失败"))?;
        let updated_at: DateTime<Utc> =
            row.try_get("updated_at").map_err(|_| storage_error("读取 recovery 更新时间失败"))?;
        snapshot.worlds.push(OperationsAlertRecoveryWorld {
            world,
            balance_credits: row
                .try_get("balance_credits")
                .map_err(|_| storage_error("读取 recovery 余额失败"))?,
            balance_status: row
                .try_get("balance_status")
                .map_err(|_| storage_error("读取 recovery 余额状态失败"))?,
            evidence_status: row
                .try_get("evidence_status")
                .map_err(|_| storage_error("读取 recovery 证据状态失败"))?,
            evidence_age_seconds: age_seconds(now, evidence_last_success_at.or(Some(updated_at))),
            balance_observed_at,
            evidence_last_success_at,
            updated_at,
        });
    }
    Ok(snapshot)
}

#[cfg(feature = "postgres")]
fn age_seconds(now: DateTime<Utc>, observed_at: Option<DateTime<Utc>>) -> i64 {
    observed_at.map(|value| now.signed_duration_since(value).num_seconds().max(0)).unwrap_or(0)
}

#[cfg(feature = "postgres")]
async fn observe_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    command: &OperationsAlertObserveCommand,
) -> Result<(), ApplicationError> {
    let batch_window = chrono_duration(command.batch_window)?;
    let next_notify_at = command
        .now
        .checked_add_signed(batch_window)
        .ok_or_else(|| storage_error("operations alert 首次通知时间溢出"))?;
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 operations alert 观察事务失败"))?;

    for observation in &command.observations {
        let mut fingerprints = Vec::with_capacity(observation.signals.len());
        for signal in &observation.signals {
            signal.validate().map_err(ApplicationError::from)?;
            if fingerprints.contains(&signal.fingerprint) {
                return rollback_operations_alert(
                    transaction,
                    "operations alert 单轮出现重复 signal",
                )
                .await;
            }
            fingerprints.push(signal.fingerprint);
            let state_version: i64 = sqlx::query_scalar(
                "INSERT INTO public.operations_alert_signals
                    (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
                     resolved_at, last_notified_at, next_notify_at, count_value, age_seconds,
                     state_version)
                 VALUES ($1, $2, $3, $4, true, $5, $5, NULL, NULL, $6, $7, $8, 1)
                 ON CONFLICT (fingerprint) DO UPDATE SET
                    active = true,
                    last_seen_at = EXCLUDED.last_seen_at,
                    resolved_at = NULL,
                    next_notify_at = COALESCE(
                        operations_alert_signals.next_notify_at, EXCLUDED.next_notify_at
                    ),
                    count_value = EXCLUDED.count_value,
                    age_seconds = EXCLUDED.age_seconds,
                    state_version = operations_alert_signals.state_version + 1
                 WHERE operations_alert_signals.kind = EXCLUDED.kind
                   AND operations_alert_signals.scope = EXCLUDED.scope
                   AND operations_alert_signals.source = EXCLUDED.source
                 RETURNING state_version",
            )
            .bind(signal.fingerprint.to_vec())
            .bind(signal.kind.as_str())
            .bind(&signal.scope)
            .bind(signal.source.as_str())
            .bind(command.now)
            .bind(next_notify_at)
            .bind(signal.count)
            .bind(signal.age_seconds)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| storage_error("写入 operations alert signal 失败"))?;
            if state_version <= 0 {
                return rollback_operations_alert(transaction, "operations alert signal 版本无效")
                    .await;
            }
        }

        let active_rows = sqlx::query(
            "SELECT fingerprint
             FROM public.operations_alert_signals
             WHERE source = $1 AND active
             FOR UPDATE",
        )
        .bind(observation.source.as_str())
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 operations alert 当前 signal 失败"))?;
        for row in active_rows {
            let fingerprint: Vec<u8> = row
                .try_get("fingerprint")
                .map_err(|_| storage_error("读取 operations alert fingerprint 失败"))?;
            if !fingerprints.iter().any(|value| value.as_slice() == fingerprint.as_slice()) {
                sqlx::query(
                    "UPDATE public.operations_alert_signals
                     SET active = false, resolved_at = $2, next_notify_at = NULL,
                         state_version = state_version + 1
                     WHERE fingerprint = $1 AND active",
                )
                .bind(fingerprint)
                .bind(command.now)
                .execute(&mut *transaction)
                .await
                .map_err(|_| storage_error("关闭已恢复 operations alert signal 失败"))?;
            }
        }
    }

    transaction.commit().await.map_err(|_| storage_error("提交 operations alert 观察事务失败"))
}

#[cfg(feature = "postgres")]
async fn rollback_operations_alert(
    transaction: sqlx::Transaction<'_, sqlx::Postgres>,
    message: &str,
) -> Result<(), ApplicationError> {
    let _ = transaction.rollback().await;
    Err(storage_error(message))
}

#[cfg(feature = "postgres")]
fn chrono_duration(value: Duration) -> Result<chrono::Duration, ApplicationError> {
    chrono::Duration::from_std(value).map_err(|_| storage_error("operations alert 时间范围溢出"))
}

fn storage_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "sqlite")]
fn capability_error() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("operations alert 仅支持 PostgreSQL 完整档")
}

#[cfg(feature = "postgres")]
async fn claim_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    request: &OperationsAlertClaimRequest,
) -> Result<Option<OperationsAlertBatchClaim>, ApplicationError> {
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 operations alert 领取事务失败"))?;

    sqlx::query(
        "UPDATE public.operations_alert_batches
         SET status = 'failed', claim_token = NULL, claimed_at = NULL,
             claim_expires_at = NULL, next_attempt_at = NULL,
             delivery_error_code = 'retry_exhausted', updated_at = $1, completed_at = $1
         WHERE status = 'claimed' AND claim_expires_at <= $1 AND attempt_count >= 3",
    )
    .bind(request.now)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("收口 operations alert 过期重试批次失败"))?;

    let reclaim_row = sqlx::query(
        "SELECT id, command_id, delivery_id, request_digest, body_digest,
                claim_fencing, attempt_count, status
         FROM public.operations_alert_batches
         WHERE (status = 'pending' AND next_attempt_at IS NOT NULL AND next_attempt_at <= $1)
            OR (status = 'claimed' AND claim_expires_at IS NOT NULL
                AND claim_expires_at <= $1 AND attempt_count < 3)
         ORDER BY created_at, id
         LIMIT 1
         FOR UPDATE SKIP LOCKED",
    )
    .bind(request.now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 operations alert 重试批次失败"))?;
    if let Some(row) = reclaim_row
        && let Some(claim) = reclaim_batch(&mut transaction, &row, request).await?
    {
        transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 operations alert 重试领取事务失败"))?;
        return Ok(Some(claim));
    }

    let rows = sqlx::query(
        "SELECT signal.fingerprint, signal.kind, signal.scope, signal.source,
                signal.count_value, signal.age_seconds, signal.state_version
         FROM public.operations_alert_signals AS signal
         WHERE signal.active
           AND signal.next_notify_at <= $1
           AND NOT EXISTS (
               SELECT 1
               FROM public.operations_alert_batch_items AS item
               INNER JOIN public.operations_alert_batches AS batch
                 ON batch.id = item.batch_id
               WHERE item.fingerprint = signal.fingerprint
                 AND batch.status IN ('pending', 'claimed')
           )
         ORDER BY CASE signal.kind
             WHEN 'source_unavailable' THEN 1
             WHEN 'service_missing' THEN 2
             WHEN 'delivery_manual_reconcile' THEN 3
             WHEN 'fulfillment_manual_reconcile' THEN 4
             WHEN 'recovery_manual_reconcile' THEN 5
             WHEN 'recovery_failed' THEN 6
             WHEN 'fulfillment_failed' THEN 7
             WHEN 'audit_failure' THEN 8
             ELSE 9
         END, signal.kind, signal.scope, signal.fingerprint
         LIMIT $2
         FOR UPDATE OF signal SKIP LOCKED",
    )
    .bind(request.now)
    .bind(i64::try_from(request.max_items).map_err(|_| OperationsAlertError::Invalid)?)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 operations alert due signal 失败"))?;
    if rows.is_empty() {
        transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 operations alert 空领取事务失败"))?;
        return Ok(None);
    }

    let mut items = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let signal = signal_from_row(row)?;
        items.push(OperationsAlertBatchItem {
            fingerprint: signal.fingerprint,
            signal_version: row
                .try_get("state_version")
                .map_err(|_| storage_error("读取 operations alert signal 版本失败"))?,
            ordinal: i32::try_from(index + 1)
                .map_err(|_| storage_error("operations alert 批次序号溢出"))?,
            summary: signal.render_summary().map_err(ApplicationError::from)?,
        });
    }
    let summary = loop {
        match canonical_summary(&items, request.max_body_bytes) {
            Ok(value) => break value,
            Err(error) if items.len() > 1 => {
                items.pop();
                let _ = error;
            }
            Err(error) => return Err(error.into()),
        }
    };
    let batch_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/mail-delivery-v1/{command_id}").as_bytes(),
    );
    let request_hash = request_hash(batch_id, delivery_id, command_id, &request.target, &summary)?;
    let claim_token = Uuid::new_v4();
    let body_digest = body_digest(&summary);
    let external_mail_id = format!("operations-alert:{batch_id}");
    let origin_id = format!("operations_alert:{batch_id}");
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": request.target.world_id,
        "external_mail_id": external_mail_id,
        "template": "operations_alert",
        "result_code": "operations_alert",
        "user_key_user_id": null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
        "body": summary,
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient,
             payload, status, created_at, updated_at)
         VALUES ($1, $2, $3, 'operations_alert', $4, $5, $6, 'prepared', $7, $7)",
    )
    .bind(delivery_id)
    .bind(command_id)
    .bind(request_hash.as_bytes().to_vec())
    .bind(&origin_id)
    .bind(&request.target.recipient)
    .bind(payload)
    .bind(request.now)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入 operations alert delivery 失败"))?;
    sqlx::query(
        "INSERT INTO public.operations_alert_batches
            (id, command_id, delivery_id, request_digest, status, claim_token,
             claim_fencing, claimed_at, claim_expires_at, next_attempt_at,
             attempt_count, item_count, body_digest, delivery_error_code,
             created_at, updated_at, completed_at)
         VALUES ($1, $2, $3, $4, 'claimed', $5, 1, $6, $7, NULL,
                 1, $8, $9, '', $6, $6, NULL)",
    )
    .bind(batch_id)
    .bind(command_id)
    .bind(delivery_id)
    .bind(request_hash.as_bytes().to_vec())
    .bind(claim_token)
    .bind(request.now)
    .bind(request.lease_until)
    .bind(i32::try_from(items.len()).map_err(|_| OperationsAlertError::Invalid)? as i32)
    .bind(body_digest.to_vec())
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入 operations alert 批次失败"))?;
    for item in &items {
        sqlx::query(
            "INSERT INTO public.operations_alert_batch_items
                (batch_id, fingerprint, signal_version, ordinal, rendered_summary)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(batch_id)
        .bind(item.fingerprint.to_vec())
        .bind(item.signal_version)
        .bind(item.ordinal)
        .bind(&item.summary)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("写入 operations alert 批次条目失败"))?;
    }
    let request_id = command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: request.now,
            actor_type: "system",
            actor_id: "delivery-service",
            action: PREPARE,
            delivery_id,
            request_id: Some(&request_id),
            source: "system",
            metadata: json!({
                "origin_kind": "operations_alert",
                "status": "prepared",
            }),
        },
    )
    .await
    .map_err(|_| storage_error("追加 operations alert delivery 准备审计失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 operations alert 领取事务失败"))?;
    Ok(Some(OperationsAlertBatchClaim {
        batch_id,
        command_id,
        delivery_id,
        request_hash,
        body_digest,
        summary,
        items,
        claim_token,
        claim_fencing: 1,
        lease_until: request.lease_until,
        attempt_count: 1,
    }))
}

#[cfg(feature = "postgres")]
async fn reclaim_batch(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &sqlx::postgres::PgRow,
    request: &OperationsAlertClaimRequest,
) -> Result<Option<OperationsAlertBatchClaim>, ApplicationError> {
    let batch_id: Uuid =
        row.try_get("id").map_err(|_| storage_error("读取 operations alert 批次 ID 失败"))?;
    let attempt_count: i32 = row
        .try_get("attempt_count")
        .map_err(|_| storage_error("读取 operations alert 尝试次数失败"))?;
    if attempt_count < 1 {
        return Err(storage_error("operations alert 尝试次数无效"));
    }
    if u32::try_from(attempt_count).ok().is_none_or(|value| value >= MAX_ATTEMPTS) {
        sqlx::query(
            "UPDATE public.operations_alert_batches
             SET status = 'failed', completed_at = $2, updated_at = $2,
                 claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL,
                 next_attempt_at = NULL, delivery_error_code = 'retry_exhausted'
             WHERE id = $1 AND status IN ('pending', 'claimed')",
        )
        .bind(batch_id)
        .bind(request.now)
        .execute(&mut **transaction)
        .await
        .map_err(|_| storage_error("写入 operations alert 重试耗尽状态失败"))?;
        return Ok(None);
    }
    let command_id: Uuid = row
        .try_get("command_id")
        .map_err(|_| storage_error("读取 operations alert 命令 ID 失败"))?;
    let delivery_id: Uuid = row
        .try_get("delivery_id")
        .map_err(|_| storage_error("读取 operations alert delivery ID 失败"))?;
    let request_digest: Vec<u8> = row
        .try_get("request_digest")
        .map_err(|_| storage_error("读取 operations alert 请求摘要失败"))?;
    let body_digest_value: Vec<u8> = row
        .try_get("body_digest")
        .map_err(|_| storage_error("读取 operations alert 正文摘要失败"))?;
    let claim_fencing: i64 = row
        .try_get("claim_fencing")
        .map_err(|_| storage_error("读取 operations alert fencing 失败"))?;
    let delivery_target: Option<(String, serde_json::Value)> =
        sqlx::query_as("SELECT recipient, payload FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|_| storage_error("读取 operations alert delivery 目标失败"))?;
    let (recipient, payload) =
        delivery_target.ok_or_else(|| storage_error("operations alert delivery 不存在"))?;
    let payload_world = payload.get("world").and_then(serde_json::Value::as_str);
    if recipient != request.target.recipient
        || payload_world != Some(request.target.world_id.as_str())
    {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict));
    }
    let item_rows = sqlx::query(
        "SELECT fingerprint, signal_version, ordinal, rendered_summary
         FROM public.operations_alert_batch_items
         WHERE batch_id = $1 ORDER BY ordinal",
    )
    .bind(batch_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 operations alert 批次条目失败"))?;
    let items = map_batch_items(item_rows)?;
    let summary = canonical_summary(&items, auction_application::MAX_OPERATIONS_ALERT_BODY_BYTES)
        .map_err(ApplicationError::from)?;
    let request_hash = request_hash_from_bytes(request_digest)?;
    let stored_body_digest = digest_from_bytes(body_digest_value)?;
    if body_digest(&summary) != stored_body_digest {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt));
    }
    let next_attempt = u32::try_from(attempt_count)
        .map_err(|_| storage_error("operations alert 尝试次数无效"))?
        .checked_add(1)
        .ok_or_else(|| storage_error("operations alert 尝试次数溢出"))?;
    let claim_fencing = claim_fencing
        .checked_add(1)
        .ok_or_else(|| storage_error("operations alert fencing 溢出"))?;
    let claim_token = Uuid::new_v4();
    let updated = sqlx::query(
        "UPDATE public.operations_alert_batches
         SET status = 'claimed', claim_token = $2, claim_fencing = $3,
             claimed_at = $4, claim_expires_at = $5, next_attempt_at = NULL,
             attempt_count = $6, delivery_error_code = '', updated_at = $4
         WHERE id = $1 AND status IN ('pending', 'claimed')",
    )
    .bind(batch_id)
    .bind(claim_token)
    .bind(claim_fencing)
    .bind(request.now)
    .bind(request.lease_until)
    .bind(i32::try_from(next_attempt).map_err(|_| OperationsAlertError::Invalid)?)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("更新 operations alert 重试租约失败"))?;
    if updated.rows_affected() != 1 {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim));
    }
    let claim = OperationsAlertBatchClaim {
        batch_id,
        command_id,
        delivery_id,
        request_hash,
        body_digest: stored_body_digest,
        summary,
        items,
        claim_token,
        claim_fencing,
        lease_until: request.lease_until,
        attempt_count: next_attempt,
    };
    claim
        .validate(auction_application::MAX_OPERATIONS_ALERT_BODY_BYTES)
        .map_err(ApplicationError::from)?;
    Ok(Some(claim))
}

#[cfg(feature = "postgres")]
fn signal_from_row(row: &sqlx::postgres::PgRow) -> Result<OperationsAlertSignal, ApplicationError> {
    let fingerprint: Vec<u8> = row
        .try_get("fingerprint")
        .map_err(|_| storage_error("读取 operations alert signal fingerprint 失败"))?;
    let kind: String =
        row.try_get("kind").map_err(|_| storage_error("读取 operations alert signal 类型失败"))?;
    let scope: String = row
        .try_get("scope")
        .map_err(|_| storage_error("读取 operations alert signal 作用域失败"))?;
    let source: String = row
        .try_get("source")
        .map_err(|_| storage_error("读取 operations alert signal 来源失败"))?;
    let count: i64 = row
        .try_get("count_value")
        .map_err(|_| storage_error("读取 operations alert signal 计数失败"))?;
    let age_seconds: i64 = row
        .try_get("age_seconds")
        .map_err(|_| storage_error("读取 operations alert signal 年龄失败"))?;
    let kind =
        parse_kind(&kind).ok_or_else(|| storage_error("operations alert signal 类型未知"))?;
    let source =
        parse_source(&source).ok_or_else(|| storage_error("operations alert signal 来源未知"))?;
    let signal = OperationsAlertSignal::new(kind, scope, source, count, age_seconds)
        .map_err(ApplicationError::from)?;
    if signal.fingerprint.as_slice() != fingerprint.as_slice() {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt));
    }
    Ok(signal)
}

#[cfg(feature = "postgres")]
fn map_batch_items(
    rows: Vec<sqlx::postgres::PgRow>,
) -> Result<Vec<OperationsAlertBatchItem>, ApplicationError> {
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let fingerprint = digest_from_bytes(
            row.try_get("fingerprint")
                .map_err(|_| storage_error("读取 operations alert 条目 fingerprint 失败"))?,
        )?;
        result.push(OperationsAlertBatchItem {
            fingerprint,
            signal_version: row
                .try_get("signal_version")
                .map_err(|_| storage_error("读取 operations alert 条目版本失败"))?,
            ordinal: row
                .try_get("ordinal")
                .map_err(|_| storage_error("读取 operations alert 条目序号失败"))?,
            summary: row
                .try_get("rendered_summary")
                .map_err(|_| storage_error("读取 operations alert 条目摘要失败"))?,
        });
    }
    if result.is_empty() {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt));
    }
    Ok(result)
}

#[cfg(feature = "postgres")]
fn digest_from_bytes(value: Vec<u8>) -> Result<[u8; 32], ApplicationError> {
    value
        .try_into()
        .map_err(|_| ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt))
}

#[cfg(feature = "postgres")]
fn request_hash_from_bytes(value: Vec<u8>) -> Result<RequestHash, ApplicationError> {
    Ok(RequestHash::new(digest_from_bytes(value)?))
}

#[cfg(feature = "postgres")]
fn parse_kind(value: &str) -> Option<OperationsAlertKind> {
    Some(match value {
        "service_missing" => OperationsAlertKind::ServiceMissing,
        "service_degraded" => OperationsAlertKind::ServiceDegraded,
        "audit_failure" => OperationsAlertKind::AuditFailure,
        "source_unavailable" => OperationsAlertKind::SourceUnavailable,
        "delivery_pending" => OperationsAlertKind::DeliveryPending,
        "delivery_manual_reconcile" => OperationsAlertKind::DeliveryManualReconcile,
        "fulfillment_backlog" => OperationsAlertKind::FulfillmentBacklog,
        "fulfillment_failed" => OperationsAlertKind::FulfillmentFailed,
        "fulfillment_manual_reconcile" => OperationsAlertKind::FulfillmentManualReconcile,
        "recovery_payment_overdue" => OperationsAlertKind::RecoveryPaymentOverdue,
        "recovery_relist_pending" => OperationsAlertKind::RecoveryRelistPending,
        "recovery_failed" => OperationsAlertKind::RecoveryFailed,
        "recovery_manual_reconcile" => OperationsAlertKind::RecoveryManualReconcile,
        "balance_low" => OperationsAlertKind::BalanceLow,
        "evidence_stale" => OperationsAlertKind::EvidenceStale,
        _ => return None,
    })
}

#[cfg(feature = "postgres")]
fn parse_source(value: &str) -> Option<OperationsAlertSource> {
    Some(match value {
        "process" => OperationsAlertSource::Process,
        "delivery" => OperationsAlertSource::Delivery,
        "fulfillment" => OperationsAlertSource::Fulfillment,
        "recovery" => OperationsAlertSource::Recovery,
        _ => return None,
    })
}

#[cfg(feature = "postgres")]
async fn mark_enqueued_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    claim: &OperationsAlertBatchClaim,
    cooldown: Duration,
) -> Result<(), ApplicationError> {
    let updated_at = Utc::now();
    let next_notify_at = updated_at
        .checked_add_signed(chrono_duration(cooldown)?)
        .ok_or_else(|| storage_error("operations alert 冷却时间溢出"))?;
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 operations alert 入队确认事务失败"))?;
    let updated = sqlx::query(
        "UPDATE public.operations_alert_batches
         SET status = 'enqueued', claim_token = NULL, claimed_at = NULL,
             claim_expires_at = NULL, next_attempt_at = NULL, delivery_error_code = '',
             updated_at = $5
         WHERE id = $1 AND delivery_id = $2 AND status = 'claimed'
           AND claim_token = $3 AND claim_fencing = $4
           AND claim_expires_at > $5",
    )
    .bind(claim.batch_id)
    .bind(claim.delivery_id)
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .bind(updated_at)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入 operations alert 入队状态失败"))?;
    if updated.rows_affected() != 1 {
        let _ = transaction.rollback().await;
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim));
    }
    let delivery_updated = sqlx::query(
        "UPDATE public.deliveries
         SET status = 'pending', updated_at = $2
         WHERE id = $1 AND origin_kind = 'operations_alert' AND status = 'prepared'",
    )
    .bind(claim.delivery_id)
    .bind(updated_at)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入 operations alert delivery 待发送状态失败"))?;
    if delivery_updated.rows_affected() != 1 {
        let _ = transaction.rollback().await;
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim));
    }
    sqlx::query(
        "UPDATE public.operations_alert_signals AS signal
         SET last_notified_at = $2, next_notify_at = $3,
             state_version = signal.state_version + 1
         FROM public.operations_alert_batch_items AS item
         WHERE item.batch_id = $1 AND item.fingerprint = signal.fingerprint
           AND signal.active",
    )
    .bind(claim.batch_id)
    .bind(updated_at)
    .bind(next_notify_at)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入 operations alert signal 冷却时间失败"))?;
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: updated_at,
            actor_type: "system",
            actor_id: "delivery-service",
            action: ACTIVATE,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "system",
            metadata: json!({"status": "pending"}),
        },
    )
    .await
    .map_err(|_| storage_error("追加 operations alert delivery 激活审计失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 operations alert 入队确认事务失败"))
}

#[cfg(feature = "postgres")]
async fn mark_claim_failed_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    claim: &OperationsAlertBatchClaim,
    error_code: OperationsAlertDispatchErrorCode,
    retry_delay: Duration,
) -> Result<(), ApplicationError> {
    let updated_at = Utc::now();
    let terminal = error_code == OperationsAlertDispatchErrorCode::DependencyMismatch
        || claim.attempt_count >= MAX_ATTEMPTS;
    let retry_at = if terminal {
        None
    } else {
        Some(
            updated_at
                .checked_add_signed(chrono_duration(retry_delay)?)
                .ok_or_else(|| storage_error("operations alert 重试时间溢出"))?,
        )
    };
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 operations alert 失败退避事务失败"))?;
    let updated = if terminal {
        let terminal_code = if error_code == OperationsAlertDispatchErrorCode::DependencyMismatch {
            error_code.as_str()
        } else {
            "retry_exhausted"
        };
        sqlx::query(
            "UPDATE public.operations_alert_batches
             SET status = 'failed', claim_token = NULL, claimed_at = NULL,
                 claim_expires_at = NULL, next_attempt_at = NULL,
                 delivery_error_code = $3, completed_at = $4, updated_at = $4
             WHERE id = $1 AND delivery_id = $2 AND status = 'claimed'
               AND claim_token = $5 AND claim_fencing = $6
               AND claim_expires_at > $4",
        )
        .bind(claim.batch_id)
        .bind(claim.delivery_id)
        .bind(terminal_code)
        .bind(updated_at)
        .bind(claim.claim_token)
        .bind(claim.claim_fencing)
        .execute(&mut *transaction)
        .await
    } else {
        let retry_at =
            retry_at.ok_or_else(|| storage_error("operations alert 可重试批次缺少退避时间"))?;
        sqlx::query(
            "UPDATE public.operations_alert_batches
             SET status = 'pending', claim_token = NULL, claimed_at = NULL,
                 claim_expires_at = NULL, next_attempt_at = $3,
                 delivery_error_code = '', completed_at = NULL, updated_at = $4
             WHERE id = $1 AND delivery_id = $2 AND status = 'claimed'
               AND claim_token = $5 AND claim_fencing = $6
               AND claim_expires_at > $4 AND attempt_count < 3",
        )
        .bind(claim.batch_id)
        .bind(claim.delivery_id)
        .bind(retry_at)
        .bind(updated_at)
        .bind(claim.claim_token)
        .bind(claim.claim_fencing)
        .execute(&mut *transaction)
        .await
    }
    .map_err(|_| storage_error("写入 operations alert 失败退避状态失败"))?;
    if updated.rows_affected() != 1 {
        let _ = transaction.rollback().await;
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim));
    }
    transaction.commit().await.map_err(|_| storage_error("提交 operations alert 失败退避事务失败"))
}

#[cfg(feature = "postgres")]
async fn list_enqueued_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    limit: u16,
) -> Result<Vec<OperationsAlertEnqueuedBatch>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT batch.id AS batch_id, batch.delivery_id, delivery.status
         FROM public.operations_alert_batches AS batch
         INNER JOIN public.deliveries AS delivery ON delivery.id = batch.delivery_id
         WHERE batch.status = 'enqueued'
           AND delivery.status IN ('succeeded', 'failed', 'manual_reconcile')
         ORDER BY delivery.completed_at, batch.id
         LIMIT $1",
    )
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|_| storage_error("读取 operations alert 已入队对账项失败"))?;
    rows.into_iter()
        .map(|row| {
            let status: String = row
                .try_get("status")
                .map_err(|_| storage_error("读取 operations alert delivery 状态失败"))?;
            let delivery_status = parse_delivery_status(&status)
                .ok_or_else(|| storage_error("operations alert delivery 终态未知"))?;
            Ok(OperationsAlertEnqueuedBatch {
                batch_id: row
                    .try_get("batch_id")
                    .map_err(|_| storage_error("读取 operations alert 批次 ID 失败"))?,
                delivery_id: row
                    .try_get("delivery_id")
                    .map_err(|_| storage_error("读取 operations alert delivery ID 失败"))?,
                delivery_status,
            })
        })
        .collect()
}

#[cfg(feature = "postgres")]
fn parse_delivery_status(value: &str) -> Option<OperationsAlertDeliveryStatus> {
    match value {
        "succeeded" => Some(OperationsAlertDeliveryStatus::Succeeded),
        "failed" => Some(OperationsAlertDeliveryStatus::Failed),
        "manual_reconcile" => Some(OperationsAlertDeliveryStatus::ManualReconcile),
        _ => None,
    }
}

#[cfg(feature = "postgres")]
async fn mark_terminal_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    batch: &OperationsAlertEnqueuedBatch,
    status: OperationsAlertDeliveryStatus,
    completed_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    if batch.delivery_status != status {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict));
    }
    let stored_error = match status {
        OperationsAlertDeliveryStatus::Succeeded => "",
        OperationsAlertDeliveryStatus::Failed => "delivery_failed",
        OperationsAlertDeliveryStatus::ManualReconcile => "delivery_manual_reconcile",
    };
    let updated = sqlx::query(
        "UPDATE public.operations_alert_batches
         SET status = $3, completed_at = $4, updated_at = $4,
             delivery_error_code = $5
         WHERE id = $1 AND delivery_id = $2 AND status = 'enqueued'
           AND EXISTS (
               SELECT 1 FROM public.deliveries AS delivery
               WHERE delivery.id = $2 AND delivery.status = $3
           )",
    )
    .bind(batch.batch_id)
    .bind(batch.delivery_id)
    .bind(status.as_str())
    .bind(completed_at)
    .bind(stored_error)
    .execute(pool)
    .await
    .map_err(|_| storage_error("写入 operations alert 终态失败"))?;
    if updated.rows_affected() != 1 {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn health_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
) -> Result<OperationsAlertHealth, ApplicationError> {
    let signal_row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE active) AS active_signal_count,
            COUNT(*) FILTER (WHERE active AND next_notify_at <= CURRENT_TIMESTAMP)
                AS due_signal_count,
            MAX(last_seen_at) AS last_observed_at
         FROM public.operations_alert_signals",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| storage_error("读取 operations alert signal 健康失败"))?;
    let batch_row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE status = 'claimed') AS claimed_batch_count,
            COUNT(*) FILTER (WHERE status = 'succeeded') AS succeeded_batch_count,
            COUNT(*) FILTER (WHERE status = 'failed') AS failed_batch_count,
            COUNT(*) FILTER (WHERE status = 'manual_reconcile') AS manual_reconcile_count,
            MAX(completed_at) FILTER (WHERE status = 'succeeded') AS last_success_at,
            MAX(completed_at) FILTER (WHERE status IN ('failed', 'manual_reconcile'))
                AS last_failure_at
         FROM public.operations_alert_batches",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| storage_error("读取 operations alert 批次健康失败"))?;
    let last_error_code: Option<String> = sqlx::query_scalar(
        "SELECT delivery_error_code
         FROM public.operations_alert_batches
         WHERE status IN ('failed', 'manual_reconcile')
         ORDER BY completed_at DESC, id DESC
         LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| storage_error("读取 operations alert 最近错误失败"))?;
    let health = OperationsAlertHealth {
        enabled: true,
        active_signal_count: signal_row
            .try_get("active_signal_count")
            .map_err(|_| storage_error("读取 operations alert 活跃计数失败"))?,
        due_signal_count: signal_row
            .try_get("due_signal_count")
            .map_err(|_| storage_error("读取 operations alert due 计数失败"))?,
        claimed_batch_count: batch_row
            .try_get("claimed_batch_count")
            .map_err(|_| storage_error("读取 operations alert claim 计数失败"))?,
        succeeded_batch_count: batch_row
            .try_get("succeeded_batch_count")
            .map_err(|_| storage_error("读取 operations alert 成功计数失败"))?,
        failed_batch_count: batch_row
            .try_get("failed_batch_count")
            .map_err(|_| storage_error("读取 operations alert 失败计数失败"))?,
        manual_reconcile_count: batch_row
            .try_get("manual_reconcile_count")
            .map_err(|_| storage_error("读取 operations alert 人工对账计数失败"))?,
        last_observed_at: signal_row
            .try_get("last_observed_at")
            .map_err(|_| storage_error("读取 operations alert 最近观察时间失败"))?,
        last_success_at: batch_row
            .try_get("last_success_at")
            .map_err(|_| storage_error("读取 operations alert 最近成功时间失败"))?,
        last_failure_at: batch_row
            .try_get("last_failure_at")
            .map_err(|_| storage_error("读取 operations alert 最近失败时间失败"))?,
        last_error_code: last_error_code.unwrap_or_default(),
    };
    health.validate().map_err(ApplicationError::from).map(|_| health)
}
