//! 注入一口价挂单的持久端口、装备来源及登记就绪探针。

use std::future::Future;
use std::ops::AsyncFnOnce;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::Context;
use auction_application::consignment::{
    AutoRelistService, AutoRelistSetting, ConsignmentError, ConsignmentFuture,
    DepositCommandService, LegacyAuctionOwnerResolver, ListingCommandStore, ListingEquipmentLookup,
    ListingLookupBudget, ListingMaintenance, ListingMaintenanceService, LookupWorkStore,
    PaymentReminderService, ResolveListingService, ReturnDispositionService, RightChallengeCrypto,
    auto_close_command, lookup_reap_exhausted_identity,
};
use auction_application::core::ApplicationFuture;
use auction_application::core::diagnostics::DiagnosticError;
use auction_application::mail::{DispositionMaintenanceService, QuarantineRecheckService};
use auction_application::runtime_switches::RuntimeSwitches;
use auction_config::{AppConfig, DataKeyDomain};
use auction_domain::consignment::ConsignmentCalendar;
use auction_integrations::{
    ConsignmentReservationCrypto, ConsignmentRightChallengeCrypto, ConsignmentSecretOpener,
    ConsignmentSecretSealer, EquipmentKeyOpener, HvDataEquipmentReader, Keyring, build_keyring,
};
use auction_jobs::consignment::{
    ConsignmentDependencies, ConsignmentLookupRuntime, ConsignmentReadinessProbe,
    ConsignmentWorker, DispositionMaintenanceRuntime, MailEvidenceReadinessRuntime,
    QuarantineRecheckRuntime, RecoveryRuntime, RightNoticeRuntime, SanctionExecutionRuntime,
    SeasonCalendarRuntime, SeasonTaskRuntime,
};
use auction_storage::{DatabaseStore, DepositRepository, DepositSettings, PgRightNoticeStore};
use tokio::sync::Notify;

use crate::bootstrap::SystemClock;
use crate::bootstrap::mail::MailControlRemoteReads;
use crate::bootstrap::mail::consignment::{
    ExecutorInputs, FrozenFirstDispatchContent, MoogleMailConsignmentTransport,
    MoogleMailRightNoticeTransport, SealedNoticeDeliverer, WorkerConsignmentRecheckReader,
    executor_runtimes,
};

#[cfg(test)]
pub(in crate::bootstrap) use tests::{consignment_config_with, consignment_test_config};

/// 持久启动预检使用的寄售端口。
pub(super) struct ConsignmentPorts {
    database: Arc<dyn LookupDatabaseReadiness>,
    maintenance: Arc<dyn ListingMaintenance>,
    commands: Arc<dyn ListingCommandStore>,
    work: Arc<dyn LookupWorkStore>,
    #[cfg(test)]
    missing_lookup_handle: Option<MissingLookupHandle>,
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
            #[cfg(test)]
            missing_lookup_handle: None,
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

/// 寄售 Worker 启动后、任务运行前接入处罚执行、恢复处置、处置维护、隔离重检、季任务、权利通知、
/// 派发 / 补寄 / 退件 Take 三个 executor、订单复核与收件就绪。
///
/// 关闭只停止新承诺：只要存在存量，维护循环仍启动；有存量却没有 4D1 读取器/传输时
/// 启动失败，不能静默跳过。处罚执行与恢复处置只依赖数据库，总是接入；权利通知是否运行由持久
/// 准入决定。
pub(super) async fn attach_loops(
    config: &AppConfig,
    store: &Arc<DatabaseStore>,
    switches: &RuntimeSwitches,
    worker_id: &str,
    worker: Option<ConsignmentWorker>,
    mail: Option<&MailControlRemoteReads>,
    mail_readiness: Vec<MailEvidenceReadinessRuntime>,
) -> anyhow::Result<Option<ConsignmentWorker>> {
    let Some(worker) = worker else {
        return Ok(None);
    };
    // 不变量 1、5：处罚账号工作只依赖数据库，幂等由存储端口保证；关闭寄售或未启用邮件时也要
    // 收口存量处罚，因此在任何邮件依赖检查之前接入。
    let worker = worker.with_sanctions(Arc::new(SanctionExecutionRuntime {
        store: store.clone(),
        wakeup: Arc::new(Notify::new()),
    }));
    // 不变量 4、5、10：退回处置、自动重列与付款提醒只写数据库（提醒经通用投递正规入队，外发不在
    // 本循环），关闭寄售也要收口已接受的订单。自动重列开关与配置段摘要只在装配根读取一次、重启
    // 生效，决定时冻结落库，之后改配置不影响已作的决定（J12）。
    let worker = worker.with_recovery(Arc::new(RecoveryRuntime {
        resolve: ReturnDispositionService::new(store.clone()),
        relist: AutoRelistService::new(store.clone()),
        reminders: PaymentReminderService::new(store.clone()),
        auto_relist: AutoRelistSetting {
            enabled: config.consignment.auto_relist_enabled(),
            config_digest: config.consignment.section_digest(),
        },
        max_open_per_user: config.consignment.max_open_per_user(),
        wakeup: Arc::new(Notify::new()),
    }));
    let has_work = store.has_consignment_work().await.context("读取寄售存量失败")?;
    // 没有邮件装配时也没有发信传输：权利通知不接入，挑战码在过期后由领取事务抑制。
    let Some(mail) = mail.filter(|mail| !mail.gateways.is_empty()) else {
        if has_work {
            anyhow::bail!("寄售仍有存量工作，但 MoogleMail 读取器/处置传输未装配");
        }
        return Ok(Some(worker.with_mail_readiness(mail_readiness)));
    };
    let key_domain = DataKeyDomain::Credential;
    let keyring = build_keyring(
        key_domain,
        config.data_key_current(key_domain),
        config.data_key_previous(key_domain).iter().map(|key| (key.id(), key.material())),
    )
    .context("装配寄售预约数据密钥环失败")?;
    let Some(keyring) = keyring else {
        if has_work {
            anyhow::bail!("寄售仍有存量工作，但未配置当前凭据数据密钥");
        }
        return Ok(Some(worker.with_mail_readiness(mail_readiness)));
    };
    // 挑战码与 Public 发码同用凭据域密钥环（不变量 7），只在发信前一刻解封。
    let challenge_crypto: Arc<dyn RightChallengeCrypto> =
        Arc::new(ConsignmentRightChallengeCrypto::new(Arc::clone(&keyring)));
    let sealer = ConsignmentSecretSealer::new(Arc::clone(keyring.current()));
    let opener = ConsignmentSecretOpener::new(keyring);
    let crypto: Arc<dyn auction_application::consignment::ReservationSecretCrypto> =
        Arc::new(ConsignmentReservationCrypto::new(sealer, opener));
    // 与 Public / Admin 同一构造：价格边界取领域常量，预约有效期取公共配置。
    let settings = DepositSettings::with_standard_price_bounds(
        config.consignment.fee_bps(),
        config.consignment.reservation_ttl_hours(),
        config.consignment.max_open_per_user(),
    )
    .map_err(|error| anyhow::anyhow!("寄售价格边界常量无效：{error:?}"))?;
    // 取得事务兑现撤回意愿时同提撤回件首轮，内容与派发同一冻结渲染（PIWX）。
    let repository = Arc::new(
        DepositRepository::new(store.as_ref().clone(), crypto, settings)
            .with_dispatch_content(Arc::new(FrozenFirstDispatchContent)),
    );
    let commands = Arc::new(DepositCommandService::new(
        repository.clone(),
        repository.clone(),
        repository.clone(),
        repository.clone(),
        repository.clone(),
    ));
    // 季窗与换季提醒同源；日历缺失/无效时只处理存量，不阻断 Worker 启动。
    let calendar = match config.auction.schedule_calendar() {
        Ok(Some(calendar)) if calendar.early_bird().is_some() => {
            match ConsignmentCalendar::from_early_bird_config(&config.auction.schedule.early_bird) {
                Ok(calendar) => Some(SeasonCalendarRuntime {
                    calendar,
                    timezone: config.auction.schedule.early_bird.timezone.clone(),
                }),
                Err(error) => {
                    tracing::warn!(error = %error, "寄售季窗日历构造失败，不冻结新季窗");
                    None
                }
            }
        }
        Ok(_) => {
            tracing::warn!("寄售季窗日历未启用，不冻结新季窗");
            None
        }
        Err(error) => {
            tracing::warn!(error = %error, "读取寄售季窗日历失败，不冻结新季窗");
            None
        }
    };
    let seasons = Arc::new(SeasonTaskRuntime {
        tasks: repository.clone(),
        commands: commands.clone(),
        calendar,
        preclose_days: config.consignment.isekai_preclose_days(),
        active: config.consignment.enabled(),
        clock: Arc::new(SystemClock),
        wakeup: Arc::new(Notify::new()),
    });
    let owners: Arc<dyn LegacyAuctionOwnerResolver> = store.clone();
    let reader = Arc::new(WorkerConsignmentRecheckReader::new(mail.gateways.clone(), owners));
    let transport = Arc::new(MoogleMailConsignmentTransport::new(mail.gateways.clone()));
    let disposition =
        DispositionMaintenanceService::new(commands.clone(), repository.clone(), transport.clone());
    let quarantine = QuarantineRecheckService::new(commands, repository, reader);
    let lease_duration =
        std::time::Duration::from_secs(config.worker.mooglemail.lease_duration_seconds);
    let paused = {
        let switches = switches.clone();
        Arc::new(move || switches.external_access_paused())
    };
    let disposition_runtime = Arc::new(DispositionMaintenanceRuntime {
        service: Arc::new(disposition),
        clock: Arc::new(SystemClock),
        worker_id: worker_id.to_owned(),
        lease_duration,
        wakeup: Arc::new(Notify::new()),
        paused: paused.clone(),
    });
    let quarantine_runtime = Arc::new(QuarantineRecheckRuntime {
        service: Arc::new(quarantine),
        clock: Arc::new(SystemClock),
        worker_id: worker_id.to_owned(),
        lease_duration,
        wakeup: Arc::new(Notify::new()),
        paused: paused.clone(),
    });
    // 不变量 4：领取提交后才发信，结果未知只记未知、不重发；收件人只取领取时的冻结身份。
    let right_notices = Arc::new(RightNoticeRuntime {
        store: Arc::new(PgRightNoticeStore::new(store.as_ref().clone())),
        deliverer: Arc::new(SealedNoticeDeliverer::new(
            challenge_crypto,
            Arc::new(MoogleMailRightNoticeTransport::new(Arc::clone(&mail.notices))),
        )),
        wakeup: Arc::new(Notify::new()),
        paused: paused.clone(),
    });
    // 不变量 4、5：三个 executor 与订单复核在邮件依赖检查之后接入，与通用投递、处置维护共用同一 transport 与
    // gateway。寄售关闭时四者都照存量收口运行：已成交订单的首派与补寄是续办旧权利
    // （D9 §355），新购买已在购买入口被关闭截止。
    let executors = executor_runtimes(ExecutorInputs {
        store: store.clone(),
        mail: Arc::clone(&mail.notices),
        disposition: transport,
        clock: Arc::new(SystemClock),
        worker_id: worker_id.to_owned(),
        lease_duration,
        paused,
    });
    Ok(Some(
        worker
            .with_disposition(disposition_runtime)
            .with_quarantine(quarantine_runtime)
            .with_seasons(seasons)
            .with_right_notices(right_notices)
            .with_dispatch(executors.dispatch)
            .with_redispatch(executors.redispatch)
            .with_downstream_rounds(executors.downstream_rounds)
            .with_return_takes(executors.return_takes)
            .with_order_rechecks(executors.order_rechecks)
            .with_collections(executors.collections)
            .with_mail_readiness(mail_readiness),
    ))
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
    // 探针只观察解析用例持有的实例，不能自己延长依赖的生命周期。
    let handles = LookupHandles {
        opener: Arc::downgrade(&opener),
        keyring: Arc::downgrade(&keyring),
        lookup: Arc::downgrade(&lookup),
    };
    #[cfg(test)]
    let handles = handles.without(ports.missing_lookup_handle);
    let readiness = InstalledLookupReadiness { database: ports.database.clone(), handles };
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
    handles: LookupHandles,
}

struct LookupHandles {
    opener: Weak<EquipmentKeyOpener>,
    keyring: Weak<Keyring>,
    lookup: Weak<dyn ListingEquipmentLookup>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
enum MissingLookupHandle {
    Opener,
    CurrentKeyring,
    Lookup,
}

#[cfg(test)]
impl LookupHandles {
    fn without(mut self, missing: Option<MissingLookupHandle>) -> Self {
        // 解析用例必须持有可调用的端口；装配时单独模拟探针缺少某一条引用。
        match missing {
            Some(MissingLookupHandle::Opener) => self.opener = Weak::new(),
            Some(MissingLookupHandle::CurrentKeyring) => self.keyring = Weak::new(),
            Some(MissingLookupHandle::Lookup) => self.lookup = Weak::<HvDataEquipmentReader>::new(),
            None => {}
        }
        self
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
                opener: self.handles.opener.upgrade().is_some(),
                current_keyring: self
                    .handles
                    .keyring
                    .upgrade()
                    .is_some_and(|keys| keys.domain() == DataKeyDomain::Equipment),
                lookup: self.handles.lookup.upgrade().is_some(),
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
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use auction_application::admin::EquipmentKeySealerPort;
    use auction_application::consignment::{
        AdminDelistListingCommand, ApplyListingLookupCommand, AutoCloseListingCommand,
        ConsignmentError, ConsignmentFuture, ListingAdmission, ListingAutoCloseEffect,
        ListingClosed, ListingCommandOutcome, ListingCommandStore, ListingEquipmentLookup,
        ListingEquipmentLookupError, ListingLookupClaim, ListingLookupEffect,
        ListingLookupSnapshot, ListingMaintenance, ListingMaintenancePage, ListingPriceReceipt,
        ListingRegistered, ListingRegistrationSecret, ListingWorkerReadiness, LookupAbandonment,
        LookupClaimRequest, LookupDispatch, LookupFence, LookupLeaseUpdate, LookupPermitStatus,
        LookupWorkStore, RegisterListingCommand, RegistrationReplayCrypto, RepriceListingCommand,
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
    use auction_integrations::{EquipmentKeySealer, HvDataConfigurationError, Keyring};
    use auction_jobs::consignment::{ConsignmentLoop, ConsignmentWorker};
    use auction_observability::WorkerResult;
    use auction_time::Timestamp;
    use tokio::sync::{Notify, watch};
    use tokio::task::JoinHandle;
    use tokio::time::{advance, timeout};
    use uuid::Uuid;

    use super::{
        ConsignmentPorts, LookupDatabaseReadiness, MissingLookupHandle, hvdata_lookup,
        start_persistent,
    };

    const EPOCH: u64 = 7;
    const TEST_KEY_ID: &str = "fixture-equipment-key";
    const TEST_KEY_MATERIAL: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    pub(in crate::bootstrap) fn consignment_test_config() -> AppConfig {
        consignment_config_with(true, true)
    }

    /// 寄售开关与数据库连接可选的 Worker 测试配置；其余字段与 [`consignment_test_config`] 相同。
    pub(in crate::bootstrap) fn consignment_config_with(
        enabled: bool,
        database: bool,
    ) -> AppConfig {
        let directory =
            std::env::temp_dir().join(format!("p2j-worker-{}", Uuid::new_v4().simple()));
        fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let worker_path = directory.join("worker.toml");
        fs::write(&worker_path, "common_file = \"common.toml\"\nsecrets_file = \"secrets.toml\"")
            .unwrap();
        fs::write(
            directory.join("common.toml"),
            format!(
                "[consignment]\nenabled = {enabled}\nlookup_timeout_seconds = 17\n\
                 lookup_max_response_bytes = 8192"
            ),
        )
        .unwrap();
        let mut secrets = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join("secrets.toml"))
            .unwrap();
        if database {
            writeln!(
                secrets,
                "database_url = \"postgresql://auction_runtime:fixture-password@127.0.0.1:5432/fixture\""
            )
            .unwrap();
        }
        writeln!(
            secrets,
            "equipment_data_key_id = \"{TEST_KEY_ID}\"\nequipment_data_key = \"{TEST_KEY_MATERIAL}\""
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

    #[test]
    fn hvdata_lookup_rejects_invalid_response_limit() {
        let config = consignment_test_config();
        assert!(hvdata_lookup(config.consignment.lookup_max_response_bytes()).is_ok());
        let result = hvdata_lookup(4095);
        assert!(matches!(
            result,
            Err(error)
                if error.downcast_ref::<HvDataConfigurationError>()
                    == Some(&HvDataConfigurationError::ResponseLimit)
        ));
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
        for (missing, result, renewals) in [
            (None, WorkerResult::Success, [1, 2]),
            (Some(MissingLookupHandle::Opener), WorkerResult::Error, [0, 0]),
            (Some(MissingLookupHandle::CurrentKeyring), WorkerResult::Error, [0, 0]),
            (Some(MissingLookupHandle::Lookup), WorkerResult::Error, [0, 0]),
        ] {
            let mut fixture = Fixture::new();
            fixture.missing_lookup_handle = missing;
            fixture.ports.sync_release.notify_one();
            fixture
                .start(&config, &switches, async |worker| {
                    let mut running = Running::new(worker.unwrap());
                    for expected_renewals in renewals {
                        assert_eq!(running.next_heartbeat().await, result, "{missing:?}");
                        assert_eq!(
                            fixture.ports.heartbeats.load(Ordering::Relaxed),
                            expected_renewals,
                            "{missing:?}"
                        );
                        advance(Duration::from_secs(10)).await;
                    }
                    running.finish().await;
                    Ok(())
                })
                .await
                .unwrap();
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
        missing_lookup_handle: Option<MissingLookupHandle>,
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
                missing_lookup_handle: None,
            }
        }

        fn assembly_ports(&self) -> ConsignmentPorts {
            ConsignmentPorts {
                database: self.ports.clone(),
                maintenance: self.ports.clone(),
                commands: self.ports.clone(),
                work: self.ports.clone(),
                missing_lookup_handle: self.missing_lookup_handle,
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
