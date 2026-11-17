//! 服务命令的回执发布、失败记录与运行中制品摘要。

use anyhow::Context;
use chrono::{DateTime, Utc};

use super::systemd::{ManagedRole, Systemctl};
use crate::cli::UnitArgs;
use crate::receipt;
use crate::receipt::{
    Action, Artifact, DeploymentReceipt, MigrationRecord, Outcome, ReceiptContext,
};
use crate::support::files::PrivateDirectory;

/// 成功（含 `already_running`）回执先于成功输出发布；写不进去就整体失败。`state` 是持锁时固定的
/// 部署状态目录，回执与锁属于同一棵树。
#[expect(
    clippy::too_many_arguments,
    reason = "回执字段逐项来自调用方，结构体包装只会复制同一组字段"
)]
pub(super) async fn publish_receipt(
    state: &PrivateDirectory,
    systemctl: &Systemctl,
    action: Action,
    outcome: Outcome,
    started_at: DateTime<Utc>,
    context: &ReceiptContext,
    migration: Option<MigrationRecord>,
    failure: Option<String>,
) -> anyhow::Result<DeploymentReceipt> {
    let artifacts = collect_artifacts(systemctl).await;
    receipt::record(state, action, outcome, started_at, context, migration, artifacts, failure)
        .context("写入部署回执失败")
}

/// 失败回执尽力追加；回执本身写不进去时把它附在原错误上，不掩盖部署失败原因。
/// `deploy`/`rollback` 在保留旧程序之前的失败也经此追加，与 service 命令同一口径。
pub(crate) async fn record_failure(
    state: &PrivateDirectory,
    systemctl: &Systemctl,
    action: Action,
    started_at: DateTime<Utc>,
    context: &ReceiptContext,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    let failure = receipt::failure_text(error);
    if let Err(record_error) = publish_receipt(
        state,
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

/// stdout 中回显的回执摘要：标识、文件路径与结果。
pub(crate) fn receipt_summary(units: &UnitArgs, receipt: &DeploymentReceipt) -> serde_json::Value {
    serde_json::json!({
        "receipt_id": receipt.receipt_id,
        "path": receipt::receipts_dir(&units.state_dir)
            .join(format!("{}.json", receipt.receipt_id)),
        "outcome": receipt.outcome,
    })
}
