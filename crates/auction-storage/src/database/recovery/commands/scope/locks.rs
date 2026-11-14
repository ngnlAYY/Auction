//! Recovery 事务中的范围、拍卖、义务和 action 锁定。

use crate::database::TrackedTransaction as Transaction;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::recovery::commands::scope::{
    ActionRow, AuctionLock, ObligationRow, OriginalLot, RecoveryRow, RecoveryScope,
};
use auction_application::recovery::RecoveryCommandError;
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};
use std::collections::BTreeMap;
use uuid::Uuid;

pub(in crate::database::recovery::commands) async fn load_recovery_scope(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
) -> Result<RecoveryScope, RecoveryCommandError> {
    let row = sqlx::query(
        r#"SELECT auction_id, lot_id, world_id, equipment_id
           FROM public.settlement_recoveries
           WHERE id = $1"#,
    )
    .bind(recovery_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::NotFound)?;
    Ok(RecoveryScope {
        auction_id: row.try_get("auction_id").map_err(map_sql_error)?,
        lot_id: row.try_get("lot_id").map_err(map_sql_error)?,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        equipment_id: row.try_get("equipment_id").map_err(map_sql_error)?,
    })
}

pub(in crate::database::recovery::commands) async fn lock_world(
    transaction: &mut Transaction<'_, Postgres>,
    world_id: &str,
) -> Result<(), RecoveryCommandError> {
    let exists = sqlx::query("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
        .bind(world_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .is_some();
    if exists { Ok(()) } else { Err(RecoveryCommandError::NotFound) }
}

pub(in crate::database::recovery::commands) async fn lock_auction(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
) -> Result<AuctionLock, RecoveryCommandError> {
    let row = sqlx::query("SELECT world_id, status FROM public.auctions WHERE id = $1 FOR UPDATE")
        .bind(auction_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .ok_or(RecoveryCommandError::NotFound)?;
    Ok(AuctionLock {
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        status: row.try_get("status").map_err(map_sql_error)?,
    })
}

pub(in crate::database::recovery::commands) async fn lock_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
) -> Result<RecoveryRow, RecoveryCommandError> {
    let row = sqlx::query(
        r#"SELECT id, auction_id, lot_id, world_id, equipment_id,
                  payment_obligation_id, equipment_obligation_id,
                  seller_payout_obligation_id, buyer_refund_obligation_id,
                  stage, resolution, state_version, blocking_code,
                  manual_reconcile_delivery_id, relisted_auction_id, relisted_lot_id,
                  custody_proof_kind, custody_proof_reference, custody_proof_digest,
                  custody_proof_observed_at, created_at
           FROM public.settlement_recoveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(recovery_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::NotFound)?;
    Ok(RecoveryRow {
        id: row.try_get("id").map_err(map_sql_error)?,
        auction_id: row.try_get("auction_id").map_err(map_sql_error)?,
        lot_id: row.try_get("lot_id").map_err(map_sql_error)?,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        equipment_id: row.try_get("equipment_id").map_err(map_sql_error)?,
        payment_obligation_id: row.try_get("payment_obligation_id").map_err(map_sql_error)?,
        equipment_obligation_id: row.try_get("equipment_obligation_id").map_err(map_sql_error)?,
        seller_payout_obligation_id: row
            .try_get("seller_payout_obligation_id")
            .map_err(map_sql_error)?,
        buyer_refund_obligation_id: row
            .try_get("buyer_refund_obligation_id")
            .map_err(map_sql_error)?,
        stage: row.try_get("stage").map_err(map_sql_error)?,
        resolution: row.try_get("resolution").map_err(map_sql_error)?,
        state_version: row.try_get("state_version").map_err(map_sql_error)?,
        blocking_code: row.try_get("blocking_code").map_err(map_sql_error)?,
        manual_reconcile_delivery_id: row
            .try_get("manual_reconcile_delivery_id")
            .map_err(map_sql_error)?,
        relisted_auction_id: row.try_get("relisted_auction_id").map_err(map_sql_error)?,
        relisted_lot_id: row.try_get("relisted_lot_id").map_err(map_sql_error)?,
        custody_proof_kind: row.try_get("custody_proof_kind").map_err(map_sql_error)?,
        custody_proof_reference: row.try_get("custody_proof_reference").map_err(map_sql_error)?,
        custody_proof_digest: row.try_get("custody_proof_digest").map_err(map_sql_error)?,
        custody_proof_observed_at: row
            .try_get("custody_proof_observed_at")
            .map_err(map_sql_error)?,
        created_at: row.try_get("created_at").map_err(map_sql_error)?,
    })
}

pub(in crate::database::recovery::commands) async fn lock_original_lot(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    lot_id: Uuid,
    equipment_id: Uuid,
) -> Result<OriginalLot, RecoveryCommandError> {
    let row = sqlx::query(
        r#"SELECT lot.equipment_id, lot.submission_id, lot.status, lot.start_price,
                  equipment.world_id AS equipment_world_id,
                  equipment.category AS equipment_category
           FROM public.lots AS lot
           INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
           WHERE lot.id = $1 AND lot.auction_id = $2 AND lot.equipment_id = $3
           FOR UPDATE OF lot, equipment"#,
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::NotFound)?;
    Ok(OriginalLot {
        equipment_id: row.try_get("equipment_id").map_err(map_sql_error)?,
        submission_id: row.try_get("submission_id").map_err(map_sql_error)?,
        status: row.try_get("status").map_err(map_sql_error)?,
        start_price: row.try_get("start_price").map_err(map_sql_error)?,
        equipment_world_id: row.try_get("equipment_world_id").map_err(map_sql_error)?,
        equipment_category: row.try_get("equipment_category").map_err(map_sql_error)?,
    })
}

pub(in crate::database::recovery::commands) async fn lock_obligations(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
) -> Result<BTreeMap<Uuid, ObligationRow>, RecoveryCommandError> {
    let mut ids = vec![
        recovery.payment_obligation_id,
        recovery.seller_payout_obligation_id,
        recovery.buyer_refund_obligation_id,
    ];
    if let Some(id) = recovery.equipment_obligation_id {
        ids.push(id);
    }
    ids.sort_unstable();
    ids.dedup();

    // 与原逐 ID 加锁保持相同 UUID 顺序；一条查询锁齐完整集合，缺失任何义务都失败关闭。
    let rows = sqlx::query(
        r#"SELECT id, auction_id, lot_id, world_id, obligation_kind, business_state,
                  equipment_id, amount, recipient_user_id, recipient_game_identity
           FROM public.settlement_obligations
           WHERE id = ANY($1)
           ORDER BY id
           FOR UPDATE"#,
    )
    .bind(&ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if rows.len() != ids.len() {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    let mut obligations = BTreeMap::new();
    for (id, row) in ids.into_iter().zip(rows) {
        let obligation = ObligationRow {
            id: row.try_get("id").map_err(map_sql_error)?,
            auction_id: row.try_get("auction_id").map_err(map_sql_error)?,
            lot_id: row.try_get("lot_id").map_err(map_sql_error)?,
            world_id: row.try_get("world_id").map_err(map_sql_error)?,
            kind: row.try_get("obligation_kind").map_err(map_sql_error)?,
            state: row.try_get("business_state").map_err(map_sql_error)?,
            equipment_id: row.try_get("equipment_id").map_err(map_sql_error)?,
            amount: row.try_get("amount").map_err(map_sql_error)?,
            recipient_user_id: row.try_get("recipient_user_id").map_err(map_sql_error)?,
            recipient_game_identity: row
                .try_get("recipient_game_identity")
                .map_err(map_sql_error)?,
        };
        let world_matches = obligation.world_id == recovery.world_id
            || (obligation.kind == "winner_payment_carrier_cod"
                && obligation.world_id == "main"
                && recovery.world_id == "isekai");
        if obligation.id != id
            || obligation.auction_id != recovery.auction_id
            || obligation.lot_id != recovery.lot_id
            || !world_matches
            || obligation.equipment_id != recovery.equipment_id
        {
            return Err(RecoveryCommandError::InvalidStoredResult);
        }
        obligations.insert(id, obligation);
    }
    Ok(obligations)
}

pub(in crate::database::recovery::commands) async fn lock_actions(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
) -> Result<Vec<ActionRow>, RecoveryCommandError> {
    let rows = sqlx::query(
        r#"SELECT id, kind, execution_state, delivery_id, proof_kind,
                  proof_reference, proof_digest, proof_observed_at
           FROM public.settlement_recovery_actions
           WHERE recovery_id = $1
           ORDER BY id
           FOR UPDATE"#,
    )
    .bind(recovery_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(ActionRow {
                id: row.try_get("id").map_err(map_sql_error)?,
                kind: row.try_get("kind").map_err(map_sql_error)?,
                state: row.try_get("execution_state").map_err(map_sql_error)?,
                delivery_id: row.try_get("delivery_id").map_err(map_sql_error)?,
                proof_kind: row.try_get("proof_kind").map_err(map_sql_error)?,
                proof_reference: row.try_get("proof_reference").map_err(map_sql_error)?,
                proof_digest: row.try_get("proof_digest").map_err(map_sql_error)?,
                proof_observed_at: row.try_get("proof_observed_at").map_err(map_sql_error)?,
            })
        })
        .collect()
}

pub(in crate::database::recovery::commands) async fn lock_fulfillment_state(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
) -> Result<Option<String>, RecoveryCommandError> {
    let row = sqlx::query(
        "SELECT execution_state FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1 FOR UPDATE",
    )
    .bind(obligation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    row.map(|row| row.try_get("execution_state").map_err(map_sql_error)).transpose()
}

pub(in crate::database::recovery::commands) async fn database_now(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<DateTime<Utc>, RecoveryCommandError> {
    // world/recovery 等锁等待可能跨过换季边界；事务起始时间不能作为重挂许可依据。
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)
}
