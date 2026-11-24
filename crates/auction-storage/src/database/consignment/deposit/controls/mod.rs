//! 卖家撤回意愿与 Admin 运营止售的 PostgreSQL 写入。

mod children;
mod stages;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, CustodyControlStore, DepositActor, DepositOperation,
    OperationsHoldReceipt, ReleaseOperationsHoldCommand, SetOperationsHoldCommand,
    WithdrawDepositCommand, WithdrawDepositReceipt,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use uuid::Uuid;

use super::outcome::{FirstOutcome, PortOutcome, replay};
use super::primitives::pre_read_identity;
use super::repository::DepositRepository;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{DepositArbitration, DepositTarget};
use crate::database::consignment::commands::session::{LockKeys, with_deposit_transaction};
use stages::{
    hold_children, hold_stage, release_children, release_stage, withdrawal_children,
    withdrawal_stage,
};

impl CustodyControlStore for DepositRepository {
    fn request_withdrawal(
        &self,
        command: WithdrawDepositCommand,
    ) -> ConsignmentFuture<'_, WithdrawDepositReceipt> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            let identity = pre_read_identity(self.store(), command.listing_id)
                .await
                .map_err(WriteError::into_application)?;
            let sellers: Vec<_> = identity.iter().map(|id| id.seller_user_id).collect();
            let equipment: Vec<_> = identity.iter().map(|id| id.equipment.clone()).collect();
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
                                DepositActor::PublicUser(command.seller_user_id),
                                DepositOperation::Withdraw,
                                DepositTarget::Listing(command.listing_id),
                                command.request_hash,
                                command.request_id.as_deref(),
                            )
                            .await?
                        {
                            DepositArbitration::Replay(stored) => replay(&stored),
                            DepositArbitration::First(writer) => {
                                match withdrawal_stage(repository, command, writer).await? {
                                    FirstOutcome::Rejected(rejection) => {
                                        Ok(PortOutcome::Rejected(rejection))
                                    }
                                    FirstOutcome::Accepted { receipt, children } => {
                                        withdrawal_children(tx.connection(), children).await?;
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

    fn set_operations_hold(
        &self,
        command: SetOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, OperationsHoldReceipt> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            let identity = pre_read_identity(self.store(), command.listing_id)
                .await
                .map_err(WriteError::into_application)?;
            let sellers: Vec<_> = identity.iter().map(|id| id.seller_user_id).collect();
            let equipment: Vec<_> = identity.iter().map(|id| id.equipment.clone()).collect();
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
                                DepositActor::Admin(command.admin_actor.clone()),
                                DepositOperation::AdminDelist,
                                DepositTarget::Listing(command.listing_id),
                                command.request_hash,
                                command.request_id.as_deref(),
                            )
                            .await?
                        {
                            DepositArbitration::Replay(stored) => replay(&stored),
                            DepositArbitration::First(writer) => {
                                match hold_stage(repository, command, writer).await? {
                                    FirstOutcome::Rejected(rejection) => {
                                        Ok(PortOutcome::Rejected(rejection))
                                    }
                                    FirstOutcome::Accepted { receipt, children } => {
                                        hold_children(tx.connection(), children).await?;
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

    fn release_operations_hold(
        &self,
        command: ReleaseOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, OperationsHoldReceipt> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            let identity = pre_read_identity(self.store(), command.listing_id)
                .await
                .map_err(WriteError::into_application)?;
            let sellers: Vec<_> = identity.iter().map(|id| id.seller_user_id).collect();
            let equipment: Vec<_> = identity.iter().map(|id| id.equipment.clone()).collect();
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
                                DepositActor::Admin(command.admin_actor.clone()),
                                DepositOperation::ReleaseOperationsHold,
                                DepositTarget::Listing(command.listing_id),
                                command.request_hash,
                                command.request_id.as_deref(),
                            )
                            .await?
                        {
                            DepositArbitration::Replay(stored) => replay(&stored),
                            DepositArbitration::First(writer) => {
                                match release_stage(repository, command, writer).await? {
                                    FirstOutcome::Rejected(rejection) => {
                                        Ok(PortOutcome::Rejected(rejection))
                                    }
                                    FirstOutcome::Accepted { receipt, children } => {
                                        release_children(tx.connection(), children).await?;
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

    fn replay_set_operations_hold(
        &self,
        admin_actor: String,
        command_id: CommandId,
        listing_id: Uuid,
        request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<OperationsHoldReceipt>> {
        Box::pin(async move {
            if command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            if listing_id.is_nil() {
                return Err(ConsignmentError::InvalidListingId);
            }
            super::replay::replay_set_operations_hold(
                self.store(),
                &admin_actor,
                command_id.as_uuid(),
                listing_id,
                request_hash,
            )
            .await
        })
    }
}
