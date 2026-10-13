//! Public 实时连接的进程总量与每用户配额；和 discovery 专属容量、请求频率分离。

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use uuid::Uuid;

use super::RealtimeSubscriptionError;

const MAX_CONNECTIONS: usize = 1024;
const MAX_CONNECTIONS_PER_USER: usize = 8;

#[derive(Default)]
struct ConnectionCounts {
    total: usize,
    users: BTreeMap<Uuid, usize>,
}

/// 配额登记只保留仍持有连接的用户；最后一条释放时删除，避免历史账号累积。
#[derive(Default)]
pub(super) struct ConnectionLimits {
    counts: Mutex<ConnectionCounts>,
}

impl ConnectionLimits {
    pub(super) fn acquire(
        self: &Arc<Self>,
        user_id: Uuid,
    ) -> Result<ConnectionPermit, RealtimeSubscriptionError> {
        if user_id.is_nil() {
            return Err(RealtimeSubscriptionError::Unavailable);
        }
        let mut counts = self.counts.lock().map_err(|_| RealtimeSubscriptionError::Unavailable)?;
        let user_count = counts.users.get(&user_id).copied().unwrap_or(0);
        if counts.total >= MAX_CONNECTIONS || user_count >= MAX_CONNECTIONS_PER_USER {
            return Err(RealtimeSubscriptionError::Capacity);
        }
        counts.total += 1;
        counts.users.insert(user_id, user_count + 1);
        Ok(ConnectionPermit { limits: Arc::clone(self), user_id })
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
    }
}
