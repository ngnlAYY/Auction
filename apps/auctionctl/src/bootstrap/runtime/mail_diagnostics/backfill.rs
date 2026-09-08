//! 通过 authenticated parsed_message 证据安全回填邮件展示投影。

use super::super::cli::MailEvidenceKeySourceArg;
use uuid::Uuid;

#[cfg(feature = "postgres")]
pub(in super::super) async fn backfill_display(
    evidence_id: Uuid,
    requested_key_source: Option<MailEvidenceKeySourceArg>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use auction_application::MailDisplaySnapshot;
    use auction_config::Role;
    use auction_integrations::DataKey;
    use auction_integrations::mooglemail::{
        MoogleMailDiagnosticArchive, MoogleMailDiagnosticArchiveConfig,
    };
    use auction_storage::{DatabaseBackend, DatabaseStore, MailDisplayBackfillInput};
    use std::{sync::Arc, time::Duration};

    if evidence_id.is_nil() {
        anyhow::bail!("evidence-id 必须是非零 UUID");
    }
    let config =
        auction_config::AppConfig::load(Role::Worker).context("load Worker configuration")?;
    let diagnostics = &config.worker.mooglemail.diagnostics;
    let (key_source, key_id, key_material) = match requested_key_source {
        Some(MailEvidenceKeySourceArg::Credential) => {
            ("credential", config.credential_data_key_id(), config.credential_data_key())
        }
        Some(MailEvidenceKeySourceArg::Equipment) => {
            ("equipment", config.equipment_data_key_id(), config.equipment_data_key())
        }
        None if config.credential_data_key_configured() => {
            ("credential", config.credential_data_key_id(), config.credential_data_key())
        }
        None => ("equipment", config.equipment_data_key_id(), config.equipment_data_key()),
    };
    let key_id = key_id.context("selected MoogleMail diagnostic parent key_id is unavailable")?;
    let key_material =
        key_material.context("selected MoogleMail diagnostic parent key is unavailable")?;
    let data_key = DataKey::from_base64(key_id, key_material)
        .context("decode selected MoogleMail diagnostic parent key")?;
    let archive = MoogleMailDiagnosticArchive::new(
        MoogleMailDiagnosticArchiveConfig {
            directory: diagnostics.directory.clone(),
            retention: Duration::from_secs(u64::from(diagnostics.retention_days) * 86_400),
            maximum_total_bytes: diagnostics.maximum_total_bytes,
            key_source: key_source.to_owned(),
        },
        Arc::new(data_key),
    )
    .map_err(|error| anyhow::anyhow!("open MoogleMail diagnostic archive: {}", error.as_str()))?;
    let message = archive.read_parsed_message(evidence_id).map_err(|error| {
        anyhow::anyhow!("read authenticated parsed_message evidence: {}", error.as_str())
    })?;
    let message_display = MailDisplaySnapshot::from_message(&message)
        .context("authenticated parsed_message cannot produce safe display projection")?;
    let database_url = config.database_url().context("database_url is required")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("mail-evidence backfill-display 只支持 PostgreSQL 完整档");
    }
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for MoogleMail display backfill")?;
    let result = store
        .backfill_mail_display(MailDisplayBackfillInput {
            mailbox_key: message.mailbox_key.clone(),
            world_id: message.world_id.clone(),
            external_mail_id: message.external_mail_id.clone(),
            content_digest: message.content_digest,
            sender: message.sender.clone(),
            subject: message.subject.clone(),
            message_display,
            evidence_id,
        })
        .await
        .context("backfill MoogleMail display projection")?;
    println!(
        "mail display backfill: mailbox_key={} world_id={} external_mail_id={} status={} evidence_id={}",
        message.mailbox_key,
        message.world_id,
        message.external_mail_id,
        if result.updated { "updated" } else { "already_present" },
        evidence_id
    );
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(in super::super) async fn backfill_display(
    _evidence_id: Uuid,
    _requested_key_source: Option<MailEvidenceKeySourceArg>,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "mail-evidence backfill-display 需要 PostgreSQL 完整档；请使用 --features postgres"
    )
}
