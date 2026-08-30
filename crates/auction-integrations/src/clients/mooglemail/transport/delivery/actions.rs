//! MoogleMail return/take 动作的身份前置校验与结果证明。

use super::*;

impl MoogleMailDeliveryTransport {
    pub(super) async fn send_action(
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
                    self.parser.prove_return_completed(&state, &response, world, external_mail_id)
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
