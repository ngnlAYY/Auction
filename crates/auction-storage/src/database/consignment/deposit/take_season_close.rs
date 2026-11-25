//! 季末收口未起写的取得工作：关闭 Take work、取消其待发投递并追加 `season_closed` 处置，再把
//! `awaiting_take` 终结为 `season_ended`。

use auction_application::consignment::{SeasonCloseCommand, SeasonCloseTarget};
use auction_domain::{
    DeliveryDispositionActorSource, DeliveryDispositionKind, DeliveryDispositionSource,
};
use uuid::Uuid;

use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::delivery::disposition::{DeliveryDispositionFact, append};

/// 关闭季窗内“从未起写”的 `awaiting_take`：写入 `consignment_work_closures`，把仍待发的 Take 投递
/// 置为 `failed` 并推进围栏、在新围栏上追加 `season_closed` 处置，再把 position 终结为
/// `season_ended`、释放实物占用。
///
/// 保护的不变量：3、4、6、8、10。关闭记录、投递取消与处置、position 更新、季末事件与占用释放同事务；
/// 处置让这条从未外写的取消不再计入告警的未处置失败（维护者 11-24 批准，数据库守卫只放行此类投递）；
/// 季末等号即失效，调用方已拒绝未到季末的命令，000136 触发器再按事务时钟复核季窗，并要求本事务
/// 写入该工作的关闭记录且该工作没有起写事实。已起写的工作（起写后外部结果未知）不匹配本查询，
/// 保留给只读核对与对账，绝不自动终结（不变量 4）；投递不在 `prepared` / `claimed` 的（例如已转人工
/// 对账）同样保守保留。围栏推进使任何仍持租约的 Worker 无法再起写；已终态的 position 不再匹配，
/// 重放不会重复关闭。
pub(super) async fn close_unstarted_take_work(
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
    // 工作行只追加、运行账号无 UPDATE 权限，不加行锁；起写写端以投递行锁串行，这里同样锁投递。
    let rows: Vec<(Uuid, Uuid, Uuid, i64, Uuid, i64)> = sqlx::query_as(
        "SELECT position.id, position.listing_id, work.id, position.version, work.delivery_id,
                listing.state_version
         FROM public.consignment_custody_positions position
         JOIN public.consignment_listings listing ON listing.id = position.listing_id
         JOIN public.consignment_deposit_items item ON item.id = position.item_id
         JOIN public.mail_disposition_work work
           ON work.decision_id = item.decision_id AND work.action = 'take'
         JOIN public.deliveries delivery ON delivery.id = work.delivery_id
         WHERE position.season_window_id = $1
           AND position.state = 'awaiting_take'
           AND delivery.status IN ('prepared', 'claimed')
           AND NOT EXISTS (SELECT 1 FROM public.mail_disposition_write_starts started
                           WHERE started.work_id = work.id)
           AND ($2::uuid IS NULL OR position.listing_id = $2)
           AND ($3::uuid IS NULL OR position.id = $3)
         ORDER BY position.id
         FOR UPDATE OF position, delivery",
    )
    .bind(window_id)
    .bind(listing_scope)
    .bind(position_scope)
    .fetch_all(writer.connection())
    .await?;
    let command_id = writer.command_id();
    for (position_id, listing_id, work_id, version, delivery_id, listing_version) in rows {
        sqlx::query(
            "INSERT INTO public.consignment_work_closures (work_id, position_id, reason, closed_at)
             VALUES ($1, $2, 'season_ended', $3)",
        )
        .bind(work_id)
        .bind(position_id)
        .bind(now)
        .execute(writer.connection())
        .await?;
        // 取消待发投递：从未进入发送围栏，围栏加一使持租约的 Worker 无法再起写。
        let cancelled: Option<i64> = sqlx::query_scalar(
            "UPDATE public.deliveries
             SET status = 'failed', lease_token = NULL, lease_expires_at = NULL,
                 completed_at = GREATEST($2, created_at, claimed_at),
                 external_reference = NULL, failure_code = 'season_ended',
                 result_metadata = '{\"write_started\":false,\"outcome_unknown\":false,\"reason_code\":\"season_ended\"}'::jsonb,
                 fencing_version = fencing_version + 1,
                 updated_at = GREATEST($2, created_at, claimed_at)
             WHERE id = $1 AND status IN ('prepared', 'claimed') AND sending_at IS NULL
             RETURNING fencing_version",
        )
        .bind(delivery_id)
        .bind(now)
        .fetch_optional(writer.connection())
        .await?;
        let Some(fencing_version) = cancelled else {
            return Err(WriteError::invariant("consignment_season_take_delivery"));
        };
        // 自证处置落在取消后的当前 fence 上：数据库守卫要求本事务已写关闭记录且该工作从未起写。
        append(
            writer.tracked(),
            DeliveryDispositionFact {
                delivery_id,
                fencing_version,
                kind: DeliveryDispositionKind::SeasonClosed,
                source: DeliveryDispositionSource::default(),
                actor_source: DeliveryDispositionActorSource::Worker,
                actor_id: "consignment-worker",
                reason: None,
                reason_code: Some("season_ended"),
                detail: None,
                command_id: None,
                request_digest: None,
            },
        )
        .await?;
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
            return Err(WriteError::invariant("consignment_season_take_position"));
        }
        sqlx::query(
            "INSERT INTO public.consignment_custody_events
                (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
                 listing_id, listing_version, occurred_at)
             VALUES ($1, $2, $3, 'season_ended', 'awaiting_take', 'season_ended', NULL, $4, $5, $6)",
        )
        .bind(position_id)
        .bind(version + 1)
        .bind(command_id)
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
            return Err(WriteError::invariant("consignment_season_take_claim"));
        }
    }
    Ok(())
}
