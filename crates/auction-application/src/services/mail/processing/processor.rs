//! MoogleMail 处理器的配置、装配和稳定公共出口。
//!
//! 路由、装备入站和 delivery outbox 编排分别位于子模块；这里保留处理器状态与构造入口，
//! 使 Worker 仍通过原有 `MailProcessorService`/`MailProcessorConfig` 路径接入。

use std::sync::Arc;

use super::*;

const AUCTION_OPERATION: &str = "auction-ingest";
const DELIVERY_RETURN_OPERATION: &str = "delivery-return";
const DELIVERY_TAKE_OPERATION: &str = "delivery-take";
const DELIVERY_NOTICE_OPERATION: &str = "delivery-notice";

#[path = "processor/auction.rs"]
mod auction;
#[path = "processor/delivery.rs"]
mod delivery;
#[path = "processor/routing.rs"]
mod routing;

/// MoogleMail 业务处理器配置。
#[derive(Debug, Clone)]
pub struct MailProcessorConfig {
    pub start_price: i64,
    pub equipment_filter: MailEquipmentFilter,
}

/// MoogleMail 业务处理器。
///
/// 拍卖、注册和换绑命令都先交给对应 owner，再通过无密文 delivery outbox 安排外部动作；
/// 结算退回邮件只记录可审计证据，由 Recovery/Settlement owner 后续消费。
pub struct MailProcessorService<E, A, D, U, C> {
    world_ids: Vec<String>,
    start_price: i64,
    equipment_filter: MailEquipmentFilter,
    equipment: Arc<E>,
    auctions: Arc<A>,
    deliveries: Arc<D>,
    users: Arc<U>,
    clock: Arc<C>,
}

impl<E, A, D, U, C> MailProcessorService<E, A, D, U, C>
where
    E: MailEquipmentReader,
    A: MailAuctionIntakePort,
    D: MailDeliveryEnqueuer,
    U: MailUserPort,
    C: Clock,
{
    /// 建立单世界 MoogleMail 业务处理器；保留该入口兼容单世界装配。
    pub fn new(
        world_id: String,
        config: MailProcessorConfig,
        equipment: Arc<E>,
        auctions: Arc<A>,
        deliveries: Arc<D>,
        users: Arc<U>,
        clock: Arc<C>,
    ) -> Result<Self, MailProcessingError> {
        Self::new_for_worlds(vec![world_id], config, equipment, auctions, deliveries, users, clock)
    }

    /// 建立可安全处理多个 MoogleMail 世界的共享业务处理器。
    pub fn new_for_worlds(
        world_ids: Vec<String>,
        config: MailProcessorConfig,
        equipment: Arc<E>,
        auctions: Arc<A>,
        deliveries: Arc<D>,
        users: Arc<U>,
        clock: Arc<C>,
    ) -> Result<Self, MailProcessingError> {
        if world_ids.is_empty()
            || world_ids.iter().any(|world_id| !matches!(world_id.as_str(), "main" | "isekai"))
            || world_ids
                .iter()
                .enumerate()
                .any(|(index, world_id)| world_ids[..index].contains(world_id))
            || config.start_price <= 0
            || config.equipment_filter.validate().is_err()
        {
            return Err(MailProcessingError::InvalidConfiguration);
        }
        Ok(Self {
            world_ids,
            start_price: config.start_price,
            equipment_filter: config.equipment_filter,
            equipment,
            auctions,
            deliveries,
            users,
            clock,
        })
    }
}

impl<E, A, D, U, C> MailProcessor for MailProcessorService<E, A, D, U, C>
where
    E: MailEquipmentReader + 'static,
    A: MailAuctionIntakePort + 'static,
    D: MailDeliveryEnqueuer + 'static,
    U: MailUserPort + 'static,
    C: Clock + 'static,
{
    fn process(&self, message: MailMessage) -> ApplicationFuture<'_, MailProcessingDecision> {
        Box::pin(async move { self.process_message(message).await })
    }
}
