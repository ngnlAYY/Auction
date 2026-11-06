//! 来源唯一、稳定命令与用途转移；所有数量改变均追加分录。

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement_funding::account::LockedFundingAccount;
use crate::database::settlement_funding::storage;
use auction_application::settlement::{FundingError, FundingMutation};
use sqlx::{Postgres, Row};
use uuid::Uuid;

pub(super) async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    action: &str,
    target: Uuid,
) -> Result<bool, FundingError> {
    super::account::verify_locked(tx, account, false).await?;
    if command.command_id.is_nil() || command.request_digest == [0; 32] {
        return Err(FundingError::Invalid);
    }
    let row = sqlx::query("SELECT uid,action,target_id,request_digest FROM public.settlement_funding_commands WHERE command_id=$1")
        .bind(command.command_id).fetch_optional(&mut **tx).await.map_err(storage)?;
    if let Some(row) = row {
        if row.try_get::<String, _>("uid").map_err(storage)? != account.uid
            || row.try_get::<String, _>("action").map_err(storage)? != action
            || row.try_get::<Uuid, _>("target_id").map_err(storage)? != target
            || row.try_get::<Vec<u8>, _>("request_digest").map_err(storage)?
                != command.request_digest
        {
            return Err(FundingError::Conflict);
        }
        return Ok(true);
    }
    let ready: bool = sqlx::query_scalar(
        "SELECT status='ready' FROM public.settlement_funding_accounts WHERE uid=$1",
    )
    .bind(account.uid())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if !ready && action != "return" {
        return Err(FundingError::Unavailable);
    }
    Ok(false)
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    action: &str,
    target: Uuid,
) -> Result<(), FundingError> {
    sqlx::query("INSERT INTO public.settlement_funding_commands(command_id,uid,action,target_id,request_digest,result) VALUES ($1,$2,$3,$4,$5,'{}')")
        .bind(command.command_id).bind(account.uid()).bind(action).bind(target).bind(command.request_digest.as_slice())
        .execute(&mut **tx).await.map_err(storage)?;
    sqlx::query("UPDATE public.settlement_funding_accounts SET state_version=state_version+1,updated_at=clock_timestamp() WHERE uid=$1")
        .bind(account.uid()).execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}

pub(super) async fn allocation(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    purpose: &str,
    lot: Option<Uuid>,
    obligation: Option<Uuid>,
) -> Result<Uuid, FundingError> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.settlement_funding_allocations(id,uid,purpose,lot_id,obligation_id,amount) VALUES($1,$2,$3,$4,$5,0) ON CONFLICT (uid,purpose,lot_id) DO NOTHING")
        .bind(id).bind(uid).bind(purpose).bind(lot).bind(obligation).execute(&mut **tx).await.map_err(storage)?;
    let row = sqlx::query("SELECT id,obligation_id FROM public.settlement_funding_allocations WHERE uid=$1 AND purpose=$2 AND lot_id IS NOT DISTINCT FROM $3 FOR UPDATE")
        .bind(uid).bind(purpose).bind(lot).fetch_one(&mut **tx).await.map_err(storage)?;
    if row.try_get::<Option<Uuid>, _>("obligation_id").map_err(storage)? != obligation {
        return Err(FundingError::Conflict);
    }
    row.try_get("id").map_err(storage)
}

pub(super) async fn source(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    kind: &str,
    key: Uuid,
    amount: i64,
    digest: &[u8; 32],
) -> Result<Uuid, FundingError> {
    if amount < 0 || *digest == [0; 32] {
        return Err(FundingError::Invalid);
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.settlement_funding_sources(id,uid,kind,source_key,amount,proof_digest) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(id).bind(uid).bind(kind).bind(key).bind(amount).bind(digest.as_slice())
        .execute(&mut **tx).await.map_err(storage)?;
    Ok(id)
}

pub(super) struct FundingEntry<'a> {
    pub debit: Option<Uuid>,
    pub credit: Option<Uuid>,
    pub source: Option<Uuid>,
    pub amount: i64,
    pub kind: &'a str,
}

pub(super) async fn entry(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command: &FundingMutation,
    movement: FundingEntry<'_>,
) -> Result<(), FundingError> {
    let FundingEntry { debit, credit, source, amount, kind } = movement;
    if amount == 0 {
        return Ok(());
    }
    if amount < 0 {
        return Err(FundingError::Invalid);
    }
    sqlx::query("INSERT INTO public.settlement_funding_entries(id,command_id,uid,debit_id,credit_id,source_id,amount,kind,proof_digest) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(Uuid::new_v4()).bind(command.command_id).bind(uid).bind(debit).bind(credit).bind(source)
        .bind(amount).bind(kind).bind(command.request_digest.as_slice()).execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}

pub(super) async fn issue(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command: &FundingMutation,
    source: Uuid,
    credit: Uuid,
    amount: i64,
) -> Result<(), FundingError> {
    let (limit, used): (i64,i64) = sqlx::query_as("SELECT s.amount,COALESCE((SELECT sum(e.amount) FROM public.settlement_funding_entries e WHERE e.source_id=s.id AND e.kind='issue'),0)::bigint FROM public.settlement_funding_sources s WHERE s.id=$1 AND s.uid=$2")
        .bind(source).bind(uid).fetch_one(&mut **tx).await.map_err(storage)?;
    if amount < 0 || used.checked_add(amount).filter(|sum| *sum <= limit).is_none() {
        return Err(FundingError::Conflict);
    }
    let changed = sqlx::query("UPDATE public.settlement_funding_allocations SET amount=amount+$1,state_version=state_version+1 WHERE id=$2 AND uid=$3 AND state='available'")
        .bind(amount).bind(credit).bind(uid).execute(&mut **tx).await.map_err(storage)?;
    if changed.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    entry(
        tx,
        uid,
        command,
        FundingEntry {
            debit: None,
            credit: Some(credit),
            source: Some(source),
            amount,
            kind: "issue",
        },
    )
    .await
}

pub(super) async fn transfer(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command: &FundingMutation,
    debit: Uuid,
    credit: Uuid,
    amount: i64,
) -> Result<(), FundingError> {
    if amount < 0 || debit == credit {
        return Err(FundingError::Invalid);
    }
    let changed = sqlx::query("UPDATE public.settlement_funding_allocations SET amount=amount-$1,state_version=state_version+1 WHERE id=$2 AND uid=$3 AND amount >= $1 AND state='available'")
        .bind(amount).bind(debit).bind(uid).execute(&mut **tx).await.map_err(storage)?;
    if changed.rows_affected() != 1 {
        return Err(FundingError::Unavailable);
    }
    let changed = sqlx::query("UPDATE public.settlement_funding_allocations SET amount=amount+$1,state_version=state_version+1 WHERE id=$2 AND uid=$3 AND state='available'")
        .bind(amount).bind(credit).bind(uid).execute(&mut **tx).await.map_err(storage)?;
    if changed.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    entry(
        tx,
        uid,
        command,
        FundingEntry {
            debit: Some(debit),
            credit: Some(credit),
            source: None,
            amount,
            kind: "transfer",
        },
    )
    .await
}
