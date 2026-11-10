//! MoogleMail Worker 的配置、业务装配和轮询运行时入口；各 owner 由子模块持有，
//! 本模块只组合所需端口集合。

use super::fulfillment::MoogleMailFulfillmentEvidenceReader;
use super::*;

mod credentials;
mod diagnostics;
mod mapping;
mod processor;
mod runtime;

/// 远端只读所需的进程内依赖：本进程唯一的装备来源适配器、按世界的 gateway 与预算。
/// 与 Worker 自身请求共用同一实例，因此不存在第二套限速器或熔断状态。
pub(super) struct MailControlRemoteReads {
    pub(super) equipment: Arc<EquipmentSourceAdapter>,
    pub(super) gateways: HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
    pub(super) maximum_pages: usize,
    pub(super) fetch_timeout: std::time::Duration,
}

/// 根据 Worker 配置装配 MoogleMail 轮询、业务处理、delivery 和履约证据读取端口。
// 装配根逐项注入显式依赖，避免为单次调用引入仅用于传参的容器。
#[allow(clippy::too_many_arguments)]
pub(super) async fn configure_mail_poll(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    profile_reader: Option<Arc<dyn auction_application::forum::ForumTargetProfileReader>>,
    progress_observer: Arc<dyn auction_application::mail::MailPollProgressObserver>,
    sent_baseline_observer: Arc<dyn auction_application::sent_baseline::SentBaselineObserver>,
    mail_scan_observer: Arc<dyn auction_application::mail::MailScanObserver>,
    reply_policy: Arc<dyn auction_application::mail::MailReplyPolicySource>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
    external_metrics: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
) -> anyhow::Result<(
    Vec<MailPollRuntime>,
    Option<Arc<dyn DeliveryTransport>>,
    Option<Arc<dyn SettlementFulfillmentEvidenceReader>>,
    Option<Arc<dyn SettlementFulfillmentCustodyReader>>,
    Option<Arc<dyn auction_application::settlement::SettlementInternalEvidenceReader>>,
    Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
    Option<MailControlRemoteReads>,
    Option<Arc<dyn auction_application::reconciliation::AssetInventoryReader>>,
)> {
    let mail_config = &config.worker.mooglemail;
    if !mail_config.enabled {
        return Ok((Vec::new(), None, None, None, None, None, None, None));
    }

    let world_configs = mail_config.configured_worlds();
    let schedule: Arc<dyn RecurringSchedule> = Arc::new(
        StandardCronSchedule::parse(&mail_config.schedule)
            .context("parse MoogleMail standard cron schedule")?,
    );
    // AppConfig 已应用 Admin 托管覆盖；省略维护日程时跟随最终生效的收件日程。
    // 即使基线关闭也解析并校验显式维护日程，在线切回隔离时不能暴露启动时未发现的错误。
    let maintenance_schedule: Arc<dyn RecurringSchedule> = match &mail_config.maintenance_schedule {
        Some(expression) => {
            Arc::new(StandardCronSchedule::parse(expression).context("解析 MoogleMail 维护 cron")?)
        }
        None => Arc::clone(&schedule),
    };
    let (credentials, ownership) =
        credentials::load_credentials(config, mail_config, &world_configs, Arc::clone(&store))
            .await?;
    let credentials = credentials.with_external_metrics(Arc::clone(&external_metrics));
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
    // 每日完整校验间隔由部署配置拥有；默认与旧常量一致，避免改变既有扫描节奏。
    client = client.with_mail_scan_daily_full_interval(chrono::Duration::seconds(
        i64::try_from(mail_config.mail_scan_daily_full_interval_seconds)
            .context("MoogleMail 扫描校验间隔超出可表示范围")?,
    ));
    if let Some(recorder) = diagnostics::configure(config)? {
        client = client.with_diagnostics(recorder);
    }
    let client = Arc::new(client.with_external_metrics(Arc::clone(&external_metrics)));
    let (processor, delivery_transport, equipment_source) =
        processor::build_processor_and_delivery(
            config,
            mail_config,
            &world_configs,
            &store,
            client.as_ref(),
            &credentials,
            sent_baseline_observer,
            Arc::clone(&mail_scan_observer),
            reply_policy,
            Arc::clone(&external_metrics),
        )?;
    let maintenance = Arc::new(auction_application::mail::MailboxMaintenanceService::new(
        Arc::new(delivery_transport.maintenance_adapter())
            as Arc<dyn auction_application::mail::MoogleMailboxMaintenance>,
        Arc::clone(&store) as Arc<dyn auction_application::mail::MoogleMailboxEvidenceStore>,
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
            mail_scan: Arc::clone(&mail_scan_observer),
            switches,
            external_metrics: Arc::clone(&external_metrics),
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
    let control_reads = MailControlRemoteReads {
        equipment: equipment_source,
        gateways: gateways.clone(),
        maximum_pages: mail_config.maximum_pages,
        fetch_timeout: std::time::Duration::from_secs(mail_config.fetch_timeout_seconds),
    };
    let inventory_reader: Arc<dyn auction_application::reconciliation::AssetInventoryReader> =
        Arc::new(super::fulfillment::MoogleMailAssetInventoryReader { gateways: gateways.clone() });
    let evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader> =
        Arc::new(MoogleMailFulfillmentEvidenceReader { gateways });
    Ok((
        runtimes,
        Some(delivery_transport),
        Some(evidence_reader),
        Some(custody_reader),
        Some(gateway),
        ownership,
        Some(control_reads),
        Some(inventory_reader),
    ))
}
