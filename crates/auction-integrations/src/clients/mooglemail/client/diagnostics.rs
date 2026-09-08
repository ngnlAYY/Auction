//! HTTP 与解析失败的加密证据接线；保留原错误的重试和写入未知语义。

use auction_application::{MailDiagnosticContext, MailDiagnosticOutcome, MailDiagnosticStage};

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
            Err(error) => Err(self
                .record_diagnostic(
                    MailDiagnosticStage::Parse,
                    target,
                    // 成功读取接口只返回正文，解析层不能伪造已经丢失的 HTTP 状态码。
                    None,
                    Some(document),
                    error,
                )
                .await),
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
                error.evidence_status = "save_failed";
                error.evidence_failure = Some(failure.as_str());
            }
            Err(_) => {
                error.evidence_status = "save_failed";
                error.evidence_failure = Some("diagnostic_timeout");
            }
        }
        error
    }
}
