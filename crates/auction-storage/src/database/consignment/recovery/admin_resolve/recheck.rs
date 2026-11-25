//! 订单复核排队：只追加一条持久复核工作，不改任何订单状态。
//!
//! 保护不变量 1、5、8：工作行与账本回执同事务提交；工作由 Worker 领取后重核证据并只追加观察，内存
//! 定时器不是工作的来源。同命令 ID 同请求返回原工作 ID，不会重复排队。

use auction_application::consignment::{RecheckOrderCommand, RecheckOrderReceipt};
use auction_domain::CommandId;
use serde_json::{Value, json};
use uuid::Uuid;

use super::ledger::{Admission, Flow, OrderLedger, admit, body_uuid, body_version, insert_success};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::dispatch::lock_order::ContractLockIdentity;

fn receipt(command_id: Uuid, body: &Value) -> Result<RecheckOrderReceipt, WriteError> {
    Ok(RecheckOrderReceipt {
        command_id: CommandId::from_uuid(command_id),
        contract_id: body_uuid(body, "contract_id")?,
        contract_version: body_version(body)?,
        work_id: body_uuid(body, "work_id")?,
    })
}

/// 事务体：准入后写工作行与 202 回执。
pub(super) async fn recheck_in_tx(
    request: &(RecheckOrderCommand, ContractLockIdentity),
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Flow<RecheckOrderReceipt>, WriteError> {
    let (command, identity) = request;
    tx.ensure_lock_set(&identity.sellers, &identity.equipment, &[])?;
    let connection = tx.connection();
    let ledger = OrderLedger {
        command_id: command.command_id.as_uuid(),
        operation: auction_application::consignment::OrderAdminOperation::Recheck,
        actor: &command.admin_actor,
        contract_id: command.contract_id,
        digest: command.request_hash,
        request_id: command.request_id.as_deref(),
    };
    let version = match admit(connection, &ledger, Some(command.expected_version)).await? {
        Flow::Rejected(rejection) => return Ok(Flow::Rejected(rejection)),
        Flow::Done(Admission::Replayed(body)) => {
            return Ok(Flow::Done(receipt(ledger.command_id, &body)?));
        }
        Flow::Done(Admission::Fresh(version)) => version,
    };
    let work_id = Uuid::new_v4();
    let now = database_now(connection).await?;
    sqlx::query(
        "INSERT INTO public.consignment_order_rechecks
            (work_id, contract_id, contract_version, requested_by, command_id, requested_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(work_id)
    .bind(command.contract_id)
    .bind(i64::try_from(version)?)
    .bind(&command.admin_actor)
    .bind(ledger.command_id)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let body = insert_success(
        connection,
        &ledger,
        202,
        "recheck_queued",
        json!({ "contract_version": version, "work_id": work_id }),
    )
    .await?;
    Ok(Flow::Done(receipt(ledger.command_id, &body)?))
}
