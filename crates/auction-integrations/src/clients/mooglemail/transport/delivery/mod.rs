//! MoogleMail delivery transport 的入口分派与稳定端口适配。

use super::*;

mod actions;
mod credentials;
mod messages;
mod settlement_secrets;

impl MoogleMailDeliveryTransport {
    /// 配置所有 Send 操作的发送前后 Sent 核对边界。
    pub fn with_send_verification(
        mut self,
        maximum_pages: usize,
        budget: std::time::Duration,
    ) -> Result<Self, MoogleMailError> {
        self.sender = self.sender.with_send_verification(maximum_pages, budget)?;
        Ok(self)
    }

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
        // 必须先附加 owner，再交给 sender 克隆，覆盖全部 compose/Sent 请求。
        let client = client.with_ownership(credentials.ownership());
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
        mut session: Option<MoogleMailCredentialSession>,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        match claim.origin_kind.as_str() {
            "mail_intake" => {
                let (payload, world) = decode_payload(&claim)?;
                match payload.operation.as_str() {
                    "send" => {
                        self.send_notification(
                            &claim,
                            &payload,
                            world,
                            session.as_mut().ok_or_else(invalid_payload)?,
                        )
                        .await
                    }
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
                    "send" => {
                        self.send_settlement(
                            &claim,
                            &payload,
                            world,
                            session.as_mut().ok_or_else(invalid_payload)?,
                        )
                        .await
                    }
                    // 旧 payload 只允许读取并隔离；原邮件的 Return 不能证明已入库装备返还。
                    // 历史执行结果无法在此证明，保留人工态，且不执行任何远端写入。
                    "return" => Err(legacy_settlement_return()),
                    _ => Err(invalid_payload()),
                }
            }
            "settlement_recovery_action" => {
                let (payload, world) = decode_payload(&claim)?;
                match payload.operation.as_str() {
                    "send" => {
                        self.send_notification(
                            &claim,
                            &payload,
                            world,
                            session.as_mut().ok_or_else(invalid_payload)?,
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
            "operations_alert" | "lot_watch" => {
                let (payload, world) = decode_payload(&claim)?;
                if payload.operation != "send" {
                    return Err(invalid_payload());
                }
                self.send_notification(
                    &claim,
                    &payload,
                    world,
                    session.as_mut().ok_or_else(invalid_payload)?,
                )
                .await
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
            "settlement" => match decode_settlement_payload(claim) {
                Ok((payload, _)) if payload.operation == "return" => return true,
                result => result.map(|(_, world)| world),
            },
            "mail_intake" | "settlement_recovery_action" | "operations_alert" | "lot_watch" => {
                decode_payload(claim).map(|(_, world)| world)
            }
            // 非法 owner/payload 必须进入正式发送路径并确定失败，不能被 availability gate
            // 永久留在 pending。
            _ => return true,
        };
        world.map_or(true, |world| self.credentials.available(world))
    }

    fn prepare(&self, claim: DeliveryWorkClaim) -> DeliveryPreparation<'_> {
        Box::pin(async move {
            let world = match claim.origin_kind.as_str() {
                "settlement" => decode_settlement_payload(&claim)
                    .ok()
                    .filter(|(payload, _)| payload.operation == "send")
                    .map(|(_, world)| world),
                "mail_intake" | "settlement_recovery_action" | "operations_alert" | "lot_watch" => {
                    decode_payload(&claim)
                        .ok()
                        .filter(|(payload, _)| payload.operation == "send")
                        .map(|(_, world)| world)
                }
                _ => None,
            };
            let session = match world {
                Some(world) => Some(self.prepare_credentials(world).await?),
                None => None,
            };
            // 独占 session 被移入尚未 poll 的 future；数据库 fence 失败或超时丢弃句柄即释放。
            let prepared: DeliveryPreparedSend<'_> =
                Box::pin(async move { self.send_claim(claim, session).await });
            Ok(prepared)
        })
    }

    fn send(
        &self,
        claim: DeliveryWorkClaim,
    ) -> Pin<Box<dyn Future<Output = Result<DeliverySendResult, DeliveryTransportError>> + Send + '_>>
    {
        Box::pin(async move { self.prepare(claim).await?.await })
    }
}

fn legacy_settlement_return() -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "legacy_return_requires_review".to_owned(),
        safe_detail: "旧结算 Return 无法证明装备返还，须人工核对库存与历史发送记录".to_owned(),
        outcome_unknown: true,
        external_reference: None,
        result_metadata: serde_json::json!({
            "category": "legacy_settlement_return",
            "write_started": false,
        }),
    }
}
