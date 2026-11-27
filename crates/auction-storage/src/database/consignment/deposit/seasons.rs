//! 季前撤回通知与旧季关闭的 PostgreSQL 写入。

use std::collections::BTreeSet;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DepositActor, DepositBusinessRejection, DepositOperation,
    SeasonCloseCommand, SeasonCloseReceipt, SeasonCloseTarget, SeasonNoticeCommand,
    SeasonNoticeReceipt, SeasonStore,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::{ListingReason, ListingState};
use sqlx::Row;
use uuid::Uuid;

use super::outcome::{FirstOutcome, PortOutcome, conflict, replay};
use super::primitives::{database_now, transition};
use super::repository::DepositRepository;
use super::season_end::end_season_positions;
use super::take_season_close::close_unstarted_take_work;
use crate::database::consignment::commands::DepositRejection;
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{DepositArbitration, DepositTarget};
use crate::database::consignment::commands::session::{LockKeys, with_deposit_transaction};
use crate::database::consignment::listings::projection::{
    LockedListing, anonymous_seller_lock_key, lock_listings,
};

impl SeasonStore for DepositRepository {
    fn queue_season_notice(
        &self,
        command: SeasonNoticeCommand,
    ) -> ConsignmentFuture<'_, SeasonNoticeReceipt> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            let keys = LockKeys {
                command_id: command.command_id.as_uuid(),
                sellers: &[command.seller_user_id.as_uuid()],
                equipment: &[],
            };
            let input = (self, command);
            with_deposit_transaction(
                self.store(),
                keys,
                &[],
                &input,
                |(repository, command), tx| {
                    Box::pin(async move {
                        tx.ensure_lock_set(&[command.seller_user_id.as_uuid()], &[], &[])?;
                        match tx
                            .arbitrate(
                                DepositActor::Worker,
                                DepositOperation::SeasonNotice,
                                DepositTarget::Season {
                                    season_id: command.season.season_id.clone(),
                                    calendar_version: command.season.calendar_version.as_bytes(),
                                },
                                command.request_hash,
                                None,
                            )
                            .await?
                        {
                            DepositArbitration::Replay(stored) => replay(&stored),
                            DepositArbitration::First(writer) => {
                                match notice_stage(repository, command, writer).await? {
                                    FirstOutcome::Rejected(rejection) => {
                                        Ok(PortOutcome::Rejected(rejection))
                                    }
                                    FirstOutcome::Accepted { receipt, children } => {
                                        notice_children(tx.connection(), children).await?;
                                        Ok(PortOutcome::Accepted(receipt))
                                    }
                                }
                            }
                        }
                    })
                },
            )
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }

    fn close_season(
        &self,
        command: SeasonCloseCommand,
    ) -> ConsignmentFuture<'_, SeasonCloseReceipt> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            let (sellers, equipment) = close_lock_keys(self.store(), command.target)
                .await
                .map_err(WriteError::into_application)?;
            let input = (self, command, sellers, equipment);
            let keys = LockKeys {
                command_id: input.1.command_id.as_uuid(),
                sellers: &input.2,
                equipment: &input.3,
            };
            with_deposit_transaction(
                self.store(),
                keys,
                &[],
                &input,
                |(repository, command, sellers, equipment), tx| {
                    Box::pin(async move {
                        tx.ensure_lock_set(sellers, equipment, &[])?;
                        match tx
                            .arbitrate(
                                DepositActor::Worker,
                                DepositOperation::SeasonClose,
                                DepositTarget::Season {
                                    season_id: command.season.season_id.clone(),
                                    calendar_version: command.season.calendar_version.as_bytes(),
                                },
                                command.request_hash,
                                None,
                            )
                            .await?
                        {
                            DepositArbitration::Replay(stored) => replay(&stored),
                            DepositArbitration::First(mut writer) => {
                                let now = database_now(writer.connection()).await?;
                                match close_stage(repository, command, writer, now).await? {
                                    FirstOutcome::Rejected(rejection) => {
                                        Ok(PortOutcome::Rejected(rejection))
                                    }
                                    FirstOutcome::Accepted { receipt, .. } => {
                                        Ok(PortOutcome::Accepted(receipt))
                                    }
                                }
                            }
                        }
                    })
                },
            )
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }
}

struct SeasonWindow {
    id: Uuid,
    end_utc: auction_time::Timestamp,
}

struct NoticeChildren {
    notice_id: Uuid,
    window_id: Uuid,
    position_id: Uuid,
    seller_user_id: Uuid,
    kind: &'static str,
    command_id: Uuid,
    now: auction_time::Timestamp,
}

async fn notice_stage(
    _repository: &DepositRepository,
    command: &SeasonNoticeCommand,
    mut writer: DepositWriter<'_, '_, '_>,
) -> Result<FirstOutcome<SeasonNoticeReceipt, NoticeChildren>, WriteError> {
    let now = database_now(writer.connection()).await?;
    let Some(window) = window_for(writer.connection(), command).await? else {
        return reject(writer, DepositBusinessRejection::SourceMismatch, now).await;
    };
    let position = sqlx::query(
        "SELECT seller_user_id, season_window_id FROM public.consignment_custody_positions
         WHERE id = $1 FOR UPDATE",
    )
    .bind(command.position_id)
    .fetch_optional(writer.connection())
    .await?;
    let Some(position) = position else {
        return reject(writer, DepositBusinessRejection::SourceMismatch, now).await;
    };
    let seller: Uuid = position.try_get("seller_user_id")?;
    let season_window_id: Option<Uuid> = position.try_get("season_window_id")?;
    if seller != command.seller_user_id.as_uuid() || season_window_id != Some(window.id) {
        return reject(writer, DepositBusinessRejection::SourceMismatch, now).await;
    }
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.consignment_season_notices
         WHERE position_id = $1 AND seller_user_id = $2 AND kind = $3",
    )
    .bind(command.position_id)
    .bind(command.seller_user_id.as_uuid())
    .bind(command.notice_kind.as_str())
    .fetch_optional(writer.connection())
    .await?;
    let notice_id = existing.unwrap_or_else(Uuid::new_v4);
    let receipt = SeasonNoticeReceipt::queued(
        command.command_id,
        command.season.season_id.clone(),
        command.position_id,
        command.seller_user_id,
        notice_id,
    );
    writer.finish(receipt.clone(), now).await?;
    Ok(FirstOutcome::Accepted {
        receipt,
        children: NoticeChildren {
            notice_id,
            window_id: window.id,
            position_id: command.position_id,
            seller_user_id: command.seller_user_id.as_uuid(),
            kind: command.notice_kind.as_str(),
            command_id: command.command_id.as_uuid(),
            now,
        },
    })
}

async fn notice_children(
    connection: &mut sqlx::PgConnection,
    children: NoticeChildren,
) -> Result<(), WriteError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_season_notices WHERE id = $1)",
    )
    .bind(children.notice_id)
    .fetch_one(&mut *connection)
    .await?;
    if !exists {
        sqlx::query(
            "INSERT INTO public.consignment_season_notices
                (id, season_window_id, position_id, seller_user_id, kind, command_id,
                 created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(children.notice_id)
        .bind(children.window_id)
        .bind(children.position_id)
        .bind(children.seller_user_id)
        .bind(children.kind)
        .bind(children.command_id)
        .bind(children.now)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

async fn close_stage(
    _repository: &DepositRepository,
    command: &SeasonCloseCommand,
    mut writer: DepositWriter<'_, '_, '_>,
    now: auction_time::Timestamp,
) -> Result<FirstOutcome<SeasonCloseReceipt, ()>, WriteError> {
    let Some(window) = window_for_close(writer.connection(), command).await? else {
        return reject(writer, DepositBusinessRejection::SourceMismatch, now).await;
    };
    // 冻结尾界不因任务迟到而延长；未到界不得提前关闭。
    if now < window.end_utc {
        return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
    }
    let listing_ids = close_targets(writer.connection(), command, &window).await?;
    let listings: Vec<LockedListing> = if listing_ids.is_empty() {
        Vec::new()
    } else {
        lock_listings(writer.connection(), &listing_ids).await?
    };
    let mut closed = Vec::new();
    for listing in &listings {
        if !listing.snapshot.state.is_active() {
            continue;
        }
        let projection =
            transition(listing.snapshot, ListingState::Closed, Some(ListingReason::SeasonClosed))?;
        writer
            .stage_listing_update(
                listing,
                projection,
                listing.current_snapshot_id,
                "season_closed",
                Some("season_closed"),
                now,
            )
            .await?;
        closed.push(listing.listing_id);
    }
    end_season_positions(&mut writer, command, window.id, now).await?;
    close_unstarted_take_work(&mut writer, command, window.id, now).await?;
    let receipt = SeasonCloseReceipt::closed(
        command.command_id,
        command.season.season_id.clone(),
        command.target,
        closed,
    );
    writer.finish(receipt.clone(), now).await?;
    Ok(FirstOutcome::Accepted { receipt, children: () })
}

async fn close_lock_keys(
    store: &crate::database::DatabaseStore,
    target: SeasonCloseTarget,
) -> Result<(Vec<Uuid>, Vec<auction_domain::consignment::ListingEquipmentIdentity>), WriteError> {
    let rows: Vec<(Option<Uuid>, Option<String>, String, String)> = match target {
        SeasonCloseTarget::World(world) => {
            sqlx::query_as(
                "SELECT seller_user_id, seller_game_name, world_id, external_equipment_id
                 FROM public.consignment_listings
                 WHERE world_id = $1 AND (state IN (
                     'resolving', 'displayed', 'awaiting_deposit', 'deposit_accepted',
                     'available', 'held_off_market', 'withdrawal_pending')
                    OR EXISTS (SELECT 1 FROM public.consignment_custody_positions position
                               WHERE position.listing_id = consignment_listings.id
                                 AND position.state IN ('in_custody', 'reconcile')))
                 ORDER BY id",
            )
            .bind(auction_domain::consignment::settlement_world_code(world))
            .fetch_all(store.pool())
            .await?
        }
        SeasonCloseTarget::Listing(listing_id) => {
            sqlx::query_as(
                "SELECT seller_user_id, seller_game_name, world_id, external_equipment_id
                 FROM public.consignment_listings WHERE id = $1",
            )
            .bind(listing_id)
            .fetch_all(store.pool())
            .await?
        }
        SeasonCloseTarget::Position(position_id) => {
            sqlx::query_as(
                "SELECT listing.seller_user_id, listing.seller_game_name, listing.world_id,
                        listing.external_equipment_id
                 FROM public.consignment_custody_positions position
                 JOIN public.consignment_listings listing ON listing.id = position.listing_id
                 WHERE position.id = $1",
            )
            .bind(position_id)
            .fetch_all(store.pool())
            .await?
        }
    };
    let mut sellers = Vec::new();
    let mut equipment = Vec::new();
    for (seller_user_id, seller_game_name, world, eid) in rows {
        sellers.push(seller_lock_key(seller_user_id, seller_game_name)?);
        equipment.push(auction_domain::consignment::ListingEquipmentIdentity::new(&world, &eid)?);
    }
    Ok((sellers, equipment))
}

/// 卖家会话锁键：账户挂单用账户 ID，匿名非寄售挂单（000138）用与预读相同的派生锁键。
/// 卖家二选一的约束保证恰有一个；读到其他组合是投影损坏，失败关闭。
fn seller_lock_key(
    seller_user_id: Option<Uuid>,
    seller_game_name: Option<String>,
) -> Result<Uuid, WriteError> {
    match (seller_user_id, seller_game_name) {
        (Some(seller_user_id), None) => Ok(seller_user_id),
        (None, Some(name)) => Ok(anonymous_seller_lock_key(&name)),
        _ => Err(WriteError::invariant("consignment_season_close_seller")),
    }
}

async fn window_for(
    connection: &mut sqlx::PgConnection,
    command: &SeasonNoticeCommand,
) -> Result<Option<SeasonWindow>, WriteError> {
    window_matching(
        connection,
        command.season.world,
        &command.season.season_id,
        command.season.calendar_version.as_bytes(),
        command.season.start_utc,
        command.season.end_utc,
        command.season.admission_cutoff_utc,
    )
    .await
}

async fn window_for_close(
    connection: &mut sqlx::PgConnection,
    command: &SeasonCloseCommand,
) -> Result<Option<SeasonWindow>, WriteError> {
    window_matching(
        connection,
        command.season.world,
        &command.season.season_id,
        command.season.calendar_version.as_bytes(),
        command.season.start_utc,
        command.season.end_utc,
        command.season.admission_cutoff_utc,
    )
    .await
}

#[allow(clippy::too_many_arguments, reason = "冻结季窗逐项相等")]
async fn window_matching(
    connection: &mut sqlx::PgConnection,
    world: SettlementWorld,
    season_id: &str,
    calendar_version: [u8; 32],
    start_utc: auction_time::Timestamp,
    end_utc: auction_time::Timestamp,
    admission_cutoff_utc: auction_time::Timestamp,
) -> Result<Option<SeasonWindow>, WriteError> {
    if world != SettlementWorld::Isekai {
        return Ok(None);
    }
    let row = sqlx::query(
        "SELECT id, start_utc, end_utc, admission_cutoff_utc
         FROM public.consignment_season_windows
         WHERE world_id = 'isekai' AND season_id = $1 AND calendar_version = $2
           AND start_utc = $3 AND end_utc = $4 AND admission_cutoff_utc = $5",
    )
    .bind(season_id)
    .bind(calendar_version.as_slice())
    .bind(start_utc)
    .bind(end_utc)
    .bind(admission_cutoff_utc)
    .fetch_optional(connection)
    .await?;
    Ok(row.map(|row| SeasonWindow { id: row.get("id"), end_utc: row.get("end_utc") }))
}

async fn close_targets(
    connection: &mut sqlx::PgConnection,
    command: &SeasonCloseCommand,
    window: &SeasonWindow,
) -> Result<Vec<Uuid>, WriteError> {
    let ids: Vec<Uuid> = match command.target {
        SeasonCloseTarget::World(world) => {
            if world != SettlementWorld::Isekai {
                return Err(conflict(DepositBusinessRejection::SourceMismatch));
            }
            // 裁定：关上该 isekai 世界里登记时刻早于冻结季末的全部非终态挂单；
            // 登记于季末及以后的新季挂单不受迟跑任务影响。
            sqlx::query_scalar(
                "SELECT id FROM public.consignment_listings
                 WHERE world_id = 'isekai' AND created_at < $1
                   AND state IN (
                       'resolving', 'displayed', 'awaiting_deposit', 'deposit_accepted',
                       'available', 'held_off_market', 'withdrawal_pending')
                 ORDER BY id",
            )
            .bind(window.end_utc)
            .fetch_all(&mut *connection)
            .await?
        }
        SeasonCloseTarget::Listing(listing_id) => {
            let belongs: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM public.consignment_custody_positions
                    WHERE listing_id = $1 AND season_window_id = $2
                    UNION ALL
                    SELECT 1 FROM public.consignment_deposit_reservations
                    WHERE listing_id = $1 AND season_window_id = $2)",
            )
            .bind(listing_id)
            .bind(window.id)
            .fetch_one(&mut *connection)
            .await?;
            if belongs {
                vec![listing_id]
            } else {
                return Err(conflict(DepositBusinessRejection::SourceMismatch));
            }
        }
        SeasonCloseTarget::Position(position_id) => {
            let listing_id: Option<Uuid> = sqlx::query_scalar(
                "SELECT listing_id FROM public.consignment_custody_positions
                 WHERE id = $1 AND season_window_id = $2",
            )
            .bind(position_id)
            .bind(window.id)
            .fetch_optional(&mut *connection)
            .await?;
            let Some(listing_id) = listing_id else {
                return Err(conflict(DepositBusinessRejection::SourceMismatch));
            };
            vec![listing_id]
        }
    };
    Ok(ids.into_iter().collect::<BTreeSet<_>>().into_iter().collect())
}

#[cfg(feature = "test-db")]
impl DepositRepository {
    /// 在测试事务内用固定锁后时刻关闭季，用于冻结界的等号边界。
    pub async fn test_close_season_at(
        &self,
        command: SeasonCloseCommand,
        at: auction_time::Timestamp,
    ) -> Result<SeasonCloseReceipt, ConsignmentError> {
        let (sellers, equipment) = close_lock_keys(self.store(), command.target)
            .await
            .map_err(WriteError::into_application)?;
        let input = (self, command, sellers, equipment, at);
        let keys = LockKeys {
            command_id: input.1.command_id.as_uuid(),
            sellers: &input.2,
            equipment: &input.3,
        };
        with_deposit_transaction(
            self.store(),
            keys,
            &[],
            &input,
            |(repository, command, sellers, equipment, at), tx| {
                Box::pin(async move {
                    tx.ensure_lock_set(sellers, equipment, &[])?;
                    match tx
                        .arbitrate(
                            DepositActor::Worker,
                            DepositOperation::SeasonClose,
                            DepositTarget::Season {
                                season_id: command.season.season_id.clone(),
                                calendar_version: command.season.calendar_version.as_bytes(),
                            },
                            command.request_hash,
                            None,
                        )
                        .await?
                    {
                        DepositArbitration::Replay(stored) => replay(&stored),
                        DepositArbitration::First(writer) => {
                            match close_stage(repository, command, writer, *at).await? {
                                FirstOutcome::Rejected(rejection) => {
                                    Ok(PortOutcome::Rejected(rejection))
                                }
                                FirstOutcome::Accepted { receipt, .. } => {
                                    Ok(PortOutcome::Accepted(receipt))
                                }
                            }
                        }
                    }
                })
            },
        )
        .await
        .map_err(WriteError::into_application)?
        .into_result()
    }
}

async fn reject<T, C>(
    writer: DepositWriter<'_, '_, '_>,
    rejection: DepositBusinessRejection,
    now: auction_time::Timestamp,
) -> Result<FirstOutcome<T, C>, WriteError> {
    writer.finish_rejection(DepositRejection::Business(rejection), now).await?;
    Ok(FirstOutcome::Rejected(rejection))
}
