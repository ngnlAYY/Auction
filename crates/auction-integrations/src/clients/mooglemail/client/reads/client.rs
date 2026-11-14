//! MoogleMail 客户端的分轮共享读取：请求复用、并发上限与读取预算。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use chrono::Utc;
use tokio::sync::{RwLock, watch};
use tokio::time;

use super::scope::{IncomingDetailGuard, WriteBarrier, read_unavailable, unavailable};
use super::{
    MAX_REQUESTS_PER_ROUND, MAX_ROUND_LIFETIME, MAX_SCOPES, MoogleMailReadDocument,
    MoogleMailReadRound, REUSE_WINDOW, ReadEpoch, ReadRequests, ReadScope, ScopeKey, ScopeState,
    lists,
};
use crate::clients::metrics::MailLockWaitGuard;
use crate::clients::mooglemail::client::{MoogleMailClient, request};
use crate::clients::mooglemail::protocol::MoogleMailCookie;
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailTarget,
    MoogleMailWorld,
};
use crate::safe_diagnostics;

impl MoogleMailClient {
    pub(super) fn read_scope(
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

    pub(crate) fn write_barrier(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<WriteBarrier, MoogleMailError> {
        let (_, scope) = self.read_scope(cookie, world)?;
        scope.invalidate();
        Ok(WriteBarrier(scope))
    }

    pub(crate) async fn incoming_detail_guard(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<IncomingDetailGuard, MoogleMailError> {
        let (_, scope) = self.read_scope(cookie, world)?;
        let lock_wait = MailLockWaitGuard::start(
            Arc::clone(&self.external_metrics),
            world,
            MoogleMailOperation::Detail,
            auction_application::external_metrics::MailLock::IncomingWrite,
        );
        let guard = Arc::clone(&scope.incoming).write_owned().await;
        lock_wait.acquired();
        scope.membership_revision.fetch_add(1, Ordering::AcqRel);
        Ok(IncomingDetailGuard { scope, _guard: guard })
    }
}
