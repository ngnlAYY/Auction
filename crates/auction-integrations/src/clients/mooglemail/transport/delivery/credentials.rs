//! MoogleMail delivery 的凭据轮换与写入结果未知边界。

use super::*;

impl MoogleMailDeliveryTransport {
    pub(super) async fn send_with_credentials(
        &self,
        request: MoogleMailSendRequest,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        let mut session = self.credentials.begin(request.world)?;
        for _ in 0..64 {
            match self.sender.send(session.cookie(), request.clone()).await {
                Ok(receipt) => return Ok(receipt),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    if error.write_started() {
                        let _ = session.invalidate_current().await;
                        return Err(error);
                    }
                    session.invalidate_current().await?;
                    session.advance()?;
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
