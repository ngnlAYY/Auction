//! 仅显式apply装配既有网关；数据库active凭据、同session与generation不可省略。

use anyhow::Context;
use auction_application::{
    AdminCredentialKind, AdminCredentialOpener, AdminCredentialRuntimeOwnership,
    AdminCredentialRuntimeReader, AdminCredentialWorld, SettlementInternalEvidenceReader,
};
use auction_config::AppConfig;
use auction_integrations::{
    CredentialSealer, DataKey,
    ehentai::{
        EhentaiClient, EhentaiClientConfig, EhentaiCookie, EhentaiCredentialSet, EhentaiGateway,
    },
    mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
        MoogleMailGateway, MoogleMailGatewayConfig, MoogleMailRuntimeCredential, MoogleMailWorld,
    },
};
use auction_storage::DatabaseStore;
use std::{sync::Arc, time::Duration};

pub(super) async fn build(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
) -> anyhow::Result<Arc<dyn SettlementInternalEvidenceReader>> {
    anyhow::ensure!(config.worker.forum.enabled, "联合身份读取要求已启用的论坛读取能力");
    let (key_id, key_material) = config
        .credential_data_key_id()
        .zip(config.credential_data_key())
        .context("资金对账仅接受数据库active凭据，不支持keyenv/配置Cookie")?;
    let key = Arc::new(
        DataKey::from_base64(key_id, key_material).context("invalid credential data key")?,
    );
    let opener = CredentialSealer::new(key);
    let records =
        store.list_active(AdminCredentialKind::MailCookie, AdminCredentialWorld::Main).await?;
    anyhow::ensure!(records.len() == 1, "require exactly one active Main/shared mail credential");
    let record = records.into_iter().next().context("mail credential unavailable")?;
    let mail_owner = Arc::new(
        store.acquire_credential_runtime_ownership(AdminCredentialKind::MailCookie).await?,
    ) as Arc<dyn AdminCredentialRuntimeOwnership>;
    let mut secret = opener.open(record.kind, record.id, &record.scope, &record.envelope)?;
    let value = std::str::from_utf8(&secret).map(str::to_owned);
    secret.fill(0);
    let cookie = MoogleMailCookie::new(value.context("mail credential invalid UTF-8")?)?;
    let credentials = MoogleMailCredentialSet::from_runtime_credentials(
        vec![MoogleMailRuntimeCredential {
            credential_id: record.id,
            state_version: record.state_version,
            player_name: record.player_name,
            world: if record.scope
                == AdminCredentialWorld::Shared.scope(AdminCredentialKind::MailCookie)
            {
                None
            } else {
                Some(MoogleMailWorld::Main)
            },
            cookie,
        }],
        store.clone(),
        Some(mail_owner),
    )?;
    let forum_config = &config.worker.forum;
    let forum_world = AdminCredentialWorld::parse(&forum_config.credential_world)
        .context("invalid forum credential world")?;
    let records = store.list_active(AdminCredentialKind::ForumCookie, forum_world).await?;
    anyhow::ensure!(records.len() == 1, "require exactly one active forum credential");
    let record = records.into_iter().next().context("forum credential unavailable")?;
    let forum_owner = Arc::new(
        store.acquire_credential_runtime_ownership(AdminCredentialKind::ForumCookie).await?,
    ) as Arc<dyn AdminCredentialRuntimeOwnership>;
    let mut secret = opener.open(record.kind, record.id, &record.scope, &record.envelope)?;
    let cookie = EhentaiCookie::from_bytes(&secret);
    secret.fill(0);
    let forum_credentials = EhentaiCredentialSet::from_owned_runtime_cookies(
        vec![(record.id, cookie?)],
        store,
        forum_owner,
    )?;
    let forum_client = Arc::new(EhentaiClient::with_config(EhentaiClientConfig {
        endpoint: url::Url::parse(
            forum_config
                .endpoint
                .as_deref()
                .unwrap_or(auction_integrations::ehentai::PRODUCTION_ENDPOINT),
        )?,
        maximum_response_bytes: forum_config.maximum_response_bytes,
        maximum_topic_pages: usize::from(forum_config.maximum_topic_pages),
        minimum_request_interval: Duration::from_millis(
            forum_config.minimum_request_interval_millis,
        ),
        request_timeout: Duration::from_secs(forum_config.request_timeout_seconds),
        test_board_mode: forum_config.test_board_mode,
        main_board_id: forum_config.main_board_id,
        isekai_board_id: forum_config.isekai_board_id,
    })?);
    let forum = Arc::new(EhentaiGateway::new(forum_client, forum_credentials)?);
    let mail_config = &config.worker.mooglemail;
    let client = Arc::new(MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: url::Url::parse(&mail_config.endpoint)?,
        maximum_response_bytes: mail_config.maximum_response_bytes,
        minimum_request_interval: Duration::from_millis(
            mail_config.minimum_request_interval_millis,
        ),
        request_timeout: Duration::from_secs(mail_config.request_timeout_seconds),
    })?);
    Ok(Arc::new(
        MoogleMailGateway::new(
            client,
            credentials,
            MoogleMailGatewayConfig {
                maximum_pages: mail_config.maximum_pages,
                sender_allowlist: Vec::new(),
                sender_allowlist_disabled: true,
                fetch_budget: Duration::from_secs(mail_config.fetch_timeout_seconds),
            },
        )?
        .with_forum_profile_reader(forum),
    ))
}
