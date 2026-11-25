//! 订单读侧：把合同、义务、轮、证明与资金事实收成 `OrderFacts`，交领域投影出订单态与阶段。
//!
//! 保护不变量 6：证据缺失、矛盾或未知一律落到 `unknown`，由领域投影吸收为 `reconcile`，这里不替
//! 任何事实“补出”一个看起来合理的阶段。订单读取不加锁、不写任何行；订单阶段事件的追加在
//! `events` 子模块，与资金事实同事务调用同一份投影，所以事件里的订单态与读到的订单态同源。

mod admin;
pub(crate) mod catalog_v2;
mod events;
mod evidence;
mod private;
#[cfg(feature = "test-db")]
mod test_access;

use std::collections::HashMap;

use auction_application::consignment::{ListingWorld, OrderCore};
use auction_domain::consignment::{
    ListingPrice, OrderDeliveryStage, OrderFacts, OrderKind, OrderLegStage, OrderOutcome,
    OrderPaymentStage, project_order_stages,
};
use auction_time::Timestamp;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

pub(crate) use events::record_order_stage_change_in_tx;

/// 订单阶段事件所在的流前缀；Public relay 只扫描 `auction:` 前缀，这个流不会被推送。
pub(crate) const ORDER_STREAM_PREFIX: &str = "consignment_order:";

/// 一条已加载的订单：角色无关事实与双方身份。
pub(super) struct LoadedOrder {
    pub(super) core: OrderCore,
    pub(super) buyer_user_id: Option<Uuid>,
    pub(super) buyer_name: Option<String>,
    pub(super) seller_user_id: Uuid,
    pub(super) seller_name: String,
}

/// 一条义务及其某一轮的事实。
pub(super) struct LegFacts {
    business_state: String,
    execution_state: String,
    delivery_status: Option<String>,
    has_round: bool,
    has_attempt: bool,
    has_sent: bool,
    has_claim_proof: bool,
    has_cash_claim: bool,
    has_return: bool,
}

impl LegFacts {
    /// 从带固定列名的查询行读取；`load_legs` 与证据查询共用同一组列名。
    pub(super) fn from_row(row: &PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            business_state: row.try_get("business_state")?,
            execution_state: row.try_get("execution_state")?,
            delivery_status: row.try_get("delivery_status")?,
            has_round: row.try_get("has_round")?,
            has_attempt: row.try_get("has_attempt")?,
            has_sent: row.try_get("has_sent")?,
            has_claim_proof: row.try_get("has_claim_proof")?,
            has_cash_claim: row.try_get("has_cash_claim")?,
            has_return: row.try_get("has_return")?,
        })
    }

    /// 履约或投递已进入失败 / 待核状态：阶段未知，不推断。
    fn is_unresolved(&self) -> bool {
        matches!(self.execution_state.as_str(), "failed" | "manual_reconcile")
            || matches!(self.delivery_status.as_deref(), Some("failed" | "manual_reconcile"))
    }

    /// 原件 / 载体腿的交割阶段。
    pub(super) fn delivery_stage(&self) -> OrderDeliveryStage {
        use OrderDeliveryStage as D;
        if self.is_unresolved() || (self.has_claim_proof && self.has_return) {
            return D::Unknown;
        }
        if self.has_claim_proof {
            return D::Claimed;
        }
        if self.has_return {
            return D::Returned;
        }
        if !self.has_round {
            return D::NotStarted;
        }
        match (self.has_sent, self.has_attempt, self.delivery_status.as_deref()) {
            (true, _, Some("succeeded")) => D::Sent,
            // 已成功却没有 Sent 绑定，或有 Sent 绑定却不是成功态：事实互相矛盾。
            (true, _, _) | (false, _, Some("succeeded")) => D::Unknown,
            (false, true, Some("sending")) => D::InFlight,
            _ => D::Prepared,
        }
    }

    /// 卖款 / 退款腿的阶段。
    fn cash_stage(&self) -> OrderLegStage {
        use OrderLegStage as L;
        if self.is_unresolved() {
            return L::Unknown;
        }
        if self.has_cash_claim {
            return L::Claimed;
        }
        if self.has_return {
            return L::Returned;
        }
        if self.has_sent {
            return L::Sent;
        }
        if self.has_round {
            return L::Reserved;
        }
        match self.business_state.as_str() {
            "blocked" => L::Blocked,
            "ready" => L::Ready,
            _ => L::NotApplicable,
        }
    }
}

/// 同一合同的各条义务腿。
#[derive(Default)]
struct Legs {
    payment: Option<LegFacts>,
    handoff: Option<LegFacts>,
    payout: Option<LegFacts>,
    refund: Option<LegFacts>,
    withdrawn: Option<LegFacts>,
}

fn leg_row(row: &PgRow) -> Result<(Uuid, String, LegFacts), sqlx::Error> {
    Ok((row.try_get("contract_id")?, row.try_get("obligation_kind")?, LegFacts::from_row(row)?))
}

/// 合同的全部义务腿；最新轮用 LATERAL 取，退件后重派的新轮不会被旧轮的退回事实遮住。
async fn load_legs(
    conn: &mut PgConnection,
    contract_ids: &[Uuid],
) -> Result<HashMap<Uuid, Legs>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT o.contract_id, o.obligation_kind, o.business_state,
                f.execution_state, d.status AS delivery_status,
                lr.round IS NOT NULL AS has_round,
                EXISTS (SELECT 1 FROM public.delivery_send_attempts a
                        WHERE a.delivery_id = lr.delivery_id) AS has_attempt,
                EXISTS (SELECT 1 FROM public.delivery_sent_bindings b
                        WHERE b.delivery_id = lr.delivery_id) AS has_sent,
                EXISTS (SELECT 1 FROM public.consignment_handoff_proofs h
                        WHERE h.obligation_id = o.id) AS has_claim_proof,
                EXISTS (SELECT 1 FROM public.consignment_dispatch_attempts attempt
                        JOIN public.consignment_dispatch_evidence e ON e.attempt_id = attempt.attempt_id
                        WHERE attempt.delivery_id = lr.delivery_id
                          AND e.category = 'cash_claim') AS has_cash_claim,
                EXISTS (SELECT 1 FROM public.consignment_return_consumptions rc
                        WHERE rc.obligation_id = o.id AND rc.original_round = lr.round) AS has_return
         FROM public.settlement_obligations o
         JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
         LEFT JOIN LATERAL (
             SELECT r.round, r.delivery_id FROM public.consignment_dispatch_rounds r
             WHERE r.obligation_id = o.id ORDER BY r.round DESC LIMIT 1
         ) lr ON true
         LEFT JOIN public.deliveries d ON d.id = lr.delivery_id
         WHERE o.contract_id = ANY($1)",
    )
    .bind(contract_ids.to_vec())
    .fetch_all(conn)
    .await?;
    let mut by_contract: HashMap<Uuid, Legs> = HashMap::new();
    for row in &rows {
        let (contract_id, kind, leg) = leg_row(row)?;
        let legs = by_contract.entry(contract_id).or_default();
        let slot = match kind.as_str() {
            "winner_equipment_cod" | "winner_payment_carrier_cod" => &mut legs.payment,
            "winner_equipment_handoff" => &mut legs.handoff,
            "seller_payout" => &mut legs.payout,
            "buyer_refund" => &mut legs.refund,
            "withdrawn_return" => &mut legs.withdrawn,
            // 寄售合同不应有其他义务种类：失败关闭，不把陌生行当作“无事发生”。
            _ => return Err(protocol("寄售订单含未知义务种类")),
        };
        if slot.replace(leg).is_some() {
            return Err(protocol("寄售订单同种义务重复"));
        }
    }
    Ok(by_contract)
}

pub(super) fn protocol(message: &str) -> sqlx::Error {
    sqlx::Error::Protocol(message.to_owned())
}

/// 行里的值超出领域范围：保留类型化原因的文本（不含任何业务输入），失败关闭。
pub(super) fn protocol_from(error: impl std::fmt::Display) -> sqlx::Error {
    sqlx::Error::Protocol(error.to_string())
}

/// 把义务腿与资金投影收成领域事实。
fn facts_of(
    kind: OrderKind,
    world: ListingWorld,
    (funded, closure): (Option<&str>, Option<&str>),
    legs: &Legs,
) -> OrderFacts {
    use OrderDeliveryStage as D;
    use OrderLegStage as L;
    use OrderPaymentStage as P;

    // 原件所在腿：Withdrawal 是撤回件；main 成交是 COD 付款腿本身；isekai 成交是后放行的交接腿。
    let delivery_leg = match (kind, world) {
        (OrderKind::Withdrawal, _) => legs.withdrawn.as_ref(),
        (OrderKind::Sale, ListingWorld::Main) => legs.payment.as_ref(),
        (OrderKind::Sale, ListingWorld::Isekai) => legs.handoff.as_ref(),
    };
    let delivery = delivery_leg.map_or(D::Unknown, LegFacts::delivery_stage);
    let payout = legs.payout.as_ref().map_or(L::NotApplicable, LegFacts::cash_stage);
    let refund = legs.refund.as_ref().map_or(L::NotApplicable, LegFacts::cash_stage);
    let payment = match (kind, legs.payment.as_ref(), funded) {
        (OrderKind::Withdrawal, _, _) => P::NotApplicable,
        (OrderKind::Sale, None, _) => P::Unknown,
        (OrderKind::Sale, Some(leg), _) if leg.is_unresolved() => P::Unknown,
        // 实收认证与付款履约满足同事务写入：两者必须同时成立，否则是矛盾事实。
        (OrderKind::Sale, Some(leg), Some(_)) => {
            if leg.execution_state == "satisfied" {
                P::Certified
            } else {
                P::Unknown
            }
        }
        (OrderKind::Sale, Some(leg), None) => {
            if leg.execution_state == "satisfied" {
                P::Unknown
            } else {
                P::Unpaid
            }
        }
    };
    // 未付取消（7A-2 的 cancel_unpaid）：买家从未付款，付款腿与卖款腿一并封闭，退款腿不适用，
    // 合同结局为已取消。封闭事实只追加，没有付款事实时才可能出现（000137 的封闭守卫）。
    let cancelled_unpaid = kind == OrderKind::Sale && closure == Some("cancelled_unpaid");
    let (payment, payout, refund) = if cancelled_unpaid {
        (P::Closed, L::Closed, L::NotApplicable)
    } else {
        (payment, payout, refund)
    };
    let outcome = match kind {
        _ if cancelled_unpaid => OrderOutcome::Cancelled,
        OrderKind::Sale if payout == L::Claimed => OrderOutcome::Completed,
        OrderKind::Sale if refund == L::Claimed => OrderOutcome::Cancelled,
        OrderKind::Withdrawal if delivery == D::Claimed => OrderOutcome::Completed,
        _ => OrderOutcome::Open,
    };
    OrderFacts { kind, outcome, delivery, payment, payout, refund }
}

/// 按给定合同 ID 的顺序加载订单；不存在的 ID 被略过。
pub(super) async fn load_orders(
    conn: &mut PgConnection,
    contract_ids: &[Uuid],
) -> Result<Vec<LoadedOrder>, sqlx::Error> {
    if contract_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT c.contract_id, c.kind, c.listing_id, c.equipment_id, c.world_id,
                c.seller_user_id, c.frozen_seller_identity, c.created_at,
                s.buyer_user_id, s.frozen_buyer_identity, s.price, s.fee, s.seller_amount,
                s.payment_deadline_at, funded.disposition AS funded_disposition,
                closure.closure_kind,
                stream.next_sequence - 1 AS event_head, stream.updated_at AS event_at
         FROM public.consignment_contracts c
         LEFT JOIN public.consignment_sales s ON s.contract_id = c.contract_id
         LEFT JOIN public.consignment_funded_sales funded ON funded.contract_id = c.contract_id
         LEFT JOIN public.consignment_sale_closures closure
                ON closure.contract_id = c.contract_id
         LEFT JOIN public.realtime_streams stream
                ON stream.stream_key = $2 || c.contract_id::text
         WHERE c.contract_id = ANY($1)",
    )
    .bind(contract_ids.to_vec())
    .bind(ORDER_STREAM_PREFIX)
    .fetch_all(&mut *conn)
    .await?;
    let legs = load_legs(conn, contract_ids).await?;
    let mut loaded: HashMap<Uuid, LoadedOrder> = HashMap::new();
    for row in &rows {
        let order = loaded_order(row, &legs)?;
        loaded.insert(order.core.contract_id, order);
    }
    Ok(contract_ids.iter().filter_map(|contract_id| loaded.remove(contract_id)).collect())
}

fn loaded_order(row: &PgRow, legs: &HashMap<Uuid, Legs>) -> Result<LoadedOrder, sqlx::Error> {
    let contract_id: Uuid = row.try_get("contract_id")?;
    let kind = match row.try_get::<&str, _>("kind")? {
        "sale" => OrderKind::Sale,
        "withdrawal" => OrderKind::Withdrawal,
        _ => return Err(protocol("寄售合同种类未知")),
    };
    let world =
        row.try_get::<&str, _>("world_id")?.parse::<ListingWorld>().map_err(protocol_from)?;
    let funded: Option<String> = row.try_get("funded_disposition")?;
    let closure: Option<String> = row.try_get("closure_kind")?;
    let empty = Legs::default();
    let stages = project_order_stages(&facts_of(
        kind,
        world,
        (funded.as_deref(), closure.as_deref()),
        legs.get(&contract_id).unwrap_or(&empty),
    ));
    let created_at: Timestamp = row.try_get("created_at")?;
    let event_head: Option<i64> = row.try_get("event_head")?;
    let event_at: Option<Timestamp> = row.try_get("event_at")?;
    let price: Option<i64> = row.try_get("price")?;
    let price = price.map(|price| ListingPrice::new(price).map_err(protocol_from)).transpose()?;
    Ok(LoadedOrder {
        core: OrderCore {
            contract_id,
            kind,
            listing_id: row.try_get("listing_id")?,
            equipment_id: row.try_get("equipment_id")?,
            world,
            stages,
            version: event_head.map_or(1, |head| head.max(1).cast_unsigned()),
            price,
            deadline_at: row.try_get("payment_deadline_at")?,
            fee: row.try_get("fee")?,
            seller_amount: row.try_get("seller_amount")?,
            updated_at: event_at.map_or(created_at, |at| at.max(created_at)),
        },
        buyer_user_id: row.try_get("buyer_user_id")?,
        buyer_name: row.try_get("frozen_buyer_identity")?,
        seller_user_id: row.try_get("seller_user_id")?,
        seller_name: row.try_get("frozen_seller_identity")?,
    })
}
