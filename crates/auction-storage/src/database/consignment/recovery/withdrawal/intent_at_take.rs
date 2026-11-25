//! 取得事务内兑现撤回意愿：意愿先到，整封取得时同事务在首个处分代上建 Withdrawal。
//!
//! 保护不变量 2、3、6：调用方（`apply_take_custody_in_tx`）已持有命令、卖家、实物与挂单锁，且已在
//! 同一事务写下首代；合同、返还义务、挂单 `withdrawal_pending`、事件与取得回执同提，任一失败整体回滚。
//! 来源命令取登记意愿的那条撤回命令：它是该意愿的唯一命令，一单一 position，所以来源唯一约束不会
//! 因整封内多件撤回而冲突，也不需要在取得事务里另造账本行。

use std::sync::Arc;

use auction_time::Timestamp;
use sqlx::Postgres;
use uuid::Uuid;

use super::generation::{NewWithdrawal, bind_withdrawal};
use crate::database::TrackedTransaction;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::purchase::FirstDispatchContentSource;

/// 刚被取得的 position 与其首代，字段全部来自取得事务锁内读出的行。
pub(crate) struct TakenPosition<'a> {
    pub(crate) position_id: Uuid,
    pub(crate) listing_id: Uuid,
    pub(crate) seller_user_id: Uuid,
    pub(crate) equipment_id: Uuid,
    pub(crate) world_id: &'a str,
    pub(crate) canonical_eid: &'a str,
    pub(crate) season_window_id: Option<Uuid>,
    /// 刚写下的首个处分代序号。
    pub(crate) generation: u64,
}

/// 为带有效撤回意愿的刚取得件建 Withdrawal 与撤回件首轮，返回合同 ID；没有有效意愿时失败关闭。
pub(crate) async fn fulfill_intent_at_take(
    tx: &mut TrackedTransaction<'_, Postgres>,
    position: &TakenPosition<'_>,
    now: Timestamp,
    content: &Arc<dyn FirstDispatchContentSource>,
) -> Result<Uuid, WriteError> {
    let intent: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, command_id FROM public.consignment_custody_controls
         WHERE position_id = $1 AND kind = 'withdrawal_intent'",
    )
    .bind(position.position_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((intent_id, intent_command_id)) = intent else {
        return Err(WriteError::invariant("consignment_withdrawal_intent_missing"));
    };
    bind_withdrawal(
        tx,
        &NewWithdrawal {
            origin_command_id: intent_command_id,
            listing_id: position.listing_id,
            position_id: position.position_id,
            generation: position.generation,
            equipment_id: position.equipment_id,
            world_id: position.world_id,
            canonical_eid: position.canonical_eid,
            season_window_id: position.season_window_id,
            seller_user_id: position.seller_user_id,
            intent_id: Some(intent_id),
            now,
        },
        content,
    )
    .await
}
