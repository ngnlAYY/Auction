//! 灭失确认的锁内事实读取与分流：当前代是否仍有有效买方交割权或资金责任。
//!
//! 保护不变量 2、6：事实只在调用方已持有命令、卖家、实物与挂单锁之后读出，并锁住 position 行；
//! 分流交领域纯函数 `route_position_loss`（D9 §5.3 “已证代管件灭失”表）。以“当前代是否仍有有效买方
//! 交割权或资金责任”分流，而不是“当前代是否签过 Sale”：已封闭且原件已收回的 Sale、从未成交的当前代、
//! 前代已封闭而新代未售都按 position 办结；Sale 未封闭则仍有买方权利，改走原 Sale resolve；读不清的一律
//! 保留占用，不终局、也不派出相反付款。

use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::recovery::LossRoutingFacts;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::recovery::withdrawal::unknown_dispatch_open;

/// 锁内读出的 position；字段全部来自持有行锁时的读取。
pub(super) struct LossPosition {
    pub(super) position_id: Uuid,
    pub(super) listing_id: Uuid,
    pub(super) seller_user_id: Uuid,
    pub(super) equipment_id: Uuid,
    pub(super) world_id: String,
    pub(super) canonical_eid: String,
    pub(super) season_window_id: Option<Uuid>,
    pub(super) state: PositionState,
    pub(super) version: i64,
    pub(super) generation: Option<i64>,
}

/// 预读 position 所属挂单，只用于取锁键，没有授权含义；锁内会重读并核对。
pub(super) async fn pre_read_listing(
    store: &DatabaseStore,
    position_id: Uuid,
) -> Result<Option<Uuid>, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT listing_id FROM public.consignment_custody_positions WHERE id = $1",
    )
    .bind(position_id)
    .fetch_optional(store.pool())
    .await?)
}

/// 锁定 position 行；不存在返回 `None`。
pub(super) async fn lock_loss_position(
    connection: &mut PgConnection,
    position_id: Uuid,
) -> Result<Option<LossPosition>, WriteError> {
    let row = sqlx::query(
        "SELECT id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid,
                season_window_id, state, version, current_generation
         FROM public.consignment_custody_positions WHERE id = $1 FOR UPDATE",
    )
    .bind(position_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let state: &str = row.try_get("state")?;
    Ok(Some(LossPosition {
        position_id: row.try_get("id")?,
        listing_id: row.try_get("listing_id")?,
        seller_user_id: row.try_get("seller_user_id")?,
        equipment_id: row.try_get("equipment_id")?,
        world_id: row.try_get("world_id")?,
        canonical_eid: row.try_get("canonical_eid")?,
        season_window_id: row.try_get("season_window_id")?,
        state: PositionState::parse(state)
            .ok_or_else(|| WriteError::invariant("consignment_loss_position_state"))?,
        version: row.try_get("version")?,
        generation: row.try_get("current_generation")?,
    }))
}

/// 灭失证据是否完整：证据属于本 position，原世界、原物与季都与 position 一致。
///
/// 证据行本身在追加时已拒绝空库存、缺 MID 与说明文字（见 `append_loss_proof`）；这里复核它确实挂在
/// 这件实物上，防止拿别的件、别的季的证据确认本件灭失。
pub(super) async fn proof_complete(
    connection: &mut PgConnection,
    position: &LossPosition,
    proof_id: Uuid,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_loss_proofs AS proof
             WHERE proof.proof_id = $1 AND proof.position_id = $2
               AND proof.world_id = $3 AND proof.canonical_eid = $4
               AND proof.season_id IS NOT DISTINCT FROM (
                   SELECT window_row.season_id FROM public.consignment_season_windows AS window_row
                   WHERE window_row.id = $5)
         )",
    )
    .bind(proof_id)
    .bind(position.position_id)
    .bind(&position.world_id)
    .bind(&position.canonical_eid)
    .bind(position.season_window_id)
    .fetch_one(connection)
    .await?)
}

/// 已封闭的 Sale 下是否仍有可派工作：原件腿的派发轮其投递尚未起写也未终结。
///
/// 不变量 4、6：这是“相反可派工作”——损失确认后它若仍会被发出，就会把已判灭失的原件寄给买家。
/// 终止它属于投递 owner，本命令不跨 owner 改投递；因此只要它还在，就按无法查清处理，
/// 保留占用，待其 owner 终止或已证结果后以新命令重试。未封闭的 Sale 本就改走原 Sale resolve，不在此判断。
async fn live_original_work_after_closure(
    connection: &mut PgConnection,
    position_id: Uuid,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
             FROM public.consignment_contracts AS contract
             JOIN public.consignment_sale_closures AS closure
               ON closure.contract_id = contract.contract_id
             JOIN public.consignment_dispatch_rounds AS round_row
               ON round_row.contract_id = contract.contract_id
             JOIN public.settlement_obligations AS obligation
               ON obligation.id = round_row.obligation_id
             JOIN public.deliveries AS delivery ON delivery.id = round_row.delivery_id
             WHERE contract.position_id = $1
               AND obligation.obligation_kind IN ('winner_equipment_cod', 'winner_equipment_handoff')
               AND delivery.status IN ('prepared', 'pending', 'claimed', 'sending')
               AND NOT EXISTS (
                   SELECT 1 FROM public.delivery_send_attempts AS send_attempt
                   WHERE send_attempt.delivery_id = delivery.id)
         )",
    )
    .bind(position_id)
    .fetch_one(connection)
    .await?)
}

/// 一份合同在当前代的权责读数。
struct ContractRow {
    generation: i64,
    kind: String,
    closed: bool,
    original_back: bool,
    funded_disposition: Option<String>,
}

/// 读出分流事实：买方交割权、资金责任与相反未知外写。
///
/// 不变量 4、6：
/// - 当前代没有合同（从未成交，或前代已封闭而新代未售）→ 两项都明确为否；
/// - 当前代是已封闭且原件已收回的 Sale → 两项都明确为否（该 Sale 的退款或取消已由原 owner 完成，
///   旧买家资金不在这里动）；已封闭但原件是否在库未证 → 买方权利无法查清；
/// - 当前代 Sale 未封闭 → 买方交割权仍有效，已收款（funded 投影在）则另有资金责任；
/// - 其他代出现未封闭的合同、当前代是 Withdrawal、投影处于无法对上的组合，一律无法查清。
pub(super) async fn routing_facts(
    connection: &mut PgConnection,
    position: &LossPosition,
    proof_complete: bool,
) -> Result<LossRoutingFacts, WriteError> {
    let opposite_unknown_write = unknown_dispatch_open(connection, position.position_id).await?
        || live_original_work_after_closure(connection, position.position_id).await?;
    let rows = sqlx::query(
        "SELECT contract.generation, contract.kind,
                closure.contract_id IS NOT NULL AS closed,
                public.c7_sale_original_back(contract.contract_id) AS original_back,
                funded.disposition AS funded_disposition
         FROM public.consignment_contracts AS contract
         LEFT JOIN public.consignment_sale_closures AS closure
           ON closure.contract_id = contract.contract_id
         LEFT JOIN public.consignment_funded_sales AS funded
           ON funded.contract_id = contract.contract_id
         WHERE contract.position_id = $1",
    )
    .bind(position.position_id)
    .fetch_all(&mut *connection)
    .await?;
    let contracts = rows
        .iter()
        .map(|row| {
            Ok(ContractRow {
                generation: row.try_get("generation")?,
                kind: row.try_get("kind")?,
                closed: row.try_get("closed")?,
                original_back: row.try_get("original_back")?,
                funded_disposition: row.try_get("funded_disposition")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let (buyer_delivery_right, funding_liability) = current_generation_rights(position, &contracts);
    Ok(LossRoutingFacts {
        proof_complete,
        current_generation_buyer_delivery_right: buyer_delivery_right,
        current_generation_funding_liability: funding_liability,
        opposite_unknown_write,
    })
}

/// 把合同行折成 (有效买方交割权, 资金责任)；`None` 表示无法查清。
fn current_generation_rights(
    position: &LossPosition,
    contracts: &[ContractRow],
) -> (Option<bool>, Option<bool>) {
    let Some(generation) = position.generation else { return (None, None) };
    let mut current = None;
    for contract in contracts {
        if contract.generation == generation {
            current = Some(contract);
        } else if contract.kind != "sale" || !contract.closed {
            // 其他代只可能是已封闭的 Sale；否则占用与交割状态读不清。
            return (None, None);
        }
    }
    let Some(current) = current else { return (Some(false), Some(false)) };
    if current.kind != "sale" {
        return (None, None);
    }
    match (current.closed, current.funded_disposition.as_deref()) {
        (true, _) if current.original_back => (Some(false), Some(false)),
        (true, _) => (None, Some(false)),
        (false, None) => (Some(true), Some(false)),
        (false, Some("funded" | "original_claimed")) => (Some(true), Some(true)),
        (false, Some(_)) => (None, None),
    }
}
