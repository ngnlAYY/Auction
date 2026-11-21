//! 卖家按锁内版本改价，保持截止与在途解析围栏。

use auction_application::consignment::{
    ConsignmentError, ListingCommandOutcome, ListingOperation, ListingPriceReceipt,
    RepriceListingCommand,
};
use auction_domain::consignment::ListingTrigger;

use super::facts::ChangedOperation;
use super::ledger::Actor;
use super::management::{Management, execute};
use super::receipts::repriced;
use super::target::existing;
use crate::database::DatabaseStore;

/// 相同价格返回原版快照，不创建版本事件。不变量 1、2、3、6、10。
pub(super) async fn reprice(
    store: &DatabaseStore,
    cmd: RepriceListingCommand,
) -> Result<ListingCommandOutcome<ListingPriceReceipt>, ConsignmentError> {
    existing(store, cmd.target.command_id, cmd.target.listing_id, &cmd, |cmd, identity, tx| {
        Box::pin(execute(
            Management {
                actor: Actor::PublicUser(cmd.target.seller_user_id.as_uuid()),
                operation: ListingOperation::Reprice,
                listing_id: cmd.target.listing_id,
                request_hash: cmd.request_hash,
                request_id: cmd.request_id.as_deref(),
                trigger: ListingTrigger::Reprice {
                    expected_version: cmd.target.expected_version,
                    price: cmd.price,
                },
                changed: ChangedOperation::Reprice,
                enabled: cmd.enabled,
            },
            identity,
            tx,
            repriced,
        ))
    })
    .await
}
