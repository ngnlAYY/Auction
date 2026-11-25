//! 季末 position 终态：关季事务内把季窗里的代管件写成 `season_ended` 并释放实物占用。

use auction_application::consignment::{SeasonCloseCommand, SeasonCloseTarget};
use uuid::Uuid;

use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;

/// 把该季窗内 `in_custody` 与 `reconcile` 的 position 终结为 `season_ended`。
///
/// 保护的不变量：3、6、8、10。position 更新、代管事件与占用释放同事务；季末等号即失效，
/// 调用方已拒绝未到季末的命令，000135 触发器再按事务时钟复核季窗已结束；终态事件无邮件证明，
/// 占用释放守卫改认“本事务的季末事件加已结束季窗”。`awaiting_take` 不在此终结，
/// 其未起写的 Take 工作由 `take_season_close` 收口；已终态的 position 不再匹配，重放不会重写。
pub(super) async fn end_season_positions(
    writer: &mut DepositWriter<'_, '_, '_>,
    command: &SeasonCloseCommand,
    window_id: Uuid,
    now: auction_time::Timestamp,
) -> Result<(), WriteError> {
    let (listing_scope, position_scope) = match command.target {
        SeasonCloseTarget::World(_) => (None, None),
        SeasonCloseTarget::Listing(listing_id) => (Some(listing_id), None),
        SeasonCloseTarget::Position(position_id) => (None, Some(position_id)),
    };
    let rows: Vec<(Uuid, Uuid, i64, String, i64)> = sqlx::query_as(
        "SELECT position.id, position.listing_id, position.version, position.state,
                listing.state_version
         FROM public.consignment_custody_positions position
         JOIN public.consignment_listings listing ON listing.id = position.listing_id
         WHERE position.season_window_id = $1
           AND position.state IN ('in_custody', 'reconcile')
           AND ($2::uuid IS NULL OR position.listing_id = $2)
           AND ($3::uuid IS NULL OR position.id = $3)
         ORDER BY position.id
         FOR UPDATE OF position",
    )
    .bind(window_id)
    .bind(listing_scope)
    .bind(position_scope)
    .fetch_all(writer.connection())
    .await?;
    let command_id = writer.command_id();
    for (position_id, listing_id, version, state, listing_version) in rows {
        let updated = sqlx::query(
            "UPDATE public.consignment_custody_positions
             SET state = 'season_ended', version = version + 1, updated_at = $3
             WHERE id = $1 AND version = $2",
        )
        .bind(position_id)
        .bind(version)
        .bind(now)
        .execute(writer.connection())
        .await?
        .rows_affected();
        if updated != 1 {
            return Err(WriteError::invariant("consignment_season_end_position"));
        }
        sqlx::query(
            "INSERT INTO public.consignment_custody_events
                (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
                 listing_id, listing_version, occurred_at)
             VALUES ($1, $2, $3, 'season_ended', $4, 'season_ended', NULL, $5, $6, $7)",
        )
        .bind(position_id)
        .bind(version + 1)
        .bind(command_id)
        .bind(state)
        .bind(listing_id)
        .bind(listing_version)
        .bind(now)
        .execute(writer.connection())
        .await?;
        let released = sqlx::query(
            "UPDATE public.consignment_equipment_claims
             SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
                 changed_at = $2, last_fact_id = $1
             WHERE position_id = $1",
        )
        .bind(position_id)
        .bind(now)
        .execute(writer.connection())
        .await?
        .rows_affected();
        if released != 1 {
            return Err(WriteError::invariant("consignment_season_end_claim"));
        }
    }
    Ok(())
}
