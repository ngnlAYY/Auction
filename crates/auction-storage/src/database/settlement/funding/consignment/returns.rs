//! 现金退件的资金回补：精确 Take 的消费同事务把原支出一次全额 `return` 回原 UID、合同与用途，并解除 UID 阻断。
//!
//! 保护不变量 2、4、6、8：每个旧 delivery 只返还一次（`returned_delivery` 来源、`consignment_funding_returns` 的
//! 唯一键）；金额等于原支出而不是它的两倍；只拿到 Take 证明、只消费邮件或没有阻断行（未知旧退件）
//! 都不放行。核清后原用途恢复，已恢复的金额不得再次发行。

use auction_application::settlement::{FundingError, FundingMutation};
use auction_domain::consignment::funding::{
    BlockClearanceEvidence, BlockedLeg, FundingMode, PrincipalConclusion, may_clear_block,
};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::blockers::leg_code;
use super::commands::replay_command;
use super::contract_allocations::{contract_allocation, locked_allocation, parse_mode, sale_facts};
use super::principal::{PrincipalFact, record_principal_fact_in_tx};
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::account::lock_funding_account;
use crate::database::settlement::funding::ledger::{
    FundingEntry, allocation, complete, entry, source, transfer,
};
use crate::database::settlement::funding::returns::record_delivery_binding;
use crate::database::settlement::funding::storage;

/// 回补动作的命令账本动作名。
const ACTION: &str = "consignment_return";

/// 一次现金退件消费的已证事实；退件接管的消费事务在写入证明与消费行之后传入。
#[derive(Debug, Clone)]
pub(crate) struct ReturnConsumptionFacts {
    pub(crate) mutation: FundingMutation,
    pub(crate) contract_id: Uuid,
    pub(crate) intake_id: Uuid,
    pub(crate) consumption_id: Uuid,
    pub(crate) leg: BlockedLeg,
    pub(crate) original_delivery_id: Uuid,
    pub(crate) amount: i64,
    pub(crate) proof_digest: [u8; 32],
}

/// 该腿在原 UID 下恢复的用途。
const fn leg_purpose(leg: BlockedLeg) -> &'static str {
    match leg {
        BlockedLeg::Refund => "refund",
        BlockedLeg::Payout => "seller",
        BlockedLeg::CarrierPrincipal => "carrier",
    }
}

/// 消费同事务：写 returned source 与一笔完整 `return`、恢复原用途、写原资金消费并解除阻断。
///
/// 调用方必须已写入同事务的精确 Take 证明与消费行（原资金消费以消费行为外键），并已取得 funding worlds
/// 锁；本原语取 UID 账户锁。同命令重放无新增；其余已消费的重复调用为冲突。
#[allow(dead_code, reason = "由第 6 期 6D5 的消费事务调用，集成前尚无调用方")]
pub(crate) async fn return_cash_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    facts: &ReturnConsumptionFacts,
) -> Result<(), FundingError> {
    if facts.contract_id.is_nil()
        || facts.intake_id.is_nil()
        || facts.consumption_id.is_nil()
        || facts.original_delivery_id.is_nil()
        || facts.amount <= 0
        || facts.proof_digest == [0; 32]
    {
        return Err(FundingError::Invalid);
    }
    let sale = sale_facts(tx, facts.contract_id).await?;
    let account =
        lock_funding_account(tx, &sale.funding_uid).await?.ok_or(FundingError::Unproven)?;
    let uid = account.uid().to_owned();
    if replay_command(tx, &uid, &facts.mutation, ACTION, facts.consumption_id).await? {
        return Ok(());
    }
    // 另一条命令已经为同一消费或同一旧 delivery 返还过：不产生第二次恢复。
    let already: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
               SELECT 1 FROM public.consignment_funding_returns
               WHERE consumption_id=$1 OR original_delivery_id=$2
                  OR (intake_id=$3 AND leg=$4)
           )"#,
    )
    .bind(facts.consumption_id)
    .bind(facts.original_delivery_id)
    .bind(facts.intake_id)
    .bind(leg_code(facts.leg))
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if already {
        return Err(FundingError::Conflict);
    }
    // 没有阻断行的退件不能凭“无消费行”放行：未知旧退件失败关闭。
    let block = sqlx::query(
        r#"SELECT id, uid, original_delivery_id, amount, original_mode, state
           FROM public.consignment_cash_return_blocks
           WHERE intake_id=$1 AND leg=$2
           FOR UPDATE"#,
    )
    .bind(facts.intake_id)
    .bind(leg_code(facts.leg))
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    let block_id: Uuid = block.try_get("id").map_err(storage)?;
    let mode: FundingMode =
        parse_mode(&block.try_get::<String, _>("original_mode").map_err(storage)?)?;
    if block.try_get::<String, _>("state").map_err(storage)? != "open"
        || block.try_get::<String, _>("uid").map_err(storage)? != uid
        || block.try_get::<Uuid, _>("original_delivery_id").map_err(storage)?
            != facts.original_delivery_id
        || block.try_get::<i64, _>("amount").map_err(storage)? != facts.amount
        || mode != sale.funding_mode
    {
        return Err(FundingError::Conflict);
    }
    // 消费必须真实存在并指认同一 intake、合同与腿（其证明由消费行的触发器要求同事务精确 Take）。
    let (obligation_id, original_round): (Uuid, i32) = sqlx::query_as(
        r#"SELECT consumption.obligation_id, consumption.original_round
           FROM public.consignment_return_consumptions AS consumption
           WHERE consumption.id=$1 AND consumption.intake_id=$2 AND consumption.contract_id=$3
             AND consumption.leg=$4"#,
    )
    .bind(facts.consumption_id)
    .bind(facts.intake_id)
    .bind(facts.contract_id)
    .bind(leg_code(facts.leg))
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    let purpose = leg_purpose(facts.leg);
    let original = locked_allocation(tx, &uid, purpose, facts.contract_id)
        .await?
        .ok_or(FundingError::Unproven)?;
    if original.state != "spent"
        || original.delivery_id != Some(facts.original_delivery_id)
        || original.amount != facts.amount
    {
        return Err(FundingError::Conflict);
    }
    // 原支出分录：以原 delivery 为命令键写入的 spent，复合绑定到原资金消费。
    let spent_entry_id: Uuid = sqlx::query_scalar(
        r#"SELECT id FROM public.settlement_funding_entries
           WHERE uid=$1 AND kind='spent' AND debit_id=$2 AND command_id=$3 AND amount=$4
           ORDER BY created_at DESC, id
           LIMIT 1"#,
    )
    .bind(&uid)
    .bind(original.id)
    .bind(facts.original_delivery_id)
    .bind(facts.amount)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    // 旧派发换代前登记历史绑定，返还后旧 delivery 仍能定位原账户。
    record_delivery_binding(tx, &account, facts.original_delivery_id, original.id).await?;
    let source_id = source(
        tx,
        &uid,
        "returned_delivery",
        facts.original_delivery_id,
        facts.amount,
        &facts.proof_digest,
    )
    .await?;
    let restored = sqlx::query(
        r#"UPDATE public.settlement_funding_allocations
           SET state='available', delivery_id=NULL, state_version=state_version+1
           WHERE id=$1 AND uid=$2 AND state='spent' AND amount=$3 AND delivery_id=$4"#,
    )
    .bind(original.id)
    .bind(&uid)
    .bind(facts.amount)
    .bind(facts.original_delivery_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    if restored.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    entry(
        tx,
        &uid,
        &facts.mutation,
        FundingEntry {
            debit: None,
            credit: Some(original.id),
            source: Some(source_id),
            amount: facts.amount,
            kind: "return",
        },
    )
    .await?;
    let funding_return_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO public.consignment_funding_returns
               (id, uid, contract_id, intake_id, consumption_id, leg, source_id,
                original_delivery_id, original_spent_entry_id, amount)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
    )
    .bind(funding_return_id)
    .bind(&uid)
    .bind(facts.contract_id)
    .bind(facts.intake_id)
    .bind(facts.consumption_id)
    .bind(leg_code(facts.leg))
    .bind(source_id)
    .bind(facts.original_delivery_id)
    .bind(spent_entry_id)
    .bind(facts.amount)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    let principal_settled = if facts.leg == BlockedLeg::CarrierPrincipal {
        settle_principal(tx, facts, &uid, mode, original.id, obligation_id, original_round).await?;
        true
    } else {
        false
    };
    let evidence = BlockClearanceEvidence {
        exact_take_proven: true,
        funding_consumed: true,
        returned_delivery_recorded: true,
        purpose_restored: true,
        principal_settled,
        take_disproved_and_permit_sealed: false,
    };
    if !may_clear_block(facts.leg, &evidence) {
        return Err(FundingError::Conflict);
    }
    let cleared = sqlx::query(
        r#"UPDATE public.consignment_cash_return_blocks
           SET state='cleared', cleared_by=$1, cleared_at=clock_timestamp()
           WHERE id=$2 AND state='open'"#,
    )
    .bind(funding_return_id)
    .bind(block_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    if cleared.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    complete(tx, &account, &facts.mutation, ACTION, facts.consumption_id).await
}

/// 载体本金腿：returned 的 1 回到原 carrier 后，严格现金回原自有，record_only 转入不可支用核销子账；
/// 同时记录 `rts_principal` 结论（与付款本金互斥）。
async fn settle_principal(
    tx: &mut Transaction<'_, Postgres>,
    facts: &ReturnConsumptionFacts,
    uid: &str,
    mode: FundingMode,
    carrier_allocation: Uuid,
    carrier_obligation_id: Uuid,
    original_round: i32,
) -> Result<(), FundingError> {
    // 载体本金固定 1 Credit；其他金额不是本金腿。
    if facts.amount != 1 {
        return Err(FundingError::Invalid);
    }
    let target = match mode {
        FundingMode::StrictCash => allocation(tx, uid, "proprietary", None, None).await?,
        FundingMode::RecordOnly => {
            contract_allocation(tx, uid, "carrier_writeoff", facts.contract_id, None).await?
        }
    };
    transfer(tx, uid, &facts.mutation, carrier_allocation, target, facts.amount).await?;
    record_principal_fact_in_tx(
        tx,
        &PrincipalFact {
            contract_id: facts.contract_id,
            carrier_obligation_id,
            original_delivery_id: facts.original_delivery_id,
            round: original_round,
            conclusion: PrincipalConclusion::RtsPrincipal,
            mode,
        },
    )
    .await
    .map(drop)
}
