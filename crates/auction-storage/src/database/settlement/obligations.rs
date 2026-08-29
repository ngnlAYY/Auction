//! 结算义务、recovery、审计与实时事件写入。
//!
//! 该文件只承载 PostgreSQL 结算事务的单一职责，事务边界仍由父模块统一装配。

use std::collections::BTreeMap;

use super::*;

pub(super) async fn insert_obligations(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &SettlementPlan,
    occurred_at: DateTime<Utc>,
) -> Result<(), SettlementTransactionError> {
    for obligation in &plan.obligations {
        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        sqlx::query(
            r#"INSERT INTO public.settlement_obligations
                   (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                    obligation_kind, business_state, state_version, recipient_user_id,
                    recipient_game_identity, amount, carrier_credit, equipment_id, submission_id,
                    prerequisite_obligation_id, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULLIF($11, ''),
                       $12, $13, $14, $15, $16, $17, $17)"#,
        )
        .bind(obligation.id.as_uuid())
        .bind(obligation.auction_id.as_uuid())
        .bind(obligation.lot_id.as_uuid())
        .bind(settlement_world_name(obligation.world))
        .bind(obligation.origin_command_id.as_uuid())
        .bind(i32_version(obligation.policy_version, "义务策略版本")?)
        .bind(obligation_kind(obligation.kind))
        .bind(obligation_state(obligation.state))
        .bind(i64_version(obligation.state_version, "义务状态版本")?)
        .bind(obligation.recipient_user_id.map(|id| id.as_uuid()))
        .bind(&obligation.recipient_game_identity)
        .bind(obligation.amount.value())
        .bind(i64::from(obligation.carrier_credit))
        .bind(obligation.equipment_id.as_uuid())
        .bind(obligation.submission_id.map(|id| id.as_uuid()))
        .bind(obligation.prerequisite_id.map(|id| id.as_uuid()))
        .bind(occurred_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;

        let operation = match obligation.kind {
            SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn => {
                "return"
            }
            _ => "send",
        };
        let dispatch_world = match obligation.kind {
            SettlementObligationKind::WinnerPaymentCarrierCod
            | SettlementObligationKind::SellerPayout
            | SettlementObligationKind::BuyerRefund => SettlementWorld::Main,
            _ => obligation.world,
        };
        sqlx::query(
            r#"INSERT INTO public.settlement_obligation_fulfillments
                   (obligation_id, execution_state, dispatch_world, delivery_operation,
                    created_at, updated_at)
               VALUES ($1, 'pending', $2, $3, $4, $4)"#,
        )
        .bind(obligation.id.as_uuid())
        .bind(settlement_world_name(dispatch_world))
        .bind(operation)
        .bind(occurred_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

/// 为每个有获胜者的 lot 建立 recovery case。
///
/// recovery 与结算义务必须处于同一个 Serializable 事务；如果只写了 payment obligation
/// 而没有写 recovery case，付款超时后 Worker 将无法形成可审计的提醒、退回和人工确认链。
pub(super) async fn insert_recoveries(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
    plan: &SettlementPlan,
    recovery_policy: RecoveryPolicy,
    occurred_at: DateTime<Utc>,
) -> Result<(), SettlementTransactionError> {
    let payment_deadline_at = occurred_at
        .checked_add_signed(recovery_policy.payment_timeout)
        .ok_or_else(|| invalid("恢复付款截止时间溢出"))?;
    let reminder_limit = i32::try_from(recovery_policy.reminder_limit)
        .map_err(|_| invalid("恢复提醒上限超出数据库范围"))?;
    let auction_uuid = auction_id.as_uuid();

    for payment in plan.obligations.iter().filter(|obligation| {
        matches!(
            obligation.kind,
            SettlementObligationKind::WinnerEquipmentCod
                | SettlementObligationKind::WinnerPaymentCarrierCod
        )
    }) {
        let winner_user_id =
            payment.recipient_user_id.ok_or_else(|| invalid("获胜付款义务缺少买方身份"))?;
        let seller_payout = find_obligation(
            &plan.obligations,
            payment.lot_id,
            SettlementObligationKind::SellerPayout,
        )
        .ok_or_else(|| invalid("结算恢复缺少卖方付款义务"))?;
        let buyer_refund = find_obligation(
            &plan.obligations,
            payment.lot_id,
            SettlementObligationKind::BuyerRefund,
        )
        .ok_or_else(|| invalid("结算恢复缺少买方退款义务"))?;
        let (recovery_world, equipment_obligation_id) = match payment.kind {
            SettlementObligationKind::WinnerEquipmentCod => (SettlementWorld::Main, None),
            SettlementObligationKind::WinnerPaymentCarrierCod => (
                SettlementWorld::Isekai,
                Some(
                    find_obligation(
                        &plan.obligations,
                        payment.lot_id,
                        SettlementObligationKind::WinnerEquipmentHandoff,
                    )
                    .ok_or_else(|| invalid("异世界结算恢复缺少装备交接义务"))?
                    .id,
                ),
            ),
            _ => return Err(invalid("结算恢复付款义务种类无效")),
        };
        let lot_uuid = payment.lot_id.as_uuid();
        let recovery_id =
            auction_domain::SettlementId::from_uuid(auction_domain::legacy_natural_key_uuid(
                "auction.recovery.case.v1",
                &[auction_uuid.as_bytes(), lot_uuid.as_bytes()],
            ));

        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        sqlx::query(
            r#"INSERT INTO public.settlement_recoveries
                   (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
                    payment_obligation_id, equipment_obligation_id, seller_payout_obligation_id,
                    buyer_refund_obligation_id, policy_version, payment_deadline_at,
                    reminder_limit, stage, state_version, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                       $13, 'awaiting_payment', 1, $14, $14)"#,
        )
        .bind(recovery_id.as_uuid())
        .bind(auction_id.as_uuid())
        .bind(payment.lot_id.as_uuid())
        .bind(settlement_world_name(recovery_world))
        .bind(payment.equipment_id.as_uuid())
        .bind(winner_user_id.as_uuid())
        .bind(payment.id.as_uuid())
        .bind(equipment_obligation_id.map(|id| id.as_uuid()))
        .bind(seller_payout.id.as_uuid())
        .bind(buyer_refund.id.as_uuid())
        .bind(i32_version(recovery_policy.version, "恢复策略版本")?)
        .bind(payment_deadline_at)
        .bind(reminder_limit)
        .bind(occurred_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

pub(super) fn find_obligation(
    obligations: &[SettlementObligation],
    lot_id: auction_domain::LotId,
    kind: SettlementObligationKind,
) -> Option<&SettlementObligation> {
    obligations.iter().find(|obligation| obligation.lot_id == lot_id && obligation.kind == kind)
}

pub(super) async fn insert_settlement_audit(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
    command_id: CommandId,
    opening_pool_version: u64,
    closing_pool_version: u64,
    plan: &SettlementPlan,
    occurred_at: DateTime<Utc>,
) -> Result<(), SettlementTransactionError> {
    let sold_count = plan.lines.iter().filter(|line| line.end_price.value() > 0).count();
    let unsold_count =
        plan.lines.len().checked_sub(sold_count).ok_or_else(|| invalid("结算审计 lot 数量无效"))?;
    let mut obligation_kind_counts = BTreeMap::new();
    for obligation in &plan.obligations {
        let count =
            obligation_kind_counts.entry(obligation_kind(obligation.kind)).or_insert(0_usize);
        *count = count.checked_add(1).ok_or_else(|| invalid("结算审计义务数量溢出"))?;
    }
    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
                request_id, source, metadata)
           VALUES ($1, $2, 'system', 'auction-worker', 'auction.settle', 'auction', $3,
                   'success', NULL, 'worker', $4)"#,
    )
    .bind(deterministic_uuid(command_id, "settlement-audit", 1))
    .bind(occurred_at)
    .bind(auction_id.to_string())
    .bind(json!({
        "auction_id": auction_id.to_string(),
        "policy_version": plan.policy_version,
        "policy_mode": settlement_mode(plan.policy_mode),
        "pool_enabled": plan.compensation_pool_enabled,
        "opening_pool_balance": plan.opening_pool.value(),
        "closing_pool_balance": plan.closing_pool.value(),
        "opening_pool_version": opening_pool_version,
        "closing_pool_version": closing_pool_version,
        "tax_total": plan.tax_total.value(),
        "discount_demand": plan.discount_demand.value(),
        "compensation_total": plan.compensation_total.value(),
        "sold_count": sold_count,
        "unsold_count": unsold_count,
        "obligation_count": plan.obligations.len(),
        "obligation_kind_counts": obligation_kind_counts,
    }))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

pub(super) async fn insert_settlement_realtime(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
    command_id: CommandId,
    state_version: u64,
    plan: &SettlementPlan,
    occurred_at: DateTime<Utc>,
) -> Result<(), SettlementTransactionError> {
    let stream_key = format!("auction:{auction_id}");
    let event_id = deterministic_uuid(command_id, "settlement-realtime-event", 1);
    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_streams (stream_key, updated_at)
           VALUES ($1, $2)
           ON CONFLICT (stream_key) DO NOTHING"#,
    )
    .bind(&stream_key)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let row = sqlx::query(
        r#"UPDATE public.realtime_streams
           SET next_sequence = next_sequence + 1, updated_at = $2
           WHERE stream_key = $1
           RETURNING next_sequence - 1 AS stream_sequence"#,
    )
    .bind(&stream_key)
    .bind(occurred_at)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let stream_sequence: i64 = row.try_get("stream_sequence").map_err(map_sql_error)?;
    let counts = sqlx::query(
        r#"SELECT
               COUNT(*) FILTER (WHERE status = 'sold')::bigint AS sold_count,
               COUNT(*) FILTER (WHERE status = 'unsold')::bigint AS unsold_count
           FROM public.lots
           WHERE auction_id = $1"#,
    )
    .bind(auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sold_count: i64 = counts.try_get("sold_count").map_err(map_sql_error)?;
    let unsold_count: i64 = counts.try_get("unsold_count").map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
               (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
                state_version, origin_command_id, created_at)
           VALUES ($1, $2, $3, 'auction.settled', 2, $4, $5, $6, $7)"#,
    )
    .bind(event_id)
    .bind(&stream_key)
    .bind(stream_sequence)
    .bind(json!({
        "auction_id": auction_id,
        "policy_version": plan.policy_version,
        "sold_count": sold_count,
        "unsold_count": unsold_count,
        "obligation_count": plan.obligations.len(),
        "state_version": state_version,
    }))
    .bind(i64_version(state_version, "拍卖状态版本")?)
    .bind(command_id.as_uuid())
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
