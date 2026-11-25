//! 灭失三个命令的端口实现：证据追加、灭失确认与案件人工结果，均先经全局命令账本仲裁。
//!
//! 保护不变量 1、3、6：同命令 ID 同请求返回首次回执，同 ID 异请求为 `command_id_conflict`；
//! 事实、回执与追加审计同一顶层事务提交（账本的延迟触发器核对回执与同事务事实一一对应）。
//! 原语在写入前返回的确定业务拒绝落为安全回执（拒绝路径不写任何业务行，无需保存点）。

use auction_application::consignment::{
    ConfirmPositionLossCommand, ConsignmentError, ConsignmentFuture, DepositActor,
    DepositBusinessRejection, DepositOperation, LossCaseOutcomeReceipt, LossProofReceipt,
    PositionLossReceipt, PositionLossStore, RecordLossCaseOutcomeCommand, RecordLossProofCommand,
};
use auction_time::Timestamp;
use serde_json::Value;
use uuid::Uuid;

use super::confirm::{ConfirmOutcome, confirm_stage};
use super::outcome::{LossOutcomeInput, append_loss_case_outcome};
use super::proof::{LossProofInput, append_loss_proof};
use super::routing::pre_read_listing;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{
    DepositArbitration, DepositTarget, StoredReceipt,
};
use crate::database::consignment::commands::{
    DepositReceipt, DepositRejection, DepositWriter, LockKeys, with_deposit_transaction,
};
use crate::database::consignment::deposit::decode_rejection;
use crate::database::consignment::deposit::primitives::{database_now, pre_read_identity};
use crate::database::consignment::deposit::repository::DepositRepository;

/// 同一事务内已落库的成功回执或确定拒绝。
enum Outcome<T> {
    Accepted(T),
    Rejected(DepositBusinessRejection),
}

impl<T> Outcome<T> {
    /// 提交后才把拒绝变成错误，保证拒绝回执不会随回滚丢失。
    fn into_result(self) -> Result<T, ConsignmentError> {
        match self {
            Self::Accepted(receipt) => Ok(receipt),
            Self::Rejected(rejection) => Err(ConsignmentError::Deposit(rejection)),
        }
    }
}

/// 解码已有账本行：拒绝只恢复安全码，成功还原回执。
fn replay<T: DepositReceipt>(stored: &StoredReceipt) -> Result<Outcome<T>, WriteError> {
    if stored.body.get("disposition").and_then(Value::as_str) == Some("rejected") {
        let code = stored.body.get("code").and_then(Value::as_str).unwrap_or_default();
        let rejection = decode_rejection(code)
            .ok_or_else(|| WriteError::invariant("consignment_deposit_rejection_code"))?;
        return Ok(Outcome::Rejected(rejection));
    }
    Ok(Outcome::Accepted(T::decode(stored)?))
}

async fn reject<T>(
    writer: DepositWriter<'_, '_, '_>,
    rejection: DepositBusinessRejection,
    now: Timestamp,
) -> Result<Outcome<T>, WriteError> {
    writer.finish_rejection(DepositRejection::Business(rejection), now).await?;
    Ok(Outcome::Rejected(rejection))
}

/// 原语的确定业务拒绝；其余错误（含并发冲突）原样上抛，由事务 owner 回滚或重试。
fn business(error: WriteError) -> Result<DepositBusinessRejection, WriteError> {
    match error {
        WriteError::Application(ConsignmentError::Deposit(rejection)) => Ok(rejection),
        other => Err(other),
    }
}

impl PositionLossStore for DepositRepository {
    fn record_loss_proof(
        &self,
        command: RecordLossProofCommand,
    ) -> ConsignmentFuture<'_, LossProofReceipt> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            // 证据追加只对 position 行加共享锁，不碰挂单与实物互斥：只需命令锁。
            let keys =
                LockKeys { command_id: command.command_id.as_uuid(), sellers: &[], equipment: &[] };
            with_deposit_transaction(self.store(), keys, &[], &command, |command, tx| {
                Box::pin(async move {
                    tx.ensure_lock_set(&[], &[], &[])?;
                    match tx
                        .arbitrate(
                            DepositActor::Admin(command.admin_actor.clone()),
                            DepositOperation::RecordPositionLossProof,
                            DepositTarget::Position(command.position_id),
                            command.request_hash,
                            command.request_id.as_deref(),
                        )
                        .await?
                    {
                        DepositArbitration::Replay(stored) => replay(&stored),
                        DepositArbitration::First(mut writer) => {
                            let now = database_now(writer.connection()).await?;
                            // 私有说明只进审计 metadata，不进回执与 DTO（与灭失确认的 reason 同一写法）。
                            writer.set_private_audit_note(
                                "loss_proof_note",
                                command.private_note.as_str(),
                            );
                            let input = LossProofInput {
                                position_id: command.position_id,
                                source_kind: &command.source_kind,
                                observation_ref: &command.observation_ref,
                                last_known_ref: command.last_known_ref.as_str(),
                                command_id: command.command_id,
                                now,
                            };
                            match append_loss_proof(writer.connection(), &input).await {
                                Ok(receipt) => {
                                    writer.finish(receipt.clone(), now).await?;
                                    Ok(Outcome::Accepted(receipt))
                                }
                                Err(error) => reject(writer, business(error)?, now).await,
                            }
                        }
                    }
                })
            })
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }

    fn confirm_position_loss(
        &self,
        command: ConfirmPositionLossCommand,
    ) -> ConsignmentFuture<'_, PositionLossReceipt> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            let store = self.store();
            let listing_id = pre_read_listing(store, command.position_id)
                .await
                .map_err(WriteError::into_application)?;
            let identity = match listing_id {
                Some(listing_id) => pre_read_identity(store, listing_id)
                    .await
                    .map_err(WriteError::into_application)?,
                None => None,
            };
            let sellers: Vec<_> = identity.iter().map(|id| id.seller_user_id).collect();
            let equipment: Vec<_> = identity.iter().map(|id| id.equipment.clone()).collect();
            let input = (command, listing_id, sellers, equipment);
            let keys = LockKeys {
                command_id: input.0.command_id.as_uuid(),
                sellers: &input.2,
                equipment: &input.3,
            };
            with_deposit_transaction(
                store,
                keys,
                &[],
                &input,
                |(command, listing_id, sellers, equipment), tx| {
                    Box::pin(async move {
                        tx.ensure_lock_set(sellers, equipment, &[])?;
                        match tx
                            .arbitrate(
                                DepositActor::Admin(command.admin_actor.clone()),
                                DepositOperation::ConfirmPositionLoss,
                                DepositTarget::Position(command.position_id),
                                command.request_hash,
                                command.request_id.as_deref(),
                            )
                            .await?
                        {
                            DepositArbitration::Replay(stored) => replay(&stored),
                            DepositArbitration::First(writer) => {
                                Ok(match confirm_stage(command, *listing_id, writer).await? {
                                    ConfirmOutcome::Accepted(receipt) => Outcome::Accepted(receipt),
                                    ConfirmOutcome::Rejected(rejection) => {
                                        Outcome::Rejected(rejection)
                                    }
                                })
                            }
                        }
                    })
                },
            )
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }

    fn record_loss_case_outcome(
        &self,
        command: RecordLossCaseOutcomeCommand,
    ) -> ConsignmentFuture<'_, LossCaseOutcomeReceipt> {
        Box::pin(record_outcome(self.store(), command))
    }
}

/// Admin 追加人工结果：命令账本、结果行与卖家回执同一事务；`LossCaseAdmin` 与 `PositionLossStore` 共用。
pub(super) async fn record_outcome(
    store: &DatabaseStore,
    command: RecordLossCaseOutcomeCommand,
) -> Result<LossCaseOutcomeReceipt, ConsignmentError> {
    if command.command_id.as_uuid().is_nil() {
        return Err(ConsignmentError::InvalidCommandId);
    }
    // 预读卖家只用于取用户锁；案件归属与版本仍由事务内原语在锁后复核。
    let seller: Option<Uuid> = sqlx::query_scalar(
        "SELECT seller_user_id FROM public.consignment_loss_cases
         WHERE case_id = $1 AND position_id = $2",
    )
    .bind(command.case_id)
    .bind(command.position_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|error| WriteError::from(error).into_application())?;
    let Some(seller_user_id) = seller else {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch));
    };
    // 两个新 ID 在重试之外生成一次；同命令重放由账本返回原回执，不使用它们。
    let input = (command, (Uuid::new_v4(), Uuid::new_v4()), [seller_user_id]);
    let keys =
        LockKeys { command_id: input.0.command_id.as_uuid(), sellers: &input.2, equipment: &[] };
    with_deposit_transaction(store, keys, &[], &input, |(command, ids, sellers), tx| {
        Box::pin(async move {
            tx.ensure_lock_set(sellers, &[], &[])?;
            match tx
                .arbitrate(
                    DepositActor::Admin(command.admin_actor.clone()),
                    DepositOperation::RecordLossCaseOutcome,
                    DepositTarget::Position(command.position_id),
                    command.request_hash,
                    command.request_id.as_deref(),
                )
                .await?
            {
                DepositArbitration::Replay(stored) => replay(&stored),
                DepositArbitration::First(mut writer) => {
                    let now = database_now(writer.connection()).await?;
                    let input = LossOutcomeInput {
                        outcome_id: ids.0,
                        seller_receipt_id: ids.1,
                        case_id: command.case_id,
                        position_id: command.position_id,
                        expected_case_version: command.expected_case_version,
                        outcome: command.outcome,
                        basis_id: command.basis_id,
                        private_note: command.private_note.as_str(),
                        command_id: command.command_id,
                        now,
                    };
                    match append_loss_case_outcome(writer.connection(), &input).await {
                        Ok(receipt) => {
                            writer.finish(receipt.clone(), now).await?;
                            Ok(Outcome::Accepted(receipt))
                        }
                        Err(error) => reject(writer, business(error)?, now).await,
                    }
                }
            }
        })
    })
    .await
    .map_err(WriteError::into_application)?
    .into_result()
}
