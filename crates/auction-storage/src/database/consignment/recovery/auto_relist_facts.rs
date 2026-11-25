//! 自动重列的锁内事实装配：把 position、上代 Sale 封闭、卖家资格、额度与服务状态读成领域的 `RelistFacts`。
//!
//! 保护不变量 2、6：判定与手动重列同一组前置条件，事实全部取自锁内读取，读不出的事实取失败关闭的值，
//! 由领域判定留库；只读，不写任何行。

use auction_application::consignment::AutoRelist;
use auction_domain::UserId;
use auction_domain::consignment::HeldOffMarketReason;
use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::recovery::{CurrentGeneration, RelistFacts, RelistMode};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::relist::DispositionRow;
use super::withdrawal::{WithdrawalPosition, current_generation, unknown_dispatch_open};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::{
    SeasonWindow, active_hold, quota_available, season_state, seller_eligibility, service_control,
};

/// 锁内读出的重列事实；读不出的事实取失败关闭的值，由领域判定留库。
pub(super) async fn relist_facts(
    connection: &mut PgConnection,
    command: &AutoRelist,
    disposition: &DispositionRow,
    listing: &crate::database::consignment::listings::projection::LockedListing,
    reason: Option<&str>,
    (position, sealed): (&WithdrawalPosition, bool),
    now: Timestamp,
) -> Result<RelistFacts, WriteError> {
    let world = auction_domain::consignment::parse_settlement_world(&position.world_id)
        .ok_or_else(|| WriteError::invariant("consignment_relist_world"))?;
    let window = season_window(connection, position.season_window_id).await?;
    let control = service_control(connection).await?;
    let seller =
        seller_eligibility(connection, UserId::from_uuid(disposition.seller_user_id)).await?;
    let open_blocks: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_cash_return_blocks AS block
             JOIN public.consignment_return_intakes AS intake ON intake.id = block.intake_id
             WHERE intake.contract_id = $1 AND block.state = 'open')",
    )
    .bind(disposition.contract_id)
    .fetch_one(&mut *connection)
    .await?;
    Ok(RelistFacts {
        listing_state: listing.snapshot.state,
        held_reason: reason.map(HeldOffMarketReason::parse).transpose()?,
        active_hold: active_hold(connection, disposition.position_id).await?.is_some(),
        relist: disposition.relist,
        seller_eligible: seller.is_some_and(|seller| {
            seller.active
                && !seller.active_blacklist
                && seller.blacklist_count == listing.blacklist_count_at_register
        }),
        quota_available: quota_available(
            connection,
            UserId::from_uuid(disposition.seller_user_id),
            now,
            command.max_open_per_user,
        )
        .await?,
        service_enabled: control.as_ref().is_some_and(|control| control.admitting),
        season_state: season_state(world, window.as_ref(), now),
        // 原件在库（in_custody），或 Sale 已按未付取消封闭、原件从未离库或已精确收回而仍记 committed
        // （重列写端先回库）；其余状态由调用方按暂态处理。
        original_in_custody: position.state == PositionState::InCustody
            || (position.state == PositionState::Committed && sealed),
        // 上代交割按处置封闭（处置关闭买方交割且 Sale 有未付取消封闭事实）、没有未收口的现金回收阻断，
        // 才算上代、退款与卖款全部封闭。
        predecessor_closed: disposition.closes_predecessor && sealed && !open_blocks,
        unknown_dispatch_open: open_blocks
            || unknown_dispatch_open(connection, disposition.position_id).await?,
        epoch_current: control
            .as_ref()
            .is_some_and(|control| control.enabled_epoch == listing.service_epoch),
        mode: RelistMode::Auto { enabled_frozen: disposition.auto_relist_enabled },
    })
}

/// 本处置的 Sale 是否是 position 当前代、已按未付取消封闭且原件在库或已精确收回。
pub(super) async fn unpaid_sale_sealed(
    connection: &mut PgConnection,
    position: &WithdrawalPosition,
    contract_id: Uuid,
) -> Result<bool, WriteError> {
    let CurrentGeneration::SaleSealed { predecessor_proof, .. } =
        current_generation(connection, position).await?
    else {
        return Ok(false);
    };
    if predecessor_proof != contract_id {
        return Ok(false);
    }
    let kind: String = sqlx::query_scalar(
        "SELECT closure_kind FROM public.consignment_sale_closures WHERE contract_id = $1",
    )
    .bind(contract_id)
    .fetch_one(connection)
    .await?;
    Ok(kind == "cancelled_unpaid")
}

async fn season_window(
    connection: &mut PgConnection,
    id: Option<Uuid>,
) -> Result<Option<SeasonWindow>, WriteError> {
    let Some(id) = id else { return Ok(None) };
    let row = sqlx::query(
        "SELECT start_utc, end_utc, admission_cutoff_utc
         FROM public.consignment_season_windows WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        Ok(SeasonWindow {
            id,
            start_utc: row.try_get("start_utc")?,
            end_utc: row.try_get("end_utc")?,
            admission_cutoff_utc: row.try_get("admission_cutoff_utc")?,
        })
    })
    .transpose()
}
