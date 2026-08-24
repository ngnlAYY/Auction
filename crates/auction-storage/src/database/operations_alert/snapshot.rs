//! Operations alert 来源快照和 signal 观察写入。
//!
//! 各来源独立失败并降级为 unavailable 投影，观察结果在 PostgreSQL 事务中追加或收敛。

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn snapshot_postgres(
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
pub(super) async fn observe_postgres(
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
