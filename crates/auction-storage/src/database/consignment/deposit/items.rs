//! 整封接受：逐件消费预约、建立 position、把强占用无空档转给实物，并暂存挂单版本。

use std::collections::{BTreeMap, BTreeSet};

use auction_application::consignment::DepositBusinessRejection;
use auction_domain::consignment::deposit::MatchedDepositItem;
use auction_domain::consignment::{ListingState, OriginalMailKey};
use auction_time::Timestamp;
use sqlx::Row;
use uuid::Uuid;

use super::outcome::conflict;
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::transition;
use crate::database::consignment::eligibility::claims::transfer_to_position;
use crate::database::consignment::listings::projection::LockedListing;

/// 整封接受后按原件序号返回的逐件事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AcceptedDepositItem {
    /// 原始附件序号。
    pub(crate) ordinal: u32,
    /// 挂单 UUID。
    pub(crate) listing_id: Uuid,
    /// 预约 UUID。
    pub(crate) reservation_id: Uuid,
    /// position UUID。
    pub(crate) position_id: Uuid,
    /// 收到后的挂单版本。
    pub(crate) listing_version: u64,
}

/// 整封接受的事务内原语。
///
/// 前置条件：调用方已取得命令、卖家、规范实物与 K 锁；已写入本事务的
/// `mail_final_dispositions` accepted 行、观察/资产、唯一 Take 工作与逐件输入。
/// 本原语自行把挂单从 `awaiting_deposit` 推进到 `deposit_accepted`，因此调用方必须在
/// `DepositWriter::finish` 前调用，最终回执的 `listing_changes` 取自
/// `DepositWriter::listing_changes`。
///
/// 不变量 2、3、6、10：任何一件失败整体返回错误，由调用方顶层事务回滚；预约截止以
/// 锁后 `now` 判断且等号失效；预约、item、position、强占用与挂单版本在同一事务内推进，
/// 不允许先释放再重占。
pub(crate) async fn accept_items_in_tx(
    writer: &mut DepositWriter<'_, '_, '_>,
    decision_id: Uuid,
    mail_key: &OriginalMailKey,
    items: &[MatchedDepositItem],
    now: Timestamp,
) -> Result<Vec<AcceptedDepositItem>, WriteError> {
    if items.is_empty() || items.len() > 20 {
        return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
    }
    let mut ordinals = BTreeSet::new();
    let mut eids = BTreeSet::new();
    let mut listing_ids = BTreeSet::new();
    let mut reservation_ids = BTreeSet::new();
    for item in items {
        if item.ordinal == 0
            || !ordinals.insert(item.ordinal)
            || !eids.insert(item.equipment.external_equipment_id().to_owned())
            || !listing_ids.insert(item.listing_id)
            || !reservation_ids.insert(item.reservation_id)
        {
            return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
        }
    }
    let decision = sqlx::query(
        "SELECT kind, source_observation_id, command_id
         FROM public.mail_final_dispositions
         WHERE bot_uid = $1 AND world_id = $2 AND external_mid = $3 AND id = $4",
    )
    .bind(mail_key.authenticated_bot_uid())
    .bind(auction_domain::consignment::settlement_world_code(mail_key.world()))
    .bind(mail_key.external_mid())
    .bind(decision_id)
    .fetch_optional(writer.connection())
    .await?
    .ok_or_else(|| WriteError::invariant("consignment_accept_decision_missing"))?;
    let kind: String = decision.try_get("kind")?;
    let observation_id: Uuid = decision.try_get("source_observation_id")?;
    let command_id: Uuid = decision.try_get("command_id")?;
    if kind != "accepted" {
        return Err(conflict(DepositBusinessRejection::DispositionConflict));
    }
    let listings: BTreeMap<Uuid, LockedListing> = {
        let ids: Vec<_> = listing_ids.iter().copied().collect();
        let rows = crate::database::consignment::listings::projection::lock_listings(
            writer.connection(),
            &ids,
        )
        .await?;
        if rows.len() != ids.len() {
            return Err(conflict(DepositBusinessRejection::ListingNotActive));
        }
        rows.into_iter().map(|listing| (listing.listing_id, listing)).collect()
    };
    let mut reservations = BTreeMap::new();
    for reservation_id in &reservation_ids {
        let row = sqlx::query(
            "SELECT listing_id, seller_user_id, world_id, canonical_eid, generation,
                    expires_at, state
             FROM public.consignment_deposit_reservations WHERE id = $1 FOR UPDATE",
        )
        .bind(reservation_id)
        .fetch_optional(writer.connection())
        .await?
        .ok_or_else(|| conflict(DepositBusinessRejection::ListingNotActive))?;
        reservations.insert(*reservation_id, row);
    }
    let mut assets = BTreeMap::new();
    for item in items {
        let season_id: Option<String> = sqlx::query_scalar(
            "SELECT season_id FROM public.consignment_season_windows WHERE id = (
                SELECT season_window_id FROM public.consignment_deposit_reservations
                WHERE id = $1)",
        )
        .bind(item.reservation_id)
        .fetch_optional(writer.connection())
        .await?
        .flatten();
        let equipment_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT equipment_id FROM public.consignment_custody_assets
             WHERE world_id = $1 AND canonical_eid = $2 AND season_id IS NOT DISTINCT FROM $3",
        )
        .bind(item.equipment.world_id())
        .bind(item.equipment.external_equipment_id())
        .bind(season_id)
        .fetch_optional(writer.connection())
        .await?;
        assets.insert(
            item.ordinal,
            equipment_id.ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?,
        );
    }
    let mut accepted = Vec::with_capacity(items.len());
    for item in items {
        let listing = listings
            .get(&item.listing_id)
            .ok_or_else(|| conflict(DepositBusinessRejection::ListingNotActive))?;
        if listing.identity.seller_user_id != item.seller_user_id.as_uuid()
            || listing.identity.equipment != item.equipment
        {
            return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
        }
        if listing.snapshot.state != ListingState::AwaitingDeposit {
            return Err(conflict(DepositBusinessRejection::ListingNotActive));
        }
        if listing.snapshot.state_version != item.listing_version {
            return Err(conflict(DepositBusinessRejection::StaleVersion));
        }
        let reservation = reservations
            .get(&item.reservation_id)
            .ok_or_else(|| conflict(DepositBusinessRejection::ListingNotActive))?;
        let reservation_state: String = reservation.try_get("state")?;
        let reservation_generation: i64 = reservation.try_get("generation")?;
        let reservation_listing: Uuid = reservation.try_get("listing_id")?;
        let reservation_seller: Uuid = reservation.try_get("seller_user_id")?;
        let reservation_world: String = reservation.try_get("world_id")?;
        let reservation_eid: String = reservation.try_get("canonical_eid")?;
        let expires_at: Timestamp = reservation.try_get("expires_at")?;
        if reservation_state != "active"
            || u64::try_from(reservation_generation).ok() != Some(item.reservation_generation)
            || reservation_listing != item.listing_id
            || reservation_seller != item.seller_user_id.as_uuid()
            || reservation_world != item.equipment.world_id()
            || reservation_eid != item.equipment.external_equipment_id()
        {
            return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
        }
        // D9 §2.1 第 4 条：截止后首次处理不能倒签；锁后时刻等号即失效。
        if now >= expires_at {
            return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
        }
        let Some(credential_ref) = item.credential_ref else {
            return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
        };
        let equipment_id = assets
            .get(&item.ordinal)
            .copied()
            .ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
        let projection = transition(listing.snapshot, ListingState::DepositAccepted, None)?;
        let accepted_version = projection.state_version.value();
        writer
            .stage_listing_update(
                listing,
                projection,
                listing.current_snapshot_id,
                "deposit_accepted",
                None,
                now,
            )
            .await?;
        let consumed = sqlx::query(
            "UPDATE public.consignment_deposit_reservations
             SET state = 'consumed', last_command_id = $2, retired_at = NULL, reason = NULL
             WHERE id = $1 AND state = 'active'",
        )
        .bind(item.reservation_id)
        .bind(command_id)
        .execute(writer.connection())
        .await?;
        if consumed.rows_affected() != 1 {
            return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
        }
        // 不变量 2、7：消费后的预约码不能再被读取或外写使用。
        sqlx::query(
            "UPDATE public.consignment_reservation_secrets
             SET ciphertext = NULL, nonce = NULL, encryption_key_id = NULL, fingerprint = NULL,
                 retired_at = $2, updated_at = $2
             WHERE reservation_id = $1 AND retired_at IS NULL",
        )
        .bind(item.reservation_id)
        .bind(now)
        .execute(writer.connection())
        .await?;
        let item_id = Uuid::new_v4();
        let position_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO public.consignment_deposit_items
                (id, bot_uid, world_id, external_mid, decision_id, ordinal, canonical_eid,
                 reservation_id, listing_id, seller_user_id, equipment_id,
                 accepted_listing_version, accepted_price, credential_ref, observation_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(item_id)
        .bind(mail_key.authenticated_bot_uid())
        .bind(auction_domain::consignment::settlement_world_code(mail_key.world()))
        .bind(mail_key.external_mid())
        .bind(decision_id)
        .bind(
            i32::try_from(item.ordinal)
                .ok()
                .ok_or_else(|| WriteError::invariant("consignment_item_ordinal"))?,
        )
        .bind(item.equipment.external_equipment_id())
        .bind(item.reservation_id)
        .bind(item.listing_id)
        .bind(item.seller_user_id.as_uuid())
        .bind(equipment_id)
        .bind(
            i64::try_from(accepted_version)
                .ok()
                .ok_or_else(|| WriteError::invariant("consignment_item_version"))?,
        )
        .bind(listing.snapshot.price.value())
        .bind(credential_ref)
        .bind(observation_id)
        .execute(writer.connection())
        .await?;
        let season_window_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT season_window_id FROM public.consignment_deposit_reservations WHERE id = $1",
        )
        .bind(item.reservation_id)
        .fetch_one(writer.connection())
        .await?;
        sqlx::query(
            "INSERT INTO public.consignment_custody_positions
                (id, item_id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid,
                 season_window_id, state, version, current_generation, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'awaiting_take', 1, NULL, $9)",
        )
        .bind(position_id)
        .bind(item_id)
        .bind(item.listing_id)
        .bind(item.seller_user_id.as_uuid())
        .bind(equipment_id)
        .bind(item.equipment.world_id())
        .bind(item.equipment.external_equipment_id())
        .bind(season_window_id)
        .bind(now)
        .execute(writer.connection())
        .await?;
        sqlx::query(
            "INSERT INTO public.consignment_custody_events
                (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
                 listing_id, listing_version, occurred_at)
             VALUES ($1, 1, $2, 'accepted', NULL, 'awaiting_take', NULL, $3, $4, $5)",
        )
        .bind(position_id)
        .bind(command_id)
        .bind(item.listing_id)
        .bind(
            i64::try_from(accepted_version)
                .ok()
                .ok_or_else(|| WriteError::invariant("consignment_item_version"))?,
        )
        .bind(now)
        .execute(writer.connection())
        .await?;
        transfer_to_position(
            writer.connection(),
            &item.equipment,
            item.reservation_id,
            position_id,
            now,
        )
        .await?;
        accepted.push(AcceptedDepositItem {
            ordinal: item.ordinal,
            listing_id: item.listing_id,
            reservation_id: item.reservation_id,
            position_id,
            listing_version: accepted_version,
        });
    }
    Ok(accepted)
}
