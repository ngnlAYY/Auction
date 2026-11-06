//! 资金写入的联合身份证据读取器：只用数据库 active 凭据，取得与 Worker 相同的 mail/forum
//! runtime owner（因此 Worker 必须先停止），复用同一 MoogleMail 会话完成身份、草稿与现金观察。

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use auction_application::admin::{
    AdminCredentialKind, AdminCredentialOpener, AdminCredentialRuntimeOwnership,
    AdminCredentialRuntimeReader, AdminCredentialWorld,
};
use auction_application::settlement::SettlementInternalEvidenceReader;
use auction_config::AppConfig;
use auction_integrations::{
    CredentialSealer, build_keyring,
    ehentai::{
        EhentaiClient, EhentaiClientConfig, EhentaiCookie, EhentaiCredentialSet, EhentaiGateway,
    },
    mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
        MoogleMailGateway, MoogleMailGatewayConfig, MoogleMailRuntimeCredential, MoogleMailWorld,
    },
};
use auction_storage::DatabaseStore;

pub(super) async fn build(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
) -> anyhow::Result<Arc<dyn SettlementInternalEvidenceReader>> {
    if !config.worker.forum.enabled {
        anyhow::bail!("联合身份读取要求 Worker 启用论坛读取能力（[worker.forum].enabled）");
    }
    let keyring = build_keyring(
        auction_config::DataKeyDomain::Credential,
        config.data_key_current(auction_config::DataKeyDomain::Credential),
        config
            .data_key_previous(auction_config::DataKeyDomain::Credential)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .map_err(|_| anyhow::anyhow!("credential 数据密钥格式无效"))?
    .context("资金写入只接受数据库 active 凭据，敏感配置须提供 credential 数据密钥")?;
    let opener = CredentialSealer::new(keyring);

    let mut records =
        store.list_active(AdminCredentialKind::MailCookie, AdminCredentialWorld::Main).await?;
    if records.len() != 1 {
        anyhow::bail!("Main 必须恰好有一个 active 邮件凭据（当前 {}）", records.len());
    }
    let record = records.pop().context("邮件凭据不可用")?;
    let mail_owner = Arc::new(
        store
            .acquire_credential_runtime_ownership(AdminCredentialKind::MailCookie)
            .await
            .context("取得邮件凭据 owner 失败；请确认 Worker 已停止")?,
    ) as Arc<dyn AdminCredentialRuntimeOwnership>;
    let mut secret = opener.open(record.kind, record.id, &record.scope, &record.envelope)?;
    let cookie = std::str::from_utf8(&secret).map(str::to_owned);
    secret.fill(0);
    let shared =
        record.scope == AdminCredentialWorld::Shared.scope(AdminCredentialKind::MailCookie);
    let mail_credentials = MoogleMailCredentialSet::from_runtime_credentials(
        vec![MoogleMailRuntimeCredential {
            credential_id: record.id,
            state_version: record.state_version,
            player_name: record.player_name,
            world: (!shared).then_some(MoogleMailWorld::Main),
            cookie: MoogleMailCookie::new(cookie.context("邮件凭据不是 UTF-8")?)?,
        }],
        store.clone(),
        Some(mail_owner),
    )?;

    let forum = &config.worker.forum;
    let forum_world = AdminCredentialWorld::parse(&forum.credential_world)
        .context("[worker.forum].credential_world 无效")?;
    let mut records = store.list_active(AdminCredentialKind::ForumCookie, forum_world).await?;
    if records.len() != 1 {
        anyhow::bail!("论坛必须恰好有一个 active 凭据（当前 {}）", records.len());
    }
    let record = records.pop().context("论坛凭据不可用")?;
    let forum_owner = Arc::new(
        store
            .acquire_credential_runtime_ownership(AdminCredentialKind::ForumCookie)
            .await
            .context("取得论坛凭据 owner 失败；请确认 Worker 已停止")?,
    ) as Arc<dyn AdminCredentialRuntimeOwnership>;
    let mut secret = opener.open(record.kind, record.id, &record.scope, &record.envelope)?;
    let cookie = EhentaiCookie::from_bytes(&secret);
    secret.fill(0);
    let forum_credentials = EhentaiCredentialSet::from_owned_runtime_cookies(
        vec![(record.id, record.scope, cookie?)],
        store,
        forum_owner,
    )?;
    let forum_client = Arc::new(EhentaiClient::with_config(EhentaiClientConfig {
        endpoint: url::Url::parse(
            forum.endpoint.as_deref().unwrap_or(auction_integrations::ehentai::PRODUCTION_ENDPOINT),
        )?,
        maximum_response_bytes: forum.maximum_response_bytes,
        maximum_topic_pages: usize::from(forum.maximum_topic_pages),
        minimum_request_interval: Duration::from_millis(forum.minimum_request_interval_millis),
        request_timeout: Duration::from_secs(forum.request_timeout_seconds),
        test_board_mode: forum.test_board_mode,
        main_board_id: forum.main_board_id,
        isekai_board_id: forum.isekai_board_id,
    })?);
    let profile_reader = Arc::new(EhentaiGateway::new(forum_client, forum_credentials)?);

    let mail = &config.worker.mooglemail;
    let client = Arc::new(MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: url::Url::parse(&mail.endpoint)?,
        maximum_response_bytes: mail.maximum_response_bytes,
        minimum_request_interval: Duration::from_millis(mail.minimum_request_interval_millis),
        request_timeout: Duration::from_secs(mail.request_timeout_seconds),
    })?);
    Ok(Arc::new(
        MoogleMailGateway::new(
            client,
            mail_credentials,
            MoogleMailGatewayConfig {
                maximum_pages: mail.maximum_pages,
                sender_allowlist: Vec::new(),
                sender_allowlist_disabled: true,
                fetch_budget: Duration::from_secs(mail.fetch_timeout_seconds),
            },
        )?
        .with_forum_profile_reader(profile_reader),
    ))
}
