//! 已在库、当前代未授合同时的直接撤回：同一事务把 Withdrawal 绑当前代、挂单转 `withdrawal_pending`。
//!
//! 保护不变量 1、2、3、6：命令已由调用方仲裁，挂单、卖家与实物锁已持有；本函数在锁内读 position 与
//! 处分代事实，交 `decide_direct_withdrawal` 判定，合同、返还义务、挂单投影、事件与 `withdrawal_accepted`
//! 回执同提。直接撤回不经过 `available`：不写 `available` 事件，不申请、不增加、不临时释放挂牌名额
//! （D9 §5.3）。决定 24：独立运营止售不阻拦取物。

use std::sync::Arc;

use auction_application::consignment::{
    DepositBusinessRejection, WithdrawDepositCommand, WithdrawDepositReceipt,
};
use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::recovery::{
    CurrentGeneration, DirectWithdrawalDecision, DirectWithdrawalFacts, DirectWithdrawalRejection,
    decide_direct_withdrawal,
};
use auction_domain::consignment::{ListingState, SeasonState, parse_settlement_world};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::generation::{
    NewWithdrawal, bind_withdrawal, current_generation, lock_withdrawal_position,
    position_season_window, unknown_dispatch_open,
};
use crate::database::consignment::commands::DepositRejection;
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::{
    active_hold, active_withdrawal_intent, season_state, transition,
};
use crate::database::consignment::listings::projection::LockedListing;
use crate::database::consignment::purchase::FirstDispatchContentSource;
use crate::database::consignment::recovery::sale_withdrawal::{
    SaleWithdrawal, withdraw_after_sale,
};

/// 直接撤回的结果；回执或拒绝都已在同一事务落库。
pub(crate) enum DirectWithdrawal {
    /// `withdrawal_accepted` 回执，合同与返还义务已写入。
    Accepted(WithdrawDepositReceipt),
    /// 确定拒绝，已写入安全拒绝回执。
    Rejected(DepositBusinessRejection),
}

/// 挂单的 position 是否已在代管中（含已承诺：Sale 封闭后的回库由 `direct_withdrawal` 判定）；
/// 锁住 position 行，供调用方在直接撤回与登记意愿之间分流。
pub(crate) async fn position_in_custody(
    connection: &mut PgConnection,
    listing_id: Uuid,
) -> Result<bool, WriteError> {
    let position = lock_withdrawal_position(connection, listing_id).await?;
    Ok(position.is_some_and(|position| {
        matches!(position.state, PositionState::InCustody | PositionState::Committed)
    }))
}

/// 直接撤回领域拒绝到确定业务码的映射。
///
/// `NotRightsHolder` 没有合同业务码：与同一入口对“非本人挂单”的既有处理一致，映射为
/// `listing_not_active`，不向非权利人泄露挂单是否存在（保守取值，见 P7C2 报告）。
const fn business_rejection(rejection: DirectWithdrawalRejection) -> DepositBusinessRejection {
    match rejection {
        DirectWithdrawalRejection::NotRightsHolder => DepositBusinessRejection::ListingNotActive,
        DirectWithdrawalRejection::EquipmentCommitted => {
            DepositBusinessRejection::EquipmentCommitted
        }
        DirectWithdrawalRejection::CustodyUnproven => DepositBusinessRejection::CustodyUnproven,
        DirectWithdrawalRejection::SeasonClosed => DepositBusinessRejection::SeasonClosed,
    }
}

/// 不变量 2、3、6：锁内取事实 → 纯函数判定 → 合同、撤回件首轮、挂单投影、事件、回执同提；任何拒绝只写
/// 安全回执。`content` 是撤回件首轮的冻结内容来源。
pub(crate) async fn direct_withdrawal(
    mut writer: DepositWriter<'_, '_, '_>,
    current: &LockedListing,
    command: &WithdrawDepositCommand,
    now: Timestamp,
    content: &Arc<dyn FirstDispatchContentSource>,
) -> Result<DirectWithdrawal, WriteError> {
    let listing_id = command.listing_id;
    let Some(position) = lock_withdrawal_position(writer.connection(), listing_id).await? else {
        return reject(writer, DepositBusinessRejection::CustodyUnproven, now).await;
    };
    let seller = command.seller_user_id.as_uuid();
    let world = parse_settlement_world(&position.world_id)
        .ok_or_else(|| WriteError::invariant("consignment_withdrawal_world"))?;
    let window = position_season_window(writer.connection(), &position).await?;
    let season = season_state(world, window.as_ref(), now);
    let generation = current_generation(writer.connection(), &position).await?;
    // 不变量 6：已承诺的 position 只有在当前代 Sale 已封闭、原件在库时才可回库；否则装备仍被买方占用。
    let recall = position.state == PositionState::Committed
        && matches!(generation, CurrentGeneration::SaleSealed { .. });
    if position.state == PositionState::Committed && !recall {
        return reject(writer, DepositBusinessRejection::EquipmentCommitted, now).await;
    }
    let facts = DirectWithdrawalFacts {
        seller_is_rights_holder: position.seller_user_id == seller
            && current.identity.seller_user_id == seller,
        // 回库在本事务内完成，判定时按回库后的状态看。
        position_state: if recall { PositionState::InCustody } else { position.state },
        generation,
        // 同合同任一腿（含买家退款、卖款）有结果未知的派发即视为相反未知外写（D9 §5.3 撤回 2）。
        opposite_unknown_write: unknown_dispatch_open(writer.connection(), position.position_id)
            .await?,
        // 预停窗口仍可安全寄还；季界已过或无法判定一律失败关闭。
        safe_to_return_in_season: matches!(
            season,
            SeasonState::NotSeasonal | SeasonState::Open | SeasonState::Preclose
        ),
        active_hold: active_hold(writer.connection(), position.position_id).await?.is_some(),
    };
    match decide_direct_withdrawal(&facts) {
        DirectWithdrawalDecision::Reject(rejection) => {
            reject(writer, business_rejection(rejection), now).await
        }
        // Sale 已封闭：追加 g+1 `withdrawal_only` 代（D9 §5.3 撤回 2、3）。
        DirectWithdrawalDecision::AppendWithdrawalOnlyGeneration {
            predecessor,
            predecessor_proof,
        } => {
            let request = SaleWithdrawal {
                position: &position,
                predecessor,
                sale_contract_id: predecessor_proof,
                recall,
            };
            let receipt =
                withdraw_after_sale(writer, current, command, &request, now, content).await?;
            Ok(DirectWithdrawal::Accepted(receipt))
        }
        DirectWithdrawalDecision::BindCurrentGeneration => {
            let Some(generation) = position.generation else {
                return reject(writer, DepositBusinessRejection::CustodyUnproven, now).await;
            };
            let intent_id =
                active_withdrawal_intent(writer.connection(), position.position_id).await?;
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
            let contract_id = bind_withdrawal(
                writer.tracked(),
                &NewWithdrawal {
                    origin_command_id: command.command_id.as_uuid(),
                    listing_id,
                    position_id: position.position_id,
                    generation,
                    equipment_id: position.equipment_id,
                    world_id: &position.world_id,
                    canonical_eid: &position.canonical_eid,
                    season_window_id: position.season_window_id,
                    seller_user_id: position.seller_user_id,
                    intent_id,
                    now,
                },
                content,
            )
            .await?;
            let receipt = WithdrawDepositReceipt::withdrawal_accepted(
                command.command_id,
                listing_id,
                projection.state_version,
                contract_id,
                position.equipment_id,
            );
            writer.finish(receipt.clone(), now).await?;
            Ok(DirectWithdrawal::Accepted(receipt))
        }
    }
}

async fn reject(
    writer: DepositWriter<'_, '_, '_>,
    rejection: DepositBusinessRejection,
    now: Timestamp,
) -> Result<DirectWithdrawal, WriteError> {
    writer.finish_rejection(DepositRejection::Business(rejection), now).await?;
    Ok(DirectWithdrawal::Rejected(rejection))
}
