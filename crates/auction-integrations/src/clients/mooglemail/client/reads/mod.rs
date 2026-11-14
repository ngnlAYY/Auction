//! 共享邮箱观测：凭据/world/runtime 隔离，独立等待者、写屏障及有界敏感响应。

use std::{
    collections::HashMap,
    ops::Deref,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore, watch};
use uuid::Uuid;

use crate::clients::mooglemail::protocol::MoogleMailCookie;
use crate::clients::mooglemail::{MoogleMailError, MoogleMailWorld};

mod client;
mod lists;
mod scope;
mod sent_boundary;
mod sent_scope;
use self::scope::{read_unavailable, unavailable};
pub(super) use scope::lock_incoming_segment;

pub(crate) use lists::{
    MoogleMailIncomingList, MoogleMailIncomingPrefix, MoogleMailSentList, MoogleMailSentPrefix,
};
pub(crate) use sent_boundary::{SentBoundary, SentBoundaryScan};
pub(crate) use sent_scope::MoogleMailSentScope;

const MAX_SCOPES: usize = 8;
const MAX_REQUESTS_PER_ROUND: usize = 512;
const MAX_ACTIVE_REQUESTS: usize = 8;
const MAX_RETAINED_BYTES: usize = 32 * 1024 * 1024;
const REUSE_WINDOW: Duration = Duration::from_secs(30);
const MAX_ROUND_LIFETIME: Duration = Duration::from_secs(120);

type SharedResult = Option<Result<Arc<MoogleMailReadDocument>, MoogleMailError>>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ScopeKey {
    credential: Uuid,
    world: MoogleMailWorld,
    generation: Option<i64>,
}

pub(super) struct MailboxReads {
    scopes: StdMutex<HashMap<ScopeKey, Arc<ReadScope>>>,
    jobs: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    reaper_started: AtomicBool,
}

impl Default for MailboxReads {
    fn default() -> Self {
        Self {
            scopes: StdMutex::new(HashMap::new()),
            jobs: Arc::new(Semaphore::new(MAX_ACTIVE_REQUESTS)),
            bytes: Arc::new(Semaphore::new(MAX_RETAINED_BYTES)),
            reaper_started: AtomicBool::new(false),
        }
    }
}

struct ReadScope {
    state: StdMutex<ScopeState>,
    revision: AtomicU64,
    // 仅阻止会改变 Inbox/Read Mail 成员关系的详情 GET 打断完整列表扫描；不涉及 compose 锁。
    incoming: Arc<RwLock<()>>,
    membership_revision: AtomicU64,
}

struct ScopeState {
    current: Option<Arc<ReadEpoch>>,
    touched: Instant,
}

struct ReadEpoch {
    revision: u64,
    started: Instant,
    observed_at: DateTime<Utc>,
    requests: StdMutex<ReadRequests>,
    lists: lists::ListCells,
}

#[derive(Default)]
struct ReadRequests {
    entries: HashMap<String, watch::Receiver<SharedResult>>,
    started: usize,
}

/// 只读轮次不含凭据或资产授权；写屏障、owner 撤销和最大生命周期都会使其失效。
#[derive(Clone)]
pub(crate) struct MoogleMailReadRound {
    key: ScopeKey,
    scope: Arc<ReadScope>,
    epoch: Arc<ReadEpoch>,
}

impl MoogleMailReadRound {
    pub(crate) fn observed_at(&self) -> DateTime<Utc> {
        self.epoch.observed_at
    }

    fn check(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<(), MoogleMailError> {
        if !cookie.is_active() || self.key.credential != cookie.observation_key() {
            return Err(unavailable(world));
        }
        if self.key.world != world
            || self.scope.state.is_poisoned()
            || self.scope.revision.load(Ordering::Acquire) != self.epoch.revision
            || self.epoch.started.elapsed() >= MAX_ROUND_LIFETIME
        {
            return Err(read_unavailable(world));
        }
        Ok(())
    }
}

/// 原始页面仅存在于有界内存；最后一个读取者释放时清零，不提供 Debug/序列化。
pub(crate) struct MoogleMailReadDocument {
    body: Vec<u8>,
    _bytes: OwnedSemaphorePermit,
}

impl Deref for MoogleMailReadDocument {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.body
    }
}

impl Drop for MoogleMailReadDocument {
    fn drop(&mut self) {
        self.body.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn incoming_segment_releases_lock_between_pages() {
        let incoming = Arc::new(RwLock::new(()));
        let revision = Arc::new(AtomicU64::new(0));
        let segment =
            lock_incoming_segment(&incoming, &revision, 0, MoogleMailWorld::Main).await.unwrap();

        // 详情 GET 的写锁在段内被阻塞；这证明扫描确实持有成员关系读锁。
        let writer = {
            let incoming = Arc::clone(&incoming);
            let revision = Arc::clone(&revision);
            tokio::spawn(async move {
                let _guard = incoming.write().await;
                revision.fetch_add(1, Ordering::AcqRel);
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!writer.is_finished(), "writer must wait while the segment is held");

        // 段结束释放后，写者可以推进并改变成员关系。
        drop(segment);
        tokio::time::timeout(Duration::from_secs(1), writer)
            .await
            .expect("writer acquired after segment release")
            .expect("writer task");

        // 下一段取锁时发现 revision 改变：本次扫描放弃，而不是继续用旧成员关系。
        assert!(
            lock_incoming_segment(&incoming, &revision, 0, MoogleMailWorld::Main).await.is_err()
        );
        assert!(
            lock_incoming_segment(&incoming, &revision, 1, MoogleMailWorld::Main).await.is_ok()
        );
    }
}
