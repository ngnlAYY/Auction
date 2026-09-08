//! MoogleMail 证据导出、认证展示回填和只读 Sent 核查命令。

mod backfill;
mod sent;

pub(super) use backfill::backfill_display;
pub(super) use sent::inspect_sent;

use super::cli::MailEvidenceKeySourceArg;
use std::path::Path;
use uuid::Uuid;

#[cfg(feature = "postgres")]
pub(super) fn export(
    evidence_id: Uuid,
    output: &Path,
    requested_key_source: Option<MailEvidenceKeySourceArg>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use auction_config::Role;
    use auction_integrations::{
        DataKey,
        mooglemail::{MoogleMailDiagnosticArchiveConfig, export_mooglemail_diagnostic},
    };
    use std::sync::Arc;
    use std::time::Duration;

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
    let archive_config = MoogleMailDiagnosticArchiveConfig {
        directory: diagnostics.directory.clone(),
        retention: Duration::from_secs(u64::from(diagnostics.retention_days) * 86_400),
        maximum_total_bytes: diagnostics.maximum_total_bytes,
        key_source: key_source.to_owned(),
    };
    export_mooglemail_diagnostic(
        archive_config,
        Arc::new(data_key),
        evidence_id,
        output.to_owned(),
    )
    .map_err(|error| {
        anyhow::anyhow!("export MoogleMail diagnostic evidence: {}", error.as_str())
    })?;
    println!("mail evidence exported: evidence_id={evidence_id} output={}", output.display());
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(super) fn export(
    _evidence_id: Uuid,
    _output: &Path,
    _requested_key_source: Option<MailEvidenceKeySourceArg>,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "mail-evidence export requires the auctionctl postgres feature; MoogleMail diagnostics are PostgreSQL complete-draft only"
    )
}
