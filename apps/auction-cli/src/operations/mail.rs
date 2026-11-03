//! MoogleMail 运维证据：加密诊断归档的离线导出，以及撤拍退回义务的 Sent 只读核查。

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use auction_application::{
    AdminCredentialKind, AdminCredentialOpener, AdminCredentialRuntimeReader, AdminCredentialWorld,
    SettlementFulfillmentSentLookupReader, canonical_settlement_fulfillment_subject,
};
use auction_config::AppConfig;
use auction_domain::{SettlementId, SettlementObligationKind, SettlementWorld};
use auction_integrations::{
    CredentialSealer, DataKey,
    mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
        MoogleMailDiagnosticArchiveConfig, MoogleMailGateway, MoogleMailGatewayConfig,
        MoogleMailSentLookupRequest, MoogleMailWorld,
    },
};
use auction_storage::DatabaseStore;
use serde_json::json;
use uuid::Uuid;

use crate::app::print_json;
use crate::cli::{KeySourceArg, MailCommand};
use crate::deployment::ConfigDir;
use crate::support::files;

pub(crate) async fn run(config_dir: &ConfigDir, command: MailCommand) -> anyhow::Result<()> {
    match command {
        MailCommand::ExportEvidence { evidence_id, output, key_source } => {
            export(config_dir, evidence_id, &output, key_source)
        }
        MailCommand::InspectSent { obligation_id, output } => {
            files::require_private_target(&output)?;
            let (config, store) = super::worker_store(
                config_dir,
                auction_storage::DatabaseConnectionPurpose::CliOperations,
            )
            .await?;
            let result = inspect_sent(&config, &store, obligation_id, &output).await;
            store.postgres_pool().close().await;
            result
        }
    }
}

/// 父密钥来源省略时优先 credential、缺失才用 equipment；历史归档须保留对应旧密钥。
fn export(
    config_dir: &ConfigDir,
    evidence_id: Uuid,
    output: &Path,
    key_source: Option<KeySourceArg>,
) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let config = config_dir.load_worker()?;
    let (source, key_id, key_material) = match key_source {
        Some(KeySourceArg::Credential) => {
            ("credential", config.credential_data_key_id(), config.credential_data_key())
        }
        Some(KeySourceArg::Equipment) => {
            ("equipment", config.equipment_data_key_id(), config.equipment_data_key())
        }
        None if config.credential_data_key_configured() => {
            ("credential", config.credential_data_key_id(), config.credential_data_key())
        }
        None => ("equipment", config.equipment_data_key_id(), config.equipment_data_key()),
    };
    let data_key = DataKey::from_base64(
        key_id.context("所选父密钥未配置")?,
        key_material.context("所选父密钥未配置")?,
    )
    .map_err(|_| anyhow::anyhow!("所选父密钥格式无效"))?;
    let diagnostics = &config.worker.mooglemail.diagnostics;
    auction_integrations::mooglemail::export_mooglemail_diagnostic(
        MoogleMailDiagnosticArchiveConfig {
            directory: diagnostics.directory.clone(),
            retention: Duration::from_secs(u64::from(diagnostics.retention_days) * 86_400),
            maximum_total_bytes: diagnostics.maximum_total_bytes,
            key_source: source.to_owned(),
        },
        Arc::new(data_key),
        evidence_id,
        output.to_owned(),
    )
    .map_err(|error| anyhow::anyhow!("导出诊断证据失败：{}", error.as_str()))?;
    print_json(&json!({ "evidence_id": evidence_id, "output": output, "key_source": source }))
}

/// 冻结目标须为单件装备、零 Credits/COD；Sent 匹配只证明发送记录，collected 不等于已送达。
async fn inspect_sent(
    config: &AppConfig,
    store: &DatabaseStore,
    obligation_id: Uuid,
    output: &Path,
) -> anyhow::Result<()> {
    let target =
        store.read_sent_lookup_target(obligation_id).await.context("读取 Sent 核查目标失败")?;
    if target.credits != 0 || target.cod != 0 || target.recipient.trim().is_empty() {
        anyhow::bail!("Sent 核查目标违反零金额或收件人不变量");
    }
    let world = match target.world {
        SettlementWorld::Main => MoogleMailWorld::Main,
        SettlementWorld::Isekai => MoogleMailWorld::Isekai,
    };
    let subject = canonical_settlement_fulfillment_subject(
        SettlementObligationKind::WithdrawnReturn,
        SettlementId::from_uuid(obligation_id),
    )
    .context("无法生成履约邮件标题")?;
    let mail = &config.worker.mooglemail;
    let client = Arc::new(
        MoogleMailClient::with_config(MoogleMailClientConfig {
            endpoint: url::Url::parse(&mail.endpoint).context("MoogleMail endpoint 无效")?,
            maximum_response_bytes: mail.maximum_response_bytes,
            minimum_request_interval: Duration::from_millis(mail.minimum_request_interval_millis),
            request_timeout: Duration::from_secs(mail.request_timeout_seconds),
        })
        .map_err(|error| anyhow::anyhow!(error.to_string()))?,
    );
    let gateway = MoogleMailGateway::new(
        client,
        readonly_credentials(config, store, target.world).await?,
        MoogleMailGatewayConfig {
            maximum_pages: mail.maximum_pages,
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: true,
            fetch_budget: Duration::from_secs(mail.fetch_timeout_seconds),
        },
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let report = gateway
        .lookup_sent(MoogleMailSentLookupRequest {
            world,
            recipient: target.recipient.clone(),
            subject: subject.clone(),
            external_equipment_id: target.external_equipment_id.clone(),
            maximum_pages: mail.maximum_pages,
            deadline: Instant::now() + Duration::from_secs(mail.fetch_timeout_seconds),
            observed_at: chrono::Utc::now(),
        })
        .await
        .map_err(|error| anyhow::anyhow!("Sent 只读查询失败：{error}"))?;
    let candidates = report
        .candidates
        .iter()
        .map(|candidate| {
            json!({
                "external_mail_id": candidate.external_mail_id,
                "recipient": candidate.recipient,
                "subject": candidate.subject,
                "sent_at": candidate.sent_at,
                "remaining_credits": candidate.remaining_credits,
                "cod_pending": candidate.cod_pending,
                "remaining_equipment_ids": candidate.remaining_equipment_ids,
                "collected": candidate.collected,
                "observed_at": candidate.observed_at,
            })
        })
        .collect::<Vec<_>>();
    let evidence = json!({
        "schema_version": 1,
        "obligation_id": obligation_id,
        "status": format!("{:?}", report.status),
        "scan_complete": report.scan_complete,
        "pages_scanned": report.pages_scanned,
        "time_unverified": true,
        "target": {
            "world": target.world.as_str(),
            "recipient": target.recipient,
            "subject": subject,
            "external_equipment_id": target.external_equipment_id,
            "sending_at": target.sending_at,
            "completed_at": target.completed_at,
        },
        "candidates": candidates,
    });
    files::write_private(output, &serde_json::to_vec_pretty(&evidence)?)?;
    print_json(&json!({
        "obligation_id": obligation_id,
        "status": evidence["status"],
        "candidates": report.candidates.len(),
        "output": output,
    }))
}

/// 一次查询同时覆盖目标 world 与 shared scope；多于一个 active 凭据即拒绝，不拼接不同快照。
async fn readonly_credentials(
    config: &AppConfig,
    store: &DatabaseStore,
    world: SettlementWorld,
) -> anyhow::Result<MoogleMailCredentialSet> {
    let cookie = if let Some((key_id, key_material)) =
        config.credential_data_key_id().zip(config.credential_data_key())
    {
        let opener = CredentialSealer::new(Arc::new(
            DataKey::from_base64(key_id, key_material)
                .map_err(|_| anyhow::anyhow!("credential 数据密钥格式无效"))?,
        ));
        let credential_world = match world {
            SettlementWorld::Main => AdminCredentialWorld::Main,
            SettlementWorld::Isekai => AdminCredentialWorld::Isekai,
        };
        let mut records = store
            .list_active(AdminCredentialKind::MailCookie, credential_world)
            .await
            .context("读取 active 邮件凭据失败")?;
        if records.len() != 1 {
            anyhow::bail!("目标 world 必须恰好有一个 active 邮件凭据（当前 {}）", records.len());
        }
        let record = records.pop().context("邮件凭据不可用")?;
        let secret = opener
            .open(record.kind, record.id, &record.scope, &record.envelope)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        String::from_utf8(secret).context("邮件凭据不是 UTF-8")?
    } else if let Some(path) = &config.worker.mooglemail.keyenv_file {
        auction_config::load_mooglemail_keyenv(path)?.bot_cookie().to_owned()
    } else {
        config.mooglemail_cookie().context("没有可用的 MoogleMail 凭据")?.to_owned()
    };
    MoogleMailCredentialSet::from_cookies(vec![
        MoogleMailCookie::new(cookie).map_err(|error| anyhow::anyhow!(error.to_string()))?,
    ])
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}
