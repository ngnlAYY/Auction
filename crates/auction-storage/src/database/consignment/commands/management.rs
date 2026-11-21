//! 改价、取消与 Admin 下架共用的授权、强制终结及版本顺序。

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingCommandOutcome, ListingOperation,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::{ListingTransition, ListingTrigger, transition_listing};
use uuid::Uuid;

use super::errors::WriteError;
use super::facts::{ChangedOperation, Decision};
use super::ledger::{Actor, Arbitration, RequestProof, StoredReceipt, Target};
use super::receipts::{reject, replay};
use super::session::CommandTransaction;
use super::target::{lock_seller, lock_target, now};
use super::termination::{close_resources, finish_forced_rejection, forced_facts};
use crate::database::consignment::listings::projection::ListingLockIdentity;
use crate::database::consignment::lookup::control::lock_control;

/// 外层动作的已认证身份及已经编码的命令语义。
pub(super) struct Management<'a> {
    pub(super) actor: Actor<'a>,
    pub(super) operation: ListingOperation,
    pub(super) listing_id: Uuid,
    pub(super) request_hash: RequestHash,
    pub(super) trigger: ListingTrigger,
    pub(super) changed: ChangedOperation<'a>,
    pub(super) enabled: bool,
}

/// 不变量 1、2、3、6、7、8：按回执、授权、改价准入、强制事实和版本顺序决定；不暴露他人的当前状态。
pub(super) async fn execute<T>(
    cmd: Management<'_>,
    identity: Option<&ListingLockIdentity>,
    tx: &mut CommandTransaction<'_>,
    decode: fn(&StoredReceipt) -> Result<T, WriteError>,
) -> Result<ListingCommandOutcome<T>, WriteError> {
    let mut writer = match tx
        .arbitrate(
            cmd.actor,
            cmd.operation,
            Target::Listing(cmd.listing_id),
            RequestProof::Digest(cmd.request_hash),
        )
        .await?
    {
        Arbitration::Replay(stored) => return replay(&stored, decode),
        Arbitration::First(writer) => writer,
    };
    let expected_seller = match cmd.actor {
        Actor::PublicUser(seller_user_id) => Some(seller_user_id),
        Actor::Admin(_) => None,
        Actor::Worker | Actor::System => {
            return Err(WriteError::invariant("consignment_management_actor"));
        }
    };
    let seller = lock_seller(writer.connection(), identity, cmd.listing_id).await?;
    // 不变量 2、6：事务外身份仅准备锁键。归属以锁住的挂单为准，先于任何状态或版本裁定。
    let current = lock_target(writer.connection(), cmd.listing_id).await?.filter(|listing| {
        expected_seller
            .is_none_or(|seller_user_id| seller_user_id == listing.identity.seller_user_id)
    });
    let Some(current) = current else {
        let now = now(writer.connection()).await?;
        return reject(writer, None, ListingBusinessRejection::ListingNotFound, now).await;
    };
    if !cmd.enabled {
        // 合同 §7.2：关闭配置下的新改价在完成归属检查后退出，由取消或扫描清退存量。
        return Err(ConsignmentError::Disabled.into());
    }
    let control = lock_control(writer.connection()).await?;
    let facts = forced_facts(writer.connection(), &current, seller.as_ref(), &control).await?;
    let now = now(writer.connection()).await?;
    let transition = transition_listing(Some(current.snapshot), cmd.trigger, now, facts);
    if let Ok(ListingTransition::ForcedTermination { listing, change }) = transition {
        return finish_forced_rejection(writer, &current, listing, change, now).await;
    }
    let decision = match transition.map_err(ConsignmentError::from) {
        Ok(ListingTransition::Unchanged(_)) => Decision::PriceUnchanged(&current),
        Ok(ListingTransition::Changed { listing, change }) => {
            writer
                .stage_change(&current, listing, current.current_snapshot_id, change, now)
                .await?;
            if !listing.state.is_active() {
                close_resources(writer.connection(), current.listing_id, now).await?;
            }
            Decision::Changed { listing_id: current.listing_id, operation: cmd.changed }
        }
        Ok(ListingTransition::ForcedTermination { .. }) => {
            return Err(WriteError::invariant("consignment_forced_management"));
        }
        Err(ConsignmentError::Business(reason)) => {
            return reject(writer, Some(current.listing_id), reason, now).await;
        }
        Err(error) => return Err(error.into()),
    };
    let receipt = writer.finish(decision, now).await?;
    Ok(ListingCommandOutcome::Accepted(decode(&receipt)?))
}
