//! 现金准备复用原session；证据租约与未poll的发送future共享同一个独占guard。

use std::time::Instant;

use auction_application::{
    AuthorizedDeliverySend, InternalEvidenceFuture, InternalEvidenceLease, InternalEvidencePurpose,
    InternalEvidenceRequest, PreparedSettlementInternalEvidence,
};
use auction_domain::SettlementWorld;

use super::*;
use crate::clients::mooglemail::MoogleMailGateway;

impl MoogleMailDeliveryTransport {
    /// 必须注入共享原client/凭据集合/owner的gateway；A在读取时核验集合身份。
    pub fn with_cash_evidence_gateway(mut self, gateway: Arc<MoogleMailGateway>) -> Self {
        self.cash_evidence_gateway = Some(gateway);
        self
    }

    pub(super) async fn prepare_cash_authorized(
        &self,
        claim: DeliveryWorkClaim,
        deadline: Instant,
    ) -> Result<AuthorizedDeliverySend<'_>, DeliveryTransportError> {
        if !requires_cash_gate(&claim) {
            return self.prepare(claim).await.map(|send| AuthorizedDeliverySend::new(send, None));
        }
        // 同一绝对预算覆盖guard排队、原clean-compose和A的全部GET。
        let deadline = auction_application::internal_evidence_deadline(deadline);
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
            let world = match claim.origin_kind.as_str() {
                "settlement" => decode_settlement_payload(&claim)?.1,
                _ => decode_payload(&claim)?.1,
            };
            let session = Arc::new(self.prepare_credentials(world).await?);
            let evidence = if let Some(gateway) = &self.cash_evidence_gateway {
                let request = InternalEvidenceRequest {
                    operation_id: claim.command_id,
                    purpose: InternalEvidencePurpose::DeliveryClaim {
                        delivery_id: claim.delivery_id,
                        claim_token: claim.lease_token,
                        claim_fencing: claim.fencing_version,
                        request_hash: claim.request_hash,
                    },
                    world: match world {
                        MoogleMailWorld::Main => SettlementWorld::Main,
                        MoogleMailWorld::Isekai => SettlementWorld::Isekai,
                    },
                    equipment: None,
                    observe_main_cash: true,
                    deadline,
                };
                let snapshot = gateway
                    .read_internal_evidence_in_session(&session, &request)
                    .await
                    .map_err(|_| cash_unproven())?;
                Some(
                    PreparedSettlementInternalEvidence::new(
                        snapshot,
                        Box::new(SharedSession(Arc::clone(&session))),
                        deadline,
                    )
                    .map_err(|_| cash_unproven())?,
                )
            } else {
                // 这不是授权。DB必须在同事务scope锁下证明无账户保护且无持久关联。
                None
            };
            let fence_lease = Box::new(SharedSession(Arc::clone(&session)));
            let send: DeliveryPreparedSend<'_> = Box::pin(async move {
                let session = Arc::try_unwrap(session).map_err(|_| cash_unproven())?;
                self.send_claim(claim, Some(session)).await
            });
            Ok(AuthorizedDeliverySend::new(send, evidence).with_fence_lease(fence_lease))
        })
        .await
        .map_err(|_| cash_unproven())?
    }
}

/// 非法或未知金额形状保守进入门禁，不能借字符串金额绕过现金授权。
pub(super) fn requires_cash_gate(claim: &DeliveryWorkClaim) -> bool {
    claim.payload.get("operation").and_then(serde_json::Value::as_str) == Some("send")
        && claim.payload.get("credits").and_then(serde_json::Value::as_i64) != Some(0)
}

pub(super) fn cash_unproven() -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "cash_authorization_unproven".to_owned(),
        safe_detail: "现金发送缺少同会话资金授权，延后并保留原资金责任".to_owned(),
        outcome_unknown: false,
        external_reference: None,
        result_metadata: json!({"stage":"cash_preflight", "write_started":false}),
    }
}

struct SharedSession(Arc<MoogleMailCredentialSession>);

impl InternalEvidenceLease for SharedSession {
    fn is_current(&self) -> bool {
        InternalEvidenceLease::is_current(self.0.as_ref())
    }

    fn check_current(&self, deadline: Instant) -> InternalEvidenceFuture<'_, ()> {
        Box::pin(self.0.check_internal_current(deadline))
    }

    fn revoke(&self) {
        self.0.revoke_internal_owner();
    }
}
