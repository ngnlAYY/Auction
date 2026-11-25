//! 补寄原件：只排队一个新轮，上一轮必须已由退件接管与消费明确封闭。
//!
//! 保护不变量 4、5、6：补寄不是重发许可——这里只记录“允许开轮 N+1”的处置与将要开的轮 ID，真正的授权、
//! 起写与 POST 仍走派发写端（`authorize_dispatch` 的 Redispatch 来源，要求同一接管与消费）；
//! 未明确封闭（没有已消费的原件退件接管）、已有更新的轮、轮号越界、非未付 COD 原件腿、或同一轮已排队，
//! 一律确定拒绝，绝不自动补发。同命令重放返回原回执。

use auction_application::consignment::{
    DepositBusinessRejection, OrderAdminOperation, OrderResolveAction, OrderResolveReceipt,
    ResolveOrderCommand,
};
use auction_domain::consignment::dispatch::{DispatchRoundKey, dispatch_round_id};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use super::ledger::{Admission, Flow, OrderLedger, admit, insert_rejection, insert_success};
use super::{Resolved, record_resolution, resolve_receipt};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::dispatch::lock_order::ContractLockIdentity;

/// 轮号上限，与 `c6_round_number_ck` 一致。
const MAX_ROUND: i32 = 100;

/// 补寄只适用于未付 COD 的原件腿（D9 §5.1：已付 isekai 的原件交接退回走退款，不补寄）。
const UNPAID_ORIGINAL_KIND: &str = "winner_equipment_cod";

/// 已封闭的上一轮及其腿。
struct ClosedRound {
    obligation_id: Uuid,
    obligation_kind: String,
    round: i32,
}

/// 证据是同合同、原件腿、已消费（精确 Take 已证）的退件接管；返回它封闭的那一轮。
async fn closed_round(
    connection: &mut PgConnection,
    contract_id: Uuid,
    intake_id: Uuid,
) -> Result<Option<ClosedRound>, WriteError> {
    let row: Option<(Uuid, String, i32)> = sqlx::query_as(
        "SELECT consumption.obligation_id, obligation.obligation_kind, consumption.original_round
         FROM public.consignment_return_consumptions AS consumption
         JOIN public.settlement_obligations AS obligation ON obligation.id = consumption.obligation_id
         WHERE consumption.intake_id = $2 AND consumption.contract_id = $1
           AND consumption.leg = 'original'",
    )
    .bind(contract_id)
    .bind(intake_id)
    .fetch_optional(connection)
    .await?;
    Ok(row.map(|(obligation_id, obligation_kind, round)| ClosedRound {
        obligation_id,
        obligation_kind,
        round,
    }))
}

/// 事务体。
pub(super) async fn redispatch_in_tx(
    request: &(ResolveOrderCommand, ContractLockIdentity),
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Flow<OrderResolveReceipt>, WriteError> {
    let (command, identity) = request;
    tx.ensure_lock_set(&identity.sellers, &identity.equipment, &[])?;
    let connection = tx.connection();
    let ledger = OrderLedger {
        command_id: command.command_id.as_uuid(),
        operation: OrderAdminOperation::Resolve,
        actor: &command.admin_actor,
        contract_id: command.contract_id,
        digest: command.request_hash,
        request_id: command.request_id.as_deref(),
    };
    let version = match admit(connection, &ledger, Some(command.expected_version)).await? {
        Flow::Rejected(rejection) => return Ok(Flow::Rejected(rejection)),
        Flow::Done(Admission::Replayed(body)) => {
            return Ok(Flow::Done(resolve_receipt(ledger.command_id, &body)?));
        }
        Flow::Done(Admission::Fresh(version)) => version,
    };
    let Some(closed) = closed_round(connection, command.contract_id, command.evidence_id).await?
    else {
        return insert_rejection(connection, &ledger, DepositBusinessRejection::EvidenceUnproven)
            .await;
    };
    let latest: Option<i32> = sqlx::query_scalar(
        "SELECT max(round) FROM public.consignment_dispatch_rounds
         WHERE contract_id = $1 AND obligation_id = $2",
    )
    .bind(command.contract_id)
    .bind(closed.obligation_id)
    .fetch_one(&mut *connection)
    .await?;
    // 只能紧接被封闭的那一轮开新轮：已有更新的轮说明补寄已发生或被别的路径开过。
    if closed.obligation_kind != UNPAID_ORIGINAL_KIND
        || latest != Some(closed.round)
        || closed.round >= MAX_ROUND
    {
        return insert_rejection(connection, &ledger, DepositBusinessRejection::ActionNotAllowed)
            .await;
    }
    let dispatch_id = dispatch_round_id(&DispatchRoundKey {
        contract_id: command.contract_id,
        obligation_id: closed.obligation_id,
        round: u32::try_from(closed.round + 1)?,
    });
    let queued: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_order_resolutions WHERE dispatch_id = $1)",
    )
    .bind(dispatch_id)
    .fetch_one(&mut *connection)
    .await?;
    if queued {
        return insert_rejection(
            connection,
            &ledger,
            DepositBusinessRejection::DispositionConflict,
        )
        .await;
    }
    let now = database_now(connection).await?;
    record_resolution(
        connection,
        &ledger,
        command,
        Resolved { resolution_id: Uuid::new_v4(), dispatch_id: Some(dispatch_id), version },
        now,
    )
    .await?;
    let body = insert_success(
        connection,
        &ledger,
        202,
        "redispatch_queued",
        json!({
            "contract_version": version,
            "action": OrderResolveAction::RedispatchOriginal.as_str(),
            "dispatch_id": dispatch_id,
        }),
    )
    .await?;
    Ok(Flow::Done(resolve_receipt(ledger.command_id, &body)?))
}
