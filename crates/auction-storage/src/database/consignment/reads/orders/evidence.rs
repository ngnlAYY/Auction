//! Admin 订单证据：各轮、尝试、退件接管、远端观察、现金用途与阻断，只含 ID、阶段码、金额、
//! 摘要与时刻（不变量 7：不含预约码、装备 key、原信正文与凭据）。
//!
//! 各类证据各取上限内的行，合并排序后再分页；订单不存在返回空。观察只列已入账的验证结果，
//! 观察本身不自动成为有效结论的约束由写端守卫，这里不另造“待定”状态。

use auction_application::consignment::{
    ConsignmentFuture, ListingPageWindow, ListingReadPage, OrderEvidence, OrderResolveAction,
};
use auction_time::Timestamp;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::LegFacts;
use crate::database::consignment::reads::{snapshot, storage_error};

/// 每类证据最多读取的行数；订单的轮数上限是 100，远小于它。
const CATEGORY_LIMIT: i64 = 500;

/// 与 `load_legs` 同列名的单轮事实片段：按轮的 delivery 与（义务, 轮号）取。
const ROUND_FACTS: &str = "o.obligation_kind, o.business_state, f.execution_state,
    d.status AS delivery_status, true AS has_round,
    EXISTS (SELECT 1 FROM public.delivery_send_attempts a
            WHERE a.delivery_id = r.delivery_id) AS has_attempt,
    EXISTS (SELECT 1 FROM public.delivery_sent_bindings b
            WHERE b.delivery_id = r.delivery_id) AS has_sent,
    EXISTS (SELECT 1 FROM public.consignment_handoff_proofs h
            WHERE h.delivery_id = r.delivery_id) AS has_claim_proof,
    EXISTS (SELECT 1 FROM public.consignment_dispatch_attempts attempt
            JOIN public.consignment_dispatch_evidence e ON e.attempt_id = attempt.attempt_id
            WHERE attempt.delivery_id = r.delivery_id AND e.category = 'cash_claim') AS has_cash_claim,
    EXISTS (SELECT 1 FROM public.consignment_return_consumptions rc
            WHERE rc.obligation_id = r.obligation_id AND rc.original_round = r.round) AS has_return";

const ROUND_JOINS: &str = "JOIN public.settlement_obligations o ON o.id = r.obligation_id
    JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
    LEFT JOIN public.deliveries d ON d.id = r.delivery_id";

fn leg_code(kind: &str) -> Result<&'static str, sqlx::Error> {
    Ok(match kind {
        "winner_equipment_cod" | "winner_equipment_handoff" | "withdrawn_return" => "original",
        "winner_payment_carrier_cod" => "carrier",
        "seller_payout" => "payout",
        "buyer_refund" => "refund",
        _ => return Err(super::protocol("寄售订单含未知义务种类")),
    })
}

async fn rounds(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<(Timestamp, OrderEvidence)>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT r.round_id, r.round, r.created_at, {ROUND_FACTS}
         FROM public.consignment_dispatch_rounds r {ROUND_JOINS}
         WHERE r.contract_id = $1 ORDER BY r.created_at, r.round_id LIMIT {CATEGORY_LIMIT}"
    )))
    .bind(contract_id)
    .fetch_all(conn)
    .await?;
    rows.iter()
        .map(|row| {
            let created_at: Timestamp = row.try_get("created_at")?;
            let kind: String = row.try_get("obligation_kind")?;
            Ok((
                created_at,
                OrderEvidence::Round {
                    round_id: row.try_get("round_id")?,
                    round_number: u32::try_from(row.try_get::<i32, _>("round")?)
                        .map_err(super::protocol_from)?,
                    leg: leg_code(&kind)?,
                    delivery_stage: LegFacts::from_row(row)?.delivery_stage().as_str(),
                    created_at,
                },
            ))
        })
        .collect()
}

async fn attempts(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<(Timestamp, OrderEvidence)>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT attempt.attempt_id, attempt.fence, attempt.started_at, r.round_id, {ROUND_FACTS}
         FROM public.consignment_dispatch_attempts attempt
         JOIN public.consignment_dispatch_rounds r
           ON r.contract_id = attempt.contract_id AND r.obligation_id = attempt.obligation_id
          AND r.round = attempt.round AND r.delivery_id = attempt.delivery_id
         {ROUND_JOINS}
         WHERE attempt.contract_id = $1
         ORDER BY attempt.started_at, attempt.attempt_id LIMIT {CATEGORY_LIMIT}"
    )))
    .bind(contract_id)
    .fetch_all(conn)
    .await?;
    rows.iter()
        .map(|row| {
            let stage = LegFacts::from_row(row)?.delivery_stage();
            // 起写事实先于 POST：只有越过 prepared 的阶段才算已起写，与 DTO 校验同口径。
            let write_started = !matches!(
                stage,
                auction_domain::consignment::OrderDeliveryStage::NotStarted
                    | auction_domain::consignment::OrderDeliveryStage::Prepared
            );
            Ok((
                row.try_get("started_at")?,
                OrderEvidence::Attempt {
                    attempt_id: row.try_get("attempt_id")?,
                    round_id: row.try_get("round_id")?,
                    fence: row.try_get::<i64, _>("fence")?.cast_unsigned(),
                    write_started,
                    delivery_stage: stage.as_str(),
                },
            ))
        })
        .collect()
}

async fn return_takes(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<(Timestamp, OrderEvidence)>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT i.id AS intake_id, i.take_work_id, i.created_at, r.round_id, start.fence AS start_fence,
               EXISTS (SELECT 1 FROM public.mail_disposition_proofs p
                       WHERE p.work_id = i.take_work_id AND p.kind = 'exact_take'
                         AND p.conclusion = 'take_proven') AS taken,
               EXISTS (SELECT 1 FROM public.mail_disposition_proofs p
                       WHERE p.work_id = i.take_work_id AND p.kind = 'exact_take'
                         AND p.conclusion = 'evidence_quarantined') AS quarantined,
                EXISTS (SELECT 1 FROM public.consignment_return_consumptions c
                        WHERE c.intake_id = i.id) AS consumed
         FROM public.consignment_return_intakes i
         JOIN public.consignment_dispatch_rounds r
           ON r.contract_id = i.contract_id AND r.obligation_id = i.obligation_id
          AND r.round = i.original_round
         LEFT JOIN public.mail_disposition_write_starts start ON start.work_id = i.take_work_id
         WHERE i.contract_id = $1 ORDER BY i.created_at, i.id LIMIT {CATEGORY_LIMIT}"
    )))
    .bind(contract_id)
    .fetch_all(conn)
    .await?;
    rows.iter().map(return_take).collect()
}

fn return_take(row: &PgRow) -> Result<(Timestamp, OrderEvidence), sqlx::Error> {
    let fence: Option<i64> = row.try_get("start_fence")?;
    let state = match (
        row.try_get::<bool, _>("consumed")?,
        row.try_get::<bool, _>("taken")?,
        row.try_get::<bool, _>("quarantined")?,
        fence.is_some(),
    ) {
        (true, _, _, _) => "consumed",
        (false, true, _, _) => "taken",
        // 只有 quarantine 证据时是“证据不足，持续核对”，不能报成已证 Take。
        (false, false, true, _) => "reconcile",
        (false, false, false, true) => "write_started",
        (false, false, false, false) => "pending",
    };
    Ok((
        row.try_get("created_at")?,
        OrderEvidence::ReturnTake {
            intake_id: row.try_get("intake_id")?,
            work_id: row.try_get("take_work_id")?,
            original_round_id: row.try_get("round_id")?,
            fence: fence.map(i64::cast_unsigned),
            state,
        },
    ))
}

async fn observations(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<(Timestamp, OrderEvidence)>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT e.id, e.category, encode(e.digest, 'hex') AS digest, e.observed_at
         FROM public.consignment_dispatch_evidence e
         JOIN public.consignment_dispatch_attempts attempt ON attempt.attempt_id = e.attempt_id
         WHERE attempt.contract_id = $1 ORDER BY e.observed_at, e.id LIMIT {CATEGORY_LIMIT}"
    )))
    .bind(contract_id)
    .fetch_all(conn)
    .await?;
    rows.iter()
        .map(|row| {
            let category = match row.try_get::<&str, _>("category")? {
                "sent_binding" => "sent",
                "original_claim" | "cash_claim" => "claim",
                "carrier_payment" => "payment",
                "rts" => "rts",
                _ => return Err(super::protocol("寄售证据类别未知")),
            };
            let observed_at: Timestamp = row.try_get("observed_at")?;
            Ok((
                observed_at,
                OrderEvidence::Observation {
                    observation_id: row.try_get("id")?,
                    category,
                    // 入账即已通过验证器；没有“待定 / 冲突”的持久列，不另造。
                    conclusion: "accepted",
                    digest: row.try_get("digest")?,
                    observed_at,
                },
            ))
        })
        .collect()
}

async fn cash_uses(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<(Timestamp, OrderEvidence)>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT a.purpose, a.state, a.amount, a.created_at
         FROM public.settlement_funding_allocations a
         WHERE a.purpose IN ('seller', 'fee', 'carrier', 'refund', 'retained')
           AND (a.consignment_contract_id = $1
                OR a.obligation_id IN (SELECT id FROM public.settlement_obligations
                                       WHERE contract_id = $1))
         ORDER BY a.created_at, a.id LIMIT {CATEGORY_LIMIT}"
    )))
    .bind(contract_id)
    .fetch_all(conn)
    .await?;
    rows.iter()
        .map(|row| {
            let purpose = match row.try_get::<&str, _>("purpose")? {
                "seller" => "seller",
                "fee" => "fee",
                "carrier" => "carrier",
                "refund" => "refund",
                _ => "retained",
            };
            let state = match row.try_get::<&str, _>("state")? {
                "available" => "ready",
                "reserved" | "in_flight" => "reserved",
                "spent" => "sent",
                _ => "unknown",
            };
            Ok((
                row.try_get("created_at")?,
                OrderEvidence::CashUse { purpose, state, amount: row.try_get("amount")? },
            ))
        })
        .collect()
}

async fn blockers(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<(Timestamp, OrderEvidence)>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT b.opened_at, b.state
         FROM public.consignment_cash_return_blocks b
         JOIN public.consignment_return_intakes i ON i.id = b.intake_id
         WHERE i.contract_id = $1 ORDER BY b.opened_at, b.id LIMIT {CATEGORY_LIMIT}"
    )))
    .bind(contract_id)
    .fetch_all(conn)
    .await?;
    rows.iter()
        .map(|row| {
            let since: Timestamp = row.try_get("opened_at")?;
            Ok((
                since,
                OrderEvidence::Blocker {
                    blocker: "opening_uid_block",
                    since,
                    resolved: row.try_get::<&str, _>("state")? == "cleared",
                },
            ))
        })
        .collect()
}

/// 恢复段：Sale 封闭事实、Admin 处置记录与复核工作（第 7 期）。
async fn recovery(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<(Timestamp, OrderEvidence)>, sqlx::Error> {
    let mut items = Vec::new();
    let closures = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT closure_kind, closed_at FROM public.consignment_sale_closures
         WHERE contract_id = $1 LIMIT {CATEGORY_LIMIT}"
    )))
    .bind(contract_id)
    .fetch_all(&mut *conn)
    .await?;
    for row in &closures {
        let closed_at: Timestamp = row.try_get("closed_at")?;
        let closure_kind = match row.try_get::<&str, _>("closure_kind")? {
            "cancelled_unpaid" => "cancelled_unpaid",
            "refund_closed" => "refund_closed",
            _ => return Err(super::protocol("寄售封闭事实种类未知")),
        };
        items.push((closed_at, OrderEvidence::Closure { closure_kind, closed_at }));
    }
    let resolutions = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT resolution_id, action, evidence_id, resolved_at
         FROM public.consignment_order_resolutions
         WHERE contract_id = $1 ORDER BY resolved_at, resolution_id LIMIT {CATEGORY_LIMIT}"
    )))
    .bind(contract_id)
    .fetch_all(&mut *conn)
    .await?;
    for row in &resolutions {
        let resolved_at: Timestamp = row.try_get("resolved_at")?;
        let action = OrderResolveAction::parse(row.try_get::<&str, _>("action")?)
            .ok_or_else(|| super::protocol("寄售处置动作未知"))?;
        items.push((
            resolved_at,
            OrderEvidence::Resolution {
                resolution_id: row.try_get("resolution_id")?,
                action: action.as_str(),
                evidence_id: row.try_get("evidence_id")?,
                resolved_at,
            },
        ));
    }
    let rechecks = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT work_id, requested_at FROM public.consignment_order_rechecks
         WHERE contract_id = $1 ORDER BY requested_at, work_id LIMIT {CATEGORY_LIMIT}"
    )))
    .bind(contract_id)
    .fetch_all(&mut *conn)
    .await?;
    for row in &rechecks {
        let requested_at: Timestamp = row.try_get("requested_at")?;
        items.push((
            requested_at,
            OrderEvidence::Recheck { work_id: row.try_get("work_id")?, requested_at },
        ));
    }
    Ok(items)
}

/// 读取并分页；类别顺序固定为轮、尝试、接管、观察、现金、阻断、恢复，类别内按时间。
pub(super) async fn read_evidence(
    conn: &mut PgConnection,
    contract_id: Uuid,
    window: ListingPageWindow,
) -> Result<Option<ListingReadPage<OrderEvidence>>, sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_contracts WHERE contract_id = $1)",
    )
    .bind(contract_id)
    .fetch_one(&mut *conn)
    .await?;
    if !exists {
        return Ok(None);
    }
    let mut all = Vec::new();
    all.extend(rounds(&mut *conn, contract_id).await?);
    all.extend(attempts(&mut *conn, contract_id).await?);
    all.extend(return_takes(&mut *conn, contract_id).await?);
    all.extend(observations(&mut *conn, contract_id).await?);
    all.extend(cash_uses(&mut *conn, contract_id).await?);
    all.extend(blockers(&mut *conn, contract_id).await?);
    all.extend(recovery(&mut *conn, contract_id).await?);
    let total = all.len() as u64;
    let start = usize::try_from(window.offset()).map_err(super::protocol_from)?;
    let size = usize::try_from(window.page_size()).map_err(super::protocol_from)?;
    let items = all.into_iter().map(|(_, item)| item).skip(start).take(size).collect();
    Ok(Some(ListingReadPage { items, total, window }))
}

/// 供 `AdminOrderRead::order_evidence` 调用的快照读。
pub(super) fn evidence_future(
    store: &crate::database::DatabaseStore,
    contract_id: Uuid,
    window: ListingPageWindow,
) -> ConsignmentFuture<'_, Option<ListingReadPage<OrderEvidence>>> {
    Box::pin(async move {
        let (mut tx, _) = snapshot(store, false).await?;
        let page = read_evidence(&mut tx, contract_id, window).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(page)
    })
}
