//! E-Hentai 论坛同步与回帖运行时装配。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn configure_forum(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    external_metrics: Arc<dyn auction_application::ExternalMetricsObserver>,
) -> anyhow::Result<(
    Option<auction_jobs::ForumSyncRuntime>,
    Option<auction_jobs::ForumReplyRuntime>,
    Option<auction_jobs::LotWatchPmRuntime>,
    Option<Arc<dyn auction_application::ForumTargetProfileReader>>,
    Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
)> {
    let forum_config = &config.worker.forum;
    if !forum_config.enabled {
        return Ok((None, None, None, None, None));
    }
    if store.backend() != DatabaseBackend::Postgres {
        anyhow::bail!("E-Hentai 论坛同步仅支持 PostgreSQL 完整档");
    }

    let credential_world = AdminCredentialWorld::parse(&forum_config.credential_world)
        .context("forum credential_world must be main, isekai, or shared")?;
    let mut runtime_ownership = None;
    let credential_keyring = build_keyring(
        auction_config::DataKeyDomain::Credential,
        config.data_key_current(auction_config::DataKeyDomain::Credential),
        config
            .data_key_previous(auction_config::DataKeyDomain::Credential)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
    let credentials = if let Some(keyring) = credential_keyring {
        let opener = CredentialSealer::new(keyring);
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
                record.scope,
                EhentaiCookie::from_bytes(&secret)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            ));
        }
        let invalidator: Arc<dyn AdminCredentialRuntimeWriter> = store.clone();
        if let Some(ownership) = ownership {
            runtime_ownership = Some(Arc::clone(&ownership));
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
        runtime_ownership = Some(Arc::clone(&ownership));
        EhentaiCredentialSet::from_owned_cookie(
            EhentaiCookie::from_bytes(cookie.as_bytes())
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            credential_world,
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
            main_board_id: forum_config.main_board_id,
            isekai_board_id: forum_config.isekai_board_id,
        })
        .map_err(|error| anyhow::Error::msg(error.to_string()))?
        .with_external_metrics(Arc::clone(&external_metrics)),
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
    let renderer: Arc<dyn ForumRenderer> =
        Arc::new(ForumThreadRenderer::with_colors(forum_config.equipment_colors.clone()));
    let equipment_keyring = build_keyring(
        auction_config::DataKeyDomain::Equipment,
        config.data_key_current(auction_config::DataKeyDomain::Equipment),
        config
            .data_key_previous(auction_config::DataKeyDomain::Equipment)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .map_err(|_| anyhow::anyhow!("forum equipment data key is invalid"))?
    .context("forum enabled but equipment data key is incomplete")?;
    let equipment_secrets: Arc<dyn PublicEquipmentLinkSecretReader> = store.clone();
    let equipment_links: Arc<dyn PublicEquipmentLinkResolver> =
        Arc::new(EquipmentGameLinkResolver::new(
            equipment_secrets,
            EquipmentKeyOpener::new(equipment_keyring),
        ));
    let sync = Some(
        auction_jobs::ForumSyncRuntime::new(
            auction_jobs::ForumSyncPorts {
                store: store_port,
                credential_availability: credential_availability.clone(),
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
        Arc::new(DatabaseCompetitionCommands::new(Arc::clone(&store)));
    let reply = Some(
        auction_jobs::ForumReplyRuntime::new(
            auction_jobs::ForumReplyPorts {
                store: reply_store,
                credential_availability,
                reader,
                writer: post_writer,
                renderer,
                equipment_links,
                commands,
            },
            forum_config.batch_size,
            std::time::Duration::from_secs(forum_config.item_timeout_seconds),
            forum_config.public_base_url.clone(),
        )?
        .with_post_body_verification(forum_config.verify_post_body)
        .with_external_metrics(Arc::clone(&external_metrics)),
    );
    // PM 复用同一网关、限速及凭据 ownership，与 MM enabled 无关。
    let pm = auction_jobs::LotWatchPmRuntime::new(
        store,
        gateway.clone(),
        std::time::Duration::from_secs(config.worker.delivery.poll_interval_seconds),
        std::time::Duration::from_secs(config.worker.delivery.jitter_seconds),
        std::time::Duration::from_secs(forum_config.item_timeout_seconds),
        std::time::Duration::from_secs(forum_config.lease_duration_seconds),
    )
    .map_err(anyhow::Error::msg)?
    .with_external_metrics(external_metrics);
    Ok((sync, reply, Some(pm), Some(gateway), runtime_ownership))
}
