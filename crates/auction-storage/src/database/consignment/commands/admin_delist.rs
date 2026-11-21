//! Admin 下架挂单，私有说明只随成功审计持久化。

use auction_application::consignment::{
    AdminDelistListingCommand, ConsignmentError, ListingClosed, ListingCommandOutcome,
    ListingOperation,
};
use auction_domain::consignment::ListingTrigger;

use super::facts::ChangedOperation;
use super::ledger::Actor;
use super::management::{Management, execute};
use super::receipts::closed;
use super::target::existing;
use crate::database::DatabaseStore;

/// 只允许已仲裁的当前活动版下架。关闭、取消解析及私有审计同提交。不变量 1、3、5、7、8。
pub(super) async fn admin_delist(
    store: &DatabaseStore,
    cmd: AdminDelistListingCommand,
) -> Result<ListingCommandOutcome<ListingClosed>, ConsignmentError> {
    existing(store, cmd.command_id, cmd.listing_id, &cmd, |cmd, identity, tx| {
        Box::pin(execute(
            Management {
                actor: Actor::Admin(&cmd.admin_actor),
                operation: ListingOperation::AdminDelist,
                listing_id: cmd.listing_id,
                request_hash: cmd.request_hash,
                trigger: ListingTrigger::AdminDelist { expected_version: cmd.expected_version },
                changed: ChangedOperation::AdminDelist(&cmd.reason),
                enabled: true,
            },
            identity,
            tx,
            closed,
        ))
    })
    .await
}
