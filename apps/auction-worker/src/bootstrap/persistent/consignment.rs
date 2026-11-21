//! 注入一口价挂单的持久端口、装备来源及登记就绪探针。

use std::future::Future;
use std::ops::AsyncFnOnce;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::Context;
use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, ListingCommandStore, ListingEquipmentLookup,
    ListingLookupBudget, ListingMaintenance, ListingMaintenanceService, LookupWorkStore,
    ResolveListingService, auto_close_command, lookup_reap_exhausted_identity,
};
use auction_application::core::ApplicationFuture;
use auction_application::core::diagnostics::DiagnosticError;
use auction_application::runtime_switches::RuntimeSwitches;
use auction_config::{AppConfig, DataKeyDomain};
use auction_integrations::{EquipmentKeyOpener, HvDataEquipmentReader, Keyring, build_keyring};
use auction_jobs::consignment::{
    ConsignmentDependencies, ConsignmentLookupRuntime, ConsignmentReadinessProbe, ConsignmentWorker,
};
use auction_storage::DatabaseStore;
use tokio::sync::Notify;

use crate::bootstrap::SystemClock;

#[cfg(test)]
pub(in crate::bootstrap) use tests::consignment_test_config;

/// 持久启动预检使用的寄售端口。
pub(super) struct ConsignmentPorts {
    database: Arc<dyn LookupDatabaseReadiness>,
    maintenance: Arc<dyn ListingMaintenance>,
    commands: Arc<dyn ListingCommandStore>,
    work: Arc<dyn LookupWorkStore>,
}

impl ConsignmentPorts {
    /// 使用同一数据库实例装配申请、清退及准入端口。
    pub(super) fn postgres(store: &Arc<DatabaseStore>) -> Self {
        Self {
            database: store.clone(),
            maintenance: store.clone(),
            commands: store.clone(),
            work: Arc::new(
                store.consignment_lookup(auto_close_command, lookup_reap_exhausted_identity),
            ),
        }
    }
}

/// 用配置中的正文上限装配固定来源。
pub(super) fn hvdata_lookup(
    maximum_bytes: usize,
) -> anyhow::Result<Arc<dyn ListingEquipmentLookup>> {
    Ok(Arc::new(HvDataEquipmentReader::new(maximum_bytes)?))
}

/// 准入同步后才许可持久任务启动，关停后的存量只交给补扫。
pub(super) async fn start_persistent(
    config: &AppConfig,
    ports: ConsignmentPorts,
    switches: &RuntimeSwitches,
    worker_id: &str,
    closed_has_stock: impl Future<Output = Result<bool, ConsignmentError>>,
    make_lookup: impl FnOnce(usize) -> anyhow::Result<Arc<dyn ListingEquipmentLookup>>,
    start: impl AsyncFnOnce(Option<ConsignmentWorker>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    // 不变量 2、5、6：先确认寄售代次同步，再启动持久任务或宣布 Worker 就绪。
    let consignment =
        prepare(config, ports, switches, worker_id, closed_has_stock, make_lookup).await?;
    start(consignment).await
}

async fn prepare(
    config: &AppConfig,
    ports: ConsignmentPorts,
    switches: &RuntimeSwitches,
    worker_id: &str,
    closed_has_stock: impl Future<Output = Result<bool, ConsignmentError>>,
    make_lookup: impl FnOnce(usize) -> anyhow::Result<Arc<dyn ListingEquipmentLookup>>,
) -> anyhow::Result<Option<ConsignmentWorker>> {
    // run_config 已验证这份启动配置的制品能力，装配过程只读取同一份配置。
    let enabled = config.consignment.enabled();
    let lookup = if enabled {
        ports.database.check_schema().await.context("一口价挂单的数据库 schema 未就绪")?;
        Some(lookup_runtime(config, &ports, switches.clone(), worker_id, make_lookup)?)
    } else {
        None
    };
    let maintenance = ListingMaintenanceService::new(ports.maintenance, ports.commands);
    // 不变量 2、5、6：必须确认代次同步提交，存量探针只在停用后运行，错误保持失败关闭。
    ConsignmentWorker::prepare(enabled, true, maintenance, lookup, closed_has_stock)
        .await
        .context("一口价挂单准入同步或存量检查失败")
}

fn lookup_runtime(
    config: &AppConfig,
    ports: &ConsignmentPorts,
    switches: RuntimeSwitches,
    worker_id: &str,
    make_lookup: impl FnOnce(usize) -> anyhow::Result<Arc<dyn ListingEquipmentLookup>>,
) -> anyhow::Result<ConsignmentLookupRuntime> {
    let key_domain = DataKeyDomain::Equipment;
    // 不变量 7：开封器同时持有当前钥和保留的旧钥，错误只保留安全分类。
    let keyring = build_keyring(
        key_domain,
        config.data_key_current(key_domain),
        config.data_key_previous(key_domain).iter().map(|key| (key.id(), key.material())),
    )
    .context("装配一口价挂单的装备数据密钥环失败")?
    .context("启用一口价挂单的 Worker 必须配置当前装备数据密钥")?;
    let opener = Arc::new(EquipmentKeyOpener::new(Arc::clone(&keyring)));
    let settings = config.consignment;
    let lookup = make_lookup(settings.lookup_max_response_bytes())?;
    let readiness =
        InstalledLookupReadiness::new(ports.database.clone(), &opener, &keyring, &lookup);
    // 不变量 5：预算交由领取事务计算数据库硬截止，经解析用例传给固定来源。
    let budget = ListingLookupBudget::new(
        settings.lookup_timeout_seconds(),
        settings.lookup_min_interval_ms(),
    )?;
    let work = ports.work.clone();
    let clock: Arc<dyn auction_application::core::Clock> = Arc::new(SystemClock);
    let service = ResolveListingService::new(
        work.clone(),
        ports.commands.clone(),
        opener,
        lookup,
        move || switches.external_access_paused(),
        clock.clone(),
        settings.publication_ttls(),
    );
    Ok(ConsignmentLookupRuntime {
        service,
        work,
        budget,
        clock,
        worker_id: worker_id.to_owned(),
        wakeup: Arc::new(Notify::new()),
        readiness: Arc::new(readiness),
    })
}

trait LookupDatabaseReadiness: Send + Sync {
    fn check_database(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;

    fn check_schema(&self) -> ApplicationFuture<'_, ()>;
}

impl LookupDatabaseReadiness for DatabaseStore {
    fn check_database(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        // 连接池用实际连接验证数据库可达；短预算避免故障时阻塞整个心跳轮次。
        Box::pin(async {
            tokio::time::timeout(Duration::from_millis(750), self.postgres_pool().acquire())
                .await
                .is_ok_and(|result| result.is_ok())
        })
    }

    fn check_schema(&self) -> ApplicationFuture<'_, ()> {
        Box::pin(self.check_database_readiness())
    }
}

struct InstalledLookupReadiness {
    database: Arc<dyn LookupDatabaseReadiness>,
    opener: Weak<EquipmentKeyOpener>,
    keyring: Weak<Keyring>,
    lookup: Weak<dyn ListingEquipmentLookup>,
}

impl InstalledLookupReadiness {
    fn new(
        database: Arc<dyn LookupDatabaseReadiness>,
        opener: &Arc<EquipmentKeyOpener>,
        keyring: &Arc<Keyring>,
        lookup: &Arc<dyn ListingEquipmentLookup>,
    ) -> Self {
        // 探针只观察解析用例持有的实例，不能自己延长依赖的生命周期。
        Self {
            database,
            opener: Arc::downgrade(opener),
            keyring: Arc::downgrade(keyring),
            lookup: Arc::downgrade(lookup),
        }
    }
}

impl ConsignmentReadinessProbe for InstalledLookupReadiness {
    fn dependencies(&self) -> ConsignmentFuture<'_, ConsignmentDependencies> {
        Box::pin(async move {
            // 不变量 5、6：每次续期前重新取得数据库与 schema 的证明，禁止探测真实来源。
            let (database, schema) =
                tokio::join!(self.database.check_database(), self.database.check_schema());
            Ok(ConsignmentDependencies {
                database,
                schema: schema
                    .inspect_err(|error| {
                        tracing::debug!(
                            kind = error.diagnostic_kind().label(),
                            "一口价挂单心跳未取得 schema 就绪证明"
                        );
                    })
                    .is_ok(),
                opener: self.opener.upgrade().is_some(),
                current_keyring: self
                    .keyring
                    .upgrade()
                    .is_some_and(|keys| keys.domain() == DataKeyDomain::Equipment),
                lookup: self.lookup.upgrade().is_some(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::future::Future;
    use std::io::Write;
    use std::ops::AsyncFnOnce;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, Weak};
    use std::time::{Duration, Instant};

    use auction_application::admin::EquipmentKeySealerPort;
    use auction_application::consignment::{
        AdminDelistListingCommand, ApplyListingLookupCommand, AutoCloseListingCommand,
        ConsignmentError, ConsignmentFuture, ListingAdmission, ListingAutoCloseEffect,
        ListingClosed, ListingCommandOutcome, ListingCommandStore, ListingEquipmentLookup,
        ListingEquipmentLookupError, ListingLookupClaim, ListingLookupEffect,
        ListingLookupSnapshot, ListingMaintenance, ListingMaintenancePage,
        ListingMaintenanceService, ListingPriceReceipt, ListingRegistered,
        ListingRegistrationSecret, ListingWorkerReadiness, LookupAbandonment, LookupClaimRequest,
        LookupDispatch, LookupFence, LookupLeaseUpdate, LookupPermitStatus, LookupWorkStore,
        RegisterListingCommand, RegistrationReplayCrypto, RepriceListingCommand,
        WithdrawListingCommand,
    };
    use auction_application::core::{ApplicationFuture, RepositoryError, TransactionFuture};
    use auction_application::runtime_switches::{
        RuntimeSwitchEvent, RuntimeSwitchName, RuntimeSwitchOperation, RuntimeSwitchRead,
        RuntimeSwitches,
    };
    use auction_config::{AppConfig, DataKeyDomain, Role};
    use auction_domain::consignment::ListingEquipmentIdentity;
    use auction_domain::{EquipmentId, EquipmentKey};
    use auction_integrations::{EquipmentKeyOpener, EquipmentKeySealer, Keyring};
    use auction_jobs::consignment::{ConsignmentLoop, ConsignmentWorker};
    use auction_observability::WorkerResult;
    use auction_time::Timestamp;
    use tokio::sync::{Notify, watch};
    use tokio::task::JoinHandle;
    use tokio::time::{advance, timeout};
    use uuid::Uuid;

    use super::{
        ConsignmentPorts, InstalledLookupReadiness, LookupDatabaseReadiness, lookup_runtime,
        start_persistent,
    };

    const EPOCH: u64 = 7;
    const TEST_KEY_ID: &str = "fixture-equipment-key";
    const TEST_KEY_MATERIAL: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    pub(in crate::bootstrap) fn consignment_test_config() -> AppConfig {
        let directory =
            std::env::temp_dir().join(format!("p2j-worker-{}", Uuid::new_v4().simple()));
        fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let worker_path = directory.join("worker.toml");
        fs::write(&worker_path, "common_file = \"common.toml\"\nsecrets_file = \"secrets.toml\"")
            .unwrap();
        fs::write(
        directory.join("common.toml"),
        "[consignment]\nenabled = true\nlookup_timeout_seconds = 17\nlookup_max_response_bytes = 8192",
    )
    .unwrap();
        let mut secrets = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join("secrets.toml"))
            .unwrap();
        writeln!(
        secrets,
        "database_url = \"postgresql://auction_runtime:fixture-password@127.0.0.1:5432/fixture\"\n\
         equipment_data_key_id = \"{TEST_KEY_ID}\"\nequipment_data_key = \"{TEST_KEY_MATERIAL}\""
    )
    .unwrap();
        let config = AppConfig::load_from_path(Role::Worker, worker_path);
        fs::remove_dir_all(directory).unwrap();
        config.unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn persistent_start_waits_for_admission_before_starting_tasks() {
        for sync_fails in [true, false] {
            let fixture = Fixture::new();
            fixture.ports.sync_fails.store(sync_fails, Ordering::Relaxed);
            let starts = Cell::new(0);
            let config = consignment_test_config();
            let switches = RuntimeSwitches::default();
            let startup = fixture.start(&config, &switches, async |worker| {
                starts.set(starts.get() + 1);
                let mut running = Running::new(worker.unwrap());
                assert_eq!(running.next_heartbeat().await, WorkerResult::Success);
                running.finish().await;
                Ok(())
            });
            tokio::pin!(startup);
            tokio::select! {
                result = &mut startup => panic!("准入同步尚未完成，装配入口提前返回：{result:?}"),
                () = fixture.ports.sync_entered.notified() => {}
            }
            assert_eq!(starts.get(), 0);
            assert_eq!(fixture.ports.claims.load(Ordering::Relaxed), 0);
            assert_eq!(fixture.ports.heartbeats.load(Ordering::Relaxed), 0);
            fixture.ports.sync_release.notify_one();
            let result = startup.await;
            if sync_fails {
                assert!(format!("{:#}", result.unwrap_err()).contains("准入同步或存量检查失败"));
                assert_eq!(starts.get(), 0);
                assert_eq!(fixture.ports.claims.load(Ordering::Relaxed), 0);
                assert_eq!(fixture.ports.heartbeats.load(Ordering::Relaxed), 0);
            } else {
                result.unwrap();
                assert_eq!(starts.get(), 1);
                assert!(fixture.ports.claims.load(Ordering::Relaxed) > 0);
                assert_eq!(fixture.ports.heartbeats.load(Ordering::Relaxed), 1);
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn persistent_lookup_uses_configured_source_limits_and_shared_pause() {
        let fixture = Fixture::new();
        fixture.ports.sync_release.notify_one();
        let keyring = test_keyring();
        let aad = EquipmentId::from_uuid(Uuid::from_u128(11));
        let envelope = EquipmentKeySealer::new(keyring.current().clone())
            .seal(aad, &EquipmentKey::new("0123456789").unwrap())
            .unwrap();
        *fixture.ports.credential.lock().unwrap() =
            Some(ListingRegistrationSecret::new(aad, envelope).unwrap());

        let config = consignment_test_config();
        let switches = RuntimeSwitches::default();
        // 安装在启动前，恢复时更新同一份快照，验证循环读取的是共享暂停闭包。
        set_pause(&switches, true);
        fixture
            .start(&config, &switches, async |worker| {
                let mut running = Running::new(worker.unwrap());
                assert_eq!(running.next_heartbeat().await, WorkerResult::Success);
                assert_eq!(*fixture.maximum_bytes.lock().unwrap(), Some(8192));
                assert_eq!(fixture.ports.claims.load(Ordering::Relaxed), 0);

                set_pause(&switches, false);
                timeout(Duration::from_secs(6), fixture.source.requested.notified()).await.unwrap();
                let elapsed =
                    (*fixture.source.deadline.lock().unwrap()).unwrap() - fixture.ports.now;
                assert_eq!(elapsed.to_std().unwrap(), Duration::from_secs(17));
                assert_eq!(fixture.ports.claims.load(Ordering::Relaxed), 1);
                running.finish().await;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn persistent_heartbeat_checks_database_and_schema_before_each_renewal() {
        let fixture = Fixture::new();
        fixture.ports.sync_release.notify_one();
        let config = consignment_test_config();
        let switches = RuntimeSwitches::default();
        fixture
            .start(&config, &switches, async |worker| {
                let mut running = Running::new(worker.unwrap());
                assert_eq!(running.next_heartbeat().await, WorkerResult::Success);
                assert_eq!(fixture.ports.heartbeats.load(Ordering::Relaxed), 1);
                for dependency in [&fixture.ports.database_ready, &fixture.ports.schema_ready] {
                    dependency.store(false, Ordering::Relaxed);
                    advance(Duration::from_secs(10)).await;
                    assert_eq!(running.next_heartbeat().await, WorkerResult::Error);
                    assert_eq!(fixture.ports.heartbeats.load(Ordering::Relaxed), 1);
                    dependency.store(true, Ordering::Relaxed);
                }
                advance(Duration::from_secs(10)).await;
                assert_eq!(running.next_heartbeat().await, WorkerResult::Success);
                assert_eq!(fixture.ports.heartbeats.load(Ordering::Relaxed), 2);
                running.finish().await;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn persistent_heartbeat_requires_each_installed_lookup_handle() {
        let config = consignment_test_config();
        let switches = RuntimeSwitches::default();
        let fixture = Fixture::new();
        let keyring = Arc::new(test_keyring());
        let opener = Arc::new(EquipmentKeyOpener::new(Arc::clone(&keyring)));
        let lookup: Arc<dyn ListingEquipmentLookup> = fixture.source.clone();

        for missing in ["opener", "current_keyring", "lookup"] {
            let mut probe =
                InstalledLookupReadiness::new(fixture.ports.clone(), &opener, &keyring, &lookup);
            match missing {
                "opener" => probe.opener = Weak::new(),
                "current_keyring" => probe.keyring = Weak::new(),
                "lookup" => {
                    let absent: Arc<dyn ListingEquipmentLookup> = Arc::new(Source::default());
                    probe.lookup = Arc::downgrade(&absent);
                }
                _ => unreachable!(),
            }
            let ports = fixture.assembly_ports();
            let mut lookup =
                lookup_runtime(&config, &ports, switches.clone(), "fixture-worker", |bytes| {
                    fixture.make_lookup(bytes)
                })
                .unwrap();
            lookup.readiness = Arc::new(probe);
            fixture.ports.sync_release.notify_one();
            let worker = ConsignmentWorker::prepare(
                true,
                true,
                ListingMaintenanceService::new(ports.maintenance, ports.commands),
                Some(lookup),
                async { panic!("开启时不应检查停用存量") },
            )
            .await
            .unwrap()
            .unwrap();
            let mut running = Running::new(worker);
            assert_eq!(running.next_heartbeat().await, WorkerResult::Error, "{missing}");
            assert_eq!(fixture.ports.heartbeats.load(Ordering::Relaxed), 0, "{missing}");
            running.finish().await;
        }
    }

    fn test_keyring() -> Keyring {
        Keyring::from_base64(DataKeyDomain::Equipment, (TEST_KEY_ID, TEST_KEY_MATERIAL), [])
            .unwrap()
    }

    fn set_pause(switches: &RuntimeSwitches, paused: bool) {
        let now = Timestamp::now();
        switches
            .install(
                RuntimeSwitchRead {
                    database_time: now,
                    events: vec![RuntimeSwitchEvent {
                        command_id: Uuid::new_v4(),
                        name: RuntimeSwitchName::ExternalAccessPaused,
                        revision: if paused { 1 } else { 2 },
                        operation: RuntimeSwitchOperation::Set,
                        value: Some(serde_json::Value::Bool(paused)),
                        expires_at: None,
                        expired_command_id: None,
                        actor_id: "fixture".to_owned(),
                        entrypoint: "cli".to_owned(),
                        reason: "测试寄售暂停".to_owned(),
                        recorded_at: now,
                    }],
                },
                Instant::now(),
            )
            .unwrap();
        assert_eq!(switches.external_access_paused(), paused);
    }

    struct Running {
        drain: watch::Sender<bool>,
        heartbeat: tokio::sync::mpsc::UnboundedReceiver<WorkerResult>,
        task: JoinHandle<()>,
    }

    impl Running {
        fn new(worker: ConsignmentWorker) -> Self {
            let (drain, receiver) = watch::channel(false);
            let (record, heartbeat) = tokio::sync::mpsc::unbounded_channel();
            let task = tokio::spawn(worker.run(receiver, move |kind, result, _, _| {
                if kind == ConsignmentLoop::Readiness {
                    record.send(result).unwrap();
                }
            }));
            Self { drain, heartbeat, task }
        }

        async fn next_heartbeat(&mut self) -> WorkerResult {
            timeout(Duration::from_secs(2), self.heartbeat.recv()).await.unwrap().unwrap()
        }

        async fn finish(self) {
            self.drain.send_replace(true);
            self.task.await.unwrap();
        }
    }

    struct Fixture {
        ports: Arc<Ports>,
        source: Arc<Source>,
        maximum_bytes: Mutex<Option<usize>>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                ports: Arc::new(Ports {
                    now: Timestamp::now(),
                    sync_entered: Notify::new(),
                    sync_release: Notify::new(),
                    sync_fails: AtomicBool::new(false),
                    database_ready: AtomicBool::new(true),
                    schema_ready: AtomicBool::new(true),
                    claims: AtomicUsize::new(0),
                    heartbeats: AtomicUsize::new(0),
                    credential: Mutex::new(None),
                }),
                source: Arc::new(Source::default()),
                maximum_bytes: Mutex::new(None),
            }
        }

        fn assembly_ports(&self) -> ConsignmentPorts {
            ConsignmentPorts {
                database: self.ports.clone(),
                maintenance: self.ports.clone(),
                commands: self.ports.clone(),
                work: self.ports.clone(),
            }
        }

        fn make_lookup(&self, bytes: usize) -> anyhow::Result<Arc<dyn ListingEquipmentLookup>> {
            self.maximum_bytes.lock().unwrap().replace(bytes);
            Ok(self.source.clone())
        }

        async fn start(
            &self,
            config: &AppConfig,
            switches: &RuntimeSwitches,
            start: impl AsyncFnOnce(Option<ConsignmentWorker>) -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            start_persistent(
                config,
                self.assembly_ports(),
                switches,
                "fixture-worker",
                async { panic!("开启时不应检查停用存量") },
                |bytes| self.make_lookup(bytes),
                start,
            )
            .await
        }
    }

    #[derive(Default)]
    struct Source {
        deadline: Mutex<Option<Timestamp>>,
        requested: Notify,
    }

    impl ListingEquipmentLookup for Source {
        fn lookup<'a>(
            &'a self,
            _identity: &'a ListingEquipmentIdentity,
            _key: &'a EquipmentKey,
            deadline: Timestamp,
        ) -> TransactionFuture<'a, ListingLookupSnapshot, ListingEquipmentLookupError> {
            Box::pin(async move {
                self.deadline.lock().unwrap().replace(deadline);
                self.requested.notify_one();
                std::future::pending().await
            })
        }
    }

    struct Ports {
        now: Timestamp,
        sync_entered: Notify,
        sync_release: Notify,
        sync_fails: AtomicBool,
        database_ready: AtomicBool,
        schema_ready: AtomicBool,
        claims: AtomicUsize,
        heartbeats: AtomicUsize,
        credential: Mutex<Option<ListingRegistrationSecret>>,
    }

    impl LookupDatabaseReadiness for Ports {
        fn check_database(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            Box::pin(async { self.database_ready.load(Ordering::Relaxed) })
        }

        fn check_schema(&self) -> ApplicationFuture<'_, ()> {
            Box::pin(async {
                if self.schema_ready.load(Ordering::Relaxed) {
                    Ok(())
                } else {
                    Err(RepositoryError::Unavailable.into())
                }
            })
        }
    }

    impl ListingMaintenance for Ports {
        fn scan_candidates(&self) -> ConsignmentFuture<'_, ListingMaintenancePage> {
            Box::pin(async {
                Ok(ListingMaintenancePage { scanned: 0, candidates: vec![], scan_after: None })
            })
        }

        fn sync_admission(&self, enabled: bool) -> ConsignmentFuture<'_, ListingAdmission> {
            Box::pin(async move {
                self.sync_entered.notify_one();
                self.sync_release.notified().await;
                assert!(enabled);
                if self.sync_fails.load(Ordering::Relaxed) {
                    Err(ConsignmentError::OutcomeUnknown)
                } else {
                    Ok(ListingAdmission { service_epoch: EPOCH, admitting: true })
                }
            })
        }
    }

    impl LookupWorkStore for Ports {
        fn claim(
            &self,
            request: LookupClaimRequest,
        ) -> ConsignmentFuture<'_, Option<ListingLookupClaim>> {
            Box::pin(async move {
                self.claims.fetch_add(1, Ordering::Relaxed);
                assert_eq!(request.service_epoch(), EPOCH);
                let Some(credential) = self.credential.lock().unwrap().take() else {
                    return Ok(None);
                };
                // 模拟数据库发放硬截止。测试最终断言固定配置值到达来源，而非只核对领取参数。
                let deadline = self.now
                    + auction_time::Duration::from_std(request.budget().timeout()).unwrap();
                let fence = LookupFence::new(
                    Uuid::from_u128(21),
                    Uuid::from_u128(22),
                    1,
                    1,
                    request.token(),
                )
                .unwrap();
                ListingLookupClaim::new(
                    fence,
                    ListingEquipmentIdentity::new("main", "42").unwrap(),
                    EPOCH,
                    1,
                    deadline,
                    deadline + auction_time::Duration::seconds(5),
                    Ok(credential),
                )
                .map(Some)
            })
        }

        fn renew(
            &self,
            _fence: LookupFence,
        ) -> ConsignmentFuture<'_, LookupLeaseUpdate<Timestamp>> {
            Box::pin(async {
                Ok(LookupLeaseUpdate::Applied(self.now + auction_time::Duration::seconds(30)))
            })
        }

        fn confirm_dispatch<'a>(
            &'a self,
            claim: &'a ListingLookupClaim,
            paused: bool,
        ) -> ConsignmentFuture<'a, LookupDispatch> {
            Box::pin(async move {
                assert!(!paused);
                Ok(LookupDispatch::Allowed { deadline: claim.deadline() })
            })
        }

        fn abandon(
            &self,
            _fence: LookupFence,
            status: LookupAbandonment,
        ) -> ConsignmentFuture<'_, LookupLeaseUpdate<()>> {
            Box::pin(async move {
                assert_eq!(status, LookupAbandonment::MayBeInFlight);
                Ok(LookupLeaseUpdate::Applied(()))
            })
        }

        fn reap_expired(&self) -> ConsignmentFuture<'_, u32> {
            Box::pin(async { Ok(0) })
        }

        fn permit_status(&self) -> ConsignmentFuture<'_, LookupPermitStatus> {
            Box::pin(async {
                Ok(LookupPermitStatus {
                    as_of: self.now,
                    service_epoch: EPOCH,
                    admitting: true,
                    next_allowed_at: self.now,
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
                assert_eq!(service_epoch, EPOCH);
                assert_eq!(readiness, ListingWorkerReadiness::Ready);
                self.heartbeats.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
        }
    }

    impl ListingCommandStore for Ports {
        fn register<'a>(
            &'a self,
            _command: RegisterListingCommand<'a>,
            _sealer: Option<&'a dyn EquipmentKeySealerPort>,
            _replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
        ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
            panic!("装配测试不得登记")
        }

        fn reprice(
            &self,
            _command: RepriceListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
            panic!("装配测试不得改价")
        }

        fn withdraw(
            &self,
            _command: WithdrawListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
            panic!("装配测试不得取消")
        }

        fn apply_lookup(
            &self,
            _command: ApplyListingLookupCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
            panic!("装配测试不提交来源结果")
        }

        fn auto_close(
            &self,
            _command: AutoCloseListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
            panic!("装配测试没有清退候选")
        }

        fn admin_delist(
            &self,
            _command: AdminDelistListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
            panic!("装配测试不得下架")
        }
    }
}
