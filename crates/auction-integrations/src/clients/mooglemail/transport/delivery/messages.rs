//! MoogleMail 通知与结算邮件的消息构造和发送。

use super::*;

impl MoogleMailDeliveryTransport {
    pub(super) async fn send_settlement(
        &self,
        claim: &DeliveryWorkClaim,
        payload: &SettlementDeliveryPayload,
        world: MoogleMailWorld,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        let (subject, body) =
            render_settlement(payload, &self.templates).ok_or_else(invalid_payload)?;
        let outcome = self
            .send_with_credentials(MoogleMailSendRequest {
                world,
                message: MoogleMailMessageFields {
                    recipient: claim.recipient.clone(),
                    subject,
                    body,
                },
                credits: payload.credits,
                cod: payload.cod,
                equipment_ids: match payload.template.as_str() {
                    "settlement_winner_equipment_cod" | "settlement_equipment_handoff" => {
                        vec![payload.external_equipment_id.clone()]
                    }
                    _ => Vec::new(),
                },
            })
            .await
            .map_err(|error| transport_error(error, false))?;
        let receipt = send_receipt(outcome)?;
        let external_reference_proven = receipt.external_mail_id.is_some();
        let external_reference =
            receipt.external_mail_id.unwrap_or_else(|| format!("delivery:{}", claim.delivery_id));
        Ok(DeliverySendResult {
            external_reference,
            result_metadata: json!({
                "operation": "send",
                "template": payload.template,
                "external_reference_proven": external_reference_proven,
                "settlement_obligation_id": payload.obligation_id,
            }),
        })
    }

    pub(super) async fn send_notification(
        &self,
        claim: &DeliveryWorkClaim,
        payload: &MailDeliveryPayload,
        world: MoogleMailWorld,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        let template = payload.template.as_deref().ok_or_else(invalid_payload)?;
        let result_code = payload.result_code.as_deref().ok_or_else(invalid_payload)?;
        let login_key = match payload.user_key_user_id {
            Some(user_id) => Some(self.login_keys.read(user_id).await.map_err(login_key_error)?),
            None => None,
        };
        let (subject, body) = if template == "operations_alert" && result_code == "operations_alert"
        {
            let summary = payload.body.as_deref().ok_or_else(invalid_payload)?;
            (
                "Auction operations alert".to_owned(),
                self.templates
                    .render("operations_alert", &[("summary", summary)])
                    .ok_or_else(invalid_payload)?,
            )
        } else {
            render_notification(
                &self.templates,
                template,
                result_code,
                login_key.as_ref(),
                &claim.recipient,
                &payload.external_mail_id,
            )
            .ok_or_else(invalid_payload)?
        };
        if !valid_line(&subject, 512, false) || !valid_line(&body, 64 * 1024, true) {
            return Err(invalid_payload());
        }
        let message = MoogleMailMessageFields { recipient: claim.recipient.clone(), subject, body };
        let outcome = self
            .send_with_credentials(MoogleMailSendRequest {
                world,
                message,
                credits: 0,
                cod: 0,
                equipment_ids: Vec::new(),
            })
            .await
            .map_err(|error| transport_error(error, false))?;
        let receipt = send_receipt(outcome)?;
        let external_reference_proven = receipt.external_mail_id.is_some();
        let external_reference =
            receipt.external_mail_id.unwrap_or_else(|| format!("delivery:{}", claim.delivery_id));
        Ok(DeliverySendResult {
            external_reference,
            result_metadata: json!({
                "operation": "send",
                "template": template,
                "external_reference_proven": external_reference_proven,
            }),
        })
    }
}
