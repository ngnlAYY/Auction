//! MoogleMail 运维证据：加密诊断归档的离线导出，以及撤拍退回义务的 Sent 只读核查。

use std::{path::Path, time::Duration};

use anyhow::Context;
use auction_application::operations::{
    WORKER_REMOTE_READ_TIMEOUT_SECONDS, WorkerRemoteReadError, WorkerRemoteReadPort,
    WorkerSentLookupRequest,
};
use auction_application::settlement::{
    SettlementFulfillmentSentLookupReader, canonical_settlement_fulfillment_subject,
};
use auction_config::AppConfig;
use auction_domain::{SettlementId, SettlementObligationKind};
use auction_integrations::{build_keyring, mooglemail::MoogleMailDiagnosticArchiveConfig};
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
    let domains = match key_source {
        Some(source) => vec![source.domain()],
        None => vec![
            auction_config::DataKeyDomain::MailDiagnostics,
            auction_config::DataKeyDomain::Credential,
            auction_config::DataKeyDomain::Equipment,
        ],
    };
    // 历史归档保留在写入时的域；头部 key_source 决定使用哪个域的密钥环。
    let keyrings = domains
        .into_iter()
        .filter_map(|domain| {
            build_keyring(
                domain,
                config.data_key_current(domain),
                config.data_key_previous(domain).iter().map(|key| (key.id(), key.material())),
            )
            .ok()
            .flatten()
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(!keyrings.is_empty(), "所选父密钥未配置");
    let diagnostics = &config.worker.mooglemail.diagnostics;
    auction_integrations::mooglemail::export_mooglemail_diagnostic(
        MoogleMailDiagnosticArchiveConfig {
            directory: diagnostics.directory.clone(),
            retention: Duration::from_secs(u64::from(diagnostics.retention_days) * 86_400),
            maximum_total_bytes: diagnostics.maximum_total_bytes,
            key_source: String::new(),
        },
        keyrings,
        evidence_id,
        output.to_owned(),
    )
    .map_err(|error| anyhow::anyhow!("导出诊断证据失败：{}", error.as_str()))?;
    print_json(&json!({ "evidence_id": evidence_id, "output": output }))
}

/// 冻结目标须为单件装备、零 Credits/COD；Sent 匹配只证明发送记录，collected 不等于已送达。
/// W1.10：远端 Sent 只读扫描改由 Worker control 执行，命令行不再持有凭据或第二个限速器；
/// Worker 不可用或外部访问被暂停时明确失败，不回退到本地直连。
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
    let subject = canonical_settlement_fulfillment_subject(
        SettlementObligationKind::WithdrawnReturn,
        SettlementId::from_uuid(obligation_id),
    )
    .context("无法生成履约邮件标题")?;
    let client = auction_integrations::WorkerRemoteReadClient::new(
        config.worker_control_socket_path(),
        std::time::Duration::from_secs(WORKER_REMOTE_READ_TIMEOUT_SECONDS),
    )
    .map_err(|error| anyhow::anyhow!("Worker control client 不可用：{error}"))?;
    let request = WorkerSentLookupRequest {
        world: target.world.as_str().to_owned(),
        recipient: target.recipient.clone(),
        subject: subject.clone(),
        external_equipment_id: target.external_equipment_id.clone(),
    };
    let report = client.lookup_sent(request).await.map_err(remote_read_error)?;
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
        "status": report.status,
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

/// Worker 侧稳定错误转为可执行提示；暂停与能力不可用不得重试成直连。
fn remote_read_error(error: WorkerRemoteReadError) -> anyhow::Error {
    match error {
        WorkerRemoteReadError::Paused => anyhow::anyhow!(
            "外部访问已暂停（external_access_paused）；请等待恢复或到期后重试，不会回退到直连"
        ),
        WorkerRemoteReadError::NotPaused => {
            anyhow::anyhow!("指定能力要求外部访问处于指定状态；当前状态不满足，未发往远端")
        }
        WorkerRemoteReadError::Unavailable => anyhow::anyhow!(
            "Worker control 不可用；Sent 远端读取能力不可用，请先确认 Worker 与 control socket"
        ),
        WorkerRemoteReadError::Timeout => anyhow::anyhow!("Worker Sent 核查超时；未取得结论"),
        WorkerRemoteReadError::Incompatible => anyhow::anyhow!("Worker control 响应不受支持"),
        WorkerRemoteReadError::Invalid => anyhow::anyhow!("Sent 核查请求无效"),
    }
}
