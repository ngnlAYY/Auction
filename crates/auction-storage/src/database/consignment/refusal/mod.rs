//! 拒收存储（第 6 期 6D3）：合格退回事实、追加裁决、买家周期、处罚构成项与唯一账号工作。
//!
//! 全部入口都是事务内原语，在调用方已持有买家用户锁（与购买共用，见 `buyer_lock`）的顶层
//! Serializable 事务连接上执行，本模块不取锁、不提交、不开保存点：
//! - `submit_return_fact`：保存返回事实、追加资格裁决，合格时写拒收并在同事务判阈值，达标即写
//!   待执行处罚、构成项与唯一账号工作（pending 处罚即新购阻断）；待判事实在新观察到来时以同一入口重裁；
//! - `close_cycle_and_open_next`：解禁路径原子关闭旧周期并开启新周期，旧周期的待执行处罚转
//!   `cycle_closed`、其账号工作取消；
//! - `buyer_sanction_pending`：购买事务在同一买家锁下重验的阻断读。
//!
//! 执行处罚与拉黑账号归第 7 期（7B-2），本模块只写到“待执行处罚 + 唯一账号工作”为止。
//! 生产调用方：购买（阻断读）、Admin 解禁（周期边界）与退件接管（`intake_fact` 推导证据后提交事实）。
//!
//! 本模块不写审计行与事件：处罚落库（`open_sanction` 是达标事务的原子结果）对应的审计与
//! `sanction_pending` 回执，由调用方（退件接管写端）在同一事务内写入（不变量 3）。

mod account_work;
mod buyer_lock;
mod cycle;
mod fact;
mod intake_fact;
mod sanction;
#[cfg(feature = "test-db")]
pub mod test_access;
mod verdict;

use auction_domain::consignment::refusal::{
    Deliverability, Qualification, RefusalKind, SanctionRule,
};
use auction_time::Timestamp;
use uuid::Uuid;

pub(crate) use buyer_lock::buyer_sanction_pending;
pub(crate) use cycle::close_cycle_and_open_next;
pub(crate) use intake_fact::{OpenedSanction, submit_acquired_return_fact};
pub(crate) use verdict::submit_return_fact;

/// 一次观察给出的证据结论；字段含义与 [`auction_domain::consignment::refusal::ReturnFactInput`] 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnEvidence {
    /// 可信 RTS 发生时原合同是否仍可交付（决定 26）。
    pub deliverability: Deliverability,
    /// 已付款或存在相反的实物交付证据。
    pub paid_or_opposite: bool,
    /// 退回物与原轮冻结的资产清单一致。
    pub asset_matches: bool,
    /// 另一个消费者（拍卖或寄售）已见过同一原信。
    pub duplicate: bool,
    /// 买家归属已被证明。
    pub ownership_proven: bool,
}

/// 提交一条退回事实或对其重新裁决。
///
/// 事实身份（原信键、合同、义务、原轮、资产腿、买家、发生时刻）一律取自已保存的退件接管行与
/// 合同的 Sale，调用方不重复提供，避免与存储事实不一致；返回事实 ID 由 K、原轮与腿派生。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnFactSubmission {
    /// 退件接管行；事实挂在它上面（`UNIQUE(intake_id)`）。
    pub intake_id: Uuid,
    /// 拒收种类；必须与合同世界对应（main 对 COD 退回，isekai 对未付载体退回）。
    pub kind: RefusalKind,
    /// 实际补证时刻；不参与资格先后。
    pub proven_at: Timestamp,
    /// isekai 有效交割关闭界点；main 为 `None`。
    pub delivery_close_at: Option<Timestamp>,
    /// 本次观察的证据结论。
    pub evidence: ReturnEvidence,
}

/// 处罚落库后的标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SanctionRecord {
    /// 处罚 ID（`sanction_id(buyer, cycle)`）。
    pub sanction_id: Uuid,
    /// 处罚所属周期。
    pub cycle_id: Uuid,
    /// 唯一待执行账号工作。
    pub account_work_id: Uuid,
    /// 触发规则。
    pub rule: SanctionRule,
    /// 判定时刻。
    pub t0: Timestamp,
}

/// `submit_return_fact` 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusalOutcome {
    /// 返回事实 ID。
    pub return_fact_id: Uuid,
    /// 当前有效的资格裁决。
    pub qualification: Qualification,
    /// 事实已有终局裁决，本次没有追加任何行（重放）。
    pub replayed: bool,
    /// 裁决为合格但事实所属周期已被解禁关闭：只留记录，不凑数、不处罚。
    pub cycle_closed: bool,
    /// 本次（或已有）与该事实相关的处罚。
    pub sanction: Option<SanctionRecord>,
}

/// `close_cycle_and_open_next` 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleTransition {
    /// 被关闭的旧周期；买家此前没有 open 周期时为 `None`。
    pub closed_cycle_id: Option<Uuid>,
    /// 新开启的周期。
    pub opened_cycle_id: Uuid,
    /// 随旧周期转为 `cycle_closed` 的待执行处罚。
    pub closed_sanction_id: Option<Uuid>,
}
