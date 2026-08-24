mod listener;

use std::time::Duration;

use anyhow::Context;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::AdminForumPreviewPort;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::{
    AdminAuctionCreatePort, AdminAuctionReader, AdminAuditPort, AdminCredentialHealthReader,
    AdminCredentialPort, AdminCredentialService, AdminIntakePort, AdminLotReader,
    AdminLotUpdatePort, AdminMailPort, AdminProxyEventReader, AdminRenameReader, AdminRulesPort,
    AdminRuntimeConfigError, AdminRuntimeConfigPort, AdminRuntimeConfigService, AdminSessionConfig,
    AdminSessionPort, AdminSessionService, AdminSessionStore, AdminUserPort, AdminUserService,
    ApplicationError, AuctionLifecycleCommandPort, DeliveryAdminPort, OperationsAlertSourceReader,
    OperationsAlertStore, RealtimeAdminPort, RecoveryCommandPort, RecoveryCommandService,
    RecoveryReportReader, SettlementReportReader,
};
use auction_application::{AdminCredentialWorld, WorkerControlPort};
use auction_cache::CacheStore;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_config::TomlRuntimeFileStore;
use auction_config::{AppConfig, CacheBackend, Role};
use auction_http::{AdminApiState, AdminHttpSecurity};
use auction_integrations::ForumPreviewAdapter;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_integrations::{
    CredentialSealer, DataKey, EquipmentKeySealer, EquipmentSourceAdapter,
    MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader, build_equipment_http_client,
};
use auction_jobs::WorkerControlClient;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{HealthReader, HealthService, HeartbeatStore};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{HeartbeatPublisher, ProcessRole, ServiceName};
use auction_observability::{ProcessMetrics, TracingConfig, TracingRuntime};
use auction_process::{
    DEFAULT_DRAIN_TIMEOUT, DrainOutcome, TaskGuard, TerminationSignals, wait_for_drain,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_storage::{
    DatabaseAdminAuctionCommands, DatabaseAdminIntakeCommands, DatabaseAdminLotCommands,
    DatabaseAdminMailCommands, DatabaseAdminRulesCommands, DatabaseAdminRuntimeCommands,
    DatabaseAuctionLifecycleCommands, DatabaseBackend, DatabaseStore,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use chrono::Duration as ChronoDuration;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::oneshot;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use uuid::Uuid;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[derive(Clone, Copy)]
struct SystemClock;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl auction_application::Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub async fn run() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Admin).context("load admin-api configuration")?;
    let tracing_config = tracing_config(&config.tracing);
    let telemetry = auction_observability::init_with_config(
        "auction_admin_api",
        "admin",
        env!("CARGO_PKG_VERSION"),
        config.log_filter.as_deref(),
        Some(&tracing_config),
        config.tracing_headers(),
    )
    .context("initialize tracing")?;

    let result = run_service(&config, &telemetry).await;
    shutdown_tracing(telemetry).await;
    result
}

async fn run_service(config: &AppConfig, telemetry: &TracingRuntime) -> anyhow::Result<()> {
    let mut signals = TerminationSignals::subscribe().context("subscribe admin-api signals")?;
    let cache = configure_cache(&config).context("configure admin-api cache")?;
    let metrics = Arc::new(
        ProcessMetrics::new_with_tracing_health(
            config.metrics.enabled,
            "auction_admin_api",
            "admin",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("initialize Prometheus metrics")?,
    );
    let bound_listener = listener::bind(&config).await.context("bind admin-api listener")?;
    let (state, heartbeat) =
        configure_state(&config, telemetry.health_reader(), Arc::clone(&metrics))
            .await
            .context("configure admin-api state")?;
    let state = state.with_metrics(metrics);
    #[cfg(any(feature = "postgres", feature = "sqlite"))]
    let mut heartbeat_task =
        heartbeat.as_ref().map(Arc::clone).map(spawn_heartbeat).map(TaskGuard::new);
    #[cfg(not(any(feature = "postgres", feature = "sqlite")))]
    let mut heartbeat_task: Option<TaskGuard<()>> = None;
    // 传入本二进制的版本，健康检查才能报告运行进程的版本而非契约 crate 的版本。
    let app = auction_http::admin_router(env!("CARGO_PKG_VERSION")).with_state(state);

    tracing::info!(
        listener = %bound_listener.description(),
        cache_backend = cache.backend(),
        "admin api listening"
    );
    let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
    let mut server_task =
        TaskGuard::new(tokio::spawn(listener::serve(bound_listener, app, async move {
            let _ = shutdown_receiver.await;
        })));
    let mut immediate_server_result = None;
    let signal = tokio::select! {
        result = server_task.join() => {
            immediate_server_result = result;
            None
        }
        signal = signals.recv() => Some(signal.context("receive admin-api shutdown signal")?),
    };

    if let Some(signal) = signal {
        tracing::info!(signal = %signal, "admin api drain started");
        let _ = shutdown_sender.send(());
    } else {
        drop(shutdown_sender);
    }
    abort_heartbeat_task(&heartbeat_task);
    let drain = async {
        join_heartbeat_task(&mut heartbeat_task).await;
        if let Some(publisher) = heartbeat.as_ref()
            && let Err(error) = publisher.publish_stopping().await
        {
            tracing::warn!(error = %error, "admin heartbeat stopping publish failed");
        }
        match immediate_server_result.take() {
            Some(result) => Some(result),
            None => server_task.join().await,
        }
    };
    let result = match wait_for_drain(&mut signals, DEFAULT_DRAIN_TIMEOUT, drain).await {
        Ok(DrainOutcome::Completed(result)) => admin_server_result(result),
        Ok(DrainOutcome::TimedOut) => {
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(anyhow::anyhow!("admin-api drain exceeded 10 seconds"))
        }
        Ok(DrainOutcome::Interrupted(signal)) => {
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(anyhow::anyhow!("admin-api drain interrupted by {signal}"))
        }
        Err(error) => {
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(error).context("wait for admin-api drain")
        }
    };
    if result.is_ok() {
        tracing::info!("admin api stopped");
    }
    result
}

fn abort_heartbeat_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

async fn join_heartbeat_task(task: &mut Option<TaskGuard<()>>) {
    let Some(task) = task else {
        return;
    };
    if let Some(Err(error)) = task.join().await
        && !error.is_cancelled()
    {
        tracing::warn!(error = %error, "admin heartbeat task join failed");
    }
}

async fn force_admin_tasks(
    heartbeat_task: &mut Option<TaskGuard<()>>,
    server_task: &mut TaskGuard<std::io::Result<()>>,
) {
    abort_heartbeat_task(heartbeat_task);
    server_task.abort();
    join_heartbeat_task(heartbeat_task).await;
    let _ = server_task.join().await;
}

fn admin_server_result(
    result: Option<Result<std::io::Result<()>, tokio::task::JoinError>>,
) -> anyhow::Result<()> {
    match result {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(error))) => Err(error).context("serve admin-api HTTP"),
        Some(Err(error)) => Err(error).context("join admin-api HTTP server"),
        None => Err(anyhow::anyhow!("admin-api HTTP server result is missing")),
    }
}

async fn shutdown_tracing(telemetry: TracingRuntime) {
    match tokio::task::spawn_blocking(move || telemetry.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(error = %error, "admin tracing shutdown failed"),
        Err(error) => tracing::warn!(error = %error, "admin tracing shutdown task failed"),
    }
}

fn tracing_config(settings: &auction_config::TracingSettings) -> TracingConfig {
    TracingConfig {
        enabled: settings.enabled,
        endpoint: settings.endpoint.clone(),
        batch_timeout: Duration::from_secs(settings.batch_timeout_seconds),
        export_timeout: Duration::from_secs(settings.export_timeout_seconds),
        queue_capacity: settings.queue_capacity as usize,
        max_export_batch_size: settings.max_export_batch_size as usize,
        sample_ratio: f64::from(settings.sample_ratio_ppm) / 1_000_000.0,
        shutdown_timeout: Duration::from_secs(settings.shutdown_timeout_seconds),
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn configure_state(
    config: &AppConfig,
    tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
) -> anyhow::Result<(AdminApiState, Option<Arc<HeartbeatPublisher>>)> {
    let security = configure_security(config)?;
    let worker_control = configure_worker_control(config);
    let Some(database_url) = config.database_url() else {
        let forum_preview: Arc<dyn AdminForumPreviewPort> = Arc::new(ForumPreviewAdapter::new());
        return Ok((
            AdminApiState::new(
                None,
                None,
                None,
                None,
                None,
                None,
                Some(forum_preview),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                worker_control,
                security,
            )
            .with_dashboard_worlds(configured_dashboard_worlds(config))
            .with_dashboard_mailbox_keys(configured_dashboard_mailbox_keys(config)),
            None,
        ));
    };
    let store =
        Arc::new(DatabaseStore::connect(database_url).await.context("connect admin database")?);
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("register admin database pool metrics")?;
    let actor =
        config.admin_actor().context("admin_actor is missing from sensitive configuration")?;
    let secret =
        config.admin_secret().context("admin_secret is missing from sensitive configuration")?;
    let lifecycle_store = Arc::clone(&store);
    let auction_create_store = Arc::clone(&store);
    let admin_rules_store = Arc::clone(&store);
    let admin_runtime: Option<Arc<dyn AdminRuntimeConfigPort>> = if store.backend()
        == DatabaseBackend::Postgres
    {
        if let Some(path) = config.runtime_config_path.clone() {
            let files = Arc::new(
                TomlRuntimeFileStore::new(path)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            );
            let commands = Arc::new(DatabaseAdminRuntimeCommands::new(Arc::clone(&store)));
            let service =
                Arc::new(AdminRuntimeConfigService::new(commands, files, Arc::new(SystemClock)));
            if let Err(error) = service.recover().await {
                match &error {
                    ApplicationError::AdminRuntime(
                        AdminRuntimeConfigError::OutcomeUnknown
                        | AdminRuntimeConfigError::ReconciliationRequired,
                    ) => {
                        tracing::error!(
                            error = %error,
                            "admin runtime 配置恢复结果需要人工核对，继续启动"
                        );
                    }
                    _ => return Err(anyhow::Error::msg(error.to_string())),
                }
            }
            Some(service)
        } else {
            None
        }
    } else {
        None
    };
    let admin_intake = if store.backend() == DatabaseBackend::Postgres {
        match config.equipment_data_key_id().zip(config.equipment_data_key()) {
            Some((key_id, key_material)) => {
                let data_key = DataKey::from_base64(key_id, key_material)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                let source = EquipmentSourceAdapter::production(build_equipment_http_client()?, 0)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                let sealer = EquipmentKeySealer::new(data_key);
                let intake: Arc<dyn AdminIntakePort> = Arc::new(DatabaseAdminIntakeCommands::new(
                    Arc::clone(&store),
                    Arc::new(source),
                    Arc::new(sealer),
                    Arc::new(SystemClock),
                ));
                Some(intake)
            }
            None => None,
        }
    } else {
        None
    };
    let forum_preview: Arc<dyn AdminForumPreviewPort> = Arc::new(ForumPreviewAdapter::new());
    let admin_audit: Arc<dyn AdminAuditPort> = store.clone();
    let admin_renames: Arc<dyn AdminRenameReader> = store.clone();
    let admin_users: Option<Arc<dyn AdminUserPort>> =
        match config.user_login_data_key_id().zip(config.user_login_data_key()) {
            Some((key_id, key_material)) => {
                let data_key = Arc::new(
                    DataKey::from_base64(key_id, key_material)
                        .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                );
                let issuer = Arc::new(MoogleMailLoginKeyIssuer::new(Arc::clone(&data_key)));
                let opener = Arc::new(MoogleMailLoginKeyReader::new(Arc::clone(&store), data_key));
                Some(Arc::new(AdminUserService::new(Arc::clone(&store), issuer, opener)))
            }
            None => None,
        };
    let admin_credentials: Option<Arc<dyn AdminCredentialPort>> =
        if store.backend() == DatabaseBackend::Postgres {
            match config.credential_data_key_id().zip(config.credential_data_key()) {
                Some((key_id, key_material)) => {
                    let data_key = Arc::new(
                        DataKey::from_base64(key_id, key_material)
                            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                    );
                    let sealer = Arc::new(CredentialSealer::new(data_key));
                    Some(Arc::new(AdminCredentialService::new(Arc::clone(&store), sealer)))
                }
                None => None,
            }
        } else {
            None
        };
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let health: Arc<dyn HealthReader> = Arc::new(HealthService::new(heartbeat_store));
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let heartbeat = Arc::new(
        HeartbeatPublisher::new(
            heartbeat_store,
            ServiceName::Admin,
            ProcessRole::Admin,
            Uuid::new_v4().to_string(),
            env!("CARGO_PKG_VERSION").to_owned(),
            Utc::now(),
            ChronoDuration::seconds(30),
        )
        .with_tracing_health(tracing_health),
    );
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "admin heartbeat starting publish failed");
    }
    let session_store: Arc<dyn AdminSessionStore> = store.clone();
    let settlement_report: Arc<dyn SettlementReportReader> = store.clone();
    let recovery_report: Arc<dyn RecoveryReportReader> = store.clone();
    let admin_auctions: Arc<dyn AdminAuctionReader> = store.clone();
    let admin_lots: Arc<dyn AdminLotReader> = store.clone();
    let admin_lot_update: Arc<dyn AdminLotUpdatePort> =
        Arc::new(DatabaseAdminLotCommands::new(Arc::clone(&store), Arc::new(SystemClock)));
    let admin_mail: Arc<dyn AdminMailPort> =
        Arc::new(DatabaseAdminMailCommands::new(Arc::clone(&store), Arc::new(SystemClock)));
    let admin_proxy_events: Arc<dyn AdminProxyEventReader> = store.clone();
    let delivery_admin: Arc<dyn DeliveryAdminPort> = store.clone();
    let operations_alert: Arc<dyn OperationsAlertStore> = store.clone();
    let operations_alert_source: Arc<dyn OperationsAlertSourceReader> = store.clone();
    let credential_health: Arc<dyn AdminCredentialHealthReader> = store.clone();
    let realtime_admin: Arc<dyn RealtimeAdminPort> = store.clone();
    let recovery_command_store = Arc::clone(&store);
    let session_config = AdminSessionConfig {
        idle_ttl: ChronoDuration::seconds(config.session.idle_ttl_seconds as i64),
        absolute_ttl: ChronoDuration::seconds(config.session.absolute_ttl_seconds as i64),
    };
    let session: Arc<dyn AdminSessionPort> = Arc::new(AdminSessionService::new(
        session_store,
        actor.to_owned(),
        secret.as_bytes().to_vec(),
        session_config,
    )?);
    let lifecycle: Arc<dyn AuctionLifecycleCommandPort> =
        Arc::new(DatabaseAuctionLifecycleCommands::new(lifecycle_store, Arc::new(SystemClock)));
    let auction_create: Arc<dyn AdminAuctionCreatePort> =
        Arc::new(DatabaseAdminAuctionCommands::new(
            auction_create_store,
            Arc::new(SystemClock),
            config.auction.policy(),
        ));
    let admin_rules: Arc<dyn AdminRulesPort> =
        Arc::new(DatabaseAdminRulesCommands::new(admin_rules_store, Arc::new(SystemClock)));
    let recovery_commands: Arc<dyn RecoveryCommandPort> =
        Arc::new(RecoveryCommandService::new(recovery_command_store));
    let state = AdminApiState::new(
        Some(session),
        Some(lifecycle),
        Some(auction_create),
        Some(admin_audit),
        Some(health),
        admin_intake,
        Some(forum_preview),
        Some(admin_mail),
        Some(admin_rules),
        admin_runtime,
        admin_credentials,
        Some(admin_renames),
        admin_users,
        Some(recovery_commands),
        Some(settlement_report),
        Some(recovery_report),
        Some(admin_auctions),
        Some(admin_lots),
        Some(admin_lot_update),
        Some(admin_proxy_events),
        Some(delivery_admin),
        Some(operations_alert),
        Some(operations_alert_source),
        Some(credential_health),
        Some(realtime_admin),
        worker_control,
        security,
    )
    .with_dashboard_worlds(configured_dashboard_worlds(config))
    .with_dashboard_mailbox_keys(configured_dashboard_mailbox_keys(config))
    .with_readiness(store);
    if let Err(error) = heartbeat.publish_ready().await {
        tracing::warn!(error = %error, "admin heartbeat ready publish failed");
    }
    Ok((state, Some(heartbeat)))
}

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
async fn configure_state(
    config: &AppConfig,
    _tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    _metrics: Arc<ProcessMetrics>,
) -> anyhow::Result<(AdminApiState, Option<Arc<auction_observability::HeartbeatPublisher>>)> {
    let security = configure_security(config)?;
    let worker_control = configure_worker_control(config);
    if config.database_url().is_some() {
        anyhow::bail!("admin-api 配置了数据库，但当前制品未编译 postgres 或 sqlite feature");
    }
    Ok((
        AdminApiState::new(
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(ForumPreviewAdapter::new())),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            worker_control,
            security,
        )
        .with_dashboard_worlds(configured_dashboard_worlds(config))
        .with_dashboard_mailbox_keys(configured_dashboard_mailbox_keys(config)),
        None,
    ))
}

fn configure_worker_control(config: &AppConfig) -> Option<Arc<dyn WorkerControlPort>> {
    match WorkerControlClient::new(config.worker_control_socket_path(), Duration::from_secs(5)) {
        Ok(client) => Some(Arc::new(client)),
        Err(error) => {
            tracing::warn!(error = %error, "Worker control client configuration is unavailable");
            None
        }
    }
}

/// 将 Admin runtime 声明的 Worker world 配置投影到 dashboard；未声明时不强制假设 world。
fn configured_dashboard_worlds(config: &AppConfig) -> Vec<AdminCredentialWorld> {
    configured_dashboard_mailboxes(config)
        .into_iter()
        .filter_map(|(world_id, _)| AdminCredentialWorld::parse(&world_id))
        .collect()
}

/// 将 Admin runtime 声明的邮箱标识投影到 Admin dashboard，保留自定义 mailbox_key 语义。
fn configured_dashboard_mailbox_keys(config: &AppConfig) -> Vec<String> {
    configured_dashboard_mailboxes(config).into_iter().map(|(_, mailbox_key)| mailbox_key).collect()
}

/// 返回 Admin runtime 明确声明的 dashboard 邮箱范围；兼容程序化装配时的 Worker 配置投影。
fn configured_dashboard_mailboxes(config: &AppConfig) -> Vec<(String, String)> {
    if let Some(runtime) = config.admin_runtime.as_ref()
        && !runtime.dashboard_mailboxes.is_empty()
    {
        return runtime
            .dashboard_mailboxes
            .iter()
            .map(|mailbox| (mailbox.world_id.clone(), mailbox.mailbox_key.clone()))
            .collect();
    }
    if !config.worker.mooglemail.enabled {
        return Vec::new();
    }

    config
        .worker
        .mooglemail
        .configured_worlds()
        .into_iter()
        .map(|world| (world.world_id, world.mailbox_key))
        .collect()
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = publisher.publish_ready().await {
                tracing::warn!(error = %error, "admin heartbeat publish failed");
            }
        }
    })
}

fn configure_security(config: &AppConfig) -> anyhow::Result<AdminHttpSecurity> {
    AdminHttpSecurity::new(
        config.http_security.cookie_secure,
        config.http_security.allowed_origins_enabled,
        &config.http_security.allowed_origins,
        config.http_security.login_rate_limit,
        config.http_security.mutation_rate_limit,
        Duration::from_secs(config.http_security.rate_limit_window_seconds),
    )
    .map_err(anyhow::Error::msg)
}

fn configure_cache(config: &AppConfig) -> anyhow::Result<CacheStore> {
    match config.cache.backend {
        CacheBackend::Disabled => Ok(CacheStore::disabled()),
        CacheBackend::Redis => {
            let redis_url = config.redis_url().context("redis cache URL is missing")?;
            CacheStore::redis(redis_url).map_err(Into::into)
        }
    }
}
