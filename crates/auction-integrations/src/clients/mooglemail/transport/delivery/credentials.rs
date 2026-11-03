//! MoogleMail delivery 的单 Cookie 会话与写入结果未知边界。

use super::*;

/// 完整排除集或临时发送前边界与原独占会话跨越持久 fence；上下文只允许消费一次。
pub(super) struct PreparedMoogleMailCredentials {
    pub(super) session: MoogleMailCredentialSession,
    pub(super) baseline: Option<MoogleMailSentBaseline>,
}

impl MoogleMailDeliveryTransport {
    /// 只读检查时取得 guard，并把它交给消耗式 prepared handle，绝不在 fence 间隙释放。
    pub(super) async fn prepare_credentials(
        &self,
        world: MoogleMailWorld,
    ) -> Result<PreparedMoogleMailCredentials, DeliveryTransportError> {
        let mut session = self
            .credentials
            .begin_exclusive(world, MoogleMailOperation::Send)
            .await
            .map_err(preparation_error)?;
        match self.sender.preflight(session.cookie(), world).await {
            Ok(snapshot) if snapshot.clean => {
                match self.sender.prepare_sent_baseline(session.cookie(), world).await {
                    Ok(baseline) => {
                        Ok(PreparedMoogleMailCredentials { session, baseline: Some(baseline) })
                    }
                    Err(error) => {
                        if error.is_session_expired() {
                            let _ = session.invalidate_current().await;
                        }
                        Err(preparation_error(error))
                    }
                }
            }
            Ok(snapshot) => Err(DeliveryTransportError {
                failure_code: "compose_not_clean".to_owned(),
                safe_detail: "compose 尚有草稿资产，延后新任务且保留旧任务核对责任".to_owned(),
                outcome_unknown: false,
                external_reference: None,
                // 只保留计数，不收件主题/正文，避免把草稿内容写进诊断。
                result_metadata: json!({
                    "stage": "preflight",
                    "write_started": false,
                    "credits": snapshot.credits,
                    "cod": snapshot.cod,
                    "attachment_count": snapshot.attachment_count,
                }),
            }),
            Err(error) => {
                if error.is_session_expired() {
                    let _ = session.invalidate_current().await;
                }
                Err(preparation_error(error))
            }
        }
    }

    pub(super) async fn send_with_credentials(
        &self,
        prepared: &mut PreparedMoogleMailCredentials,
        request: MoogleMailSendRequest,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        // fence 后只消费一次基线；不换 Cookie、不重新扫描基线，也不释放后重获 guard。
        let baseline = prepared.baseline.take().ok_or_else(|| {
            MoogleMailError::new(
                MoogleMailOperation::Send,
                request.world,
                MoogleMailErrorCategory::Configuration,
                false,
            )
        })?;
        let result = self.sender.send_prepared(prepared.session.cookie(), request, baseline).await;
        // 诊断 GET 的失效同样撤销凭据，但保留原投递结果；不轮换或重发。
        if !prepared.session.cookie().is_active()
            || result
                .as_ref()
                .is_err_and(|error| error.category == MoogleMailErrorCategory::SessionExpired)
        {
            let _ = prepared.session.invalidate_current().await;
        }
        result
    }
}

/// 仅用于 fence 前的只读路径；不能把相同错误映射用于已经开始写入的发送阶段。
fn preparation_error(error: MoogleMailError) -> DeliveryTransportError {
    let mut error = transport_error(error, false);
    error.outcome_unknown = false;
    error.result_metadata["preflight_blocked"] = true.into();
    error
}
