//! Delivery outbox 的领取、外部发送、租约续期和终态落库；
//! 外部结果未知时保留安全失败语义并进入持久化对账，不能凭本地超时推断远端未写入。
//!
//! 子模块按职责拆分：轮次领取与在途编排（`cycle`）、单个 claim 的发送编排（`dispatch`）、
//! 只读准备/fence/未开始释放（`handoff`）、批次结果计量（`report`），以及租约、
//! 通知指标和提交后唤醒适配。

mod cycle;
mod dispatch;
mod handoff;
mod lease;
mod notification;
mod report;
mod wakeup;

pub(super) use cycle::run_delivery_cycle;
pub(super) use wakeup::run as run_delivery_wakeup;
