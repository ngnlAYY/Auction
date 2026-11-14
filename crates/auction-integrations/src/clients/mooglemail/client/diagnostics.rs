//! HTTP 与解析失败的加密证据接线；保留原错误的重试和写入未知语义。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use auction_application::mail::{
    MailDiagnosticContext, MailDiagnosticOutcome, MailDiagnosticStage,
};

use super::*;

impl MoogleMailClient {
    /// 解析失败时仍持有原始响应，在释放该响应前留存加密证据。
    pub(crate) async fn diagnose_parse<T>(
        &self,
        target: &MoogleMailTarget,
        document: &[u8],
        result: Result<T, MoogleMailError>,
    ) -> Result<T, MoogleMailError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "parse_response",
                    safe_diagnostics::project(&error),
                    DiagnosticContext::default(),
                );
                Err(self
                    .record_diagnostic(
                        MailDiagnosticStage::Parse,
                        target,
                        // 成功读取接口只返回正文，解析层不能伪造已经丢失的 HTTP 状态码。
                        None,
                        Some(document),
                        error,
                    )
                    .await)
            }
        }
    }

    pub(super) async fn diagnose_http(
        &self,
        target: &MoogleMailTarget,
        status: Option<u16>,
        body: Option<&[u8]>,
        error: MoogleMailError,
    ) -> MoogleMailError {
        self.record_diagnostic(MailDiagnosticStage::Transport, target, status, body, error).await
    }

    async fn record_diagnostic(
        &self,
        stage: MailDiagnosticStage,
        target: &MoogleMailTarget,
        status: Option<u16>,
        body: Option<&[u8]>,
        mut error: MoogleMailError,
    ) -> MoogleMailError {
        let Some(recorder) = &self.diagnostics else {
            error.evidence_status = "disabled";
            return error;
        };
        // 同一忙碌期只保存首份战斗页：探测或其他重复命中不再占用诊断容量。
        // 只跳过账号忙证据，write_unknown 等其他类别的保全不受影响。
        if error.category == MoogleMailErrorCategory::AccountBusy
            && !self.account_busy.retain_busy_evidence(error.world)
        {
            error.evidence_status = "suppressed";
            return error;
        }
        // 故障期间同类只读失败只留几份样本，避免同一故障页占满归档并挤掉其他证据；
        // 写入阶段、写入未知与身份类证据不受配额限制。
        if !self.evidence_budget.permits(stage, &error, std::time::Instant::now()) {
            error.evidence_status = "suppressed";
            return error;
        }
        // typed target 只接受受限 ID；不能记录 URL，其路径和查询可能包含装备 key。
        let external_mail_id = target
            .url
            .query_pairs()
            .find(|(name, _)| name == "mid")
            .map(|(_, value)| value.into_owned());
        let context = MailDiagnosticContext {
            stage,
            operation: error.operation.as_str().to_owned(),
            world_id: error.world.as_str().to_owned(),
            mailbox_key: None,
            external_mail_id,
            external_equipment_id: None,
            attachment_position: None,
        };
        let outcome = MailDiagnosticOutcome {
            failure_code: error.category.as_str().to_owned(),
            decision_code: None,
        };
        match time::timeout(
            Duration::from_secs(2),
            recorder.retain_response(context, status, body, outcome),
        )
        .await
        {
            Ok(Ok(receipt)) => {
                error.evidence_id = Some(receipt.evidence_id);
                error.evidence_status =
                    if receipt.body_available { "retained" } else { "body_unavailable" };
            }
            Ok(Err(failure)) => {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "retain_response",
                    safe_diagnostics::project(&failure),
                    DiagnosticContext::default(),
                );
                error.evidence_status = "save_failed";
                error.evidence_failure = Some(failure.as_str());
            }
            Err(diagnostic_cause) => {
                safe_diagnostics::warn(
                    "mooglemail_diagnostics",
                    "retain_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(error.world.as_str()),
                        ..Default::default()
                    },
                );

                error.evidence_status = "save_failed";
                error.evidence_failure = Some("diagnostic_timeout");
            }
        }
        error
    }
}
