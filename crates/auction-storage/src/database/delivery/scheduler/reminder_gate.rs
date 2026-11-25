//! 寄售付款提醒的 sending 围栏复验（7A-2 的派前复验执行点）。
//!
//! 保护不变量 4、6：提醒是一次性通知，排队之后买家可能已付款、合同可能已取消或已有新一轮寄出；
//! 这些情形下不得再进入 `sending`（零发送）。判定只有存储端 `pre_dispatch_in` 一份
//! 实现，围栏只在本事务内复验并拒绝，不改任何行：被拒绝的投递按业务门正常延期，不会被
//! 误记为发送。已进入 sending 或结果未知的提醒由投递层按未知结果核对，永不自动补发。
//! 其他来源与模板直接放行，拍卖投递行为不受影响。
//!
//! 同一复验也供领取前的退役扫描使用（7A-3）：从未进入 sending 的作废提醒在扫描事务内被找出，
//! 由 `watch` 以预检退役收口，避免投递永远停在 pending / claimed 并被反复领取。

use auction_application::delivery::DeliveryWorkClaim;
use auction_domain::consignment::recovery::ReminderDecision;
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::recovery::reminder::pre_dispatch_in;

/// 付款提醒的冻结模板名。
const PAYMENT_REMINDER_TEMPLATE: &str = "consignment_payment_reminder";

/// 复验失败的两种形态：数据库错误（可重试分类）或持久事实失配。
pub(super) enum GateError {
    /// SQL 错误。
    Sqlx(sqlx::Error),
    /// 提醒事实读取失败（序列化冲突或持久不变量失配）。
    Storage(WriteError),
}

impl From<sqlx::Error> for GateError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

/// 领取前扫描一次最多处理的作废提醒数；与其余预检退役扫描的批量一致。
const VOIDED_SCAN_LIMIT: i64 = 100;

/// 该 claim 是否允许进入 sending；非付款提醒恒为真。
pub(super) async fn allows_sending(
    tx: &mut Transaction<'_, Postgres>,
    claim: &DeliveryWorkClaim,
) -> Result<bool, GateError> {
    let is_reminder = claim.origin_kind == "consignment_notice"
        && claim.payload.get("template").and_then(Value::as_str) == Some(PAYMENT_REMINDER_TEMPLATE);
    if !is_reminder {
        return Ok(true);
    }
    let contract: Option<Uuid> = sqlx::query_scalar(
        "SELECT contract_id FROM public.consignment_payment_reminders WHERE delivery_id = $1",
    )
    .bind(claim.delivery_id)
    .fetch_optional(&mut **tx)
    .await?;
    // 没有提醒决定的提醒投递没有业务依据，失败关闭。
    let Some(contract) = contract else { return Ok(false) };
    let decision = pre_dispatch_in(tx, contract).await.map_err(GateError::Storage)?;
    Ok(decision == ReminderDecision::Queue)
}

/// 找出复验结论为作废（已付、已取消、轮已变）且从未进入 sending 的提醒投递，并在本事务内锁住它们。
///
/// 保护不变量 4、6：候选只取 `pending` / `claimed` 且 `sending_at` 为空的行，并以
/// `FOR UPDATE SKIP LOCKED` 与正在执行的 sending 围栏互斥；已进入 sending 或结果未知的提醒不在候选内，
/// 照旧零补发、走人工核对。判定与围栏共用 [`pre_dispatch_in`]，不另立一套状态机。作废依据（付款、
/// 取消处置、新一轮寄出）都是只追加事实，一旦成立不会撤回，所以在扫描事务的隔离级别下判定也不会过期。
/// 读不出合同事实等持久失配以错误返回，由调用方阻断本轮领取（失败关闭），不会把它当成作废。
pub(super) async fn voided_deliveries(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Vec<Uuid>, GateError> {
    let candidates: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT delivery.id, reminder.contract_id
         FROM public.deliveries AS delivery
         JOIN public.consignment_payment_reminders AS reminder ON reminder.delivery_id = delivery.id
         WHERE delivery.status IN ('pending', 'claimed')
           AND delivery.sending_at IS NULL
           AND delivery.origin_kind = 'consignment_notice'
           AND delivery.payload_template = $1
         ORDER BY delivery.created_at, delivery.id
         LIMIT $2
         FOR UPDATE OF delivery SKIP LOCKED",
    )
    .bind(PAYMENT_REMINDER_TEMPLATE)
    .bind(VOIDED_SCAN_LIMIT)
    .fetch_all(&mut **tx)
    .await?;
    let mut voided = Vec::new();
    for (delivery_id, contract_id) in candidates {
        let decision = pre_dispatch_in(tx, contract_id).await.map_err(GateError::Storage)?;
        if matches!(
            decision,
            ReminderDecision::VoidPaid
                | ReminderDecision::VoidCancelled
                | ReminderDecision::VoidRound
        ) {
            voided.push(delivery_id);
        }
    }
    Ok(voided)
}
