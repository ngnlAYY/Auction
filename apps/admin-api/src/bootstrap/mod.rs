use std::time::Duration;

use anyhow::Context;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::AdminForumPreviewPort;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::{
    AdminAuctionCreatePort, AdminAuctionReader, AdminAuditPort, AdminIntakePort, AdminLotReader,
    AdminLotUpdatePort, AdminMailPort, AdminProxyEventReader, AdminRenameReader, AdminRulesPort,
    AdminRuntimeConfigPort, AdminRuntimeConfigService, AdminSessionConfig, AdminSessionPort,
    AdminSessionService, AdminSessionStore, AuctionLifecycleCommandPort, DeliveryAdminPort,
    RealtimeAdminPort, RecoveryCommandPort, RecoveryCommandService, RecoveryReportReader,
    SettlementReportReader,
};
use auction_cache::CacheStore;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_config::TomlRuntimeFileStore;
use auction_config::{AppConfig, CacheBackend, Role};
use auction_http::{AdminApiState, AdminHttpSecurity};
use auction_integrations::ForumPreviewAdapter;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_integrations::{
    DataKey, EquipmentKeySealer, EquipmentSourceAdapter, build_equipment_http_client,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{HealthReader, HealthService, HeartbeatStore};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{HeartbeatPublisher, ProcessRole, ServiceName};
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
use tokio::net::TcpListener;
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
    auction_observability::init("auction_admin_api", config.log_filter.as_deref())
        .context("initialize tracing")?;
    let cache = configure_cache(&config).context("configure admin-api cache")?;
    let address = config.listen_addr.context("admin-api must have a listen address")?;
    let listener = TcpListener::bind(address).await?;
    let (state, heartbeat) = configure_state(&config).await.context("configure admin-api state")?;
    #[cfg(any(feature = "postgres", feature = "sqlite"))]
    let heartbeat_task = heartbeat.as_ref().map(Arc::clone).map(spawn_heartbeat);
    #[cfg(not(any(feature = "postgres", feature = "sqlite")))]
    let heartbeat_task: Option<tokio::task::JoinHandle<()>> = None;
    // 传入本二进制的版本，健康检查才能报告运行进程的版本而非契约 crate 的版本。
    let app = auction_http::admin_router(env!("CARGO_PKG_VERSION")).with_state(state);

    tracing::info!(%address, cache_backend = cache.backend(), "admin api listening");
    let result =
        axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .await;
    if let Some(publisher) = heartbeat.as_ref()
        && let Err(error) = publisher.publish_stopping().await
    {
        tracing::warn!(error = %error, "admin heartbeat stopping publish failed");
    }
    if let Some(task) = heartbeat_task {
        task.abort();
    }
    result?;
    Ok(())
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn configure_state(
    config: &AppConfig,
) -> anyhow::Result<(AdminApiState, Option<Arc<HeartbeatPublisher>>)> {
    let security = configure_security(config)?;
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
                security,
            ),
            None,
        ));
    };
    let store =
        Arc::new(DatabaseStore::connect(database_url).await.context("connect admin database")?);
    let actor =
        config.admin_actor().context("admin_actor is missing from sensitive configuration")?;
    let secret =
        config.admin_secret().context("admin_secret is missing from sensitive configuration")?;
    let lifecycle_store = Arc::clone(&store);
    let auction_create_store = Arc::clone(&store);
    let admin_rules_store = Arc::clone(&store);
    let admin_runtime: Option<Arc<dyn AdminRuntimeConfigPort>> =
        if store.backend() == DatabaseBackend::Postgres {
            config
                .runtime_config_path
                .clone()
                .map(|path| {
                    let files = Arc::new(
                        TomlRuntimeFileStore::new(path)
                            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                    );
                    let commands = Arc::new(DatabaseAdminRuntimeCommands::new(Arc::clone(&store)));
                    Ok::<Arc<dyn AdminRuntimeConfigPort>, anyhow::Error>(Arc::new(
                        AdminRuntimeConfigService::new(commands, files, Arc::new(SystemClock)),
                    ))
                })
                .transpose()?
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
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let health: Arc<dyn HealthReader> = Arc::new(HealthService::new(heartbeat_store));
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let heartbeat = Arc::new(HeartbeatPublisher::new(
        heartbeat_store,
        ServiceName::Admin,
        ProcessRole::Admin,
        Uuid::new_v4().to_string(),
        env!("CARGO_PKG_VERSION").to_owned(),
        Utc::now(),
        ChronoDuration::seconds(30),
    ));
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
        Arc::new(DatabaseAdminAuctionCommands::new(auction_create_store, Arc::new(SystemClock)));
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
        Some(admin_renames),
        Some(recovery_commands),
        Some(settlement_report),
        Some(recovery_report),
        Some(admin_auctions),
        Some(admin_lots),
        Some(admin_lot_update),
        Some(admin_proxy_events),
        Some(delivery_admin),
        Some(realtime_admin),
        security,
    );
    if let Err(error) = heartbeat.publish_ready().await {
        tracing::warn!(error = %error, "admin heartbeat ready publish failed");
    }
    Ok((state, Some(heartbeat)))
}

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
async fn configure_state(
    config: &AppConfig,
) -> anyhow::Result<(AdminApiState, Option<Arc<auction_observability::HeartbeatPublisher>>)> {
    let security = configure_security(config)?;
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
            security,
        ),
        None,
    ))
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
