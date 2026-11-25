//! 软关闭后的只读命令重放。
//!
//! 关闭只截止新承诺；关闭前已经接受并落库的同一 `command_id` 仍必须返回首次回执
//! （不变量 1、D9 §6.3）。这里按命令 ID 读取账本、核对 actor/操作/目标/摘要并复用
//! 4K 的 v2 回执解码，不写任何行。

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositOperation, OperationsHoldReceipt, RelistReceipt,
    ReservationReceipt,
};
use auction_application::core::RequestHash;
use auction_domain::UserId;
use sqlx::Row;
use sqlx::postgres::PgRow;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::outcome::decode_rejection;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::DepositReceipt;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{
    DepositTarget, StoredReceipt, actor_reference,
};

/// 按原命令重放一条预约签发回执。
pub(super) async fn replay_issue_reservation(
    store: &DatabaseStore,
    seller_user_id: UserId,
    command_id: Uuid,
    listing_id: Uuid,
    request_hash: RequestHash,
) -> Result<Option<ReservationReceipt>, ConsignmentError> {
    if command_id.is_nil() {
        return Err(ConsignmentError::InvalidCommandId);
    }
    if listing_id.is_nil() {
        return Err(ConsignmentError::InvalidListingId);
    }
    replay(
        store,
        command_id,
        DepositActor::PublicUser(seller_user_id),
        DepositOperation::IssueReservation,
        DepositTarget::Listing(listing_id),
        request_hash,
    )
    .await
}

/// 按原命令重放一条 Admin 下架/止售回执。
pub(super) async fn replay_set_operations_hold(
    store: &DatabaseStore,
    admin_actor: &str,
    command_id: Uuid,
    listing_id: Uuid,
    request_hash: RequestHash,
) -> Result<Option<OperationsHoldReceipt>, ConsignmentError> {
    replay(
        store,
        command_id,
        DepositActor::Admin(admin_actor.to_owned()),
        DepositOperation::AdminDelist,
        DepositTarget::Listing(listing_id),
        request_hash,
    )
    .await
}

/// 按原命令重放一条卖家重列回执（7C-3）；首次是确定拒绝时返回同一拒绝。
pub(crate) async fn replay_relist(
    store: &DatabaseStore,
    seller_user_id: UserId,
    command_id: Uuid,
    listing_id: Uuid,
    request_hash: RequestHash,
) -> Result<Option<RelistReceipt>, ConsignmentError> {
    if command_id.is_nil() {
        return Err(ConsignmentError::InvalidCommandId);
    }
    if listing_id.is_nil() {
        return Err(ConsignmentError::InvalidListingId);
    }
    replay(
        store,
        command_id,
        DepositActor::PublicUser(seller_user_id),
        DepositOperation::Relist,
        DepositTarget::Listing(listing_id),
        request_hash,
    )
    .await
}

async fn replay<T: DepositReceipt>(
    store: &DatabaseStore,
    command_id: Uuid,
    actor: DepositActor,
    operation: DepositOperation,
    target: DepositTarget,
    request_hash: RequestHash,
) -> Result<Option<T>, ConsignmentError> {
    replay_inner(store, command_id, actor, operation, target, request_hash)
        .await
        .map_err(WriteError::into_application)
}

async fn replay_inner<T: DepositReceipt>(
    store: &DatabaseStore,
    command_id: Uuid,
    actor: DepositActor,
    operation: DepositOperation,
    target: DepositTarget,
    request_hash: RequestHash,
) -> Result<Option<T>, WriteError> {
    if !target.valid_for(operation) {
        return Err(WriteError::invariant("consignment_deposit_replay_target"));
    }
    let row = sqlx::query(
        "SELECT schema_version, actor_kind, actor_ref, operation, request_target,
                request_digest, target_kind, target_id, target_bot_uid, target_world_id,
                target_external_mid, target_season_id, target_calendar_version,
                result_status, result_code, result_body
         FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(command_id)
    .fetch_optional(store.pool())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let schema_version: i16 = row.try_get("schema_version")?;
    let digest: &[u8] = row.try_get("request_digest")?;
    let digest_matches =
        digest.len() == 32 && bool::from(digest.ct_eq(request_hash.as_bytes().as_slice()));
    if schema_version != 2
        || row.try_get::<&str, _>("actor_kind")? != actor.kind()
        || row.try_get::<&str, _>("actor_ref")? != actor_reference(&actor)
        || row.try_get::<&str, _>("operation")? != operation.as_str()
        || row.try_get::<&str, _>("request_target")? != target.request_target()
        || !same_target(&target, &row)?
        || !digest_matches
    {
        return Err(ConsignmentError::CommandIdConflict.into());
    }
    let stored = StoredReceipt {
        status: row.try_get("result_status")?,
        code: row.try_get("result_code")?,
        body: row.try_get("result_body")?,
    };
    if stored.body.get("disposition").and_then(serde_json::Value::as_str) == Some("rejected") {
        let code = stored.body.get("code").and_then(serde_json::Value::as_str).unwrap_or_default();
        let rejection = decode_rejection(code)
            .ok_or_else(|| WriteError::invariant("consignment_deposit_replay_rejection"))?;
        if i16::try_from(rejection.status()).ok() != Some(stored.status) {
            return Err(WriteError::invariant("consignment_deposit_replay_status"));
        }
        return Err(ConsignmentError::Deposit(rejection).into());
    }
    Ok(Some(T::decode(&stored)?))
}

fn same_target(target: &DepositTarget, row: &PgRow) -> Result<bool, WriteError> {
    Ok(match target {
        DepositTarget::Listing(id)
        | DepositTarget::Reservation(id)
        | DepositTarget::Quarantine(id)
        | DepositTarget::FinalDecision(id)
        | DepositTarget::OriginalAttempt(id)
        | DepositTarget::Position(id) => {
            row.try_get::<Option<&str>, _>("target_kind")? == Some(target.kind())
                && row.try_get::<Option<Uuid>, _>("target_id")? == Some(*id)
        }
        DepositTarget::MailKey(key) => {
            row.try_get::<Option<&str>, _>("target_kind")? == Some("mail_key")
                && row.try_get::<Option<&str>, _>("target_bot_uid")?
                    == Some(key.authenticated_bot_uid())
                && row.try_get::<Option<&str>, _>("target_world_id")?
                    == Some(auction_domain::consignment::settlement_world_code(key.world()))
                && row.try_get::<Option<&str>, _>("target_external_mid")?
                    == Some(key.external_mid())
        }
        DepositTarget::Season { season_id, calendar_version } => {
            row.try_get::<Option<&str>, _>("target_kind")? == Some("season")
                && row.try_get::<Option<&str>, _>("target_season_id")? == Some(season_id)
                && row.try_get::<Option<&[u8]>, _>("target_calendar_version")?
                    == Some(calendar_version.as_slice())
        }
    })
}
