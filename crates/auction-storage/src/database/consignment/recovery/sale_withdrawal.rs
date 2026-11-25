//! Sale 已占代 g 后的撤回：回库（committed -> in_custody）、追加 g+1 处分代与带前驱链路的 Withdrawal。
//!
//! 保护不变量 2、3、4、6：调用方（`direct_withdrawal`）已持有命令、卖家、实物与挂单锁，并已由领域判定
//! 确认旧 Sale 有封闭事实、原件从未离库或已精确收回、无相反未知外写；本模块在同一事务里写下
//! 回库事件与 position 迁移、唯一后继代与当前指针 CAS、Withdrawal 合同与返还义务、挂单
//! `withdrawal_pending`、事件与回执，任何一项失败整体回滚。该代只能签 Withdrawal：不产生
//! `available` 版本或目录事件，不申请、增加或临时释放挂牌名额（D9 §5.3 撤回 3）。

use std::sync::Arc;

use auction_application::consignment::{WithdrawDepositCommand, WithdrawDepositReceipt};
use auction_domain::consignment::ListingState;
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::withdrawal::{
    NewWithdrawal, SealedPredecessor, WithdrawalPosition, bind_withdrawal_after_sale,
    closure_proof_ref,
};
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::{active_withdrawal_intent, transition};
use crate::database::consignment::listings::projection::LockedListing;
use crate::database::consignment::purchase::FirstDispatchContentSource;

/// 撤回代的用途码；与 000137 的 `c7_generation_purpose_ck` 一致。
const WITHDRAWAL_ONLY_PURPOSE: &str = "withdrawal_only";

/// 一次 Sale 占代撤回的锁内输入。
pub(super) struct SaleWithdrawal<'a> {
    /// 锁内读出的 position。
    pub(super) position: &'a WithdrawalPosition,
    /// 前驱代序号 g（即当前代）。
    pub(super) predecessor: u64,
    /// 被取代的已封闭 Sale 合同。
    pub(super) sale_contract_id: Uuid,
    /// position 仍是 `committed`（Sale 封闭后原件在库）：先回库再换代。
    pub(super) recall: bool,
}

/// 回库：position `committed -> in_custody`，同版本写 `uncommitted` 事件，其 `proof_id` 是 Sale 封闭事实。
///
/// 事件先于 position 更新写入：000137 的守卫在 position 更新时核对同事务的事件、封闭事实与
/// `c7_sale_original_back`。返回 position 的新版本。
pub(super) async fn recall_to_custody(
    connection: &mut PgConnection,
    position: &WithdrawalPosition,
    sale_contract_id: Uuid,
    command_id: Uuid,
    listing: (Uuid, u64),
    now: Timestamp,
) -> Result<i64, WriteError> {
    let version = position.version + 1;
    sqlx::query(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ($1, $2, $3, 'uncommitted', 'committed', 'in_custody', $7, $4, $5, $6)",
    )
    .bind(position.position_id)
    .bind(version)
    .bind(command_id)
    .bind(listing.0)
    .bind(i64::try_from(listing.1)?)
    .bind(now)
    .bind(sale_contract_id)
    .execute(&mut *connection)
    .await?;
    let moved = sqlx::query(
        "UPDATE public.consignment_custody_positions
         SET state = 'in_custody', version = $2, updated_at = $3
         WHERE id = $1 AND state = 'committed' AND version = $4",
    )
    .bind(position.position_id)
    .bind(version)
    .bind(now)
    .bind(position.version)
    .execute(connection)
    .await?;
    if moved.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_recall_position_version"));
    }
    Ok(version)
}

/// 追加 g+1 处分代并把当前指针 CAS 到 g+1，返回 position 的新版本。
///
/// 新代沿用前代的取得证明、服务代次、黑名单基线与季窗（原物仍是同一次精确取得的那件）；唯一后继由
/// `(position, predecessor)` 唯一键兜底，指针只在 `current_generation = g` 且版本未变时前进
/// （不变量 2：并发撤回、重列、购买至多一方取得该代）。
pub(super) async fn append_successor_generation(
    connection: &mut PgConnection,
    position: &WithdrawalPosition,
    version: i64,
    predecessor: u64,
    purpose: &str,
    command_id: Uuid,
    now: Timestamp,
) -> Result<i64, WriteError> {
    let predecessor_generation = i64::try_from(predecessor)?;
    let appended = sqlx::query(
        "INSERT INTO public.consignment_disposition_generations
            (position_id, generation, purpose, predecessor, proof_id, service_epoch,
             blacklist_baseline_ids, season_window_id, created_command_id, created_at)
         SELECT position_id, generation + 1, $3, generation, proof_id, service_epoch,
                blacklist_baseline_ids, season_window_id, $4, $5
         FROM public.consignment_disposition_generations
         WHERE position_id = $1 AND generation = $2",
    )
    .bind(position.position_id)
    .bind(predecessor_generation)
    .bind(purpose)
    .bind(command_id)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if appended.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_successor_generation_missing"));
    }
    let next_version = version + 1;
    let moved = sqlx::query(
        "UPDATE public.consignment_custody_positions
         SET current_generation = $2 + 1, version = $3, updated_at = $4
         WHERE id = $1 AND current_generation = $2 AND version = $5",
    )
    .bind(position.position_id)
    .bind(predecessor_generation)
    .bind(next_version)
    .bind(now)
    .bind(version)
    .execute(connection)
    .await?;
    if moved.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_generation_pointer_cas"));
    }
    Ok(next_version)
}

/// 已封闭 Sale 占代 g 时的撤回：回库（需要时）、g+1 撤回代、Withdrawal 与撤回件首轮、挂单投影、事件与
/// 回执同提。
pub(super) async fn withdraw_after_sale(
    mut writer: DepositWriter<'_, '_, '_>,
    current: &LockedListing,
    command: &WithdrawDepositCommand,
    request: &SaleWithdrawal<'_>,
    now: Timestamp,
    content: &Arc<dyn FirstDispatchContentSource>,
) -> Result<WithdrawDepositReceipt, WriteError> {
    let command_id = writer.command_id();
    let position = request.position;
    let mut version = position.version;
    if request.recall {
        version = recall_to_custody(
            writer.connection(),
            position,
            request.sale_contract_id,
            command_id,
            (current.listing_id, current.snapshot.state_version.value()),
            now,
        )
        .await?;
    }
    append_successor_generation(
        writer.connection(),
        position,
        version,
        request.predecessor,
        WITHDRAWAL_ONLY_PURPOSE,
        command_id,
        now,
    )
    .await?;
    let proof_ref = closure_proof_ref(writer.connection(), request.sale_contract_id).await?;
    let intent_id = active_withdrawal_intent(writer.connection(), position.position_id).await?;
    let projection = transition(current.snapshot, ListingState::WithdrawalPending, None)?;
    writer
        .stage_listing_update(
            current,
            projection,
            current.current_snapshot_id,
            "withdrawal_accepted",
            None,
            now,
        )
        .await?;
    let contract_id = bind_withdrawal_after_sale(
        writer.tracked(),
        &NewWithdrawal {
            origin_command_id: command_id,
            listing_id: current.listing_id,
            position_id: position.position_id,
            generation: request.predecessor + 1,
            equipment_id: position.equipment_id,
            world_id: &position.world_id,
            canonical_eid: &position.canonical_eid,
            season_window_id: position.season_window_id,
            seller_user_id: position.seller_user_id,
            intent_id,
            now,
        },
        &SealedPredecessor {
            sale_contract_id: request.sale_contract_id,
            generation: request.predecessor,
            closure_proof_ref: &proof_ref,
        },
        content,
    )
    .await?;
    let receipt = WithdrawDepositReceipt::withdrawal_accepted(
        command.command_id,
        current.listing_id,
        projection.state_version,
        contract_id,
        position.equipment_id,
    );
    writer.finish(receipt.clone(), now).await?;
    Ok(receipt)
}
