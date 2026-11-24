//! 单封邮件异常的加密留证与安全日志；原始业务结果不依赖证据保存成功。

use std::{sync::Arc, time::Duration};

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::mail::{
    MailDiagnosticContext, MailDiagnosticOutcome, MailDiagnosticReceipt, MailDiagnosticRecorder,
    MailDiagnosticStage, MailMessage, MailProcessingDecision, PreparedMailEvidence,
};

use super::business::MailProcessFailure;
use crate::worker::diagnostics;

pub(super) struct MailMessageDiagnostic {
    context: MailDiagnosticContext,
    recorder: Option<Arc<dyn MailDiagnosticRecorder>>,
    prepared: Option<PreparedMailEvidence>,
    unavailable: &'static str,
}

impl MailMessageDiagnostic {
    pub(super) fn prepare(
        recorder: Option<Arc<dyn MailDiagnosticRecorder>>,
        message: &MailMessage,
    ) -> Self {
        let context = MailDiagnosticContext {
            stage: MailDiagnosticStage::Business,
            operation: "process".to_owned(),
            world_id: message.world_id.clone(),
            mailbox_key: Some(message.mailbox_key.clone()),
            external_mail_id: Some(message.external_mail_id.clone()),
            external_equipment_id: None,
            attachment_position: None,
        };
        let (prepared, unavailable) = match &recorder {
            Some(recorder) => match recorder.prepare_message(context.clone(), message) {
                Ok(prepared) => (Some(prepared), ""),
                Err(error) => {
                    diagnostics::unpersisted(error.diagnostic_kind());
                    tracing::warn!(
                        world_id = %context.world_id,
                        mailbox_key = ?context.mailbox_key,
                        external_mail_id = ?context.external_mail_id,
                        evidence_status = "prepare_failed",
                        diagnostic_error = error.as_str(),
                        "MoogleMail diagnostic preparation failed"
                    );
                    (None, "prepare_failed")
                }
            },
            None => (None, "disabled"),
        };
        Self { context, recorder, prepared, unavailable }
    }

    pub(super) async fn retain_failure(self, failure_code: &'static str) {
        tracing::warn!(
            world_id = %self.context.world_id,
            mailbox_key = ?self.context.mailbox_key,
            external_mail_id = ?self.context.external_mail_id,
            failure_code,
            "MoogleMail business processing failed"
        );
        let _ = self
            .retain(MailDiagnosticOutcome {
                failure_code: failure_code.to_owned(),
                decision_code: None,
            })
            .await;
    }

    pub(super) async fn retain_decision(
        self,
        decision: MailProcessingDecision,
    ) -> MailProcessingDecision {
        if decision.status == "accepted" {
            return decision;
        }
        // 只读取固定诊断字段，禁止将整个业务 metadata 或正文写入普通日志。
        let reason_code = decision
            .metadata
            .get("reason_code")
            .or_else(|| decision.metadata.get("owner_result_code"))
            .and_then(|value| value.as_str())
            .filter(|value| safe_code(value))
            .unwrap_or("business_rejected");
        let pricing = decision.metadata.get("pricing_diagnostic");
        let detail_code = pricing
            .and_then(|value| value.get("detail_code"))
            .and_then(|value| value.as_str())
            .filter(|value| safe_code(value));
        tracing::warn!(
            world_id = %self.context.world_id,
            mailbox_key = ?self.context.mailbox_key,
            external_mail_id = ?self.context.external_mail_id,
            decision_code = %decision.decision_code,
            reason_code,
            detail_code,
            body_bytes = ?pricing.and_then(|value| value.get("body_bytes")).and_then(serde_json::Value::as_u64),
            token_count = ?pricing.and_then(|value| value.get("token_count")).and_then(serde_json::Value::as_u64),
            field_index = ?pricing.and_then(|value| value.get("field_index")).and_then(serde_json::Value::as_u64),
            attachment_count = ?decision.metadata.get("attachment_count").and_then(serde_json::Value::as_u64),
            "MoogleMail business decision rejected"
        );
        let outcome = MailDiagnosticOutcome {
            failure_code: reason_code.to_owned(),
            decision_code: Some(decision.decision_code.clone()),
        };
        // 证据 UUID 每次取证都不同，不能进入参与重放比较的 decision_metadata。
        // 通过日志中的 mailbox/world/mail ID 关联证据，保持原有入站幂等比较不变。
        let _ = self.retain(outcome).await;
        decision
    }

    async fn retain(self, outcome: MailDiagnosticOutcome) -> Option<MailDiagnosticReceipt> {
        let Self { context, recorder, prepared, unavailable } = self;
        let (Some(recorder), Some(prepared)) = (recorder, prepared) else {
            tracing::warn!(
                world_id = %context.world_id,
                external_mail_id = ?context.external_mail_id,
                evidence_status = unavailable,
                "MoogleMail diagnostic evidence unavailable"
            );
            return None;
        };
        match tokio::time::timeout(Duration::from_secs(2), recorder.retain(prepared, outcome)).await
        {
            Ok(Ok(receipt)) => Some(receipt),
            Ok(Err(error)) => {
                tracing::warn!(
                    world_id = %context.world_id,
                    external_mail_id = ?context.external_mail_id,
                    evidence_status = "save_failed",
                    diagnostic_error = error.as_str(),
                    "MoogleMail diagnostic evidence unavailable"
                );
                None
            }
            Err(error) => {
                diagnostics::unpersisted_error(
                    "mail_poll",
                    "retain_diagnostic_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                None
            }
        }
    }
}

fn safe_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// 旧路由决定与候选路径共用的结果收口：留证、失败关闭与最终决定留档只在这里定义一次。
///
/// 先解析已冻结的装备诊断，再按决定闭集校验；校验失败以 `invalid_business_decision` 失败关闭，
/// 不把损坏决定写进入站事实。
pub(super) async fn finalize_legacy_result(
    diagnostic: MailMessageDiagnostic,
    mut result: Result<MailProcessingDecision, MailProcessFailure>,
    pending_diagnostics: Vec<auction_application::mail::PendingMailEquipmentDiagnostic>,
    decision_diagnostic: Option<usize>,
) -> Result<MailProcessingDecision, MailProcessFailure> {
    for (index, pending) in pending_diagnostics.into_iter().enumerate() {
        let resolved = pending.resolve().await;
        if decision_diagnostic == Some(index)
            && let Ok(decision) = &mut result
            && decision.mail_kind == "equipment"
            && decision.status == "rejected"
            && resolved.validate()
        {
            decision.equipment_diagnostic = Some(resolved);
        }
    }
    match result {
        Ok(decision) => {
            if decision.validate().is_err() {
                diagnostic.retain_failure("invalid_business_decision").await;
                return Err(MailProcessFailure {
                    failure_code: "invalid_business_decision",
                    safe_detail: "MoogleMail 业务处理器返回了无效决定",
                });
            }
            Ok(diagnostic.retain_decision(decision).await)
        }
        Err(failure) => {
            diagnostic.retain_failure(failure.failure_code).await;
            Err(failure)
        }
    }
}
