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
        let equipment_ids =
            self.resolve_settlement_equipment(&payload.equipment_secret_refs).await?;
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
                equipment_ids,
            })
            .await
            .map_err(|error| transport_error(error, false))?;
        let receipt = send_receipt(outcome)?;
        let external_reference_proven = true;
        let identity = receipt.external_mail_id;
        // 旧版 fulfillment snapshot 使用 world/operation 前缀区分远端 ID 与本地 fallback；
        // reconcile 必须先校验此前缀，不能把任意 delivery 引用当作 Sent 邮件 ID。
        let external_reference = format!("mooglemail:{world}:send:{identity}");
        Ok(DeliverySendResult {
            external_reference,
            result_metadata: json!({
                "operation": "send",
                "template": payload.template,
                "external_reference_proven": external_reference_proven,
                "sent_verified": true,
                "proof": "sent_exact_payload_v1",
                "settlement_obligation_id": payload.variables.get("reference"),
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
        let external_reference_proven = true;
        let reference_identity = receipt.external_mail_id;
        let external_reference = format!("mooglemail:{world}:send:{reference_identity}");
        Ok(DeliverySendResult {
            external_reference,
            result_metadata: json!({
                "operation": "send",
                "template": template,
                "external_reference_proven": external_reference_proven,
                "sent_verified": true,
                "proof": "sent_exact_payload_v1",
            }),
        })
    }
}
