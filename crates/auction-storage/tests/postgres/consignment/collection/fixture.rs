//! 收款与确领推进循环的测试夹具（PIWQ）：回环观察替身（只计读次数、按开关回答已领取或未领取，声明的验证器版本
//! 由用例给出）与委托存储的待办来源；在两世界贯通夹具 `Order` 上组装生产循环。替身没有任何外写能力，夹具值都是
//! 明显的假值，不连接任何外部服务。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use auction_application::consignment::{
    CollectionCursor, CollectionPage, ConsignmentFuture, DispatchService, PendingCollection,
};
use auction_application::settlement::FreshCashObservation;
use auction_domain::consignment::dispatch::EvidenceCategory;
use auction_jobs::consignment::{
    COLLECTION_PAGE_SIZE, CollectionObservation, CollectionObserver, CollectionRead,
    CollectionRound, CollectionRuntime, CollectionSource,
};
use auction_storage::{CollectedRoundRepository, DatabaseStore};
use auction_time::Timestamp;
use tokio::sync::Notify;

use super::super::acceptance::chain::Order;
use super::super::dispatch::carrier_funding::BALANCE;
use super::super::funding::cod_fixture::cash;
use super::super::returns::fixture::SIGNED_VALIDATOR;

/// 生产观察端口声明的未签收版本（与 `auction-integrations` 的常量同值）。
pub(in crate::consignment) const UNSIGNED_COLLECTION_VALIDATOR: &str =
    "consignment_dispatch_collected_v1-unsigned";

/// 回环观察替身：每次观察计一次读；`collected` 决定回答已领取还是仍有剩余，`only_mid` 给出时只有该 Sent MID
/// 算已领取。
pub(in crate::consignment) struct Collector {
    version: &'static str,
    collected: AtomicBool,
    only_mid: Option<&'static str>,
    reads: AtomicUsize,
}

/// 替身对某封 Sent 的证据摘要：只由 Sent MID 决定，同一领取事实重读得到同一摘要。
pub(in crate::consignment) fn collected_digest(sent_mid: &str) -> [u8; 32] {
    let mut digest = [0xc0; 32];
    for (slot, byte) in digest.iter_mut().zip(sent_mid.bytes()) {
        *slot = byte;
    }
    digest
}

impl Collector {
    /// 以 `test-db` 构建认的已签收版本观察，Sent 已被领取。
    pub(in crate::consignment) fn signed() -> Arc<Self> {
        Self::with_version(SIGNED_VALIDATOR)
    }

    /// 以给定验证器版本观察，Sent 已被领取。
    pub(in crate::consignment) fn with_version(version: &'static str) -> Arc<Self> {
        Arc::new(Self {
            version,
            collected: AtomicBool::new(true),
            only_mid: None,
            reads: AtomicUsize::new(0),
        })
    }

    /// 已签收版本，只有 `sent_mid` 这一封已被领取，其余仍有剩余。
    pub(in crate::consignment) fn collecting_only(sent_mid: &'static str) -> Arc<Self> {
        Arc::new(Self {
            version: SIGNED_VALIDATOR,
            collected: AtomicBool::new(true),
            only_mid: Some(sent_mid),
            reads: AtomicUsize::new(0),
        })
    }

    /// 切换远端是否已领取。
    pub(in crate::consignment) fn set_collected(&self, collected: bool) {
        self.collected.store(collected, Ordering::SeqCst);
    }

    /// 已读取的次数。
    pub(in crate::consignment) fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

impl CollectionObserver for Collector {
    fn validator_version(&self) -> &str {
        self.version
    }

    fn observe<'a>(
        &'a self,
        pending: &'a PendingCollection,
    ) -> ConsignmentFuture<'a, CollectionRead> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let collected = self.collected.load(Ordering::SeqCst)
            && self.only_mid.is_none_or(|mid| mid == pending.sent_mid);
        let read = if collected {
            CollectionRead::Collected(CollectionObservation {
                remote_observation_id: pending.sent_mid.clone(),
                observed_at: Timestamp::now(),
                evidence_digest: collected_digest(&pending.sent_mid),
            })
        } else {
            CollectionRead::Pending
        };
        Box::pin(async move { Ok(read) })
    }

    fn fresh_main_cash<'a>(
        &'a self,
        _uid: &'a str,
    ) -> ConsignmentFuture<'a, Option<FreshCashObservation>> {
        Box::pin(async move { Ok(Some(cash(BALANCE))) })
    }
}

/// 委托存储的待办来源（与 Worker 装配同一实现方式）。
struct StoreSource(CollectedRoundRepository);

impl CollectionSource for StoreSource {
    fn pending_collections(
        &self,
        after: Option<CollectionCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, CollectionPage> {
        Box::pin(self.0.pending(after, limit))
    }

    fn validator_signed(&self, category: EvidenceCategory, version: &str) -> bool {
        self.0.validator_signed(category, version)
    }
}

/// 以运行账号与给定观察替身、页大小组装生产推进循环（真实证据 / 认证 / 确领写口）。
pub(in crate::consignment) fn collection_runtime(
    store: &DatabaseStore,
    service: Arc<DispatchService>,
    collector: &Arc<Collector>,
    page_size: u32,
) -> CollectionRuntime {
    CollectionRuntime {
        source: Arc::new(StoreSource(CollectedRoundRepository::new(store.clone()))),
        observer: collector.clone(),
        service,
        funding: Arc::new(store.clone()),
        wakeup: Arc::new(Notify::new()),
        paused: Arc::new(|| false),
        page_size,
    }
}

impl Order {
    /// 以给定观察替身组装生产推进循环，跑一次扫描（生产页大小）。每次调用都是新的运行时，等同 Worker 重启。
    pub(in crate::consignment) async fn collect_with(
        &self,
        collector: &Arc<Collector>,
    ) -> CollectionRound {
        collection_runtime(&self.runtime, self.service.clone(), collector, COLLECTION_PAGE_SIZE)
            .run_once()
            .await
    }

    /// 以已签收的测试验证器、远端已领取跑一页推进循环。
    pub(in crate::consignment) async fn run_collections(&self) -> CollectionRound {
        self.collect_with(&Collector::signed()).await
    }

    /// 已起写的 Take 数（含代管夹具自身的取件；推进循环不得新增）。
    pub(in crate::consignment) async fn take_write_starts(&self) -> i64 {
        self.count("mail_disposition_write_starts").await
    }
}
