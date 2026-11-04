//! MoogleMail Worker 的配置、业务装配和轮询运行时入口；各 owner 由子模块持有，
//! 本模块只组合所需端口集合。

#[cfg(feature = "postgres")]
use super::fulfillment::MoogleMailFulfillmentEvidenceReader;
#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
mod credentials;
#[cfg(feature = "postgres")]
mod diagnostics;
#[cfg(feature = "postgres")]
mod mapping;
#[cfg(feature = "postgres")]
mod processor;
#[cfg(feature = "postgres")]
mod runtime;

#[cfg(feature = "postgres")]
/// 根据 Worker 配置装配 MoogleMail 轮询、业务处理、delivery 和履约证据读取端口。
pub(super) async fn configure_mail_poll(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    profile_reader: Option<Arc<dyn auction_application::ForumTargetProfileReader>>,
    progress_observer: Arc<dyn auction_application::MailPollProgressObserver>,
    sent_baseline_observer: Arc<dyn auction_application::SentBaselineObserver>,
    switches: auction_application::RuntimeSwitches,
) -> anyhow::Result<(
    Vec<MailPollRuntime>,
    Option<Arc<dyn DeliveryTransport>>,
    Option<Arc<dyn SettlementFulfillmentEvidenceReader>>,
    Option<Arc<dyn SettlementFulfillmentCustodyReader>>,
    Option<Arc<dyn auction_application::SettlementInternalEvidenceReader>>,
    Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
)> {
    let mail_config = &config.worker.mooglemail;
    if !mail_config.enabled {
        return Ok((Vec::new(), None, None, None, None, None));
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
        // AppConfig 已应用 Admin 托管覆盖；省略维护日程时跟随最终生效的收件日程。
        // 即使基线关闭也解析并校验显式维护日程，在线切回隔离时不能暴露启动时未发现的错误。
        let maintenance_schedule: Arc<dyn RecurringSchedule> =
            match &mail_config.maintenance_schedule {
                Some(expression) => Arc::new(
                    StandardCronSchedule::parse(expression).context("解析 MoogleMail 维护 cron")?,
                ),
                None => Arc::clone(&schedule),
            };
        let (credentials, ownership) =
            credentials::load_credentials(config, mail_config, &world_configs, Arc::clone(&store))
                .await?;
        let endpoint = Url::parse(&mail_config.endpoint).context("parse MoogleMail endpoint")?;
        let mut client = MoogleMailClient::with_config(MoogleMailClientConfig {
            endpoint,
            maximum_response_bytes: mail_config.maximum_response_bytes,
            minimum_request_interval: std::time::Duration::from_millis(
                mail_config.minimum_request_interval_millis,
            ),
            request_timeout: std::time::Duration::from_secs(mail_config.request_timeout_seconds),
        })
        .map_err(|error| anyhow::Error::msg(error.to_string()))?;
        if let Some(recorder) = diagnostics::configure(config)? {
            client = client.with_diagnostics(recorder);
        }
        let client = Arc::new(client);
        let (processor, delivery_transport) = processor::build_processor_and_delivery(
            config,
            mail_config,
            &world_configs,
            &store,
            client.as_ref(),
            &credentials,
            sent_baseline_observer,
        )?;
        let maintenance = Arc::new(auction_application::MailboxMaintenanceService::new(
            Arc::new(delivery_transport.maintenance_adapter())
                as Arc<dyn auction_application::MoogleMailboxMaintenance>,
            Arc::clone(&store) as Arc<dyn auction_application::MoogleMailboxEvidenceStore>,
            Arc::new(SystemClock),
        ));
        let (runtimes, gateways) = runtime::build_poll_runtimes(
            mail_config,
            Arc::clone(&client),
            credentials,
            profile_reader,
            processor,
            runtime::MailPollRuntimePorts {
                store: Arc::clone(&store),
                schedule,
                maintenance_schedule,
                progress_observer,
                switches,
            },
        )
        .await?;
        // 各世界独立维护；共享同一读取协调器，不让主世界维护覆盖异世界邮件。
        let runtimes = runtimes
            .into_iter()
            .map(|runtime| runtime.with_maintenance(Arc::clone(&maintenance)))
            .collect();
        // gateway 本身接受显式 world；C/D 复用同一个实例、凭据集合与 owner。
        // 论坛关闭时不注入 reader，联合证据明确不可用，不隐式开启论坛能力。
        let gateway = gateways
            .get(&MoogleMailWorld::Main)
            .or_else(|| gateways.get(&MoogleMailWorld::Isekai))
            .cloned()
            .context("MoogleMail enabled but no world gateway is configured")?;
        let custody_reader: Arc<dyn SettlementFulfillmentCustodyReader> =
            Arc::new(super::fulfillment::MoogleMailFulfillmentCustodyReader {
                gateway: Arc::clone(&gateway),
            });
        let delivery_transport: Arc<dyn DeliveryTransport> = Arc::new(
            delivery_transport
                .with_cash_evidence_gateway(gateway.clone())
                .with_funding_policy_reader(store.clone()),
        );
        let evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader> =
            Arc::new(MoogleMailFulfillmentEvidenceReader { gateways });
        Ok((
            runtimes,
            Some(delivery_transport),
            Some(evidence_reader),
            Some(custody_reader),
            Some(gateway),
            ownership,
        ))
    }
}
