//! 代管控制命令的锁内判定与子事实暂存。

use auction_application::consignment::{
    DepositBusinessRejection, DepositOperation, ListingStateChange, OperationsHoldDisposition,
    OperationsHoldReceipt, ReceiptDisposition, ReleaseOperationsHoldCommand,
    SetOperationsHoldCommand, WithdrawDepositCommand, WithdrawDepositReceipt, WithdrawDisposition,
};
use auction_domain::consignment::deposit::{
    WithdrawalIntentOutcome, decide_withdrawal_intent, operations_hold_reason,
    release_operations_hold_reason,
};
use auction_domain::consignment::{HeldOffMarketReason, ListingState};
use auction_domain::{CommandId, UserId};
use sqlx::Row;
use uuid::Uuid;

use super::super::outcome::{FirstOutcome, domain, domain_write};
use super::super::primitives::{active_hold, database_now, lock_listing, transition};
use super::super::repository::DepositRepository;
use super::children::{
    active_intent, insert_hold_controls, insert_intent_controls, listing_positions,
    release_hold_group,
};
use crate::database::consignment::commands::DepositReceipt;
use crate::database::consignment::commands::DepositRejection;
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::StoredReceipt;
use crate::database::consignment::eligibility::claims::cancel_active_reservations;
use crate::database::consignment::listings::projection::ListingLockIdentity;
use crate::database::consignment::recovery::withdrawal::{
    DirectWithdrawal, direct_withdrawal, position_in_custody,
};

/// 已存在意愿时不制造新版本的回执映射；其余字段与原回执一致。
struct UnchangedWithdraw(WithdrawDepositReceipt);

impl DepositReceipt for UnchangedWithdraw {
    fn operation(&self) -> DepositOperation {
        self.0.operation()
    }

    fn status(&self) -> i16 {
        self.0.status()
    }

    fn disposition(&self) -> ReceiptDisposition {
        self.0.disposition()
    }

    fn command_id(&self) -> CommandId {
        self.0.command_id()
    }

    fn body(&self) -> serde_json::Value {
        self.0.body()
    }

    fn listing_changes(&self) -> Vec<ListingStateChange> {
        Vec::new()
    }

    fn decode(_stored: &StoredReceipt) -> Result<Self, WriteError> {
        Err(WriteError::invariant("consignment_unchanged_withdraw_decode"))
    }
}

pub(super) enum WithdrawalChildren {
    Intent {
        listing_id: Uuid,
        primary: Option<Uuid>,
        command_id: Uuid,
        actor_ref: String,
        now: auction_time::Timestamp,
    },
    Close {
        listing_id: Uuid,
        command_id: Uuid,
        now: auction_time::Timestamp,
    },
    /// 直接撤回：合同、挂单与回执已在阶段内同事务写完，没有后续子事实。
    Direct,
}

/// 撤回人必须是挂单卖家；匿名挂单（000138）只认自填名与账户名 lower 等值的本站账户，
/// 数据库关闭事件门以同一口径复核（不变量 2、6）。
async fn withdrawer_owns(
    connection: &mut sqlx::PgConnection,
    identity: &ListingLockIdentity,
    withdrawer: UserId,
) -> Result<bool, WriteError> {
    let Some(name) = identity.seller_game_name.as_deref() else {
        return Ok(identity.seller_user_id == withdrawer.as_uuid());
    };
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.users WHERE id = $1 AND lower(username) = lower($2))",
    )
    .bind(withdrawer.as_uuid())
    .bind(name)
    .fetch_one(connection)
    .await?)
}

pub(super) async fn withdrawal_stage(
    repository: &DepositRepository,
    command: &WithdrawDepositCommand,
    mut writer: DepositWriter<'_, '_, '_>,
) -> Result<FirstOutcome<WithdrawDepositReceipt, WithdrawalChildren>, WriteError> {
    let now = database_now(writer.connection()).await?;
    let Some(current) = lock_listing(writer.connection(), command.listing_id).await? else {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    };
    if !withdrawer_owns(writer.connection(), &current.identity, command.seller_user_id).await? {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    }
    if current.snapshot.state_version != command.expected_version {
        return reject(writer, DepositBusinessRejection::StaleVersion, now).await;
    }
    match decide_withdrawal_intent(current.snapshot.state) {
        Ok(outcome) => {
            let existing = active_intent(writer.connection(), command.listing_id).await?;
            // 不变量 2、6：已在库的件不再裸转 `withdrawal_pending`，一律走直接撤回绑定 Withdrawal；
            // 取得事务里已兑现的意愿（`AlreadyPending` 且有意愿）仍按原重放处理，
            // 直接撤回已接受而无意愿的件重复撤回则拒绝，不制造第二份意愿或空版本。
            if position_in_custody(writer.connection(), command.listing_id).await? {
                match outcome {
                    WithdrawalIntentOutcome::AlreadyPending if existing.is_none() => {
                        return reject(writer, DepositBusinessRejection::ActionNotAllowed, now)
                            .await;
                    }
                    WithdrawalIntentOutcome::AlreadyPending => {}
                    _ => {
                        let content = repository.dispatch_content();
                        return match direct_withdrawal(writer, &current, command, now, content)
                            .await?
                        {
                            DirectWithdrawal::Accepted(receipt) => Ok(FirstOutcome::Accepted {
                                receipt,
                                children: WithdrawalChildren::Direct,
                            }),
                            DirectWithdrawal::Rejected(rejection) => {
                                Ok(FirstOutcome::Rejected(rejection))
                            }
                        };
                    }
                }
            } else if outcome == WithdrawalIntentOutcome::MoveToWithdrawalPending {
                // 已上架的挂单其 position 必在代管中；否则代管状态未证明，不裸转。
                return reject(writer, DepositBusinessRejection::CustodyUnproven, now).await;
            }
            let intent_id = existing.unwrap_or_else(Uuid::new_v4);
            let target_state = match outcome {
                WithdrawalIntentOutcome::MoveToWithdrawalPending => ListingState::WithdrawalPending,
                WithdrawalIntentOutcome::RemainDepositAccepted => ListingState::DepositAccepted,
                WithdrawalIntentOutcome::RemainHeldOffMarket => ListingState::HeldOffMarket,
                WithdrawalIntentOutcome::AlreadyPending => current.snapshot.state,
            };
            let (listing_version, state) = if existing.is_some() {
                // 已存在的撤回意愿不再新增版本或事件；否则会伪造第二次状态迁移。
                (current.snapshot.state_version, current.snapshot.state)
            } else {
                let reason_code: Option<&'static str> =
                    if target_state == ListingState::HeldOffMarket {
                        let raw: Option<String> = sqlx::query_scalar(
                            "SELECT reason_code FROM public.consignment_listings WHERE id = $1",
                        )
                        .bind(command.listing_id)
                        .fetch_one(writer.connection())
                        .await?;
                        raw.as_deref()
                            .map(HeldOffMarketReason::parse)
                            .transpose()?
                            .map(HeldOffMarketReason::as_str)
                    } else {
                        None
                    };
                let projection = transition(current.snapshot, target_state, None)?;
                writer
                    .stage_listing_update(
                        &current,
                        projection,
                        current.current_snapshot_id,
                        "withdrawal_requested",
                        reason_code,
                        now,
                    )
                    .await?;
                (projection.state_version, projection.state)
            };
            let receipt = WithdrawDepositReceipt::new(
                command.command_id,
                WithdrawDisposition::WithdrawalRequested,
                command.listing_id,
                listing_version,
                state,
                Some(intent_id),
            );
            if existing.is_some() {
                writer.finish(UnchangedWithdraw(receipt.clone()), now).await?;
            } else {
                writer.finish(receipt.clone(), now).await?;
            }
            Ok(FirstOutcome::Accepted {
                receipt,
                children: WithdrawalChildren::Intent {
                    listing_id: command.listing_id,
                    primary: existing.is_none().then_some(intent_id),
                    command_id: command.command_id.as_uuid(),
                    actor_ref: command.seller_user_id.as_uuid().to_string(),
                    now,
                },
            })
        }
        Err(error) if domain(error) == DepositBusinessRejection::ActionNotAllowed => {
            if !matches!(
                current.snapshot.state,
                ListingState::Resolving | ListingState::Displayed | ListingState::AwaitingDeposit
            ) {
                return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
            }
            let projection = transition(
                current.snapshot,
                ListingState::Closed,
                Some(auction_domain::consignment::ListingReason::SellerCancelled),
            )?;
            writer
                .stage_listing_update(
                    &current,
                    projection,
                    current.current_snapshot_id,
                    "closed",
                    Some("seller_cancelled"),
                    now,
                )
                .await?;
            let receipt = WithdrawDepositReceipt::new(
                command.command_id,
                WithdrawDisposition::Closed,
                command.listing_id,
                projection.state_version,
                ListingState::Closed,
                None,
            );
            writer.finish(receipt.clone(), now).await?;
            Ok(FirstOutcome::Accepted {
                receipt,
                children: WithdrawalChildren::Close {
                    listing_id: command.listing_id,
                    command_id: command.command_id.as_uuid(),
                    now,
                },
            })
        }
        Err(error) => reject(writer, domain(error), now).await,
    }
}

pub(super) async fn withdrawal_children(
    connection: &mut sqlx::PgConnection,
    children: WithdrawalChildren,
) -> Result<(), WriteError> {
    match children {
        WithdrawalChildren::Intent { listing_id, primary, command_id, actor_ref, now } => {
            insert_intent_controls(connection, listing_id, primary, command_id, &actor_ref, now)
                .await
        }
        WithdrawalChildren::Close { listing_id, command_id, now } => {
            cancel_active_reservations(connection, listing_id, command_id, now).await
        }
        WithdrawalChildren::Direct => Ok(()),
    }
}

pub(super) enum HoldChildren {
    Hold {
        positions: Vec<Uuid>,
        hold_id: Uuid,
        command_id: Uuid,
        actor_ref: String,
        now: auction_time::Timestamp,
    },
    Close {
        listing_id: Uuid,
        command_id: Uuid,
        now: auction_time::Timestamp,
    },
}

pub(super) async fn hold_stage(
    _repository: &DepositRepository,
    command: &SetOperationsHoldCommand,
    mut writer: DepositWriter<'_, '_, '_>,
) -> Result<FirstOutcome<OperationsHoldReceipt, HoldChildren>, WriteError> {
    let now = database_now(writer.connection()).await?;
    let Some(current) = lock_listing(writer.connection(), command.listing_id).await? else {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    };
    if current.snapshot.state_version != command.expected_version {
        return reject(writer, DepositBusinessRejection::StaleVersion, now).await;
    }
    match current.snapshot.state {
        ListingState::Resolving | ListingState::Displayed | ListingState::AwaitingDeposit => {
            let projection = transition(
                current.snapshot,
                ListingState::Closed,
                Some(auction_domain::consignment::ListingReason::AdminDelisted),
            )?;
            writer
                .stage_listing_update(
                    &current,
                    projection,
                    current.current_snapshot_id,
                    "closed",
                    Some("admin_delisted"),
                    now,
                )
                .await?;
            let receipt = OperationsHoldReceipt::new(
                command.command_id,
                OperationsHoldDisposition::Closed,
                command.listing_id,
                projection.state_version,
                ListingState::Closed,
                None,
                None,
            )
            .map_err(domain_write)?;
            // 私有运营说明只进审计：Admin v2 读模型要求 reason_code=admin_delisted 的挂单
            // 能连到带 `admin_delist_reason` 的成功审计，否则整条记录判为不可用。
            writer.set_private_audit_note("admin_delist_reason", command.reason.as_str());
            writer.finish(receipt.clone(), now).await?;
            Ok(FirstOutcome::Accepted {
                receipt,
                children: HoldChildren::Close {
                    listing_id: command.listing_id,
                    command_id: command.command_id.as_uuid(),
                    now,
                },
            })
        }
        ListingState::DepositAccepted | ListingState::Available | ListingState::HeldOffMarket => {
            let positions = listing_positions(writer.connection(), command.listing_id).await?;
            if positions.is_empty() {
                return Err(WriteError::invariant("consignment_hold_position_missing"));
            }
            for &position_id in &positions {
                if active_hold(writer.connection(), position_id).await?.is_some() {
                    return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
                }
            }
            let reason = operations_hold_reason(current.snapshot.state).map_err(domain_write)?;
            let projection = transition(current.snapshot, ListingState::HeldOffMarket, None)?;
            writer
                .stage_listing_update(
                    &current,
                    projection,
                    current.current_snapshot_id,
                    "hold_set",
                    Some(reason.as_str()),
                    now,
                )
                .await?;
            let hold_id = Uuid::new_v4();
            let receipt = OperationsHoldReceipt::new(
                command.command_id,
                OperationsHoldDisposition::HoldSet,
                command.listing_id,
                projection.state_version,
                ListingState::HeldOffMarket,
                Some(hold_id),
                Some(reason),
            )
            .map_err(domain_write)?;
            writer.finish(receipt.clone(), now).await?;
            Ok(FirstOutcome::Accepted {
                receipt,
                children: HoldChildren::Hold {
                    positions,
                    hold_id,
                    command_id: command.command_id.as_uuid(),
                    actor_ref: command.admin_actor.clone(),
                    now,
                },
            })
        }
        _ => reject(writer, DepositBusinessRejection::ListingNotActive, now).await,
    }
}

pub(super) async fn hold_children(
    connection: &mut sqlx::PgConnection,
    children: HoldChildren,
) -> Result<(), WriteError> {
    match children {
        HoldChildren::Hold { positions, hold_id, command_id, actor_ref, now } => {
            insert_hold_controls(connection, &positions, hold_id, command_id, &actor_ref, now).await
        }
        HoldChildren::Close { listing_id, command_id, now } => {
            cancel_active_reservations(connection, listing_id, command_id, now).await
        }
    }
}

pub(super) struct ReleaseChildren {
    listing_id: Uuid,
    hold_command_id: Uuid,
    command_id: Uuid,
    actor_ref: String,
    now: auction_time::Timestamp,
}

pub(super) async fn release_stage(
    _repository: &DepositRepository,
    command: &ReleaseOperationsHoldCommand,
    mut writer: DepositWriter<'_, '_, '_>,
) -> Result<FirstOutcome<OperationsHoldReceipt, ReleaseChildren>, WriteError> {
    let now = database_now(writer.connection()).await?;
    let Some(current) = lock_listing(writer.connection(), command.listing_id).await? else {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    };
    if current.snapshot.state_version != command.expected_version {
        return reject(writer, DepositBusinessRejection::StaleVersion, now).await;
    }
    // 按命令携带的止售 UUID 选中该组的那一行：`version` 是逐 position 计数，
    // 多 position 的组不能用它挑出主行（主行 ID 是 `insert_hold_controls` 写在首个 position 上的）。
    let hold = sqlx::query(
        "SELECT hold.id, hold.command_id
         FROM public.consignment_custody_controls hold
         JOIN public.consignment_custody_positions position ON position.id = hold.position_id
         WHERE hold.kind = 'hold_set' AND position.listing_id = $1 AND hold.id = $2
           AND NOT EXISTS (
               SELECT 1 FROM public.consignment_custody_controls release
               WHERE release.predecessor_id = hold.id)",
    )
    .bind(command.listing_id)
    .bind(command.hold_id)
    .fetch_optional(writer.connection())
    .await?;
    let Some(hold) = hold else {
        return reject(writer, DepositBusinessRejection::HoldMismatch, now).await;
    };
    let current_hold_id: Uuid = hold.try_get("id")?;
    let hold_command_id: Uuid = hold.try_get("command_id")?;
    let reason = release_operations_hold_reason(
        current.snapshot.state,
        Some(current_hold_id),
        command.hold_id,
    )
    .map_err(domain_write)?;
    let projection = transition(current.snapshot, ListingState::HeldOffMarket, None)?;
    writer
        .stage_listing_update(
            &current,
            projection,
            current.current_snapshot_id,
            "hold_released",
            Some(reason.as_str()),
            now,
        )
        .await?;
    let receipt = OperationsHoldReceipt::new(
        command.command_id,
        OperationsHoldDisposition::HoldReleased,
        command.listing_id,
        projection.state_version,
        ListingState::HeldOffMarket,
        Some(command.hold_id),
        Some(reason),
    )
    .map_err(domain_write)?;
    writer.finish(receipt.clone(), now).await?;
    Ok(FirstOutcome::Accepted {
        receipt,
        children: ReleaseChildren {
            listing_id: command.listing_id,
            hold_command_id,
            command_id: command.command_id.as_uuid(),
            actor_ref: command.admin_actor.clone(),
            now,
        },
    })
}

pub(super) async fn release_children(
    connection: &mut sqlx::PgConnection,
    children: ReleaseChildren,
) -> Result<(), WriteError> {
    release_hold_group(
        connection,
        children.listing_id,
        children.hold_command_id,
        children.command_id,
        &children.actor_ref,
        children.now,
    )
    .await
}

async fn reject<T, C>(
    writer: DepositWriter<'_, '_, '_>,
    rejection: DepositBusinessRejection,
    now: auction_time::Timestamp,
) -> Result<FirstOutcome<T, C>, WriteError> {
    writer.finish_rejection(DepositRejection::Business(rejection), now).await?;
    Ok(FirstOutcome::Rejected(rejection))
}
