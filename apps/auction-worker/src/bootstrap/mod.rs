use std::{pin::Pin, sync::Arc};

#[cfg(feature = "postgres")]
use std::collections::{HashMap, HashSet};

use anyhow::Context;
#[cfg(feature = "postgres")]
use auction_application::admin::{
    AdminCredentialKind, AdminCredentialOpener, AdminCredentialRuntimeOwnership,
    AdminCredentialRuntimeReader, AdminCredentialRuntimeWriter, AdminCredentialWorld,
    AdminMailPort, RecoveryBuyerBlacklister,
};
#[cfg(feature = "postgres")]
use auction_application::auction::{
    ApplicationFuture, AuctionLifecycleCommandPort, AuctionLifecyclePolicy, CompetitionCommandPort,
    SlowProxyCommandPort, SlowProxyDueReader,
};
#[cfg(feature = "postgres")]
use auction_application::core::{ApplicationError, Clock};
#[cfg(feature = "postgres")]
use auction_application::delivery::{DeliveryTransport, DeliveryWorkStore};
#[cfg(feature = "postgres")]
use auction_application::forum::{
    ForumCredentialAvailability, ForumPostWriter, ForumPublisher, ForumRenderer, ForumReplyStore,
    ForumSyncStore, ForumTopicReader,
};
#[cfg(feature = "postgres")]
use auction_application::mail::MailDeliveryEnqueuer;
#[cfg(feature = "postgres")]
use auction_application::mail::{
    MailAuctionIntakeService, MailEquipmentFilter, MailLoginKeyReader, MailPollStore,
    MailProcessor, MailProcessorConfig, MailUserService,
};
#[cfg(feature = "postgres")]
use auction_application::operations::{
    OperationsAlertRuntimeStatus, OperationsAlertSourceReader, OperationsAlertStore,
};
#[cfg(feature = "postgres")]
use auction_application::public::{PublicEquipmentLinkResolver, PublicEquipmentLinkSecretReader};
#[cfg(feature = "postgres")]
use auction_application::reconciliation::AssetReconciliationService;
#[cfg(feature = "postgres")]
use auction_application::recovery::{
    RecoveryHealthConfig, RecoveryHealthService, RecoveryResolutionStore, RecoveryWorkStore,
};
#[cfg(feature = "postgres")]
use auction_application::scheduling::RecurringSchedule;
#[cfg(feature = "postgres")]
use auction_application::scheduling::ScheduledRoundStore;
#[cfg(feature = "postgres")]
use auction_application::settlement::SettlementFulfillmentEquipmentSecretReader;
#[cfg(feature = "postgres")]
use auction_application::settlement::{
    SettlementCommandPort, SettlementFulfillmentBalanceEvidence, SettlementFulfillmentCommandPort,
    SettlementFulfillmentCustodyReader, SettlementFulfillmentEvidenceReader,
    SettlementFulfillmentRecipientReader, SettlementFulfillmentSentEvidence,
    SettlementFulfillmentService, SettlementFulfillmentStore, SettlementWorkStore,
};
use auction_config::{AppConfig, Role};
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
    MoogleMailGatewayConfig, MoogleMailInventoryCategory, MoogleMailMessage,
    MoogleMailNotificationTemplates, MoogleMailSentLookupRequest, MoogleMailWorld,
    into_application_message,
};
#[cfg(feature = "postgres")]
use auction_integrations::{
    CredentialSealer, EquipmentGameLinkResolver, EquipmentKeyOpener, ForumThreadRenderer,
    build_keyring,
};
#[cfg(feature = "postgres")]
use auction_integrations::{
    EquipmentKeySealer, EquipmentSourceAdapter, EquipmentSourcePolicy, MoogleMailLoginKeyIssuer,
    MoogleMailLoginKeyReader, build_equipment_http_client,
};
#[cfg(feature = "postgres")]
use auction_jobs::{AssetReconciliationRuntime, MailPollRuntime, RecoveryHealthRuntime};
#[cfg(feature = "postgres")]
use auction_jobs::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntimeConfig,
};
#[cfg(any(feature = "postgres", unix))]
use auction_jobs::{WorkerControlHandle, WorkerControlServer, WorkerControlServerConfig};
#[cfg(feature = "postgres")]
use auction_observability::{
    HeartbeatHealthReader, HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatStore,
    ProcessRole, ServiceName,
};
use auction_observability::{ProcessMetrics, TracingConfig, TracingHealthReader, TracingRuntime};
use auction_process::{TaskGuard, TerminationSignals};
#[cfg(feature = "postgres")]
use auction_storage::{
    DatabaseAdminMailCommands, DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands,
    DatabaseSettlementCommands, DatabaseStore,
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

mod shutdown;

async fn run_config(config: AppConfig) -> anyhow::Result<()> {
    let mut signals = TerminationSignals::subscribe().context("subscribe worker signals")?;
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
    auction_observability::install_safe_panic_hook();

    let mut forced = false;
    let result = run_service(&config, &telemetry, &mut signals, &mut forced).await;
    if !forced {
        auction_observability::shutdown_tracing("worker", telemetry, &mut signals).await;
    }
    result
}

async fn run_service(
    config: &AppConfig,
    telemetry: &TracingRuntime,
    signals: &mut TerminationSignals,
    forced: &mut bool,
) -> anyhow::Result<()> {
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
    // 告警读侧先做影子比对，旧口径继续持有裁决权；分类口径只在
    // 观察到差异为零并由操作者显式切换后生效。
    let switches = auction_application::runtime_switches::RuntimeSwitches::new(
        auction_application::runtime_switches::RuntimeSwitchRole::Worker,
        config.readiness.audit_failure_mode,
        config.readiness.cache_enabled,
    )
    .with_sli_snapshot_baseline(config.metrics.sli_snapshot_enabled)
    .with_mooglemail_poll_isolation_baseline(config.worker.mooglemail.poll_isolation_enabled)
    .with_metrics_reject_forwarded_requests_baseline(config.metrics.reject_forwarded_requests)
    .with_disposition_read_mode_baseline(
        auction_application::operations::DispositionReadMode::Shadow,
    );
    let _metrics_task = config.metrics.loopback_listen_address(config.role).map(|address| {
        TaskGuard::new(auction_http::spawn_metrics_listener(
            address,
            config.role.name(),
            Arc::clone(&metrics),
            switches.clone(),
        ))
    });
    tracing::info!(
        database_configured = config.database_url().is_some(),
        worker_concurrency = config.worker_concurrency(),
        "worker configured"
    );

    let shutdown_runtime = shutdown::WorkerShutdown::new(config.shutdown, metrics.shutdown());
    let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
    // 首个运行时故障即可触发收口；固定容量避免并发故障排队或阻塞 Drop 路径。
    let (runtime_failure_sender, mut runtime_failure_receiver) =
        tokio::sync::mpsc::channel::<&'static str>(1);
    let worker = run_worker(
        config,
        telemetry.health_reader(),
        Arc::clone(&metrics),
        switches,
        runtime_failure_sender,
        shutdown_runtime.lifecycle.clone(),
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
            shutdown_runtime.close();
            let _ = shutdown_sender.send(());
            shutdown_runtime.drain(signals, &mut worker, auction_observability::ShutdownReason::RuntimeFailure, forced).await.context("drain worker after runtime failure")?;
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
            shutdown_runtime.close();
            let _ = shutdown_sender.send(());
            shutdown_runtime.drain(signals, &mut worker, auction_observability::ShutdownReason::Signal, forced).await
        }
    };
    if result.is_ok() {
        tracing::info!("worker stopped");
    }
    result
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
#[cfg(feature = "postgres")]
mod hot;
mod mail;
mod operations_alert;
mod persistent;
mod skeleton;

#[cfg(feature = "postgres")]
use persistent::run_worker;
#[cfg(not(feature = "postgres"))]
use skeleton::run_worker;
