//! 按 RTS 的 K 接管：锁内复判，同事务写原信终局、唯一 Take 工作与投递、intake。
//!
//! 保护不变量 3、4、6、8：K 的终局槽、intake 与 Take 工作互相指认，唯一键独立于命令 ID；
//! 来源、轮、清单与同腿唯一由领域 `decide_acquire_return` 在锁内按持久事实复判，调用方自称的
//! 事实只用来定位，不被信任；任何矛盾都不接管、不留半写行。
//!
//! intake 写完之后同一事务内：提交返回事实（`refusal::submit_acquired_return_fact`，以 intake 为外键，
//! 须在它之后）、达阈值时写 `open_sanction` 回执与审计、追加订单阶段事件并刷新权利（不变量 3）。
//! 三个调用方（直接接管、整封首次决定、隔离解除）都必须已把成交买家放进会话锁集合。

use auction_application::consignment::{DepositBusinessRejection, DispositionAction};
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::returns::{
    AcquireConflict, AcquirePlan, AcquireQuarantine, LockedAcquireFacts, ReturnIntakeFacts,
    decide_acquire_return, return_intake_id,
};
use sqlx::PgConnection;
use uuid::Uuid;

use super::intake::{
    IntakeRow, RoundLocator, existing_of, k_final, load_intake_by_key, read_origin,
    same_leg_other_k,
};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;
use crate::database::consignment::recovery::record_open_sanction;
use crate::database::consignment::refusal::submit_acquired_return_fact;
use crate::database::mail::originals::disposition::{
    WorkSeed, create_work_in_tx, delivery_identity, insert_final_decision, insert_initial_decision,
};
use crate::database::mail::originals::identity::MailKeyColumns;
use crate::database::mail::originals::observations::load_observation;
use crate::database::mail::originals::{conflict, invariant};

/// 接管请求：只含定位信息与命令身份，其余事实全部锁内重读。
pub(crate) struct AcquireRequest<'a> {
    /// RTS 的稳定原信键。
    pub(crate) key: &'a OriginalMailKey,
    /// 所引的 RTS 观察。
    pub(crate) observation_id: Uuid,
    /// 如何定位被退回的那一轮。
    pub(crate) locator: RoundLocator,
    /// 写入终局处置的命令 ID（直接接管是自己的命令，隔离解除是解除命令）。
    pub(crate) command_id: Uuid,
    /// 直接接管同时写首次决定；隔离解除的首次决定（隔离）早已存在。
    pub(crate) write_initial_decision: bool,
}

/// 一次接管的落库结果。
#[derive(Debug, Clone)]
pub(crate) struct AcquiredIntake {
    /// 接管 ID。
    pub(crate) intake_id: Uuid,
    /// 原信终局处置 ID。
    pub(crate) decision_id: Uuid,
    /// 唯一 Take 工作。
    pub(crate) take_work_id: Uuid,
    /// Take 投递。
    pub(crate) take_delivery_id: Uuid,
    /// 接管事实。
    pub(crate) facts: ReturnIntakeFacts,
}

impl From<IntakeRow> for AcquiredIntake {
    fn from(row: IntakeRow) -> Self {
        Self {
            intake_id: row.intake_id,
            decision_id: row.decision_id,
            take_work_id: row.take_work_id,
            take_delivery_id: row.take_delivery_id,
            facts: row.facts,
        }
    }
}

/// 接管判定的落库结果；隔离与冲突不写任何接管行。
pub(crate) enum AcquireOutcome {
    /// 本事务首次接管。
    Acquired(AcquiredIntake),
    /// 同 K 同事实已接管过，没有新增行。
    Replay(AcquiredIntake),
    /// 只隔离、零资产动作。
    Quarantine(AcquireQuarantine),
    /// 冲突案；第二个不同 MID 已记入冲突表。
    Conflict(AcquireConflict),
}

/// 事务内接管；调用方已持有命令、买家、实物与 K 的会话锁并在 Serializable 事务内。
pub(crate) async fn acquire_in_tx(
    connection: &mut PgConnection,
    request: &AcquireRequest<'_>,
) -> Result<AcquireOutcome, WriteError> {
    let key = request.key;
    let observation = load_observation(connection, key, request.observation_id)
        .await?
        .ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
    let origin = read_origin(connection, request.locator, key, &observation).await?;
    let existing = load_intake_by_key(connection, key).await?;
    let other_leg_holder = match &origin {
        Some(read) => same_leg_other_k(connection, &read.origin, key).await?,
        None => None,
    };
    let facts = LockedAcquireFacts {
        k: key.clone(),
        rts_observation_id: observation.observation_id,
        rts_returned_at: observation.received_at,
        // 观察必须是系统 RTS 且没有被识别为旧拍卖 owner；来源的其余证明由 read_origin 的持久事实承担。
        trusted_rts: observation.system_rts && observation.legacy_auction_owner.is_none(),
        observed_manifest_digest: origin.as_ref().and_then(|read| read.observed_manifest_digest),
        origin: origin.map(|read| read.origin),
        existing_for_k: existing.as_ref().map(existing_of),
        k_final: k_final(connection, key).await?,
        same_leg_other_k: other_leg_holder.as_ref().map(|(_, other)| other.clone()),
    };
    match decide_acquire_return(&facts) {
        AcquirePlan::Acquire(intake) => {
            let acquired = write_intake(connection, request, &observation, &intake).await?;
            record_intake_effects(connection, &acquired).await?;
            Ok(AcquireOutcome::Acquired(acquired))
        }
        AcquirePlan::Replay => {
            let existing = existing.ok_or_else(|| invariant("consignment_acquire_replay_row"))?;
            Ok(AcquireOutcome::Replay(existing.into()))
        }
        AcquirePlan::Quarantine(reason) => Ok(AcquireOutcome::Quarantine(reason)),
        AcquirePlan::Conflict(reason) => {
            if let (AcquireConflict::SameLegSecondMid, Some((holder, _))) =
                (reason, &other_leg_holder)
            {
                super::conflicts::record_second_mid(
                    connection,
                    key,
                    *holder,
                    observation.observation_id,
                )
                .await?;
            }
            Ok(AcquireOutcome::Conflict(reason))
        }
    }
}

/// 接管行之后的同事务效果：返回事实（达阈值时连同 `open_sanction` 回执）与订单阶段事件。
///
/// 只在首次接管时执行；重放不再追加任何事实或事件（不变量 1、3）。
async fn record_intake_effects(
    connection: &mut PgConnection,
    acquired: &AcquiredIntake,
) -> Result<(), WriteError> {
    if let Some(opened) = submit_acquired_return_fact(connection, acquired.intake_id).await? {
        record_open_sanction(connection, &opened).await?;
    }
    let now = database_now(connection).await?;
    record_order_stage_change_in_tx(connection, acquired.facts.contract_id, now).await
}

/// 写入顺序受外键与延迟约束决定：首次决定 → 终局处置 → 投递与工作 → intake。
async fn write_intake(
    connection: &mut PgConnection,
    request: &AcquireRequest<'_>,
    observation: &crate::database::mail::originals::observations::StoredObservation,
    intake: &ReturnIntakeFacts,
) -> Result<AcquiredIntake, WriteError> {
    let key = request.key;
    let now = database_now(connection).await?;
    let decision_id = Uuid::new_v4();
    let take_work_id = Uuid::new_v4();
    let intake_id = return_intake_id(key);
    if request.write_initial_decision {
        insert_initial_decision(
            connection,
            key,
            decision_id,
            observation.observation_id,
            request.command_id,
            "acquire_return",
            None,
            now,
        )
        .await?;
    }
    insert_final_decision(
        connection,
        key,
        decision_id,
        "acquire_return",
        observation.observation_id,
        request.command_id,
        now,
    )
    .await?;
    let (take_delivery_id, delivery_command_id) =
        delivery_identity(decision_id, DispositionAction::Take);
    // 主题沿寄售处置的固定值；RTS 的真实主题没有持久列（S2-8 定论：不加列）。Take 的证明只按冻结清单摘要与
    // 原信键比对，不比对主题，执行器不得以这里的主题判定 Take 是否成立。
    create_work_in_tx(
        connection,
        &WorkSeed {
            work_id: take_work_id,
            delivery_id: take_delivery_id,
            delivery_command_id,
            decision_id,
            action: DispositionAction::Take,
            mail_key: key,
            observation,
            subject: "consign",
            return_manifest: None,
            now,
        },
    )
    .await?;
    let columns = MailKeyColumns::from_key(key);
    sqlx::query(
        "INSERT INTO public.consignment_return_intakes
            (id, bot_uid, world_id, external_mid, decision_id, decision_kind, contract_id,
             obligation_id, original_round, original_delivery_id, original_fence, sent_mid, leg,
             rts_observation_id, rts_returned_at, manifest_digest, session_id, take_work_id,
             take_delivery_id)
         VALUES ($1, $2, $3, $4, $5, 'acquire_return', $6, $7, $8, $9, $10, $11, $12, $13, $14,
                 $15, $16, $17, $18)",
    )
    .bind(intake_id)
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(decision_id)
    .bind(intake.contract_id)
    .bind(intake.obligation_id)
    .bind(i32::try_from(intake.original_round)?)
    .bind(intake.original_attempt.delivery_id)
    .bind(i64::try_from(intake.original_attempt.fence)?)
    .bind(&intake.sent_mid)
    .bind(intake.leg.as_str())
    .bind(intake.rts_observation_id)
    .bind(intake.rts_returned_at)
    .bind(intake.frozen_manifest_digest.as_slice())
    .bind(intake.session_id)
    .bind(take_work_id)
    .bind(take_delivery_id)
    .execute(&mut *connection)
    .await?;
    Ok(AcquiredIntake {
        intake_id,
        decision_id,
        take_work_id,
        take_delivery_id,
        facts: intake.clone(),
    })
}
