//! 直接接管入口（`ReturnIntakeStore::acquire_return`）：一个顶层事务，命令账本 + 锁内接管。
//!
//! 隔离、冲突与证据不足都不占命令 ID：命令 ID 只由 K 派生，若把“暂时隔离”记入账本，之后带足证据的
//! 同一封信就永远被它挡住。只有真正接管（或同事实重放）才追加成功回执。

use auction_application::consignment::{
    AcquireReceipt, AcquireReturn, ConsignmentError, DepositBusinessRejection, ReturnOperation,
};
use auction_domain::CommandId;
use auction_domain::consignment::returns::AcquireConflict;
use serde_json::{Value, json};
use uuid::Uuid;

use super::acquire::{AcquireOutcome, AcquireRequest, AcquiredIntake, acquire_in_tx};
use super::intake::{RoundLocator, load_intake};
use super::ledger::{
    Arbitration, LedgerCommand, Rejection, Step, Target, arbitrate, insert_success,
};
use super::locks::pre_read_by_round;
use crate::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};

/// 直接接管：预读买家与实物以取锁，锁内由 `acquire_in_tx` 重读重判。
pub(super) async fn acquire_return(
    store: &DatabaseStore,
    command: AcquireReturn,
) -> Result<AcquireReceipt, ConsignmentError> {
    let facts = &command.facts;
    let mut pre =
        store.pool().acquire().await.map_err(|error| WriteError::from(error).into_application())?;
    let plan = pre_read_by_round(&mut pre, facts.obligation_id, facts.original_round)
        .await
        .map_err(WriteError::into_application)?;
    drop(pre);
    // 没有这一轮的持久事实：来源不明，零工作、继续隔离。
    let Some(plan) = plan else {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven));
    };
    let (sellers, equipment) = (plan.sellers(), plan.equipment());
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &sellers,
        equipment: &equipment,
    };
    let key = facts.k.clone();
    let mail_keys = [key.clone()];
    let transaction = with_deposit_transaction(store, keys, &mail_keys, &command, |command, tx| {
        let (sellers, equipment, key) = (sellers.clone(), equipment.clone(), key.clone());
        Box::pin(async move {
            tx.ensure_lock_set(&sellers, &equipment, std::slice::from_ref(&key))?;
            let ledger = LedgerCommand {
                command_id: command.command_id.as_uuid(),
                operation: ReturnOperation::AcquireReturn,
                target: Target::MailKey(&key),
                digest: command.request_hash,
            };
            match arbitrate(tx.connection(), &ledger).await? {
                Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
                Step::Done(Arbitration::Replay(body)) => {
                    return replay(tx.connection(), command.command_id, &body)
                        .await
                        .map(Step::Done);
                }
                Step::Done(Arbitration::First) => {}
            }
            let outcome = acquire_in_tx(
                tx.connection(),
                &AcquireRequest {
                    key: &key,
                    observation_id: command.facts.rts_observation_id,
                    locator: RoundLocator::ByObligationRound {
                        obligation_id: command.facts.obligation_id,
                        round: command.facts.original_round,
                    },
                    command_id: command.command_id.as_uuid(),
                    write_initial_decision: true,
                },
            )
            .await?;
            match outcome {
                AcquireOutcome::Acquired(acquired) => {
                    record(tx.connection(), &ledger, &acquired).await?;
                    Ok(Step::Done(receipt(command.command_id, &acquired, false)))
                }
                AcquireOutcome::Replay(acquired) => {
                    record(tx.connection(), &ledger, &acquired).await?;
                    Ok(Step::Done(receipt(command.command_id, &acquired, true)))
                }
                AcquireOutcome::Quarantine(reason) => {
                    tracing::info!(?reason, "寄售 RTS 暂不接管，继续隔离");
                    Ok(Step::Rejected(Rejection::EvidenceUnproven))
                }
                AcquireOutcome::Conflict(AcquireConflict::SameLegSecondMid) => {
                    // 冲突案行已在 acquire_in_tx 里追加，随本事务提交。
                    Ok(Step::Rejected(Rejection::DispositionConflict))
                }
                AcquireOutcome::Conflict(reason) => {
                    tracing::warn!(?reason, "寄售 RTS 接管冲突，不接管");
                    // 没有任何写入：回滚并报告相反处置或异来源。
                    Err(WriteError::Application(ConsignmentError::Deposit(
                        DepositBusinessRejection::DispositionConflict,
                    )))
                }
            }
        })
    })
    .await
    .map_err(WriteError::into_application)?;
    transaction.into_result()
}

fn receipt(command_id: CommandId, acquired: &AcquiredIntake, replayed: bool) -> AcquireReceipt {
    AcquireReceipt {
        command_id,
        intake_id: acquired.intake_id,
        work_id: acquired.take_work_id,
        decision_id: acquired.decision_id,
        replayed,
    }
}

async fn record(
    connection: &mut sqlx::PgConnection,
    ledger: &LedgerCommand<'_>,
    acquired: &AcquiredIntake,
) -> Result<(), WriteError> {
    insert_success(
        connection,
        ledger,
        "return_acquired",
        json!({
            "return_intake_id": acquired.intake_id.to_string(),
            "original_round": acquired.facts.original_round,
            "take_work_id": acquired.take_work_id.to_string(),
            "take_delivery_id": acquired.take_delivery_id.to_string(),
        }),
    )
    .await
}

async fn replay(
    connection: &mut sqlx::PgConnection,
    command_id: CommandId,
    body: &Value,
) -> Result<AcquireReceipt, WriteError> {
    let intake_id = body
        .get("return_intake_id")
        .and_then(Value::as_str)
        .and_then(|text| Uuid::parse_str(text).ok())
        .ok_or_else(|| WriteError::invariant("consignment_acquire_replay_body"))?;
    let row = load_intake(connection, intake_id)
        .await?
        .ok_or_else(|| WriteError::invariant("consignment_acquire_replay_intake"))?;
    Ok(AcquireReceipt {
        command_id,
        intake_id,
        work_id: row.take_work_id,
        decision_id: row.decision_id,
        replayed: true,
    })
}
