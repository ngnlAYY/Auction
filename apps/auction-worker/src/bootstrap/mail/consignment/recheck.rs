//! 订单复核的只读 Sent 读取适配：把原 attempt 冻结的 Send 身份交给 MoogleMail 投递传输的只读复核入口。
//!
//! 不变量 4、6、7：只读，不取写锁、不 POST；世界与冻结 Send 身份不一致时不读，按“查不到”交完成推导
//! （`still_unknown`）。冻结 Credits 与 COD、附件一起交给复核入口逐项比对（现金轮的载体 1、卖款 S、退款 P）。
//! 收件人与主题只进入本次比对，不进日志。

use std::sync::Arc;

use auction_application::consignment::{ConsignmentFuture, RecheckDispatchAttempt};
use auction_application::core::Clock;
use auction_integrations::mooglemail::{
    ConsignmentDispatchRecheck, ConsignmentDispatchRecheckTarget, MoogleMailDeliveryTransport,
    MoogleMailWorld,
};
use auction_jobs::consignment::{DispatchSentReader, SentObservation};

/// 经 Worker 已装配的投递传输只读复核寄售派发。
pub struct MoogleMailDispatchSentReader {
    /// 与派发同一个 transport 实例（只用其只读入口）。
    pub transport: Arc<MoogleMailDeliveryTransport>,
    /// Sent 观察时刻来源。
    pub clock: Arc<dyn Clock>,
}

impl DispatchSentReader for MoogleMailDispatchSentReader {
    fn find_sent<'a>(
        &'a self,
        attempt: &'a RecheckDispatchAttempt,
    ) -> ConsignmentFuture<'a, Option<SentObservation>> {
        Box::pin(async move {
            let world = MoogleMailWorld::from(attempt.world);
            let expectation = &attempt.expectation;
            // 冻结 Send 身份的世界必须就是轮的世界。
            if expectation.world_id != world.as_str() {
                tracing::warn!(
                    code = "recheck_identity_mismatch",
                    "订单复核的冻结 Send 身份不符，不读取"
                );
                return Ok(None);
            }
            let target = ConsignmentDispatchRecheckTarget {
                world,
                sending_at: attempt.sending_at,
                recipient: expectation.recipient.clone(),
                subject: expectation.subject.clone(),
                body_digest: expectation.body_digest,
                credits: expectation.credits,
                cod: expectation.cod,
                equipment_ids: expectation.equipment_ids.clone(),
            };
            Ok(match self.transport.recheck_consignment_dispatch(&target).await {
                ConsignmentDispatchRecheck::Found(sent) => Some(SentObservation {
                    sent_mid: sent.sent_mid,
                    observed_at: self.clock.now(),
                    validator_version: sent.validator_version.to_owned(),
                    evidence_digest: sent.evidence_digest,
                }),
                ConsignmentDispatchRecheck::NotFound | ConsignmentDispatchRecheck::Unavailable => {
                    None
                }
            })
        })
    }
}
