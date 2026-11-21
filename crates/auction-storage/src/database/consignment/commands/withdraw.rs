//! 卖家取消挂单，同时释放占用、撤销解析并擦除登记信封。

use auction_application::consignment::{
    ConsignmentError, ListingClosed, ListingCommandOutcome, ListingOperation,
    WithdrawListingCommand,
};
use auction_domain::consignment::ListingTrigger;

use super::facts::ChangedOperation;
use super::ledger::Actor;
use super::management::{Management, execute};
use super::receipts::closed;
use super::target::existing;
use crate::database::DatabaseStore;

/// 取消以当前活动版为前置，保留第一次已提交的关闭原因。不变量 1、2、3、5、8。
pub(super) async fn withdraw(
    store: &DatabaseStore,
    cmd: WithdrawListingCommand,
) -> Result<ListingCommandOutcome<ListingClosed>, ConsignmentError> {
    existing(store, cmd.target.command_id, cmd.target.listing_id, &cmd, |cmd, identity, tx| {
        Box::pin(execute(
            Management {
                actor: Actor::PublicUser(cmd.target.seller_user_id.as_uuid()),
                operation: ListingOperation::Withdraw,
                listing_id: cmd.target.listing_id,
                request_hash: cmd.request_hash,
                trigger: ListingTrigger::Withdraw { expected_version: cmd.target.expected_version },
                changed: ChangedOperation::Withdraw,
                enabled: true,
            },
            identity,
            tx,
            closed,
        ))
    })
    .await
}
