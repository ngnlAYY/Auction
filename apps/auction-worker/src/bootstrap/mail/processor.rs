//! MoogleMail 入站 processor、装备来源、用户 key 和 delivery transport 装配。

use crate::bootstrap::SystemClock;
use anyhow::Context;
use auction_application::mail::{
    MailAuctionIntakeService, MailEquipmentFilter, MailLoginKeyReader, MailProcessor,
    MailProcessorConfig, MailUserService,
};
use auction_application::settlement::SettlementFulfillmentEquipmentSecretReader;
use auction_config::AppConfig;
use auction_integrations::mooglemail::{
    MoogleMailClient, MoogleMailCredentialSet, MoogleMailDeliveryTransport,
    MoogleMailNotificationTemplates,
};
use auction_integrations::{
    EquipmentKeyOpener, EquipmentKeySealer, EquipmentSourceAdapter, EquipmentSourcePolicy,
    MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader, build_equipment_http_client, build_keyring,
};
use auction_storage::DatabaseStore;
use std::sync::Arc;

/// processor、delivery transport 与本进程唯一的装备来源适配器。
/// control 远端读取复用同一实例，因此 Admin 装备读取与 Worker 自身请求共用同一请求门与熔断。
type MailProcessorPorts =
    (Arc<dyn MailProcessor>, MoogleMailDeliveryTransport, Arc<EquipmentSourceAdapter>);

// 装配根逐项注入显式依赖；参数数量来自配置/存储/外部客户端的清晰边界，不引入仅用于传参的容器。
#[allow(clippy::too_many_arguments)]
pub(super) fn build_processor_and_delivery(
    config: &AppConfig,
    mail_config: &auction_config::MoogleMailWorkerConfig,
    world_configs: &[auction_config::MoogleMailWorldConfig],
    store: &Arc<DatabaseStore>,
    client: &MoogleMailClient,
    credentials: &MoogleMailCredentialSet,
    sent_baseline_observer: Arc<dyn auction_application::sent_baseline::SentBaselineObserver>,
    mail_scan_observer: Arc<dyn auction_application::mail::MailScanObserver>,
    reply_policy: Arc<dyn auction_application::mail::MailReplyPolicySource>,
    external_metrics: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
) -> anyhow::Result<MailProcessorPorts> {
    let postgres_store = store.as_ref();
    let equipment_keyring = build_keyring(
        auction_config::DataKeyDomain::Equipment,
        config.data_key_current(auction_config::DataKeyDomain::Equipment),
        config
            .data_key_previous(auction_config::DataKeyDomain::Equipment)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))?
    .context("MoogleMail business processing requires equipment_data_key")?;
    let sealer = Arc::new(EquipmentKeySealer::new(Arc::clone(equipment_keyring.current())));
    let delivery_opener = EquipmentKeyOpener::new(equipment_keyring);
    let equipment_client =
        build_equipment_http_client().map_err(|error| anyhow::Error::msg(error.to_string()))?;
    let mut equipment =
        EquipmentSourceAdapter::production(equipment_client, EquipmentSourcePolicy::default())
            .map_err(|error| anyhow::Error::msg(error.to_string()))?
            .with_mooglemail_credentials(credentials.clone())
            .with_diagnostic_reporter(super::diagnostics::report_equipment_failure);
    if let Some(recorder) = client.diagnostic_recorder() {
        equipment = equipment.with_diagnostics(recorder);
    }
    let equipment = Arc::new(equipment);
    let equipment_for_control = Arc::clone(&equipment);
    let equipment_secrets: Arc<dyn SettlementFulfillmentEquipmentSecretReader> = store.clone();
    let auctions = Arc::new(MailAuctionIntakeService::new(
        Arc::new(
            postgres_store
                .for_transaction(auction_observability::DatabaseTransactionOwner::MailIntake),
        ),
        sealer,
        Arc::new(SystemClock),
    ));
    let deliveries = Arc::clone(store);
    let user_login_keyring = build_keyring(
        auction_config::DataKeyDomain::UserLogin,
        config.data_key_current(auction_config::DataKeyDomain::UserLogin),
        config
            .data_key_previous(auction_config::DataKeyDomain::UserLogin)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))?
    .context("MoogleMail business processing requires user_login_data_key")?;
    let key_issuer =
        Arc::new(MoogleMailLoginKeyIssuer::new(Arc::clone(user_login_keyring.current())));
    let users = Arc::new(MailUserService::new(Arc::clone(store), key_issuer));
    let login_keys: Arc<dyn MailLoginKeyReader> =
        Arc::new(MoogleMailLoginKeyReader::new(Arc::clone(store), user_login_keyring));
    // 与创建被超提醒的事务共用同一份快照；数据库里的修订由周期刷新任务载入。
    let notification_templates = MoogleMailNotificationTemplates::live(store.live_mail_templates());
    let delivery_transport = MoogleMailDeliveryTransport::with_templates(
        client.clone(),
        credentials.clone(),
        login_keys,
        notification_templates,
    )
    .with_send_verification(
        mail_config.maximum_pages,
        std::time::Duration::from_secs(mail_config.fetch_timeout_seconds),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))?
    .with_settlement_equipment_secrets(equipment_secrets, delivery_opener)
    .with_sent_baseline(mail_config.sent_baseline_mode, sent_baseline_observer)
    .with_mail_scan_observer(mail_scan_observer)
    .with_mail_scan_checkpoint_store(
        Arc::clone(store) as Arc<dyn auction_application::mail::MailScanCheckpointStore>
    );
    let processor: Arc<dyn MailProcessor> = Arc::new(
        auction_application::mail::MailProcessorService::new_for_worlds(
            world_configs.iter().map(|world| world.world_id.clone()).collect(),
            MailProcessorConfig {
                start_price: mail_config.start_price.unwrap_or(50_000),
                rekey_enabled: mail_config.rekey_enabled,
                account_reply_window_seconds: mail_config.account_reply_window_seconds,
                explanation_window_seconds: mail_config.explanation_mail_window_seconds,
                explanation_limit: mail_config.explanation_mail_limit,
                equipment_filter: MailEquipmentFilter {
                    enabled: mail_config.equipment_filter.enabled,
                    qualities: mail_config.equipment_filter.qualities.clone(),
                    prefixes: mail_config.equipment_filter.prefixes.clone(),
                    suffixes: mail_config.equipment_filter.suffixes.clone(),
                    categories: mail_config.equipment_filter.categories.clone(),
                    equipment_types: mail_config.equipment_filter.equipment_types.clone(),
                    level_min: mail_config.equipment_filter.level_min,
                    level_max: mail_config.equipment_filter.level_max,
                    // 早鸟窗口内的投稿改用早鸟品质；与排期共用同一配置投影。
                    early_bird: config
                        .auction
                        .schedule_calendar()?
                        .and_then(|calendar| calendar.early_bird().cloned()),
                },
            },
            equipment,
            auctions,
            deliveries,
            users,
            Arc::new(SystemClock),
        )
        .map_err(|error| anyhow::Error::msg(error.to_string()))?
        .with_external_metrics(external_metrics)
        .with_reply_policy_source(reply_policy),
    );
    Ok((processor, delivery_transport, equipment_for_control))
}
