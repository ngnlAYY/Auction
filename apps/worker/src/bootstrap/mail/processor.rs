//! MoogleMail 入站 processor、装备来源、用户 key 和 delivery transport 装配。

use super::super::*;

pub(super) fn build_processor_and_delivery(
    config: &AppConfig,
    mail_config: &auction_config::MoogleMailWorkerConfig,
    world_configs: &[auction_config::MoogleMailWorldConfig],
    store: &Arc<DatabaseStore>,
    client: &MoogleMailClient,
    credentials: &MoogleMailCredentialSet,
) -> anyhow::Result<(Arc<dyn MailProcessor>, Arc<dyn DeliveryTransport>)> {
    match store.as_ref() {
        DatabaseStore::Postgres(postgres_store) => {
            let data_key_id = config
                .equipment_data_key_id()
                .context("MoogleMail business processing requires equipment_data_key_id")?;
            let data_key_material = config
                .equipment_data_key()
                .context("MoogleMail business processing requires equipment_data_key")?;
            let data_key = DataKey::from_base64(data_key_id, data_key_material)
                .map_err(|error| anyhow::Error::msg(error.to_string()))?;
            let sealer = Arc::new(EquipmentKeySealer::new(data_key));
            let equipment_client = build_equipment_http_client()
                .map_err(|error| anyhow::Error::msg(error.to_string()))?;
            let equipment = Arc::new(
                EquipmentSourceAdapter::production(
                    equipment_client,
                    EquipmentSourcePolicy::default(),
                )
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            );
            let auctions = Arc::new(MailAuctionIntakeService::new(
                Arc::new(postgres_store.clone()),
                sealer,
                Arc::new(SystemClock),
            ));
            let deliveries = Arc::clone(store);
            let user_data_key_id = config
                .user_login_data_key_id()
                .context("MoogleMail business processing requires user_login_data_key_id")?;
            let user_data_key_material = config
                .user_login_data_key()
                .context("MoogleMail business processing requires user_login_data_key")?;
            let user_data_key = Arc::new(
                DataKey::from_base64(user_data_key_id, user_data_key_material)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            );
            let key_issuer = Arc::new(MoogleMailLoginKeyIssuer::new(Arc::clone(&user_data_key)));
            let users = Arc::new(MailUserService::new(Arc::clone(store), key_issuer));
            let login_keys: Arc<dyn MailLoginKeyReader> =
                Arc::new(MoogleMailLoginKeyReader::new(Arc::clone(store), user_data_key));
            let notification_templates =
                MoogleMailNotificationTemplates::from_values(mail_config.mail_templates.values())
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
            let delivery_transport: Arc<dyn DeliveryTransport> =
                Arc::new(MoogleMailDeliveryTransport::with_templates(
                    client.clone(),
                    credentials.clone(),
                    login_keys,
                    notification_templates,
                ));
            let processor: Arc<dyn MailProcessor> = Arc::new(
                auction_application::MailProcessorService::new_for_worlds(
                    world_configs.iter().map(|world| world.world_id.clone()).collect(),
                    MailProcessorConfig {
                        start_price: mail_config
                            .start_price
                            .unwrap_or(config.auction.economics.start_price),
                        equipment_filter: MailEquipmentFilter {
                            enabled: mail_config.equipment_filter.enabled,
                            qualities: mail_config.equipment_filter.qualities.clone(),
                            prefixes: mail_config.equipment_filter.prefixes.clone(),
                            suffixes: mail_config.equipment_filter.suffixes.clone(),
                            categories: mail_config.equipment_filter.categories.clone(),
                            equipment_types: mail_config.equipment_filter.equipment_types.clone(),
                            level_min: mail_config.equipment_filter.level_min,
                            level_max: mail_config.equipment_filter.level_max,
                        },
                    },
                    equipment,
                    auctions,
                    deliveries,
                    users,
                    Arc::new(SystemClock),
                )
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            );
            Ok((processor, delivery_transport))
        }
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("MoogleMail 业务处理仅支持 PostgreSQL 完整档"),
    }
}
