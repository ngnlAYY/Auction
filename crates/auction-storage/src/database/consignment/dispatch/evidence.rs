//! 只追加的派发证据：同 `(尝试, 类别, 远端观察)` 重放，同键异摘要判冲突。
//!
//! 保护不变量 3、4、6、8：证据一经写入不改不删；观察不会自动成为有效结论，只有 Sent 绑定类证据
//! 在同事务里写入跨来源唯一的 Sent MID 绑定，绑定失败不留证据行；绑定成功时同事务把起写所在的
//! 轮投递从 `sending` 推进到 `succeeded`（照拍卖投递成功的写法与守卫），重放不重复推进。

use auction_application::consignment::{DispatchOperation, EvidenceReceipt, RecordEvidence};
use auction_application::settlement::CashLeg;
use auction_domain::consignment::dispatch::{EvidenceCategory, dispatch_attempt_id};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use super::cash_funding::{self, CashSend};
use super::claim_proof::cash_claim_signed;
use super::ledger::{
    Arbitration, LedgerCommand, Rejection, Step, TargetKind, arbitrate, insert_rejection,
    insert_success,
};
use super::sent_binding::bind_dispatch_sent;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;
use crate::database::delivery::audit::{
    DeliveryAudit, SUCCEED, append as append_delivery_audit, worker_state_metadata,
};

/// 追加一条证据。
pub(super) async fn record_evidence_in_tx(
    command: &RecordEvidence,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Step<EvidenceReceipt>, WriteError> {
    let attempt_id = dispatch_attempt_id(&command.attempt);
    let ledger = LedgerCommand {
        command_id: command.command_id.as_uuid(),
        operation: DispatchOperation::RecordEvidence,
        target: TargetKind::Attempt,
        target_id: attempt_id,
        digest: command.request_hash,
    };
    match arbitrate(tx.connection(), &ledger).await? {
        Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
        Step::Done(Arbitration::Replay(_)) => {
            return Ok(Step::Done(EvidenceReceipt {
                command_id: command.command_id,
                replayed: true,
            }));
        }
        Step::Done(Arbitration::First) => {}
    }
    let fence = i64::try_from(command.attempt.fence)?;
    let started: Option<(String, Uuid, Uuid, i64)> = sqlx::query_as(
        "SELECT send.world_id, attempt.contract_id, attempt.obligation_id, round.credits
         FROM public.consignment_dispatch_attempts AS attempt
         JOIN public.consignment_dispatch_rounds AS round ON round.delivery_id = attempt.delivery_id
         JOIN public.delivery_send_attempts AS send
           ON send.delivery_id = attempt.delivery_id AND send.fencing_version = attempt.fence
         WHERE attempt.delivery_id = $1 AND attempt.fence = $2 AND attempt.attempt_id = $3",
    )
    .bind(command.attempt.delivery_id)
    .bind(fence)
    .bind(attempt_id)
    .fetch_optional(tx.connection())
    .await?;
    let Some((world, contract_id, obligation_id, credits)) = started else {
        // 没有起写的尝试不能挂证据。
        return insert_rejection(tx.connection(), &ledger, Rejection::ActionNotAllowed).await;
    };
    let existing = sqlx::query(
        "SELECT id, digest FROM public.consignment_dispatch_evidence
         WHERE attempt_id = $1 AND category = $2 AND remote_observation_id = $3",
    )
    .bind(attempt_id)
    .bind(command.category.as_str())
    .bind(&command.remote_observation_id)
    .fetch_optional(tx.connection())
    .await?;
    if let Some(row) = existing {
        let evidence_id: Uuid = row.try_get("id")?;
        let stored: Vec<u8> = row.try_get("digest")?;
        // 同键同摘要才是重放；同键异摘要是冲突，旧证据不被改写。
        if stored.as_slice() != command.evidence_digest.as_slice() {
            return insert_rejection(tx.connection(), &ledger, Rejection::EvidenceConflict).await;
        }
        insert_success(
            tx.connection(),
            &ledger,
            "evidence_recorded",
            json!({ "attempt_id": attempt_id, "evidence_id": evidence_id }),
        )
        .await?;
        return Ok(Step::Done(EvidenceReceipt { command_id: command.command_id, replayed: true }));
    }
    // 现金腿（载体本金、卖款、退款）的 Sent 绑定同事务把占用转 spent（D9 §4.3–4.4）；卖款 / 退款腿出自已签收
    // 验证器的现金确领证据同事务追加 claimed（PIWP，D9 §2.3 `remote_cash_claimed`），未签收只记观察。资金锁
    // 前缀与义务行锁先于 Sent 绑定与投递行写入（D9 §1.3）；资金失败整笔回滚，证据不落（不变量 3、4、6）。
    let cash_leg = match command.category {
        EvidenceCategory::SentBinding | EvidenceCategory::CashClaim => {
            cash_funding::cash_leg_of(tx.connection(), obligation_id).await?
        }
        _ => None,
    };
    let send = |leg| CashSend {
        command_id: command.command_id,
        request_hash: command.request_hash,
        contract_id,
        obligation_id,
        delivery_id: command.attempt.delivery_id,
        leg,
        amount: credits,
    };
    let cash_sent =
        cash_leg.filter(|_| command.category == EvidenceCategory::SentBinding).map(send);
    let cash_claim = cash_leg
        .filter(|leg| {
            command.category == EvidenceCategory::CashClaim
                && matches!(leg, CashLeg::Seller | CashLeg::Refund)
                && cash_claim_signed(&command.validator_version)
        })
        .map(send);
    if cash_sent.is_some() || cash_claim.is_some() {
        cash_funding::lock_funding_prefix(tx, contract_id).await?;
        cash_funding::lock_obligation(tx.connection(), obligation_id).await?;
    }
    if command.category == EvidenceCategory::SentBinding {
        let bound = bind_dispatch_sent(
            tx.tracked(),
            command.attempt.delivery_id,
            fence,
            &world,
            &command.remote_observation_id,
        )
        .await?;
        if !bound {
            return insert_rejection(tx.connection(), &ledger, Rejection::EvidenceConflict).await;
        }
    }
    let evidence_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.consignment_dispatch_evidence
            (id, attempt_id, category, remote_observation_id, validator_version, digest,
             observed_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(evidence_id)
    .bind(attempt_id)
    .bind(command.category.as_str())
    .bind(&command.remote_observation_id)
    .bind(&command.validator_version)
    .bind(command.evidence_digest.as_slice())
    .bind(command.observed_at)
    .execute(tx.connection())
    .await?;
    if command.category == EvidenceCategory::SentBinding {
        // 先推进投递终态，再写订单阶段事件：事件读到的是“已寄出”，而不是“有 Sent 却未成功”的矛盾态。
        let sent = SentClose { fence, world: &world, attempt_id, evidence_id };
        close_sent_delivery(tx, command, &sent).await?;
    }
    if let Some(send) = &cash_sent {
        cash_funding::settle_sent(tx, send).await?;
    }
    if let Some(send) = &cash_claim {
        cash_funding::settle_claimed(tx, send).await?;
    }
    // 新证据与订单阶段事件、权利刷新同事务（不变量 3）；同键重放不改事实，不追加。
    let now = database_now(tx.connection()).await?;
    record_order_stage_change_in_tx(tx.connection(), contract_id, now).await?;
    insert_success(
        tx.connection(),
        &ledger,
        "evidence_recorded",
        json!({ "attempt_id": attempt_id, "evidence_id": evidence_id }),
    )
    .await?;
    Ok(Step::Done(EvidenceReceipt { command_id: command.command_id, replayed: false }))
}

/// 已绑定 Sent MID 的起写事实。
struct SentClose<'a> {
    fence: i64,
    world: &'a str,
    attempt_id: Uuid,
    evidence_id: Uuid,
}

/// Sent 绑定证据同事务收口：起写所在 fence 仍是当前 fence 的 `sending` 投递转 `succeeded`，
/// 照拍卖成功写法清租约、写完成时刻与 `mooglemail:<world>:send:<MID>` 引用，并追加成功审计。
///
/// 保护不变量 3、4、6：投递已不在 `sending`（已成功、或被人工核对 / 过期回收改走别的状态）时不改，
/// 证据照记、结论交人工核对，绝不覆盖已有终态。证明标签 `consignment_sent_binding_v1` 不是拍卖
/// 认可的冻结 Sent 证明：拍卖资金收据与退款 / 恢复的证明谓词（`delivery_has_frozen_sent_proof` 等）
/// 不会把寄售成功当作拍卖寄出。状态型谓词 `sent_delivery_sql!`（资产对账、寄售偿付、看板）只看
/// `succeeded`，会把它计入“已寄出 / 在途”：这一口径是对的（统筹方裁定），资产对账按 Bot 实物流向，
/// 寄售寄出同样离开 Bot 库存。
/// 不要求租约仍有效：Sent 绑定本身就是该 fence 已寄出的证明，过租不能抹掉已证事实。
async fn close_sent_delivery(
    tx: &mut DepositTransaction<'_, '_>,
    command: &RecordEvidence,
    sent: &SentClose<'_>,
) -> Result<(), WriteError> {
    let reference = format!("mooglemail:{}:send:{}", sent.world, command.remote_observation_id);
    let metadata = json!({
        "operation": "send",
        "proof": "consignment_sent_binding_v1",
        "attempt_id": sent.attempt_id,
        "evidence_id": sent.evidence_id,
        "sending_fencing_version": sent.fence,
        "validator_version": command.validator_version,
    });
    let closed: Option<i32> = sqlx::query_scalar(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         UPDATE public.deliveries
         SET status = 'succeeded', lease_token = NULL, lease_expires_at = NULL,
             completed_at = (SELECT at FROM sql_clock), external_reference = $3,
             failure_code = NULL, result_metadata = result_metadata || $4::jsonb,
             updated_at = (SELECT at FROM sql_clock)
         WHERE id = $1 AND status = 'sending' AND fencing_version = $2
           AND sending_at IS NOT NULL AND external_reference IS NULL
         RETURNING attempt_count",
    )
    .bind(command.attempt.delivery_id)
    .bind(sent.fence)
    .bind(&reference)
    .bind(&metadata)
    .fetch_optional(tx.connection())
    .await?;
    let Some(attempt_count) = closed else {
        tracing::warn!(
            code = "dispatch_sent_not_sending",
            "寄售轮投递已不在 sending，Sent 证据只记不推进"
        );
        return Ok(());
    };
    let occurred_at = database_now(tx.connection()).await?;
    append_delivery_audit(
        tx.tracked(),
        DeliveryAudit {
            occurred_at,
            actor_type: "worker",
            actor_id: "consignment-dispatch",
            action: SUCCEED,
            delivery_id: command.attempt.delivery_id,
            request_id: Some(&command.command_id.as_uuid().to_string()),
            source: "worker",
            metadata: worker_state_metadata(
                "succeeded",
                sent.fence,
                u32::try_from(attempt_count)?,
                None,
            ),
        },
    )
    .await?;
    Ok(())
}
