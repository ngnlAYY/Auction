//! Admin 订单 recheck 与 resolve 的存储实现。
//!
//! 实现 `OrderAdminStore`：recheck 排入持久复核工作；resolve 的动作是闭集——退款类三个动作与 `retain_entitlement`
//! 共用 6B 资金写端（`refund` / `entitlement`），`cancel_unpaid` 调 7A-2 的封闭写端（`unpaid`），补寄只排队新轮
//! （`redispatch`）。资金类动作的核证据、资金写入、封闭事实、处置记录、订单阶段事件与回执在**同一个**顶层事务里
//! 完成（不变量 3）；资金原语的确定性失败只回滚到保存点再把拒绝写入账本，暂时性失败整笔回滚。
//! 每个写操作先取命令锁与卖家 / 规范实物会话锁，再开 Serializable 顶层事务；序列化冲突有界重试，
//! 提交结果未知时以同一命令 ID 重放读取原回执。

mod entitlement;
mod ledger;
mod recheck;
mod redispatch;
mod refund;
mod unpaid;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DepositBusinessRejection, ListingBusinessRejection,
    OrderAdminOperation, OrderAdminStore, OrderResolveAction, OrderResolveReceipt,
    RecheckOrderCommand, RecheckOrderReceipt, ResolveOrderCommand,
};
use auction_application::settlement::FundingError;
use auction_domain::CommandId;
use auction_time::Timestamp;
use serde_json::Value;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use self::ledger::{
    Admission, Flow, OrderLedger, admit, body_uuid, body_version, contract_version, insert_success,
};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::dispatch::lock_order::{
    ContractLockIdentity, pre_read_by_contract,
};
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;
use crate::database::consignment::returns::funding_error;
use crate::database::settlement::funding::lock_funding_worlds;

/// 一条处置记录的落库输入。
struct Resolved {
    resolution_id: Uuid,
    /// 仅补寄：将要开的新轮 ID。
    dispatch_id: Option<Uuid>,
    /// 落账时的合同版本。
    version: u64,
}

/// 追加处置记录；数据库触发器是退款已转换、补寄已封闭的最后一道门（不变量 4、6）。
async fn record_resolution(
    connection: &mut PgConnection,
    ledger: &OrderLedger<'_>,
    command: &ResolveOrderCommand,
    resolved: Resolved,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_order_resolutions
            (resolution_id, contract_id, action, evidence_id, dispatch_id, contract_version,
             resolved_by, command_id, resolved_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(resolved.resolution_id)
    .bind(command.contract_id)
    .bind(command.action.as_str())
    .bind(command.evidence_id)
    .bind(resolved.dispatch_id)
    .bind(i64::try_from(resolved.version)?)
    .bind(&command.admin_actor)
    .bind(ledger.command_id)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// 把成功回执正文还原为应用回执；形状错误说明账本被改坏，失败关闭。
fn resolve_receipt(command_id: Uuid, body: &Value) -> Result<OrderResolveReceipt, WriteError> {
    let command_id = CommandId::from_uuid(command_id);
    let contract_id = body_uuid(body, "contract_id")?;
    let contract_version = body_version(body)?;
    let action = body
        .get("action")
        .and_then(Value::as_str)
        .and_then(OrderResolveAction::parse)
        .ok_or_else(|| WriteError::invariant("consignment_order_receipt_body"))?;
    match body.get("disposition").and_then(Value::as_str) {
        Some("resolution_recorded") if action != OrderResolveAction::RedispatchOriginal => {
            Ok(OrderResolveReceipt::ResolutionRecorded {
                command_id,
                contract_id,
                contract_version,
                action,
                resolution_id: body_uuid(body, "resolution_id")?,
            })
        }
        Some("redispatch_queued") if action == OrderResolveAction::RedispatchOriginal => {
            Ok(OrderResolveReceipt::RedispatchQueued {
                command_id,
                contract_id,
                contract_version,
                dispatch_id: body_uuid(body, "dispatch_id")?,
            })
        }
        _ => Err(WriteError::invariant("consignment_order_receipt_body")),
    }
}

/// 预读合同的锁键身份；合同不存在没有可锁的实物，也没有可挂靠的账本目标，按 404 报告且不入账。
async fn lock_identity(
    store: &DatabaseStore,
    contract_id: Uuid,
) -> Result<ContractLockIdentity, ConsignmentError> {
    let identity =
        pre_read_by_contract(store, contract_id).await.map_err(WriteError::into_application)?;
    if identity.sellers.is_empty() {
        return Err(ListingBusinessRejection::ListingNotFound.into());
    }
    Ok(identity)
}

fn keys<'a>(command_id: CommandId, identity: &'a ContractLockIdentity) -> LockKeys<'a> {
    LockKeys {
        command_id: command_id.as_uuid(),
        sellers: &identity.sellers,
        equipment: &identity.equipment,
    }
}

fn ledger_of(command: &ResolveOrderCommand) -> OrderLedger<'_> {
    OrderLedger {
        command_id: command.command_id.as_uuid(),
        operation: OrderAdminOperation::Resolve,
        actor: &command.admin_actor,
        contract_id: command.contract_id,
        digest: command.request_hash,
        request_id: command.request_id.as_deref(),
    }
}

/// 准入结果：原回执重放，或首次写入。
enum Gate {
    Replayed(OrderResolveReceipt),
    Fresh,
}

/// 不变量 1、2、6：账本仲裁、合同锁与预期版本；资金类动作在同一事务里继续核证据与写资金。
async fn gate(
    connection: &mut PgConnection,
    ledger: &OrderLedger<'_>,
    expected_version: u64,
) -> Result<Flow<Gate>, WriteError> {
    Ok(match admit(connection, ledger, Some(expected_version)).await? {
        Flow::Rejected(rejection) => Flow::Rejected(rejection),
        Flow::Done(Admission::Replayed(body)) => {
            Flow::Done(Gate::Replayed(resolve_receipt(ledger.command_id, &body)?))
        }
        Flow::Done(Admission::Fresh(_)) => Flow::Done(Gate::Fresh),
    })
}

/// 资金类动作的落账尾段：订单阶段事件（读到同事务内已写的资金事实）、处置记录与成功回执。
async fn finish(
    connection: &mut PgConnection,
    ledger: &OrderLedger<'_>,
    command: &ResolveOrderCommand,
) -> Result<Flow<OrderResolveReceipt>, WriteError> {
    let now = database_now(connection).await?;
    // 阶段变了：事件、权利刷新与处置同事务（不变量 3），两者读到的都是本事务已写的事实。
    record_order_stage_change_in_tx(connection, command.contract_id, now).await?;
    let version = contract_version(connection, command.contract_id).await?;
    let resolution_id = Uuid::new_v4();
    record_resolution(
        connection,
        ledger,
        command,
        Resolved { resolution_id, dispatch_id: None, version },
        now,
    )
    .await?;
    let body = insert_success(
        connection,
        ledger,
        200,
        "resolution_recorded",
        json!({
            "contract_version": version,
            "action": command.action.as_str(),
            "resolution_id": resolution_id,
        }),
    )
    .await?;
    Ok(Flow::Done(resolve_receipt(ledger.command_id, &body)?))
}

/// 保存点：资金原语的确定性失败（冲突、证据不足）只回滚到这里，再把拒绝写入账本。
const FUNDING_SAVEPOINT: &str = "order_resolve_funding";

async fn savepoint(connection: &mut PgConnection) -> Result<(), WriteError> {
    sqlx::query(sqlx::AssertSqlSafe(format!("SAVEPOINT {FUNDING_SAVEPOINT}")))
        .execute(connection)
        .await?;
    Ok(())
}

async fn rollback_to_savepoint(connection: &mut PgConnection) -> Result<(), WriteError> {
    sqlx::query(sqlx::AssertSqlSafe(format!("ROLLBACK TO SAVEPOINT {FUNDING_SAVEPOINT}")))
        .execute(connection)
        .await?;
    Ok(())
}

/// 资金原语对确定性失败的分类；暂时性失败不入账，整笔回滚后以原命令 ID 重试。
fn rejection_of(error: &FundingError) -> Option<DepositBusinessRejection> {
    match error {
        FundingError::Conflict => Some(DepositBusinessRejection::DispositionConflict),
        FundingError::Unproven => Some(DepositBusinessRejection::EvidenceUnproven),
        FundingError::Invalid => Some(DepositBusinessRejection::ActionNotAllowed),
        FundingError::Unavailable
        | FundingError::Unsupported
        | FundingError::SerializationConflict
        | FundingError::Storage => None,
    }
}

/// 互斥的处置动作组：退款类三动作互斥，其余各自成组。
const fn exclusive_group(action: OrderResolveAction) -> &'static [&'static str] {
    match action {
        OrderResolveAction::RefundReturned
        | OrderResolveAction::RefundLost
        | OrderResolveAction::RefundSeason => &["refund_returned", "refund_lost", "refund_season"],
        OrderResolveAction::CancelUnpaid => &["cancel_unpaid"],
        OrderResolveAction::RetainEntitlement => &["retain_entitlement"],
        OrderResolveAction::RedispatchOriginal => &["redispatch_original"],
    }
}

/// 同一合同已有互斥的处置：退款类三动作与未付取消每合同至多一条，权利保留按退回处置（证据）至多一条。
async fn already_resolved(
    connection: &mut PgConnection,
    command: &ResolveOrderCommand,
) -> Result<bool, WriteError> {
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_order_resolutions
                        WHERE action = ANY($2)
                          AND CASE WHEN $3 THEN evidence_id = $4 ELSE contract_id = $1 END)",
    )
    .bind(command.contract_id)
    .bind(exclusive_group(command.action))
    .bind(command.action == OrderResolveAction::RetainEntitlement)
    .bind(command.evidence_id)
    .fetch_one(connection)
    .await?;
    Ok(found)
}

/// 资金类动作与补寄的事务体：先过账本准入，再按动作分派；资金写入前先取 funding worlds 锁
/// （锁序与 `resolve_return` 一致，之后各原语自取 UID 账户锁）。
async fn resolve_in_tx(
    request: &(ResolveOrderCommand, ContractLockIdentity),
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Flow<OrderResolveReceipt>, WriteError> {
    let (command, identity) = request;
    if command.action == OrderResolveAction::RedispatchOriginal {
        return redispatch::redispatch_in_tx(request, tx).await;
    }
    tx.ensure_lock_set(&identity.sellers, &identity.equipment, &[])?;
    let ledger = ledger_of(command);
    match gate(tx.connection(), &ledger, command.expected_version).await? {
        Flow::Rejected(rejection) => return Ok(Flow::Rejected(rejection)),
        Flow::Done(Gate::Replayed(receipt)) => return Ok(Flow::Done(receipt)),
        Flow::Done(Gate::Fresh) => {}
    }
    lock_funding_worlds(tx.tracked()).await.map_err(funding_error)?;
    match command.action {
        OrderResolveAction::CancelUnpaid => unpaid::cancel_in_tx(command, &ledger, tx).await,
        OrderResolveAction::RetainEntitlement => {
            entitlement::retain_in_tx(command, &ledger, tx).await
        }
        _ => refund::refund_in_tx(command, &ledger, tx).await,
    }
}

impl OrderAdminStore for DatabaseStore {
    fn recheck_order(
        &self,
        command: RecheckOrderCommand,
    ) -> ConsignmentFuture<'_, RecheckOrderReceipt> {
        Box::pin(async move {
            let identity = lock_identity(self, command.contract_id).await?;
            let request = (command, identity);
            with_deposit_transaction(
                self,
                keys(request.0.command_id, &request.1),
                &[],
                &request,
                |request, tx| Box::pin(recheck::recheck_in_tx(request, tx)),
            )
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }

    fn resolve_order(
        &self,
        command: ResolveOrderCommand,
    ) -> ConsignmentFuture<'_, OrderResolveReceipt> {
        Box::pin(async move {
            let identity = lock_identity(self, command.contract_id).await?;
            let request = (command, identity);
            with_deposit_transaction(
                self,
                keys(request.0.command_id, &request.1),
                &[],
                &request,
                |request, tx| Box::pin(resolve_in_tx(request, tx)),
            )
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }
}
