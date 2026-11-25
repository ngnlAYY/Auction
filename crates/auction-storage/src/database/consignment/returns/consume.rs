//! 退件消费：同事务写精确 Take 证明、消费行与实际效果（实物回位或现金回补并解除 UID 阻断）。
//!
//! 保护不变量 2、3、4、6、8：消费只一次（唯一 K、唯一 intake、唯一原轮资产腿）；只有精确 Take
//! 证明、起写事实、命令引用与接管互相指认才消费，证据不足（含验证器未签收）保留核对、不写任何行、
//! 不占命令 ID；矛盾失败关闭。证明、消费与资金回补同事务提交，不存在“有证明无消费”的半态。

use auction_application::consignment::{
    ConsignmentError, ConsumeReceipt, ConsumeReturn, MailProofKind, ProofDisposition,
    ReturnOperation,
};
use auction_application::settlement::FundingMutation;
use auction_domain::CommandId;
use auction_domain::consignment::ProofValidatorStatus;
use auction_domain::consignment::returns::{
    ConsumeCommandRef, ConsumeConflict, ConsumeDecision, ConsumeEffect, ConsumeFacts, ConsumePlan,
    ExistingConsumption, StartedRef, TakeProofFacts, decide_consumption,
};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::conflicts::funding_error;
use super::intake::{IntakeRow, load_intake};
use super::ledger::{
    Arbitration, LedgerCommand, Rejection, Step, Target, arbitrate, insert_success,
};
use super::locks::pre_read_by_intake;
use crate::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;
use crate::database::mail::originals::identity::MailKeyColumns;
use crate::database::mail::originals::invariant_decoding;
use crate::database::mail::originals::proofs::{ProofWrite, load_start, upsert_proof};
use crate::database::settlement::funding::consignment::returns::{
    ReturnConsumptionFacts, return_cash_in_tx,
};
use crate::database::settlement::funding::lock_funding_worlds;

/// 已签收的验证器版本。本期全部未签收（R5-C10）：生产路径的消费恒被拒，签收是单独提交。
const SIGNED_OFF_VALIDATORS: &[&str] = &[];

/// 仅 `test-db` 构建认的已签收验证器版本，供测试夹具走通消费正例；生产构建不含它。
#[cfg(feature = "test-db")]
const TEST_SIGNED_OFF_VALIDATOR: &str = "test-signed-off-validator-v1";

fn validator_status(version: &str) -> ProofValidatorStatus {
    #[cfg(feature = "test-db")]
    if version == TEST_SIGNED_OFF_VALIDATOR {
        return ProofValidatorStatus::SignedOff;
    }
    if SIGNED_OFF_VALIDATORS.contains(&version) {
        ProofValidatorStatus::SignedOff
    } else {
        ProofValidatorStatus::Unsigned
    }
}

/// 消费端口实现：预读锁集合后在一个顶层 Serializable 事务内完成。
pub(super) async fn consume_return(
    store: &DatabaseStore,
    command: ConsumeReturn,
) -> Result<ConsumeReceipt, ConsignmentError> {
    let mut pre =
        store.pool().acquire().await.map_err(|error| WriteError::from(error).into_application())?;
    let pre_read = pre_read_by_intake(&mut pre, command.intake_id)
        .await
        .map_err(WriteError::into_application)?;
    drop(pre);
    let Some((key, plan)) = pre_read else {
        return Err(ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::SourceMismatch,
        ));
    };
    let (sellers, equipment) = (plan.sellers(), plan.equipment());
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &sellers,
        equipment: &equipment,
    };
    let mail_keys = [key.clone()];
    let transaction = with_deposit_transaction(store, keys, &mail_keys, &command, |command, tx| {
        let (sellers, equipment, key) = (sellers.clone(), equipment.clone(), key.clone());
        Box::pin(async move {
            tx.ensure_lock_set(&sellers, &equipment, std::slice::from_ref(&key))?;
            let ledger = LedgerCommand {
                command_id: command.command_id.as_uuid(),
                operation: ReturnOperation::ConsumeReturn,
                target: Target::Intake(command.intake_id),
                digest: command.request_hash,
            };
            match arbitrate(tx.connection(), &ledger).await? {
                Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
                Step::Done(Arbitration::Replay(body)) => {
                    return replay_receipt(tx, command, &body).await.map(Step::Done);
                }
                Step::Done(Arbitration::First) => {}
            }
            // 锁序：funding worlds 先于投递、工作等业务行（回补原语随后取 UID 账户锁）。
            lock_funding_worlds(tx.tracked()).await.map_err(funding_error)?;
            consume_in_tx(tx, command, &ledger).await
        })
    })
    .await
    .map_err(WriteError::into_application)?;
    transaction.into_result()
}

async fn replay_receipt(
    tx: &mut DepositTransaction<'_, '_>,
    command: &ConsumeReturn,
    body: &Value,
) -> Result<ConsumeReceipt, WriteError> {
    let proof_id = body
        .get("proof_id")
        .and_then(Value::as_str)
        .and_then(|text| Uuid::parse_str(text).ok())
        .ok_or_else(|| WriteError::invariant("consignment_consume_replay_body"))?;
    let intake = load_intake(tx.connection(), command.intake_id)
        .await?
        .ok_or_else(|| WriteError::invariant("consignment_consume_replay_intake"))?;
    Ok(ConsumeReceipt {
        command_id: command.command_id,
        intake_id: intake.intake_id,
        proof_id,
        leg: intake.facts.leg,
        replayed: true,
    })
}

async fn consume_in_tx(
    tx: &mut DepositTransaction<'_, '_>,
    command: &ConsumeReturn,
    ledger: &LedgerCommand<'_>,
) -> Result<Step<ConsumeReceipt>, WriteError> {
    let Some(intake) = load_intake(tx.connection(), command.intake_id).await? else {
        return Ok(Step::Rejected(Rejection::SourceMismatch));
    };
    sqlx::query("SELECT id FROM public.deliveries WHERE id = $1 FOR UPDATE")
        .bind(intake.take_delivery_id)
        .fetch_one(tx.connection())
        .await?;
    let started = load_start(tx.connection(), intake.take_work_id)
        .await?
        .map(|start| StartedRef { attempt_id: start.attempt_id, fence: start.fence });
    let existing_consumption = load_consumption(tx.connection(), &intake).await?;
    let facts = ConsumeFacts {
        intake_id: intake.intake_id,
        intake: intake.facts.clone(),
        started,
        existing_consumption,
        leg_consumed_by_other: leg_consumed_by_other(tx.connection(), &intake).await?,
        proof: read_proof(tx.connection(), &intake, command, started).await?,
        command: ConsumeCommandRef {
            attempt: StartedRef { attempt_id: command.original_attempt_id, fence: command.fence },
            validator_version: command.validator_version.clone(),
            proof_digest: command.proof_digest,
        },
    };
    match decide_consumption(&facts) {
        ConsumeDecision::Consume(plan) => {
            let started =
                started.ok_or_else(|| WriteError::invariant("consignment_not_started"))?;
            let receipt = write_consumption(tx, command, &intake, &plan, started).await?;
            insert_success(
                tx.connection(),
                ledger,
                "return_consumed",
                json!({
                    "contract_id": intake.facts.contract_id.to_string(),
                    // 合同不可变，版本恒为 1（与购买回执一致）。
                    "contract_version": 1,
                    "return_intake_id": intake.intake_id.to_string(),
                    "consumption_id": receipt.1.to_string(),
                    "proof_id": receipt.0.proof_id.to_string(),
                }),
            )
            .await?;
            // 不变量 3：消费改变订单阶段（已领回），事件与权利刷新与消费同事务；重放分支不再追加。
            let now = database_now(tx.connection()).await?;
            record_order_stage_change_in_tx(tx.connection(), intake.facts.contract_id, now).await?;
            Ok(Step::Done(receipt.0))
        }
        ConsumeDecision::AlreadyConsumed { proof_id } => {
            let body = already_body(tx.connection(), &intake, proof_id).await?;
            insert_success(tx.connection(), ledger, "return_consumed", body).await?;
            Ok(Step::Done(ConsumeReceipt {
                command_id: command.command_id,
                intake_id: intake.intake_id,
                proof_id,
                leg: intake.facts.leg,
                replayed: true,
            }))
        }
        // 证据不足保留核对：不写任何行、不占命令 ID，之后带足证据的同一接管仍可消费。
        ConsumeDecision::Reconcile(_) => Ok(Step::Rejected(Rejection::EvidenceUnproven)),
        ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch) => {
            Ok(Step::Rejected(Rejection::EvidenceConflict))
        }
        ConsumeDecision::Conflict(
            ConsumeConflict::ConsumedByOther | ConsumeConflict::LegAlreadyConsumed,
        ) => Ok(Step::Rejected(Rejection::DispositionConflict)),
    }
}

async fn already_body(
    connection: &mut PgConnection,
    intake: &IntakeRow,
    proof_id: Uuid,
) -> Result<Value, WriteError> {
    let consumption_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.consignment_return_consumptions WHERE intake_id = $1",
    )
    .bind(intake.intake_id)
    .fetch_one(connection)
    .await?;
    Ok(json!({
        "contract_id": intake.facts.contract_id.to_string(),
        "contract_version": 1,
        "return_intake_id": intake.intake_id.to_string(),
        "consumption_id": consumption_id.to_string(),
        "proof_id": proof_id.to_string(),
    }))
}

async fn load_consumption(
    connection: &mut PgConnection,
    intake: &IntakeRow,
) -> Result<Option<ExistingConsumption>, WriteError> {
    let row = sqlx::query(
        "SELECT consumption.proof_id, proof.digest
         FROM public.consignment_return_consumptions AS consumption
         JOIN public.mail_disposition_proofs AS proof ON proof.id = consumption.proof_id
         WHERE consumption.intake_id = $1
            OR (consumption.bot_uid = $2 AND consumption.world_id = $3
                AND consumption.external_mid = $4)",
    )
    .bind(intake.intake_id)
    .bind(MailKeyColumns::from_key(&intake.key).bot_uid)
    .bind(MailKeyColumns::from_key(&intake.key).world_id)
    .bind(MailKeyColumns::from_key(&intake.key).external_mid)
    .fetch_optional(&mut *connection)
    .await?;
    row.map(|row| {
        Ok(ExistingConsumption {
            intake_id: intake.intake_id,
            k: intake.key.clone(),
            proof_id: row.try_get("proof_id")?,
            proof_digest: <[u8; 32]>::try_from(row.try_get::<&[u8], _>("digest")?)
                .map_err(invariant_decoding("consignment_proof_digest"))?,
        })
    })
    .transpose()
}

async fn leg_consumed_by_other(
    connection: &mut PgConnection,
    intake: &IntakeRow,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_return_consumptions
                        WHERE contract_id = $1 AND obligation_id = $2 AND original_round = $3
                          AND leg = $4 AND intake_id <> $5)",
    )
    .bind(intake.facts.contract_id)
    .bind(intake.facts.obligation_id)
    .bind(i32::try_from(intake.facts.original_round)?)
    .bind(intake.facts.leg.as_str())
    .bind(intake.intake_id)
    .fetch_one(connection)
    .await?)
}

/// 证明事实：命令所引观察必须是同一 K 的观察，验证器、摘要齐全才算“已验证”。
///
/// 精确性（整封 Take，而非 Return 或部分）由已签收验证器出具的证明承担；本端只复核观察属于同一 K，
/// 并以验证器签收状态把关。没有观察则没有证明。
async fn read_proof(
    connection: &mut PgConnection,
    intake: &IntakeRow,
    command: &ConsumeReturn,
    started: Option<StartedRef>,
) -> Result<Option<TakeProofFacts>, WriteError> {
    let columns = MailKeyColumns::from_key(&intake.key);
    let observed: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.mail_envelope_observations
                        WHERE bot_uid = $1 AND world_id = $2 AND external_mid = $3 AND id = $4)",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(command.observation_id)
    .fetch_one(connection)
    .await?;
    let Some(attempt) = started.filter(|_| observed) else { return Ok(None) };
    Ok(Some(TakeProofFacts {
        // 证明行在消费事务里才写入；这里的 ID 只是占位，真实 ID 由落库返回。
        proof_id: Uuid::nil(),
        exact_take: true,
        validated: !command.validator_version.is_empty() && command.proof_digest != [0; 32],
        validator_status: validator_status(&command.validator_version),
        validator_version: command.validator_version.clone(),
        proof_digest: command.proof_digest,
        k: intake.key.clone(),
        attempt,
    }))
}

/// 唯一一次落库：证明 → 消费 → 实物回位或现金回补。返回回执与消费 ID。
async fn write_consumption(
    tx: &mut DepositTransaction<'_, '_>,
    command: &ConsumeReturn,
    intake: &IntakeRow,
    plan: &ConsumePlan,
    started: StartedRef,
) -> Result<(ConsumeReceipt, Uuid), WriteError> {
    let now = database_now(tx.connection()).await?;
    let proof_id = Uuid::new_v4();
    let (written, conclusion) = upsert_proof(
        tx.connection(),
        &ProofWrite {
            proof_id,
            work_id: intake.take_work_id,
            decision_id: intake.decision_id,
            delivery_id: intake.take_delivery_id,
            fence: started.fence,
            attempt_id: started.attempt_id,
            kind: MailProofKind::ExactTake,
            conclusion: ProofDisposition::TakeProven,
            observation_id: command.observation_id,
            validator_version: &command.validator_version,
            digest: command.proof_digest,
            now,
        },
    )
    .await?;
    // 证明必须是本事务新写的结论性证明；已有的旧证明不能充当本次消费的证据。
    if written != proof_id || conclusion != ProofDisposition::TakeProven {
        return Err(WriteError::invariant("consignment_consume_proof_slot"));
    }
    let consumption_id = Uuid::new_v4();
    let columns = MailKeyColumns::from_key(&intake.key);
    sqlx::query(
        "INSERT INTO public.consignment_return_consumptions
            (id, intake_id, contract_id, obligation_id, original_round, leg, bot_uid, world_id,
             external_mid, work_id, delivery_id, attempt_id, fence, proof_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(consumption_id)
    .bind(intake.intake_id)
    .bind(intake.facts.contract_id)
    .bind(intake.facts.obligation_id)
    .bind(i32::try_from(intake.facts.original_round)?)
    .bind(plan.leg.as_str())
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(intake.take_work_id)
    .bind(intake.take_delivery_id)
    .bind(started.attempt_id)
    .bind(i64::try_from(started.fence)?)
    .bind(proof_id)
    .execute(tx.connection())
    .await?;
    match plan.effect {
        ConsumeEffect::RecoverOriginal => {
            restore_position(tx.connection(), intake, command.command_id, proof_id).await?;
        }
        // 载体是 Bot 自有资产，退回不改代管位置；本金腿属于现金腿。
        ConsumeEffect::RecoverCarrier => {}
        ConsumeEffect::ReturnCash => {
            let leg = intake
                .facts
                .leg
                .blocked_leg()
                .ok_or_else(|| WriteError::invariant("consignment_cash_leg"))?;
            let amount = round_credits(tx.connection(), intake).await?;
            return_cash_in_tx(
                tx.tracked(),
                &ReturnConsumptionFacts {
                    mutation: FundingMutation {
                        command_id: command.command_id.as_uuid(),
                        request_digest: command.request_hash.as_bytes(),
                    },
                    contract_id: intake.facts.contract_id,
                    intake_id: intake.intake_id,
                    consumption_id,
                    leg,
                    original_delivery_id: intake.facts.original_attempt.delivery_id,
                    amount,
                    proof_digest: command.proof_digest,
                },
            )
            .await
            .map_err(funding_error)?;
        }
    }
    Ok((
        ConsumeReceipt {
            command_id: CommandId::from_uuid(command.command_id.as_uuid()),
            intake_id: intake.intake_id,
            proof_id,
            leg: intake.facts.leg,
            replayed: false,
        },
        consumption_id,
    ))
}

/// 该轮冻结的 Credits（现金腿的回补金额）。
async fn round_credits(
    connection: &mut PgConnection,
    intake: &IntakeRow,
) -> Result<i64, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT credits FROM public.consignment_dispatch_rounds
         WHERE contract_id = $1 AND obligation_id = $2 AND round = $3",
    )
    .bind(intake.facts.contract_id)
    .bind(intake.facts.obligation_id)
    .bind(i32::try_from(intake.facts.original_round)?)
    .fetch_one(connection)
    .await?)
}

/// 原件腿消费：Sale 的 position `in_transit → committed`（买方合同仍在），撤回件（PIWX）的
/// `in_transit → in_custody`（原件回库、无买方），并追加同版本代管事件；其他前态失败关闭。
///
/// 保护不变量 3、6、8：状态推进与不可变事件同事务，触发器再核同事务存在该合同原件腿的消费与合同种类。
async fn restore_position(
    connection: &mut PgConnection,
    intake: &IntakeRow,
    command_id: CommandId,
    take_proof_id: Uuid,
) -> Result<(), WriteError> {
    let position: Option<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT position.id, position.listing_id, position.state, contract.kind
         FROM public.consignment_contracts AS contract
         JOIN public.consignment_custody_positions AS position ON position.id = contract.position_id
         WHERE contract.contract_id = $1 FOR UPDATE OF position",
    )
    .bind(intake.facts.contract_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((position_id, listing_id, state, kind)) = position else {
        return Err(WriteError::invariant("consignment_consume_position_missing"));
    };
    // 只有 `in_transit` 回位合法（R5 C2）：其他前态（如 reconcile）不静默放过，
    // 整笔消费回滚交人工核对（不变量 6）。回到哪里由合同种类决定；未知种类同样失败关闭。
    // 回库事件（in_custody）必须引用证明：撤回件以本次精确 Take 证明为据；Sale 的 committed 事件不带证明。
    let (to_state, event_proof) = match kind.as_str() {
        "sale" => ("committed", None),
        "withdrawal" => ("in_custody", Some(take_proof_id)),
        _ => return Err(WriteError::invariant("consignment_consume_contract_kind")),
    };
    if state != "in_transit" {
        return Err(WriteError::invariant("consignment_consume_position_state"));
    }
    let version: i64 = sqlx::query_scalar(
        "UPDATE public.consignment_custody_positions
         SET state = $2, version = version + 1, updated_at = statement_timestamp()
         WHERE id = $1 AND state = 'in_transit' RETURNING version",
    )
    .bind(position_id)
    .bind(to_state)
    .fetch_one(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         SELECT $1, $2, $3, $5, 'in_transit', $5, $6, $4, state_version,
                statement_timestamp()
         FROM public.consignment_listings WHERE id = $4",
    )
    .bind(position_id)
    .bind(version)
    .bind(command_id.as_uuid())
    .bind(listing_id)
    .bind(to_state)
    .bind(event_proof)
    .execute(&mut *connection)
    .await?;
    Ok(())
}
