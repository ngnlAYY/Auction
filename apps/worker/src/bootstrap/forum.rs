//! E-Hentai 论坛同步与回帖运行时装配。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn configure_forum(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    delivery_enabled: bool,
) -> anyhow::Result<(Option<auction_jobs::ForumSyncRuntime>, Option<auction_jobs::ForumReplyRuntime>)>
{
    let forum_config = &config.worker.forum;
    if !forum_config.enabled {
        return Ok((None, None));
    }
    if store.backend() != DatabaseBackend::Postgres {
        anyhow::bail!("E-Hentai 论坛同步仅支持 PostgreSQL 完整档");
    }

    let credential_world = AdminCredentialWorld::parse(&forum_config.credential_world)
        .context("forum credential_world must be main, isekai, or shared")?;
    let credentials = if let Some((key_id, key_material)) =
        config.credential_data_key_id().zip(config.credential_data_key())
    {
        let data_key = Arc::new(
            DataKey::from_base64(key_id, key_material)
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        );
        let opener = CredentialSealer::new(Arc::clone(&data_key));
        let reader: Arc<dyn AdminCredentialRuntimeReader> = store.clone();
        let records = reader
            .list_active(AdminCredentialKind::ForumCookie, credential_world)
            .await
            .context("load active E-Hentai forum credentials")?;
        anyhow::ensure!(records.len() <= 1, "E-Hentai 论坛只允许一个 active forum_cookie");
        let ownership = if records.is_empty() {
            None
        } else {
            Some(Arc::new(
                store
                    .acquire_credential_runtime_ownership(AdminCredentialKind::ForumCookie)
                    .await
                    .context("acquire active E-Hentai forum credential ownership")?,
            ) as Arc<dyn AdminCredentialRuntimeOwnership>)
        };
        let mut cookies = Vec::with_capacity(records.len());
        for record in records {
            let secret = opener
                .open(record.kind, record.id, &record.scope, &record.envelope)
                .map_err(|error| anyhow::Error::msg(error.to_string()))?;
            cookies.push((
                record.id,
                EhentaiCookie::from_bytes(&secret)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            ));
        }
        let invalidator: Arc<dyn AdminCredentialRuntimeWriter> = store.clone();
        if let Some(ownership) = ownership {
            EhentaiCredentialSet::from_owned_runtime_cookies(cookies, invalidator, ownership)
        } else {
            EhentaiCredentialSet::from_runtime_cookies(cookies, invalidator)
        }
    } else {
        let cookie = config
            .ehentai_cookie()
            .context("forum enabled but no ehentai_cookie or credential data key")?;
        let ownership = Arc::new(
            store
                .acquire_credential_runtime_ownership(AdminCredentialKind::ForumCookie)
                .await
                .context("acquire configured E-Hentai forum credential ownership")?,
        ) as Arc<dyn AdminCredentialRuntimeOwnership>;
        EhentaiCredentialSet::from_owned_cookie(
            EhentaiCookie::from_bytes(cookie.as_bytes())
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            ownership,
        )
    }
    .map_err(|error| anyhow::Error::msg(error.to_string()))?;

    let endpoint = forum_config
        .endpoint
        .as_deref()
        .map(Url::parse)
        .transpose()
        .context("parse E-Hentai forum endpoint")?
        .unwrap_or(Url::parse(auction_integrations::ehentai::PRODUCTION_ENDPOINT)?);
    let client = Arc::new(
        EhentaiClient::with_config(EhentaiClientConfig {
            endpoint,
            maximum_response_bytes: forum_config.maximum_response_bytes,
            maximum_topic_pages: usize::from(forum_config.maximum_topic_pages),
            minimum_request_interval: std::time::Duration::from_millis(
                forum_config.minimum_request_interval_millis,
            ),
            request_timeout: std::time::Duration::from_secs(forum_config.request_timeout_seconds),
            test_board_mode: forum_config.test_board_mode,
        })
        .map_err(|error| anyhow::Error::msg(error.to_string()))?,
    );
    let gateway = Arc::new(
        EhentaiGateway::new(Arc::clone(&client), credentials)
            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
    );
    let store_port: Arc<dyn ForumSyncStore> = store.clone();
    let credential_availability: Arc<dyn ForumCredentialAvailability> = gateway.clone();
    let publisher: Arc<dyn ForumPublisher> = gateway.clone();
    let reader: Arc<dyn ForumTopicReader> = gateway.clone();
    let post_writer: Arc<dyn ForumPostWriter> = gateway.clone();
    let renderer: Arc<dyn ForumRenderer> = Arc::new(ForumThreadRenderer);
    let (equipment_key_id, equipment_key_material) = config
        .equipment_data_key_id()
        .zip(config.equipment_data_key())
        .context("forum enabled but equipment data key is incomplete")?;
    let equipment_data_key = DataKey::from_base64(equipment_key_id, equipment_key_material)
        .map_err(|_| anyhow::anyhow!("forum equipment data key is invalid"))?;
    let equipment_secrets: Arc<dyn PublicEquipmentLinkSecretReader> = store.clone();
    let equipment_links: Arc<dyn PublicEquipmentLinkResolver> =
        Arc::new(EquipmentGameLinkResolver::new(
            equipment_secrets,
            EquipmentKeyOpener::new(equipment_data_key),
        ));
    let sync = Some(
        auction_jobs::ForumSyncRuntime::new(
            auction_jobs::ForumSyncPorts {
                store: store_port,
                credential_availability,
                publisher,
                reader: reader.clone(),
                post_writer: post_writer.clone(),
                renderer: renderer.clone(),
                equipment_links: equipment_links.clone(),
            },
            forum_config.batch_size,
            std::time::Duration::from_secs(forum_config.item_timeout_seconds),
            forum_config.public_base_url.clone(),
        )?
        .with_post_body_verification(forum_config.verify_post_body),
    );
    let reply_store: Arc<dyn ForumReplyStore> = store.clone();
    let commands: Arc<dyn CompetitionCommandPort> =
        Arc::new(DatabaseCompetitionCommands::new(Arc::clone(&store), Arc::new(SystemClock)));
    let notifier: Option<Arc<dyn MailDeliveryEnqueuer>> =
        if delivery_enabled { Some(store.clone()) } else { None };
    let reply = Some(
        auction_jobs::ForumReplyRuntime::new(
            auction_jobs::ForumReplyPorts {
                store: reply_store,
                reader,
                writer: post_writer,
                renderer,
                equipment_links,
                commands,
                notifier,
            },
            forum_config.batch_size,
            std::time::Duration::from_secs(forum_config.item_timeout_seconds),
            forum_config.public_base_url.clone(),
        )?
        .with_post_body_verification(forum_config.verify_post_body),
    );
    Ok((sync, reply))
}
