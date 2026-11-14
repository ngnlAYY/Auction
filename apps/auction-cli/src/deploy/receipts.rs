//! 部署回执与失败记录。

use chrono::Utc;

use super::artifact;
use crate::receipt;
use crate::receipt::{Action, DeploymentReceipt, Outcome, ReceiptContext};
use crate::service::{ManagedRole, Systemctl};

pub(crate) async fn record_failure(
    units: &crate::cli::UnitArgs,
    systemctl: &Systemctl,
    action: Action,
    started_at: chrono::DateTime<Utc>,
    context: &ReceiptContext,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    let failure = receipt::failure_text(error);
    let mut artifacts = Vec::new();
    for role in ManagedRole::START_ORDER {
        let Ok(state) = systemctl.show(role).await else { continue };
        if state.running() {
            artifacts.push(receipt::artifact_from_process(role.name(), state.main_pid));
        }
    }
    if let Err(record_error) = receipt::record(
        &units.state_dir,
        action,
        Outcome::Failed,
        started_at,
        context,
        None,
        artifacts,
        Some(failure),
    ) {
        return Err(anyhow::anyhow!("{error:#}；部署回执也未写入：{record_error:#}"));
    }
    Ok(())
}

pub(crate) fn receipt_summary(
    units: &crate::cli::UnitArgs,
    receipt: &DeploymentReceipt,
) -> serde_json::Value {
    serde_json::json!({
        "receipt_id": receipt.receipt_id,
        "path": receipt::receipts_dir(&units.state_dir)
            .join(format!("{}.json", receipt.receipt_id)),
        "outcome": receipt.outcome,
    })
}

pub(crate) fn artifact_projection(artifact: &artifact::Artifact) -> serde_json::Value {
    serde_json::json!({
        "directory": artifact.root,
        "package": artifact.package,
        "source_commit": artifact.source_commit,
        "source_fingerprint": artifact.source_fingerprint,
        "binaries": artifact.binaries.iter().map(|binary| serde_json::json!({
            "name": binary.name,
            "sha256": binary.sha256,
            "bytes": binary.bytes,
        })).collect::<Vec<_>>(),
        "public_payload_sha256": artifact.public_payload_sha256,
        "admin_payload_sha256": artifact.admin_payload_sha256,
    })
}

/// 最近一次成功部署的源码提交；用于回滚记录的可追溯性，缺失时不伪造。
pub(super) fn previous_source_commit(units: &crate::cli::UnitArgs) -> Option<String> {
    let receipts = receipt::list(&units.state_dir, 20).ok()?;
    receipts
        .into_iter()
        .find(|receipt| receipt.outcome == Outcome::Succeeded)
        .and_then(|receipt| receipt.deploy)
        .map(|deploy| deploy.source_commit)
}
