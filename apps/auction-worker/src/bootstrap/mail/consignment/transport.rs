//! 按世界 gateway 组合寄售处置传输，并投影一次性许可执行结果。

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use auction_application::consignment::{ConsignmentError, ConsignmentFuture, PostPermit};
use auction_application::mail::{
    ConsignmentDispositionExecution, ConsignmentDispositionProof, ConsignmentDispositionRequest,
    ConsignmentDispositionTransport, ConsignmentNotSent, ConsignmentNotSentReason,
    ConsignmentSentUnprovenReason, PreparedDisposition, disposition_proof_observation_id,
};
use auction_integrations::mooglemail::{
    DispositionAction as IntegrationAction, DispositionOutcome, DispositionRequest,
    DispositionTarget, ExpectedReturn, FrozenMailAttachment, FrozenMailManifest, MoogleMailGateway,
    MoogleMailWorld, PreparedConsignmentDisposition, execute_prepared_consignment_disposition,
};
use auction_jobs::consignment::{TakeExecution, TakeProof};

/// 组合按世界 gateway 的寄售处置传输。
pub struct MoogleMailConsignmentTransport {
    gateways: HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
}

impl MoogleMailConsignmentTransport {
    /// 使用 Worker 已装配的同一组 gateway，避免第二套限速或凭据状态。
    pub fn new(gateways: HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>) -> Self {
        Self { gateways }
    }
}

impl ConsignmentDispositionTransport for MoogleMailConsignmentTransport {
    fn prepare(
        &self,
        request: ConsignmentDispositionRequest,
        deadline: Instant,
    ) -> ConsignmentFuture<'_, Result<Box<dyn PreparedDisposition>, ConsignmentNotSent>> {
        Box::pin(async move {
            let world = MoogleMailWorld::from(request.manifest.world);
            let Some(gateway) = self.gateways.get(&world) else {
                return Ok(Err(not_sent(ConsignmentNotSentReason::TemporaryUnavailable)));
            };
            let Some(integration) = integration_request(&request) else {
                return Ok(Err(not_sent(ConsignmentNotSentReason::RequestInvalid)));
            };
            let prepared =
                match gateway.prepare_consignment_disposition(integration, deadline).await {
                    Ok(prepared) => prepared,
                    Err(reason) => return Ok(Err(map_not_sent(reason))),
                };
            let prepared: Box<dyn PreparedDisposition> = Box::new(WorkerPreparedDisposition {
                observed_manifest_digest: prepared.observed_manifest_digest(),
                prepared,
                observation_id: disposition_proof_observation_id(
                    request.manifest.manifest_digest,
                    false,
                ),
            });
            Ok(Ok(prepared))
        })
    }

    fn execute(
        &self,
        prepared: Box<dyn PreparedDisposition>,
        permit: PostPermit,
        _deadline: Instant,
    ) -> ConsignmentFuture<'_, ConsignmentDispositionExecution> {
        Box::pin(async move {
            let prepared = prepared
                .into_execution_handle()
                .and_then(|handle| handle.downcast::<WorkerPreparedDisposition>().ok())
                .ok_or(ConsignmentError::InvalidDepositRequest)?;
            let outcome = execute_prepared_consignment_disposition(prepared.prepared, permit).await;
            Ok(execution_result(outcome))
        })
    }
}

impl MoogleMailConsignmentTransport {
    /// 退件 Take 的一次性执行：消费许可写一次，并把 4D1 证明原样投影为 Take 证明。
    ///
    /// 与 [`ConsignmentDispositionTransport::execute`] 共用同一执行入口；区别只在投影——未签收的证明
    /// 也带出验证器版本与摘要，交消费事务在锁内判定并保留核对（不变量 4、6）。句柄不符、动作不是
    /// Take、写前失败、写后待证一律未知，绝不再 Take。
    pub(super) async fn execute_take(
        &self,
        prepared: Box<dyn PreparedDisposition>,
        permit: PostPermit,
    ) -> TakeExecution {
        let Some(prepared) = prepared
            .into_execution_handle()
            .and_then(|handle| handle.downcast::<WorkerPreparedDisposition>().ok())
        else {
            return TakeExecution::Unknown;
        };
        match execute_prepared_consignment_disposition(prepared.prepared, permit).await {
            DispositionOutcome::Proven(proof) if proof.action() == IntegrationAction::Take => {
                let signed = proof.clone().into_signed().is_some();
                TakeExecution::Proven(TakeProof {
                    observation_id: disposition_proof_observation_id(proof.proof_digest(), signed),
                    validator_version: proof.validator_version().to_owned(),
                    proof_digest: proof.proof_digest(),
                })
            }
            _ => TakeExecution::Unknown,
        }
    }
}

struct WorkerPreparedDisposition {
    prepared: PreparedConsignmentDisposition,
    observed_manifest_digest: [u8; 32],
    observation_id: uuid::Uuid,
}

impl PreparedDisposition for WorkerPreparedDisposition {
    fn session_binding_digest(&self) -> [u8; 32] {
        self.prepared.session_binding_digest()
    }

    fn observed_manifest_digest(&self) -> [u8; 32] {
        self.observed_manifest_digest
    }

    fn observation_id(&self) -> uuid::Uuid {
        self.observation_id
    }

    fn into_execution_handle(self: Box<Self>) -> Option<Box<dyn Any + Send>> {
        // 只有本装配根构造的不透明句柄能回到自己的执行器；`execute` 在此后立即
        // `downcast::<WorkerPreparedDisposition>()`。类型不符、句柄缺失或已被消费都返回
        // `None`，由调用方按失败关闭处理（不变量 6），不做任何资产动作。
        Some(self)
    }
}

fn integration_request(request: &ConsignmentDispositionRequest) -> Option<DispositionRequest> {
    let frozen = FrozenMailManifest {
        world: MoogleMailWorld::from(request.manifest.world),
        external_mail_id: request.manifest.external_mail_id.clone(),
        sender: request.manifest.sender.clone(),
        recipient: request.manifest.recipient.clone(),
        subject: request.manifest.subject.clone(),
        body_digest: request.manifest.body_digest,
        credits: request.manifest.credits,
        cod: request.manifest.cod,
        received_minute_raw: request.manifest.received_minute_raw.clone(),
        received_at: request.manifest.received_at,
        attachments: request
            .manifest
            .attachments
            .iter()
            .map(|attachment| FrozenMailAttachment {
                ordinal: attachment.ordinal,
                name: attachment.name.clone(),
                canonical_eid: attachment.canonical_eid.clone(),
            })
            .collect(),
        manifest_digest: request.manifest.manifest_digest,
    };
    let target = DispositionTarget::new(
        request.final_decision_id,
        request.work_id,
        request.delivery_id,
        request.attempt_id,
        request.fence,
        frozen.clone(),
    )?;
    Some(match request.action {
        auction_application::consignment::DispositionAction::Take => {
            DispositionRequest::Take(target)
        }
        auction_application::consignment::DispositionAction::Return => DispositionRequest::Return {
            target: Box::new(target),
            expected_return: ExpectedReturn::new(frozen),
        },
    })
}

fn map_not_sent(reason: auction_integrations::mooglemail::NotSentReason) -> ConsignmentNotSent {
    use auction_integrations::mooglemail::NotSentReason;

    let reason = match reason {
        NotSentReason::ManifestChanged => ConsignmentNotSentReason::ManifestChanged,
        NotSentReason::IdentityChanged => ConsignmentNotSentReason::IdentityChanged,
        NotSentReason::ActionUnavailable => ConsignmentNotSentReason::ActionUnavailable,
        NotSentReason::DeadlineExpired => ConsignmentNotSentReason::DeadlineExpired,
        NotSentReason::PermitMismatch
        | NotSentReason::WriteNotStarted
        | NotSentReason::RequestInvalid => ConsignmentNotSentReason::RequestInvalid,
        NotSentReason::TemporaryUnavailable => ConsignmentNotSentReason::TemporaryUnavailable,
    };
    not_sent(reason)
}

fn not_sent(reason: ConsignmentNotSentReason) -> ConsignmentNotSent {
    ConsignmentNotSent { reason, observed_manifest_digest: None }
}

fn execution_result(outcome: DispositionOutcome) -> ConsignmentDispositionExecution {
    match outcome {
        DispositionOutcome::NotSent { reason } => ConsignmentDispositionExecution::NotSent {
            reason: map_not_sent_reason(reason),
            observed_manifest_digest: None,
        },
        DispositionOutcome::SentUnproven { reason, .. } => {
            ConsignmentDispositionExecution::SentUnproven {
                reason: match reason {
                    auction_integrations::mooglemail::SentUnprovenReason::WriteUnknown => {
                        ConsignmentSentUnprovenReason::WriteUnknown
                    }
                    auction_integrations::mooglemail::SentUnprovenReason::ProofUnproven => {
                        ConsignmentSentUnprovenReason::ProofUnproven
                    }
                },
            }
        }
        DispositionOutcome::Proven(proof) => {
            let signed = proof.clone().into_signed().is_some();
            let action = match proof.action() {
                IntegrationAction::Take => {
                    auction_application::consignment::DispositionAction::Take
                }
                IntegrationAction::Return => {
                    auction_application::consignment::DispositionAction::Return
                }
            };
            match ConsignmentDispositionProof::from_adapter(
                action,
                disposition_proof_observation_id(proof.proof_digest(), signed),
                proof.validator_version().to_owned(),
                proof.proof_digest(),
                signed,
            ) {
                Ok(proof) => ConsignmentDispositionExecution::Proven(proof),
                Err(error) => {
                    tracing::warn!(error = %error, "寄售处置证明投影失败，按未证明处理");
                    ConsignmentDispositionExecution::SentUnproven {
                        reason: ConsignmentSentUnprovenReason::ProofUnproven,
                    }
                }
            }
        }
    }
}

fn map_not_sent_reason(
    reason: auction_integrations::mooglemail::NotSentReason,
) -> ConsignmentNotSentReason {
    use auction_integrations::mooglemail::NotSentReason;

    match reason {
        NotSentReason::ManifestChanged => ConsignmentNotSentReason::ManifestChanged,
        NotSentReason::IdentityChanged => ConsignmentNotSentReason::IdentityChanged,
        NotSentReason::ActionUnavailable => ConsignmentNotSentReason::ActionUnavailable,
        NotSentReason::DeadlineExpired => ConsignmentNotSentReason::DeadlineExpired,
        NotSentReason::PermitMismatch
        | NotSentReason::WriteNotStarted
        | NotSentReason::RequestInvalid => ConsignmentNotSentReason::RequestInvalid,
        NotSentReason::TemporaryUnavailable => ConsignmentNotSentReason::TemporaryUnavailable,
    }
}
