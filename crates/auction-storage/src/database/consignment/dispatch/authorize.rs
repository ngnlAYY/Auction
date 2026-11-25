//! 授权派发轮：已由购买事务建立的首轮按原样确认，其余首轮与新轮要求投递已备好且冻结身份一致。
//!
//! 保护不变量 5、6：同义务的轮授权在履约行锁下串行；新轮只能紧接上一轮，并引用已封闭上一轮的
//! 退件接管与消费（同一合同、义务、原轮），缺任一项都失败关闭，绝不无依据重发。新轮冻结的权利人
//! 必须是该义务的受益人（与购买事务对首轮的核对同类），不符同样失败关闭。补寄新轮建立的同一事务里，
//! 履约行从上一轮的投递放回可领取的初始 `pending`（不指向投递），由派发领取与衔接改指新轮投递；
//! 只在上一轮已被退件消费封闭、履约行仍指向上一轮投递（或从未衔接）且未满足时允许。

use auction_application::consignment::{AuthorizeDispatch, DispatchOperation, RoundReceipt};
use auction_domain::consignment::dispatch::{
    DispatchRoundKey, RoundOrigin, dispatch_round_id, next_round_number,
};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::first_dispatch::hex;
use super::ledger::{
    Arbitration, LedgerCommand, Rejection, Step, TargetKind, arbitrate, insert_rejection,
    insert_success,
};
use super::lock_order::{lock_listing_and_position, read_by_contract};
use super::round::{Authorization, insert_round, latest_round, load_by_key, load_by_round_id};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;

/// 授权首轮或合法新轮。
pub(super) async fn authorize_in_tx(
    command: &AuthorizeDispatch,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Step<RoundReceipt>, WriteError> {
    let round_id = dispatch_round_id(&command.key);
    let ledger = LedgerCommand {
        command_id: command.command_id.as_uuid(),
        operation: DispatchOperation::AuthorizeDispatch,
        target: TargetKind::Round,
        target_id: round_id,
        digest: command.request_hash,
    };
    // 锁集合与预读不同则退出重规划（D9 §1.3）。
    let current = read_by_contract(tx.connection(), command.key.contract_id).await?;
    tx.ensure_lock_set(&current.sellers, &current.equipment, &[])?;
    match arbitrate(tx.connection(), &ledger).await? {
        Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
        Step::Done(Arbitration::Replay(body)) => {
            let delivery_id = body
                .get("delivery_id")
                .and_then(Value::as_str)
                .and_then(|text| Uuid::parse_str(text).ok())
                .ok_or_else(|| WriteError::invariant("consignment_dispatch_receipt_body"))?;
            return Ok(Step::Done(RoundReceipt {
                command_id: command.command_id,
                round_id,
                round: command.key.round,
                delivery_id,
                replayed: true,
            }));
        }
        Step::Done(Arbitration::First) => {}
    }
    let connection = tx.connection();
    if command.frozen.validate().is_err() {
        return insert_rejection(connection, &ledger, Rejection::ActionNotAllowed).await;
    }
    // 锁序（R5 §3.8）：挂单 → position → 履约行；合同不存在则来源不符。
    if !lock_listing_and_position(connection, command.key.contract_id).await? {
        return insert_rejection(connection, &ledger, Rejection::SourceMismatch).await;
    }
    // 再锁履约行判轮次：同一义务的授权在此串行，并核对义务确属该合同。
    let owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT obligation.contract_id
         FROM public.settlement_obligation_fulfillments AS fulfillment
         JOIN public.settlement_obligations AS obligation ON obligation.id = fulfillment.obligation_id
         WHERE fulfillment.obligation_id = $1 FOR UPDATE OF fulfillment",
    )
    .bind(command.key.obligation_id)
    .fetch_optional(&mut *connection)
    .await?;
    if owner != Some(command.key.contract_id) {
        return insert_rejection(connection, &ledger, Rejection::SourceMismatch).await;
    }
    if let Some(existing) = load_by_round_id(connection, round_id).await? {
        // 购买事务已建立的首轮：冻结身份与投递一致才确认，不另建、不改写。
        if matches!(command.origin, RoundOrigin::First)
            && existing.delivery_id == command.delivery_id
            && existing.matches(&command.frozen)
        {
            return finish(connection, &ledger, command, round_id, true).await;
        }
        return insert_rejection(connection, &ledger, Rejection::SourceMismatch).await;
    }
    // 不变量 6：新轮冻结的权利人必须等于该义务的受益人；否则这一轮会以别人的名义行使权利，
    // 起写处的授权虽会保留不发，但轮事实一经写入不可改，所以在建轮时就失败关闭。
    let beneficiary = beneficiary_identity(connection, command).await?;
    if beneficiary.as_deref() != Some(command.frozen.rights_holder_identity.as_str()) {
        return insert_rejection(connection, &ledger, Rejection::SourceMismatch).await;
    }
    let prior =
        latest_round(connection, command.key.contract_id, command.key.obligation_id).await?;
    let Ok(next) = next_round_number(prior, &command.origin) else {
        return insert_rejection(connection, &ledger, Rejection::ActionNotAllowed).await;
    };
    if next != command.key.round {
        return insert_rejection(connection, &ledger, Rejection::ActionNotAllowed).await;
    }
    let authorization = match command.origin {
        RoundOrigin::First => Authorization::first(),
        RoundOrigin::Redispatch { closed_by_return_intake } => {
            // 封闭上一轮的事实：该 intake 属于同合同、同义务、上一轮，且已有消费。
            let consumption: Option<Uuid> = sqlx::query_scalar(
                "SELECT consumption.id
                 FROM public.consignment_return_intakes AS intake
                 JOIN public.consignment_return_consumptions AS consumption
                   ON consumption.intake_id = intake.id
                 WHERE intake.id = $1 AND intake.contract_id = $2 AND intake.obligation_id = $3
                   AND intake.original_round = $4",
            )
            .bind(closed_by_return_intake)
            .bind(command.key.contract_id)
            .bind(command.key.obligation_id)
            .bind(i32::try_from(next - 1)?)
            .fetch_optional(&mut *connection)
            .await?;
            let Some(consumption_id) = consumption else {
                return insert_rejection(connection, &ledger, Rejection::EvidenceUnproven).await;
            };
            Authorization {
                prior_round: Some(next - 1),
                intake_id: Some(closed_by_return_intake),
                consumption_id: Some(consumption_id),
            }
        }
    };
    if !prepared_delivery_matches(connection, command).await? {
        return insert_rejection(connection, &ledger, Rejection::SourceMismatch).await;
    }
    // 补寄新轮：履约行必须仍停在上一轮（写入前判定，拒绝时不留任何事实）。
    let reset = match authorization.prior_round {
        Some(prior_round) => {
            let prior_key = DispatchRoundKey { round: prior_round, ..command.key };
            let Some(prior) = load_by_key(&mut *connection, &prior_key).await? else {
                return insert_rejection(connection, &ledger, Rejection::SourceMismatch).await;
            };
            if !fulfillment_on_prior_round(&mut *connection, command, prior.delivery_id).await? {
                return insert_rejection(connection, &ledger, Rejection::SourceMismatch).await;
            }
            Some(prior.delivery_id)
        }
        None => None,
    };
    insert_round(
        connection,
        &command.key,
        round_id,
        command.delivery_id,
        &command.frozen,
        authorization,
    )
    .await?;
    if let Some(prior_delivery) = reset {
        reset_fulfillment(&mut *connection, command, prior_delivery).await?;
    }
    // 新轮与订单阶段事件、权利刷新同事务（不变量 3）；确认购买时已建的首轮不改事实，不追加。
    let now = database_now(&mut *connection).await?;
    record_order_stage_change_in_tx(&mut *connection, command.key.contract_id, now).await?;
    finish(connection, &ledger, command, round_id, false).await
}

/// 义务受益人的冻结身份：买方腿取 sale 冻结买方，卖方腿取合同冻结卖方；种类映射与
/// `lock_send_beneficiary`、`authorize_entitlement_recipient_in_tx` 一致。合同与 sale 行只追加，
/// 无锁读取不改变本事务的锁序。种类不在映射内或身份缺失返回 `None`（调用方失败关闭）。
async fn beneficiary_identity(
    connection: &mut PgConnection,
    command: &AuthorizeDispatch,
) -> Result<Option<String>, WriteError> {
    let identity: Option<Option<String>> = sqlx::query_scalar(
        "SELECT CASE
                    WHEN obligation.obligation_kind IN ('winner_equipment_cod',
                         'winner_payment_carrier_cod', 'winner_equipment_handoff', 'buyer_refund')
                        THEN sale.frozen_buyer_identity
                    WHEN obligation.obligation_kind IN ('seller_payout', 'withdrawn_return',
                         'unsold_return')
                        THEN contract.frozen_seller_identity
                END
         FROM public.settlement_obligations AS obligation
         JOIN public.consignment_contracts AS contract
           ON contract.contract_id = obligation.contract_id
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = obligation.contract_id
         WHERE obligation.id = $1 AND obligation.contract_id = $2",
    )
    .bind(command.key.obligation_id)
    .bind(command.key.contract_id)
    .fetch_optional(connection)
    .await?;
    Ok(identity.flatten())
}

/// 履约行（已在本事务锁住）仍停在上一轮：指向上一轮的投递，或从未衔接（初始 `pending`、不指向投递）；
/// 已满足（付款已证）的义务不再补寄。
async fn fulfillment_on_prior_round(
    connection: &mut PgConnection,
    command: &AuthorizeDispatch,
    prior_delivery: Uuid,
) -> Result<bool, WriteError> {
    let on_prior: Option<bool> = sqlx::query_scalar(
        "SELECT (delivery_id = $2 OR (delivery_id IS NULL AND execution_state = 'pending'))
                AND execution_state <> 'satisfied'
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(command.key.obligation_id)
    .bind(prior_delivery)
    .fetch_optional(connection)
    .await?;
    Ok(on_prior == Some(true))
}

/// 补寄新轮建立后把履约行放回可领取的初始 `pending`，清掉上一轮的衔接、证明与领取（列集合照拍卖
/// `settlement/returns/resend.rs` 的补寄重置）；上一轮的投递、尝试与证据都不改写（不变量 8）。
/// 守卫与 [`fulfillment_on_prior_round`] 相同，行已在本事务锁住，不中则说明事实被并发改动，整笔回滚。
async fn reset_fulfillment(
    connection: &mut PgConnection,
    command: &AuthorizeDispatch,
    prior_delivery: Uuid,
) -> Result<(), WriteError> {
    let updated = sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'pending', delivery_id = NULL,
             prepare_command_id = NULL, prepare_request_digest = NULL,
             activate_command_id = NULL, activate_request_digest = NULL,
             proof_kind = NULL, proof_reference = NULL, proof_digest = NULL,
             proof_observed_at = NULL, last_error_code = NULL, completed_at = NULL,
             claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL,
             next_attempt_at = NULL, retry_count = 0, state_version = state_version + 1,
             updated_at = statement_timestamp()
         WHERE obligation_id = $1 AND execution_state <> 'satisfied'
           AND (delivery_id = $2 OR (delivery_id IS NULL AND execution_state = 'pending'))",
    )
    .bind(command.key.obligation_id)
    .bind(prior_delivery)
    .execute(connection)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_redispatch_fulfillment_reset"));
    }
    Ok(())
}

/// 新轮的投递必须已备好（`prepared`）、属于该义务，且载荷里的冻结身份摘要与请求一致。
async fn prepared_delivery_matches(
    connection: &mut PgConnection,
    command: &AuthorizeDispatch,
) -> Result<bool, WriteError> {
    let row = sqlx::query(
        "SELECT payload->>'identity_digest' AS identity_digest
         FROM public.deliveries
         WHERE id = $1 AND status = 'prepared' AND origin_kind = 'settlement' AND origin_id = $2",
    )
    .bind(command.delivery_id)
    .bind(command.key.obligation_id.to_string())
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let digest: Option<String> = row.try_get("identity_digest")?;
    Ok(digest.as_deref() == Some(hex(&command.frozen.identity_digest()).as_str()))
}

async fn finish(
    connection: &mut PgConnection,
    ledger: &LedgerCommand,
    command: &AuthorizeDispatch,
    round_id: Uuid,
    replayed: bool,
) -> Result<Step<RoundReceipt>, WriteError> {
    insert_success(
        connection,
        ledger,
        "round_created",
        json!({
            "round_id": round_id,
            "delivery_id": command.delivery_id,
            "contract_id": command.key.contract_id,
            "obligation_id": command.key.obligation_id,
            "round": command.key.round,
        }),
    )
    .await?;
    Ok(Step::Done(RoundReceipt {
        command_id: command.command_id,
        round_id,
        round: command.key.round,
        delivery_id: command.delivery_id,
        replayed,
    }))
}
