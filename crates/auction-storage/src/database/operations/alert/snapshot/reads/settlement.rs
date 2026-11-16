//! 结算面的投递、履约与恢复水位快照。

use auction_application::core::ApplicationError;
use auction_application::operations::{
    MAX_OPERATIONS_ALERT_COLLECTION_OVERDUE_ITEMS, OperationsAlertCollectionOverdueItem,
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot,
    OperationsAlertRecoverySnapshot, OperationsAlertRecoveryWorld,
};
use auction_domain::SettlementObligationKind;
use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{UNDISPOSED, age_seconds};
use crate::database::storage_error;
use crate::database::storage_error_from;

pub(super) async fn delivery_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
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

/// 履约分类 CTE，`$1` 是观察时刻；水位计数与长期未领取清单共用，保证同一口径。
///
/// “等待领取”只拆出已寄出的 Send 履约：COD/载体付款义务沿用“等待买家付款”的既有口径，
/// 等待付款或前置的项保持原分类。拆出后它们不再计入积压，但仍是未满足义务，只换分组展示。
macro_rules! fulfillment_classified_cte {
    () => {
        concat!(
            "WITH obligation_state AS (
            SELECT obligation.id, obligation.business_state, obligation.obligation_kind,
                   obligation.prerequisite_obligation_id, obligation.auction_id,
                   obligation.lot_id, obligation.equipment_id, delivery.recipient,
                   COALESCE(fulfillment.execution_state, '') AS execution_state,
                   fulfillment.last_error_code,
                   COALESCE(fulfillment.created_at, obligation.created_at) AS backlog_at,
                   delivery.completed_at AS sent_at,
                   (obligation.obligation_kind NOT IN (
                        'winner_equipment_cod', 'winner_payment_carrier_cod'
                    )
                    AND COALESCE(",
            crate::database::settlement::fulfillment::sent_awaiting_collection_sql!(
                "fulfillment",
                "delivery"
            ),
            ", false)) AS sent_send,
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
            LEFT JOIN public.deliveries AS delivery
              ON delivery.id = fulfillment.delivery_id
            LEFT JOIN public.settlement_recoveries AS recovery
              ON recovery.payment_obligation_id = obligation.id
         ), classified AS (
            SELECT obligation_state.*,
                   (business_state = 'ready' AND sent_send
                    AND NOT waiting_payment AND NOT waiting_prerequisite) AS awaiting_collection
            FROM obligation_state
         )"
        )
    };
}

/// 履约水位计数；`$2` 是超期截止时刻（观察时刻减长期未领取阈值），可为 NULL。
const FULFILLMENT_COUNTS: &str = concat!(
    fulfillment_classified_cte!(),
    "
         SELECT
            COUNT(*) FILTER (WHERE business_state = 'ready'
                AND execution_state IN ('pending', 'prepared', 'enqueued')
                AND NOT waiting_payment AND NOT waiting_prerequisite
                AND NOT awaiting_collection) AS pending,
            COUNT(*) FILTER (WHERE business_state = 'blocked'
                AND execution_state NOT IN ('failed', 'satisfied')
                AND NOT waiting_payment AND NOT waiting_prerequisite) AS blocked,
            COUNT(*) FILTER (WHERE execution_state = 'failed'
                AND NOT (last_error_code = 'operator_external_buyer_compensation'
                    AND EXISTS (SELECT 1 FROM public.settlement_equipment_losses loss
                        WHERE loss.refund_obligation_id = classified.id))) AS failed,
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
            COUNT(*) FILTER (WHERE awaiting_collection) AS awaiting_collection,
            -- 最近一次 Sent 读取失败只说明“读不到”，不能据此判断收件人是否已领取。
            COUNT(*) FILTER (WHERE awaiting_collection
                AND last_error_code = 'evidence_unavailable') AS awaiting_collection_unreadable,
            MIN(backlog_at) FILTER (WHERE business_state = 'ready'
                AND execution_state IN ('pending', 'prepared', 'enqueued')
                AND NOT waiting_payment AND NOT waiting_prerequisite
                AND NOT awaiting_collection
                OR (business_state = 'blocked'
                    AND execution_state NOT IN ('failed', 'satisfied')
                    AND NOT waiting_payment AND NOT waiting_prerequisite)) AS oldest_backlog,
            MIN(backlog_at) FILTER (WHERE waiting_prerequisite
                AND ((business_state = 'ready'
                      AND execution_state IN ('pending', 'prepared', 'enqueued'))
                     OR (business_state = 'blocked'
                         AND execution_state NOT IN ('failed', 'satisfied'))))
                AS oldest_waiting_prerequisite,
            MIN(sent_at) FILTER (WHERE awaiting_collection) AS oldest_awaiting_collection,
            -- 超期：寄出时刻不晚于观察时刻减阈值；未给出阈值（$2 为 NULL）时恒为 0。
            COUNT(*) FILTER (WHERE awaiting_collection AND sent_at <= $2)
                AS collection_overdue
         FROM classified"
);

/// 长期未领取清单：只列寄出时刻不晚于 `$2` 的等待领取义务，寄出最早优先，最多 `$3` 条。
/// 只取 Admin 可见的业务标识与展示字段，不读投递载荷、凭据或外部错误原文。拍品代码按分类
/// 前缀加分类内编号生成，`lot_number` 取 `category_number`，与 Admin lot 页和结算报表一致；
/// 全场编号会在混合类别的场次里错号。
const COLLECTION_OVERDUE_ITEMS: &str = concat!(
    fulfillment_classified_cte!(),
    "
         SELECT classified.id AS obligation_id, classified.obligation_kind,
                auction.world_id AS auction_world, auction.sequence AS auction_sequence,
                lot.category_number AS lot_number, equipment.category AS equipment_category,
                equipment.name AS equipment_name, classified.recipient, classified.sent_at,
                COALESCE(classified.last_error_code = 'evidence_unavailable', false)
                    AS last_read_failed
         FROM classified
         JOIN public.auctions AS auction ON auction.id = classified.auction_id
         JOIN public.lots AS lot ON lot.id = classified.lot_id
         JOIN public.equipment AS equipment ON equipment.id = classified.equipment_id
         WHERE classified.awaiting_collection AND classified.sent_at <= $2
         ORDER BY classified.sent_at, classified.id
         LIMIT $3"
);

/// SQL 绑定用的清单上限（`LIMIT` 取 bigint）；编译期确认可无损表示。
const COLLECTION_OVERDUE_LIMIT_SQL: i64 = {
    assert!(MAX_OPERATIONS_ALERT_COLLECTION_OVERDUE_ITEMS <= i64::MAX as usize);
    MAX_OPERATIONS_ALERT_COLLECTION_OVERDUE_ITEMS as i64
};

/// 履约水位与长期未领取清单在同一只读快照内读取，超期件数与清单不会因并发收口而互相矛盾。
/// 未给出阈值时不评估长期未领取：超期件数为 0、清单为空。
pub(super) async fn fulfillment_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
    collection_overdue_after: Option<std::time::Duration>,
) -> Result<
    (OperationsAlertFulfillmentSnapshot, Vec<OperationsAlertCollectionOverdueItem>),
    ApplicationError,
> {
    // 超期即寄出时刻不晚于观察时刻减阈值；阈值超出时间范围时失败关闭，不当作“没有超期”。
    let overdue_cutoff = collection_overdue_after
        .map(|threshold| {
            chrono::Duration::from_std(threshold)
                .ok()
                .and_then(|threshold| now.checked_sub_signed(threshold))
                .ok_or_else(|| storage_error("长期未领取阈值超出时间范围"))
        })
        .transpose()?;
    let mut transaction = pool
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "打开 operations alert 履约只读快照失败"))?;
    let row = sqlx::query(FULFILLMENT_COUNTS)
        .bind(now)
        .bind(overdue_cutoff)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取 operations alert 履约水位失败"))?;
    let mut items = Vec::new();
    if overdue_cutoff.is_some() {
        let rows = sqlx::query(COLLECTION_OVERDUE_ITEMS)
            .bind(now)
            .bind(overdue_cutoff)
            .bind(COLLECTION_OVERDUE_LIMIT_SQL)
            .fetch_all(&mut *transaction)
            .await
            .map_err(|error| storage_error_from(error, "读取长期未领取清单失败"))?;
        for row in rows {
            items.push(collection_overdue_item(&row, now)?);
        }
    }
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "关闭 operations alert 履约只读快照失败"))?;
    let snapshot = OperationsAlertFulfillmentSnapshot {
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
        awaiting_collection: row
            .try_get("awaiting_collection")
            .map_err(|error| storage_error_from(error, "读取履约等待领取计数失败"))?,
        awaiting_collection_unreadable: row
            .try_get("awaiting_collection_unreadable")
            .map_err(|error| storage_error_from(error, "读取履约等待领取读取失败计数失败"))?,
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
        oldest_awaiting_collection_age_seconds: age_seconds(
            now,
            row.try_get("oldest_awaiting_collection")
                .map_err(|error| storage_error_from(error, "读取履约等待领取时间失败"))?,
        ),
        collection_overdue: row
            .try_get("collection_overdue")
            .map_err(|error| storage_error_from(error, "读取履约长期未领取计数失败"))?,
    };
    Ok((snapshot, items))
}

/// 长期未领取清单的一行；义务种类按领域闭集解析，未知值失败关闭。
fn collection_overdue_item(
    row: &sqlx::postgres::PgRow,
    now: DateTime<Utc>,
) -> Result<OperationsAlertCollectionOverdueItem, ApplicationError> {
    let column = |error| storage_error_from(error, "读取长期未领取清单失败");
    let kind: String = row.try_get("obligation_kind").map_err(column)?;
    let sent_at: DateTime<Utc> = row.try_get("sent_at").map_err(column)?;
    Ok(OperationsAlertCollectionOverdueItem {
        obligation_id: row.try_get("obligation_id").map_err(column)?,
        obligation_kind: SettlementObligationKind::from_name(&kind)
            .ok_or_else(|| storage_error("长期未领取清单的义务种类未知"))?,
        auction_world: row.try_get("auction_world").map_err(column)?,
        auction_sequence: row.try_get("auction_sequence").map_err(column)?,
        lot_number: row.try_get("lot_number").map_err(column)?,
        equipment_category: row.try_get("equipment_category").map_err(column)?,
        equipment_name: row.try_get("equipment_name").map_err(column)?,
        recipient: row.try_get("recipient").map_err(column)?,
        sent_at,
        age_seconds: age_seconds(now, Some(sent_at)),
        last_read_failed: row.try_get("last_read_failed").map_err(column)?,
    })
}

pub(super) async fn recovery_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
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
