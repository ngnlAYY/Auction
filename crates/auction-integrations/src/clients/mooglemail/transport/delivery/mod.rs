//! MoogleMail delivery transport 的入口分派与稳定端口适配。

use super::*;

mod actions;
mod credentials;
mod messages;
mod settlement_secrets;

impl MoogleMailDeliveryTransport {
    /// 使用已经由配置层校验的 MoogleMail client 和凭据构造 transport。
    pub fn new(
        client: MoogleMailClient,
        credentials: MoogleMailCredentialSet,
        login_keys: Arc<dyn MailLoginKeyReader>,
    ) -> Self {
        Self::with_templates(
            client,
            credentials,
            login_keys,
            MoogleMailNotificationTemplates::default(),
        )
    }

    /// 使用指定的已校验模板快照构造 transport。
    pub fn with_templates(
        client: MoogleMailClient,
        credentials: MoogleMailCredentialSet,
        login_keys: Arc<dyn MailLoginKeyReader>,
        templates: MoogleMailNotificationTemplates,
    ) -> Self {
        let sender = MoogleMailSender::new(client.clone());
        Self {
            client,
            credentials,
            sender,
            parser: MoogleMailParser,
            login_keys,
            settlement_equipment_secrets: None,
            settlement_equipment_opener: None,
            templates,
        }
    }

    /// 注入结算装备 owner 引用解析器；持久 payload 只保存 UUID，发送前才短暂打开装备 key。
    pub fn with_settlement_equipment_secrets(
        mut self,
        secrets: Arc<dyn SettlementFulfillmentEquipmentSecretReader>,
        opener: EquipmentKeyOpener,
    ) -> Self {
        self.settlement_equipment_secrets = Some(secrets);
        self.settlement_equipment_opener = Some(opener);
        self
    }

    async fn send_claim(
        &self,
        claim: DeliveryWorkClaim,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        match claim.origin_kind.as_str() {
            "mail_intake" => {
                let (payload, world) = decode_payload(&claim)?;
                match payload.operation.as_str() {
                    "send" => self.send_notification(&claim, &payload, world).await,
                    "return" => {
                        self.send_action(
                            &claim,
                            &payload.external_mail_id,
                            world,
                            MoogleMailOperation::ReturnMessage,
                            None,
                        )
                        .await
                    }
                    "take" => {
                        self.send_action(
                            &claim,
                            &payload.external_mail_id,
                            world,
                            MoogleMailOperation::Take,
                            Some(ActionExpectation {
                                credits: payload.credits,
                                cod: payload.cod,
                                attachment_ids: payload.attachment_ids.clone(),
                            }),
                        )
                        .await
                    }
                    _ => Err(invalid_payload()),
                }
            }
            "settlement" => {
                let (payload, world) = decode_settlement_payload(&claim)?;
                match payload.operation.as_str() {
                    "send" => self.send_settlement(&claim, &payload, world).await,
                    "return" => {
                        let external_mail_id = payload
                            .source_external_mail_id
                            .as_deref()
                            .ok_or_else(invalid_payload)?;
                        self.send_action(
                            &claim,
                            external_mail_id,
                            world,
                            MoogleMailOperation::ReturnMessage,
                            None,
                        )
                        .await
                    }
                    _ => Err(invalid_payload()),
                }
            }
            "settlement_recovery_action" => {
                let (payload, world) = decode_payload(&claim)?;
                match payload.operation.as_str() {
                    "send" => self.send_notification(&claim, &payload, world).await,
                    "take" => {
                        self.send_action(
                            &claim,
                            &payload.external_mail_id,
                            world,
                            MoogleMailOperation::Take,
                            Some(ActionExpectation {
                                credits: payload.credits,
                                cod: payload.cod,
                                attachment_ids: payload.attachment_ids.clone(),
                            }),
                        )
                        .await
                    }
                    _ => Err(invalid_payload()),
                }
            }
            "operations_alert" => {
                let (payload, world) = decode_payload(&claim)?;
                if payload.operation != "send" {
                    return Err(invalid_payload());
                }
                self.send_notification(&claim, &payload, world).await
            }
            _ => Err(invalid_payload()),
        }
    }
}

impl DeliveryTransport for MoogleMailDeliveryTransport {
    fn maximum_concurrency(&self) -> u16 {
        // MoogleMail compose 是账号级可变状态；先在领取循环形成背压，避免多个 claim 进入
        // `sending` 后等待凭据锁并把排队时间误计为远端请求超时。
        1
    }

    fn available(&self, claim: &DeliveryWorkClaim) -> bool {
        let world = match claim.origin_kind.as_str() {
            "settlement" => decode_settlement_payload(claim).map(|(_, world)| world),
            "mail_intake" | "settlement_recovery_action" | "operations_alert" => {
                decode_payload(claim).map(|(_, world)| world)
            }
            // 非法 owner/payload 必须进入正式发送路径并确定失败，不能被 availability gate
            // 永久留在 pending。
            _ => return true,
        };
        world.map_or(true, |world| self.credentials.available(world))
    }

    fn send(
        &self,
        claim: DeliveryWorkClaim,
    ) -> Pin<Box<dyn Future<Output = Result<DeliverySendResult, DeliveryTransportError>> + Send + '_>>
    {
        Box::pin(async move { self.send_claim(claim).await })
    }
}
