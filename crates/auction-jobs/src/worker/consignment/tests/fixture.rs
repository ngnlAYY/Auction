//! 用可控时间和显式阻塞点替换寄售持久端口及来源。

use std::collections::VecDeque;
use std::future::{Future, ready};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::admin::EquipmentKeySealerPort;
use auction_application::consignment::{
    AdminDelistListingCommand, ApplyListingLookupCommand, AutoCloseCandidate, AutoCloseEvidence,
    AutoCloseListingCommand, ConsignmentError, ConsignmentFuture, ListingAdmission,
    ListingAutoCloseEffect, ListingClosed, ListingCommandOutcome, ListingCommandStore,
    ListingCredentialError, ListingEquipmentKeyOpener, ListingEquipmentLookup,
    ListingEquipmentLookupError, ListingLookupBudget, ListingLookupClaim, ListingLookupEffect,
    ListingLookupSnapshot, ListingMaintenance, ListingMaintenancePage, ListingMaintenanceService,
    ListingPriceReceipt, ListingRegistered, ListingRegistrationSecret, ListingWorkerReadiness,
    LookupAbandonment, LookupClaimRequest, LookupDispatch, LookupFence, LookupLeaseUpdate,
    LookupPermitStatus, LookupWorkStore, RegisterListingCommand, RegistrationReplayCrypto,
    RepriceListingCommand, ResolveListingService, WithdrawListingCommand,
};
use auction_application::core::Clock;
use auction_domain::consignment::{ListingEquipmentIdentity, ListingPublicationTtls};
use auction_domain::{EquipmentId, EquipmentKey, EquipmentKeyEnvelope, LotId};
use auction_observability::WorkerResult;
use auction_time::{TimeDelta, Timestamp};
use tokio::sync::{Notify, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, advance};
use uuid::Uuid;

use crate::worker::consignment::{
    ConsignmentDependencies, ConsignmentLookupRuntime, ConsignmentLoop, ConsignmentReadinessProbe,
    ConsignmentWorker,
};

pub(super) const EPOCH: u64 = 7;

pub(super) fn time(seconds: i64) -> Timestamp {
    Timestamp::from_timestamp(seconds, 0).unwrap()
}

struct TestClock(Instant);

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        time(0) + TimeDelta::from_std(self.0.elapsed()).unwrap()
    }
}

#[derive(Default)]
pub(super) struct Gate {
    pub blocked: AtomicBool,
    pub active: AtomicBool,
    pub resume: Notify,
}

impl Gate {
    async fn wait(&self) {
        if self.blocked.load(Ordering::Relaxed) {
            self.active.store(true, Ordering::Relaxed);
            let _active = InProgress(&self.active);
            self.resume.notified().await;
        }
    }
}

struct InProgress<'a>(&'a AtomicBool);

impl Drop for InProgress<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

pub(super) struct Fixture {
    pub work: Arc<Work>,
    pub commands: Arc<Commands>,
    pub equipment: Arc<Equipment>,
    pub scans: Arc<Scans>,
    pub readiness: Arc<Readiness>,
    pub wakeup: Arc<Notify>,
    pub paused: Arc<AtomicBool>,
    clock: Arc<TestClock>,
}

impl Fixture {
    pub(super) fn new() -> Self {
        Self {
            work: Arc::default(),
            commands: Arc::default(),
            equipment: Arc::default(),
            scans: Arc::default(),
            readiness: Arc::default(),
            wakeup: Arc::default(),
            paused: Arc::default(),
            clock: Arc::new(TestClock(Instant::now())),
        }
    }

    pub(super) async fn prepare(
        &self,
        enabled: bool,
        closed_has_stock: impl Future<Output = Result<bool, ConsignmentError>>,
    ) -> Result<Option<ConsignmentWorker>, ConsignmentError> {
        self.prepare_with_capability(enabled, true, closed_has_stock).await
    }

    pub(super) async fn prepare_with_capability(
        &self,
        enabled: bool,
        artifact_capable: bool,
        closed_has_stock: impl Future<Output = Result<bool, ConsignmentError>>,
    ) -> Result<Option<ConsignmentWorker>, ConsignmentError> {
        let paused = self.paused.clone();
        let lookup = enabled.then(|| ConsignmentLookupRuntime {
            service: ResolveListingService::new(
                self.work.clone(),
                self.commands.clone(),
                self.equipment.clone(),
                self.equipment.clone(),
                move || paused.load(Ordering::Relaxed),
                self.clock.clone(),
                ListingPublicationTtls::new(30, 7).unwrap(),
            ),
            work: self.work.clone(),
            budget: ListingLookupBudget::new(10, 1000).unwrap(),
            clock: self.clock.clone(),
            worker_id: "测试 Worker".to_owned(),
            wakeup: self.wakeup.clone(),
            readiness: self.readiness.clone(),
        });
        ConsignmentWorker::prepare(
            enabled,
            artifact_capable,
            ListingMaintenanceService::new(self.scans.clone(), self.commands.clone()),
            lookup,
            closed_has_stock,
        )
        .await
    }

    pub(super) async fn start(&self) -> Running {
        let worker = self
            .prepare(true, async { panic!("启用启动不应探测停用存量") })
            .await
            .unwrap()
            .unwrap();
        Running::spawn(worker)
    }
}

pub(super) struct Running {
    pub drain: watch::Sender<bool>,
    pub tasks: JoinSet<()>,
    pub observations: Arc<Mutex<Vec<(ConsignmentLoop, WorkerResult, u32)>>>,
}

impl Running {
    pub(super) fn spawn(worker: ConsignmentWorker) -> Self {
        let (drain, receiver) = watch::channel(false);
        let observations = Arc::new(Mutex::new(Vec::new()));
        let recorded = observations.clone();
        let mut tasks = JoinSet::new();
        tasks.spawn(worker.run(receiver, move |kind, result, _duration, failed| {
            recorded.lock().unwrap().push((kind, result, failed));
        }));
        Self { drain, tasks, observations }
    }

    pub(super) async fn finish(mut self) {
        self.drain.send_replace(true);
        self.tasks.join_next().await.unwrap().unwrap();
    }
}

pub(super) async fn settle() {
    // 运行在同一 Tokio 线程；显式让已就绪分支注册计时器，负例检查时不触发自动推进。
    for _ in 0..12 {
        tokio::task::yield_now().await;
    }
}

pub(super) async fn advance_ms(ms: u64) {
    advance(Duration::from_millis(ms)).await;
    settle().await;
}

#[derive(Clone, Copy)]
pub(super) enum ClaimStep {
    Idle,
    Claim,
    Error(ConsignmentError),
}

#[derive(Default)]
pub(super) struct Work {
    pub steps: Mutex<VecDeque<ClaimStep>>,
    pub claims: Mutex<Vec<(Instant, u64, Uuid)>>,
    pub wake_at: AtomicI64,
    pub permit_reads: AtomicUsize,
    pub reaps: AtomicUsize,
    pub reap_fails: AtomicBool,
    pub ready: Mutex<Vec<(Instant, u64, ListingWorkerReadiness)>>,
    pub ready_fails: AtomicBool,
    pub abandons: Mutex<Vec<LookupAbandonment>>,
    pub abandon_gate: Gate,
    pub dispatch_gate: Gate,
}

impl LookupWorkStore for Work {
    fn claim(
        &self,
        request: LookupClaimRequest,
    ) -> ConsignmentFuture<'_, Option<ListingLookupClaim>> {
        Box::pin(async move {
            self.claims.lock().unwrap().push((
                Instant::now(),
                request.service_epoch(),
                request.token(),
            ));
            match self.steps.lock().unwrap().pop_front().unwrap_or(ClaimStep::Idle) {
                ClaimStep::Idle => Ok(None),
                ClaimStep::Error(error) => Err(error),
                ClaimStep::Claim => {
                    let fence = LookupFence::new(
                        Uuid::from_u128(43),
                        Uuid::from_u128(42),
                        1,
                        1,
                        request.token(),
                    )?;
                    let identity = ListingEquipmentIdentity::new("main", "42")?;
                    let envelope = EquipmentKeyEnvelope::new(
                        vec![4; 20],
                        vec![1; 12],
                        "fixture-key".to_owned(),
                        "ab".repeat(32),
                    )
                    .unwrap();
                    let credential = ListingRegistrationSecret::new(
                        EquipmentId::from_uuid(Uuid::from_u128(46)),
                        envelope,
                    );
                    Ok(Some(ListingLookupClaim::new(
                        fence,
                        identity,
                        request.service_epoch(),
                        1,
                        time(1000),
                        time(1005),
                        credential,
                    )?))
                }
            }
        })
    }

    fn renew(&self, _fence: LookupFence) -> ConsignmentFuture<'_, LookupLeaseUpdate<Timestamp>> {
        Box::pin(ready(Ok(LookupLeaseUpdate::Applied(time(1005)))))
    }

    fn confirm_dispatch<'a>(
        &'a self,
        _claim: &'a ListingLookupClaim,
        paused: bool,
    ) -> ConsignmentFuture<'a, LookupDispatch> {
        Box::pin(async move {
            self.dispatch_gate.wait().await;
            if paused {
                Ok(LookupDispatch::Paused)
            } else {
                Ok(LookupDispatch::Allowed { deadline: time(1000) })
            }
        })
    }

    fn abandon(
        &self,
        _fence: LookupFence,
        status: LookupAbandonment,
    ) -> ConsignmentFuture<'_, LookupLeaseUpdate<()>> {
        Box::pin(async move {
            self.abandons.lock().unwrap().push(status);
            self.abandon_gate.wait().await;
            Ok(LookupLeaseUpdate::Applied(()))
        })
    }

    fn reap_expired(&self) -> ConsignmentFuture<'_, u32> {
        Box::pin(async {
            self.reaps.fetch_add(1, Ordering::Relaxed);
            if self.reap_fails.load(Ordering::Relaxed) {
                Err(ConsignmentError::Unavailable)
            } else {
                Ok(0)
            }
        })
    }

    fn permit_status(&self) -> ConsignmentFuture<'_, LookupPermitStatus> {
        Box::pin(async {
            self.permit_reads.fetch_add(1, Ordering::Relaxed);
            Ok(LookupPermitStatus {
                as_of: time(0),
                service_epoch: EPOCH,
                admitting: true,
                next_allowed_at: time(self.wake_at.load(Ordering::Relaxed)),
                cooldown_until: None,
                permit_until: None,
            })
        })
    }

    fn record_worker_readiness(
        &self,
        service_epoch: u64,
        readiness: ListingWorkerReadiness,
    ) -> ConsignmentFuture<'_, ()> {
        Box::pin(async move {
            self.ready.lock().unwrap().push((Instant::now(), service_epoch, readiness));
            if self.ready_fails.load(Ordering::Relaxed) {
                Err(ConsignmentError::Unavailable)
            } else {
                Ok(())
            }
        })
    }
}

#[derive(Default)]
pub(super) struct Equipment {
    pub requests: AtomicUsize,
    pub gate: Gate,
}

impl ListingEquipmentKeyOpener for Equipment {
    fn open(
        &self,
        _credential: &ListingRegistrationSecret,
    ) -> Result<EquipmentKey, ListingCredentialError> {
        EquipmentKey::new("face00face").map_err(|_error| ListingCredentialError)
    }
}

impl ListingEquipmentLookup for Equipment {
    fn lookup<'a>(
        &'a self,
        _identity: &'a ListingEquipmentIdentity,
        _key: &'a EquipmentKey,
        _deadline: Timestamp,
    ) -> auction_application::core::TransactionFuture<
        'a,
        ListingLookupSnapshot,
        ListingEquipmentLookupError,
    > {
        Box::pin(async {
            self.requests.fetch_add(1, Ordering::Relaxed);
            self.gate.wait().await;
            Err(ListingEquipmentLookupError::SourceUnavailable)
        })
    }
}

#[derive(Default)]
pub(super) struct Commands {
    pub apply:
        Mutex<VecDeque<Result<ListingCommandOutcome<ListingLookupEffect>, ConsignmentError>>>,
    pub applied: AtomicUsize,
    pub apply_gate: Gate,
    pub close:
        Mutex<VecDeque<Result<ListingCommandOutcome<ListingAutoCloseEffect>, ConsignmentError>>>,
    pub fail_close_listing: Mutex<Option<Uuid>>,
    pub closed: Mutex<Vec<AutoCloseCandidate>>,
    pub close_gate: Gate,
}

impl ListingCommandStore for Commands {
    fn register<'a>(
        &'a self,
        _command: RegisterListingCommand<'a>,
        _sealer: Option<&'a dyn EquipmentKeySealerPort>,
        _replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
    ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
        panic!("Worker 循环不应登记挂单")
    }

    fn reprice(
        &self,
        _command: RepriceListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
        panic!("Worker 循环不应修改价格")
    }

    fn withdraw(
        &self,
        _command: WithdrawListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        panic!("Worker 循环不应主动取消")
    }

    fn apply_lookup(
        &self,
        _command: ApplyListingLookupCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
        Box::pin(async {
            self.applied.fetch_add(1, Ordering::Relaxed);
            self.apply_gate.wait().await;
            self.apply.lock().unwrap().pop_front().expect("解析测试需指定提交结果")
        })
    }

    fn auto_close(
        &self,
        command: AutoCloseListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
        Box::pin(async move {
            self.closed.lock().unwrap().push(command.candidate);
            self.close_gate.wait().await;
            if *self.fail_close_listing.lock().unwrap() == Some(command.candidate.listing_id) {
                return Err(ConsignmentError::Unavailable);
            }
            self.close
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(ListingCommandOutcome::Accepted(ListingAutoCloseEffect::Unchanged)))
        })
    }

    fn admin_delist(
        &self,
        _command: AdminDelistListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        panic!("Worker 循环不应发起管理员下架")
    }
}

#[derive(Default)]
pub(super) struct Scans {
    pub syncs: Mutex<Vec<bool>>,
    pub sync_error: Mutex<Option<ConsignmentError>>,
    pub scans: Mutex<Vec<Instant>>,
    pub pages: Mutex<VecDeque<ListingMaintenancePage>>,
}

impl ListingMaintenance for Scans {
    fn scan_candidates(&self) -> ConsignmentFuture<'_, ListingMaintenancePage> {
        Box::pin(async {
            self.scans.lock().unwrap().push(Instant::now());
            Ok(self.pages.lock().unwrap().pop_front().unwrap_or_else(|| page(vec![], None)))
        })
    }

    fn sync_admission(&self, enabled: bool) -> ConsignmentFuture<'_, ListingAdmission> {
        Box::pin(async move {
            self.syncs.lock().unwrap().push(enabled);
            if let Some(error) = *self.sync_error.lock().unwrap() {
                return Err(error);
            }
            Ok(ListingAdmission { service_epoch: EPOCH, admitting: enabled })
        })
    }
}

pub(super) fn page(
    candidates: Vec<AutoCloseCandidate>,
    scan_after: Option<Uuid>,
) -> ListingMaintenancePage {
    ListingMaintenancePage { scanned: candidates.len().try_into().unwrap(), candidates, scan_after }
}

pub(super) fn candidate(listing: u128) -> AutoCloseCandidate {
    let evidence = match listing {
        1 => AutoCloseEvidence::LookupDeadline(time(10)),
        2 => AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(Uuid::from_u128(50))),
        3 => AutoCloseEvidence::SellerIneligible(Uuid::from_u128(51)),
        _ => AutoCloseEvidence::ServiceDisabled(EPOCH),
    };
    AutoCloseCandidate { listing_id: Uuid::from_u128(listing), evidence }
}

pub(super) struct Readiness {
    pub deps: Mutex<ConsignmentDependencies>,
    pub error: Mutex<Option<ConsignmentError>>,
    pub gate: Gate,
    pub calls: AtomicUsize,
}

impl Default for Readiness {
    fn default() -> Self {
        Self {
            deps: Mutex::new(ConsignmentDependencies {
                database: true,
                schema: true,
                opener: true,
                current_keyring: true,
                lookup: true,
            }),
            error: Mutex::default(),
            gate: Gate::default(),
            calls: AtomicUsize::default(),
        }
    }
}

impl ConsignmentReadinessProbe for Readiness {
    fn dependencies(&self) -> ConsignmentFuture<'_, ConsignmentDependencies> {
        Box::pin(async {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.gate.wait().await;
            if let Some(error) = *self.error.lock().unwrap() {
                return Err(error);
            }
            Ok(*self.deps.lock().unwrap())
        })
    }
}
