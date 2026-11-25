//! 寄售 RTS 的接管接线：整封首次决定与隔离解除在同一顶层事务交退件接管写端，锁集合按原轮预读买家。
//!
//! 保护不变量 2、3、4、6：
//! - 接管同事务写返回事实（可能固定处罚），所以成交买家必须在会话锁集合里（命令 → 买家 → 实物 → K）；
//!   预读只决定取哪些锁，锁内由接管写端按持久事实重读重判；
//! - 锁内复判为“只隔离”或同腿第二个 MID 时整封隔离、零资产动作；其余矛盾失败关闭、不留半写行。

use std::collections::BTreeSet;

use auction_application::consignment::{
    ConsignmentError, ConsignmentRtsLocator, DecideEnvelopeCommand, DepositBusinessRejection,
    EnvelopeDisposition, EnvelopeReceipt, ReleaseQuarantineCommand, ReturnIntakeReference,
};
use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    EnvelopeDecision, EnvelopeObservation, QuarantineReason, ReleaseKind, TrustedConsignmentRts,
};
use auction_domain::consignment::returns::AcquireConflict;
use auction_domain::consignment::{
    ListingEquipmentIdentity, OriginalMailKey, settlement_world_code,
};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::conflict;
use super::first_quarantine::quarantine_envelope_in_tx;
use super::observations::StoredObservation;
#[cfg(feature = "test-db")]
use super::observations::{load_observation, observation_from_stored};
use super::quarantine::{envelope_equipment, envelope_sellers};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::returns::{
    AcquireOutcome, AcquireRequest, LockPlan, RoundLocator, acquire_in_tx, pre_read_by_round,
    pre_read_by_round_id,
};

/// 整封首次决定的锁集合；接管分支另加原轮合同的买家与实物。
///
/// 直接决定与 poll 同事务决定共用：接管同事务写返回事实（可能固定处罚），成交买家必须在会话锁里
/// （不变量 2，锁序 命令 → 买家 → 实物 → K）。
pub(crate) async fn envelope_lock_set(
    store: &DatabaseStore,
    command: &DecideEnvelopeCommand,
) -> Result<(Vec<Uuid>, Vec<ListingEquipmentIdentity>), ConsignmentError> {
    let mut sellers = envelope_sellers(&command.facts);
    let mut equipment = envelope_equipment(&command.facts);
    if let EnvelopeDecision::AcquireReturn(rts) = &command.decision {
        let mut pre = store
            .pool()
            .acquire()
            .await
            .map_err(|error| WriteError::from(error).into_application())?;
        let plan = pre_read_by_round(&mut pre, rts.obligation_id, rts.original_round)
            .await
            .map_err(WriteError::into_application)?;
        merge(&mut sellers, &mut equipment, plan);
    }
    Ok((sellers, equipment))
}

/// 隔离解除的锁集合：映射出的卖家与观察里的实物；拟定接管时另加 RTS 派发引用所指原轮的买家与实物。
pub(crate) async fn release_lock_set(
    pre: &mut PgConnection,
    command: &ReleaseQuarantineCommand,
    key: &OriginalMailKey,
    observation: &StoredObservation,
    mapped_seller: Option<UserId>,
) -> Result<(Vec<Uuid>, Vec<ListingEquipmentIdentity>), WriteError> {
    let mut sellers: Vec<Uuid> = mapped_seller.map(UserId::as_uuid).into_iter().collect();
    let world = key.world();
    let mut equipment: Vec<ListingEquipmentIdentity> = observation
        .items
        .iter()
        .filter_map(|item| {
            item.canonical_eid.as_ref().and_then(|eid| {
                ListingEquipmentIdentity::new(
                    auction_domain::consignment::settlement_world_code(item.world.unwrap_or(world)),
                    eid,
                )
                .ok()
            })
        })
        .collect();
    let round_id = observation
        .legacy_return_evidence
        .dispatch_reference
        .as_deref()
        .and_then(|reference| Uuid::parse_str(reference).ok());
    if let (ReleaseKind::AcquireReturn, Some(round_id)) = (command.proposed_kind, round_id) {
        let plan = pre_read_by_round_id(pre, round_id).await?;
        merge(&mut sellers, &mut equipment, plan);
    }
    Ok((sellers, equipment))
}

/// 把预读到的买家与实物并入锁集合（会话锁协议按集合去重排序）。
fn merge(
    sellers: &mut Vec<Uuid>,
    equipment: &mut Vec<ListingEquipmentIdentity>,
    plan: Option<LockPlan>,
) {
    let Some(plan) = plan else { return };
    let mut users: BTreeSet<Uuid> = sellers.drain(..).collect();
    users.extend(plan.sellers());
    sellers.extend(users);
    equipment.extend(plan.equipment());
}

/// poll 预读：按系统 RTS 的寄售线索在派发轮里唯一定位原轮；读不出、不唯一或观察不是系统 RTS 都没有来源。
///
/// 只用于定位与取锁（不变量 6）：派发引用必须是该义务在同一 Bot、同一世界的轮 ID；没有引用时该义务在
/// 这个 Bot 与世界恰有一轮才定位。来源是否成立仍由接管写端在锁内按原 Sent、腿与冻结清单复判。
pub(crate) async fn locate_consignment_rts(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    observation: &EnvelopeObservation,
    locator: Option<&ConsignmentRtsLocator>,
) -> Result<Option<TrustedConsignmentRts>, WriteError> {
    let Some(locator) = locator else { return Ok(None) };
    if !observation.system_rts || observation.legacy_auction_owner.is_some() {
        return Ok(None);
    }
    // 引用原文不是规范轮 ID 时不猜测，按来源不明处理。
    let round_id = match locator.dispatch_reference.as_deref() {
        Some(reference) => match Uuid::parse_str(reference) {
            Ok(round_id) if round_id.to_string() == reference => Some(round_id),
            _ => return Ok(None),
        },
        None => None,
    };
    let rows = sqlx::query(
        "SELECT round FROM public.consignment_dispatch_rounds
         WHERE obligation_id = $1 AND bot_uid = $2 AND world_id = $3
           AND ($4::uuid IS NULL OR round_id = $4)",
    )
    .bind(locator.obligation_id)
    .bind(key.authenticated_bot_uid())
    .bind(settlement_world_code(key.world()))
    .bind(round_id)
    .fetch_all(&mut *connection)
    .await?;
    let [row] = rows.as_slice() else { return Ok(None) };
    Ok(Some(TrustedConsignmentRts {
        obligation_id: locator.obligation_id,
        original_round: u32::try_from(row.try_get::<i32, _>("round")?)?,
    }))
}

/// 整封首次决定为接管：同事务调用接管写端，成功回执与直接接管的 `return_acquired` 同形。
pub(crate) async fn acquire_envelope_in_tx(
    writer: &mut DepositWriter<'_, '_, '_>,
    command: &DecideEnvelopeCommand,
    stored: &StoredObservation,
    rts: &TrustedConsignmentRts,
    now: Timestamp,
) -> Result<EnvelopeReceipt, WriteError> {
    let command_id = writer.command_id();
    let outcome = acquire_in_tx(
        writer.connection(),
        &AcquireRequest {
            key: &command.mail_key,
            observation_id: stored.observation_id,
            locator: RoundLocator::ByObligationRound {
                obligation_id: rts.obligation_id,
                round: rts.original_round,
            },
            command_id,
            write_initial_decision: true,
        },
    )
    .await?;
    match outcome {
        AcquireOutcome::Acquired(acquired) => {
            let mut receipt = EnvelopeReceipt::new(
                command.command_id,
                EnvelopeDisposition::ReturnAcquired,
                acquired.decision_id,
            );
            receipt.return_intake = Some(ReturnIntakeReference {
                return_intake_id: acquired.intake_id,
                original_round: acquired.facts.original_round,
                take_work_id: acquired.take_work_id,
                take_delivery_id: acquired.take_delivery_id,
            });
            Ok(receipt)
        }
        // 原身份或轮不明、清单不符：只隔离（D9 §2.2）。同腿第二个 MID 的冲突案行已由接管写端追加，
        // 整封同样隔离待 Admin 处理；两者都不写 intake、零资产动作。
        AcquireOutcome::Quarantine(reason) => {
            tracing::info!(?reason, "寄售 RTS 锁内复判不接管，整封隔离");
            let reason = QuarantineReason::SuspectedSystemRts;
            quarantine_envelope_in_tx(writer, command, stored, reason, now).await
        }
        AcquireOutcome::Conflict(AcquireConflict::SameLegSecondMid) => {
            tracing::warn!("寄售 RTS 主张已被另一封接管的同一腿，整封隔离");
            let reason = QuarantineReason::SuspectedSystemRts;
            quarantine_envelope_in_tx(writer, command, stored, reason, now).await
        }
        // 首次决定时 K 已有接管或别的终局：事实互相矛盾，失败关闭、整笔回滚。
        AcquireOutcome::Replay(_) | AcquireOutcome::Conflict(_) => {
            Err(conflict(DepositBusinessRejection::DispositionConflict))
        }
    }
}

#[cfg(feature = "test-db")]
impl crate::database::DatabaseStore {
    /// PG 测试入口：按库内原样还原整封观察（锁外只读），供测试构造与之逐项一致的整封决定命令。
    pub async fn test_stored_envelope_observation(
        &self,
        key: &OriginalMailKey,
        observation_id: Uuid,
    ) -> Result<Option<auction_domain::consignment::deposit::EnvelopeObservation>, ConsignmentError>
    {
        let mut connection = self
            .pool()
            .acquire()
            .await
            .map_err(|error| WriteError::from(error).into_application())?;
        let stored = load_observation(&mut connection, key, observation_id)
            .await
            .map_err(WriteError::into_application)?;
        Ok(stored.as_ref().map(|stored| observation_from_stored(stored, None)))
    }
}
