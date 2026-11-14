//! MoogleMail return/take 动作的身份前置校验与结果证明。

use std::sync::Arc;

use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::delivery::{DeliverySendCredentialBinding, DeliveryTakeExpectation};
use auction_application::delivery::{
    DeliverySendResult, DeliveryTransportError, DeliveryWorkClaim,
};
use auction_application::settlement::InternalEvidenceError;
use serde_json::json;

use crate::clients::mooglemail::gateway::MoogleMailCredentialSession;
use crate::clients::mooglemail::transport::types::MoogleMailDeliveryTransport;
use crate::clients::mooglemail::transport::{
    ActionExpectation, action_name, action_proof_unproven, decode_payload,
    exact_action_expectation, invalid_payload, preflight_unproven, transport_error,
};
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailForm, MoogleMailOperation, MoogleMailWorld,
};
use crate::safe_diagnostics;

/// Take 的原凭据会话先于持久 fence 取得；动作 future 只能消费这份会话。
pub(super) struct PreparedMoogleMailTake {
    pub(super) session: Arc<MoogleMailCredentialSession>,
    pub(super) expectation: Arc<DeliveryTakeExpectation>,
}

impl MoogleMailDeliveryTransport {
    pub(super) async fn prepare_take(
        &self,
        claim: &DeliveryWorkClaim,
    ) -> Result<Option<PreparedMoogleMailTake>, DeliveryTransportError> {
        if claim.origin_kind == "settlement" {
            return Ok(None);
        }
        let (payload, world) = decode_payload(claim)?;
        if payload.operation != "take" {
            return Ok(None);
        }
        let mut expectation = DeliveryTakeExpectation {
            world_id: payload.world,
            external_mail_id: payload.external_mail_id,
            credential: None,
        };
        if !expectation.validate() {
            return Err(invalid_payload());
        }
        let session = self
            .credentials
            .begin_exclusive(world, MoogleMailOperation::Take)
            .await
            .map_err(|error| transport_error(error, false))?;
        session.check_current().await.map_err(|error| transport_error(error, false))?;
        expectation.credential = match session.internal_credential_binding(world) {
            Ok((credential_id, credential_version, runtime, _)) => {
                Some(DeliverySendCredentialBinding {
                    credential_id,
                    credential_version,
                    runtime_generation: runtime.generation,
                })
            }
            // 配置 Cookie 可即时 Take，但不能用未来当前账号的缺失页恢复这次动作。
            Err(InternalEvidenceError::Unsupported) => None,
            Err(diagnostic_cause) => {
                safe_diagnostics::warn(
                    "mooglemail_take",
                    "bind_credential",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );

                return Err(transport_error(
                    MoogleMailError::new(
                        MoogleMailOperation::Take,
                        world,
                        MoogleMailErrorCategory::CredentialUnavailable,
                        false,
                    ),
                    false,
                ));
            }
        };
        Ok(Some(PreparedMoogleMailTake {
            session: Arc::new(session),
            expectation: Arc::new(expectation),
        }))
    }

    pub(super) async fn send_action(
        &self,
        claim: &DeliveryWorkClaim,
        external_mail_id: &str,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
        expected: Option<ActionExpectation>,
        mut session: MoogleMailCredentialSession,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        let target = self
            .client
            .detail(world, external_mail_id)
            .map_err(|error| transport_error(error, false))?;
        let before = match self.client.get(&target, session.cookie()).await {
            Ok(document) => document,
            Err(error) if error.is_session_expired() => {
                session
                    .invalidate_current()
                    .await
                    .map_err(|error| transport_error(error, false))?;
                // 单 Cookie 撤销后即无可用凭据，保持原动作前置失败分类，不尝试换号。
                return Err(transport_error(
                    MoogleMailError::new(
                        operation,
                        world,
                        MoogleMailErrorCategory::CredentialUnavailable,
                        false,
                    ),
                    false,
                ));
            }
            Err(error) => return Err(transport_error(error, false)),
        };
        let state = self
            .client
            .diagnose_parse(&target, &before, self.parser.parse_action_state(&before, world))
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_take",
                    "parse_preflight",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                preflight_unproven(operation)
            })?;
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
            return Err(preflight_unproven(operation));
        }
        // 预检只放行 Return 与 Take；其余操作已在上方失败关闭。
        let returning = operation == MoogleMailOperation::ReturnMessage;
        let form = if returning {
            MoogleMailForm::return_message(state.token())
        } else {
            MoogleMailForm::take(state.token())
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
        let proof = if returning {
            self.parser.prove_return_completed(&state, &response, world, external_mail_id)
        } else {
            self.parser.prove_take_completed(&state, &response, world)
        };
        self.client.diagnose_parse(&target, &response, proof).await.map_err(
            |diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_take",
                    "verify_take_receipt",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                action_proof_unproven(operation)
            },
        )?;
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
        Ok(DeliverySendResult {
            external_reference: format!(
                "mooglemail:{}:{}:{}",
                world,
                action_name(operation),
                external_mail_id
            ),
            result_metadata,
        })
    }
}
