//! 确领事务的写入部分：确领证明、position 已交付、释放占用、交接边、挂单成交关闭、funded 投影前进、isekai
//! 原件交割腿满足与卖款放行。调用方（[`super`]）已按锁序取锁并核完证据与付款认证，本模块只写、不判业务。
//!
//! 保护不变量 3、6、8：每个状态推进都带同版本的不可变事件；任何条件更新命中行数不符即失败关闭、整笔回滚。

use auction_application::consignment::ProveHandoff;
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{Evidence, Facts};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::dispatch::round::StoredRound;

/// 履约证明标签：与 COD 实收认证满足付款腿时同一标签（Sent 详情的领取结论）。
const PROOF_KIND: &str = "mooglemail_sent_collected";

/// 确领证明、position 已交付与同版本代管事件、释放实物占用、交接边、挂单成交关闭与挂单事件（R5-C2）。
pub(super) async fn write_handoff(
    connection: &mut PgConnection,
    command: &ProveHandoff,
    facts: &Facts,
    evidence: &Evidence,
    proof_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let command_id = command.command_id.as_uuid();
    let listing_version = facts.listing_version + 1;
    let position_version = facts.position_version + 1;
    sqlx::query(
        "INSERT INTO public.consignment_handoff_proofs
            (id, position_id, generation, contract_id, obligation_id, delivery_id, fence,
             validator_version, proof_digest, observed_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(proof_id)
    .bind(facts.position_id)
    .bind(facts.generation)
    .bind(command.key.contract_id)
    .bind(command.key.obligation_id)
    .bind(command.attempt.delivery_id)
    .bind(i64::try_from(command.attempt.fence)?)
    .bind(&evidence.validator_version)
    .bind(evidence.digest.as_slice())
    .bind(evidence.observed_at)
    .execute(&mut *connection)
    .await?;
    let moved = sqlx::query(
        "UPDATE public.consignment_custody_positions
         SET state = 'handed_over', version = $3, updated_at = GREATEST(updated_at, $4)
         WHERE id = $1 AND state = $2 AND version = $3 - 1",
    )
    .bind(facts.position_id)
    .bind(&facts.position_state)
    .bind(position_version)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if moved.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_handoff_position_cas"));
    }
    sqlx::query(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             handoff_proof_id, listing_id, listing_version, occurred_at)
         VALUES ($1, $2, $3, 'handed_over', $4, 'handed_over', NULL, $5, $6, $7, $8)",
    )
    .bind(facts.position_id)
    .bind(position_version)
    .bind(command_id)
    .bind(&facts.position_state)
    .bind(proof_id)
    .bind(facts.listing_id)
    .bind(listing_version)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    // 释放强占用：最后事实指向本确领证明（`c6_claim_handoff_fact`）。
    let released = sqlx::query(
        "UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = $2, last_fact_id = $3
         WHERE position_id = $1",
    )
    .bind(facts.position_id)
    .bind(now)
    .bind(proof_id)
    .execute(&mut *connection)
    .await?;
    if released.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_handoff_claim_release"));
    }
    insert_delivered_edge(connection, facts, proof_id, now).await?;
    sqlx::query(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ($1, $2, $3, 'worker', 'closed', 'sale_pending', 'closed', 'sold', $4, $5)",
    )
    .bind(facts.listing_id)
    .bind(listing_version)
    .bind(command_id)
    .bind(facts.price)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let closed = sqlx::query(
        "UPDATE public.consignment_listings
         SET state = 'closed', reason_code = 'sold', state_version = $2,
             updated_at = GREATEST(updated_at, $3)
         WHERE id = $1 AND state = 'sale_pending' AND state_version = $2 - 1",
    )
    .bind(facts.listing_id)
    .bind(listing_version)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if closed.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_handoff_listing_cas"));
    }
    Ok(())
}

/// 交接历史追加 `consignment_delivered` 边：原卖家 → 买家，接在同实物、同季的最近一条边之后（D9 §2.4，R5-C2）。
async fn insert_delivered_edge(
    connection: &mut PgConnection,
    facts: &Facts,
    proof_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let season_id: Option<String> = sqlx::query_scalar(
        "SELECT season_id FROM public.consignment_custody_assets WHERE equipment_id = $1",
    )
    .bind(facts.equipment_id)
    .fetch_one(&mut *connection)
    .await?;
    let predecessor: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.consignment_equipment_handoffs
         WHERE world_id = $1 AND canonical_eid = $2 AND season_id IS NOT DISTINCT FROM $3
         ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(&facts.world_id)
    .bind(&facts.canonical_eid)
    .bind(&season_id)
    .fetch_optional(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_equipment_handoffs
            (id, equipment_id, world_id, canonical_eid, season_id, predecessor_id,
             from_user_id, to_user_id, kind, auction_lot_id, position_id, proof_ref,
             occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'consignment_delivered', NULL, $9, $10, $11)",
    )
    .bind(Uuid::new_v4())
    .bind(facts.equipment_id)
    .bind(&facts.world_id)
    .bind(&facts.canonical_eid)
    .bind(&season_id)
    .bind(predecessor)
    .bind(facts.seller_user_id)
    .bind(facts.buyer_user_id)
    .bind(facts.position_id)
    .bind(format!("consignment-handoff:{proof_id}"))
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// funded 投影 `funded → original_claimed`：原件已确领，退款转换不再可能（`convert_refund` 只认 `funded`）。
pub(super) async fn advance_funded(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<(), WriteError> {
    let advanced = sqlx::query(
        "UPDATE public.consignment_funded_sales
         SET disposition = 'original_claimed', version = version + 1
         WHERE contract_id = $1 AND disposition = 'funded'",
    )
    .bind(contract_id)
    .execute(connection)
    .await?;
    if advanced.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_handoff_funded"));
    }
    Ok(())
}

/// isekai 原件交割腿以确领证明满足（履约行已锁，衔接在本轮投递）；main 的原件 COD 腿已由实收认证满足。
pub(super) async fn satisfy_handoff(
    connection: &mut PgConnection,
    command: &ProveHandoff,
    round: &StoredRound,
    evidence: &Evidence,
) -> Result<(), WriteError> {
    let updated = sqlx::query(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'satisfied', proof_kind = $3, proof_reference = $4,
             proof_digest = $5, proof_observed_at = $6,
             claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL,
             completed_at = (SELECT at FROM sql_clock), updated_at = (SELECT at FROM sql_clock),
             state_version = state_version + 1, next_attempt_at = NULL, last_error_code = NULL
         WHERE obligation_id = $1 AND execution_state = 'enqueued' AND delivery_id = $2
           AND delivery_operation = 'send'",
    )
    .bind(command.key.obligation_id)
    .bind(round.delivery_id)
    .bind(PROOF_KIND)
    .bind(&command.remote_observation_id)
    .bind(evidence.digest.as_slice())
    .bind(evidence.observed_at)
    .execute(connection)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_handoff_fulfillment"));
    }
    Ok(())
}

/// 卖款 `blocked → ready`：前置恰为本原件腿义务，且前置履约已满足（同事务刚满足或已由实收认证满足）。
pub(super) async fn release_payout(
    connection: &mut PgConnection,
    contract_id: Uuid,
    original_obligation_id: Uuid,
) -> Result<(), WriteError> {
    let released = sqlx::query(
        "UPDATE public.settlement_obligations AS payout
         SET business_state = 'ready', state_version = payout.state_version + 1,
             updated_at = clock_timestamp()
         FROM public.settlement_obligation_fulfillments AS prerequisite
         WHERE payout.contract_id = $1 AND payout.obligation_kind = 'seller_payout'
           AND payout.business_state = 'blocked'
           AND payout.prerequisite_obligation_id = $2
           AND prerequisite.obligation_id = $2 AND prerequisite.execution_state = 'satisfied'",
    )
    .bind(contract_id)
    .bind(original_obligation_id)
    .execute(connection)
    .await?;
    if released.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_handoff_payout_release"));
    }
    Ok(())
}
