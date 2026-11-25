//! COD 实收认证：寄售付款义务转 `satisfied` 的唯一入口。
//!
//! 保护不变量 1、2、3、6：一个顶层 Serializable 事务内复验冻结事实、消费 opening 份额、建立实收身份与
//! 收据、过渡用途分账、funded 投影与付款满足；任一步失败整笔回滚，不留孤立 `satisfied`、下游 `ready`
//! 或半笔收入。同一经济实收的重放与并发只读原结果，新增发行、转移、费用与下游工作全为零。

use auction_application::settlement::{CertifyCod, CertifyReceipt, FundingError};
use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::{CodReceiptSplit, plan_cod_funding};
use serde_json::json;
use sqlx::Postgres;
use uuid::Uuid;

use super::command_ledger::{SuccessReceipt, append_success};
use super::distribution::{
    distribute, resolve_targets, write_fee_earned, write_funded_sale, write_principal_fact,
};
use super::freeze::verify_freeze;
use super::fresh_cash::verify_fresh_cash;
use super::opening_slice::{SliceRow, borrow_plan, insert_uses, load_slices, rule};
use super::order_stage::record_stage_change;
use super::receipt::{
    ContractObligations, ExistingReceipt, ReceiptFacts, existing_receipt, funding_sql_error,
    insert_receipt, load_facts, lock_delivery_rows, lock_obligations, proof_digest, receipt_key_id,
    record_replayed_command, remote_payment, verify_boundary_chain,
};
use super::record_only::check_mode;
use super::serializable::run_serializable;
use crate::DatabaseStore;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::account::lock_funding_account;
use crate::database::settlement::funding::ledger::{complete, replay, source};
use crate::database::settlement::funding::{lock_funding_worlds, storage};

const ACTION: &str = "consignment_cod";
/// 回执账本里的操作名，与 000136 的 `c4_valid_v2_receipt` 一致。
const ACTION_OPERATION: &str = "certify_cod";

/// 认证一笔 COD 实收；世界与收款腿先互校（`check_leg`），再进入事务。
pub(crate) async fn certify_cod(
    store: &DatabaseStore,
    command: CertifyCod,
) -> Result<CertifyReceipt, FundingError> {
    command.check_leg()?;
    command.check_cash()?;
    if command.mutation.command_id.is_nil() || command.mutation.request_digest == [0; 32] {
        return Err(FundingError::Invalid);
    }
    run_serializable(store, &command, |tx, command| Box::pin(certify_in_tx(tx, command))).await
}

async fn certify_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
) -> Result<CertifyReceipt, FundingError> {
    // 锁序（D9 §1.3、R5 §3.8）：funding worlds → UID 账户 → obligations → allocations（opening 原余额、
    // 过渡与目标用途）→ deliveries → fulfillments；与 6B3 的现金腿、退款与阻断路径同序，不会成环。
    lock_funding_worlds(tx).await?;
    let account =
        lock_funding_account(tx, &command.receipt.uid).await?.ok_or(FundingError::Unavailable)?;
    let key_id = receipt_key_id(&command.receipt);
    if replay(tx, &account, &command.mutation, ACTION, command.payment_obligation_id).await? {
        let existing =
            existing_receipt(tx, command, key_id).await?.ok_or(FundingError::Conflict)?;
        return receipt_of(command, &existing, true);
    }
    // 不变量 6：已按未付取消封闭的 Sale 不再接受实收认证（与 c7_sale_closure_paid 相反方向的守卫）。
    // 确定拒绝、零收据零 funded 零分录；钱若已实收，留作该 UID 的未归属实收，由既有现金对账转人工。
    // 放在账本重放之后：已提交的同命令仍原样重放（不变量 1）。
    if sale_closed_unpaid(tx, command.contract_id).await? {
        return Err(FundingError::Conflict);
    }
    if !account.ready() {
        return Err(FundingError::Unavailable);
    }
    check_mode(&account, command.mode)?;
    let obligations = lock_obligations(tx, command).await?;

    let slices = attributed_slices(tx, command, key_id).await?;
    let included: i64 = slices.iter().map(SliceRow::amount).sum();
    if let Some(existing) = existing_receipt(tx, command, key_id).await? {
        // 同一经济实收：相容者读原结果并只登记本命令；异合同、异拆分或异额进冲突。
        if !existing.is_replay_of(command, key_id, included) {
            return Err(FundingError::Conflict);
        }
        record_replayed_command(
            tx,
            &command.receipt.uid,
            &command.mutation,
            ACTION,
            command.payment_obligation_id,
        )
        .await?;
        let receipt = receipt_of(command, &existing, true)?;
        append_receipt(tx, command, &receipt).await?;
        return Ok(receipt);
    }

    let split = CodReceiptSplit::new(command.gross, included).map_err(rule)?;
    let plan = plan_cod_funding(command.world, command.mode, command.price, command.quote, split)
        .map_err(rule)?;
    let payout_obligation_id = obligations.payout.ok_or(FundingError::Unproven)?;
    let borrows = borrow_plan(tx, &command.receipt.uid, &slices).await?;
    let targets = resolve_targets(tx, command, &plan, payout_obligation_id).await?;
    lock_delivery_rows(tx, command).await?;

    let facts = load_facts(tx, command).await?;
    facts.verify(command)?;
    verify_freeze(tx, command, facts.attempt_id).await?;
    verify_boundary_chain(tx, command).await?;
    let remote = remote_payment(tx, command, facts.attempt_id).await?;
    verify_fresh_cash(tx, &account, command, plan.new_issue).await?;

    // 在本事务内暂写付款满足及证明引用，让要求 satisfied 的 SQL 可读；失败时随整笔回滚。
    let digest = proof_digest(command, &remote, included);
    satisfy_payment(tx, command, &remote.evidence_id, &digest).await?;
    let source_id = source(
        tx,
        &command.receipt.uid,
        "collected_payment",
        command.payment_obligation_id,
        plan.new_issue,
        &digest,
    )
    .await?;
    insert_receipt(tx, command, source_id, key_id, &remote, split).await?;
    insert_uses(tx, &slices, source_id, key_id).await?;
    let fee_entry = distribute(tx, command, &plan, source_id, &borrows, &targets).await?;
    write_principal_fact(tx, command, facts.round_number).await?;
    write_fee_earned(tx, command, source_id, fee_entry).await?;
    write_funded_sale(tx, command, source_id).await?;
    release_downstream(tx, command, &facts, &obligations).await?;
    // 不变量 3：订单阶段事件与权利刷新同实收认证同事务，两者读自同一事务内已写入的资金事实。
    record_stage_change(tx, command.contract_id).await?;
    complete(tx, &account, &command.mutation, ACTION, command.payment_obligation_id).await?;
    let receipt = CertifyReceipt {
        contract_id: command.contract_id,
        receipt_source_id: source_id,
        split,
        plan,
        replayed: false,
    };
    append_receipt(tx, command, &receipt).await?;
    Ok(receipt)
}

/// 该 Sale 是否已有 `cancelled_unpaid` 封闭事实；读在资金账户锁之后，与封闭写端同锁序。
async fn sale_closed_unpaid(
    tx: &mut Transaction<'_, Postgres>,
    contract_id: Uuid,
) -> Result<bool, FundingError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_sale_closures
                        WHERE contract_id = $1 AND closure_kind = 'cancelled_unpaid')",
    )
    .bind(contract_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(funding_sql_error)
}

/// 同事务追加 `payment_funded` 回执与审计（不变量 1、3、8）；合同没有独立版本列，购买回执起恒为 1。
async fn append_receipt(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    receipt: &CertifyReceipt,
) -> Result<(), FundingError> {
    append_success(
        tx,
        SuccessReceipt {
            mutation: &command.mutation,
            operation: ACTION_OPERATION,
            target_kind: "obligation",
            target_id: command.payment_obligation_id,
            disposition: "payment_funded",
            fields: json!({
                "contract_id": receipt.contract_id.to_string(),
                "contract_version": 1,
                "obligation_id": command.payment_obligation_id.to_string(),
                "receipt_source_id": receipt.receipt_source_id.to_string(),
                "gross": receipt.split.gross(),
                "opening_included": receipt.split.opening_included(),
                "new_issue": receipt.split.new_issue(),
            }),
        },
    )
    .await
}

/// 命令声明的 opening 归属：给出 `attribution_id` 必须正是本实收业务键且份额已封存；
/// 未给出（已证全部未含）时，该键下不得已有封存份额，否则同一金额可能被计两次。
async fn attributed_slices(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    key_id: Uuid,
) -> Result<Vec<SliceRow>, FundingError> {
    let stored = load_slices(tx, &command.receipt.uid, key_id).await?;
    match command.attribution_id {
        Some(attribution_id) if attribution_id == key_id && !stored.is_empty() => Ok(stored),
        Some(_) => Err(FundingError::Unproven),
        None if stored.is_empty() => Ok(Vec::new()),
        None => Err(FundingError::Conflict),
    }
}

/// 已存收据还原的回执；计划由命令与已存拆分复算，重放不产生任何新写入。
fn receipt_of(
    command: &CertifyCod,
    existing: &ExistingReceipt,
    replayed: bool,
) -> Result<CertifyReceipt, FundingError> {
    let split = existing.split()?;
    let plan = plan_cod_funding(command.world, command.mode, command.price, command.quote, split)
        .map_err(rule)?;
    Ok(CertifyReceipt {
        contract_id: command.contract_id,
        receipt_source_id: existing.source_id,
        split,
        plan,
        replayed,
    })
}

/// 付款义务转 `satisfied`：只接受已激活（`enqueued`）且 delivery 一致的履约，行锁已持。
async fn satisfy_payment(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    evidence_id: &Uuid,
    digest: &[u8; 32],
) -> Result<(), FundingError> {
    let updated = sqlx::query(
        r#"WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
           UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'satisfied',
               proof_kind = 'mooglemail_sent_collected',
               proof_reference = $2,
               proof_digest = $3,
               proof_observed_at = (SELECT observed_at
                                    FROM public.consignment_dispatch_evidence WHERE id = $4),
               claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL,
               completed_at = (SELECT at FROM sql_clock),
               updated_at = (SELECT at FROM sql_clock),
               state_version = state_version + 1,
               next_attempt_at = NULL,
               last_error_code = NULL
           WHERE obligation_id = $1
             AND execution_state = 'enqueued'
             AND delivery_id = $5
             AND delivery_operation = 'send'"#,
    )
    .bind(command.payment_obligation_id)
    .bind(&command.receipt.payment_mail_id)
    .bind(digest.as_slice())
    .bind(evidence_id)
    .bind(command.original_delivery_id)
    .execute(&mut **tx)
    .await
    .map_err(funding_sql_error)?;
    if updated.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    Ok(())
}

/// 付款满足后的下游放行：main 的卖款必须等原件确领，本事务不放；isekai 只有未过交割界才放原件交接，
/// 过界的实收保留责任，不放原件（D9 §4.2 第 3 步），退款转换由 `convert_refund` 另行处理。
async fn release_downstream(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    facts: &ReceiptFacts,
    obligations: &ContractObligations,
) -> Result<(), FundingError> {
    let (SettlementWorld::Isekai, Some(true), Some(handoff)) =
        (command.world, facts.before_season_end, obligations.handoff)
    else {
        return Ok(());
    };
    sqlx::query(
        r#"UPDATE public.settlement_obligations
           SET business_state = 'ready',
               state_version = state_version + 1,
               updated_at = clock_timestamp()
           WHERE id = $1 AND prerequisite_obligation_id = $2 AND business_state = 'blocked'"#,
    )
    .bind(handoff)
    .bind(command.payment_obligation_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}
