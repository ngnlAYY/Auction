//! 派发、补寄、退件 Take 三个 executor 与订单复核消费端的运行时装配：工作来源与传输适配只委托存储与既有 MoogleMail
//! 传输，许可只由应用层用例铸造。
//!
//! 不变量 4、5：三个 executor 共用 Worker 已装配的同一个 MoogleMail delivery transport 与同一组
//! gateway（凭据独占锁与发送器状态只有一份）；租约与 fence 全部持久在数据库，暂停外部访问时不领取、
//! 不外写。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{DispatchService, DispatchWorkStore, ReturnService};
use auction_application::core::Clock;
use auction_application::settlement::SettlementInternalEvidenceReader;
use auction_integrations::mooglemail::MoogleMailDeliveryTransport;
use auction_jobs::consignment::{
    COLLECTION_PAGE_SIZE, CollectionRuntime, ConsignmentDispatchRuntime, DownstreamRoundRuntime,
    OrderRecheckRuntime, RedispatchRuntime, ReturnTakeRuntime,
};
use auction_storage::{
    CollectedRoundRepository, DatabaseStore, DispatchWorkRepository, DownstreamRoundRepository,
};
use tokio::sync::Notify;

use super::collection::{MoogleMailCollectionObserver, StoreCollections};
use super::content::FrozenFirstDispatchContent;
use super::dispatch::MoogleMailDispatchTransport;
use super::recheck::MoogleMailDispatchSentReader;
use super::return_take::MoogleMailReturnTakeTransport;
use super::transport::MoogleMailConsignmentTransport;
use super::work::{
    StoreDispatchWork, StoreDownstreamRounds, StoreRedispatchWork, StoreReturnTakeWork,
};

/// 装配三个 executor 所需的共享依赖。
pub struct ExecutorInputs {
    /// 同一数据库实例。
    pub store: Arc<DatabaseStore>,
    /// Worker 已装配的 delivery transport（与通用投递、权利通知同一实例）。
    pub mail: Arc<MoogleMailDeliveryTransport>,
    /// 与期 4 处置维护同一组 gateway 的处置传输。
    pub disposition: Arc<MoogleMailConsignmentTransport>,
    /// 时刻来源。
    pub clock: Arc<dyn Clock>,
    /// 内部证据入口：Worker 已装配的同一个 MoogleMailGateway（严格现金收款认证读 main 现金）。
    pub evidence: Arc<dyn SettlementInternalEvidenceReader>,
    /// 当前 Worker 实例名。
    pub worker_id: String,
    /// 每次领取的持久租约时长（也作写前准备的外部截止）。
    pub lease_duration: Duration,
    /// 外部访问暂停快照。
    pub paused: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// 三个 executor 的运行时。
pub struct ExecutorRuntimes {
    /// 派发 executor。
    pub dispatch: Arc<ConsignmentDispatchRuntime>,
    /// 补寄消费端。
    pub redispatch: Arc<RedispatchRuntime>,
    /// 下游首轮消费端（isekai 原件交割、卖款、退款）。
    pub downstream_rounds: Arc<DownstreamRoundRuntime>,
    /// 退件 Take executor。
    pub return_takes: Arc<ReturnTakeRuntime>,
    /// 订单复核消费端（只读 Sent，经既有写口补记）。
    pub order_rechecks: Arc<OrderRecheckRuntime>,
    /// 收款与确领推进循环（只读 Sent，经既有写口记录证据、认证与确领）。
    pub collections: Arc<CollectionRuntime>,
}

/// 装配派发、补寄、退件 Take 三个 executor 与订单复核消费端。
pub fn executor_runtimes(inputs: ExecutorInputs) -> ExecutorRuntimes {
    let ExecutorInputs {
        store,
        mail,
        disposition,
        clock,
        evidence,
        worker_id,
        lease_duration,
        paused,
    } = inputs;
    // 补寄新轮在存储备好事务里渲染，内容来源与 Public 购买冻结首轮是同一实现。
    let work: Arc<dyn DispatchWorkStore> = Arc::new(DispatchWorkRepository::new(
        store.as_ref().clone(),
        Arc::new(FrozenFirstDispatchContent),
    ));
    let service = Arc::new(DispatchService::new(store.clone()));
    let dispatch = Arc::new(ConsignmentDispatchRuntime {
        work: Arc::new(StoreDispatchWork {
            store: store.clone(),
            work: work.clone(),
            clock: clock.clone(),
        }),
        service: service.clone(),
        transport: Arc::new(MoogleMailDispatchTransport {
            work: work.clone(),
            transport: mail.clone(),
            clock: clock.clone(),
        }),
        worker_id: worker_id.clone(),
        lease_duration,
        wakeup: Arc::new(Notify::new()),
        paused: paused.clone(),
    });
    let redispatch = Arc::new(RedispatchRuntime {
        source: Arc::new(StoreRedispatchWork { work }),
        service: service.clone(),
        wakeup: Arc::new(Notify::new()),
    });
    // 下游首轮在存储备好事务里渲染，内容来源与购买首轮、补寄新轮是同一实现。
    let downstream_rounds = Arc::new(DownstreamRoundRuntime {
        source: Arc::new(StoreDownstreamRounds {
            repository: DownstreamRoundRepository::new(
                store.as_ref().clone(),
                Arc::new(FrozenFirstDispatchContent),
            ),
        }),
        service: service.clone(),
        wakeup: Arc::new(Notify::new()),
    });
    // 收款与确领推进只读 Sent；验证器未签收时不读、不推进（签收清单与写口同源）。
    let collections = Arc::new(CollectionRuntime {
        source: Arc::new(StoreCollections {
            repository: CollectedRoundRepository::new(store.as_ref().clone()),
        }),
        observer: Arc::new(MoogleMailCollectionObserver {
            transport: mail.clone(),
            clock: clock.clone(),
            evidence,
        }),
        service: service.clone(),
        funding: store.clone(),
        wakeup: Arc::new(Notify::new()),
        paused: paused.clone(),
        page_size: COLLECTION_PAGE_SIZE,
    });
    let order_rechecks = Arc::new(OrderRecheckRuntime {
        store: store.clone(),
        reader: Arc::new(MoogleMailDispatchSentReader { transport: mail, clock: clock.clone() }),
        service,
        clock: clock.clone(),
        worker_id: worker_id.clone(),
        lease_duration,
        wakeup: Arc::new(Notify::new()),
        paused: paused.clone(),
    });
    let return_takes = Arc::new(ReturnTakeRuntime {
        work: Arc::new(StoreReturnTakeWork { store: store.clone(), clock }),
        service: Arc::new(ReturnService::new(store.clone())),
        transport: Arc::new(MoogleMailReturnTakeTransport {
            store,
            disposition,
            deadline: lease_duration,
        }),
        worker_id,
        lease_duration,
        wakeup: Arc::new(Notify::new()),
        paused,
    });
    ExecutorRuntimes {
        dispatch,
        redispatch,
        downstream_rounds,
        return_takes,
        order_rechecks,
        collections,
    }
}
