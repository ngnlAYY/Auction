//! MoogleMail 轮询、业务处理、delivery gateway 与入站 envelope 转换。

#[cfg(feature = "postgres")]
use super::fulfillment::MoogleMailFulfillmentEvidenceReader;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::*;

#[cfg(feature = "postgres")]
struct MoogleMailWorkerGateway {
    gateway: Arc<MoogleMailGateway>,
    world: MoogleMailWorld,
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
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
        let credentials = if let Some((key_id, key_material)) =
            config.credential_data_key_id().zip(config.credential_data_key())
        {
            let data_key = Arc::new(
                DataKey::from_base64(key_id, key_material)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            );
            let opener = CredentialSealer::new(Arc::clone(&data_key));
            let reader: Arc<dyn AdminCredentialRuntimeReader> = store.clone();
            let mut records = Vec::new();
            let mut seen_record_ids = HashSet::new();
            for world_config in &world_configs {
                let credential_world = AdminCredentialWorld::parse(&world_config.world_id)
                    .context("MoogleMail world_id must be main or isekai")?;
                for record in reader
                    .list_active(AdminCredentialKind::MailCookie, credential_world)
                    .await
                    .context("load active MoogleMail credentials")?
                {
                    // shared scope 会在 main/isekai 两次查询中出现，只把同一凭据放入池一次。
                    if seen_record_ids.insert(record.id) {
                        records.push((record, credential_world));
                    }
                }
            }
            let mut cookies = Vec::with_capacity(records.len());
            for (record, credential_world) in records {
                let secret = opener
                    .open(
                        record.kind,
                        record.id,
                        &record.scope,
                        &record.envelope,
                    )
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                let value = match String::from_utf8(secret) {
                    Ok(value) => value,
                    Err(error) => {
                        let mut bytes = error.into_bytes();
                        bytes.fill(0);
                        anyhow::bail!("MoogleMail 凭据不是有效 UTF-8");
                    }
                };
                cookies.push((
                    record.id,
                    if record.scope
                        == AdminCredentialWorld::Shared.scope(AdminCredentialKind::MailCookie)
                    {
                        None
                    } else {
                        Some(match credential_world {
                            AdminCredentialWorld::Main => MoogleMailWorld::Main,
                            AdminCredentialWorld::Isekai => MoogleMailWorld::Isekai,
                            AdminCredentialWorld::Shared => {
                                anyhow::bail!("MoogleMail 凭据 world scope 无效")
                            }
                        })
                    },
                    MoogleMailCookie::new(value)
                        .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                ));
            }
            let invalidator: Arc<dyn AdminCredentialRuntimeWriter> = store.clone();
            MoogleMailCredentialSet::from_runtime_cookies_with_scopes(cookies, invalidator)
        } else if let Some(path) = &mail_config.keyenv_file {
            let keyenv = load_mooglemail_keyenv(path).context("load MoogleMail keyenv")?;
            MoogleMailCredentialSet::new(keyenv.bot_cookie(), keyenv.test_cookie())
        } else {
            let cookie = config.mooglemail_cookie().context(
                "MoogleMail enabled but no keyenv_file, credential pool, or sensitive mooglemail_cookie",
            )?;
            MoogleMailCredentialSet::new(cookie, cookie)
        }
        .map_err(|error| anyhow::Error::msg(error.to_string()))?;
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
        let (processor, delivery_transport): (Arc<dyn MailProcessor>, Arc<dyn DeliveryTransport>) =
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
                    let deliveries = Arc::clone(&store);
                    let user_data_key_id = config.user_login_data_key_id().context(
                        "MoogleMail business processing requires user_login_data_key_id",
                    )?;
                    let user_data_key_material = config
                        .user_login_data_key()
                        .context("MoogleMail business processing requires user_login_data_key")?;
                    let user_data_key = Arc::new(
                        DataKey::from_base64(user_data_key_id, user_data_key_material)
                            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                    );
                    let key_issuer =
                        Arc::new(MoogleMailLoginKeyIssuer::new(Arc::clone(&user_data_key)));
                    let users = Arc::new(MailUserService::new(Arc::clone(&store), key_issuer));
                    let login_keys: Arc<dyn MailLoginKeyReader> =
                        Arc::new(MoogleMailLoginKeyReader::new(Arc::clone(&store), user_data_key));
                    let notification_templates = MoogleMailNotificationTemplates::from_values(
                        mail_config.mail_templates.values(),
                    )
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                    let delivery_transport: Arc<dyn DeliveryTransport> =
                        Arc::new(MoogleMailDeliveryTransport::with_templates(
                            (*client).clone(),
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
                                    equipment_types: mail_config
                                        .equipment_filter
                                        .equipment_types
                                        .clone(),
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
                    (processor, delivery_transport)
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    anyhow::bail!("MoogleMail 业务处理仅支持 PostgreSQL 完整档");
                }
            };
        let store_port: Arc<dyn MailPollStore> = store.clone();
        let mut gateways = HashMap::with_capacity(world_configs.len());
        let mut runtimes = Vec::with_capacity(world_configs.len());
        for world_config in world_configs {
            let world = MoogleMailWorld::parse(&world_config.world_id)
                .context("MoogleMail world_id must be main or isekai")?;
            let gateway = Arc::new(
                MoogleMailGateway::new(
                    Arc::clone(&client),
                    credentials.clone(),
                    MoogleMailGatewayConfig {
                        maximum_pages: mail_config.maximum_pages,
                        sender_allowlist: world_config.sender_allowlist,
                        sender_allowlist_disabled: world_config.sender_allowlist_disabled,
                    },
                )
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            );
            store_port
                .ensure_state(world_config.mailbox_key.clone(), Utc::now())
                .await
                .context("initialize MoogleMail poll state")?;
            gateways.insert(world, Arc::clone(&gateway));
            let gateway_port: Arc<dyn MailPollGateway> =
                Arc::new(MoogleMailWorkerGateway { gateway, world });
            runtimes.push(MailPollRuntime::new(
                Arc::clone(&store_port),
                gateway_port,
                Some(Arc::clone(&processor)),
                world_config.world_id,
                world_config.mailbox_key,
                Arc::clone(&schedule),
                MailPollRuntimeConfig {
                    batch_size: mail_config.batch_size,
                    jitter: std::time::Duration::from_secs(mail_config.jitter_seconds),
                    lease_duration: std::time::Duration::from_secs(
                        mail_config.lease_duration_seconds,
                    ),
                    renew_interval: std::time::Duration::from_secs(
                        mail_config.renew_interval_seconds,
                    ),
                    fetch_attempts: mail_config.fetch_attempts,
                    fetch_timeout: std::time::Duration::from_secs(
                        mail_config.fetch_timeout_seconds,
                    ),
                    process_attempts: mail_config.process_attempts,
                    item_timeout: std::time::Duration::from_secs(mail_config.item_timeout_seconds),
                    retry_base: std::time::Duration::from_secs(mail_config.retry_base_seconds),
                    retry_maximum: std::time::Duration::from_secs(
                        mail_config.retry_maximum_seconds,
                    ),
                },
            )?);
        }
        let evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader> =
            Arc::new(MoogleMailFulfillmentEvidenceReader { gateways });
        Ok((runtimes, Some(delivery_transport), Some(evidence_reader)))
    }
}

#[cfg(feature = "postgres")]
impl MailPollGateway for MoogleMailWorkerGateway {
    fn fetch(
        &self,
        request: MailPollFetchRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<MailPollPage, MailPollGatewayError>>
                + Send
                + '_,
        >,
    > {
        let gateway = Arc::clone(&self.gateway);
        let world = self.world;
        Box::pin(async move {
            if request.world_id != world.as_str() {
                return Err(MailPollGatewayError {
                    failure_code: "configuration".to_owned(),
                    safe_detail: "MoogleMail Worker 世界配置与 gateway 不一致".to_owned(),
                    temporary: false,
                });
            }
            let page = gateway
                .fetch(world, request.cursor_value.as_deref(), usize::from(request.limit))
                .await
                .map_err(|error| MailPollGatewayError {
                    failure_code: error.category.as_str().to_owned(),
                    safe_detail: format!("MoogleMail {} 操作失败", error.category),
                    temporary: error.is_temporary(),
                })?;
            let messages = page
                .messages
                .into_iter()
                .map(|message| map_mooglemail_message(message, request.mailbox_key.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| MailPollGatewayError {
                    failure_code: "protocol_drift".to_owned(),
                    safe_detail: "MoogleMail 业务 envelope 转换失败".to_owned(),
                    temporary: false,
                })?;
            Ok(MailPollPage {
                mailbox_key: request.mailbox_key,
                world_id: world.as_str().to_owned(),
                next_cursor: page.next_cursor,
                messages,
            })
        })
    }
}

#[cfg(feature = "postgres")]
fn map_mooglemail_message(
    message: MoogleMailMessage,
    mailbox_key: String,
) -> Result<MailPollFetchedMessage, auction_integrations::mooglemail::MoogleMailError> {
    let external_mail_id = message.external_mail_id.clone();
    let world_id = message.world.as_str().to_owned();
    let received_at = message.received_at;
    let subject = message.subject.clone();
    let content_digest = message.content_digest();
    let attachment_count = u32::try_from(message.attachments.len()).unwrap_or(u32::MAX);
    let credits = message.credits;
    let cod = message.cod;
    let returnable = message.returnable;
    let claimable = message.claimable;
    let business = into_application_message(message, mailbox_key)?;
    Ok(MailPollFetchedMessage {
        external_mail_id,
        world_id,
        received_at,
        subject,
        content_digest,
        attachment_count,
        credits,
        cod,
        returnable,
        claimable,
        business: Some(business),
    })
}
