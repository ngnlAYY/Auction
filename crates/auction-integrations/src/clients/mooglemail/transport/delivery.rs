use super::*;

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
        Self { client, credentials, sender, parser: MoogleMailParser, login_keys, templates }
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

    async fn send_settlement(
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

    async fn send_notification(
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

    /// 读侧 session 失效时轮换并回写当前凭据；写侧失效只回写，不发起第二次 compose。
    async fn send_with_credentials(
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

    async fn send_action(
        &self,
        claim: &DeliveryWorkClaim,
        external_mail_id: &str,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
        expected: Option<ActionExpectation>,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        let target = self
            .client
            .detail(world, external_mail_id)
            .map_err(|error| transport_error(error, false))?;
        let mut session =
            self.credentials.begin(world).map_err(|error| transport_error(error, false))?;
        for _ in 0..64 {
            let before = match self.client.get(&target, session.cookie()).await {
                Ok(document) => document,
                Err(error) if error.is_session_expired() => {
                    session
                        .invalidate_current()
                        .await
                        .map_err(|error| transport_error(error, false))?;
                    session.advance().map_err(|error| transport_error(error, false))?;
                    continue;
                }
                Err(error) => return Err(transport_error(error, false)),
            };
            let state =
                self.parser.parse_action_state(&before, world).map_err(|_| preflight_unproven())?;
            let preflight_proven = state.sender().eq_ignore_ascii_case(&claim.recipient)
                && match operation {
                    MoogleMailOperation::ReturnMessage => state.returnable && expected.is_none(),
                    MoogleMailOperation::Take => {
                        state.claimable
                            && expected
                                .as_ref()
                                .is_some_and(|expected| exact_action_expectation(&state, expected))
                    }
                    _ => false,
                };
            if !preflight_proven {
                return Err(preflight_unproven());
            }
            let form = match operation {
                MoogleMailOperation::ReturnMessage => MoogleMailForm::return_message(state.token()),
                MoogleMailOperation::Take => MoogleMailForm::take(state.token()),
                _ => unreachable!("validated MoogleMail action operation"),
            };
            let form = form.map_err(|error| transport_error(error, false))?;
            let response = match self.client.post(&target, session.cookie(), form).await {
                Ok(response) => response,
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    let _ = session.invalidate_current().await;
                    return Err(transport_error(error, true));
                }
                Err(error) => return Err(transport_error(error, true)),
            };
            match operation {
                MoogleMailOperation::ReturnMessage => {
                    self.parser.prove_return_completed(&response, world, external_mail_id)
                }
                MoogleMailOperation::Take => {
                    self.parser.prove_take_completed(&state, &response, world)
                }
                _ => unreachable!("validated MoogleMail action operation"),
            }
            .map_err(|_| action_proof_unproven())?;
            let mut result_metadata = json!({
                "operation": action_name(operation),
                "external_mail_id": external_mail_id,
                "proof": "action_removed_or_success_page",
            });
            if let Some(expected) = expected.as_ref() {
                result_metadata["expected_credits"] = expected.credits.into();
                result_metadata["expected_cod"] = expected.cod.into();
                result_metadata["expected_attachment_ids"] = serde_json::Value::Array(
                    expected.attachment_ids.iter().cloned().map(Into::into).collect(),
                );
            }
            return Ok(DeliverySendResult {
                external_reference: format!(
                    "mooglemail:{}:{}:{}",
                    world,
                    action_name(operation),
                    external_mail_id
                ),
                result_metadata,
            });
        }
        Err(DeliveryTransportError {
            failure_code: "credential_unavailable".to_owned(),
            safe_detail: "MoogleMail 没有可用凭据".to_owned(),
            outcome_unknown: false,
            external_reference: None,
            result_metadata: json!({}),
        })
    }
}

impl DeliveryTransport for MoogleMailDeliveryTransport {
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
