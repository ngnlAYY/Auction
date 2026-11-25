//! 收款与确领推进循环的装配适配（PIWQ）：待办来源只委托存储，领取观察只经 MoogleMail 投递传输的只读入口。
//!
//! 不变量 4、6、7：观察只读，不取写锁、不 POST、不 Take；冻结 Send 身份的世界与轮的发信世界不一致时不读，按
//! “无法确定”等待。严格现金收款认证所需的新鲜 main 现金观察本期没有只读入口（资金证据的用途闭集只有派发领取、
//! 结算领取与对账三类），一律返回 `None`：严格现金的认证照旧等待，记账模式不读余额、不受影响。收件人与主题只
//! 进入本次比对，不进日志。

use std::sync::Arc;

use auction_application::consignment::{
    CollectionCursor, CollectionPage, ConsignmentFuture, PendingCollection,
};
use auction_application::core::Clock;
use auction_application::settlement::FreshCashObservation;
use auction_domain::consignment::dispatch::EvidenceCategory;
use auction_integrations::mooglemail::{
    CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION, ConsignmentCollection,
    ConsignmentCollectionTarget, MoogleMailDeliveryTransport, MoogleMailWorld,
};
use auction_jobs::consignment::{
    CollectionObservation, CollectionObserver, CollectionRead, CollectionSource,
};
use auction_storage::CollectedRoundRepository;

/// 存储待办来源。
pub struct StoreCollections {
    /// 收款与确领待办的存储读口。
    pub repository: CollectedRoundRepository,
}

impl CollectionSource for StoreCollections {
    fn pending_collections(
        &self,
        after: Option<CollectionCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, CollectionPage> {
        Box::pin(self.repository.pending(after, limit))
    }

    fn validator_signed(&self, category: EvidenceCategory, version: &str) -> bool {
        self.repository.validator_signed(category, version)
    }
}

/// 经 Worker 已装配的投递传输只读观察领取状态。
pub struct MoogleMailCollectionObserver {
    /// 与派发同一个 transport 实例（只用其只读入口）。
    pub transport: Arc<MoogleMailDeliveryTransport>,
    /// 观察时刻来源。
    pub clock: Arc<dyn Clock>,
}

impl CollectionObserver for MoogleMailCollectionObserver {
    fn validator_version(&self) -> &str {
        CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION
    }

    fn observe<'a>(
        &'a self,
        pending: &'a PendingCollection,
    ) -> ConsignmentFuture<'a, CollectionRead> {
        Box::pin(async move {
            let world = MoogleMailWorld::from(pending.send_world);
            let expectation = &pending.expectation;
            if expectation.world_id != world.as_str() {
                tracing::warn!(
                    code = "collection_identity_mismatch",
                    "收款确领的冻结 Send 身份与发信世界不符，不读取"
                );
                return Ok(CollectionRead::Unavailable);
            }
            let target = ConsignmentCollectionTarget {
                world,
                sent_mid: pending.sent_mid.clone(),
                sending_at: pending.sending_at,
                recipient: expectation.recipient.clone(),
                subject: expectation.subject.clone(),
                body_digest: expectation.body_digest,
                credits: expectation.credits,
                cod: expectation.cod,
                equipment_ids: expectation.equipment_ids.clone(),
            };
            Ok(match self.transport.observe_consignment_collection(&target).await {
                // 版本必须就是本观察端口声明的版本：循环据声明的版本判签收并记入证据。
                ConsignmentCollection::Collected { validator_version, evidence_digest }
                    if validator_version == self.validator_version() =>
                {
                    CollectionRead::Collected(CollectionObservation {
                        remote_observation_id: pending.sent_mid.clone(),
                        observed_at: self.clock.now(),
                        evidence_digest,
                    })
                }
                ConsignmentCollection::Pending => CollectionRead::Pending,
                ConsignmentCollection::Collected { .. } | ConsignmentCollection::Unavailable => {
                    CollectionRead::Unavailable
                }
            })
        })
    }

    fn fresh_main_cash<'a>(
        &'a self,
        _uid: &'a str,
    ) -> ConsignmentFuture<'a, Option<FreshCashObservation>> {
        Box::pin(async move {
            tracing::warn!(
                code = "collection_cash_unavailable",
                "严格现金收款认证没有只读现金观察入口，等待"
            );
            Ok(None)
        })
    }
}
