//! 现有挂单写事务的锁键准备、卖家锁与端口结果转换。

use auction_application::consignment::{
    ConsignmentError, ListingCommandOutcome, ListingCommandRejection, UnrecordedListingRejection,
};
use auction_domain::CommandId;
use auction_time::Timestamp;
use futures_util::future::BoxFuture;
use sqlx::PgConnection;
use uuid::Uuid;

use super::errors::WriteError;
use super::session::{CommandTransaction, LockKeys, with_command_transaction};
use crate::database::DatabaseStore;
use crate::database::consignment::eligibility::accounts::{SellerEligibility, lock_listing_seller};
use crate::database::consignment::listings::projection::{
    ListingLockIdentity, LockedListing, lock_listings, pre_read_listing_identity,
};

/// 不变量 1、2：预读只准备不可变锁键，即使没找到目标仍进行全局命令仲裁。
pub(super) async fn existing<T, C: Sync>(
    store: &DatabaseStore,
    command_id: CommandId,
    listing_id: Uuid,
    input: &C,
    operation: impl for<'tx> Fn(
        &'tx C,
        Option<&'tx ListingLockIdentity>,
        &'tx mut CommandTransaction<'_>,
    ) -> BoxFuture<'tx, Result<ListingCommandOutcome<T>, WriteError>>,
) -> Result<ListingCommandOutcome<T>, ConsignmentError> {
    if listing_id.is_nil() {
        return Err(ConsignmentError::InvalidListingId);
    }
    let identity =
        pre_read_listing_identity(store, listing_id).await.map_err(WriteError::into_application)?;
    let sellers: Vec<_> = identity.iter().map(|identity| identity.seller_user_id).collect();
    let equipment: Vec<_> = identity.iter().map(|identity| identity.equipment.clone()).collect();
    let keys =
        LockKeys { command_id: command_id.as_uuid(), sellers: &sellers, equipment: &equipment };
    result(
        with_command_transaction(store, keys, &(input, identity), |(input, identity), tx| {
            operation(*input, identity.as_ref(), tx)
        })
        .await,
    )
}

/// 转换不写账本的事务退出。必须先由事务 owner 确认回滚。
pub(super) fn result<T>(
    result: Result<ListingCommandOutcome<T>, WriteError>,
) -> Result<ListingCommandOutcome<T>, ConsignmentError> {
    let rejection = match result {
        Err(WriteError::Application(ConsignmentError::CommandIdConflict)) => {
            UnrecordedListingRejection::CommandIdConflict
        }
        Err(WriteError::Application(ConsignmentError::Disabled)) => {
            UnrecordedListingRejection::Disabled
        }
        Err(WriteError::Application(ConsignmentError::LeaseLost)) => {
            UnrecordedListingRejection::LeaseLost
        }
        Err(WriteError::Application(ConsignmentError::ConditionNotMet)) => {
            UnrecordedListingRejection::ConditionNotMet
        }
        other => return other.map_err(WriteError::into_application),
    };
    Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(rejection)))
}

/// 先取资格行锁，再允许调用方按 UUID 顺序锁挂单。不变量 2、6。
pub(super) async fn lock_seller(
    conn: &mut PgConnection,
    identity: Option<&ListingLockIdentity>,
    listing_id: Uuid,
) -> Result<Option<SellerEligibility>, WriteError> {
    if let Some(identity) = identity {
        let name = identity.seller_game_name.as_deref();
        return Ok(lock_listing_seller(conn, identity.seller_user_id, name).await?);
    }
    let appeared: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_listings WHERE id = $1)",
    )
    .bind(listing_id)
    .fetch_one(conn)
    .await?;
    if appeared {
        // 目标在锁键预读后出现，本次未持业务锁，需要原样重试。
        return Err(ConsignmentError::Unavailable.into());
    }
    Ok(None)
}

/// 读取并锁定目标投影。
pub(super) async fn lock_target(
    conn: &mut PgConnection,
    listing_id: Uuid,
) -> Result<Option<LockedListing>, WriteError> {
    Ok(lock_listings(conn, &[listing_id]).await?.pop())
}

/// 当前数据库墙钟，供锁内迁移与固定快照谓词使用。
pub(super) async fn now(conn: &mut PgConnection) -> Result<Timestamp, WriteError> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(conn).await?)
}
