//! 寄售恶意拒收的领域规则：退回事实资格、isekai 90 天窗口、处罚阈值与确定性 ID。
//!
//! 纯规则，不读时钟、不访问存储；锁、周期持久化与事务由存储车道承担。

mod fact;
mod ids;
mod qualify;
mod sanction;
mod window;

pub use fact::{Deliverability, RefusalKind, ReturnFactInput};
pub use ids::{cycle_id, return_fact_id, sanction_id};
pub use qualify::{NotCounted, PendingReason, Qualification, qualify_return};
pub use sanction::{
    ISEKAI_MIN_THRESHOLD, QualifiedRefusal, SanctionDecision, SanctionRule,
    evaluate_cycle_sanction, evaluate_sanction,
};
pub use window::{ISEKAI_WINDOW_DAYS, IsekaiWindow, isekai_window};

#[cfg(test)]
mod tests;
