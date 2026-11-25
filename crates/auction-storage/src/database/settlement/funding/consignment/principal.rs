//! isekai 载体本金的结论事实：每 `(合同, 载体义务, 原 delivery, 轮)` 至多一个，付款本金与 RTS 本金互斥。
//!
//! 保护不变量 6：本金只属于 isekai 的载体付款义务；同一槽位出现相反结论失败关闭，相同结论是幂等重放。

use auction_application::settlement::{CodLeg, FundingError};
use auction_domain::consignment::funding::{
    FundingMode, FundingRuleError, PrincipalConclusion, resolve_principal_conclusion,
};
use sqlx::Postgres;
use uuid::Uuid;

use super::contract_allocations::mode_code;
use super::receipt::funding_sql_error;
use super::world_leg::checked_payment_leg;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::{invalid, storage};

/// 载体本金固定 1 Credit。
const CARRIER_PRINCIPAL: i64 = 1;

/// 一个本金结论槽位及其结论。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PrincipalFact {
    pub(crate) contract_id: Uuid,
    pub(crate) carrier_obligation_id: Uuid,
    pub(crate) original_delivery_id: Uuid,
    pub(crate) round: i32,
    pub(crate) conclusion: PrincipalConclusion,
    pub(crate) mode: FundingMode,
}

const fn conclusion_code(conclusion: PrincipalConclusion) -> &'static str {
    match conclusion {
        PrincipalConclusion::PaymentPrincipal => "payment_principal",
        PrincipalConclusion::RtsPrincipal => "rts_principal",
    }
}

fn parse_conclusion(value: &str) -> Result<PrincipalConclusion, FundingError> {
    [PrincipalConclusion::PaymentPrincipal, PrincipalConclusion::RtsPrincipal]
        .into_iter()
        .find(|conclusion| conclusion_code(*conclusion) == value)
        .ok_or(FundingError::Unproven)
}

/// 记录本金结论；新写入返回 `true`，同结论重放返回 `false`，相反结论为冲突。
///
/// 调用方已持 UID 账户锁（付款分账事务与退件消费事务各自在自己的锁序内调用）。
#[allow(dead_code, reason = "付款分账（6B2）与退件回补共用；6B2 集成前只有回补路径调用")]
pub(crate) async fn record_principal_fact_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    fact: &PrincipalFact,
) -> Result<bool, FundingError> {
    // 世界与收款腿互校走 `check_leg` 同源规则；本金只属于载体腿，原件腿的付款没有载体本金。
    let leg = checked_payment_leg(tx, fact.carrier_obligation_id, fact.contract_id).await?;
    if leg != CodLeg::Carrier {
        return Err(FundingError::Invalid);
    }
    let existing: Option<String> = sqlx::query_scalar(
        r#"SELECT conclusion FROM public.consignment_principal_facts
           WHERE contract_id=$1 AND carrier_obligation_id=$2 AND original_delivery_id=$3
             AND round=$4"#,
    )
    .bind(fact.contract_id)
    .bind(fact.carrier_obligation_id)
    .bind(fact.original_delivery_id)
    .bind(fact.round)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    let existing = existing.as_deref().map(parse_conclusion).transpose()?;
    match resolve_principal_conclusion(existing, fact.conclusion) {
        Ok(false) => return Ok(false),
        Ok(true) => {}
        Err(FundingRuleError::PrincipalConflict) => return Err(FundingError::Conflict),
        Err(error) => return Err(invalid(error)),
    }
    sqlx::query(
        r#"INSERT INTO public.consignment_principal_facts
               (id, contract_id, carrier_obligation_id, original_delivery_id, round, conclusion,
                mode, amount)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(fact.contract_id)
    .bind(fact.carrier_obligation_id)
    .bind(fact.original_delivery_id)
    .bind(fact.round)
    .bind(conclusion_code(fact.conclusion))
    .bind(mode_code(fact.mode))
    .bind(CARRIER_PRINCIPAL)
    .execute(&mut **tx)
    .await
    // 并发争同一槽位时败方撞 `c6_principal_once_uq`（23505）：这是确定冲突而非可重试的存储失败，
    // 整笔回滚（不变量 2、6）；序列化冲突仍按 40001 / 40P01 交由重试。
    .map_err(funding_sql_error)?;
    Ok(true)
}
