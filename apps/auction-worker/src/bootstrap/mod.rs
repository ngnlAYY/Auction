use std::{pin::Pin, sync::Arc};

#[cfg(feature = "postgres")]
use std::collections::{HashMap, HashSet};

use anyhow::Context;
#[cfg(feature = "postgres")]
use auction_application::{
    AdminCredentialKind, AdminCredentialOpener, AdminCredentialRuntimeOwnership,
    AdminCredentialRuntimeReader, AdminCredentialRuntimeWriter, AdminCredentialWorld,
    AdminMailPort, ApplicationError, ApplicationFuture, AuctionLifecycleCommandPort,
    AuctionLifecyclePolicy, Clock, CompetitionCommandPort, DeliveryTransport, DeliveryWorkStore,
    ForumCredentialAvailability, ForumPostWriter, ForumPublisher, ForumRenderer, ForumReplyStore,
    ForumSyncStore, ForumTopicReader, MailDeliveryEnqueuer, OperationsAlertRuntimeStatus,
    OperationsAlertSourceReader, OperationsAlertStore, PublicEquipmentLinkResolver,
    PublicEquipmentLinkSecretReader, RecoveryBuyerBlacklister, RecoveryHealthConfig,
    RecoveryHealthService, RecoveryResolutionStore, RecoveryWorkStore, ScheduledRoundStore,
    SettlementCommandPort, SettlementFulfillmentBalanceEvidence, SettlementFulfillmentCommandPort,
    SettlementFulfillmentCustodyReader, SettlementFulfillmentEvidenceReader,
    SettlementFulfillmentRecipientReader, SettlementFulfillmentSentEvidence,
    SettlementFulfillmentService, SettlementFulfillmentStore, SettlementWorkStore,
    SlowProxyCommandPort, SlowProxyDueReader,
};
#[cfg(feature = "postgres")]
use auction_application::{
    MailAuctionIntakeService, MailEquipmentFilter, MailLoginKeyReader, MailPollStore,
    MailProcessor, MailProcessorConfig, MailUserService, RecurringSchedule,
    SettlementFulfillmentEquipmentSecretReader,
};
use auction_cache::CacheStore;
use auction_config::{AppConfig, CacheBackend, Role};
#[cfg(feature = "postgres")]
use auction_config::{StandardCronSchedule, load_mooglemail_keyenv};
#[cfg(feature = "postgres")]
use auction_domain::SettlementWorld;
#[cfg(feature = "postgres")]
use auction_integrations::ehentai::{
    EhentaiClient, EhentaiClientConfig, EhentaiCookie, EhentaiCredentialSet, EhentaiGateway,
};
#[cfg(feature = "postgres")]
use auction_integrations::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
    MoogleMailDeliveryTransport, MoogleMailError, MoogleMailErrorCategory, MoogleMailGateway,
    MoogleMailGatewayConfig, MoogleMailMessage, MoogleMailNotificationTemplates, MoogleMailWorld,
    into_application_message,
};
#[cfg(feature = "postgres")]
use auction_integrations::{
    CredentialSealer, DataKey, EquipmentGameLinkResolver, EquipmentKeyOpener, ForumThreadRenderer,
};
#[cfg(feature = "postgres")]
use auction_integrations::{
    EquipmentKeySealer, EquipmentSourceAdapter, EquipmentSourcePolicy, MoogleMailLoginKeyIssuer,
    MoogleMailLoginKeyReader, build_equipment_http_client,
};
#[cfg(feature = "postgres")]
use auction_jobs::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntimeConfig,
};
#[cfg(feature = "postgres")]
use auction_jobs::{MailPollRuntime, RecoveryHealthRuntime};
#[cfg(any(feature = "postgres", unix))]
use auction_jobs::{WorkerControlHandle, WorkerControlServer, WorkerControlServerConfig};
#[cfg(feature = "postgres")]
use auction_observability::{
    HeartbeatHealthReader, HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatStore,
    ProcessRole, ServiceName,
};
use auction_observability::{ProcessMetrics, TracingConfig, TracingHealthReader, TracingRuntime};
use auction_process::{
    DEFAULT_DRAIN_TIMEOUT, DrainOutcome, TaskGuard, TerminationSignals, wait_for_drain,
};
#[cfg(feature = "postgres")]
use auction_storage::{
    DatabaseAdminMailCommands, DatabaseAuctionLifecycleCommands, DatabaseBackend,
    DatabaseCompetitionCommands, DatabaseSettlementCommands, DatabaseStore,
};
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use url::Url;
#[cfg(feature = "postgres")]
use uuid::Uuid;

type ShutdownFuture = Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

#[cfg(feature = "postgres")]
#[derive(Clone, Copy)]
struct SystemClock;

#[cfg(feature = "postgres")]
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(feature = "postgres")]
type SettlementPorts = (
    Option<Arc<dyn SettlementWorkStore>>,
    Option<Arc<dyn SettlementCommandPort>>,
    Option<Arc<dyn SettlementFulfillmentStore>>,
    Option<Arc<dyn SettlementFulfillmentCommandPort>>,
    Option<Arc<dyn DeliveryWorkStore>>,
    Option<Arc<dyn DeliveryTransport>>,
    Option<Arc<dyn SettlementFulfillmentEvidenceReader>>,
);

/// 启动独立 Worker，沿用角色配置选择器。
pub async fn run() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    run_config(config).await
}

/// 从显式角色路径启动，供单文件 supervisor 派生的独立进程使用。
pub async fn run_from_path(path: std::path::PathBuf) -> anyhow::Result<()> {
    let config = AppConfig::load_from_path(Role::Worker, path)
        .context("load explicit worker configuration")?;
    run_config(config).await
}

async fn run_config(config: AppConfig) -> anyhow::Result<()> {
    let tracing_config = tracing_config(&config.tracing);
    let telemetry = auction_observability::init_with_config(
        "auction_worker",
        "worker",
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
    let mut signals = TerminationSignals::subscribe().context("subscribe worker signals")?;
    let metrics = Arc::new(
        ProcessMetrics::new_with_tracing_health(
            config.metrics.enabled,
            "auction_worker",
            "worker",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("initialize Prometheus metrics")?,
    );
    let cache = configure_cache(config).context("configure worker cache")?;
    tracing::info!(
        database_configured = config.database_url().is_some(),
        cache_backend = cache.backend(),
        worker_concurrency = config.worker_concurrency(),
        "worker configured"
    );

    let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
    // 首个运行时故障即可触发收口；固定容量避免并发故障排队或阻塞 Drop 路径。
    let (runtime_failure_sender, mut runtime_failure_receiver) =
        tokio::sync::mpsc::channel::<&'static str>(1);
    let worker = run_worker(
        config,
        telemetry.health_reader(),
        Arc::clone(&metrics),
        runtime_failure_sender,
        Box::pin(async move {
            let _ = shutdown_receiver.await;
        }),
    );
    tokio::pin!(worker);
    let runtime_failure = async {
        match runtime_failure_receiver.recv().await {
            Some(reason) => reason,
            // 未启用 control/凭据或启动提前失败时，sender 关闭本身不是运行时故障。
            None => std::future::pending::<&'static str>().await,
        }
    };
    tokio::pin!(runtime_failure);
    let result = tokio::select! {
        result = &mut worker => {
            drop(shutdown_sender);
            result
        }
        reason = &mut runtime_failure => {
            tracing::error!(reason, "worker drain started");
            let _ = shutdown_sender.send(());
            drain_worker(&mut signals, &mut worker).await.context("drain worker after runtime failure")?;
            if reason == "control_server_stopped" {
                return Err(anyhow::anyhow!("worker control server stopped; worker restart required"));
            }
            if reason == "lot_watch_pm_stopped" {
                return Err(anyhow::anyhow!("lot watch PM worker stopped; worker restart required"));
            }
            if reason == "persistent_worker_stopped" {
                return Err(anyhow::anyhow!("persistent worker stopped; worker restart required"));
            }
            // 即使所有在途任务正常结束也须非零退出，由进程管理器获取新 owner/generation。
            Err(anyhow::anyhow!("credential runtime ownership revoked; worker restart required"))
        }
        signal = signals.recv() => {
            let signal = signal.context("receive worker shutdown signal")?;
            tracing::info!(signal = %signal, "worker drain started");
            let _ = shutdown_sender.send(());
            drain_worker(&mut signals, &mut worker).await
        }
    };
    if result.is_ok() {
        tracing::info!("worker stopped");
    }
    result
}

/// 信号与运行时故障共用排空预算及错误映射；具体任务仍由原有 future 和守卫拥有。
async fn drain_worker(
    signals: &mut TerminationSignals,
    worker: impl std::future::Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()> {
    match wait_for_drain(signals, DEFAULT_DRAIN_TIMEOUT, worker).await {
        Ok(DrainOutcome::Completed(result)) => result,
        Ok(DrainOutcome::TimedOut) => Err(anyhow::anyhow!("worker drain exceeded 10 seconds")),
        Ok(DrainOutcome::Interrupted(signal)) => {
            Err(anyhow::anyhow!("worker drain interrupted by {signal}"))
        }
        Err(error) => Err(error).context("wait for worker drain"),
    }
}

async fn shutdown_tracing(telemetry: TracingRuntime) {
    match tokio::task::spawn_blocking(move || telemetry.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(error = %error, "worker tracing shutdown failed"),
        Err(error) => tracing::warn!(error = %error, "worker tracing shutdown task failed"),
    }
}

fn tracing_config(settings: &auction_config::TracingSettings) -> TracingConfig {
    TracingConfig {
        enabled: settings.enabled,
        endpoint: settings.endpoint.clone(),
        batch_timeout: std::time::Duration::from_secs(settings.batch_timeout_seconds),
        export_timeout: std::time::Duration::from_secs(settings.export_timeout_seconds),
        queue_capacity: settings.queue_capacity as usize,
        max_export_batch_size: settings.max_export_batch_size as usize,
        sample_ratio: f64::from(settings.sample_ratio_ppm) / 1_000_000.0,
        shutdown_timeout: std::time::Duration::from_secs(settings.shutdown_timeout_seconds),
    }
}

mod control;
mod forum;
mod fulfillment;
mod mail;
mod operations_alert;
mod persistent;
mod skeleton;

#[cfg(feature = "postgres")]
async fn run_worker(
    config: &AppConfig,
    tracing_health: Arc<dyn TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    persistent::run_worker(config, tracing_health, metrics, runtime_failure, shutdown).await
}

#[cfg(not(feature = "postgres"))]
async fn run_worker(
    config: &AppConfig,
    tracing_health: Arc<dyn TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    skeleton::run_worker(config, tracing_health, metrics, runtime_failure, shutdown).await
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
