//! 整封取得证明成立后逐件推进 position 与挂单。

use std::collections::BTreeMap;

use auction_application::consignment::{
    ConsignmentError, CustodyListingChange, DepositBusinessRejection,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::deposit::{
    ListingDecision, ListingDecisionFacts, PositionState, decide_listing_after_custody,
};
use auction_domain::consignment::{HeldOffMarketReason, ListingState};
use auction_time::Timestamp;
use sqlx::Row;
use uuid::Uuid;

use super::generations::{FIRST_GENERATION, FIRST_GENERATION_PURPOSE};
use super::history::custody_acquired_proof_ref;
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::{
    SeasonWindow, active_hold, active_withdrawal_intent, current_snapshot, quota_available,
    season_state, seller_eligibility, service_control, transition,
};
use crate::database::consignment::listings::projection::{LockedListing, lock_listings};

struct CustodyPosition {
    position_id: Uuid,
    listing_id: Uuid,
    seller_user_id: Uuid,
    equipment_id: Uuid,
    world_id: String,
    canonical_eid: String,
    season_window_id: Option<Uuid>,
    state: String,
    version: i64,
}

/// 全封取得证明成立后的逐件推进。
///
/// 前置条件：调用方已持有命令、卖家、实物与 K 锁；`mail_disposition_proofs` 已有本事务
/// 写入的 `exact_take/take_proven` 行。命令行由 [`DepositWriter::finish`] 最后写入，
/// 000134 的 custody event 与 generation 外键在提交时统一校验。不变量 2、3、6、10：
/// 任一 position 失败整体回滚，挂单与 position 的版本同事务推进。
///
/// `max_open_per_user` 由应用配置冻结值传入；存储只按锁内有效活动数判断名额。
pub(crate) async fn apply_take_custody_in_tx(
    writer: &mut DepositWriter<'_, '_, '_>,
    decision_id: Uuid,
    proof_id: Uuid,
    now: Timestamp,
    max_open_per_user: i64,
) -> Result<Vec<CustodyListingChange>, WriteError> {
    if max_open_per_user < 1 {
        return Err(rejection(DepositBusinessRejection::ActionNotAllowed));
    }
    let command_id = writer.command_id();
    let proof = sqlx::query(
        "SELECT work_id, kind, conclusion FROM public.mail_disposition_proofs
         WHERE id = $1 AND decision_id = $2",
    )
    .bind(proof_id)
    .bind(decision_id)
    .fetch_optional(writer.connection())
    .await?
    .ok_or_else(|| rejection(DepositBusinessRejection::EvidenceUnproven))?;
    let kind: String = proof.try_get("kind")?;
    let conclusion: String = proof.try_get("conclusion")?;
    let work_id: Uuid = proof.try_get("work_id")?;
    if kind != "exact_take" || conclusion != "take_proven" {
        return Err(rejection(DepositBusinessRejection::EvidenceUnproven));
    }
    let work = sqlx::query(
        "SELECT action FROM public.mail_disposition_work WHERE id = $1 AND decision_id = $2",
    )
    .bind(work_id)
    .bind(decision_id)
    .fetch_optional(writer.connection())
    .await?;
    let Some(work) = work else {
        return Err(rejection(DepositBusinessRejection::EvidenceUnproven));
    };
    let action: String = work.try_get("action")?;
    if action != "take" {
        return Err(rejection(DepositBusinessRejection::SourceMismatch));
    }
    let rows = sqlx::query(
        "SELECT p.id, p.listing_id, p.seller_user_id, p.equipment_id, p.world_id,
                p.canonical_eid, p.season_window_id, p.state, p.version
         FROM public.consignment_custody_positions p
         JOIN public.consignment_deposit_items i ON i.id = p.item_id
         WHERE i.decision_id = $1
         ORDER BY i.ordinal, p.id FOR UPDATE OF p",
    )
    .bind(decision_id)
    .fetch_all(writer.connection())
    .await?;
    if rows.is_empty() {
        return Err(rejection(DepositBusinessRejection::EvidenceUnproven));
    }
    let positions: Vec<CustodyPosition> = rows
        .iter()
        .map(|row| {
            Ok(CustodyPosition {
                position_id: row.try_get("id")?,
                listing_id: row.try_get("listing_id")?,
                seller_user_id: row.try_get("seller_user_id")?,
                equipment_id: row.try_get("equipment_id")?,
                world_id: row.try_get("world_id")?,
                canonical_eid: row.try_get("canonical_eid")?,
                season_window_id: row.try_get("season_window_id")?,
                state: row.try_get("state")?,
                version: row.try_get("version")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()?;
    let listing_ids: Vec<Uuid> = positions.iter().map(|position| position.listing_id).collect();
    let listings: BTreeMap<Uuid, LockedListing> = lock_listings(writer.connection(), &listing_ids)
        .await?
        .into_iter()
        .map(|listing| (listing.listing_id, listing))
        .collect();
    if listings.len() != positions.len() {
        return Err(rejection(DepositBusinessRejection::ListingNotActive));
    }
    let control = service_control(writer.connection()).await?;
    let Some(control) = control else {
        return Err(rejection(DepositBusinessRejection::MailUnavailable));
    };
    let mut changes = Vec::with_capacity(positions.len());
    for position in positions {
        if position.state != "awaiting_take" || position.version != 1 {
            return Err(rejection(DepositBusinessRejection::ActionNotAllowed));
        }
        let listing = listings
            .get(&position.listing_id)
            .ok_or_else(|| rejection(DepositBusinessRejection::ListingNotActive))?;
        // Admin 止售可在 Take 前把挂单留在 held_off_market；未代管件的撤回意愿也可能已
        // 登记。这些状态仍须完成 position 推进，不能因挂单投影先行变化而丢失取得事实。
        if !matches!(
            listing.snapshot.state,
            ListingState::DepositAccepted
                | ListingState::HeldOffMarket
                | ListingState::WithdrawalPending
        ) || listing.identity.seller_user_id != position.seller_user_id
            || listing.identity.equipment.external_equipment_id() != position.canonical_eid
            || listing.identity.equipment.world_id() != position.world_id
        {
            return Err(rejection(DepositBusinessRejection::ActionNotAllowed));
        }
        let world = parse_world(&position.world_id)?;
        let window = season_window(writer.connection(), position.season_window_id).await?;
        let season = season_state(world, window.as_ref(), now);
        let withdrawal_requested =
            active_withdrawal_intent(writer.connection(), position.position_id).await?.is_some();
        let operations_hold =
            active_hold(writer.connection(), position.position_id).await?.is_some();
        let seller = seller_eligibility(
            writer.connection(),
            auction_domain::UserId::from_uuid(position.seller_user_id),
        )
        .await?;
        let seller_eligible = seller.is_some_and(|seller| {
            seller.active
                && !seller.active_blacklist
                && seller.blacklist_count == listing.blacklist_count_at_register
        });
        let service_enabled = control.admitting && control.enabled_epoch == listing.service_epoch;
        let quota = quota_available(
            writer.connection(),
            auction_domain::UserId::from_uuid(position.seller_user_id),
            now,
            max_open_per_user,
        )
        .await?;
        let decision = decide_listing_after_custody(ListingDecisionFacts {
            season_state: season,
            withdrawal_requested,
            operations_hold,
            seller_eligible,
            service_enabled,
            quota_available: quota,
        });
        let (version, state, held_reason) =
            listing_projection(writer, listing, decision, now).await?;
        advance_position(writer, &position, command_id, proof_id, version, now).await?;
        changes.push(CustodyListingChange {
            listing_id: position.listing_id,
            listing_version: version,
            state,
            held_reason,
            position_id: position.position_id,
            position_state: PositionState::InCustody,
        });
    }
    Ok(changes)
}

async fn advance_position(
    writer: &mut DepositWriter<'_, '_, '_>,
    position: &CustodyPosition,
    command_id: Uuid,
    proof_id: Uuid,
    listing_version: u64,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE public.consignment_custody_positions
         SET state = 'in_custody', version = 2, current_generation = 1, updated_at = $2
         WHERE id = $1 AND state = 'awaiting_take' AND version = 1",
    )
    .bind(position.position_id)
    .bind(now)
    .execute(writer.connection())
    .await?;
    let (service_epoch, baseline): (i64, Vec<Uuid>) = sqlx::query_as(
        "SELECT service_epoch, blacklist_baseline_ids
         FROM public.consignment_listings WHERE id = $1 FOR UPDATE",
    )
    .bind(position.listing_id)
    .fetch_one(writer.connection())
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_disposition_generations
            (position_id, generation, purpose, predecessor, proof_id, service_epoch,
             blacklist_baseline_ids, season_window_id, created_command_id, created_at)
         VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, $8, $9)",
    )
    .bind(position.position_id)
    .bind(i64::try_from(FIRST_GENERATION)?)
    .bind(FIRST_GENERATION_PURPOSE)
    .bind(proof_id)
    .bind(service_epoch)
    .bind(baseline)
    .bind(position.season_window_id)
    .bind(command_id)
    .bind(now)
    .execute(writer.connection())
    .await?;
    let season_id: Option<String> = sqlx::query_scalar(
        "SELECT season_id FROM public.consignment_custody_assets WHERE equipment_id = $1",
    )
    .bind(position.equipment_id)
    .fetch_one(writer.connection())
    .await?;
    let predecessor: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.consignment_equipment_handoffs
         WHERE world_id = $1 AND canonical_eid = $2 AND season_id IS NOT DISTINCT FROM $3
         ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(&position.world_id)
    .bind(&position.canonical_eid)
    .bind(&season_id)
    .fetch_optional(writer.connection())
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_equipment_handoffs
            (id, equipment_id, world_id, canonical_eid, season_id, predecessor_id,
             from_user_id, to_user_id, kind, auction_lot_id, position_id, proof_ref,
             occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7, 'custody_acquired', NULL, $8, $9, $10)",
    )
    .bind(Uuid::new_v4())
    .bind(position.equipment_id)
    .bind(&position.world_id)
    .bind(&position.canonical_eid)
    .bind(&season_id)
    .bind(predecessor)
    .bind(position.seller_user_id)
    .bind(position.position_id)
    .bind(custody_acquired_proof_ref(proof_id))
    .bind(now)
    .execute(writer.connection())
    .await?;
    // 不变量 2、3、8：position 的每次状态推进都必须有同版本的不可变代管事件，
    // 后续释放强占用时以该事件及其证明复核持有人。
    sqlx::query(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ($1, 2, $2, 'in_custody', 'awaiting_take', 'in_custody', $3, $4, $5, $6)",
    )
    .bind(position.position_id)
    .bind(command_id)
    .bind(proof_id)
    .bind(position.listing_id)
    .bind(i64::try_from(listing_version)?)
    .bind(now)
    .execute(writer.connection())
    .await?;
    Ok(())
}

async fn listing_projection(
    writer: &mut DepositWriter<'_, '_, '_>,
    listing: &LockedListing,
    decision: ListingDecision,
    now: Timestamp,
) -> Result<(u64, ListingState, Option<HeldOffMarketReason>), WriteError> {
    // 已下架或撤回态没有可用的后续挂单迁移；取得只推进 position 并保留原投影。
    if listing.snapshot.state == ListingState::HeldOffMarket {
        if decision == ListingDecision::WithdrawalPending {
            let projection =
                transition(current_snapshot(listing), ListingState::WithdrawalPending, None)?;
            writer
                .stage_listing_update(
                    listing,
                    projection,
                    listing.current_snapshot_id,
                    "withdrawal_pending",
                    None,
                    now,
                )
                .await?;
            return Ok((projection.state_version.value(), ListingState::WithdrawalPending, None));
        }
        let reason: Option<String> =
            sqlx::query_scalar("SELECT reason_code FROM public.consignment_listings WHERE id = $1")
                .bind(listing.listing_id)
                .fetch_one(writer.connection())
                .await?;
        let held_reason = reason.as_deref().map(HeldOffMarketReason::parse).transpose()?;
        return Ok((
            listing.snapshot.state_version.value(),
            ListingState::HeldOffMarket,
            held_reason,
        ));
    }
    if listing.snapshot.state == ListingState::WithdrawalPending {
        return Ok((listing.snapshot.state_version.value(), ListingState::WithdrawalPending, None));
    }
    match decision {
        ListingDecision::Available => {
            let projection = transition(current_snapshot(listing), ListingState::Available, None)?;
            writer
                .stage_listing_update(
                    listing,
                    projection,
                    listing.current_snapshot_id,
                    "available",
                    None,
                    now,
                )
                .await?;
            Ok((projection.state_version.value(), ListingState::Available, None))
        }
        ListingDecision::HeldOffMarket(reason) => {
            let projection =
                transition(current_snapshot(listing), ListingState::HeldOffMarket, None)?;
            writer
                .stage_listing_update(
                    listing,
                    projection,
                    listing.current_snapshot_id,
                    "hold_set",
                    Some(reason.as_str()),
                    now,
                )
                .await?;
            Ok((projection.state_version.value(), ListingState::HeldOffMarket, Some(reason)))
        }
        ListingDecision::WithdrawalPending => {
            let projection =
                transition(current_snapshot(listing), ListingState::WithdrawalPending, None)?;
            writer
                .stage_listing_update(
                    listing,
                    projection,
                    listing.current_snapshot_id,
                    "withdrawal_pending",
                    None,
                    now,
                )
                .await?;
            Ok((projection.state_version.value(), ListingState::WithdrawalPending, None))
        }
        ListingDecision::SeasonPreclose | ListingDecision::SeasonClosed => {
            Ok((listing.snapshot.state_version.value(), ListingState::DepositAccepted, None))
        }
    }
}

fn parse_world(value: &str) -> Result<SettlementWorld, WriteError> {
    auction_domain::consignment::parse_settlement_world(value)
        .ok_or_else(|| WriteError::invariant("consignment_custody_world"))
}

async fn season_window(
    connection: &mut sqlx::PgConnection,
    id: Option<Uuid>,
) -> Result<Option<SeasonWindow>, WriteError> {
    let Some(id) = id else { return Ok(None) };
    let row = sqlx::query(
        "SELECT season_id, start_utc, end_utc, admission_cutoff_utc, calendar_version
         FROM public.consignment_season_windows WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(SeasonWindow {
        id,
        start_utc: row.try_get("start_utc")?,
        end_utc: row.try_get("end_utc")?,
        admission_cutoff_utc: row.try_get("admission_cutoff_utc")?,
    }))
}

fn rejection(error: DepositBusinessRejection) -> WriteError {
    ConsignmentError::Deposit(error).into()
}
