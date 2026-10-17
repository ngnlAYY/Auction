//! MoogleMail delivery 的单 Cookie 会话与写入结果未知边界。

use super::*;

impl MoogleMailDeliveryTransport {
    /// 只读检查时取得 guard，并把它交给消耗式 prepared handle，绝不在 fence 间隙释放。
    pub(super) async fn prepare_credentials(
        &self,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailCredentialSession, DeliveryTransportError> {
        let mut session = self
            .credentials
            .begin_exclusive(world, MoogleMailOperation::Send)
            .await
            .map_err(|error| transport_error(error, false))?;
        match self.sender.preflight(session.cookie(), world).await {
            Ok(true) => Ok(session),
            Ok(false) => Err(DeliveryTransportError {
                failure_code: "compose_not_clean".to_owned(),
                safe_detail: "compose 尚有草稿资产，延后新任务且保留旧任务核对责任".to_owned(),
                outcome_unknown: false,
                external_reference: None,
                result_metadata: json!({"stage": "preflight", "write_started": false}),
            }),
            Err(error) => {
                if error.is_session_expired() {
                    let _ = session.invalidate_current().await;
                }
                Err(transport_error(error, false))
            }
        }
    }

    pub(super) async fn send_with_credentials(
        &self,
        session: &mut MoogleMailCredentialSession,
        request: MoogleMailSendRequest,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        // fence 后仅执行一次发送；不换 Cookie、不释放后重获 guard。
        let result = self.sender.send(session.cookie(), request).await;
        if result.as_ref().is_err_and(|error| error.is_session_expired()) {
            let _ = session.invalidate_current().await;
        }
        result
    }
}
