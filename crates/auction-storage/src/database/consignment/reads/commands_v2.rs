//! 按原玩家身份读取 v2 写命令的已入账回执（D9 §6.1 `GET /me/consignment/commands/{id}`）。
//!
//! 复用命令账本 `consignment_commands` 与各命令重放所用的同一解码，不另建账本、不写任何行。
//! 只认 `schema_version = 2`、`public_user` 且 actor 为会话用户的购买、重列、撤回与预约命令；
//! 非本人、v1 命令、系统命令与不存在同为 `None`（不变量 1、7）。行与解码结果不自洽时失败关闭。

use auction_application::consignment::{
    ConsignmentFuture, OwnedCommandReadV2, OwnedCommandV2, OwnedDepositDecision, PurchaseOutcome,
    RelistReceipt, ReservationReceipt, WithdrawDepositReceipt,
};
use auction_domain::{CommandId, UserId};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::DepositReceipt;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::StoredReceipt;
use crate::database::consignment::deposit::decode_rejection;
use crate::database::consignment::purchase::decode_owned_purchase;

impl OwnedCommandReadV2 for DatabaseStore {
    fn owned_command_v2(
        &self,
        actor_user_id: UserId,
        command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<OwnedCommandV2>> {
        Box::pin(async move {
            owned_command_v2(self, actor_user_id, command_id)
                .await
                .map_err(WriteError::into_application)
        })
    }
}

async fn owned_command_v2(
    store: &DatabaseStore,
    actor: UserId,
    command_id: CommandId,
) -> Result<Option<OwnedCommandV2>, WriteError> {
    // 不变量 1、7：以原 actor 而非当前挂单查询账本；已提交回执不可变，单条读即可。
    let row = sqlx::query(
        "SELECT operation, target_kind, target_id, result_status, result_code, result_body
         FROM public.consignment_commands
         WHERE command_id = $1 AND schema_version = 2
           AND actor_kind = 'public_user' AND actor_ref = $2
           AND operation IN ('purchase', 'relist', 'withdraw', 'issue_reservation')",
    )
    .bind(command_id.as_uuid())
    .bind(actor.as_uuid().to_string())
    .fetch_optional(store.pool())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    // 四种命令的账本目标都是挂单；其他目标说明行与操作不符。
    if row.try_get::<Option<&str>, _>("target_kind")? != Some("listing") {
        return Err(corrupt());
    }
    let target: Option<Uuid> = row.try_get("target_id")?;
    let target = target.ok_or_else(corrupt)?;
    let operation: String = row.try_get("operation")?;
    let stored = StoredReceipt {
        status: row.try_get("result_status")?,
        code: row.try_get("result_code")?,
        body: row.try_get("result_body")?,
    };
    let command = match operation.as_str() {
        "purchase" => OwnedCommandV2::Purchase(purchase(stored, command_id, target)?),
        "relist" => OwnedCommandV2::Relist(deposit(
            &stored,
            command_id,
            target,
            |receipt: &RelistReceipt| receipt.listing_id,
        )?),
        "withdraw" => OwnedCommandV2::Withdraw(deposit(
            &stored,
            command_id,
            target,
            |receipt: &WithdrawDepositReceipt| receipt.listing_id,
        )?),
        "issue_reservation" => OwnedCommandV2::IssueReservation(deposit(
            &stored,
            command_id,
            target,
            |receipt: &ReservationReceipt| receipt.listing_id,
        )?),
        _ => return Err(corrupt()),
    };
    Ok(Some(command))
}

/// 购买沿用重放解码；拒绝另核首次状态码，成交另核目标挂单。
fn purchase(
    stored: StoredReceipt,
    command_id: CommandId,
    target: Uuid,
) -> Result<PurchaseOutcome, WriteError> {
    let status = stored.status;
    let outcome = decode_owned_purchase(stored.status, stored.code, stored.body, command_id)?;
    let consistent = match &outcome {
        PurchaseOutcome::Purchased(receipt) => receipt.listing_id == target && status < 400,
        PurchaseOutcome::Rejected(rejection) => {
            i16::try_from(rejection.http_status()).ok() == Some(status)
        }
    };
    if !consistent {
        return Err(corrupt());
    }
    Ok(outcome)
}

/// 收件族沿用 4K 的 v2 回执解码；拒绝只恢复安全码并核首次状态码，成功另核命令与目标。
fn deposit<T: DepositReceipt>(
    stored: &StoredReceipt,
    command_id: CommandId,
    target: Uuid,
    listing_of: impl Fn(&T) -> Uuid,
) -> Result<OwnedDepositDecision<T>, WriteError> {
    if stored.body.get("disposition").and_then(Value::as_str) == Some("rejected") {
        let code = stored.body.get("code").and_then(Value::as_str).unwrap_or_default();
        let rejection = decode_rejection(code).ok_or_else(corrupt)?;
        if i16::try_from(rejection.status()).ok() != Some(stored.status) {
            return Err(corrupt());
        }
        return Ok(OwnedDepositDecision::Rejected(rejection));
    }
    let receipt = T::decode(stored)?;
    if receipt.command_id() != command_id || listing_of(&receipt) != target {
        return Err(corrupt());
    }
    Ok(OwnedDepositDecision::Accepted(receipt))
}

fn corrupt() -> WriteError {
    WriteError::invariant("consignment_owned_command_v2")
}
