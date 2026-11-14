//! 服务命令的回执发布、失败记录与运行中制品摘要。

use anyhow::Context;
use chrono::{DateTime, Utc};

use super::systemd::{ManagedRole, Systemctl};
use crate::cli::UnitArgs;
use crate::receipt;
use crate::receipt::{
    Action, Artifact, DeploymentReceipt, MigrationRecord, Outcome, ReceiptContext,
};

/// 成功（含 `already_running`）回执先于成功输出发布；写不进去就整体失败。
#[allow(clippy::too_many_arguments)]
pub(super) async fn publish_receipt(
    units: &UnitArgs,
    systemctl: &Systemctl,
    action: Action,
    outcome: Outcome,
    started_at: DateTime<Utc>,
    context: &ReceiptContext,
    migration: Option<MigrationRecord>,
    failure: Option<String>,
) -> anyhow::Result<DeploymentReceipt> {
    let artifacts = collect_artifacts(systemctl).await;
    receipt::record(
        &units.state_dir,
        action,
        outcome,
        started_at,
        context,
        migration,
        artifacts,
        failure,
    )
    .context("写入部署回执失败")
}

/// 失败回执尽力追加；回执本身写不进去时把它附在原错误上，不掩盖部署失败原因。
pub(super) async fn record_failure(
    units: &UnitArgs,
    systemctl: &Systemctl,
    action: Action,
    started_at: DateTime<Utc>,
    context: &ReceiptContext,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    let failure = receipt::failure_text(error);
    if let Err(record_error) = publish_receipt(
        units,
        systemctl,
        action,
        Outcome::Failed,
        started_at,
        context,
        None,
        Some(failure),
    )
    .await
    {
        return Err(anyhow::anyhow!("{error:#}；部署回执也未写入：{record_error:#}"));
    }
    Ok(())
}

/// 只把实际运行的三个角色进程作为程序证据；停止或不可读的角色不伪造摘要。
pub(super) async fn collect_artifacts(systemctl: &Systemctl) -> Vec<Artifact> {
    let mut artifacts = Vec::with_capacity(3);
    for role in ManagedRole::START_ORDER {
        let Ok(state) = systemctl.show(role).await else { continue };
        if state.running() {
            artifacts.push(receipt::artifact_from_process(role.name(), state.main_pid));
        }
    }
    artifacts
}

pub(super) fn receipt_summary(units: &UnitArgs, receipt: &DeploymentReceipt) -> serde_json::Value {
    serde_json::json!({
        "receipt_id": receipt.receipt_id,
        "path": receipt::receipts_dir(&units.state_dir)
            .join(format!("{}.json", receipt.receipt_id)),
        "outcome": receipt.outcome,
    })
}
