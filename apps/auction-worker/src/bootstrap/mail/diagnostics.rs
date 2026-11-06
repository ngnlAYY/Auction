//! 诊断归档与结构化日志装配；只记录安全回执，不输出敏感载荷。

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use auction_application::mail::{
    MailDiagnosticContext, MailDiagnosticError, MailDiagnosticFuture, MailDiagnosticOutcome,
    MailDiagnosticReceipt, MailDiagnosticRecorder, MailDiagnosticStage, MailMessage,
    PreparedMailEvidence,
};
use auction_config::AppConfig;
use auction_integrations::{
    build_keyring,
    mooglemail::{MoogleMailDiagnosticArchive, MoogleMailDiagnosticArchiveConfig},
};

pub(super) fn configure(
    config: &AppConfig,
) -> anyhow::Result<Option<Arc<dyn MailDiagnosticRecorder>>> {
    let diagnostics = &config.worker.mooglemail.diagnostics;
    if !diagnostics.enabled {
        tracing::warn!("MoogleMail diagnostic evidence retention disabled by configuration");
        return Ok(None);
    }
    // 独立诊断域优先；历史部署仍可沿用 credential/equipment 父密钥，新的写入会随配置切换。
    let domains = [
        auction_config::DataKeyDomain::MailDiagnostics,
        auction_config::DataKeyDomain::Credential,
        auction_config::DataKeyDomain::Equipment,
    ];
    let mut selected = None;
    for domain in domains {
        let keyring = build_keyring(
            domain,
            config.data_key_current(domain),
            config.data_key_previous(domain).iter().map(|key| (key.id(), key.material())),
        )
        .map_err(|_| anyhow::anyhow!("MoogleMail 诊断父密钥格式无效"))?;
        if let Some(keyring) = keyring {
            selected = Some(keyring);
            break;
        }
    }
    let keyring = selected.context(
        "MoogleMail 诊断启用时需要 mail_diagnostics_data_key、credential_data_key 或 equipment_data_key 父密钥",
    )?;
    let key_source = keyring.domain().as_str();
    let archive = MoogleMailDiagnosticArchive::new(
        MoogleMailDiagnosticArchiveConfig {
            directory: diagnostics.directory.clone(),
            retention: Duration::from_secs(u64::from(diagnostics.retention_days) * 86_400),
            maximum_total_bytes: diagnostics.maximum_total_bytes,
            key_source: key_source.to_owned(),
        },
        keyring,
    )
    .map_err(|error| anyhow::anyhow!("MoogleMail 诊断配置无效：{}", error.as_str()))?;
    tracing::info!(
        directory = %diagnostics.directory.display(),
        retention_days = diagnostics.retention_days,
        maximum_total_bytes = diagnostics.maximum_total_bytes,
        key_source,
        "MoogleMail encrypted diagnostic evidence configured"
    );
    Ok(Some(Arc::new(LoggedMailDiagnostics { archive })))
}

struct LoggedMailDiagnostics {
    archive: MoogleMailDiagnosticArchive,
}

/// 安全失败日志独立于原始留证开关，装配根始终将这个出口传给装备来源适配器。
pub(super) fn report_equipment_failure(
    context: &auction_application::mail::MailEquipmentDiagnosticContext,
    snapshot: &auction_application::mail::MailEquipmentDiagnosticSnapshot,
) {
    tracing::warn!(
        mailbox_key = %context.mailbox_key,
        world_id = %context.world_id,
        external_mail_id = %context.external_mail_id,
        external_equipment_id = %snapshot.equipment_id,
        attachment_position = snapshot.attachment_position,
        stage = snapshot.stage.as_str(),
        category = snapshot.category.as_str(),
        detail_code = ?snapshot.detail_code,
        http_status = ?snapshot.http_status,
        elapsed_millis = ?snapshot.elapsed_millis,
        evidence_id = ?snapshot.evidence_id,
        evidence_status = snapshot.evidence_status.as_str(),
        retention_error = ?snapshot.retention_error,
        "MoogleMail equipment processing failed"
    );
}

impl MailDiagnosticRecorder for LoggedMailDiagnostics {
    fn prepare_response(
        &self,
        context: MailDiagnosticContext,
        status: Option<u16>,
        body: Option<&[u8]>,
    ) -> Result<PreparedMailEvidence, MailDiagnosticError> {
        self.archive.prepare_response(context, status, body)
    }

    fn prepare_message(
        &self,
        context: MailDiagnosticContext,
        message: &MailMessage,
    ) -> Result<PreparedMailEvidence, MailDiagnosticError> {
        self.archive.prepare_message(context, message)
    }

    fn retain(
        &self,
        prepared: PreparedMailEvidence,
        outcome: MailDiagnosticOutcome,
    ) -> MailDiagnosticFuture<'_, MailDiagnosticReceipt> {
        let mut guard = RetentionReportGuard {
            context: prepared.context().clone(),
            outcome: outcome.clone(),
            evidence_id: Some(prepared.evidence_id()),
            reported: false,
        };
        let retained = self.archive.retain(prepared, outcome);
        Box::pin(async move {
            let result = retained.await;
            guard.complete(&result);
            result
        })
    }

    fn retain_response(
        &self,
        context: MailDiagnosticContext,
        status: Option<u16>,
        body: Option<&[u8]>,
        outcome: MailDiagnosticOutcome,
    ) -> MailDiagnosticFuture<'_, MailDiagnosticReceipt> {
        let mut guard = RetentionReportGuard {
            context: context.clone(),
            outcome: outcome.clone(),
            evidence_id: None,
            reported: false,
        };
        let retained = self.archive.retain_response(context, status, body, outcome);
        Box::pin(async move {
            let result = retained.await;
            guard.complete(&result);
            result
        })
    }
}

/// 租约取消或外层时限可能丢弃留证 future；明确报告未确认完成，不能静默当作已保存。
/// 已开始的本地阻塞任务仍受文件锁和容量约束完成，不再发起任何业务动作。
struct RetentionReportGuard {
    context: MailDiagnosticContext,
    outcome: MailDiagnosticOutcome,
    evidence_id: Option<uuid::Uuid>,
    reported: bool,
}

impl RetentionReportGuard {
    fn complete(&mut self, result: &Result<MailDiagnosticReceipt, MailDiagnosticError>) {
        self.reported = true;
        // 完成与取消共用同一份安全元数据，避免仅为正常日志再保留一组字符串副本。
        report(&self.context, &self.outcome, result);
    }
}

impl Drop for RetentionReportGuard {
    fn drop(&mut self) {
        if !self.reported {
            tracing::warn!(
                stage = stage_name(self.context.stage),
                world_id = %self.context.world_id,
                operation = %self.context.operation,
                mailbox_key = ?self.context.mailbox_key,
                external_mail_id = ?self.context.external_mail_id,
                external_equipment_id = ?self.context.external_equipment_id,
                attachment_position = ?self.context.attachment_position,
                failure_code = %self.outcome.failure_code,
                evidence_id = ?self.evidence_id,
                evidence_status = "completion_unconfirmed",
                diagnostic_error = "retention_cancelled",
                "MoogleMail diagnostic retention completion unconfirmed"
            );
        }
    }
}

fn report(
    context: &MailDiagnosticContext,
    outcome: &MailDiagnosticOutcome,
    result: &Result<MailDiagnosticReceipt, MailDiagnosticError>,
) {
    let stage = stage_name(context.stage);
    match result {
        Ok(receipt) => tracing::warn!(
            stage,
            operation = %context.operation,
            world_id = %context.world_id,
            mailbox_key = ?context.mailbox_key,
            external_mail_id = ?context.external_mail_id,
            external_equipment_id = ?context.external_equipment_id,
            attachment_position = ?context.attachment_position,
            failure_code = %outcome.failure_code,
            decision_code = ?outcome.decision_code,
            evidence_id = %receipt.evidence_id,
            plaintext_bytes = receipt.plaintext_bytes,
            sha256 = %receipt.sha256,
            body_available = receipt.body_available,
            evidence_status = if receipt.body_available { "retained" } else { "body_unavailable" },
            "MoogleMail diagnostic evidence retained"
        ),
        Err(error) => tracing::warn!(
            stage,
            operation = %context.operation,
            world_id = %context.world_id,
            mailbox_key = ?context.mailbox_key,
            external_mail_id = ?context.external_mail_id,
            external_equipment_id = ?context.external_equipment_id,
            attachment_position = ?context.attachment_position,
            failure_code = %outcome.failure_code,
            evidence_status = "save_failed",
            diagnostic_error = error.as_str(),
            "MoogleMail diagnostic evidence retention failed"
        ),
    }
}

fn stage_name(stage: MailDiagnosticStage) -> &'static str {
    match stage {
        MailDiagnosticStage::Transport => "transport",
        MailDiagnosticStage::Parse => "parse",
        MailDiagnosticStage::Business => "business",
    }
}
