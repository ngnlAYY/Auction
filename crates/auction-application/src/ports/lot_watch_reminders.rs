//! 被超提醒与自动标记的提交后 outbox 处理端口。
//!
//! 出价 Serializable 事务只追加 `lot_watch_events` 事实；本端口负责在提交之后按 lot 的
//! `state_version` 顺序应用既有的标记/领先周期/提醒状态机。处理事务与事件完成标记同一事务
//! 提交，因此崩溃只会重试，不会丢失或重复提醒。端口不暴露 SQL、事件负载或用户敏感字段。

use crate::ports::auction::ApplicationFuture;

/// 一次 outbox 处理批次的计数；不含事件内容与正文。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LotWatchReminderBatch {
    /// 本轮成功领取的事件数（含过期租约回收）。
    pub claimed: u16,
    /// 处理并标记完成的事件数。
    pub completed: u16,
    /// 处理失败、已释放租约等待重试的事件数。
    pub retried: u16,
}

impl LotWatchReminderBatch {
    pub fn is_empty(self) -> bool {
        self.claimed == 0
    }
}

/// 提交后事件处理器；实现必须保证同 lot 竞价事件的顺序与事件完成标记的原子性。
pub trait LotWatchReminderStore: Send + Sync {
    /// 领取并处理至多 `limit` 个到期事件。失败的事件保留持久记录并退避重试，永不删除。
    fn process_due(
        &self,
        worker_id: String,
        lease_seconds: u32,
        limit: u16,
    ) -> ApplicationFuture<'_, LotWatchReminderBatch>;
}
