//! Admin 确认 `equipment_recovered`（isekai 已付款、交付被退回、装备经精确 Take 证明收回）时，在
//! 确认事务内收口不再可执行的依赖履约：从未开始的卖方货款收口为 `failed/recovery_sale_voided`，
//! 已寄出又被收回的 isekai 装备交付收口为 `failed/recovery_equipment_recovered`，并在原交付投递
//! 当前 fence 上追加 `recovery_equipment_recovered` 统一处置事实。已满足的载体付款与其资金证明
//! 保持不变，买方退款由确认事务另行激活。
//!
//! 只复用既有 `failed` 执行状态与明确失败码；原交付投递、发送证明、Take 证明与资金事实不改写。
//! 原交付与本轮 Take 的链路不精确相符、卖方货款已开始或已有支出、存在更新派发或未解决写入，或
//! 已消费的退件不能把交付每一次可能外达的发送逐一归属（含本轮退件之后又有发送）时拒绝确认，
//! 整个确认事务回滚，交给人工核对。

use auction_application::recovery::{RecoveryCommandError, RecoveryConfirmationCommand};
use auction_time::Timestamp;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::closure::{ClosureAudit, SALE_VOIDED_CODE, append_closure_audit, fail_fulfillment};
use crate::database::TrackedTransaction as Transaction;
use crate::database::delivery::disposition::{DeliveryDispositionFact, append};
use crate::database::recovery::commands::binding::return_attributable_to_sends;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::recovery::commands::scope::{ActionRow, RecoveryRow};
use crate::database::settlement::fulfillment::recovered_handoff_sent_proof_sql;

/// 已寄出又被收回的 isekai 装备交付的失败码，也是处置事实的原因码。
const EQUIPMENT_RECOVERED_CODE: &str = "recovery_equipment_recovered";

/// 卖方货款仍从未开始：履约 `pending`，没有投递、准备或激活身份，没有以它为来源的投递，资金
/// 分配（若有）仍是未绑定投递的 `available`。`$1` 是卖方货款义务。
const SELLER_UNTOUCHED: &str = r#"SELECT EXISTS (
    SELECT 1
    FROM public.settlement_obligation_fulfillments AS fulfillment
    WHERE fulfillment.obligation_id = $1
      AND fulfillment.execution_state = 'pending'
      AND fulfillment.delivery_id IS NULL
      AND fulfillment.prepare_command_id IS NULL
      AND fulfillment.activate_command_id IS NULL
      AND NOT EXISTS (
          SELECT 1 FROM public.deliveries AS delivery
          WHERE delivery.origin_kind = 'settlement' AND delivery.origin_id = $1::text
      )
      AND NOT EXISTS (
          SELECT 1 FROM public.settlement_funding_allocations AS allocation
          WHERE allocation.obligation_id = $1
            AND (allocation.state <> 'available' OR allocation.delivery_id IS NOT NULL)
      )
)"#;

/// 原交付与本轮 Take 的精确链路：`$1` 原交付投递、`$2` 交付义务、`$3` 选中的 Take 动作、`$4`
/// 恢复记录、`$5` 恢复记录的装备。
///
/// - 原交付是该义务唯一的派发，是已在 isekai 成功寄出的 Send，并有可信 Sent 证明（与内部退款
///   校验、资产冲突门共用同一谓词）；
/// - 系统退件绑定该交付义务、属于 isekai，且已被本恢复记录消费，正是恢复记录当前的退件；
/// - Take 的来源邮件就是这封退件，证明种类、引用（退件 MID）与恢复记录的 custody 摘要一致，预期
///   只取回这件装备（零 Credits、零 COD），Take 投递按同一 MID 在 isekai 成功。
const RECOVERED_HANDOFF_CHAIN: &str = concat!(
    r#"SELECT EXISTS (
    SELECT 1
    FROM public.deliveries AS handoff
    JOIN public.settlement_recoveries AS recovery ON recovery.id = $4
    JOIN public.settlement_recovery_actions AS take
      ON take.id = $3 AND take.recovery_id = recovery.id
    JOIN public.deliveries AS take_delivery ON take_delivery.id = take.delivery_id
    JOIN public.inbound_mail AS returned ON returned.id = take.source_mail_id
    JOIN public.equipment AS equipment ON equipment.id = $5
    WHERE handoff.id = $1
      AND handoff.origin_kind = 'settlement'
      AND handoff.origin_id = $2::text
      AND handoff.payload_operation = 'send'
      AND handoff.payload_world = 'isekai'
      AND "#,
    recovered_handoff_sent_proof_sql!("handoff"),
    r#"
      AND NOT EXISTS (
          SELECT 1 FROM public.deliveries AS other
          WHERE other.origin_kind = 'settlement' AND other.origin_id = $2::text
            AND other.id <> handoff.id
      )
      AND returned.id = recovery.return_mail_id
      AND returned.world_id = 'isekai'
      AND returned.mail_kind = 'delivery'
      AND returned.status = 'accepted'
      AND returned.decision_code = 'settlement_recovery_returned'
      AND returned.decision_metadata->>'obligation_id' = $2::text
      AND EXISTS (
          SELECT 1 FROM public.settlement_recovery_return_consumptions AS consumed
          WHERE consumed.return_mail_id = returned.id AND consumed.recovery_id = recovery.id
      )
      AND take.kind = 'recover_equipment_take'
      AND take.execution_state = 'satisfied'
      AND take.proof_kind = 'delivery_recovery_take_succeeded'
      AND take.proof_reference = returned.external_mail_id
      AND take.proof_digest = recovery.custody_proof_digest
      AND take.expected_credits = 0
      AND take.expected_cod = 0
      AND take.expected_attachment_ids = ARRAY[equipment.external_equipment_id]
      AND equipment.world_id = 'isekai'
      AND take_delivery.origin_kind = 'settlement_recovery_action'
      AND take_delivery.origin_id = 'recovery:' || recovery.id::text
      AND take_delivery.status = 'succeeded'
      AND take_delivery.payload_operation = 'take'
      AND take_delivery.payload_world = 'isekai'
      AND take_delivery.external_reference = 'mooglemail:isekai:take:' || returned.external_mail_id
)"#
);

/// 收口 `equipment_recovered` 的依赖履约；调用方已按既有锁序锁定恢复记录、义务、动作与全部
/// 履约行，并已复核付款满足、交付就绪与 custody 证明。
pub(in crate::database::recovery::commands) async fn close_recovered_dependents(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    take: &ActionRow,
    command: &RecoveryConfirmationCommand,
    now: Timestamp,
) -> Result<(), RecoveryCommandError> {
    let handoff = recovery.equipment_obligation_id.ok_or(RecoveryCommandError::NotConfirmable)?;
    // 卖方货款已开始执行或已有支出：撤销会与之冲突，交给人工核对。
    let seller_untouched: bool = sqlx::query_scalar(SELLER_UNTOUCHED)
        .bind(recovery.seller_payout_obligation_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    if !seller_untouched {
        return Err(RecoveryCommandError::NotConfirmable);
    }

    // 交付履约行已锁定；只接受已寄出后仍未满足（等待领取或转人工）的原交付。
    let row = sqlx::query(
        "SELECT execution_state, delivery_id
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(handoff)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::NotConfirmable)?;
    let handoff_state: String = row.try_get("execution_state").map_err(map_sql_error)?;
    let delivery_id = row
        .try_get::<Option<Uuid>, _>("delivery_id")
        .map_err(map_sql_error)?
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    if !matches!(handoff_state.as_str(), "enqueued" | "manual_reconcile") {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    // 原交付投递已在履约行之前按规范锁序锁定（与收回/取消的原付款投递同序），处置绑定它的当前 fence。
    let fencing_version: i64 = sqlx::query_scalar(
        "SELECT fencing_version FROM public.deliveries WHERE id = $1 FOR UPDATE",
    )
    .bind(delivery_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let exact: bool = sqlx::query_scalar(RECOVERED_HANDOFF_CHAIN)
        .bind(delivery_id)
        .bind(handoff)
        .bind(take.id)
        .bind(recovery.id)
        .bind(recovery.equipment_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    if !exact || fencing_version < 1 {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    // 当前成功发送与可信 Sent 证明必须就是被收回的那一次：交付义务每一次可能已到达远端的发送都
    // 要由本恢复记录已消费的系统退件逐一归属，且本轮退件之后没有任何发送。退件之前另有可能外达
    // 的发送、或退件之后又寄出时，装备可能正在买家手里，不能与旧退件和 Take 证明拼接成收回确认；
    // 返回明确的阻断原因交人工（不变量 2、4、6）。
    if !return_attributable_to_sends(transaction, handoff, recovery.id).await? {
        return Err(RecoveryCommandError::SendUnattributed);
    }

    for (obligation_id, previous_state, failure_code) in [
        (recovery.seller_payout_obligation_id, "pending", SALE_VOIDED_CODE),
        (handoff, handoff_state.as_str(), EQUIPMENT_RECOVERED_CODE),
    ] {
        fail_fulfillment(transaction, obligation_id, failure_code, now).await?;
        append_closure_audit(
            transaction,
            ClosureAudit { obligation_id, recovery_id: recovery.id, previous_state, failure_code },
            command,
            now,
        )
        .await?;
    }

    let actor_id = if auction_domain::valid_text(&command.actor.actor_id, 128) {
        command.actor.actor_id.as_str()
    } else {
        "admin"
    };
    append(
        transaction,
        DeliveryDispositionFact {
            delivery_id,
            fencing_version,
            kind: auction_domain::DeliveryDispositionKind::RecoveryEquipmentRecovered,
            source: auction_domain::DeliveryDispositionSource {
                settlement_recovery_id: Some(recovery.id),
                ..Default::default()
            },
            actor_source: auction_domain::DeliveryDispositionActorSource::Admin,
            actor_id,
            reason: Some("Admin confirmed equipment_recovered after the handoff was returned"),
            reason_code: Some(EQUIPMENT_RECOVERED_CODE),
            detail: Some(&serde_json::json!({
                "recovery_id": recovery.id,
                "command_id": command.command_id.as_uuid(),
                "take_action_id": take.id,
                "take_delivery_id": take.delivery_id,
                "equipment_obligation_id": handoff,
                "seller_payout_obligation_id": recovery.seller_payout_obligation_id,
                "buyer_refund_obligation_id": recovery.buyer_refund_obligation_id,
            })),
            command_id: None,
            request_digest: None,
        },
    )
    .await
    .map_err(map_sql_error)
}
