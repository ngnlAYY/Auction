//! 多挂单与锁行为的测试场景编排。

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositBusinessRejection, DepositOperation,
    EnvelopeDisposition, EnvelopeReceipt,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::consignment::{ListingState, ListingVersion, OriginalMailKey};
use uuid::Uuid;

use super::super::errors::{WriteError, invalid_projection};
use super::super::ledger::{DepositArbitration, DepositTarget};
use super::super::session::{LockKeys, with_deposit_transaction};
use super::primitives::{error_text, fixture_now};
use crate::database::DatabaseStore;
use crate::database::consignment::listings::projection::{
    lock_listings, pre_read_listing_identity,
};

impl DatabaseStore {
    /// 一条命令推进两个挂单；第二个版本冲突时整条命令回滚。
    pub async fn test_deposit_advance_two_listings(
        &self,
        command_id: Uuid,
        first: Uuid,
        second: Uuid,
        first_expected: u64,
        second_expected: u64,
    ) -> Result<(), String> {
        let first_identity = pre_read_listing_identity(self, first)
            .await
            .map_err(error_text)?
            .ok_or_else(|| "first listing missing".to_owned())?;
        let second_identity = pre_read_listing_identity(self, second)
            .await
            .map_err(error_text)?
            .ok_or_else(|| "second listing missing".to_owned())?;
        let sellers = vec![first_identity.seller_user_id, second_identity.seller_user_id];
        let equipment = vec![first_identity.equipment.clone(), second_identity.equipment.clone()];
        let mail_key = OriginalMailKey::new("12345", auction_domain::SettlementWorld::Main, "555")
            .map_err(|error| format!("fixture mail key: {error}"))?;
        let keys = LockKeys { command_id, sellers: &sellers, equipment: &equipment };
        let input = (first, second, first_expected, second_expected, mail_key);
        with_deposit_transaction(
            self,
            keys,
            std::slice::from_ref(&input.4),
            &input,
            |(first, second, first_expected, second_expected, mail_key), tx| {
                Box::pin(async move {
                    match tx
                        .arbitrate(
                            DepositActor::Worker,
                            DepositOperation::DecideEnvelope,
                            DepositTarget::MailKey(mail_key.clone()),
                            RequestHash::new([0x42; 32]),
                            None,
                        )
                        .await?
                    {
                        DepositArbitration::Replay(_) => Ok(()),
                        DepositArbitration::First(mut writer) => {
                            let locked =
                                lock_listings(writer.connection(), &[*first, *second]).await?;
                            let first_locked = locked
                                .iter()
                                .find(|listing| listing.listing_id == *first)
                                .ok_or_else(|| {
                                    WriteError::invariant("consignment_deposit_first_listing")
                                })?;
                            let second_locked = locked
                                .iter()
                                .find(|listing| listing.listing_id == *second)
                                .ok_or_else(|| {
                                    WriteError::invariant("consignment_deposit_second_listing")
                                })?;
                            if first_locked.snapshot.state_version.value() != *first_expected {
                                return Err(WriteError::from(ConsignmentError::from(
                                    DepositBusinessRejection::StaleVersion,
                                )));
                            }
                            let now = fixture_now()?;
                            writer
                                .stage_listing_update(
                                    first_locked,
                                    accepted_projection(first_locked)?,
                                    first_locked.current_snapshot_id,
                                    "deposit_accepted",
                                    None,
                                    now,
                                )
                                .await?;
                            if second_locked.snapshot.state_version.value() != *second_expected {
                                return Err(WriteError::from(ConsignmentError::from(
                                    DepositBusinessRejection::StaleVersion,
                                )));
                            }
                            writer
                                .stage_listing_update(
                                    second_locked,
                                    accepted_projection(second_locked)?,
                                    second_locked.current_snapshot_id,
                                    "deposit_accepted",
                                    None,
                                    now,
                                )
                                .await?;
                            let mut receipt = EnvelopeReceipt::new(
                                CommandId::from_uuid(writer.tx.command_id),
                                EnvelopeDisposition::Accepted,
                                Uuid::new_v5(&Uuid::NAMESPACE_URL, b"deposit-multi-listing"),
                            );
                            receipt.listing_changes = writer.listing_changes();
                            writer.finish(receipt, now).await?;
                            Ok(())
                        }
                    }
                })
            },
        )
        .await
        .map_err(error_text)
    }
}

fn accepted_projection(
    current: &crate::database::consignment::listings::projection::LockedListing,
) -> Result<auction_domain::consignment::ListingSnapshot, WriteError> {
    let version = current
        .snapshot
        .state_version
        .value()
        .checked_add(1)
        .ok_or_else(|| WriteError::invariant("consignment_deposit_version"))?;
    Ok(auction_domain::consignment::ListingSnapshot {
        state: ListingState::DepositAccepted,
        state_version: ListingVersion::new(version).map_err(invalid_projection)?,
        ..current.snapshot
    })
}
