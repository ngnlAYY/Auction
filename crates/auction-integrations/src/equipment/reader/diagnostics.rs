//! 装备读取的敏感证据准备与留存；不参与装备业务决定。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use super::*;

pub(super) fn prepare_observation(
    adapter: &EquipmentSourceAdapter,
    context: MailEquipmentDiagnosticContext,
    stage: MailEquipmentDiagnosticStage,
    category: MailEquipmentDiagnosticCategory,
    status: Option<u16>,
    elapsed: Option<u64>,
    mut body: Option<Vec<u8>>,
) -> MailEquipmentObservation {
    let Some(recorder) = adapter.diagnostics.as_ref() else {
        if let Some(bytes) = body.as_mut() {
            bytes.fill(0);
        }
        return MailEquipmentObservation {
            context,
            stage,
            category,
            source_detail_code: None,
            http_status: status,
            elapsed_millis: elapsed,
            prepared: None,
            preparation_status: MailEquipmentEvidenceStatus::Disabled,
            preparation_error: None,
        };
    };
    let prepared =
        recorder.prepare_response(diagnostic_context(&context, stage), status, body.as_deref());
    if let Some(bytes) = body.as_mut() {
        bytes.fill(0);
    }
    match prepared {
        Ok(prepared) => MailEquipmentObservation {
            context,
            stage,
            category,
            source_detail_code: None,
            http_status: status,
            elapsed_millis: elapsed,
            prepared: Some(prepared),
            preparation_status: MailEquipmentEvidenceStatus::Prepared,
            preparation_error: None,
        },
        Err(error) => MailEquipmentObservation {
            context,
            stage,
            category,
            source_detail_code: None,
            http_status: status,
            elapsed_millis: elapsed,
            prepared: None,
            preparation_status: MailEquipmentEvidenceStatus::PrepareFailed,
            preparation_error: Some(error.as_str().to_owned()),
        },
    }
}

fn diagnostic_context(
    context: &MailEquipmentDiagnosticContext,
    stage: MailEquipmentDiagnosticStage,
) -> MailDiagnosticContext {
    MailDiagnosticContext {
        stage: match stage {
            // 归档描述响应捕获阶段；后续验证失败阶段由独立诊断摘要和最终日志表达。
            MailEquipmentDiagnosticStage::Parse | MailEquipmentDiagnosticStage::Validate => {
                MailDiagnosticStage::Parse
            }
            MailEquipmentDiagnosticStage::Request | MailEquipmentDiagnosticStage::Response => {
                MailDiagnosticStage::Transport
            }
        },
        operation: "equipment_read".to_owned(),
        world_id: context.world_id.clone(),
        mailbox_key: Some(context.mailbox_key.clone()),
        external_mail_id: Some(context.external_mail_id.clone()),
        external_equipment_id: Some(context.external_equipment_id.clone()),
        attachment_position: Some(context.attachment_position),
    }
}

pub(super) async fn retain_failure(
    recorder: Option<Arc<dyn MailDiagnosticRecorder>>,
    reporter: Option<Arc<DiagnosticReporter>>,
    observation: MailEquipmentObservation,
    stage: MailEquipmentDiagnosticStage,
    category: MailEquipmentDiagnosticCategory,
    detail_code: Option<String>,
) -> MailEquipmentDiagnosticSnapshot {
    // 来源码只能由闭集枚举提供，且仅可落到真正的页面解析失败；其它阶段保留原诊断语义。
    let source_detail = if stage == MailEquipmentDiagnosticStage::Parse
        && category == MailEquipmentDiagnosticCategory::ProtocolDrift
    {
        observation.source_detail_code.map(|reason| reason.as_str().to_owned())
    } else {
        None
    };
    let detail_code = detail_code.or(source_detail);
    let evidence_id = observation.prepared.as_ref().map(PreparedMailEvidence::evidence_id);
    let mut snapshot = base_snapshot(&observation, stage, category, detail_code.clone());
    let Some(prepared) = observation.prepared else {
        snapshot.evidence_status =
            if observation.preparation_status == MailEquipmentEvidenceStatus::Disabled {
                MailEquipmentEvidenceStatus::Disabled
            } else {
                MailEquipmentEvidenceStatus::PrepareFailed
            };
        snapshot.retention_error =
            if snapshot.evidence_status == MailEquipmentEvidenceStatus::Disabled {
                None
            } else {
                observation.preparation_error.or_else(|| Some("invalid_context".to_owned()))
            };
        return report(reporter.as_ref(), &observation.context, snapshot);
    };
    let Some(recorder) = recorder else {
        snapshot.evidence_status = if evidence_id.is_some() {
            MailEquipmentEvidenceStatus::SaveFailed
        } else {
            MailEquipmentEvidenceStatus::Disabled
        };
        snapshot.evidence_id = evidence_id;
        snapshot.retention_error = evidence_id.map(|_| "recorder_unavailable".to_owned());
        return report(reporter.as_ref(), &observation.context, snapshot);
    };
    let outcome = MailDiagnosticOutcome {
        failure_code: category.as_str().to_owned(),
        decision_code: detail_code.clone(),
    };
    let retained =
        tokio::time::timeout(std::time::Duration::from_secs(2), recorder.retain(prepared, outcome))
            .await;
    let (status, retained_id, error) = match retained {
        Ok(Ok(receipt)) => (
            if receipt.body_available {
                MailEquipmentEvidenceStatus::Retained
            } else {
                MailEquipmentEvidenceStatus::BodyUnavailable
            },
            Some(receipt.evidence_id),
            None,
        ),
        Ok(Err(error)) => {
            (MailEquipmentEvidenceStatus::SaveFailed, evidence_id, Some(error.as_str().to_owned()))
        }
        Err(diagnostic_cause) => {
            safe_diagnostics::warn(
                "equipment_diagnostics",
                "retain_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            (
                MailEquipmentEvidenceStatus::CompletionUnconfirmed,
                evidence_id,
                Some("retention_timeout".to_owned()),
            )
        }
    };
    snapshot.evidence_id = retained_id;
    snapshot.evidence_status = status;
    snapshot.retention_error = error;
    report(reporter.as_ref(), &observation.context, snapshot)
}

/// 仅记录固定来源与闭集原因，不把原名称、header、页面、URL 或 key 拼入错误和日志。
pub(super) fn source_parse_error(
    source_kind: &'static str,
    reason: EquipmentParseError,
) -> AdminIntakeError {
    safe_diagnostics::warn(
        "equipment_fetch",
        match source_kind {
            "html" => "parse_html_equipment",
            _ => "parse_json_equipment",
        },
        safe_diagnostics::project(&reason),
        DiagnosticContext::default(),
    );
    AdminIntakeError::InvalidSource
}

fn report(
    reporter: Option<&Arc<DiagnosticReporter>>,
    context: &MailEquipmentDiagnosticContext,
    snapshot: MailEquipmentDiagnosticSnapshot,
) -> MailEquipmentDiagnosticSnapshot {
    if let Some(reporter) = reporter {
        reporter(context, &snapshot);
    }
    snapshot
}

fn base_snapshot(
    observation: &MailEquipmentObservation,
    stage: MailEquipmentDiagnosticStage,
    category: MailEquipmentDiagnosticCategory,
    detail_code: Option<String>,
) -> MailEquipmentDiagnosticSnapshot {
    MailEquipmentDiagnosticSnapshot {
        schema_version: 1,
        equipment_id: observation.context.external_equipment_id.clone(),
        attachment_position: observation.context.attachment_position,
        stage,
        category,
        detail_code,
        http_status: observation.http_status,
        elapsed_millis: observation.elapsed_millis,
        evidence_id: None,
        evidence_status: MailEquipmentEvidenceStatus::PrepareFailed,
        retention_error: None,
    }
}
