//! 灭失确认的事务体：当前代无买方权责时，position 办结为 `lost`、挂单关闭、建案与卖家回执同提。
//!
//! 保护不变量 2、3、6、8：调用方已持有命令、卖家与实物锁并完成账本仲裁；本函数在锁内复验 position
//! 版本、状态与挂单状态，交 `route_position_loss` 分流。只有“当前代既无有效买方交割权也无资金责任”才写：
//! 唯一损失事实、`lost` 事件、position 迁移、占用释放、人工案与卖家回执、挂单 `closed/loss_confirmed`、
//! 原身份通知与回执同一事务提交。处分代指针始终留在确认时的当前代（不新增代、不动旧 Sale、不动旧
//! 买家资金，零退款零赔款）；当前代仍有买方权责拒绝并改走原 Sale resolve；无法查清则整事务回滚，
//! 保留占用与买卖双方责任，不做任何状态迁移：按确定拒绝 `evidence_conflict` 记账本与审计，并同事务写一条
//! 按 position 持久的核对记录（`consignment_loss_reconciles`）作为核对案，另加运维告警信号作提醒（见 `reconcile`）。
//!
//! 管理员的私有 reason 写入审计 metadata（`loss_reason`），不进回执与 DTO。
//!
//! 本命令不改变任何订单阶段：Sale 与买家资金原样不动，故不写订单阶段事件（6E2）。写入的 `lost` 事件、
//! 事实与案件由 000137 的触发器在同事务内核对；`lost` 释放占用走 7M-2 的守卫分支。

use auction_application::consignment::{
    ConfirmPositionLossCommand, ConsignmentError, DepositBusinessRejection, PositionLossReceipt,
};
use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::recovery::{LossRoute, LossRoutingError, route_position_loss};
use auction_domain::consignment::{ListingReason, ListingState};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::case::{LossFactRef, OpenedLossCase, open_loss_case};
use super::reconcile::{raise_reconcile_signal, record_reconcile};
use super::routing::{LossPosition, lock_loss_position, proof_complete, routing_facts};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::{DepositRejection, DepositWriter};
use crate::database::consignment::deposit::primitives::{database_now, lock_listing, transition};

/// 同一事务内已落库的成功回执或确定拒绝。
pub(super) enum ConfirmOutcome {
    Accepted(PositionLossReceipt),
    Rejected(DepositBusinessRejection),
}

async fn reject(
    writer: DepositWriter<'_, '_, '_>,
    rejection: DepositBusinessRejection,
    now: Timestamp,
) -> Result<ConfirmOutcome, WriteError> {
    writer.finish_rejection(DepositRejection::Business(rejection), now).await?;
    Ok(ConfirmOutcome::Rejected(rejection))
}

/// 仍占用实物、可以确认灭失的四个 position 状态（与 000137 的 `lost` 来源态一致）。
const fn loss_allowed(state: PositionState) -> bool {
    matches!(
        state,
        PositionState::InCustody
            | PositionState::Committed
            | PositionState::InTransit
            | PositionState::Reconcile
    )
}

/// 不变量 2、3、6：锁内取事实 → 纯函数分流 → 事实、迁移、释放、建案、挂单关闭、通知与回执同提。
pub(super) async fn confirm_stage(
    command: &ConfirmPositionLossCommand,
    listing_id: Option<Uuid>,
    mut writer: DepositWriter<'_, '_, '_>,
) -> Result<ConfirmOutcome, WriteError> {
    let now = database_now(writer.connection()).await?;
    // 不变量 7、8：管理员的私有说明只进追加审计（成功与确定拒绝都带），不进回执与 DTO。
    writer.set_private_audit_note("loss_reason", command.reason.as_str());
    let Some(listing_id) = listing_id else {
        return reject(writer, DepositBusinessRejection::SourceMismatch, now).await;
    };
    let Some(current) = lock_listing(writer.connection(), listing_id).await? else {
        return reject(writer, DepositBusinessRejection::SourceMismatch, now).await;
    };
    let Some(position) = lock_loss_position(writer.connection(), command.position_id).await? else {
        return reject(writer, DepositBusinessRejection::SourceMismatch, now).await;
    };
    // 预读后 position 换了挂单：锁集合已失效，退出由调用方原样重试重新取锁。
    if position.listing_id != listing_id {
        return Err(ConsignmentError::Unavailable.into());
    }
    if position.version.cast_unsigned() != command.expected_version {
        return reject(writer, DepositBusinessRejection::StaleVersion, now).await;
    }
    // 不变量 6：挂单只有可售或已下架两态才有 `loss_confirmed` 的关闭边；其余态（待寄、已售待履约、
    // 撤回中）没有合同给出灭失的去向，失败关闭。
    if !loss_allowed(position.state)
        || !matches!(current.snapshot.state, ListingState::Available | ListingState::HeldOffMarket)
    {
        return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
    }

    let complete = proof_complete(writer.connection(), &position, command.proof_id).await?;
    let facts = routing_facts(writer.connection(), &position, complete).await?;
    match route_position_loss(&facts) {
        Err(LossRoutingError::ProofIncomplete) => {
            return reject(writer, DepositBusinessRejection::EvidenceUnproven, now).await;
        }
        // 当前代仍有有效买方交割权或资金责任：不走 position 终局，改走原 Sale resolve（D9 §5.3）。
        Ok(LossRoute::ResolveInSale) => {
            return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
        }
        // 无法查清：保留占用与买卖双方责任、不做任何状态迁移。命令按确定拒绝 `evidence_conflict` 写入账本与审计
        // （含私有说明），同事务写一条运维告警信号作为核对案；补证后由 Admin 以新命令重试（裁定）。
        Ok(LossRoute::KeepOccupancy) => {
            let command_id = writer.command_id();
            record_reconcile(writer.connection(), position.position_id, command_id, now).await?;
            raise_reconcile_signal(writer.connection(), now).await?;
            return reject(writer, DepositBusinessRejection::EvidenceConflict, now).await;
        }
        Ok(LossRoute::ClosePosition) => {}
    }
    let generation = position
        .generation
        .ok_or_else(|| WriteError::invariant("consignment_loss_generation_missing"))?;

    let command_id = writer.command_id();
    let loss_fact_id = Uuid::new_v4();
    let ids = OpenedLossCase {
        case_id: Uuid::new_v4(),
        outcome_id: Uuid::new_v4(),
        seller_receipt_id: Uuid::new_v4(),
    };
    // 写入顺序受 000137 守卫约束：事实先于 position 迁移，position 迁移先于挂单关闭与占用释放。
    insert_loss_fact(
        writer.connection(),
        &position,
        (loss_fact_id, command.proof_id, generation),
        command_id,
        now,
    )
    .await?;
    let position_version = mark_position_lost(
        writer.connection(),
        &position,
        command.proof_id,
        (command_id, current.snapshot.state_version.value()),
        now,
    )
    .await?;
    release_claim(writer.connection(), &position, loss_fact_id, now).await?;
    let opened = open_loss_case(
        writer.connection(),
        &LossFactRef {
            loss_fact_id,
            position_id: position.position_id,
            seller_user_id: position.seller_user_id,
        },
        auction_domain::CommandId::from_uuid(command_id),
        ids,
        now,
    )
    .await?;
    let projection =
        transition(current.snapshot, ListingState::Closed, Some(ListingReason::LossConfirmed))?;
    writer
        .stage_listing_update(
            &current,
            projection,
            current.current_snapshot_id,
            "loss_confirmed",
            Some("loss_confirmed"),
            now,
        )
        .await?;
    enqueue_loss_notice(writer.connection(), &position, opened.case_id, now).await?;
    let receipt = PositionLossReceipt::new(
        command.command_id,
        current.listing_id,
        projection.state_version,
        position.position_id,
        position_version.cast_unsigned(),
        loss_fact_id,
        opened.case_id,
        opened.seller_receipt_id,
    );
    writer.finish(receipt.clone(), now).await?;
    Ok(ConfirmOutcome::Accepted(receipt))
}

/// 唯一损失事实：记录确认时的当前代，不新增代（`position_id` 唯一约束兜底“至多一次终局”）。
async fn insert_loss_fact(
    connection: &mut PgConnection,
    position: &LossPosition,
    (loss_fact_id, proof_id, generation): (Uuid, Uuid, i64),
    command_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_loss_facts
            (loss_fact_id, position_id, proof_id, listing_id, seller_user_id, equipment_id,
             world_id, canonical_eid, generation, origin_command_id, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(loss_fact_id)
    .bind(position.position_id)
    .bind(proof_id)
    .bind(position.listing_id)
    .bind(position.seller_user_id)
    .bind(position.equipment_id)
    .bind(&position.world_id)
    .bind(&position.canonical_eid)
    .bind(generation)
    .bind(command_id)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// `lost` 事件与 position 迁移，返回新版本；当前处分代指针不动。
///
/// 事件的 `proof_id` 是损失证据；迁移按版本做 CAS，守卫另核同事务的损失事实与不换代。
async fn mark_position_lost(
    connection: &mut PgConnection,
    position: &LossPosition,
    proof_id: Uuid,
    (command_id, listing_version): (Uuid, u64),
    now: Timestamp,
) -> Result<i64, WriteError> {
    let version = position.version + 1;
    sqlx::query(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ($1, $2, $3, 'lost', $4, 'lost', $5, $6, $7, $8)",
    )
    .bind(position.position_id)
    .bind(version)
    .bind(command_id)
    .bind(position.state.as_str())
    .bind(proof_id)
    .bind(position.listing_id)
    .bind(i64::try_from(listing_version)?)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let moved = sqlx::query(
        "UPDATE public.consignment_custody_positions
         SET state = 'lost', version = $2, updated_at = $3
         WHERE id = $1 AND version = $4",
    )
    .bind(position.position_id)
    .bind(version)
    .bind(now)
    .bind(position.version)
    .execute(connection)
    .await?;
    if moved.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_loss_position_version"));
    }
    Ok(version)
}

/// 释放实物占用；`last_fact_id` 是本事务的损失事实（7M-2 的 `lost` 分支核对）。
async fn release_claim(
    connection: &mut PgConnection,
    position: &LossPosition,
    loss_fact_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let released = sqlx::query(
        "UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = $2, last_fact_id = $3
         WHERE position_id = $1",
    )
    .bind(position.position_id)
    .bind(now)
    .bind(loss_fact_id)
    .execute(connection)
    .await?;
    if released.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_loss_claim"));
    }
    Ok(())
}

/// 原身份通知：只带案件引用与卖家冻结身份（取最近一份合同冻结的卖家身份，从未成交没有合同时才取卖家当前
/// 用户名，因为 position 本身不冻结身份），不含任何密文、装备 key 或正文（正文由 7D-5 / 7E 定）。
async fn enqueue_loss_notice(
    connection: &mut PgConnection,
    position: &LossPosition,
    case_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let queued = sqlx::query(
        "INSERT INTO public.consignment_right_notice_outbox
            (work_id, notice_kind, loss_case_id, frozen_identity, world_id, created_at)
         SELECT $1::uuid, 'loss_receipt', $2::uuid,
                COALESCE((SELECT contract.frozen_seller_identity
                          FROM public.consignment_contracts AS contract
                          WHERE contract.position_id = $6
                          ORDER BY contract.generation DESC LIMIT 1), username),
                $4::text, $5::timestamptz
         FROM public.users WHERE id = $3",
    )
    .bind(Uuid::new_v4())
    .bind(case_id)
    .bind(position.seller_user_id)
    .bind(&position.world_id)
    .bind(now)
    .bind(position.position_id)
    .execute(connection)
    .await?;
    if queued.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_loss_notice_seller"));
    }
    Ok(())
}
