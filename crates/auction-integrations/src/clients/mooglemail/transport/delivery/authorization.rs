//! 所有 Send 共用原 session、冻结请求与撤销 guard；现金另附同会话资金证据。

use std::time::Instant;

use auction_application::{
    AuthorizedDeliverySend, DeliverySendCredentialBinding, InternalEvidenceError,
    InternalEvidenceFuture, InternalEvidenceLease, InternalEvidencePurpose,
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

    /// 复用数据库资金策略；记账模式仍取得真实身份与原独占会话，不读取现金余额。
    pub fn with_funding_policy_reader(
        mut self,
        reader: Arc<dyn auction_application::FundingPolicyReader>,
    ) -> Self {
        self.funding_policy_reader = Some(reader);
        self
    }

    pub(super) async fn prepare_authorized_claim(
        &self,
        claim: DeliveryWorkClaim,
        deadline: Instant,
    ) -> Result<AuthorizedDeliverySend<'_>, DeliveryTransportError> {
        // 原 Worker 绝对预算覆盖渲染、排队、compose、完整 Sent 基线和证据准备。
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
            let Some(rendered) = self.render_send(&claim).await? else {
                let take = self.prepare_take(&claim).await?;
                let expectation = take.as_ref().map(|take| Arc::clone(&take.expectation));
                let fence_lease =
                    take.as_ref().map(|take| Box::new(SharedSession(Arc::clone(&take.session))));
                let send: DeliveryPreparedSend<'_> = Box::pin(self.send_action_claim(claim, take));
                let mut prepared = AuthorizedDeliverySend::new(send, None);
                if let Some(expectation) = expectation {
                    prepared = prepared.with_take_expectation(expectation);
                }
                if let Some(fence_lease) = fence_lease {
                    prepared = prepared.with_fence_lease(fence_lease);
                }
                return Ok(prepared);
            };
            let mut expectation = rendered.expectation(None)?;
            let world = rendered.request.world;
            let PreparedMoogleMailCredentials { session, baseline } =
                self.prepare_credentials(world).await?;
            expectation.credential = match session.internal_credential_binding(world) {
                Ok((credential_id, credential_version, runtime, _)) => {
                    Some(DeliverySendCredentialBinding {
                        credential_id,
                        credential_version,
                        runtime_generation: runtime.generation,
                    })
                }
                // 配置 Cookie 没有持久身份，可即时证明发送，但不能凭后续换号观察恢复。
                Err(InternalEvidenceError::Unsupported) => None,
                Err(_) => return Err(cash_unproven()),
            };
            let expectation = Arc::new(expectation);
            let session = Arc::new(session);
            let evidence = if rendered.request.credits > 0 {
                self.prepare_cash_evidence(&claim, &session, world, deadline).await?
            } else {
                None
            };
            let fence_lease = Box::new(SharedSession(Arc::clone(&session)));
            let send_expectation = Arc::clone(&expectation);
            let send: DeliveryPreparedSend<'_> = Box::pin(async move {
                let session = Arc::try_unwrap(session).map_err(|_| cash_unproven())?;
                self.send_rendered(
                    rendered,
                    &mut PreparedMoogleMailCredentials { session, baseline },
                    &send_expectation,
                    claim.fencing_version,
                )
                .await
            });
            Ok(AuthorizedDeliverySend::new(send, evidence)
                .with_fence_lease(fence_lease)
                .with_send_expectation(expectation))
        })
        .await
        .map_err(|_| cash_unproven())?
    }

    async fn prepare_cash_evidence(
        &self,
        claim: &DeliveryWorkClaim,
        session: &Arc<MoogleMailCredentialSession>,
        world: MoogleMailWorld,
        deadline: Instant,
    ) -> Result<Option<PreparedSettlementInternalEvidence>, DeliveryTransportError> {
        let policy = match &self.funding_policy_reader {
            Some(reader) => reader.funding_policy().await.map_err(|_| cash_unproven())?,
            None => auction_application::FundingPolicy {
                mode: auction_application::FundingAccountingMode::StrictCash,
                expected_uid: None,
                policy_version: 0,
            },
        };
        let Some(gateway) = &self.cash_evidence_gateway else {
            if !policy.observes_cash() {
                return Err(cash_unproven());
            }
            // 这不是授权。DB 必须在同事务 scope 锁下证明无账户保护且无持久关联。
            return Ok(None);
        };
        // 资金证据开始读取后收紧到原有 30 秒上限，绝不延长已经生成的证据寿命。
        let deadline = auction_application::internal_evidence_deadline(deadline);
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
            observe_main_cash: policy.observes_cash(),
            deadline,
        };
        let snapshot = gateway.read_internal_evidence_in_session(session, &request).await.map_err(
            |error| {
                // 预检失败必须留下去敏原因，否则运维无法区分会话/页面/证据过期。
                tracing::warn!(
                    delivery_id = %claim.delivery_id,
                    world = ?request.world,
                    error = ?error,
                    "MoogleMail 现金预检证据读取失败"
                );
                cash_unproven()
            },
        )?;
        if !policy.matches_identity(&snapshot.identity.authenticated_uid) {
            return Err(cash_unproven());
        }
        PreparedSettlementInternalEvidence::new(
            snapshot,
            Box::new(SharedSession(Arc::clone(session))),
            deadline,
        )
        .map(Some)
        .map_err(|error| {
            tracing::warn!(
                delivery_id = %claim.delivery_id,
                error = ?error,
                "MoogleMail 现金预检证据未能绑定会话租约"
            );
            cash_unproven()
        })
    }
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
