//! 寄售第 7 期拒收与恢复的领域规则：退回处置矩阵、重列判定与付款提醒判定。
//!
//! 全部是不读库、不读时钟的纯函数；输入是调用方在锁内读好的事实，输出是决定。

mod disposition;
mod errors;
mod loss;
mod relist;
mod reminder;
mod sanction;
mod withdrawal;

pub use disposition::{
    BuyerRightEffect, DeliveryClosure, ListingEffect, PrincipalEffect, RefundEffect,
    RelistEligibility, ReturnDispositionDecision, ReturnDispositionFacts, ReturnedLeg,
    decide_return_disposition,
};
pub use errors::RecoveryDomainError;
pub use loss::{LossRoute, LossRoutingError, LossRoutingFacts, route_position_loss};
pub use relist::{RelistDecision, RelistFacts, RelistMode, decide_relist};
pub use reminder::{ReminderDecision, ReminderFacts, ReminderStage, decide_reminder};
pub use sanction::{
    SANCTION_REASON_ISEKAI_UNPAID_CARRIER_RETURNS, SANCTION_REASON_MAIN_COD_RETURNS,
    SanctionExecution, SanctionExecutionFacts, SanctionWorkState, UserStatusFact,
    decide_sanction_execution, sanction_rule_from_reason_code,
};
pub use withdrawal::{
    CurrentGeneration, DirectWithdrawalDecision, DirectWithdrawalFacts, DirectWithdrawalRejection,
    decide_direct_withdrawal,
};
