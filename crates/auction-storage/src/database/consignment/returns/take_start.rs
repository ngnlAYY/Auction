//! Take 起写：与第 4 期同一起写原语（`mail_disposition_write_starts`），现金腿同事务建立 UID 阻断。
//!
//! 保护不变量 4、5：同一 intake 及其唯一 Take 工作各自最多一条成功起写事实，由
//! `mail_disposition_write_starts PK(work_id)` 独立于命令 ID、attempt 与 fence 保证；只有首次
//! 提交的执行者可 POST，其余（新 fence、新命令 ID、重放）只得“已提交”。现金腿的 UID opening
//! 阻断与起写同事务提交：Take 起写成功就一定有阻断，阻断建立失败则起写一并回滚。

use auction_application::consignment::{
    ConsignmentError, ReturnOperation, StartReceipt, StartReturnTake, WriteStartOutcome,
};
use auction_domain::CommandId;
use auction_domain::consignment::funding::{FundingMode, OpeningBlock};
use auction_domain::consignment::returns::{
    ClaimFacts, StartedRef, TakeStartDecision, TakeStartFacts, TakeStartRejection,
    TakeStartRequest, decide_take_start,
};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::conflicts::funding_error;
use super::intake::{IntakeRow, load_intake};
use super::ledger::{
    Arbitration, LedgerCommand, Rejection, Step, Target, arbitrate, insert_rejection,
    insert_success,
};
use super::locks::pre_read_by_intake;
use crate::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::mail::originals::invariant_decoding;
use crate::database::mail::originals::proofs::{
    insert_start, load_attempt, load_start, load_work_for_update,
};
use crate::database::settlement::funding::consignment::blockers::open_cash_return_block_in_tx;
use crate::database::settlement::funding::lock_funding_worlds;

/// 起写端口实现：预读锁集合后在一个顶层 Serializable 事务内完成。
pub(super) async fn start_return_take(
    store: &DatabaseStore,
    command: StartReturnTake,
) -> Result<WriteStartOutcome, ConsignmentError> {
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
                operation: ReturnOperation::StartReturnTake,
                target: Target::Intake(command.intake_id),
                digest: command.request_hash,
            };
            match arbitrate(tx.connection(), &ledger).await? {
                Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
                Step::Done(Arbitration::Replay(body)) => {
                    return Ok(Step::Done(StartOutcome::Already(receipt_of(
                        command.command_id,
                        &body,
                    )?)));
                }
                Step::Done(Arbitration::First) => {}
            }
            // 锁序：funding worlds 先于投递、工作等业务行（现金腿的阻断原语在其后取 UID 账户锁）。
            lock_funding_worlds(tx.tracked()).await.map_err(funding_error)?;
            start_in_tx(tx, command, &ledger).await
        })
    })
    .await
    .map_err(WriteError::into_application)?;
    match transaction.into_result()? {
        StartOutcome::First(receipt) => Ok(WriteStartOutcome::FirstCommitted(receipt)),
        StartOutcome::Already(receipt) => Ok(WriteStartOutcome::AlreadyCommitted(receipt)),
    }
}

/// 事务内的起写结果；与端口的 `WriteStartOutcome` 同形，另带账本拒绝通道。
enum StartOutcome {
    First(StartReceipt),
    Already(StartReceipt),
}

/// 重放：原成功回执只读，不再起写、不发许可。
fn receipt_of(command_id: CommandId, body: &Value) -> Result<StartReceipt, WriteError> {
    let uuid = |field: &'static str| -> Result<Uuid, WriteError> {
        body.get(field)
            .and_then(Value::as_str)
            .and_then(|text| Uuid::parse_str(text).ok())
            .ok_or_else(|| WriteError::invariant("consignment_start_replay_body"))
    };
    let fence = body
        .get("fence")
        .and_then(Value::as_u64)
        .ok_or_else(|| WriteError::invariant("consignment_start_replay_body"))?;
    Ok(StartReceipt::started(
        command_id,
        uuid("decision_id")?,
        uuid("work_id")?,
        uuid("attempt_id")?,
        fence,
    ))
}

async fn start_in_tx(
    tx: &mut crate::database::consignment::commands::session::DepositTransaction<'_, '_>,
    command: &StartReturnTake,
    ledger: &LedgerCommand<'_>,
) -> Result<Step<StartOutcome>, WriteError> {
    let Some(intake) = load_intake(tx.connection(), command.intake_id).await? else {
        return insert_rejection(tx.connection(), ledger, Rejection::SourceMismatch).await;
    };
    let work = load_work_for_update(tx.connection(), intake.take_work_id)
        .await?
        .ok_or_else(|| WriteError::invariant("consignment_intake_work_missing"))?;
    // 请求自称的工作与投递必须就是该 intake 唯一 Take 工作绑定的那一个。
    if work.work_id != command.work_id
        || work.delivery_id != command.delivery_id
        || work.delivery_id != intake.take_delivery_id
        || work.decision_id != intake.decision_id
    {
        return insert_rejection(tx.connection(), ledger, Rejection::SourceMismatch).await;
    }
    sqlx::query("SELECT id FROM public.deliveries WHERE id = $1 FOR UPDATE")
        .bind(work.delivery_id)
        .fetch_one(tx.connection())
        .await?;
    let now = database_now(tx.connection()).await?;
    let started = load_start(tx.connection(), work.work_id)
        .await?
        .map(|start| StartedRef { attempt_id: start.attempt_id, fence: start.fence });
    let claim = read_claim(tx.connection(), &intake, command).await?;
    let facts = TakeStartFacts {
        intake_open: !consumed(tx.connection(), intake.intake_id).await?,
        // intake 与其唯一工作共用同一张起写表（PK work_id），两侧读到的是同一条事实。
        intake_start: started,
        work_start: started,
        opposite_action_present: opposite_action(tx.connection(), &intake).await?,
        claim,
        request: TakeStartRequest {
            attempt_id: command.attempt_id,
            fence: command.fence,
            claim_token: command.claim_token,
            session_binding_digest: command.session_binding_digest,
            expected_manifest_digest: command.expected_manifest_digest,
        },
    };
    match decide_take_start(&facts) {
        TakeStartDecision::Already(started) => {
            // 已有成功起写：只核对原 attempt，不写任何行、不发许可。
            let start = load_start(tx.connection(), work.work_id)
                .await?
                .ok_or_else(|| WriteError::invariant("consignment_start_missing"))?;
            Ok(Step::Done(StartOutcome::Already(StartReceipt::started(
                CommandId::from_uuid(start.command_id),
                intake.decision_id,
                work.work_id,
                started.attempt_id,
                started.fence,
            ))))
        }
        TakeStartDecision::Reject(reason) => {
            insert_rejection(tx.connection(), ledger, rejection_of(reason)).await
        }
        TakeStartDecision::First => {
            insert_start(
                tx.connection(),
                work.work_id,
                intake.decision_id,
                work.delivery_id,
                command.attempt_id,
                command.fence,
                command.command_id.as_uuid(),
                now,
            )
            .await?;
            if let Some(block) = cash_block(tx.connection(), &intake).await? {
                open_cash_return_block_in_tx(tx.tracked(), &block).await.map_err(funding_error)?;
            }
            let receipt = StartReceipt::started(
                command.command_id,
                intake.decision_id,
                work.work_id,
                command.attempt_id,
                command.fence,
            );
            insert_success(
                tx.connection(),
                ledger,
                "write_started",
                json!({
                    "return_intake_id": intake.intake_id.to_string(),
                    "work_id": work.work_id.to_string(),
                    "decision_id": intake.decision_id.to_string(),
                    "attempt_id": command.attempt_id.to_string(),
                    "fence": command.fence,
                }),
            )
            .await?;
            Ok(Step::Done(StartOutcome::First(receipt)))
        }
    }
}

/// 确定拒绝的持久码；只占该次尝试的命令 ID。
const fn rejection_of(reason: TakeStartRejection) -> Rejection {
    match reason {
        TakeStartRejection::StartedInconsistent => Rejection::EvidenceConflict,
        TakeStartRejection::IntakeNotOpen | TakeStartRejection::OppositeAction => {
            Rejection::ActionNotAllowed
        }
        TakeStartRejection::FenceMismatch => Rejection::FenceMismatch,
        TakeStartRejection::LeaseLost => Rejection::LeaseLost,
        TakeStartRejection::SessionMismatch => Rejection::EvidenceUnproven,
        TakeStartRejection::ManifestMismatch => Rejection::SourceMismatch,
    }
}

/// 锁后读当前领取：投递的 fence、token、租约，与最新 attempt 的会话绑定。
async fn read_claim(
    connection: &mut PgConnection,
    intake: &IntakeRow,
    command: &StartReturnTake,
) -> Result<ClaimFacts, WriteError> {
    let delivery = sqlx::query(
        "SELECT status, lease_token, fencing_version,
                COALESCE(lease_expires_at > clock_timestamp(), false) AS lease_active
         FROM public.deliveries WHERE id = $1",
    )
    .bind(intake.take_delivery_id)
    .fetch_one(&mut *connection)
    .await?;
    let status: String = delivery.try_get("status")?;
    let lease_token: Option<Uuid> = delivery.try_get("lease_token")?;
    let lease_active: bool = delivery.try_get("lease_active")?;
    let current_fence = u64::try_from(delivery.try_get::<i64, _>("fencing_version")?)?;
    // 当前 attempt 是该投递 fence 最大的那一条；请求的 attempt 对不上就是旧领取。
    let current: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, session_id FROM public.mail_disposition_attempts
         WHERE delivery_id = $1 ORDER BY fence DESC LIMIT 1",
    )
    .bind(intake.take_delivery_id)
    .fetch_optional(&mut *connection)
    .await?;
    let (current_attempt_id, session_id) = current.unwrap_or((Uuid::nil(), Uuid::nil()));
    let requested = load_attempt(connection, command.attempt_id).await?;
    // 请求的 attempt 必须属于该工作与投递，且冻结摘要与请求一致；否则按“不是当前领取”处理。
    let attempt_matches = requested.as_ref().is_some_and(|attempt| {
        attempt.work_id == intake.take_work_id
            && attempt.delivery_id == intake.take_delivery_id
            && attempt.fence == command.fence
            && attempt.expected_digest == command.expected_manifest_digest
    });
    let binding: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT binding_digest FROM public.mail_evidence_sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_optional(&mut *connection)
    .await?;
    let session_binding_digest =
        binding.as_deref().and_then(|bytes| <[u8; 32]>::try_from(bytes).ok()).unwrap_or([0; 32]);
    Ok(ClaimFacts {
        current_fence,
        current_attempt_id: if attempt_matches { current_attempt_id } else { Uuid::nil() },
        current_claim_token: lease_token.unwrap_or_else(Uuid::nil),
        lease_live: status == "claimed" && lease_active,
        session_binding_digest,
        frozen_manifest_digest: intake.facts.frozen_manifest_digest,
    })
}

async fn consumed(connection: &mut PgConnection, intake_id: Uuid) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_return_consumptions WHERE intake_id = $1)",
    )
    .bind(intake_id)
    .fetch_one(connection)
    .await?)
}

/// 该 K 上是否已有相反动作（Return 工作）；寄售接管的 K 只可能有一个 Take 工作。
async fn opposite_action(
    connection: &mut PgConnection,
    intake: &IntakeRow,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.mail_disposition_work
                        WHERE decision_id = $1 AND action <> 'take')",
    )
    .bind(intake.decision_id)
    .fetch_one(connection)
    .await?)
}

/// 现金腿的 UID opening 阻断；实物腿没有。金额取该轮冻结的 Credits。
async fn cash_block(
    connection: &mut PgConnection,
    intake: &IntakeRow,
) -> Result<Option<OpeningBlock>, WriteError> {
    let Some(leg) = intake.facts.leg.blocked_leg() else { return Ok(None) };
    let (uid, mode, credits): (String, String, i64) = sqlx::query_as(
        "SELECT sale.funding_uid, sale.funding_mode, round.credits
         FROM public.consignment_sales AS sale
         JOIN public.consignment_dispatch_rounds AS round
           ON round.contract_id = sale.contract_id
         WHERE sale.contract_id = $1 AND round.obligation_id = $2 AND round.round = $3",
    )
    .bind(intake.facts.contract_id)
    .bind(intake.facts.obligation_id)
    .bind(i32::try_from(intake.facts.original_round)?)
    .fetch_one(connection)
    .await?;
    let mode = match mode.as_str() {
        "strict_cash" => FundingMode::StrictCash,
        "record_only" => FundingMode::RecordOnly,
        _ => return Err(WriteError::invariant("consignment_funding_mode")),
    };
    OpeningBlock::new(
        uid,
        intake.intake_id,
        leg,
        intake.facts.original_attempt.delivery_id,
        credits,
        mode,
    )
    .map(Some)
    .map_err(invariant_decoding("consignment_cash_block"))
}
