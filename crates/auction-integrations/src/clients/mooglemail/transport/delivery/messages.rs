//! MoogleMail 通知与结算邮件在 sending fence 前渲染，写入只消费同一份内存消息。

use auction_application::delivery::{DeliverySendCredentialBinding, DeliverySendExpectation};
use auction_application::delivery::{
    DeliverySendResult, DeliveryTransportError, DeliveryWorkClaim,
};
use serde_json::json;

use super::credentials::PreparedMoogleMailCredentials;
use crate::clients::mooglemail::forms::valid_body;
use crate::clients::mooglemail::protocol::valid_line;
use crate::clients::mooglemail::transport::types::MoogleMailDeliveryTransport;
use crate::clients::mooglemail::transport::{
    MailDeliveryPayload, SettlementDeliveryPayload, decode_payload, decode_settlement_payload,
    invalid_payload, login_key_error, render_notification, render_settlement, send_receipt,
    transport_error,
};
use crate::clients::mooglemail::{MoogleMailInventoryCategory, parser::canonical_mail_body_digest};
use crate::clients::mooglemail::{MoogleMailMessageFields, MoogleMailSendRequest, MoogleMailWorld};

/// 正文只留在消耗式 future；摘要和资产身份从这份实际请求生成，不能在 fence 后重建。
pub(super) struct RenderedMoogleMailSend {
    pub(super) request: MoogleMailSendRequest,
    template: String,
    settlement_obligation_id: Option<String>,
}

impl RenderedMoogleMailSend {
    pub(super) fn expectation(
        &self,
        credential: Option<DeliverySendCredentialBinding>,
    ) -> Result<DeliverySendExpectation, DeliveryTransportError> {
        let expectation = DeliverySendExpectation {
            world_id: self.request.world.to_string(),
            recipient: self.request.message.recipient.clone(),
            subject: self.request.message.subject.clone(),
            body_digest: canonical_mail_body_digest(&self.request.message.body),
            credits: self.request.credits,
            cod: self.request.cod,
            equipment_ids: self.request.equipment_ids.clone(),
            credential,
        };
        if !expectation.validate() {
            return Err(invalid_payload());
        }
        Ok(expectation)
    }
}

impl MoogleMailDeliveryTransport {
    /// 完整解码先于凭据与远端预检；坏 payload 不能伪装成可无限延期的 preflight。
    pub(super) async fn render_send(
        &self,
        claim: &DeliveryWorkClaim,
    ) -> Result<Option<RenderedMoogleMailSend>, DeliveryTransportError> {
        let mut rendered = match claim.origin_kind.as_str() {
            "settlement" => {
                let (payload, world) = decode_settlement_payload(claim)?;
                if payload.operation != "send" {
                    return Ok(None);
                }
                self.render_settlement_send(claim, payload, world).await?
            }
            kind if crate::clients::mooglemail::transport::payload::is_notification_origin(
                kind,
            ) =>
            {
                let (payload, world) = decode_payload(claim)?;
                if payload.operation != "send" {
                    return Ok(None);
                }
                self.render_notification_send(claim, payload, world).await?
            }
            _ => return Err(invalid_payload()),
        };
        rendered.request.equipment_ids.sort_unstable();
        if !rendered.request.validate() {
            return Err(invalid_payload());
        }
        Ok(Some(rendered))
    }

    async fn render_settlement_send(
        &self,
        claim: &DeliveryWorkClaim,
        mut payload: SettlementDeliveryPayload,
        world: MoogleMailWorld,
    ) -> Result<RenderedMoogleMailSend, DeliveryTransportError> {
        let (subject, body) =
            render_settlement(&payload, &self.templates).ok_or_else(invalid_payload)?;
        let equipment_ids =
            self.resolve_settlement_equipment(&payload.equipment_secret_refs).await?;
        // 冻结装备名的域大类只用于减少 Armory 页面读取；名称不识别时不提示。
        let category = if equipment_ids.is_empty() {
            None
        } else {
            payload
                .variables
                .get("item_name")
                .and_then(|name| MoogleMailInventoryCategory::hint_from_name(name))
        };
        Ok(RenderedMoogleMailSend {
            request: MoogleMailSendRequest {
                world,
                message: MoogleMailMessageFields {
                    recipient: claim.recipient.clone(),
                    subject,
                    body,
                },
                credits: payload.credits,
                cod: payload.cod,
                equipment_ids,
                category,
            },
            template: payload.template,
            settlement_obligation_id: payload.variables.remove("reference"),
        })
    }

    async fn render_notification_send(
        &self,
        claim: &DeliveryWorkClaim,
        mut payload: MailDeliveryPayload,
        world: MoogleMailWorld,
    ) -> Result<RenderedMoogleMailSend, DeliveryTransportError> {
        let template = payload.template.take().ok_or_else(invalid_payload)?;
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
        } else if template == "lot_watch" && result_code == "outbid" {
            // watch owner 在创建通知时按 `lot_watch` 模板渲染并冻结正文；发送阶段只读取冻结值，
            // 不再读模板或代理秘密上限。
            ("Auction outbid notice".to_owned(), payload.body.take().ok_or_else(invalid_payload)?)
        } else if template == "auction_submission_receipt" && result_code == "accepted" {
            // 回执正文在录入事实确定后冻结，发送前不重新查询 lot 或价格。
            (
                auction_application::mail::SUBMISSION_RECEIPT_SUBJECT.to_owned(),
                payload.body.take().ok_or_else(invalid_payload)?,
            )
        } else if template == "auction_seller_summary" && result_code == "summary" {
            // 卖方汇总正文在结算事实确定后冻结，发送前不重新聚合 lot 或金额。
            (
                auction_application::player_notifications::SELLER_SUMMARY_SUBJECT.to_owned(),
                payload.body.take().ok_or_else(invalid_payload)?,
            )
        } else if template == "isekai_season_reminder" && result_code == "season_end" {
            // 换季提醒正文绑定已冻结的赛季边界，发送前不重算边界或义务集合。
            (
                auction_application::player_notifications::SEASON_REMINDER_SUBJECT.to_owned(),
                payload.body.take().ok_or_else(invalid_payload)?,
            )
        } else {
            render_notification(
                &self.templates,
                &template,
                result_code,
                login_key.as_ref(),
                &claim.recipient,
                &payload.world,
                &payload.external_mail_id,
            )
            .ok_or_else(invalid_payload)?
        };
        if !valid_line(&subject, 512, false) || !valid_body(&body) {
            return Err(invalid_payload());
        }
        Ok(RenderedMoogleMailSend {
            request: MoogleMailSendRequest {
                world,
                message: MoogleMailMessageFields {
                    recipient: claim.recipient.clone(),
                    subject,
                    body,
                },
                credits: 0,
                cod: 0,
                equipment_ids: Vec::new(),
                category: None,
            },
            template,
            settlement_obligation_id: None,
        })
    }

    /// sending fence 后不读取 key、不解析 payload、不重新渲染，只发送已冻结的实际请求。
    pub(super) async fn send_rendered(
        &self,
        rendered: RenderedMoogleMailSend,
        session: &mut PreparedMoogleMailCredentials,
        expectation: &DeliverySendExpectation,
        fencing_version: i64,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        let world = rendered.request.world;
        // 是否有资产随信寄出决定收件人不存在时的收口方式：零资产通知失败关闭，
        // 带资产发送仍需人工取回退信。该判断只读冻结期望，不改变发送内容。
        let carries_assets =
            expectation.credits > 0 || expectation.cod > 0 || !expectation.equipment_ids.is_empty();
        let outcome = self
            .send_with_credentials(session, rendered.request)
            .await
            .map_err(|error| transport_error(error, false))?;
        let receipt = send_receipt(outcome, carries_assets)?;
        // 旧版 fulfillment snapshot 使用 world/operation 前缀区分远端 ID 与本地 fallback；
        // reconcile 必须先校验此前缀，不能把任意 delivery 引用当作 Sent 邮件 ID。
        let external_reference = format!("mooglemail:{world}:send:{}", receipt.external_mail_id);
        Ok(DeliverySendResult {
            external_reference,
            result_metadata: json!({
                "operation": "send",
                "template": rendered.template,
                "external_reference_proven": true,
                "sent_verified": true,
                "proof": "sent_exact_payload_v1",
                "settlement_obligation_id": rendered.settlement_obligation_id,
                "send_expectation": expectation,
                "sending_fencing_version": fencing_version,
            }),
        })
    }
}
