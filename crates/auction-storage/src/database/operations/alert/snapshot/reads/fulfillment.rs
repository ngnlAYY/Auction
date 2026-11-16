//! 结算履约水位与长期未领取清单：一条只读语句完成分类、计数与清单，口径与扫描范围见
//! `FULFILLMENT_SNAPSHOT`。

use auction_application::core::ApplicationError;
use auction_application::operations::{
    MAX_OPERATIONS_ALERT_COLLECTION_OVERDUE_ITEMS, OperationsAlertCollectionOverdueItem,
    OperationsAlertCollectionOverdueRead, OperationsAlertFulfillmentSnapshot,
};
use auction_domain::SettlementObligationKind;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::age_seconds;
use crate::database::storage_error;
use crate::database::storage_error_from;

/// 履约水位与长期未领取清单的单条只读查询：`$1` 是观察时刻，`$2` 是超期截止时刻（观察时刻
/// 减长期未领取阈值；NULL 表示不评估超期），`$3` 决定是否读清单，`$4` 是清单上限。一条语句
/// 即一个快照，超期件数与清单不会因并发收口而互相矛盾。
///
/// 扫描范围：只有尚未满足的履约且义务为 `ready`/`blocked`、或履约停在 `failed`/
/// `manual_reconcile` 的行，以及缺少履约行的 `blocked` 义务，才可能计入任何水位；已满足的历史
/// 与从未激活的或有退款不进入分类。尚未满足的履约经迁移 000110 的部分索引
/// `settlement_obligation_fulfillments_open_idx` 读取；活动集合先收成数组，后续联表按主键逐行
/// 查找，不随历史顺序扫描投递表。
///
/// 分类只算一次（`classified` 物化），计数、各最老时刻与清单都读这一份结果。等待付款要求恢复
/// 记录在付款期内，且本义务自身的投递有冻结发送证明（履约以义务为主键、投递以履约为唯一引用，
/// 即原“本义务履约 JOIN 其投递”的存在判定）；等待前置即前置义务的履约不是 `satisfied`（含缺
/// 行）。两者都改为按主键联表，不再在每个计数里重复展开子查询。
///
/// “等待领取”只拆出已寄出的 Send 履约：COD/载体付款义务沿用“等待买家付款”的既有口径，等待
/// 付款或前置的项保持原分类。拆出后它们不再计入积压，但仍是未满足义务，只换分组展示。清单只取
/// Admin 可见的业务标识与展示字段，不读投递载荷、凭据或外部错误原文；拍品代码按分类前缀加分类
/// 内编号生成，`lot_number` 取 `category_number`，与 Admin lot 页和结算报表一致。
const FULFILLMENT_SNAPSHOT: &str = concat!(
    "WITH live_ids AS (
        SELECT ARRAY(
            SELECT open.obligation_id
            FROM public.settlement_obligation_fulfillments AS open
            JOIN public.settlement_obligations AS obligation
              ON obligation.id = open.obligation_id
            WHERE open.execution_state <> 'satisfied'
              AND (obligation.business_state IN ('ready', 'blocked')
                   OR open.execution_state IN ('failed', 'manual_reconcile'))
            UNION ALL
            SELECT obligation.id
            FROM public.settlement_obligations AS obligation
            WHERE obligation.business_state = 'blocked'
              AND NOT EXISTS (
                  SELECT 1 FROM public.settlement_obligation_fulfillments AS existing
                  WHERE existing.obligation_id = obligation.id
              )
        ) AS ids
     ), classified AS MATERIALIZED (
        SELECT obligation.id, obligation.business_state, obligation.obligation_kind,
               obligation.auction_id, obligation.lot_id, obligation.equipment_id,
               delivery.recipient,
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
                AND delivery.id IS NOT NULL
                AND COALESCE(public.delivery_has_frozen_sent_proof(
                    delivery.id, delivery.result_metadata, delivery.external_reference
                ), false)) AS waiting_payment,
               (obligation.obligation_kind = 'seller_payout'
                AND obligation.prerequisite_obligation_id IS NOT NULL
                AND prerequisite.execution_state IS DISTINCT FROM 'satisfied')
                   AS waiting_prerequisite,
               (fulfillment.execution_state = 'failed'
                AND fulfillment.last_error_code = 'operator_external_buyer_compensation'
                AND EXISTS (SELECT 1 FROM public.settlement_equipment_losses AS loss
                    WHERE loss.refund_obligation_id = obligation.id)) AS failure_disposed
        FROM public.settlement_obligations AS obligation
        LEFT JOIN public.settlement_obligation_fulfillments AS fulfillment
          ON fulfillment.obligation_id = obligation.id
        LEFT JOIN public.deliveries AS delivery
          ON delivery.id = fulfillment.delivery_id
        LEFT JOIN public.settlement_recoveries AS recovery
          ON recovery.payment_obligation_id = obligation.id
        LEFT JOIN public.settlement_obligation_fulfillments AS prerequisite
          ON prerequisite.obligation_id = obligation.prerequisite_obligation_id
        -- 转成数组表达式：`= ANY (子查询)` 会按集合比较，数组需显式标注类型。
        WHERE obligation.id = ANY ((SELECT ids FROM live_ids)::uuid[])
     ), grouped AS (
        SELECT classified.*,
               (business_state = 'ready' AND sent_send
                AND NOT waiting_payment AND NOT waiting_prerequisite) AS awaiting_collection
        FROM classified
     ), overdue AS (
        SELECT grouped.id, grouped.obligation_kind, grouped.auction_id, grouped.lot_id,
               grouped.equipment_id, grouped.recipient, grouped.sent_at,
               COALESCE(grouped.last_error_code = 'evidence_unavailable', false)
                   AS last_read_failed
        FROM grouped
        WHERE $3 AND grouped.awaiting_collection AND grouped.sent_at <= $2
        ORDER BY grouped.sent_at, grouped.id
        LIMIT $4
     ), items AS (
        SELECT
            COALESCE(array_agg(overdue.id ORDER BY overdue.sent_at, overdue.id), '{}')
                AS item_obligation_ids,
            COALESCE(array_agg(overdue.obligation_kind ORDER BY overdue.sent_at, overdue.id),
                '{}') AS item_obligation_kinds,
            COALESCE(array_agg(auction.world_id ORDER BY overdue.sent_at, overdue.id), '{}')
                AS item_auction_worlds,
            COALESCE(array_agg(auction.sequence ORDER BY overdue.sent_at, overdue.id), '{}')
                AS item_auction_sequences,
            COALESCE(array_agg(lot.category_number ORDER BY overdue.sent_at, overdue.id), '{}')
                AS item_lot_numbers,
            COALESCE(array_agg(equipment.category ORDER BY overdue.sent_at, overdue.id), '{}')
                AS item_equipment_categories,
            COALESCE(array_agg(equipment.name ORDER BY overdue.sent_at, overdue.id), '{}')
                AS item_equipment_names,
            COALESCE(array_agg(overdue.recipient ORDER BY overdue.sent_at, overdue.id), '{}')
                AS item_recipients,
            COALESCE(array_agg(overdue.sent_at ORDER BY overdue.sent_at, overdue.id), '{}')
                AS item_sent_ats,
            COALESCE(array_agg(overdue.last_read_failed ORDER BY overdue.sent_at, overdue.id),
                '{}') AS item_last_read_failures
        FROM overdue
        JOIN public.auctions AS auction ON auction.id = overdue.auction_id
        JOIN public.lots AS lot ON lot.id = overdue.lot_id
        JOIN public.equipment AS equipment ON equipment.id = overdue.equipment_id
     )
     SELECT counts.*, items.*
     FROM (
        SELECT
            COUNT(*) FILTER (WHERE business_state = 'ready'
                AND execution_state IN ('pending', 'prepared', 'enqueued')
                AND NOT waiting_payment AND NOT waiting_prerequisite
                AND NOT awaiting_collection) AS pending,
            COUNT(*) FILTER (WHERE business_state = 'blocked'
                AND execution_state NOT IN ('failed', 'satisfied')
                AND NOT waiting_payment AND NOT waiting_prerequisite) AS blocked,
            COUNT(*) FILTER (WHERE execution_state = 'failed' AND NOT failure_disposed)
                AS failed,
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
            -- 等待领取按积压时刻口径的最老年龄；只供 Admin 看板缺省响应按已发布口径并回积压。
            MIN(backlog_at) FILTER (WHERE awaiting_collection)
                AS oldest_awaiting_collection_backlog,
            -- 超期：寄出时刻不晚于观察时刻减阈值；未给出阈值（$2 为 NULL）时恒为 0。
            COUNT(*) FILTER (WHERE awaiting_collection AND sent_at <= $2)
                AS collection_overdue
        FROM grouped
     ) AS counts
     CROSS JOIN items"
);

/// SQL 绑定用的清单上限（`LIMIT` 取 bigint）；编译期确认可无损表示。
const COLLECTION_OVERDUE_LIMIT_SQL: i64 = {
    assert!(MAX_OPERATIONS_ALERT_COLLECTION_OVERDUE_ITEMS <= i64::MAX as usize);
    MAX_OPERATIONS_ALERT_COLLECTION_OVERDUE_ITEMS as i64
};

/// 履约水位与长期未领取清单；读取范围由 `collection` 决定（只有 Admin 看板读清单）。不评估
/// 长期未领取时超期件数为 0、清单为空。
pub(super) async fn fulfillment_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
    collection: OperationsAlertCollectionOverdueRead,
) -> Result<
    (OperationsAlertFulfillmentSnapshot, Vec<OperationsAlertCollectionOverdueItem>),
    ApplicationError,
> {
    // 超期即寄出时刻不晚于观察时刻减阈值；阈值超出时间范围时失败关闭，不当作“没有超期”。
    let overdue_cutoff = collection
        .threshold()
        .map(|threshold| {
            chrono::Duration::from_std(threshold)
                .ok()
                .and_then(|threshold| now.checked_sub_signed(threshold))
                .ok_or_else(|| storage_error("长期未领取阈值超出时间范围"))
        })
        .transpose()?;
    let row = sqlx::query(FULFILLMENT_SNAPSHOT)
        .bind(now)
        .bind(overdue_cutoff)
        .bind(collection.with_items() && overdue_cutoff.is_some())
        .bind(COLLECTION_OVERDUE_LIMIT_SQL)
        .fetch_one(pool)
        .await
        .map_err(|error| storage_error_from(error, "读取 operations alert 履约水位失败"))?;
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
        oldest_awaiting_collection_backlog_age_seconds: age_seconds(
            now,
            row.try_get("oldest_awaiting_collection_backlog")
                .map_err(|error| storage_error_from(error, "读取履约等待领取积压时间失败"))?,
        ),
        collection_overdue: row
            .try_get("collection_overdue")
            .map_err(|error| storage_error_from(error, "读取履约长期未领取计数失败"))?,
    };
    Ok((snapshot, collection_overdue_items(&row, now)?))
}

/// 长期未领取清单：同一行里按寄出最早优先排好的并列数组，逐项拼回；数组长度不一致或义务种类
/// 不在领域闭集内时失败关闭。
fn collection_overdue_items(
    row: &sqlx::postgres::PgRow,
    now: DateTime<Utc>,
) -> Result<Vec<OperationsAlertCollectionOverdueItem>, ApplicationError> {
    let column = |error| storage_error_from(error, "读取长期未领取清单失败");
    let obligation_ids: Vec<Uuid> = row.try_get("item_obligation_ids").map_err(column)?;
    let kinds: Vec<String> = row.try_get("item_obligation_kinds").map_err(column)?;
    let worlds: Vec<String> = row.try_get("item_auction_worlds").map_err(column)?;
    let sequences: Vec<i64> = row.try_get("item_auction_sequences").map_err(column)?;
    let lot_numbers: Vec<i32> = row.try_get("item_lot_numbers").map_err(column)?;
    let categories: Vec<String> = row.try_get("item_equipment_categories").map_err(column)?;
    let names: Vec<String> = row.try_get("item_equipment_names").map_err(column)?;
    let recipients: Vec<String> = row.try_get("item_recipients").map_err(column)?;
    let sent_ats: Vec<DateTime<Utc>> = row.try_get("item_sent_ats").map_err(column)?;
    let read_failures: Vec<bool> = row.try_get("item_last_read_failures").map_err(column)?;
    let count = obligation_ids.len();
    if [
        kinds.len(),
        worlds.len(),
        sequences.len(),
        lot_numbers.len(),
        categories.len(),
        names.len(),
        recipients.len(),
        sent_ats.len(),
        read_failures.len(),
    ]
    .iter()
    .any(|length| *length != count)
    {
        return Err(storage_error("长期未领取清单的列长度不一致"));
    }
    let mut kinds = kinds.into_iter();
    let mut worlds = worlds.into_iter();
    let mut sequences = sequences.into_iter();
    let mut lot_numbers = lot_numbers.into_iter();
    let mut categories = categories.into_iter();
    let mut names = names.into_iter();
    let mut recipients = recipients.into_iter();
    let mut sent_ats = sent_ats.into_iter();
    let mut read_failures = read_failures.into_iter();
    let mut items = Vec::with_capacity(count);
    for obligation_id in obligation_ids {
        let (
            Some(kind),
            Some(auction_world),
            Some(auction_sequence),
            Some(lot_number),
            Some(equipment_category),
            Some(equipment_name),
            Some(recipient),
            Some(sent_at),
            Some(last_read_failed),
        ) = (
            kinds.next(),
            worlds.next(),
            sequences.next(),
            lot_numbers.next(),
            categories.next(),
            names.next(),
            recipients.next(),
            sent_ats.next(),
            read_failures.next(),
        )
        else {
            return Err(storage_error("长期未领取清单的列长度不一致"));
        };
        items.push(OperationsAlertCollectionOverdueItem {
            obligation_id,
            obligation_kind: SettlementObligationKind::from_name(&kind)
                .ok_or_else(|| storage_error("长期未领取清单的义务种类未知"))?,
            auction_world,
            auction_sequence,
            lot_number,
            equipment_category,
            equipment_name,
            recipient,
            sent_at,
            age_seconds: age_seconds(now, Some(sent_at)),
            last_read_failed,
        });
    }
    Ok(items)
}
