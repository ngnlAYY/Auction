//! MoogleMail Worker 的配置、业务装配和轮询运行时入口。
//!
//! 凭据恢复、业务 processor/delivery、poll runtime/gateway 和消息 envelope 映射由子模块
//! 分别拥有；本模块只负责把这些 owner 组合成 Worker 所需的端口集合。

#[cfg(feature = "postgres")]
use super::fulfillment::MoogleMailFulfillmentEvidenceReader;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::*;

#[cfg(feature = "postgres")]
mod credentials;
#[cfg(feature = "postgres")]
mod mapping;
#[cfg(feature = "postgres")]
mod processor;
#[cfg(feature = "postgres")]
mod runtime;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
/// 根据 Worker 配置装配 MoogleMail 轮询、业务处理、delivery 和履约证据读取端口。
pub(super) async fn configure_mail_poll(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
) -> anyhow::Result<(
    Vec<MailPollRuntime>,
    Option<Arc<dyn DeliveryTransport>>,
    Option<Arc<dyn SettlementFulfillmentEvidenceReader>>,
)> {
    let mail_config = &config.worker.mooglemail;
    if !mail_config.enabled {
        return Ok((Vec::new(), None, None));
    }

    #[cfg(not(feature = "postgres"))]
    {
        let _ = store;
        anyhow::bail!("MoogleMail Worker 轮询仅支持 PostgreSQL 完整档");
    }

    #[cfg(feature = "postgres")]
    {
        if store.backend() != DatabaseBackend::Postgres {
            anyhow::bail!("MoogleMail Worker 轮询仅支持 PostgreSQL 完整档");
        }
        let world_configs = mail_config.configured_worlds();
        let schedule: Arc<dyn RecurringSchedule> = Arc::new(
            StandardCronSchedule::parse(&mail_config.schedule)
                .context("parse MoogleMail standard cron schedule")?,
        );
        let credentials =
            credentials::load_credentials(config, mail_config, &world_configs, Arc::clone(&store))
                .await?;
        let endpoint = Url::parse(&mail_config.endpoint).context("parse MoogleMail endpoint")?;
        let client = Arc::new(
            MoogleMailClient::with_config(MoogleMailClientConfig {
                endpoint,
                maximum_response_bytes: mail_config.maximum_response_bytes,
                minimum_request_interval: std::time::Duration::from_millis(
                    mail_config.minimum_request_interval_millis,
                ),
                request_timeout: std::time::Duration::from_secs(
                    mail_config.request_timeout_seconds,
                ),
            })
            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        );
        let (processor, delivery_transport) = processor::build_processor_and_delivery(
            config,
            mail_config,
            &world_configs,
            &store,
            client.as_ref(),
            &credentials,
        )?;
        let (runtimes, gateways) = runtime::build_poll_runtimes(
            mail_config,
            world_configs,
            Arc::clone(&client),
            credentials,
            processor,
            Arc::clone(&store),
            schedule,
        )
        .await?;
        let evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader> =
            Arc::new(MoogleMailFulfillmentEvidenceReader { gateways });
        Ok((runtimes, Some(delivery_transport), Some(evidence_reader)))
    }
}
