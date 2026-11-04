//! Public 实时连接的进程总量与每用户配额；和 discovery 专属容量、请求频率分离。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use tokio::sync::Notify;
use uuid::Uuid;

use super::RealtimeSubscriptionError;

const MAX_CONNECTIONS: usize = 1024;
const MAX_CONNECTIONS_PER_USER: usize = 8;

#[derive(Default)]
struct ConnectionCounts {
    total: usize,
    draining: bool,
    users: BTreeMap<Uuid, usize>,
}

/// 配额登记只保留仍持有连接的用户；最后一条释放时删除，避免历史账号累积。
#[derive(Default)]
pub(super) struct ConnectionLimits {
    counts: Mutex<ConnectionCounts>,
    drained: Notify,
}

impl ConnectionLimits {
    pub(super) fn acquire(
        self: &Arc<Self>,
        user_id: Uuid,
    ) -> Result<ConnectionPermit, RealtimeSubscriptionError> {
        if user_id.is_nil() {
            return Err(RealtimeSubscriptionError::Unavailable);
        }
        let mut counts = self.counts.lock().map_err(|diagnostic_cause| {
            diagnostics::error(
                "realtime_hub",
                "lock_user_counts",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            RealtimeSubscriptionError::Unavailable
        })?;
        if counts.draining {
            return Err(RealtimeSubscriptionError::Unavailable);
        }
        let user_count = counts.users.get(&user_id).copied().unwrap_or(0);
        if counts.total >= MAX_CONNECTIONS || user_count >= MAX_CONNECTIONS_PER_USER {
            return Err(RealtimeSubscriptionError::Capacity);
        }
        counts.total += 1;
        counts.users.insert(user_id, user_count + 1);
        Ok(ConnectionPermit { limits: Arc::clone(self), user_id })
    }

    pub(super) fn begin_shutdown(&self) {
        if let Ok(mut counts) = self.counts.lock() {
            counts.draining = true;
        }
    }

    /// 许可在升级闭包创建前取得，普通流、发现流及尚未完成升级的任务都在同一计数中。
    pub(super) async fn wait_drained(&self) -> Result<(), RealtimeSubscriptionError> {
        loop {
            // notify_waiters 从 Notified 创建时起生效；先登记再读计数，避免最后一次 Drop 丢失唤醒。
            let drained = self.drained.notified();
            {
                let counts = self.counts.lock().map_err(|diagnostic_cause| {
                    diagnostics::error(
                        "realtime_hub",
                        "lock_drain_state",
                        DiagnosticError::diagnostic_kind(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    RealtimeSubscriptionError::Unavailable
                })?;
                if !counts.draining {
                    return Err(RealtimeSubscriptionError::Unavailable);
                }
                if counts.total == 0 {
                    return Ok(());
                }
            }
            drained.await;
        }
    }
}

/// 槽位跟随订阅生命周期；升级失败、取消、断连和慢消费者结束均由 Drop 归还。
pub(super) struct ConnectionPermit {
    limits: Arc<ConnectionLimits>,
    user_id: Uuid,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        let Ok(mut counts) = self.limits.counts.lock() else {
            return;
        };
        let Some(user_count) = counts.users.get_mut(&self.user_id) else {
            return;
        };
        *user_count -= 1;
        if *user_count == 0 {
            counts.users.remove(&self.user_id);
        }
        counts.total -= 1;
        let drained = counts.total == 0;
        drop(counts);
        if drained {
            self.limits.drained.notify_waiters();
        }
    }
}
