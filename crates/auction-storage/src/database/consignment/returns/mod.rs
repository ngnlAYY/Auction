//! 寄售退件接管写端：按 RTS 的 K 接管、Take 起写与消费（第 6 期 6D5）。
//!
//! 接管把原信终局处置、唯一 Take 工作与投递、intake 同事务写入；起写复用第 4 期的
//! `mail_disposition_write_starts`，现金腿同事务建立 UID opening 阻断；消费同事务写精确 Take 证明、
//! 消费与实际效果（实物回位或现金回补）。判定规则在 `auction_domain::consignment::returns`，
//! 本模块只在锁内读取事实、调用领域判定并落库。

mod acquire;
mod conflicts;
mod consume;
mod direct_acquire;
mod intake;
mod ledger;
mod locks;
mod release_path;
mod take_start;
mod take_work;

use auction_application::consignment::{
    AcquireReceipt, AcquireReturn, ConsignmentFuture, ConsumeReceipt, ConsumeReturn,
    ReturnIntakeStore, StartReturnTake, WriteStartOutcome,
};

use crate::DatabaseStore;

pub(crate) use acquire::{AcquireOutcome, AcquireRequest, acquire_in_tx};
pub(crate) use conflicts::funding_error;
pub(crate) use intake::RoundLocator;
pub(crate) use locks::{LockPlan, pre_read_by_round, pre_read_by_round_id};
pub(crate) use release_path::release_acquire;

impl ReturnIntakeStore for DatabaseStore {
    fn acquire_return(&self, command: AcquireReturn) -> ConsignmentFuture<'_, AcquireReceipt> {
        Box::pin(direct_acquire::acquire_return(self, command))
    }

    fn start_return_take(
        &self,
        command: StartReturnTake,
    ) -> ConsignmentFuture<'_, WriteStartOutcome> {
        Box::pin(take_start::start_return_take(self, command))
    }

    fn consume_return(&self, command: ConsumeReturn) -> ConsignmentFuture<'_, ConsumeReceipt> {
        Box::pin(consume::consume_return(self, command))
    }
}
