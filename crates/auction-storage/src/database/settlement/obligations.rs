//! 结算义务、recovery、审计与实时事件写入；事务边界由父模块统一装配。

use std::collections::BTreeMap;

use crate::database::PostgresCompetitionTransaction;
use crate::database::audit_health::AuditWriteResult;
use crate::database::settlement::{
    SETTLEMENT_BATCH_SIZE, execute_settlement_batch, i32_version, i64_version, invalid,
    map_competition_error, map_sql_error,
};
use auction_application::SettlementTransactionError;
use auction_domain::{
    AuctionId, CommandId, RecoveryPolicy, SettlementObligation, SettlementObligationKind,
    SettlementPlan, SettlementWorld, deterministic_uuid,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Row;

/// 已完成数据库整数换算的义务行，写入前一次性校验全部版本。
struct ObligationRow<'a> {
    obligation: &'a SettlementObligation,
    policy_version: i32,
    state_version: i64,
}

pub(super) async fn insert_obligations(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &SettlementPlan,
    occurred_at: DateTime<Utc>,
) -> Result<(), SettlementTransactionError> {
    let rows = plan
        .obligations
        .iter()
        .map(|obligation| {
            Ok(ObligationRow {
                obligation,
                policy_version: i32_version(obligation.policy_version, "义务策略版本")?,
                state_version: i64_version(obligation.state_version, "义务状态版本")?,
            })
        })
        .collect::<Result<Vec<_>, SettlementTransactionError>>()?;
    // 前置义务外键为 DEFERRABLE，批内顺序不影响校验；履约行在同批义务写入后插入，满足即时外键。
    for rows in rows.chunks(SETTLEMENT_BATCH_SIZE) {
        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            r#"INSERT INTO public.settlement_obligations
                   (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                    obligation_kind, business_state, state_version, recipient_user_id,
                    recipient_game_identity, amount, carrier_credit, equipment_id, submission_id,
                    prerequisite_obligation_id, created_at, updated_at) "#,
        );
        query.push_values(rows, |mut values, row| {
            let obligation = row.obligation;
            values
                .push_bind(obligation.id.as_uuid())
                .push_bind(obligation.auction_id.as_uuid())
                .push_bind(obligation.lot_id.as_uuid())
                .push_bind(obligation.world.as_str())
                .push_bind(obligation.origin_command_id.as_uuid())
                .push_bind(row.policy_version)
                .push_bind(obligation.kind.as_str())
                .push_bind(obligation.state.as_str())
                .push_bind(row.state_version)
                .push_bind(obligation.recipient_user_id.map(|id| id.as_uuid()))
                .push("NULLIF(")
                .push_bind_unseparated(obligation.recipient_game_identity.as_str())
                .push_unseparated(", '')")
                .push_bind(obligation.amount.value())
                .push_bind(i64::from(obligation.carrier_credit))
                .push_bind(obligation.equipment_id.as_uuid())
                .push_bind(obligation.submission_id.map(|id| id.as_uuid()))
                .push_bind(obligation.prerequisite_id.map(|id| id.as_uuid()))
                .push_bind(occurred_at)
                .push_bind(occurred_at);
        });
        execute_settlement_batch(transaction, query, rows.len()).await?;

        // 投稿装备已经由入库流程领取；流拍与撤拍均须逐件发送库存装备，不能 Return 原邮件。
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            r#"INSERT INTO public.settlement_obligation_fulfillments
                   (obligation_id, execution_state, dispatch_world, delivery_operation,
                    created_at, updated_at) "#,
        );
        query.push_values(rows, |mut values, row| {
            values
                .push_bind(row.obligation.id.as_uuid())
                .push("'pending'")
                .push_bind(dispatch_world(row.obligation).as_str())
                .push("'send'")
                .push_bind(occurred_at)
                .push_bind(occurred_at);
        });
        execute_settlement_batch(transaction, query, rows.len()).await?;
    }
    Ok(())
}

/// 主世界承担付款、卖方结款与买方退款；其余义务沿用冻结计划中的世界。
fn dispatch_world(obligation: &SettlementObligation) -> SettlementWorld {
    match obligation.kind {
        SettlementObligationKind::WinnerPaymentCarrierCod
        | SettlementObligationKind::SellerPayout
        | SettlementObligationKind::BuyerRefund => SettlementWorld::Main,
        _ => obligation.world,
    }
}

/// 已解析关联义务的 recovery case 行；任何缺失关联都在写入前失败。
struct RecoveryRow<'a> {
    id: auction_domain::SettlementId,
    payment: &'a SettlementObligation,
    winner_user_id: auction_domain::UserId,
    world: SettlementWorld,
    equipment_obligation_id: Option<auction_domain::SettlementId>,
    seller_payout_obligation_id: auction_domain::SettlementId,
    buyer_refund_obligation_id: auction_domain::SettlementId,
}

/// 为每个有获胜者的 lot 建立 recovery case。
/// recovery 与结算义务必须处于同一个 Serializable 事务；只写 payment obligation 而不写
/// recovery case，付款超时后 Worker 将无法形成可审计的提醒、退回和人工确认链。
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
    let cod_return_limit = i32::try_from(recovery_policy.cod_return_limit)
        .ok()
        .filter(|limit| (1..=10).contains(limit))
        .ok_or_else(|| invalid("恢复 COD 退回上限超出数据库范围"))?;
    let policy_version = i32_version(recovery_policy.version, "恢复策略版本")?;
    let rows = recovery_rows(auction_id, plan)?;
    for rows in rows.chunks(SETTLEMENT_BATCH_SIZE) {
        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            r#"INSERT INTO public.settlement_recoveries
                   (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
                    payment_obligation_id, equipment_obligation_id, seller_payout_obligation_id,
                    buyer_refund_obligation_id, policy_version, payment_deadline_at,
                    reminder_limit, cod_return_limit, stage, state_version, created_at, updated_at) "#,
        );
        query.push_values(rows, |mut values, row| {
            values
                .push_bind(row.id.as_uuid())
                .push_bind(auction_id.as_uuid())
                .push_bind(row.payment.lot_id.as_uuid())
                .push_bind(row.world.as_str())
                .push_bind(row.payment.equipment_id.as_uuid())
                .push_bind(row.winner_user_id.as_uuid())
                .push_bind(row.payment.id.as_uuid())
                .push_bind(row.equipment_obligation_id.map(|id| id.as_uuid()))
                .push_bind(row.seller_payout_obligation_id.as_uuid())
                .push_bind(row.buyer_refund_obligation_id.as_uuid())
                .push_bind(policy_version)
                .push_bind(payment_deadline_at)
                .push_bind(reminder_limit)
                .push_bind(cod_return_limit)
                .push("'awaiting_payment'")
                .push("1")
                .push_bind(occurred_at)
                .push_bind(occurred_at);
        });
        execute_settlement_batch(transaction, query, rows.len()).await?;
    }
    Ok(())
}

/// 按计划中的付款义务顺序解析 recovery 关联；每个付款只查同 lot 的冻结义务，避免反复扫描计划。
fn recovery_rows(
    auction_id: AuctionId,
    plan: &SettlementPlan,
) -> Result<Vec<RecoveryRow<'_>>, SettlementTransactionError> {
    let auction_uuid = auction_id.as_uuid();
    let mut obligations = BTreeMap::new();
    for obligation in &plan.obligations {
        obligations.entry((obligation.lot_id, obligation.kind.as_str())).or_insert(obligation);
    }

    let mut rows = Vec::new();
    for payment in plan.obligations.iter().filter(|obligation| {
        matches!(
            obligation.kind,
            SettlementObligationKind::WinnerEquipmentCod
                | SettlementObligationKind::WinnerPaymentCarrierCod
        )
    }) {
        let winner_user_id =
            payment.recipient_user_id.ok_or_else(|| invalid("获胜付款义务缺少买方身份"))?;
        let seller_payout =
            find_obligation(&obligations, payment.lot_id, SettlementObligationKind::SellerPayout)
                .ok_or_else(|| invalid("结算恢复缺少卖方付款义务"))?;
        let buyer_refund =
            find_obligation(&obligations, payment.lot_id, SettlementObligationKind::BuyerRefund)
                .ok_or_else(|| invalid("结算恢复缺少买方退款义务"))?;
        let (world, equipment_obligation_id) = match payment.kind {
            SettlementObligationKind::WinnerEquipmentCod => (SettlementWorld::Main, None),
            SettlementObligationKind::WinnerPaymentCarrierCod => (
                SettlementWorld::Isekai,
                Some(
                    find_obligation(
                        &obligations,
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
        rows.push(RecoveryRow {
            id: auction_domain::SettlementId::from_uuid(auction_domain::legacy_natural_key_uuid(
                "auction.recovery.case.v1",
                &[auction_uuid.as_bytes(), lot_uuid.as_bytes()],
            )),
            payment,
            winner_user_id,
            world,
            equipment_obligation_id,
            seller_payout_obligation_id: seller_payout.id,
            buyer_refund_obligation_id: buyer_refund.id,
        });
    }
    Ok(rows)
}

fn find_obligation<'a>(
    obligations: &BTreeMap<(auction_domain::LotId, &'static str), &'a SettlementObligation>,
    lot_id: auction_domain::LotId,
    kind: SettlementObligationKind,
) -> Option<&'a SettlementObligation> {
    obligations.get(&(lot_id, kind.as_str())).copied()
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
        let count = obligation_kind_counts.entry(obligation.kind.as_str()).or_insert(0_usize);
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
        "policy_mode": plan.policy_mode.as_str(),
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
    let stream_sequence = crate::database::realtime::allocate_stream_sequence(
        transaction,
        &stream_key,
        Some(occurred_at),
    )
    .await
    .map_err(map_sql_error)?;
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
