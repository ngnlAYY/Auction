//! 撤回件首轮随撤回事务写入：返还义务的 prepared 投递、轮行与冻结身份一次落库（PIWX）。
//!
//! 保护不变量 2、3、4、6：本原语只在调用方的撤回事务（直接撤回、Sale 封闭后的撤回代、取得事务里
//! 兑现的意愿）内运行，命令、卖家、实物与挂单锁由调用方持有，不取锁、不提交；首轮键为
//! `(contract, withdrawn_return, 1)`，投递与命令 ID 由轮 ID 确定性派生。冻结权利人与收件人就是合同冻结的
//! 卖家，零 Credits、零 COD，标的恰为合同的规范装备；任一不符整笔回滚，不留半个首轮。投递停在
//! `prepared`，首次起写仍须经 `mark_write_started`。

use std::sync::Arc;

use auction_application::consignment::ConsignmentError;
use auction_domain::SettlementObligationKind;
use auction_domain::consignment::dispatch::{
    DispatchRoundKey, FrozenDispatchIdentity, dispatch_ref,
};
use auction_domain::consignment::{ConsignmentContract, ConsignmentObligationGraph};
use sqlx::Postgres;
use uuid::Uuid;

use super::first_dispatch::{FirstRound, insert_first_round};
use crate::database::TrackedTransaction;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::purchase::{
    FirstDispatchContentSource, WithdrawalDispatchRequest,
};

/// 在撤回事务内为 Withdrawal 的返还义务建立首轮。调用方须已在同一事务写入合同义务图。
///
/// D0 取合同建立时刻：撤回没有付款期限，D0 只作为冻结身份的时间锚点（不进正文）。
pub(crate) async fn create_withdrawal_round_in_tx(
    tx: &mut TrackedTransaction<'_, Postgres>,
    graph: &ConsignmentObligationGraph,
    content: &Arc<dyn FirstDispatchContentSource>,
) -> Result<FirstRound, WriteError> {
    let ConsignmentContract::Withdrawal { core, .. } = &graph.contract else {
        return Err(WriteError::invariant("consignment_withdrawal_round_contract"));
    };
    let [obligation] = graph.obligations.as_slice() else {
        return Err(WriteError::invariant("consignment_withdrawal_round_graph"));
    };
    if obligation.kind != SettlementObligationKind::WithdrawnReturn
        || obligation.prerequisite_id.is_some()
        || obligation.contract_id != core.contract_id
        || obligation.amount.value() != 0
        || obligation.carrier_credit != 0
    {
        return Err(WriteError::invariant("consignment_withdrawal_round_graph"));
    }
    let key = DispatchRoundKey {
        contract_id: core.contract_id.as_uuid(),
        obligation_id: obligation.id.as_uuid(),
        round: 1,
    };
    let round_ref = dispatch_ref(&key);
    let item_name = item_name(tx, core.listing_id).await?;
    let bot_uid = custody_bot_uid(tx, core.position_id).await?;
    let rendered = content.withdrawal_content(&WithdrawalDispatchRequest {
        world: core.world,
        round_ref: &round_ref,
        item_name: &item_name,
        canonical_eid: &core.canonical_eid,
        obligation,
    })?;
    // 不变量 6：寄还的必须就是合同的规范装备，渲染给出别的附件即失败关闭，不发错件。
    if rendered.equipment_ids.as_slice() != [core.canonical_eid.clone()] {
        return Err(ConsignmentError::Unavailable.into());
    }
    let frozen = FrozenDispatchIdentity {
        rights_holder_identity: core.frozen_seller_identity.clone(),
        bot_uid,
        world: core.world,
        subject: rendered.subject,
        template: rendered.template,
        body_digest: rendered.body_digest,
        round_ref,
        d0: core.created_at,
        credits: 0,
        cod: 0,
        equipment_ids: rendered.equipment_ids,
    };
    if frozen.validate().is_err() {
        return Err(WriteError::invariant("consignment_withdrawal_round_identity"));
    }
    insert_first_round(tx, key, &frozen, &core.frozen_seller_identity).await
}

/// 邮件主题冻结用的装备名：登记时的安全快照（与购买首轮同源）。
async fn item_name(
    tx: &mut TrackedTransaction<'_, Postgres>,
    listing_id: Uuid,
) -> Result<String, WriteError> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT name FROM public.consignment_snapshots WHERE listing_id = $1
         ORDER BY generation DESC LIMIT 1",
    )
    .bind(listing_id)
    .fetch_optional(&mut **tx)
    .await?;
    name.ok_or_else(|| WriteError::invariant("consignment_withdrawal_snapshot_missing"))
}

/// 代管原件的认证 Bot UID：取得该件的整封所在 Bot（position → 收件项）。缺失即失败关闭。
async fn custody_bot_uid(
    tx: &mut TrackedTransaction<'_, Postgres>,
    position_id: Uuid,
) -> Result<String, WriteError> {
    let bot_uid: Option<String> = sqlx::query_scalar(
        "SELECT item.bot_uid
         FROM public.consignment_custody_positions AS position
         JOIN public.consignment_deposit_items AS item ON item.id = position.item_id
         WHERE position.id = $1",
    )
    .bind(position_id)
    .fetch_optional(&mut **tx)
    .await?;
    bot_uid.ok_or_else(|| WriteError::invariant("consignment_withdrawal_bot_missing"))
}
