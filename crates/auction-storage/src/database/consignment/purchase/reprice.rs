//! v2 改价（R5-C11）：只允许 `available` 且尚无 Sale 的挂单，其余一律 `equipment_committed` 等确定拒绝。
//!
//! 保护不变量 1、2、3、6、10：同命令 ID 同请求回原回执；判定与写入在同一 Serializable 事务内，
//! 持卖家用户与规范实物会话锁、挂单与 position 行锁，所以与购买、止售、撤回交换提交顺序只得
//! 其一；价格变更与版本、事件、回执同事务。已冻结的 Sale 价格不受改价影响。

use auction_application::consignment::{
    ConsignmentError, ListingPriceDisposition, RepriceV2Command, RepriceV2Outcome, RepriceV2Receipt,
};
use auction_domain::UserId;
use auction_domain::consignment::purchase::{
    PurchasePosition, RepriceDecision, RepriceFacts, RepriceRejection, decide_reprice_available,
};
use auction_domain::consignment::{ListingPrice, ListingVersion};
use auction_time::Timestamp;
use serde_json::{Value, json};
use uuid::Uuid;

use super::facts::{generation_facts, lock_listing_row, lock_position};
use super::ledger::{
    Arbitration, CommandIdentity, Operation, StoredReceipt, arbitrate, insert_receipt,
    rejection_receipt,
};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::deposit::primitives::database_now;

/// 改价事务的输入：命令与预读的挂单卖家。
pub(super) struct RepriceCall<'a> {
    pub(super) command: &'a RepriceV2Command,
    pub(super) pre_read_seller: Option<Uuid>,
}

/// 一个改价事务（一次重试尝试）。
pub(super) async fn reprice_in_tx(
    call: &RepriceCall<'_>,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<RepriceV2Outcome, WriteError> {
    let command = call.command;
    let input = command.input;
    let identity = CommandIdentity {
        command_id: command.command_id.as_uuid(),
        user_id: command.seller_user_id.as_uuid(),
        operation: Operation::Reprice,
        listing_id: input.listing_id,
        digest: command.request_hash,
        request_id: command.request_id.as_deref(),
    };
    match arbitrate(tx.connection(), &identity).await? {
        Arbitration::Replay(stored) => return decode_replay(&stored, command),
        Arbitration::First => {}
    }
    let now = database_now(tx.connection()).await?;
    let Some(listing) = lock_listing_row(tx.connection(), input.listing_id).await? else {
        // 目标不存在：没有可挂靠回执的挂单，确定拒绝不入账；预读后才出现则要求原样重试。
        return match call.pre_read_seller {
            None => Ok(RepriceV2Outcome::Rejected(RepriceRejection::ListingNotFound)),
            Some(_) => Err(ConsignmentError::Unavailable.into()),
        };
    };
    if call.pre_read_seller != Some(listing.seller_user_id) {
        return Err(ConsignmentError::Unavailable.into());
    }
    let position_row = lock_position(tx.connection(), input.listing_id).await?;
    let (position, sale_exists) = match &position_row {
        Some(row) => {
            let facts = generation_facts(tx.connection(), row.position_id, row.generation).await?;
            (
                Some(PurchasePosition {
                    position_id: row.position_id,
                    state: row.state,
                    generation: row.generation,
                    generation_consumed: facts.consumed,
                    take_proven: facts.take_proven,
                }),
                facts.consumed || facts.other_contract,
            )
        }
        None => (None, false),
    };
    let facts = RepriceFacts {
        seller_user_id: UserId::from_uuid(listing.seller_user_id),
        listing: listing.listing,
        position,
        sale_exists,
    };
    match decide_reprice_available(&input, &facts) {
        Err(rejection) => {
            let status = if rejection == RepriceRejection::ListingNotFound { 404 } else { 409 };
            let receipt = rejection_receipt(&identity, status, rejection.code())?;
            insert_receipt(tx.connection(), &identity, &receipt, now).await?;
            Ok(RepriceV2Outcome::Rejected(rejection))
        }
        Ok(RepriceDecision::Unchanged) => {
            let receipt =
                success_receipt(&identity, "unchanged", listing.listing.version, input.price);
            insert_receipt(tx.connection(), &identity, &receipt, now).await?;
            Ok(accepted(
                command,
                ListingPriceDisposition::Unchanged,
                listing.listing.version,
                input.price,
                false,
            ))
        }
        Ok(RepriceDecision::Changed { price, listing_version_after, .. }) => {
            let receipt = success_receipt(&identity, "repriced", listing_version_after, price);
            insert_receipt(tx.connection(), &identity, &receipt, now).await?;
            stage_repriced(tx, &listing.listing, listing_version_after, price, command, now)
                .await?;
            Ok(accepted(
                command,
                ListingPriceDisposition::Repriced,
                listing_version_after,
                price,
                false,
            ))
        }
    }
}

/// 价格与版本更新及同版本 `repriced` 事件；快照与期限不变，状态保持 `available`。
async fn stage_repriced(
    tx: &mut DepositTransaction<'_, '_>,
    listing: &auction_domain::consignment::purchase::PurchaseListing,
    version_after: ListingVersion,
    price: ListingPrice,
    command: &RepriceV2Command,
    now: Timestamp,
) -> Result<(), WriteError> {
    let version = i64::try_from(version_after.value())?;
    sqlx::query(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ($1, $2, $3, 'public_user', 'repriced', 'available', 'available', NULL, $4, $5)",
    )
    .bind(listing.listing_id)
    .bind(version)
    .bind(command.command_id.as_uuid())
    .bind(price.value())
    .bind(now)
    .execute(tx.connection())
    .await?;
    let moved = sqlx::query(
        "UPDATE public.consignment_listings
         SET price = $4, state_version = $3, updated_at = $5
         WHERE id = $1 AND state_version = $2 AND state = 'available'",
    )
    .bind(listing.listing_id)
    .bind(i64::try_from(listing.version.value())?)
    .bind(version)
    .bind(price.value())
    .bind(now)
    .execute(tx.connection())
    .await?;
    if moved.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_reprice_listing_version"));
    }
    Ok(())
}

fn success_receipt(
    identity: &CommandIdentity<'_>,
    disposition: &str,
    version: ListingVersion,
    price: ListingPrice,
) -> StoredReceipt {
    StoredReceipt {
        status: 200,
        code: "ok".to_owned(),
        body: json!({
            "schema_version": 2,
            "operation": Operation::Reprice.as_str(),
            "disposition": disposition,
            "command_id": identity.command_id.to_string(),
            "listing_id": identity.listing_id,
            "listing_version": version.value(),
            "state": "available",
            "price": price.value(),
        }),
    }
}

fn accepted(
    command: &RepriceV2Command,
    disposition: ListingPriceDisposition,
    version: ListingVersion,
    price: ListingPrice,
    replayed: bool,
) -> RepriceV2Outcome {
    RepriceV2Outcome::Accepted(RepriceV2Receipt {
        command_id: command.command_id,
        listing_id: command.input.listing_id,
        listing_version: version,
        price,
        disposition,
        replayed,
    })
}

/// 把已存回执还原为端口结果；拒绝回执只恢复安全码。
fn decode_replay(
    stored: &StoredReceipt,
    command: &RepriceV2Command,
) -> Result<RepriceV2Outcome, WriteError> {
    if stored.status >= 400 {
        let rejection = [
            RepriceRejection::ListingNotFound,
            RepriceRejection::ListingNotActive,
            RepriceRejection::StaleVersion,
            RepriceRejection::EquipmentCommitted,
        ]
        .into_iter()
        .find(|rejection| rejection.code() == stored.code)
        .ok_or_else(|| WriteError::invariant("consignment_reprice_rejection_code"))?;
        return Ok(RepriceV2Outcome::Rejected(rejection));
    }
    let body = &stored.body;
    let disposition = match body.get("disposition").and_then(Value::as_str) {
        Some("repriced") => ListingPriceDisposition::Repriced,
        Some("unchanged") => ListingPriceDisposition::Unchanged,
        _ => return Err(WriteError::invariant("consignment_reprice_receipt_field")),
    };
    let version = body
        .get("listing_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| WriteError::invariant("consignment_reprice_receipt_field"))?;
    let price = body
        .get("price")
        .and_then(Value::as_i64)
        .ok_or_else(|| WriteError::invariant("consignment_reprice_receipt_field"))?;
    Ok(accepted(
        command,
        disposition,
        ListingVersion::new(version)?,
        ListingPrice::new(price)?,
        true,
    ))
}
