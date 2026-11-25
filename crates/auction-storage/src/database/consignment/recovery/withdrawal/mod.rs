//! 寄售撤回合同的存储写入：取得事务内兑现撤回意愿、无 Sale 占代时的直接撤回，以及二者共用的
//! Withdrawal 合同绑定。
//!
//! Sale 已占代 g 的撤回（追加 `withdrawal_only` 代）由上级模块的 `sale_withdrawal` 承接：本模块只读出
//! 当前代状态（`current_generation`），遇到未封闭或无法读清的合同一律失败关闭。

mod direct;
mod generation;
mod intent_at_take;

pub(crate) use direct::{DirectWithdrawal, direct_withdrawal, position_in_custody};
pub(in crate::database::consignment::recovery) use generation::{
    NewWithdrawal, SealedPredecessor, WithdrawalPosition, bind_withdrawal_after_sale,
    closure_proof_ref, current_generation, lock_withdrawal_position, position_season_window,
    read_withdrawal_position, unknown_dispatch_open,
};
pub(crate) use intent_at_take::{TakenPosition, fulfill_intent_at_take};
