//! 竞价事务提交成功后的独立步骤：待定序号补号、标记事件处理与 delivery 唤醒。
//!
//! 这些步骤各自开启短事务或单条语句，必须在事务 runner 的观测窗口之外执行：窗口内的 BEGIN
//! 会结束本次尝试计量，并把已归属的阶段（如 Final）重置为 owner 默认值。失败都不回滚已提交
//! 的出价；持久事件与工作项仍由 relay 与 Worker 扫描兜底（不变量 5：内存唤醒只是提示）。

use uuid::Uuid;

use super::PostgresCompetitionTransaction;
use crate::database::DatabaseStore;

/// 已提交事务留给提交后执行的工作；回滚的尝试不会产生本值。
pub(super) struct AfterCommit {
    deferred_stream_keys: Vec<String>,
    lot_watch_events: Vec<Uuid>,
    delivery_wakeup: bool,
}

impl AfterCommit {
    /// 在提交前从事务对象取出；只有提交成功后才允许 [`AfterCommit::apply`]。
    pub(super) fn take(transaction: &mut PostgresCompetitionTransaction) -> Self {
        Self {
            deferred_stream_keys: transaction.take_deferred_stream_keys(),
            lot_watch_events: transaction.take_lot_watch_events(),
            delivery_wakeup: transaction.take_delivery_wakeup(),
        }
    }

    /// 顺序固定为补号、标记事件、唤醒；场次排队凭证由调用方持有到本函数返回。
    pub(super) async fn apply(self, store: &DatabaseStore) {
        // 竞价事件的序号在提交后分配，失败由 relay 兜底。
        crate::database::realtime::allocate_deferred_sequences_after_commit(
            store,
            &self.deferred_stream_keys,
        )
        .await;
        // 标记与提醒在提交后的独立事务应用；失败不回滚出价，持久事件仍由 Worker 扫描重试。
        crate::database::lot_watch::process_committed_events(store, &self.lot_watch_events).await;
        // 唤醒只在提交后单独发送，避免 NOTIFY 提交锁。
        if self.delivery_wakeup
            && let Err(error) =
                crate::database::delivery::scheduler::publish_after_commit(store.pool()).await
        {
            tracing::warn!(
                error = %error,
                "delivery wakeup after slow proxy commit failed; polling remains active"
            );
        }
    }
}
