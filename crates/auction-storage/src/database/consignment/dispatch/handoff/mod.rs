//! 买家确领原件（`prove_handoff`，D9 §2.3 `recipient_took_original`、§4.2 第 3 步、§6.2）：确领证明、
//! position 已交付、释放实物占用、挂单成交关闭、交接边、funded 投影前进与卖款放行一个顶层事务。
//!
//! 保护不变量 1、2、3、4、6、8：
//! - 命令 ID 只由原物义务加原轮决定；缺证据、验证器未签收、付款未认证、position 或挂单不在可交付态都是
//!   “临时缺证”，整笔回滚且不写账本（D9 §1.3：暂不可用不占死未来系统命令），之后补证同 ID 重试。
//! - 观察只有经已签收验证器产出、且绑定该轮唯一起写尝试与已绑定的 Sent 时才成为确领结论；同轮已接管原件
//!   退件（RTS）与确领互斥（D9 §2.3），出现即确定冲突。
//! - 锁序（D9 §1.3）：资金前缀 funding worlds → UID 账户，再挂单 → position → 合同义务（按 UUID）→ funded
//!   投影 → 履约行；与收款认证、退款转换、派发起写同序，不成环。
//! - 卖款只在“实收已认证（main 原件 COD 腿已满足；isekai 载体付款已满足）且原件确领已证”时由 `blocked`
//!   转 `ready`；它的前置就是本原件腿义务（main 为原件 COD、isekai 为原件交割），与图约束同源。

mod write;

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, DispatchOperation, HandoffReceipt, ProveHandoff,
};
use auction_domain::consignment::dispatch::dispatch_attempt_id;
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use self::write::{advance_funded, release_payout, satisfy_handoff, write_handoff};
use super::cash_funding;
use super::claim_proof::original_claim_signed;
use super::ledger::{
    Arbitration, LedgerCommand, Rejection, Step, TargetKind, arbitrate, insert_rejection,
    insert_success,
};
use super::lock_order::{lock_listing_and_position, read_by_contract};
use super::round::{StoredRound, load_by_key};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;

/// 原件腿义务种类：main 的原件 COD 与 isekai 的原件交割（与 `c6_guard_handoff_proof` 同一闭集）。
const MAIN_ORIGINAL: &str = "winner_equipment_cod";
const ISEKAI_HANDOFF: &str = "winner_equipment_handoff";

/// 合同、position 与挂单在本事务锁内读到的事实。
struct Facts {
    listing_id: Uuid,
    position_id: Uuid,
    generation: i64,
    seller_user_id: Uuid,
    buyer_user_id: Uuid,
    equipment_id: Uuid,
    world_id: String,
    canonical_eid: String,
    position_state: String,
    position_version: i64,
    listing_state: String,
    listing_version: i64,
    price: i64,
}

/// 被消费的确领证据。
struct Evidence {
    digest: Vec<u8>,
    validator_version: String,
    observed_at: Timestamp,
}

/// 确领事务体（见模块说明）。
pub(super) async fn prove_handoff_in_tx(
    command: &ProveHandoff,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Step<HandoffReceipt>, WriteError> {
    let ledger = LedgerCommand {
        command_id: command.command_id.as_uuid(),
        operation: DispatchOperation::ProveHandoff,
        target: TargetKind::Obligation,
        target_id: command.key.obligation_id,
        digest: command.request_hash,
    };
    let contract_id = command.key.contract_id;
    // 锁集合与预读不同则退出重规划（D9 §1.3）。
    let current = read_by_contract(tx.connection(), contract_id).await?;
    tx.ensure_lock_set(&current.sellers, &current.equipment, &[])?;
    match arbitrate(tx.connection(), &ledger).await? {
        Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
        Step::Done(Arbitration::Replay(body)) => {
            return Ok(Step::Done(receipt_from_body(command, &body, true)?));
        }
        Step::Done(Arbitration::First) => {}
    }
    // 合同不是 Sale 或不存在：没有可锁的实物与资金身份，确定来源不符。
    let Some(kind) = original_leg_kind(tx.connection(), command).await? else {
        return insert_rejection(tx.connection(), &ledger, Rejection::SourceMismatch).await;
    };
    cash_funding::lock_funding_prefix(tx, contract_id).await?;
    if !lock_listing_and_position(tx.connection(), contract_id).await? {
        return insert_rejection(tx.connection(), &ledger, Rejection::SourceMismatch).await;
    }
    sqlx::query(
        "SELECT id FROM public.settlement_obligations WHERE contract_id = $1 ORDER BY id FOR UPDATE",
    )
    .bind(contract_id)
    .execute(tx.connection())
    .await?;
    let Some(round) = load_by_key(tx.connection(), &command.key).await? else {
        return insert_rejection(tx.connection(), &ledger, Rejection::SourceMismatch).await;
    };
    if round.delivery_id != command.attempt.delivery_id {
        return insert_rejection(tx.connection(), &ledger, Rejection::SourceMismatch).await;
    }
    // 原物领取与原物 RTS 互斥（D9 §2.3）：同轮已接管原件退件即确定冲突，保留原占用待核。
    if original_returned(tx.connection(), command).await? {
        return insert_rejection(tx.connection(), &ledger, Rejection::EvidenceConflict).await;
    }
    let evidence = load_evidence(tx.connection(), command, &round).await?;
    let funded = funded_disposition(tx.connection(), contract_id).await?;
    match funded.as_deref() {
        Some("funded") => {}
        // 已转退款或已确领：与本次确领相反或重复的资金结论，确定冲突（不变量 6）。
        Some(_) => {
            return insert_rejection(tx.connection(), &ledger, Rejection::EvidenceConflict).await;
        }
        // 尚无实收认证：卖款不能放、原件交接也先不定论，等认证后重试。
        None => return Err(pending()),
    }
    if !payment_certified(tx.connection(), command, &kind, &round).await? {
        return Err(pending());
    }
    let facts = load_facts(tx.connection(), contract_id).await?;
    if !matches!(facts.position_state.as_str(), "committed" | "in_transit")
        || facts.listing_state != "sale_pending"
    {
        // 核对中、已交付或已灭失：不在可交付态，不改任何事实（D9 §1.2）。
        return Err(pending());
    }
    let proof_id = Uuid::new_v4();
    let listing_version = facts.listing_version + 1;
    let receipt = HandoffReceipt {
        command_id: command.command_id,
        proof_id,
        listing_id: facts.listing_id,
        listing_version: u64::try_from(listing_version)?,
        replayed: false,
    };
    // 挂单与代管事件的守卫按同事务的回执核对命令，先写回执（与迁移 136 的约束测试同序）。
    insert_success(tx.connection(), &ledger, "original_claimed", receipt_fields(command, &receipt))
        .await?;
    let now = database_now(tx.connection()).await?;
    write_handoff(tx.connection(), command, &facts, &evidence, proof_id, now).await?;
    advance_funded(tx.connection(), contract_id).await?;
    if kind == ISEKAI_HANDOFF {
        satisfy_handoff(tx.connection(), command, &round, &evidence).await?;
    }
    release_payout(tx.connection(), contract_id, command.key.obligation_id).await?;
    // 不变量 3：确领、成交关闭、卖款放行与订单阶段事件、权利刷新同事务。
    record_order_stage_change_in_tx(tx.connection(), contract_id, now).await?;
    Ok(Step::Done(receipt))
}

/// 临时缺证：整笔回滚、不写账本，同一系统命令以后补证重试（D9 §1.3）。
fn pending() -> WriteError {
    ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven).into()
}

/// 本轮义务是该 Sale 合同的原件腿时返回其种类；义务种类与合同只追加，无锁读取。
async fn original_leg_kind(
    connection: &mut PgConnection,
    command: &ProveHandoff,
) -> Result<Option<String>, WriteError> {
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT obligation.obligation_kind
         FROM public.settlement_obligations AS obligation
         JOIN public.consignment_sales AS sale ON sale.contract_id = obligation.contract_id
         WHERE obligation.id = $1 AND obligation.contract_id = $2
           AND obligation.obligation_kind IN ($3, $4)",
    )
    .bind(command.key.obligation_id)
    .bind(command.key.contract_id)
    .bind(MAIN_ORIGINAL)
    .bind(ISEKAI_HANDOFF)
    .fetch_optional(connection)
    .await?;
    Ok(kind)
}

/// 同轮原件是否已有退件接管（可信 RTS 已被接管即说明原件被退回）。
async fn original_returned(
    connection: &mut PgConnection,
    command: &ProveHandoff,
) -> Result<bool, WriteError> {
    let returned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_return_intakes
                        WHERE contract_id = $1 AND obligation_id = $2 AND original_round = $3)",
    )
    .bind(command.key.contract_id)
    .bind(command.key.obligation_id)
    .bind(i32::try_from(command.key.round)?)
    .fetch_one(connection)
    .await?;
    Ok(returned)
}

/// 确领证据：必须挂在该轮唯一起写尝试上、该尝试已绑定 Sent、摘要逐字节相同，且出自已签收的验证器。
/// 任一不满足都是临时缺证（之后可有新的观察），不占命令 ID。
async fn load_evidence(
    connection: &mut PgConnection,
    command: &ProveHandoff,
    round: &StoredRound,
) -> Result<Evidence, WriteError> {
    let attempt_id = dispatch_attempt_id(&command.attempt);
    let fence = i64::try_from(command.attempt.fence)?;
    let row = sqlx::query(
        "SELECT evidence.digest, evidence.validator_version, evidence.observed_at
         FROM public.consignment_dispatch_attempts AS attempt
         JOIN public.delivery_sent_bindings AS binding
           ON binding.delivery_id = attempt.delivery_id AND binding.fencing_version = attempt.fence
         JOIN public.consignment_dispatch_evidence AS evidence
           ON evidence.attempt_id = attempt.attempt_id
         WHERE attempt.delivery_id = $1 AND attempt.fence = $2 AND attempt.attempt_id = $3
           AND attempt.contract_id = $4 AND attempt.obligation_id = $5 AND attempt.round = $6
           AND evidence.category = 'original_claim' AND evidence.remote_observation_id = $7",
    )
    .bind(round.delivery_id)
    .bind(fence)
    .bind(attempt_id)
    .bind(command.key.contract_id)
    .bind(command.key.obligation_id)
    .bind(i32::try_from(command.key.round)?)
    .bind(&command.remote_observation_id)
    .fetch_optional(connection)
    .await?
    .ok_or_else(pending)?;
    let evidence = Evidence {
        digest: row.try_get("digest")?,
        validator_version: row.try_get("validator_version")?,
        observed_at: row.try_get("observed_at")?,
    };
    if evidence.digest.as_slice() != command.evidence_digest.as_slice()
        || !original_claim_signed(&evidence.validator_version)
    {
        tracing::warn!(
            code = "handoff_evidence_unproven",
            "原件确领证据摘要不符或验证器未签收，不放行"
        );
        return Err(pending());
    }
    Ok(evidence)
}

/// funded 投影的处置（锁住）；尚无实收认证返回 `None`。
async fn funded_disposition(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Option<String>, WriteError> {
    let disposition: Option<String> = sqlx::query_scalar(
        "SELECT disposition FROM public.consignment_funded_sales WHERE contract_id = $1 FOR UPDATE",
    )
    .bind(contract_id)
    .fetch_optional(connection)
    .await?;
    Ok(disposition)
}

/// 实收已认证：main 原件 COD 腿本身已满足（`certify_cod` 是唯一入口）；isekai 载体付款已满足，且原件交割
/// 腿的履约行已衔接到本轮投递、尚未满足。
async fn payment_certified(
    connection: &mut PgConnection,
    command: &ProveHandoff,
    kind: &str,
    round: &StoredRound,
) -> Result<bool, WriteError> {
    let certified: Option<bool> = sqlx::query_scalar(
        "SELECT CASE WHEN $2 = 'winner_equipment_cod'
                    THEN fulfillment.execution_state = 'satisfied'
                         AND fulfillment.delivery_id = $3
                    ELSE payment.execution_state = 'satisfied'
                         AND fulfillment.execution_state = 'enqueued'
                         AND fulfillment.delivery_id = $3
               END
         FROM public.settlement_obligations AS obligation
         JOIN public.settlement_obligation_fulfillments AS fulfillment
           ON fulfillment.obligation_id = obligation.id
         LEFT JOIN public.settlement_obligation_fulfillments AS payment
           ON payment.obligation_id = obligation.prerequisite_obligation_id
         WHERE obligation.id = $1
         FOR UPDATE OF fulfillment",
    )
    .bind(command.key.obligation_id)
    .bind(kind)
    .bind(round.delivery_id)
    .fetch_optional(connection)
    .await?;
    Ok(certified == Some(true))
}

/// 合同、position 与挂单事实（挂单与 position 已在本事务锁住）。
async fn load_facts(connection: &mut PgConnection, contract_id: Uuid) -> Result<Facts, WriteError> {
    let row = sqlx::query(
        "SELECT contract.listing_id, contract.position_id, contract.generation,
                contract.seller_user_id, sale.buyer_user_id, contract.equipment_id,
                contract.world_id, contract.canonical_eid,
                position.state AS position_state, position.version AS position_version,
                position.current_generation, listing.state AS listing_state,
                listing.state_version AS listing_version, listing.price
         FROM public.consignment_contracts AS contract
         JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
         JOIN public.consignment_custody_positions AS position ON position.id = contract.position_id
         JOIN public.consignment_listings AS listing ON listing.id = contract.listing_id
         WHERE contract.contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| WriteError::invariant("consignment_handoff_contract"))?;
    let generation: i64 = row.try_get("generation")?;
    // 已交付只能落在签约的那一代：指针已离开该代说明合同与实物不符，失败关闭。
    if row.try_get::<Option<i64>, _>("current_generation")? != Some(generation) {
        return Err(WriteError::invariant("consignment_handoff_generation"));
    }
    Ok(Facts {
        listing_id: row.try_get("listing_id")?,
        position_id: row.try_get("position_id")?,
        generation,
        seller_user_id: row.try_get("seller_user_id")?,
        buyer_user_id: row.try_get("buyer_user_id")?,
        equipment_id: row.try_get("equipment_id")?,
        world_id: row.try_get("world_id")?,
        canonical_eid: row.try_get("canonical_eid")?,
        position_state: row.try_get("position_state")?,
        position_version: row.try_get("position_version")?,
        listing_state: row.try_get("listing_state")?,
        listing_version: row.try_get("listing_version")?,
        price: row.try_get("price")?,
    })
}

/// 回执必有字段（迁移 137 `c4_valid_v2_receipt` 的 `prove_handoff` 分支）；合同没有独立版本列，恒为 1。
fn receipt_fields(command: &ProveHandoff, receipt: &HandoffReceipt) -> Value {
    json!({
        "contract_id": command.key.contract_id,
        "contract_version": 1,
        "obligation_id": command.key.obligation_id,
        "proof_id": receipt.proof_id,
        "listing_id": receipt.listing_id,
        "listing_version": receipt.listing_version,
        "state": "closed",
    })
}

/// 由已存成功回执还原；字段缺失说明账本被改坏，失败关闭。
fn receipt_from_body(
    command: &ProveHandoff,
    body: &Value,
    replayed: bool,
) -> Result<HandoffReceipt, WriteError> {
    let uuid = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .and_then(|text| Uuid::parse_str(text).ok())
            .ok_or_else(|| WriteError::invariant("consignment_handoff_receipt_body"))
    };
    let listing_version = body
        .get("listing_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| WriteError::invariant("consignment_handoff_receipt_body"))?;
    Ok(HandoffReceipt {
        command_id: command.command_id,
        proof_id: uuid("proof_id")?,
        listing_id: uuid("listing_id")?,
        listing_version,
        replayed,
    })
}
