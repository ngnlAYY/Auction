//! 读取作用域、写屏障与收件段锁。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use tokio::time;

use super::{MAX_ROUND_LIFETIME, MailboxReads, ReadScope};
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
};

impl MailboxReads {
    pub(super) fn start_reaper(self: &Arc<Self>) {
        if self.reaper_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let reads = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                time::sleep(Duration::from_secs(5)).await;
                let Some(reads) = reads.upgrade() else { break };
                if let Ok(mut scopes) = reads.scopes.lock() {
                    scopes.retain(|_, scope| {
                        let Ok(mut state) = scope.state.lock() else { return false };
                        if state
                            .current
                            .as_ref()
                            .is_some_and(|epoch| epoch.started.elapsed() >= MAX_ROUND_LIFETIME)
                        {
                            state.current = None;
                        }
                        Arc::strong_count(scope) > 1 || state.touched.elapsed() < MAX_ROUND_LIFETIME
                    });
                }
            }
        });
    }
}

impl ReadScope {
    pub(super) fn invalidate(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut state) = self.state.lock() {
            state.current = None;
            state.touched = Instant::now();
        }
    }
}

/// 正常完成、未知响应和取消都执行第二道屏障，拒绝写请求期间产生的旧观测。
pub(crate) struct WriteBarrier(pub(super) Arc<ReadScope>);

impl Drop for WriteBarrier {
    fn drop(&mut self) {
        self.0.invalidate();
    }
}

pub(crate) struct IncomingDetailGuard {
    pub(super) scope: Arc<ReadScope>,
    pub(super) _guard: OwnedRwLockWriteGuard<()>,
}

impl Drop for IncomingDetailGuard {
    fn drop(&mut self) {
        self.scope.membership_revision.fetch_add(1, Ordering::AcqRel);
    }
}

/// 长列表扫描的每一段重新获取成员关系读锁；段间释放后详情 GET 可以推进。
/// 若段间成员关系已改变，本次扫描放弃（不缓存半份列表），由下一轮按新 revision 重扫。
pub(crate) async fn lock_incoming_segment(
    incoming: &Arc<RwLock<()>>,
    membership_revision: &AtomicU64,
    expected: u64,
    world: MoogleMailWorld,
) -> Result<OwnedRwLockReadGuard<()>, MoogleMailError> {
    let guard = Arc::clone(incoming).read_owned().await;
    if membership_revision.load(Ordering::Acquire) != expected {
        return Err(read_unavailable(world));
    }
    Ok(guard)
}

pub(super) fn read_unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::RemoteTemporary,
        true,
    )
}

pub(super) fn unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::CredentialUnavailable,
        false,
    )
}
