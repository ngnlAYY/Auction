//! 结算退件领回后的资金返还：凭精确 Take 证明把原派发的 spent 分配恢复为 available。
//!
//! 返还只写一次：`settlement_funding_returns` 以原投递为主键，并唯一绑定退件案件、Take 投递与
//! `returned_delivery` 来源；分配金额保持原额 M（不是 2M），恢复原用途（货款或退款），不重新执行
//! 付款分配或 seller→refund 转换。补寄由新投递重新经 reserve/in_flight/spent 资金门。被冻结账户
//! 仍可记录已发生的真实回收，但不因此解冻或获准补寄。

use auction_application::settlement::{FundingError, FundingMutation};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::account::LockedFundingAccount;
use crate::database::settlement::funding::ledger::{FundingEntry, complete, entry, replay, source};
use crate::database::settlement::funding::storage;

/// 一次返还的入站与 Take 绑定；金额与证明摘要来自已核验的领回事实。
#[derive(Debug, Clone, Copy)]
pub struct FundingReturnBinding<'a> {
    pub return_mail_id: Uuid,
    pub take_delivery_id: Uuid,
    /// 领回证明确认的 Credits；必须等于原分配金额。
    pub credits: i64,
    pub proof_digest: &'a [u8; 32],
}

/// 原派发是否仍有可返还的已支出分配（当前指针指向它且状态为 spent）。调用方已持资金锁前缀。
pub async fn funded_delivery_spent(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    delivery_id: Uuid,
) -> Result<bool, FundingError> {
    sqlx::query_scalar(
        r#"SELECT EXISTS(
               SELECT 1 FROM public.settlement_funding_allocations
               WHERE uid=$1 AND delivery_id=$2 AND state='spent'
                 AND purpose IN ('seller','refund')
           )"#,
    )
    .bind(account.uid())
    .bind(delivery_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)
}

/// 返还原派发的已支出分配。同一命令重放返回原结果；原分配不是精确的 spent 原额、来源或返还事实
/// 已存在时失败关闭（不变量 1、6、8）。调用方已持资金锁前缀并在同一事务内写入领回事实。
pub async fn return_funded_delivery(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    original_delivery_id: Uuid,
    binding: FundingReturnBinding<'_>,
) -> Result<Uuid, FundingError> {
    if original_delivery_id.is_nil()
        || binding.return_mail_id.is_nil()
        || binding.take_delivery_id.is_nil()
        || binding.credits <= 0
        || *binding.proof_digest == [0; 32]
    {
        return Err(FundingError::Invalid);
    }
    if replay(tx, account, command, "return", original_delivery_id).await? {
        return sqlx::query_scalar(
            r#"SELECT allocation_id FROM public.settlement_funding_delivery_bindings
               WHERE delivery_id=$1 AND uid=$2"#,
        )
        .bind(original_delivery_id)
        .bind(account.uid())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage);
    }
    let row = sqlx::query(
        r#"SELECT id, amount, state, purpose
           FROM public.settlement_funding_allocations
           WHERE uid=$1 AND delivery_id=$2
           FOR UPDATE"#,
    )
    .bind(account.uid())
    .bind(original_delivery_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    let allocation_id: Uuid = row.try_get("id").map_err(storage)?;
    let amount: i64 = row.try_get("amount").map_err(storage)?;
    let state: String = row.try_get("state").map_err(storage)?;
    let purpose: String = row.try_get("purpose").map_err(storage)?;
    if state != "spent"
        || !matches!(purpose.as_str(), "seller" | "refund")
        || amount != binding.credits
    {
        return Err(FundingError::Conflict);
    }
    // 旧派发换代前先登记历史绑定（reserve 时通常已登记），返还后旧投递仍能定位原账户。
    record_delivery_binding(tx, account, original_delivery_id, allocation_id).await?;
    let source_id = source(
        tx,
        account.uid(),
        "returned_delivery",
        original_delivery_id,
        amount,
        binding.proof_digest,
    )
    .await?;
    sqlx::query(
        r#"INSERT INTO public.settlement_funding_returns
               (delivery_id, command_id, proof_digest, evidence, return_mail_id,
                take_delivery_id, source_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(original_delivery_id)
    .bind(command.command_id)
    .bind(binding.proof_digest.as_slice())
    .bind(serde_json::json!({
        "schema_version": 1,
        "kind": "settlement_return_take",
        "allocation_id": allocation_id,
        "purpose": purpose,
        "credits": amount,
    }))
    .bind(binding.return_mail_id)
    .bind(binding.take_delivery_id)
    .bind(source_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    let changed = sqlx::query(
        r#"UPDATE public.settlement_funding_allocations
           SET state='available',
               delivery_id=NULL,
               state_version=state_version+1
           WHERE id=$1 AND uid=$2 AND state='spent' AND amount=$3 AND delivery_id=$4"#,
    )
    .bind(allocation_id)
    .bind(account.uid())
    .bind(amount)
    .bind(original_delivery_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    if changed.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    entry(
        tx,
        account.uid(),
        command,
        FundingEntry {
            debit: None,
            credit: Some(allocation_id),
            source: Some(source_id),
            amount,
            kind: "return",
        },
    )
    .await?;
    complete(tx, account, command, "return", original_delivery_id).await?;
    Ok(allocation_id)
}

/// 只追加登记投递曾占用的分配；同一投递已登记为同一分配时幂等，绑定到别的分配失败关闭。
pub(super) async fn record_delivery_binding(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    delivery_id: Uuid,
    allocation_id: Uuid,
) -> Result<(), FundingError> {
    sqlx::query(
        r#"INSERT INTO public.settlement_funding_delivery_bindings (delivery_id, allocation_id, uid)
           VALUES ($1, $2, $3)
           ON CONFLICT (delivery_id) DO NOTHING"#,
    )
    .bind(delivery_id)
    .bind(allocation_id)
    .bind(account.uid())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    let bound: (Uuid, String) = sqlx::query_as(
        r#"SELECT allocation_id, uid FROM public.settlement_funding_delivery_bindings
           WHERE delivery_id=$1"#,
    )
    .bind(delivery_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if bound != (allocation_id, account.uid().to_owned()) {
        return Err(FundingError::Conflict);
    }
    Ok(())
}
