//! Recovery 锁定范围、状态校验与履约证据。
//!
//! 该文件只承载 PostgreSQL recovery command 的单一职责，事务装配仍由父模块统一负责。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(super) struct RecoveryScope {
    pub(super) auction_id: Uuid,
    pub(super) lot_id: Uuid,
    pub(super) world_id: String,
    pub(super) equipment_id: Uuid,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(super) struct RecoveryRow {
    pub(super) id: Uuid,
    pub(super) auction_id: Uuid,
    pub(super) lot_id: Uuid,
    pub(super) world_id: String,
    pub(super) equipment_id: Uuid,
    pub(super) payment_obligation_id: Uuid,
    pub(super) equipment_obligation_id: Option<Uuid>,
    pub(super) seller_payout_obligation_id: Uuid,
    pub(super) buyer_refund_obligation_id: Uuid,
    pub(super) stage: String,
    pub(super) resolution: Option<String>,
    pub(super) state_version: i64,
    pub(super) blocking_code: Option<String>,
    pub(super) manual_reconcile_delivery_id: Option<Uuid>,
    pub(super) relisted_auction_id: Option<Uuid>,
    pub(super) relisted_lot_id: Option<Uuid>,
    pub(super) custody_proof_kind: Option<String>,
    pub(super) custody_proof_reference: Option<String>,
    pub(super) custody_proof_digest: Option<Vec<u8>>,
    pub(super) custody_proof_observed_at: Option<DateTime<Utc>>,
    pub(super) created_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(super) struct OriginalLot {
    pub(super) equipment_id: Uuid,
    pub(super) submission_id: Option<Uuid>,
    pub(super) status: String,
    pub(super) start_price: i64,
    pub(super) equipment_world_id: String,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(super) struct ObligationRow {
    pub(super) id: Uuid,
    pub(super) auction_id: Uuid,
    pub(super) lot_id: Uuid,
    pub(super) world_id: String,
    pub(super) kind: String,
    pub(super) state: String,
    pub(super) equipment_id: Uuid,
    pub(super) amount: i64,
    pub(super) recipient_user_id: Option<Uuid>,
    pub(super) recipient_game_identity: Option<String>,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(super) struct ActionRow {
    pub(super) id: Uuid,
    pub(super) kind: String,
    pub(super) state: String,
    pub(super) delivery_id: Option<Uuid>,
    pub(super) proof_kind: Option<String>,
    pub(super) proof_reference: Option<String>,
    pub(super) proof_digest: Option<Vec<u8>>,
    pub(super) proof_observed_at: Option<DateTime<Utc>>,
}

#[cfg(feature = "postgres")]
pub(super) struct RelistResult {
    pub(super) stage: &'static str,
    pub(super) resolution: &'static str,
    pub(super) auction_id: Option<Uuid>,
    pub(super) lot_id: Option<Uuid>,
}

#[cfg(feature = "postgres")]
pub(super) async fn load_replayed_command(
    transaction: &mut Transaction<'_, Postgres>,
    command: &RecoveryConfirmationCommand,
) -> Result<Option<RecoveryConfirmationResult>, RecoveryCommandError> {
    let Some(row) = sqlx::query(
        r#"SELECT recovery_id, expected_state_version, action, request_digest,
                  result_code, result_payload
           FROM public.settlement_recovery_commands
           WHERE command_id = $1"#,
    )
    .bind(command.command_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    else {
        return Ok(None);
    };

    let stored_recovery_id: Uuid = row.try_get("recovery_id").map_err(map_sql_error)?;
    let stored_version: i64 = row.try_get("expected_state_version").map_err(map_sql_error)?;
    let stored_action: String = row.try_get("action").map_err(map_sql_error)?;
    let stored_digest: Vec<u8> = row.try_get("request_digest").map_err(map_sql_error)?;
    if stored_recovery_id != command.recovery_id.as_uuid()
        || stored_version != command.expected_state_version
        || stored_action != command.action.as_str()
        || stored_digest != command.request_hash.as_bytes()
    {
        return Err(RecoveryCommandError::CommandConflict);
    }

    let result_code: String = row.try_get("result_code").map_err(map_sql_error)?;
    if result_code != "ok" {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let payload: Value = row.try_get("result_payload").map_err(map_sql_error)?;
    let result: RecoveryConfirmationResult =
        serde_json::from_value(payload).map_err(|_| RecoveryCommandError::InvalidStoredResult)?;
    result.validate()?;
    if result.recovery_id != command.recovery_id || result.action != command.action {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    if result.state_version <= command.expected_state_version {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    Ok(Some(result))
}

#[cfg(feature = "postgres")]
pub(super) async fn load_recovery_scope(
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

#[cfg(feature = "postgres")]
pub(super) async fn lock_world(
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

#[cfg(feature = "postgres")]
pub(super) async fn lock_auction(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
) -> Result<AuctionLock, RecoveryCommandError> {
    let row = sqlx::query(
        "SELECT world_id, status, state_version FROM public.auctions WHERE id = $1 FOR UPDATE",
    )
    .bind(auction_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::NotFound)?;
    Ok(AuctionLock {
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        status: row.try_get("status").map_err(map_sql_error)?,
        state_version: row.try_get("state_version").map_err(map_sql_error)?,
    })
}

#[cfg(feature = "postgres")]
pub(super) struct AuctionLock {
    pub(super) world_id: String,
    pub(super) status: String,
    #[allow(dead_code)]
    pub(super) state_version: i64,
}

#[cfg(feature = "postgres")]
pub(super) async fn lock_recovery(
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

#[cfg(feature = "postgres")]
pub(super) fn validate_recovery_row(
    recovery: &RecoveryRow,
    scope: &RecoveryScope,
    command: &RecoveryConfirmationCommand,
) -> Result<(), RecoveryCommandError> {
    if recovery.id != command.recovery_id.as_uuid()
        || recovery.auction_id != scope.auction_id
        || recovery.lot_id != scope.lot_id
        || recovery.world_id != scope.world_id
        || recovery.equipment_id != scope.equipment_id
    {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    if recovery.state_version != command.expected_state_version {
        return Err(RecoveryCommandError::StaleState);
    }
    if recovery.stage != "reclaiming"
        || recovery.resolution.is_some()
        || recovery.relisted_auction_id.is_some()
        || recovery.relisted_lot_id.is_some()
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn lock_original_lot(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    lot_id: Uuid,
    equipment_id: Uuid,
) -> Result<OriginalLot, RecoveryCommandError> {
    let row = sqlx::query(
        r#"SELECT lot.equipment_id, lot.submission_id, lot.status, lot.start_price,
                  equipment.world_id AS equipment_world_id
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
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn lock_obligations(
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

    let mut obligations = BTreeMap::new();
    for id in ids {
        let row = sqlx::query(
            r#"SELECT id, auction_id, lot_id, world_id, obligation_kind, business_state,
                      equipment_id, amount, recipient_user_id, recipient_game_identity
               FROM public.settlement_obligations
               WHERE id = $1
               FOR UPDATE"#,
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .ok_or(RecoveryCommandError::NotConfirmable)?;
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
        if obligation.id != id
            || obligation.auction_id != recovery.auction_id
            || obligation.lot_id != recovery.lot_id
            || obligation.world_id != recovery.world_id
            || obligation.equipment_id != recovery.equipment_id
        {
            return Err(RecoveryCommandError::InvalidStoredResult);
        }
        obligations.insert(id, obligation);
    }
    Ok(obligations)
}

#[cfg(feature = "postgres")]
pub(super) async fn lock_actions(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
) -> Result<Vec<ActionRow>, RecoveryCommandError> {
    let rows = sqlx::query(
        r#"SELECT id, kind, execution_state, delivery_id, proof_kind,
                  proof_reference, proof_digest, proof_observed_at
           FROM public.settlement_recovery_actions
           WHERE recovery_id = $1
           ORDER BY kind, id
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

#[cfg(feature = "postgres")]
pub(super) fn validate_actions<'a>(
    recovery: &RecoveryRow,
    actions: &'a [ActionRow],
    requested: RecoveryConfirmationAction,
) -> Result<&'a ActionRow, RecoveryCommandError> {
    let expected_kind = confirmation_action_kind(&recovery.world_id, requested)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    let mut candidate = None;
    let mut blacklist_satisfied = false;
    let mut candidate_action = None;
    for action in actions {
        if action.kind == "buyer_blacklist" && action.state == "satisfied" {
            blacklist_satisfied = true;
        }
        if action.state != "satisfied" {
            continue;
        }
        let Some(candidate_value) = action_confirmation(&recovery.world_id, &action.kind) else {
            continue;
        };
        if candidate.is_some_and(|value| value != candidate_value) {
            return Err(RecoveryCommandError::NotConfirmable);
        }
        candidate = Some(candidate_value);
        candidate_action = Some(action);
    }

    let selected = actions
        .iter()
        .find(|action| action.kind == expected_kind)
        .ok_or(RecoveryCommandError::EvidenceUnresolved)?;
    if candidate != Some(requested)
        || selected.state != "satisfied"
        || selected.delivery_id.is_none()
        || selected.proof_kind.is_none()
        || selected.proof_reference.is_none()
        || selected.proof_digest.is_none()
        || selected.proof_observed_at.is_none()
        || candidate_action.map(|action| action.id) != Some(selected.id)
    {
        return Err(RecoveryCommandError::EvidenceUnresolved);
    }
    if matches!(
        requested,
        RecoveryConfirmationAction::Reclaimed | RecoveryConfirmationAction::Cancelled
    ) && !blacklist_satisfied
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    Ok(selected)
}

#[cfg(feature = "postgres")]
pub(super) fn validate_custody_evidence(
    recovery: &RecoveryRow,
    action: &ActionRow,
) -> Result<(), RecoveryCommandError> {
    if recovery.blocking_code.is_some()
        || recovery.manual_reconcile_delivery_id.is_some()
        || recovery.custody_proof_kind.as_deref() != Some("delivery_recovery_take_succeeded")
        || recovery.custody_proof_reference.is_none()
        || recovery.custody_proof_digest.is_none()
        || recovery.custody_proof_observed_at.is_none()
        || recovery.custody_proof_kind != action.proof_kind
        || recovery.custody_proof_reference != action.proof_reference
        || recovery.custody_proof_digest != action.proof_digest
        || recovery.custody_proof_observed_at != action.proof_observed_at
    {
        return Err(RecoveryCommandError::EvidenceUnresolved);
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn validate_obligations(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    obligations: &BTreeMap<Uuid, ObligationRow>,
    action: RecoveryConfirmationAction,
) -> Result<(), RecoveryCommandError> {
    let payment = obligations
        .get(&recovery.payment_obligation_id)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    let seller = obligations
        .get(&recovery.seller_payout_obligation_id)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    let refund = obligations
        .get(&recovery.buyer_refund_obligation_id)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    if seller.kind != "seller_payout"
        || seller.state != "blocked"
        || seller.amount <= 0
        || seller.recipient_game_identity.as_deref().is_none_or(str::is_empty)
        || refund.kind != "buyer_refund"
        || refund.state != "contingent"
        || refund.amount <= 0
        || refund.recipient_user_id.is_none()
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }

    let payment_state = lock_fulfillment_state(transaction, payment.id).await?;
    match action {
        RecoveryConfirmationAction::Reclaimed => {
            if recovery.world_id != "main"
                || payment.kind != "winner_equipment_cod"
                || payment.amount <= 0
                || payment.recipient_user_id.is_none()
                || payment_state.as_deref() == Some("satisfied")
            {
                return Err(RecoveryCommandError::NotConfirmable);
            }
        }
        RecoveryConfirmationAction::Cancelled => {
            if recovery.world_id != "isekai"
                || payment.kind != "winner_payment_carrier_cod"
                || payment.amount <= 0
                || payment.recipient_user_id.is_none()
                || payment_state.as_deref() == Some("satisfied")
            {
                return Err(RecoveryCommandError::NotConfirmable);
            }
        }
        RecoveryConfirmationAction::EquipmentRecovered => {
            let equipment_id =
                recovery.equipment_obligation_id.ok_or(RecoveryCommandError::NotConfirmable)?;
            let equipment =
                obligations.get(&equipment_id).ok_or(RecoveryCommandError::NotConfirmable)?;
            let equipment_state = lock_fulfillment_state(transaction, equipment.id).await?;
            if recovery.world_id != "isekai"
                || payment.kind != "winner_payment_carrier_cod"
                || payment_state.as_deref() != Some("satisfied")
                || equipment.kind != "winner_equipment_handoff"
                || equipment.state != "blocked"
                || equipment_state.as_deref() == Some("satisfied")
            {
                return Err(RecoveryCommandError::NotConfirmable);
            }
        }
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn lock_fulfillment_state(
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

#[cfg(feature = "postgres")]
pub(super) async fn database_now(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<DateTime<Utc>, RecoveryCommandError> {
    sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)
}

#[cfg(feature = "postgres")]
pub(super) async fn activate_buyer_refund(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    now: DateTime<Utc>,
) -> Result<(), RecoveryCommandError> {
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligations
           SET business_state = 'ready', state_version = state_version + 1, updated_at = $1
           WHERE id = $2 AND auction_id = $3 AND lot_id = $4
             AND obligation_kind = 'buyer_refund' AND business_state = 'contingent'
           RETURNING id"#,
    )
    .bind(now)
    .bind(recovery.buyer_refund_obligation_id)
    .bind(recovery.auction_id)
    .bind(recovery.lot_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if updated.is_none() {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    Ok(())
}
