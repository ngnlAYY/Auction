//! 全部现金Send统一资金门；准备/租约/外部first-poll仍由delivery owner拥有。

use super::{ledger::*, *};
use auction_application::{FundingDeliveryOutcome, FundingReturnKind, FundingReturnProof};

/// 精确可信origin映射，不从任意metadata授予资金权限；generic无映射明确延期。
pub async fn reserve_funded_delivery(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    delivery_id: Uuid,
) -> Result<Uuid, FundingError> {
    if replay(tx, account, command, "reserve", delivery_id).await? {
        return sqlx::query_scalar(
            "SELECT id FROM public.settlement_funding_allocations WHERE uid=$1 AND delivery_id=$2",
        )
        .bind(account.uid())
        .bind(delivery_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage);
    }
    let row = sqlx::query("SELECT d.payload,o.id AS obligation_id,o.obligation_kind,o.lot_id,o.amount,o.carrier_credit,
        o.world_id AS obligation_world,f.dispatch_world,f.delivery_operation,
        (SELECT e.world_id FROM public.equipment e WHERE e.id=o.equipment_id) AS equipment_world,
        EXISTS(SELECT 1 FROM public.lots l JOIN public.lot_settlements s ON s.lot_id=l.id
          WHERE l.id=o.lot_id AND l.auction_id=o.auction_id AND l.equipment_id=o.equipment_id
            AND s.buyer_amount=o.amount) AS original_lot_bound
        FROM public.deliveries d JOIN public.settlement_obligations o ON d.origin_kind='settlement' AND d.origin_id=o.id::text JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id AND f.delivery_id=d.id WHERE d.id=$1 AND d.status IN ('prepared','pending','claimed') AND o.business_state='ready'")
        .bind(delivery_id).fetch_optional(&mut **tx).await.map_err(storage)?.ok_or(FundingError::Unproven)?;
    let payload: serde_json::Value = row.try_get("payload").map_err(storage)?;
    let credits =
        payload.get("credits").and_then(serde_json::Value::as_i64).ok_or(FundingError::Invalid)?;
    if credits <= 0
        || payload.get("operation").and_then(serde_json::Value::as_str) != Some("send")
        || payload.get("world").and_then(serde_json::Value::as_str) != Some("main")
    {
        return Err(FundingError::Invalid);
    }
    let kind: String = row.try_get("obligation_kind").map_err(storage)?;
    let lot: Uuid = row.try_get("lot_id").map_err(storage)?;
    let obligation: Uuid = row.try_get("obligation_id").map_err(storage)?;
    let purpose = match kind.as_str() {
        "seller_payout" => "seller",
        "buyer_refund" => "refund",
        "winner_payment_carrier_cod" => "carrier",
        _ => return Err(FundingError::Unproven),
    };
    let expected: i64 = row
        .try_get(if purpose == "carrier" { "carrier_credit" } else { "amount" })
        .map_err(storage)?;
    if expected != credits {
        return Err(FundingError::Conflict);
    }
    let allocation_id = allocation(tx, account.uid(), purpose, Some(lot), Some(obligation)).await?;
    let current_amount: i64 =
        sqlx::query_scalar("SELECT amount FROM public.settlement_funding_allocations WHERE id=$1")
            .bind(allocation_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage)?;
    if purpose == "carrier" && current_amount == 0 {
        if account.records_only() {
            // 仅原Isekai付款的Main 1C载体记账；generic支出不能据此创建任意会计来源。
            let buyer: i64 = row.try_get("amount").map_err(storage)?;
            let cod = buyer.checked_add(1).ok_or(FundingError::Invalid)?;
            let expected = serde_json::json!({"schema_version":1,"operation":"send","world":"main",
                "template":"settlement_payment_carrier","variables":{"reference":obligation.to_string(),"amount":cod.to_string()},
                "credits":1,"cod":cod,"equipment_secret_refs":[]});
            if credits != 1
                || buyer <= 0
                || payload != expected
                || row.try_get::<String, _>("obligation_world").map_err(storage)? != "isekai"
                || row.try_get::<String, _>("equipment_world").map_err(storage)? != "isekai"
                || row.try_get::<String, _>("dispatch_world").map_err(storage)? != "main"
                || row.try_get::<String, _>("delivery_operation").map_err(storage)? != "send"
                || !row.try_get::<bool, _>("original_lot_bound").map_err(storage)?
            {
                return Err(FundingError::Unproven);
            }
            let source = source(
                tx,
                account.uid(),
                "record_carrier",
                delivery_id,
                1,
                &command.request_digest,
            )
            .await?;
            issue(tx, account.uid(), command, source, allocation_id, 1).await?;
        } else {
            let proprietary = allocation(tx, account.uid(), "proprietary", None, None).await?;
            transfer(tx, account.uid(), command, proprietary, allocation_id, credits).await?;
        }
    }
    let changed = sqlx::query("UPDATE public.settlement_funding_allocations SET state='reserved',delivery_id=$1,state_version=state_version+1 WHERE id=$2 AND uid=$3 AND state='available' AND amount=$4 AND (delivery_id IS NULL OR delivery_id=$1)")
        .bind(delivery_id).bind(allocation_id).bind(account.uid()).bind(credits).execute(&mut **tx).await.map_err(storage)?;
    if changed.rows_affected() != 1 {
        return Err(FundingError::Unavailable);
    }
    entry(
        tx,
        account.uid(),
        command,
        FundingEntry {
            debit: Some(allocation_id),
            credit: Some(allocation_id),
            source: None,
            amount: credits,
            kind: "reserve",
        },
    )
    .await?;
    complete(tx, account, command, "reserve", delivery_id).await?;
    Ok(allocation_id)
}

/// 与sending fence同事务；先核验新鲜真实cash且提交后续租成功，才可首次poll外部future。
pub async fn start_funded_delivery(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    delivery_id: Uuid,
) -> Result<(), FundingError> {
    super::account::verify_locked(tx, account, true).await?;
    let row = sqlx::query("UPDATE public.settlement_funding_allocations SET state='in_flight',state_version=state_version+1 WHERE uid=$1 AND delivery_id=$2 AND state='reserved' RETURNING id,amount")
        .bind(account.uid()).bind(delivery_id).fetch_optional(&mut **tx).await.map_err(storage)?.ok_or(FundingError::Conflict)?;
    state_entry(tx, account, delivery_id, &row, "in_flight").await
}

async fn state_entry(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    delivery_id: Uuid,
    row: &sqlx::postgres::PgRow,
    state: &str,
) -> Result<(), FundingError> {
    let hash: Vec<u8> =
        sqlx::query_scalar("SELECT request_digest FROM public.deliveries WHERE id=$1")
            .bind(delivery_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage)?;
    let command = FundingMutation {
        command_id: delivery_id,
        request_digest: hash.try_into().map_err(invalid)?,
    };
    let id: Uuid = row.try_get("id").map_err(storage)?;
    entry(
        tx,
        account.uid(),
        &command,
        FundingEntry {
            debit: Some(id),
            credit: Some(id),
            source: None,
            amount: row.try_get("amount").map_err(storage)?,
            kind: state,
        },
    )
    .await?;
    sqlx::query("UPDATE public.settlement_funding_accounts SET state_version=state_version+1,updated_at=clock_timestamp() WHERE uid=$1")
        .bind(account.uid()).execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}

/// Sent是外流不是available；未知、人工failed、丢租约统一保留占用。
pub async fn finish_funded_delivery(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    delivery_id: Uuid,
    outcome: FundingDeliveryOutcome,
) -> Result<(), FundingError> {
    super::account::verify_locked(tx, account, false).await?;
    let state = if outcome == FundingDeliveryOutcome::Sent { "spent" } else { "unknown" };
    let row = sqlx::query("SELECT id,amount,state FROM public.settlement_funding_allocations WHERE uid=$1 AND delivery_id=$2 FOR UPDATE")
        .bind(account.uid()).bind(delivery_id).fetch_one(&mut **tx).await.map_err(storage)?;
    let previous: String = row.try_get("state").map_err(storage)?;
    if previous == state {
        return Ok(());
    }
    if previous != "in_flight" && previous != "reserved" && previous != "unknown" {
        return Err(FundingError::Conflict);
    }
    if state == "spent" && previous == "reserved" {
        return Err(FundingError::Conflict);
    }
    sqlx::query("UPDATE public.settlement_funding_allocations SET state=$1,state_version=state_version+1 WHERE id=$2")
        .bind(state).bind(row.try_get::<Uuid,_>("id").map_err(storage)?).execute(&mut **tx).await.map_err(storage)?;
    state_entry(tx, account, delivery_id, &row, state).await
}

/// 只有精确未执行或真实退回到账才能返还；不得用admin failed或余额变化代替证明。
pub async fn return_funded_delivery(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    delivery_id: Uuid,
    proof: &FundingReturnProof,
) -> Result<(), FundingError> {
    if replay(tx, account, command, "return", delivery_id).await? {
        return Ok(());
    }
    if proof.digest == [0; 32]
        || proof.amount <= 0
        || proof.delivery_id != delivery_id
        || !proof.binding.validate()
        || proof.binding.authenticated_uid != account.uid()
        || proof.binding.world != auction_domain::SettlementWorld::Main
        || if account.records_only() {
            proof.cash.is_some()
        } else {
            proof.cash.as_ref().is_none_or(|cash| {
                !proof.binding.same_account(&cash.binding)
                    || cash.binding.world != auction_domain::SettlementWorld::Main
            })
        }
        || proof.proof_reference.is_empty()
        || proof.proof_reference.len() > 256
        || proof.proof_reference.trim() != proof.proof_reference
        || proof.proof_reference.chars().any(char::is_control)
    {
        return Err(FundingError::Invalid);
    }
    let fresh:bool=sqlx::query_scalar("SELECT $1 BETWEEN clock_timestamp()-interval '30 seconds' AND clock_timestamp() AND $2 BETWEEN clock_timestamp()-interval '30 seconds' AND clock_timestamp()")
        .bind(proof.observed_at).bind(proof.binding.observed_at).fetch_one(&mut **tx).await.map_err(storage)?;
    if !fresh {
        return Err(FundingError::Unproven);
    }
    let row = sqlx::query("SELECT id,amount,state FROM public.settlement_funding_allocations WHERE uid=$1 AND delivery_id=$2 FOR UPDATE")
        .bind(account.uid()).bind(delivery_id).fetch_one(&mut **tx).await.map_err(storage)?;
    let state: String = row.try_get("state").map_err(storage)?;
    let amount: i64 = row.try_get("amount").map_err(storage)?;
    let id: Uuid = row.try_get("id").map_err(storage)?;
    if amount != proof.amount
        || state == "available"
        || (proof.kind == FundingReturnKind::DefinitelyNotExecuted && state == "spent")
    {
        return Err(FundingError::Conflict);
    }
    // 返还恢复原用途，不授权再投资，也不删除原delivery关联用于阻止同一source二次返还。
    sqlx::query("UPDATE public.settlement_funding_allocations SET state='available',state_version=state_version+1 WHERE id=$1")
        .bind(id).execute(&mut **tx).await.map_err(storage)?;
    let local:i64=sqlx::query_scalar("SELECT COALESCE(sum(amount),0)::bigint FROM public.settlement_funding_allocations WHERE uid=$1 AND state<>'spent'")
        .bind(account.uid()).fetch_one(&mut **tx).await.map_err(storage)?;
    if let Some(cash) = &proof.cash {
        let fresh: bool = sqlx::query_scalar(
            "SELECT $1 BETWEEN clock_timestamp()-interval '30 seconds' AND clock_timestamp()",
        )
        .bind(cash.observed_at)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage)?;
        if !fresh || local > cash.credits {
            return Err(FundingError::Unproven);
        }
    }
    let evidence = serde_json::json!({"kind":match proof.kind {FundingReturnKind::DefinitelyNotExecuted=>"definitely_not_executed",FundingReturnKind::ReturnedAndReceived=>"returned_and_received"},
        "reference":proof.proof_reference,"amount":amount,"observed_at":proof.observed_at,
        "uid":proof.binding.authenticated_uid,"credential_id":proof.binding.credential_id,
        "credential_version":proof.binding.credential_version,"runtime_generation":proof.binding.runtime.generation,
        "cash_credits":proof.cash.as_ref().map(|cash|cash.credits),"cash_observed_at":proof.cash.as_ref().map(|cash|cash.observed_at),
        "cash_digest":proof.cash.as_ref().map(|cash|cash.digest),"accounting_mode":if account.records_only(){"record_only"}else{"strict_cash"}});
    sqlx::query("INSERT INTO public.settlement_funding_returns(delivery_id,command_id,proof_digest,evidence) VALUES($1,$2,$3,$4)")
        .bind(delivery_id).bind(command.command_id).bind(proof.digest.as_slice()).bind(evidence).execute(&mut **tx).await.map_err(storage)?;
    let proof_command =
        FundingMutation { command_id: command.command_id, request_digest: proof.digest };
    entry(
        tx,
        account.uid(),
        &proof_command,
        FundingEntry { debit: Some(id), credit: Some(id), source: None, amount, kind: "return" },
    )
    .await?;
    complete(tx, account, command, "return", delivery_id).await
}
