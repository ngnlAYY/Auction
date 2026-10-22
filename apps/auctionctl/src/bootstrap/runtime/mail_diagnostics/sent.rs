//! WithdrawnReturn 的只读 Sent 核对命令实现。

use std::path::PathBuf;

#[cfg(feature = "postgres")]
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(feature = "postgres")]
use anyhow::Context;
#[cfg(feature = "postgres")]
use auction_application::{
    AdminCredentialKind, AdminCredentialOpener, AdminCredentialRuntimeReader, AdminCredentialWorld,
    SettlementFulfillmentSentLookupReader,
};
#[cfg(feature = "postgres")]
use auction_config::Role;
#[cfg(feature = "postgres")]
use auction_integrations::{
    CredentialSealer, DataKey,
    mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
        MoogleMailGateway, MoogleMailGatewayConfig, MoogleMailSentLookupRequest,
    },
};
#[cfg(feature = "postgres")]
use auction_storage::DatabaseStore;
#[cfg(feature = "postgres")]
use chrono::Utc;
#[cfg(feature = "postgres")]
use serde_json::json;
use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::super::postgres::write_private_evidence;

/// 对指定撤拍义务执行只读 Sent 核对并创建安全 JSON 报告。
#[cfg(feature = "postgres")]
pub(crate) async fn inspect_sent(obligation_id: Uuid, output: PathBuf) -> anyhow::Result<()> {
    let config =
        auction_config::AppConfig::load(Role::Worker).context("load Worker configuration")?;
    let database_url = config.database_url().context("Worker database_url is unavailable")?;
    let store = DatabaseStore::connect(database_url).await.context("connect PostgreSQL")?;
    let target = store
        .read_sent_lookup_target(obligation_id)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if target.credits != 0 || target.cod != 0 || target.recipient.trim().is_empty() {
        anyhow::bail!("withdrawn return Sent target violates zero-amount or recipient invariant");
    }

    let target_world_name = format!("{:?}", target.world);
    let credentials = load_readonly_credentials(&config, &store, &target_world_name).await?;
    let mail_config = &config.worker.mooglemail;
    let endpoint = url::Url::parse(&mail_config.endpoint).context("parse MoogleMail endpoint")?;
    let client = Arc::new(
        MoogleMailClient::with_config(MoogleMailClientConfig {
            endpoint,
            maximum_response_bytes: mail_config.maximum_response_bytes,
            minimum_request_interval: Duration::from_millis(
                mail_config.minimum_request_interval_millis,
            ),
            request_timeout: Duration::from_secs(mail_config.request_timeout_seconds),
        })
        .map_err(|error| anyhow::anyhow!(error.to_string()))?,
    );
    let gateway = MoogleMailGateway::new(
        client,
        credentials,
        MoogleMailGatewayConfig {
            maximum_pages: mail_config.maximum_pages,
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: true,
            fetch_budget: Duration::from_secs(mail_config.fetch_timeout_seconds),
        },
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let observed_at = Utc::now();
    let mail_world = match format!("{:?}", target.world).as_str() {
        "Main" => auction_integrations::mooglemail::MoogleMailWorld::Main,
        "Isekai" => auction_integrations::mooglemail::MoogleMailWorld::Isekai,
        _ => anyhow::bail!("invalid settlement world"),
    };
    let report = gateway
        .lookup_sent(MoogleMailSentLookupRequest {
            world: mail_world,
            recipient: target.recipient.clone(),
            subject: format!("Auction equipment delivery [{obligation_id}]"),
            external_equipment_id: target.external_equipment_id.clone(),
            maximum_pages: mail_config.maximum_pages,
            deadline: Instant::now()
                .checked_add(Duration::from_secs(mail_config.fetch_timeout_seconds))
                .context("Sent lookup deadline overflow")?,
            observed_at,
        })
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    let candidates = report
        .candidates
        .iter()
        .map(|candidate| {
            json!({
                "external_mail_id": candidate.external_mail_id,
                "recipient": candidate.recipient,
                "subject": candidate.subject,
                "sent_at": candidate.sent_at.to_rfc3339(),
                "remaining_credits": candidate.remaining_credits,
                "cod_pending": candidate.cod_pending,
                "remaining_equipment_ids": candidate.remaining_equipment_ids,
                "collected": candidate.collected,
                "observed_at": candidate.observed_at.to_rfc3339(),
            })
        })
        .collect::<Vec<_>>();
    let output_value = json!({
        "schema_version": 1,
        "obligation_id": obligation_id,
        "status": format!("{:?}", report.status),
        "scan_complete": report.scan_complete,
        "pages_scanned": report.pages_scanned,
        "time_unverified": true,
        "target": {
            "world": format!("{:?}", target.world),
            "recipient": target.recipient,
            "subject": format!("Auction equipment delivery [{obligation_id}]"),
            "external_equipment_id": target.external_equipment_id,
            "credits": target.credits,
            "cod": target.cod,
            "sending_at": target.sending_at.map(|value| value.to_rfc3339()),
            "completed_at": target.completed_at.map(|value| value.to_rfc3339()),
        },
        "candidates": candidates,
    });
    let evidence = serde_json::to_vec_pretty(&output_value).context("encode Sent lookup report")?;
    write_private_evidence(&output, &evidence)?;
    println!(
        "sent lookup: obligation_id={obligation_id} status={:?} candidates={} output={}",
        report.status,
        report.candidates.len(),
        output.display()
    );
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn inspect_sent(_obligation_id: Uuid, _output: PathBuf) -> anyhow::Result<()> {
    anyhow::bail!("sent lookup requires the auctionctl postgres feature")
}

#[cfg(feature = "postgres")]
async fn load_readonly_credentials(
    config: &auction_config::AppConfig,
    store: &DatabaseStore,
    target_world: &str,
) -> anyhow::Result<MoogleMailCredentialSet> {
    if let Some((key_id, key_material)) =
        config.credential_data_key_id().zip(config.credential_data_key())
    {
        let data_key = Arc::new(
            DataKey::from_base64(key_id, key_material)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?,
        );
        let opener = CredentialSealer::new(data_key);
        let reader: &dyn AdminCredentialRuntimeReader = store;
        let mut records = Vec::new();
        let world = match target_world {
            "Main" => AdminCredentialWorld::Main,
            "Isekai" => AdminCredentialWorld::Isekai,
            _ => anyhow::bail!("invalid settlement world"),
        };
        for world in [world, AdminCredentialWorld::Shared] {
            let mut active = reader
                .list_active(AdminCredentialKind::MailCookie, world)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            records.append(&mut active);
        }
        records.sort_by_key(|record| record.id);
        records.dedup_by_key(|record| record.id);
        anyhow::ensure!(records.len() <= 1, "more than one active MoogleMail credential");
        let Some(record) = records.pop() else {
            anyhow::bail!("no active MoogleMail credential");
        };
        let secret = opener
            .open(record.kind, record.id, &record.scope, &record.envelope)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let cookie = String::from_utf8(secret).context("MoogleMail credential is not UTF-8")?;
        return MoogleMailCredentialSet::from_cookies(vec![
            MoogleMailCookie::new(cookie).map_err(|error| anyhow::anyhow!(error.to_string()))?,
        ])
        .map_err(|error| anyhow::anyhow!(error.to_string()));
    }
    if let Some(path) = &config.worker.mooglemail.keyenv_file {
        let keyenv =
            auction_config::load_mooglemail_keyenv(path).context("load MoogleMail keyenv")?;
        return MoogleMailCredentialSet::from_cookies(vec![
            MoogleMailCookie::new(keyenv.bot_cookie())
                .map_err(|error| anyhow::anyhow!(error.to_string()))?,
        ])
        .map_err(|error| anyhow::anyhow!(error.to_string()));
    }
    let cookie = config.mooglemail_cookie().context("no MoogleMail credential configured")?;
    MoogleMailCredentialSet::from_cookies(vec![
        MoogleMailCookie::new(cookie).map_err(|error| anyhow::anyhow!(error.to_string()))?,
    ])
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}
