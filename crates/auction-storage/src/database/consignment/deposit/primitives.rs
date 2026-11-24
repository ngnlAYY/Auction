//! 4B 各端口共用的锁内事实读取与投影推进。
//!
//! 这里只放不携带业务决定的读取和纯投影构造；预约、整封接受、代管控制与季任务的
//! 具体事务分别留在自己的叶子。

use auction_domain::consignment::{
    ListingReason, ListingSnapshot, ListingState, ListingVersion, settlement_world_code,
};
use auction_domain::{SettlementWorld, UserId};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::{WriteError, invalid_projection};
use crate::database::consignment::eligibility::accounts::{
    SellerEligibility, lock_seller_eligibility,
};
use crate::database::consignment::eligibility::occupancy::effective_activity_count;
use crate::database::consignment::listings::projection::{
    ListingLockIdentity, LockedListing, lock_listings, pre_read_listing_identity,
};

/// 当前数据库墙钟。
pub(crate) async fn database_now(connection: &mut PgConnection) -> Result<Timestamp, WriteError> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(connection).await?)
}

/// 预读目标锁键；没有授权含义。
pub(crate) async fn pre_read_identity(
    store: &DatabaseStore,
    listing_id: Uuid,
) -> Result<Option<ListingLockIdentity>, WriteError> {
    pre_read_listing_identity(store, listing_id).await
}

/// 锁定一条挂单。
pub(crate) async fn lock_listing(
    connection: &mut PgConnection,
    listing_id: Uuid,
) -> Result<Option<LockedListing>, WriteError> {
    Ok(lock_listings(connection, &[listing_id]).await?.pop())
}

/// 同一状态、版本加一、价格与期限不变的投影。
pub(crate) fn next_version(snapshot: ListingSnapshot) -> Result<ListingSnapshot, WriteError> {
    let state_version = ListingVersion::new(
        snapshot
            .state_version
            .value()
            .checked_add(1)
            .ok_or_else(|| WriteError::invariant("consignment_listing_version_overflow"))?,
    )
    .map_err(invalid_projection)?;
    Ok(ListingSnapshot { state_version, ..snapshot })
}

/// 以固定价格、期限进入新状态或同态增版。
pub(crate) fn transition(
    snapshot: ListingSnapshot,
    state: ListingState,
    reason: Option<ListingReason>,
) -> Result<ListingSnapshot, WriteError> {
    let mut next = next_version(snapshot)?;
    next.state = state;
    next.reason = reason;
    Ok(next)
}

/// 世界收件就绪行。
pub(crate) struct MailReadiness {
    pub(crate) session_id: Uuid,
    pub(crate) bot_uid: String,
    pub(crate) allowlist_mode: String,
    pub(crate) allowed_senders: Vec<String>,
    pub(crate) detail_ready: bool,
    pub(crate) take_ready: bool,
    pub(crate) return_ready: bool,
    pub(crate) valid_until: Timestamp,
}

impl MailReadiness {
    /// 是否可按当前世界处理指定卖家的系统 RTS。
    pub(crate) fn accepts_sender(&self, seller_name: &str, now: Timestamp) -> bool {
        self.valid_until > now
            && self.detail_ready
            && self.return_ready
            && (self.allowlist_mode == "disabled"
                || self.allowed_senders.iter().any(|sender| sender == seller_name))
    }

    /// 公开状态使用的完整协议能力。
    pub(crate) fn fully_ready(&self, now: Timestamp) -> bool {
        self.valid_until > now && self.detail_ready && self.take_ready && self.return_ready
    }
}

/// 读取世界收件就绪行。
pub(crate) async fn mail_readiness(
    connection: &mut PgConnection,
    world: SettlementWorld,
) -> Result<Option<MailReadiness>, WriteError> {
    let Some(row) = sqlx::query(
        "SELECT r.session_id, r.allowlist_mode, r.allowed_senders,
                r.detail_ready, r.take_ready, r.return_ready, r.valid_until, s.bot_uid
         FROM public.consignment_mail_readiness r
         JOIN public.mail_evidence_sessions s ON s.id = r.session_id
         WHERE r.world_id = $1",
    )
    .bind(settlement_world_code(world))
    .fetch_optional(connection)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(MailReadiness {
        session_id: row.try_get("session_id")?,
        bot_uid: row.try_get("bot_uid")?,
        allowlist_mode: row.try_get("allowlist_mode")?,
        allowed_senders: row.try_get("allowed_senders")?,
        detail_ready: row.try_get("detail_ready")?,
        take_ready: row.try_get("take_ready")?,
        return_ready: row.try_get("return_ready")?,
        valid_until: row.try_get("valid_until")?,
    }))
}

/// 锁定卖家资格行。
pub(crate) async fn seller_eligibility(
    connection: &mut PgConnection,
    seller_user_id: UserId,
) -> Result<Option<SellerEligibility>, WriteError> {
    Ok(lock_seller_eligibility(connection, seller_user_id).await?)
}

/// 卖家当前资格是否允许新寄售业务。
pub(crate) fn eligible(seller: Option<&SellerEligibility>) -> bool {
    seller.is_some_and(|seller| seller.active && !seller.active_blacklist)
}

/// 卖家是否就是该世界的认证 Bot。
pub(crate) async fn bot_party(
    connection: &mut PgConnection,
    seller_user_id: UserId,
    bot_uid: &str,
) -> Result<bool, WriteError> {
    let game_player_id: Option<String> =
        sqlx::query_scalar("SELECT game_player_id FROM public.users WHERE id = $1")
            .bind(seller_user_id.as_uuid())
            .fetch_optional(connection)
            .await?
            .flatten();
    Ok(game_player_id.as_deref() == Some(bot_uid))
}

/// 卖家显示名；用于就绪 allowlist 与预约冻结的 verified sender。
pub(crate) async fn seller_name(
    connection: &mut PgConnection,
    seller_user_id: UserId,
) -> Result<Option<String>, WriteError> {
    Ok(sqlx::query_scalar("SELECT username FROM public.users WHERE id = $1")
        .bind(seller_user_id.as_uuid())
        .fetch_optional(connection)
        .await?)
}

/// 当前 active 预约或 position 持有的实物强占用。
pub(crate) async fn claim_holder(
    connection: &mut PgConnection,
    identity: &ListingLockIdentity,
) -> Result<Option<(Option<Uuid>, Option<Uuid>)>, WriteError> {
    let row = sqlx::query(
        "SELECT reservation_id, position_id FROM public.consignment_equipment_claims
         WHERE world_id = $1 AND canonical_eid = $2",
    )
    .bind(identity.equipment.world_id())
    .bind(identity.equipment.external_equipment_id())
    .fetch_optional(connection)
    .await?;
    Ok(row.map(|row| (row.get("reservation_id"), row.get("position_id"))))
}

/// 按运营止售闭集读取当前未解除的 hold。
pub(crate) async fn active_hold(
    connection: &mut PgConnection,
    position_id: Uuid,
) -> Result<Option<Uuid>, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT hold.id
         FROM public.consignment_custody_controls hold
         WHERE hold.position_id = $1 AND hold.kind = 'hold_set'
           AND NOT EXISTS (
               SELECT 1 FROM public.consignment_custody_controls release
               WHERE release.predecessor_id = hold.id)
         ORDER BY hold.version DESC LIMIT 1",
    )
    .bind(position_id)
    .fetch_optional(connection)
    .await?)
}

/// 当前未解除的撤回意愿。
pub(crate) async fn active_withdrawal_intent(
    connection: &mut PgConnection,
    position_id: Uuid,
) -> Result<Option<Uuid>, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM public.consignment_custody_controls
         WHERE position_id = $1 AND kind = 'withdrawal_intent'",
    )
    .bind(position_id)
    .fetch_optional(connection)
    .await?)
}

/// 当前 unpack 的寄售服务代次与接单状态。
pub(crate) struct ServiceControl {
    pub(crate) admitting: bool,
    pub(crate) enabled_epoch: i64,
    pub(crate) worker_ready_until: Option<Timestamp>,
}

/// 读取寄售解析单例。
pub(crate) async fn service_control(
    connection: &mut PgConnection,
) -> Result<Option<ServiceControl>, WriteError> {
    let row = sqlx::query(
        "SELECT admitting, enabled_epoch, worker_ready_until
         FROM public.consignment_lookup_control WHERE id = 1",
    )
    .fetch_optional(connection)
    .await?;
    Ok(row.map(|row| ServiceControl {
        admitting: row.get("admitting"),
        enabled_epoch: row.get("enabled_epoch"),
        worker_ready_until: row.get("worker_ready_until"),
    }))
}

/// 当前 isekai 季窗。
pub(crate) struct SeasonWindow {
    pub(crate) id: Uuid,
    pub(crate) start_utc: Timestamp,
    pub(crate) end_utc: Timestamp,
    pub(crate) admission_cutoff_utc: Timestamp,
}

/// 读取覆盖 `now` 的 isekai 季窗。
pub(crate) async fn current_season(
    connection: &mut PgConnection,
    now: Timestamp,
) -> Result<Option<SeasonWindow>, WriteError> {
    let row = sqlx::query(
        "SELECT id, start_utc, end_utc, admission_cutoff_utc
         FROM public.consignment_season_windows
         WHERE world_id = 'isekai' AND start_utc <= $1 AND end_utc > $1
         ORDER BY end_utc ASC, id ASC LIMIT 1",
    )
    .bind(now)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(SeasonWindow {
        id: row.try_get("id")?,
        start_utc: row.try_get("start_utc")?,
        end_utc: row.try_get("end_utc")?,
        admission_cutoff_utc: row.try_get("admission_cutoff_utc")?,
    }))
}

/// 由冻结界计算季态；界点无法判定时返回 `Unknown`。
pub(crate) fn season_state(
    world: SettlementWorld,
    window: Option<&SeasonWindow>,
    now: Timestamp,
) -> auction_domain::consignment::SeasonState {
    use auction_domain::consignment::SeasonState;
    if world == SettlementWorld::Main {
        return SeasonState::NotSeasonal;
    }
    let Some(window) = window else { return SeasonState::Unknown };
    if now >= window.end_utc {
        SeasonState::Closed
    } else if now >= window.admission_cutoff_utc {
        SeasonState::Preclose
    } else if now < window.start_utc {
        SeasonState::Unknown
    } else {
        SeasonState::Open
    }
}

/// 卖家当前活动额度是否还能容纳一件新的可售件。
pub(crate) async fn quota_available(
    connection: &mut PgConnection,
    seller_user_id: UserId,
    now: Timestamp,
    max_open_per_user: i64,
) -> Result<bool, WriteError> {
    let count = effective_activity_count(connection, seller_user_id, now).await?;
    Ok(count < max_open_per_user)
}

/// 在锁内读取挂单的当前投影。
pub(crate) fn current_snapshot(listing: &LockedListing) -> ListingSnapshot {
    listing.snapshot
}
