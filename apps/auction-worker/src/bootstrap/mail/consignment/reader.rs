//! 组合 gateway、旧 owner 判定器与持久观察的隔离重检读取器。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use auction_application::consignment::{ConsignmentFuture, LegacyAuctionOwnerResolver};
use auction_application::mail::{
    QuarantineRecheckClaim, QuarantineRecheckReadError, QuarantineRecheckReader,
    QuarantineReobservation,
};
use auction_domain::consignment::deposit::SenderIdentity;
use auction_integrations::mooglemail::{
    ConsignmentReadOutcome, ConsignmentReadPending, MoogleMailGateway, MoogleMailWorld,
};
use uuid::Uuid;

/// 组合 gateway、旧 owner 判定器与持久观察的隔离重检读取器。
pub struct WorkerConsignmentRecheckReader {
    gateways: HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
    owners: Arc<dyn LegacyAuctionOwnerResolver>,
}

impl WorkerConsignmentRecheckReader {
    /// 注入按世界 gateway 与旧 owner 判定器。
    pub fn new(
        gateways: HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
        owners: Arc<dyn LegacyAuctionOwnerResolver>,
    ) -> Self {
        Self { gateways, owners }
    }
}

impl QuarantineRecheckReader for WorkerConsignmentRecheckReader {
    fn reread(
        &self,
        claim: &QuarantineRecheckClaim,
        deadline: Instant,
    ) -> ConsignmentFuture<'_, Result<QuarantineReobservation, QuarantineRecheckReadError>> {
        let claim = claim.clone();
        Box::pin(async move {
            let world = MoogleMailWorld::from(claim.locator.world);
            let Some(gateway) = self.gateways.get(&world) else {
                return Ok(Err(QuarantineRecheckReadError::TemporaryUnavailable));
            };
            let bound = match gateway.reread_quarantine(&claim.locator, deadline).await {
                ConsignmentReadOutcome::Observed(bound) => *bound,
                ConsignmentReadOutcome::Pending(reason) => {
                    return Ok(Err(match reason {
                        ConsignmentReadPending::AllowlistDenied => {
                            QuarantineRecheckReadError::InvalidObservation
                        }
                        ConsignmentReadPending::RateLimited
                        | ConsignmentReadPending::AccountBusy
                        | ConsignmentReadPending::AuthenticatedUidUnavailable
                        | ConsignmentReadPending::TransientReadUnavailable => {
                            QuarantineRecheckReadError::TemporaryUnavailable
                        }
                    }));
                }
            };
            if bound.remote_observation_id().is_nil() {
                return Ok(Err(QuarantineRecheckReadError::InvalidObservation));
            }
            let draft = bound.to_envelope_observation(
                claim.previous_observation_id,
                claim.previous_binding.as_ref(),
                None,
            );
            // 身份门只有存储层 `LegacyAuctionOwnerResolver::resolve` 一份：它要求 Verified、
            // 系统 RTS 与规范主题同时成立。读取器不重复实现，未认证时 resolve 返回 None，
            // 观察仍可追加但不会自动交接（不变量 6）。
            let owner = match self.owners.resolve(&claim.mail_key, &draft).await {
                Ok(owner) => owner,
                Err(error) => {
                    tracing::debug!(
                        code = error.api_error().map_or("unavailable", |(_, code)| code),
                        "隔离重检旧 owner 判定失败，本次按暂时不可用重试"
                    );
                    return Ok(Err(QuarantineRecheckReadError::TemporaryUnavailable));
                }
            };
            let mut observation = bound.to_envelope_observation(
                claim.previous_observation_id,
                claim.previous_binding.as_ref(),
                owner,
            );
            // 存储按旧观察行的会话摘要核对“同一条原信仍由同一认证身份续读”。认证摘要包含
            // 本次观察时刻，每次读取都会变化；同 UID（含改名）续读时沿用旧摘要，名字换 UID
            // 则保留新摘要并由存储拒绝追加，隔离保持 active。不变量 3、6。
            if observation.sender_identity == SenderIdentity::Verified
                && let Some(previous) = claim.previous_binding.as_ref()
                && previous.authenticated_uid == bound.binding().authenticated_uid
                && previous.world == bound.binding().world
            {
                observation.session_binding_digest = previous.digest;
            }
            if observation.manifest_digest == [0; 32]
                || observation.session_binding_digest == [0; 32]
            {
                return Ok(Err(QuarantineRecheckReadError::InvalidObservation));
            }
            // 不变量 5、6：同一隔离、同一清单摘要、同一会话绑定摘要就是同一远端观察；
            // 不能把每次领取的 work/token 混进身份，否则原 owner 长期拒绝时会按退避无限
            // 追加观察并抬高隔离版本。内容（清单或会话）变化才产生新的身份。
            let remote_observation_id = quarantine_observation_identity(
                bound.remote_observation_id(),
                observation.manifest_digest,
                observation.session_binding_digest,
            );
            Ok(Ok(QuarantineReobservation {
                remote_observation_id,
                observed_at: bound.observed_at(),
                observation,
                evidence_ref: None,
            }))
        })
    }
}

/// 隔离内观察的稳定远端身份：只由远端原身份、清单摘要与会话绑定摘要派生。
fn quarantine_observation_identity(
    remote: Uuid,
    manifest_digest: [u8; 32],
    session_binding_digest: [u8; 32],
) -> String {
    let mut input = Vec::with_capacity(16 + 32 + 32 + 56);
    input.extend_from_slice(b"auction/consignment/quarantine-observation-identity/v1\0");
    input.extend_from_slice(remote.as_bytes());
    input.extend_from_slice(&manifest_digest);
    input.extend_from_slice(&session_binding_digest);
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &input).to_string()
}
