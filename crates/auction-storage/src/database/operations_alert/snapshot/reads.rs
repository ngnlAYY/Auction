//! Operations alert 各 owner 水位的安全读取。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use crate::database::DatabaseStore;
use crate::database::operations_alert::storage_error;
use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertCredentialSnapshot, OperationsAlertDeliverySnapshot,
    OperationsAlertForumSnapshot, OperationsAlertFulfillmentSnapshot,
    OperationsAlertLifecycleSnapshot, OperationsAlertMailSnapshot,
    OperationsAlertNotificationSnapshot, OperationsAlertProcessSnapshot,
    OperationsAlertProcessState, OperationsAlertRecoverySnapshot, OperationsAlertRecoveryWorld,
    OperationsAlertSnapshot,
};
use auction_application::reconciliation::AssetReconciliationStore;
use auction_observability::{HeartbeatStatus, HeartbeatStore, ServiceName, TracingStatus};
use chrono::{DateTime, Utc};
use sqlx::Row;

pub(crate) async fn snapshot_postgres(
    store: &super::super::DatabaseStore,
    now: DateTime<Utc>,
) -> Result<OperationsAlertSnapshot, ApplicationError> {
    // 每个 source 独立读取；单个 owner 的查询失败只生成该 source 的 unavailable signal，
    // 不让一个损坏的履约/恢复投影掩盖 process 或 delivery 的告警。
    let heartbeat_store = store.clone();
    let process = process_snapshot(&heartbeat_store).await.ok();
    let delivery = delivery_snapshot(store.pool(), now).await.ok();
    let fulfillment = fulfillment_snapshot(store.pool(), now).await.ok();
    let recovery = recovery_snapshot(store.pool(), now).await.ok();
    let mail = mail_snapshot(store.pool(), now).await.ok();
    let credential = credential_snapshot(store.pool()).await.ok();
    let lifecycle = lifecycle_snapshot(store.pool(), now).await.ok();
    let forum = forum_snapshot(store.pool(), now).await.ok();
    let notification = notification_snapshot(store.pool(), now).await.ok();
    let reconciliation = reconciliation_snapshot(store, now).await.ok();
    Ok(OperationsAlertSnapshot {
        process,
        delivery,
        fulfillment,
        recovery,
        mail,
        credential,
        lifecycle,
        forum,
        notification,
        reconciliation,
    })
}

/// 只读取最近追加的两类报告；报告缺失时保持 `None`，evaluator 明确降级。
async fn reconciliation_snapshot(
    store: &super::super::DatabaseStore,
    now: DateTime<Utc>,
) -> Result<auction_application::operations::OperationsAlertReconciliationSnapshot, ApplicationError>
{
    let reports = store.latest_asset_reports().await?;
    let custody_report_age_seconds =
        reports.custody.as_ref().map(|report| age_seconds(now, Some(report.completed_at)));
    let solvency_report_age_seconds =
        reports.solvency.as_ref().map(|report| age_seconds(now, Some(report.completed_at)));
    let mut worlds = Vec::with_capacity(2);
    for world in ["isekai", "main"] {
        let custody_missing_count = reports
            .custody
            .as_ref()
            .and_then(|report| report.worlds.iter().find(|entry| entry.world == world))
            .map(|entry| entry.missing_count)
            .unwrap_or(0);
        let solvency_shortfall_credits = reports
            .solvency
            .as_ref()
            .and_then(|report| report.worlds.iter().find(|entry| entry.world == world))
            .map(|entry| entry.shortfall_credits)
            .unwrap_or(0);
        worlds.push(auction_application::operations::OperationsAlertReconciliationWorld {
            world,
            custody_missing_count,
            solvency_shortfall_credits,
        });
    }
    Ok(auction_application::operations::OperationsAlertReconciliationSnapshot {
        custody_report_age_seconds,
        solvency_report_age_seconds,
        worlds,
    })
}

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

/// 已处置终态的统一判据：当前 fence 上已有解除异常终态的追加处置事实。
///
/// `delivery_dispositions.resolves_alert` 在写入时按领域闭集固定，交付与通知两个来源共用
/// 同一事实，读侧不再按来源拼接专用排除谓词（Admin 判失败、装备损失、历史自寄、
/// 预检退役与操作者确认各自只写一行处置）。预检退役中当前只有失去资格的关注提醒解除
/// 失败计数，沿用迁移 `000088` 之前的口径；放开其余退役属有意改变判定，须用户批准。
const UNDISPOSED: &str = "NOT EXISTS (
                    SELECT 1 FROM public.delivery_dispositions disposition
                    WHERE disposition.delivery_id = d.id
                      AND disposition.fencing_version = d.fencing_version
                      AND disposition.resolves_alert
                )";

async fn delivery_snapshot(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertDeliverySnapshot, ApplicationError> {
    // 仅完整匹配追加修复事实及原投递身份的 failed 才已解决；不能仅凭 metadata 消除告警。
    // 替换内容只有已审核的编译期 SQL 常量，不插入任何运行时业务值；仅缓存 SQL 文本，不缓存水位。
    static QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        r#"SELECT
            COUNT(*) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status IN ('prepared', 'pending', 'claimed', 'sending')) AS pending,
            COUNT(*) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status IN ('claimed', 'sending')) AS claimed,
            COUNT(*) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status = 'failed'
                -- 已处置终态不再计入，各自以不可变处置事实为证（历史自寄、Admin 判失败、
                -- 装备损失、预检退役、操作者确认）。结算类人工判失败仍由履约水位的 failed 暴露。
                AND /* undisposed */
                ) AS failed,
            COUNT(*) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status = 'manual_reconcile') AS manual_reconcile,
            COUNT(*) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status = 'manual_reconcile'
                AND failure_code = 'recipient_not_found'
                AND result_metadata @>
                    '{"reason_code":"recipient_not_found","take_required":true}'::jsonb)
                AS returned_mail_manual_reconcile,
            MIN(created_at) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status IN ('prepared', 'pending', 'claimed', 'sending')) AS oldest_pending,
            MIN(claimed_at) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status IN ('claimed', 'sending')) AS oldest_claimed,
            MIN(created_at) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status = 'manual_reconcile') AS oldest_manual,
            MIN(created_at) FILTER (WHERE origin_kind <> 'operations_alert'
                AND status = 'manual_reconcile'
                AND failure_code = 'recipient_not_found'
                AND result_metadata @>
                    '{"reason_code":"recipient_not_found","take_required":true}'::jsonb)
                AS oldest_returned_mail,
            MAX(updated_at) FILTER (WHERE origin_kind <> 'operations_alert') AS last_update
         FROM public.deliveries d"#
            .replace("/* undisposed */", UNDISPOSED)
    });
    let row = sqlx::query(sqlx::AssertSqlSafe(QUERY.as_str()))
        .fetch_one(pool)
        .await
        .map_err(|error| storage_error_from(error, "读取 operations alert delivery 水位失败"))?;
    let pending: i64 = row
        .try_get("pending")
        .map_err(|error| storage_error_from(error, "读取 delivery 积压计数失败"))?;
    let failed: i64 = row
        .try_get("failed")
        .map_err(|error| storage_error_from(error, "读取 delivery 失败计数失败"))?;
    let manual_reconcile: i64 = row
        .try_get("manual_reconcile")
        .map_err(|error| storage_error_from(error, "读取 delivery 人工对账计数失败"))?;
    let returned_mail_manual_reconcile: i64 = row
        .try_get("returned_mail_manual_reconcile")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 退信人工对账计数失败"))?;
    Ok(OperationsAlertDeliverySnapshot {
        pending,
        failed,
        manual_reconcile,
        returned_mail_manual_reconcile,
        claimed: row
            .try_get("claimed")
            .map_err(|error| storage_error_from(error, "读取 delivery 已领取计数失败"))?,
        oldest_pending_age_seconds: age_seconds(
            now,
            row.try_get("oldest_pending")
                .map_err(|error| storage_error_from(error, "读取 delivery 积压时间失败"))?,
        ),
        oldest_claimed_age_seconds: age_seconds(
            now,
            row.try_get("oldest_claimed")
                .map_err(|error| storage_error_from(error, "读取 delivery 领取时间失败"))?,
        ),
        oldest_manual_reconcile_age_seconds: age_seconds(
            now,
            row.try_get("oldest_manual")
                .map_err(|error| storage_error_from(error, "读取 delivery 人工对账时间失败"))?,
        ),
        oldest_returned_mail_age_seconds: age_seconds(
            now,
            row.try_get("oldest_returned_mail").map_err(|error| {
                storage_error_from(error, "读取 MoogleMail 退信人工对账时间失败")
            })?,
        ),
        last_update_at: row
            .try_get("last_update")
            .map_err(|error| storage_error_from(error, "读取 delivery 更新时间失败"))?,
    })
}

async fn fulfillment_snapshot(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertFulfillmentSnapshot, ApplicationError> {
    let row = sqlx::query(
        "WITH obligation_state AS (
            SELECT obligation.id, obligation.business_state, obligation.obligation_kind,
                   obligation.prerequisite_obligation_id,
                   COALESCE(fulfillment.execution_state, '') AS execution_state,
                   fulfillment.last_error_code,
                   COALESCE(fulfillment.created_at, obligation.created_at) AS backlog_at,
                   (recovery.id IS NOT NULL
                    AND recovery.stage = 'awaiting_payment'
                    AND recovery.payment_deadline_at > $1
                    AND EXISTS (
                        SELECT 1
                        FROM public.settlement_obligation_fulfillments AS payment_f
                        JOIN public.deliveries AS payment_d
                          ON payment_d.id = payment_f.delivery_id
                        WHERE payment_f.obligation_id = obligation.id
                          AND public.delivery_has_frozen_sent_proof(
                              payment_d.id,
                              payment_d.result_metadata,
                              payment_d.external_reference
                          )
                    )) AS waiting_payment,
                   (obligation.obligation_kind = 'seller_payout'
                    AND obligation.prerequisite_obligation_id IS NOT NULL
                    AND NOT EXISTS (
                        SELECT 1 FROM public.settlement_obligation_fulfillments AS prerequisite
                        WHERE prerequisite.obligation_id = obligation.prerequisite_obligation_id
                          AND prerequisite.execution_state = 'satisfied'
                    )) AS waiting_prerequisite
            FROM public.settlement_obligations AS obligation
            LEFT JOIN public.settlement_obligation_fulfillments AS fulfillment
              ON fulfillment.obligation_id = obligation.id
            LEFT JOIN public.settlement_recoveries AS recovery
              ON recovery.payment_obligation_id = obligation.id
         )
         SELECT
            COUNT(*) FILTER (WHERE business_state = 'ready'
                AND execution_state IN ('pending', 'prepared', 'enqueued')
                AND NOT waiting_payment AND NOT waiting_prerequisite) AS pending,
            COUNT(*) FILTER (WHERE business_state = 'blocked'
                AND execution_state NOT IN ('failed', 'satisfied')
                AND NOT waiting_payment AND NOT waiting_prerequisite) AS blocked,
            COUNT(*) FILTER (WHERE execution_state = 'failed'
                AND NOT (last_error_code = 'operator_external_buyer_compensation'
                    AND EXISTS (SELECT 1 FROM public.settlement_equipment_losses loss
                        WHERE loss.refund_obligation_id = obligation_state.id))) AS failed,
            COUNT(*) FILTER (WHERE execution_state = 'manual_reconcile')
                AS manual_reconcile,
            COUNT(*) FILTER (WHERE business_state = 'ready'
                AND execution_state IN ('pending', 'prepared', 'enqueued')
                AND waiting_payment) AS waiting_payment,
            COUNT(*) FILTER (WHERE waiting_prerequisite
                AND ((business_state = 'ready'
                      AND execution_state IN ('pending', 'prepared', 'enqueued'))
                     OR (business_state = 'blocked'
                         AND execution_state NOT IN ('failed', 'satisfied'))))
                AS waiting_prerequisite,
            MIN(backlog_at) FILTER (WHERE business_state = 'ready'
                AND execution_state IN ('pending', 'prepared', 'enqueued')
                AND NOT waiting_payment AND NOT waiting_prerequisite
                OR (business_state = 'blocked'
                    AND execution_state NOT IN ('failed', 'satisfied')
                    AND NOT waiting_payment AND NOT waiting_prerequisite)) AS oldest_backlog,
            MIN(backlog_at) FILTER (WHERE waiting_prerequisite
                AND ((business_state = 'ready'
                      AND execution_state IN ('pending', 'prepared', 'enqueued'))
                     OR (business_state = 'blocked'
                         AND execution_state NOT IN ('failed', 'satisfied'))))
                AS oldest_waiting_prerequisite
         FROM obligation_state",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 履约水位失败"))?;
    Ok(OperationsAlertFulfillmentSnapshot {
        pending: row
            .try_get("pending")
            .map_err(|error| storage_error_from(error, "读取履约待处理计数失败"))?,
        blocked: row
            .try_get("blocked")
            .map_err(|error| storage_error_from(error, "读取履约阻塞计数失败"))?,
        failed: row
            .try_get("failed")
            .map_err(|error| storage_error_from(error, "读取履约失败计数失败"))?,
        manual_reconcile: row
            .try_get("manual_reconcile")
            .map_err(|error| storage_error_from(error, "读取履约人工对账计数失败"))?,
        waiting_payment: row
            .try_get("waiting_payment")
            .map_err(|error| storage_error_from(error, "读取履约等待付款计数失败"))?,
        waiting_prerequisite: row
            .try_get("waiting_prerequisite")
            .map_err(|error| storage_error_from(error, "读取履约等待前置计数失败"))?,
        oldest_backlog_age_seconds: age_seconds(
            now,
            row.try_get("oldest_backlog")
                .map_err(|error| storage_error_from(error, "读取履约积压时间失败"))?,
        ),
        oldest_prerequisite_wait_age_seconds: age_seconds(
            now,
            row.try_get("oldest_waiting_prerequisite")
                .map_err(|error| storage_error_from(error, "读取履约等待前置时间失败"))?,
        ),
    })
}

async fn recovery_snapshot(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
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
    .map_err(|error| storage_error_from(error, "读取 operations alert recovery 水位失败"))?;
    let mut snapshot = OperationsAlertRecoverySnapshot {
        payment_overdue_count: 0,
        payment_mail_unsent_count: 0,
        relist_pending_count: 0,
        failed_action_count: 0,
        manual_reconcile_count: 0,
        oldest_overdue_age_seconds: 0,
        oldest_payment_mail_unsent_age_seconds: 0,
        worlds: Vec::with_capacity(rows.len()),
    };
    for row in rows {
        let world: String = row
            .try_get("world_id")
            .map_err(|error| storage_error_from(error, "读取 recovery 世界失败"))?;
        let payment_overdue_count: i64 = row
            .try_get("payment_overdue_count")
            .map_err(|error| storage_error_from(error, "读取 recovery 逾期计数失败"))?;
        let relist_pending_count: i64 = row
            .try_get("relist_pending_count")
            .map_err(|error| storage_error_from(error, "读取 recovery 重挂计数失败"))?;
        let failed_action_count: i64 = row
            .try_get("failed_action_count")
            .map_err(|error| storage_error_from(error, "读取 recovery 失败计数失败"))?;
        let manual_reconcile_count: i64 = row
            .try_get("manual_reconcile_count")
            .map_err(|error| storage_error_from(error, "读取 recovery 人工对账计数失败"))?;
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
            .map_err(|error| storage_error_from(error, "读取 recovery 逾期时间失败"))?;
        snapshot.oldest_overdue_age_seconds =
            snapshot.oldest_overdue_age_seconds.max(oldest_overdue_age_seconds);
        let balance_observed_at: Option<DateTime<Utc>> = row
            .try_get("balance_observed_at")
            .map_err(|error| storage_error_from(error, "读取 recovery 余额时间失败"))?;
        let evidence_last_success_at: Option<DateTime<Utc>> = row
            .try_get("evidence_last_success_at")
            .map_err(|error| storage_error_from(error, "读取 recovery 证据时间失败"))?;
        let updated_at: DateTime<Utc> = row
            .try_get("updated_at")
            .map_err(|error| storage_error_from(error, "读取 recovery 更新时间失败"))?;
        snapshot.worlds.push(OperationsAlertRecoveryWorld {
            world,
            balance_credits: row
                .try_get("balance_credits")
                .map_err(|error| storage_error_from(error, "读取 recovery 余额失败"))?,
            balance_status: row
                .try_get("balance_status")
                .map_err(|error| storage_error_from(error, "读取 recovery 余额状态失败"))?,
            evidence_status: row
                .try_get("evidence_status")
                .map_err(|error| storage_error_from(error, "读取 recovery 证据状态失败"))?,
            evidence_age_seconds: age_seconds(now, evidence_last_success_at.or(Some(updated_at))),
            balance_stall_age_seconds: {
                // 只有“忙/不可用”状态才累计停滞时间；最后一次成功余额观测越久越危险。
                let balance_status: String = row
                    .try_get("balance_status")
                    .map_err(|error| storage_error_from(error, "读取 recovery 余额状态失败"))?;
                if matches!(balance_status.as_str(), "busy" | "unavailable" | "unknown") {
                    age_seconds(now, balance_observed_at.or(Some(updated_at)))
                } else {
                    0
                }
            },
            balance_observed_at,
            evidence_last_success_at,
            updated_at,
        });
    }

    // 付款期限已过但付款投递没有发送证明；该判断复用资金/履约的唯一强谓词。
    let unsent = sqlx::query(
        r#"SELECT
               count(*)::bigint AS payment_mail_unsent_count,
               COALESCE(MAX(GREATEST(
                   0,
                   EXTRACT(EPOCH FROM ($1::timestamptz - recovery.payment_deadline_at))::bigint
               )), 0)::bigint AS oldest_payment_mail_unsent_age_seconds
           FROM public.settlement_recoveries AS recovery
           LEFT JOIN public.settlement_obligation_fulfillments AS payment_f
             ON payment_f.obligation_id = recovery.payment_obligation_id
           LEFT JOIN public.deliveries AS payment_d
             ON payment_d.id = payment_f.delivery_id
           WHERE recovery.stage IN ('awaiting_payment', 'payment_overdue')
             AND recovery.payment_deadline_at <= $1
             AND NOT COALESCE(payment_f.execution_state = 'satisfied', false)
             AND NOT COALESCE(public.delivery_has_frozen_sent_proof(
                 payment_d.id, payment_d.result_metadata, payment_d.external_reference
             ), false)"#,
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取付款邮件未寄出水位失败"))?;
    snapshot.payment_mail_unsent_count = unsent
        .try_get("payment_mail_unsent_count")
        .map_err(|error| storage_error_from(error, "读取付款邮件未寄出计数失败"))?;
    snapshot.oldest_payment_mail_unsent_age_seconds = unsent
        .try_get("oldest_payment_mail_unsent_age_seconds")
        .map_err(|error| storage_error_from(error, "读取付款邮件未寄出时间失败"))?;
    Ok(snapshot)
}

fn age_seconds(now: DateTime<Utc>, observed_at: Option<DateTime<Utc>>) -> i64 {
    observed_at.map(|value| now.signed_duration_since(value).num_seconds().max(0)).unwrap_or(0)
}

async fn mail_snapshot(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertMailSnapshot, ApplicationError> {
    // 显式暂停的邮箱不算停滞，Admin 的暂停是决定而不是故障。
    // 没有任何轮询行表示尚未开始轮询（首次轮询会建立行），此时不能凭空判停。
    let row = sqlx::query(
        "SELECT
            COALESCE(GREATEST(0, EXTRACT(EPOCH FROM ($1::timestamptz - MIN(last_poll_at)))::bigint), 0)
                AS last_poll_age,
            COALESCE(MAX(consecutive_failures), 0)::bigint AS consecutive_failures,
            (SELECT MAX(checkpoint.last_full_pages_scanned)::bigint
             FROM public.mail_scan_checkpoints AS checkpoint
             JOIN (
                 SELECT id, state_version
                 FROM public.credential_keys
                 WHERE credential_kind = 'mail_cookie' AND status = 'active'
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1
             ) AS current_credential
               ON current_credential.id = checkpoint.credential_id
              AND current_credential.state_version = checkpoint.credential_version
             WHERE checkpoint.stream = 'sent'
               AND checkpoint.last_full_pages_observed_at >= $1 - interval '7 days')
                AS sent_pages_scanned,
            (SELECT MAX(checkpoint.last_full_page_limit)::bigint
             FROM public.mail_scan_checkpoints AS checkpoint
             JOIN (
                 SELECT id, state_version
                 FROM public.credential_keys
                 WHERE credential_kind = 'mail_cookie' AND status = 'active'
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1
             ) AS current_credential
               ON current_credential.id = checkpoint.credential_id
              AND current_credential.state_version = checkpoint.credential_version
             WHERE checkpoint.stream = 'sent'
               AND checkpoint.last_full_pages_observed_at >= $1 - interval '7 days')
                AS sent_page_limit
         FROM public.mail_poll_state
         WHERE NOT paused",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 收件轮询水位失败"))?;
    Ok(OperationsAlertMailSnapshot {
        last_poll_age_seconds: row
            .try_get("last_poll_age")
            .map_err(|error| storage_error_from(error, "读取收件轮询时间失败"))?,
        consecutive_failures: row
            .try_get("consecutive_failures")
            .map_err(|error| storage_error_from(error, "读取收件轮询失败次数失败"))?,
        sent_pages_scanned: row
            .try_get("sent_pages_scanned")
            .map_err(|error| storage_error_from(error, "读取 Sent 完整枚举页数失败"))?,
        sent_page_limit: row
            .try_get("sent_page_limit")
            .map_err(|error| storage_error_from(error, "读取 Sent 完整枚举页数上限失败"))?,
    })
}

async fn credential_snapshot(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
) -> Result<OperationsAlertCredentialSnapshot, ApplicationError> {
    // 只看每个（种类, scope）最新的凭据行：历史失效行不构成持续告警。
    let row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE credential_kind = 'mail_cookie')::bigint AS invalid_mail_cookie,
            COUNT(*) FILTER (WHERE credential_kind = 'external_api')::bigint AS invalid_external_api
         FROM (
            SELECT DISTINCT ON (credential_kind, scope) credential_kind, status
            FROM public.credential_keys
            ORDER BY credential_kind, scope, created_at DESC, id DESC
         ) AS current
         WHERE status = 'invalid'",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 凭据状态失败"))?;
    Ok(OperationsAlertCredentialSnapshot {
        invalid_mail_cookie: row
            .try_get("invalid_mail_cookie")
            .map_err(|error| storage_error_from(error, "读取邮件凭据失效计数失败"))?,
        invalid_external_api: row
            .try_get("invalid_external_api")
            .map_err(|error| storage_error_from(error, "读取外部凭据失效计数失败"))?,
    })
}

async fn lifecycle_snapshot(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertLifecycleSnapshot, ApplicationError> {
    // 两个 owner 事实：结束时间已过的 live 场次（应进入 Final），以及到期未执行的慢速代理。
    let row = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM public.auctions
                WHERE status = 'live' AND effective_end_at IS NOT NULL AND effective_end_at < $1)
                ::bigint AS phase_overdue,
            COALESCE((SELECT EXTRACT(EPOCH FROM ($1::timestamptz - MIN(effective_end_at)))::bigint
                FROM public.auctions
                WHERE status = 'live' AND effective_end_at IS NOT NULL
                  AND effective_end_at < $1), 0) AS oldest_phase_age,
            (SELECT COUNT(*) FROM public.lots
                WHERE status = 'live' AND slow_due_at IS NOT NULL AND slow_due_at < $1)
                ::bigint AS proxy_overdue,
            COALESCE((SELECT EXTRACT(EPOCH FROM ($1::timestamptz - MIN(slow_due_at)))::bigint
                FROM public.lots
                WHERE status = 'live' AND slow_due_at IS NOT NULL AND slow_due_at < $1), 0)
                AS oldest_proxy_age,
            (SELECT COUNT(DISTINCT obligation.id)
             FROM (
                 SELECT MIN(auction.intake_opens_at) AS at
                 FROM public.auctions AS auction
                 WHERE auction.world_id = 'isekai'
                   AND auction.status <> 'cancelled'
                   AND auction.intake_opens_at > $1
                   AND auction.intake_opens_at <= $1 + interval '7 days'
             ) AS boundary
             JOIN public.settlement_obligations AS obligation
               ON obligation.world_id = 'isekai'
             LEFT JOIN public.settlement_obligation_fulfillments AS fulfillment
               ON fulfillment.obligation_id = obligation.id
             WHERE boundary.at IS NOT NULL
               -- 未触发的 contingent 退款不是当前欠付；failed/manual_reconcile 仍需人工收口。
               -- 装备损失线下补偿已由不可变处置事实收口，不计入换季欠付。
               AND obligation.business_state <> 'contingent'
               AND COALESCE(fulfillment.execution_state, 'pending')
                   IN ('pending', 'prepared', 'enqueued', 'failed', 'manual_reconcile')
               AND NOT (
                   COALESCE(fulfillment.execution_state, 'pending') = 'failed'
                   AND fulfillment.last_error_code = 'operator_external_buyer_compensation'
                   AND EXISTS (
                       SELECT 1
                       FROM public.settlement_equipment_losses AS loss
                       WHERE loss.refund_obligation_id = obligation.id
                   )
               )) AS season_obligations_pending",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 生命周期水位失败"))?;
    Ok(OperationsAlertLifecycleSnapshot {
        phase_overdue_count: row
            .try_get("phase_overdue")
            .map_err(|error| storage_error_from(error, "读取场次逾期计数失败"))?,
        oldest_phase_overdue_age_seconds: row
            .try_get("oldest_phase_age")
            .map_err(|error| storage_error_from(error, "读取场次逾期时间失败"))?,
        proxy_expiry_overdue_count: row
            .try_get("proxy_overdue")
            .map_err(|error| storage_error_from(error, "读取慢速代理逾期计数失败"))?,
        oldest_proxy_overdue_age_seconds: row
            .try_get("oldest_proxy_age")
            .map_err(|error| storage_error_from(error, "读取慢速代理逾期时间失败"))?,
        season_obligations_pending: row
            .try_get("season_obligations_pending")
            .map_err(|error| storage_error_from(error, "读取换季未完成义务计数失败"))?,
    })
}

async fn forum_snapshot(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertForumSnapshot, ApplicationError> {
    // 主题发布失败是终态；私信只有进入 manual_reconcile 才算失败，未开始发送的退役不算。
    let row = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM public.auction_forum_sync WHERE status = 'failed')::bigint
                AS sync_failed,
            COALESCE((SELECT EXTRACT(EPOCH FROM ($1::timestamptz - MIN(updated_at)))::bigint
                FROM public.auction_forum_sync WHERE status = 'failed'), 0) AS oldest_sync_age,
            (SELECT COUNT(*) FROM public.lot_watch_pm_deliveries
                WHERE status = 'manual_reconcile')::bigint AS message_failed,
            COALESCE((SELECT EXTRACT(EPOCH FROM ($1::timestamptz - MIN(updated_at)))::bigint
                FROM public.lot_watch_pm_deliveries
                WHERE status = 'manual_reconcile'), 0) AS oldest_message_age,
            (SELECT sync.remote_page_count::bigint
             FROM public.auction_forum_sync AS sync
             JOIN public.auctions AS auction ON auction.id = sync.auction_id
             WHERE sync.status = 'published'
               AND sync.remote_page_count IS NOT NULL
               AND sync.remote_page_count_observed_at >= $1 - interval '7 days'
               AND (auction.status IN ('preview', 'live', 'final', 'settling')
                    OR (auction.status IN ('settled', 'cancelled')
                        AND sync.terminal_refresh_enabled
                        AND sync.terminal_synced_version IS NULL))
             ORDER BY sync.remote_page_count DESC, sync.auction_id
             LIMIT 1) AS remote_page_count,
            (SELECT sync.remote_page_limit::bigint
             FROM public.auction_forum_sync AS sync
             JOIN public.auctions AS auction ON auction.id = sync.auction_id
             WHERE sync.status = 'published'
               AND sync.remote_page_count IS NOT NULL
               AND sync.remote_page_count_observed_at >= $1 - interval '7 days'
               AND (auction.status IN ('preview', 'live', 'final', 'settling')
                    OR (auction.status IN ('settled', 'cancelled')
                        AND sync.terminal_refresh_enabled
                        AND sync.terminal_synced_version IS NULL))
             ORDER BY sync.remote_page_count DESC, sync.auction_id
             LIMIT 1) AS remote_page_limit",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 论坛失败水位失败"))?;
    Ok(OperationsAlertForumSnapshot {
        sync_failed_count: row
            .try_get("sync_failed")
            .map_err(|error| storage_error_from(error, "读取论坛发布失败计数失败"))?,
        oldest_sync_failed_age_seconds: row
            .try_get("oldest_sync_age")
            .map_err(|error| storage_error_from(error, "读取论坛发布失败时间失败"))?,
        message_failed_count: row
            .try_get("message_failed")
            .map_err(|error| storage_error_from(error, "读取论坛私信失败计数失败"))?,
        oldest_message_failed_age_seconds: row
            .try_get("oldest_message_age")
            .map_err(|error| storage_error_from(error, "读取论坛私信失败时间失败"))?,
        remote_page_count: row
            .try_get("remote_page_count")
            .map_err(|error| storage_error_from(error, "读取论坛远端分页页数失败"))?,
        remote_page_limit: row
            .try_get("remote_page_limit")
            .map_err(|error| storage_error_from(error, "读取论坛远端分页页数上限失败"))?,
    })
}

async fn notification_snapshot(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertNotificationSnapshot, ApplicationError> {
    // 结算与运维告警自身不属于“非结算通知”；已处置终态与 delivery 来源共用同一谓词。
    static QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let mut query = String::from(
            "SELECT
                COUNT(*) FILTER (WHERE d.status = 'failed')::bigint AS failed,
                COALESCE(EXTRACT(EPOCH FROM
                    ($1::timestamptz - MIN(d.created_at) FILTER (WHERE d.status = 'failed'))
                )::bigint, 0) AS oldest_failed_age,
                COUNT(*) FILTER (WHERE d.status = 'failed' AND /* postage */)::bigint AS postage,
                COALESCE(EXTRACT(EPOCH FROM
                    ($1::timestamptz - MIN(d.created_at) FILTER (WHERE d.status = 'failed' AND /* postage */))
                )::bigint, 0) AS oldest_postage_age
             FROM public.deliveries d
             WHERE d.origin_kind NOT IN
                 ('settlement', 'settlement_recovery_action', 'operations_alert')
               AND /* undisposed */",
        );
        query = query.replace("/* undisposed */", UNDISPOSED).replace(
            "/* postage */",
            "d.failure_code IN ('send_rejected', 'send_rejected_after_stage')
                    AND d.result_metadata->>'reason_code' = 'insufficient_postage'",
        );
        query
    });
    let row = sqlx::query(sqlx::AssertSqlSafe(QUERY.as_str()))
        .bind(now)
        .fetch_one(pool)
        .await
        .map_err(|error| storage_error_from(error, "读取 operations alert 通知失败水位失败"))?;
    Ok(OperationsAlertNotificationSnapshot {
        failed_count: row
            .try_get("failed")
            .map_err(|error| storage_error_from(error, "读取通知失败计数失败"))?,
        oldest_failed_age_seconds: row
            .try_get("oldest_failed_age")
            .map_err(|error| storage_error_from(error, "读取通知失败时间失败"))?,
        postage_insufficient_count: row
            .try_get("postage")
            .map_err(|error| storage_error_from(error, "读取邮资不足计数失败"))?,
        oldest_postage_age_seconds: row
            .try_get("oldest_postage_age")
            .map_err(|error| storage_error_from(error, "读取邮资不足时间失败"))?,
    })
}
