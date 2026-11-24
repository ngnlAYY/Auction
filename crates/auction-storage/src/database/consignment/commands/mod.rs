//! 寄售六类写命令及其会话仲裁、账本与原子事实写入。

mod admin_delist;
mod apply_lookup;
mod auto_close;
mod credentials;
mod equipment_lease;
pub(crate) mod errors;
pub(crate) mod facts;
pub(crate) mod ledger;
mod management;
mod receipts;
mod receipts_v2;
mod register;
mod reprice;
pub(crate) mod session;
mod snapshot;
mod target;
mod termination;
#[cfg(feature = "test-db")]
pub mod test_access_v2;
mod withdraw;
mod writer_v2;

#[cfg(feature = "test-db")]
mod pg_tests;

use auction_application::admin::EquipmentKeySealerPort;
use auction_application::consignment::{
    AdminDelistListingCommand, ApplyListingLookupCommand, AutoCloseListingCommand,
    ConsignmentFuture, ListingAutoCloseEffect, ListingClosed, ListingCommandOutcome,
    ListingCommandStore, ListingLookupEffect, ListingPriceReceipt, ListingRegistered,
    RegisterListingCommand, RegistrationReplayCrypto, RepriceListingCommand,
    WithdrawListingCommand,
};

use crate::database::DatabaseStore;

impl ListingCommandStore for DatabaseStore {
    fn register<'a>(
        &'a self,
        command: RegisterListingCommand<'a>,
        sealer: Option<&'a dyn EquipmentKeySealerPort>,
        replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
    ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
        Box::pin(register::register(self, command, sealer, replay_crypto))
    }

    fn reprice(
        &self,
        command: RepriceListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
        Box::pin(reprice::reprice(self, command))
    }

    fn withdraw(
        &self,
        command: WithdrawListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        Box::pin(withdraw::withdraw(self, command))
    }

    fn apply_lookup(
        &self,
        command: ApplyListingLookupCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
        Box::pin(apply_lookup::apply_lookup(self, command))
    }

    fn auto_close(
        &self,
        command: AutoCloseListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
        Box::pin(auto_close::auto_close(self, command))
    }

    fn admin_delist(
        &self,
        command: AdminDelistListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        Box::pin(admin_delist::admin_delist(self, command))
    }
}

pub(crate) use equipment_lease::EquipmentLease;
pub(crate) use receipts_v2::DepositReceipt;
pub(crate) use session::{LockKeys, with_deposit_transaction};
pub(crate) use writer_v2::{DepositRejection, DepositWriter};
