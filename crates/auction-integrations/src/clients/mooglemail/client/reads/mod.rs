//! 共享邮箱观测：凭据/world/runtime 隔离，独立等待者、写屏障及有界敏感响应。

use auction_application::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::clients::metrics::MailLockWaitGuard;
use crate::diagnostics as safe_diagnostics;

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
use tokio::sync::{OwnedRwLockWriteGuard, OwnedSemaphorePermit, RwLock, Semaphore, watch};
use uuid::Uuid;

use super::*;

mod lists;
mod sent_boundary;
mod sent_scope;

pub(crate) use lists::{MoogleMailSentList, MoogleMailSentPrefix};
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

impl MoogleMailClient {
    fn read_scope(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<(ScopeKey, Arc<ReadScope>), MoogleMailError> {
        if !cookie.is_active() || self.ownership.as_ref().is_some_and(|owner| !owner.is_valid()) {
            return Err(unavailable(world));
        }
        let key = ScopeKey {
            credential: cookie.observation_key(),
            world,
            generation: self
                .ownership
                .as_ref()
                .and_then(|owner| owner.mail_runtime_binding())
                .map(|binding| binding.generation),
        };
        let mut scopes = self.reads.scopes.lock().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "lock_read_scopes",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            read_unavailable(world)
        })?;
        if let Some(scope) = scopes.get(&key) {
            return Ok((key, Arc::clone(scope)));
        }
        if scopes.len() >= MAX_SCOPES {
            return Err(read_unavailable(world));
        }
        let scope = Arc::new(ReadScope {
            state: StdMutex::new(ScopeState { current: None, touched: Instant::now() }),
            revision: AtomicU64::new(0),
            incoming: Arc::new(RwLock::new(())),
            membership_revision: AtomicU64::new(0),
        });
        scopes.insert(key, Arc::clone(&scope));
        drop(scopes);
        self.reads.start_reaper();
        Ok((key, scope))
    }

    /// fresh 建立请求屏障；其后的请求绝不加入屏障前的在途读取。
    pub(crate) fn read_round(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        fresh: bool,
    ) -> Result<MoogleMailReadRound, MoogleMailError> {
        let (key, scope) = self.read_scope(cookie, world)?;
        let mut state = scope.state.lock().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "lock_round_cache",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            read_unavailable(world)
        })?;
        state.touched = Instant::now();
        if fresh {
            scope.revision.fetch_add(1, Ordering::AcqRel);
            state.current = None;
        }
        let epoch = state
            .current
            .as_ref()
            .filter(|epoch| epoch.started.elapsed() < REUSE_WINDOW)
            .cloned()
            .unwrap_or_else(|| {
                let epoch = Arc::new(ReadEpoch {
                    revision: scope.revision.load(Ordering::Acquire),
                    started: Instant::now(),
                    observed_at: Utc::now(),
                    requests: StdMutex::new(ReadRequests::default()),
                    lists: lists::ListCells::default(),
                });
                state.current = Some(Arc::clone(&epoch));
                epoch
            });
        drop(state);
        Ok(MoogleMailReadRound { key, scope, epoch })
    }

    /// 为已知 MID 的履约证明建立独立新鲜轮次；不复用 claim 前的页面，也不失效其他读取者。
    /// 此轮次仍受同一 owner/代次、全局请求并发及内存上限约束，本身不授予资产提交权限。
    pub(crate) fn isolated_read_round(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailReadRound, MoogleMailError> {
        let (key, scope) = self.read_scope(cookie, world)?;
        let mut state = scope.state.lock().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "lock_isolated_round",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            read_unavailable(world)
        })?;
        state.touched = Instant::now();
        let epoch = Arc::new(ReadEpoch {
            revision: scope.revision.load(Ordering::Acquire),
            started: Instant::now(),
            observed_at: Utc::now(),
            requests: StdMutex::new(ReadRequests::default()),
            lists: lists::ListCells::default(),
        });
        drop(state);
        Ok(MoogleMailReadRound { key, scope, epoch })
    }

    /// 同一轮次的同一 typed 目标只派发一次；取消某个等待者不取消共享请求。
    pub(crate) async fn get_in_round(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
    ) -> Result<Arc<MoogleMailReadDocument>, MoogleMailError> {
        if target.origin != self.builder.origin
            || !matches!(
                target.operation,
                MoogleMailOperation::Inbox
                    | MoogleMailOperation::ReadMail
                    | MoogleMailOperation::Sent
                    | MoogleMailOperation::SentDetail
                    | MoogleMailOperation::Detail
            )
        {
            return Err(MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::Configuration,
                false,
            ));
        }
        self.check_read_round(cookie, round).await?;
        round.check(cookie, target.world)?;
        let membership = if matches!(
            target.operation,
            MoogleMailOperation::Inbox | MoogleMailOperation::ReadMail
        ) {
            round.scope.membership_revision.load(Ordering::Acquire)
        } else {
            0
        };
        let key = format!("{membership}:{}", target.url);
        let mut receiver = {
            let mut requests = round.epoch.requests.lock().map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "lock_requests",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        ..Default::default()
                    },
                );
                read_unavailable(target.world)
            })?;
            // 当前等待者仍收到原错误；后续重试不复用失败结果，也不借替换条目重置请求预算。
            if let Some(receiver) = requests
                .entries
                .get(&key)
                .filter(|receiver| !matches!(receiver.borrow().as_ref(), Some(Err(_))))
            {
                receiver.clone()
            } else {
                if requests.started >= MAX_REQUESTS_PER_ROUND {
                    return Err(read_unavailable(target.world));
                }
                let permit = Arc::clone(&self.reads.jobs).try_acquire_owned().map_err(
                    |diagnostic_cause| {
                        safe_diagnostics::warn(
                            "mooglemail_read",
                            "reserve_job_slot",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext {
                                world: diagnostic_world(target.world.as_str()),
                                ..Default::default()
                            },
                        );
                        read_unavailable(target.world)
                    },
                )?;
                let (sender, receiver) = watch::channel(None);
                requests.started += 1;
                requests.entries.insert(key, receiver.clone());
                let client = self.clone();
                let target = target.clone();
                let cookie = cookie.clone();
                let round = round.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let lifetime = MAX_ROUND_LIFETIME.saturating_sub(round.epoch.started.elapsed());
                    let result = time::timeout(client.request_timeout.min(lifetime), async {
                        round.check(&cookie, target.world)?;
                        let mut body = request::SensitiveBytes(client.get(&target, &cookie).await?);
                        client.check_read_round(&cookie, &round).await?;
                        let bytes = match Arc::clone(&client.reads.bytes)
                            .try_acquire_many_owned(body.0.capacity() as u32)
                        {
                            Ok(bytes) => bytes,
                            Err(diagnostic_cause) => {
                                safe_diagnostics::warn(
                                    "mooglemail_read",
                                    "reserve_response_bytes",
                                    safe_diagnostics::project(&diagnostic_cause),
                                    DiagnosticContext {
                                        world: diagnostic_world(target.world.as_str()),
                                        ..Default::default()
                                    },
                                );

                                body.0.fill(0);
                                return Err(read_unavailable(target.world));
                            }
                        };
                        Ok(Arc::new(MoogleMailReadDocument {
                            body: std::mem::take(&mut body.0),
                            _bytes: bytes,
                        }))
                    })
                    .await
                    .unwrap_or_else(|_| Err(read_unavailable(target.world)));
                    sender.send_replace(Some(result));
                });
                receiver
            }
        };
        let result = loop {
            let result = receiver.borrow_and_update().clone();
            if let Some(result) = result {
                break result;
            }
            receiver.changed().await.map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "wait_response",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        ..Default::default()
                    },
                );
                read_unavailable(target.world)
            })?;
        };
        self.check_read_round(cookie, round).await?;
        result
    }

    pub(crate) async fn check_read_round(
        &self,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
    ) -> Result<(), MoogleMailError> {
        round.check(cookie, round.key.world)?;
        self.check_ownership().await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "check_round_owner",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(round.key.world.as_str()),
                    ..Default::default()
                },
            );
            unavailable(round.key.world)
        })?;
        let generation = self
            .ownership
            .as_ref()
            .and_then(|owner| owner.mail_runtime_binding())
            .map(|binding| binding.generation);
        if generation != round.key.generation {
            return Err(unavailable(round.key.world));
        }
        round.check(cookie, round.key.world)
    }

    pub(super) fn write_barrier(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<WriteBarrier, MoogleMailError> {
        let (_, scope) = self.read_scope(cookie, world)?;
        scope.invalidate();
        Ok(WriteBarrier(scope))
    }

    pub(super) async fn incoming_detail_guard(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<IncomingDetailGuard, MoogleMailError> {
        let (_, scope) = self.read_scope(cookie, world)?;
        let lock_wait = MailLockWaitGuard::start(
            Arc::clone(&self.external_metrics),
            world,
            MoogleMailOperation::Detail,
            auction_application::MailLock::IncomingWrite,
        );
        let guard = Arc::clone(&scope.incoming).write_owned().await;
        lock_wait.acquired();
        scope.membership_revision.fetch_add(1, Ordering::AcqRel);
        Ok(IncomingDetailGuard { scope, _guard: guard })
    }
}

impl MailboxReads {
    fn start_reaper(self: &Arc<Self>) {
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
    fn invalidate(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut state) = self.state.lock() {
            state.current = None;
            state.touched = Instant::now();
        }
    }
}

/// 正常完成、未知响应和取消都执行第二道屏障，拒绝写请求期间产生的旧观测。
pub(super) struct WriteBarrier(Arc<ReadScope>);

impl Drop for WriteBarrier {
    fn drop(&mut self) {
        self.0.invalidate();
    }
}

pub(super) struct IncomingDetailGuard {
    scope: Arc<ReadScope>,
    _guard: OwnedRwLockWriteGuard<()>,
}

impl Drop for IncomingDetailGuard {
    fn drop(&mut self) {
        self.scope.membership_revision.fetch_add(1, Ordering::AcqRel);
    }
}

fn read_unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::RemoteTemporary,
        true,
    )
}

fn unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::CredentialUnavailable,
        false,
    )
}
