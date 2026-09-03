//! MoogleMail delivery 的单 Cookie 会话与写入结果未知边界。

use super::*;

impl MoogleMailDeliveryTransport {
    pub(super) async fn send_with_credentials(
        &self,
        request: MoogleMailSendRequest,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        let mut session =
            self.credentials.begin_exclusive(request.world, MoogleMailOperation::Send).await?;
        for _ in 0..64 {
            match self.sender.send(session.cookie(), request.clone()).await {
                Ok(receipt) => return Ok(receipt),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    if error.write_started() {
                        let _ = session.invalidate_current().await;
                        return Err(error);
                    }
                    session.invalidate_current().await?;
                    session.advance_exclusive().await?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(MoogleMailError::new(
            MoogleMailOperation::Send,
            request.world,
            MoogleMailErrorCategory::CredentialUnavailable,
            false,
        ))
    }
}
