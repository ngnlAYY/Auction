//! MoogleMail 处理器的配置、装配和稳定公共出口。

use std::sync::Arc;

use crate::ports::core::Clock;
use crate::ports::mail::{
    MailAuctionIntakePort, MailDeliveryEnqueuer, MailEquipmentFilter, MailEquipmentReader,
    MailMessage, MailProcessingError, MailProcessor, MailUserPort,
};
use crate::ports::mail::{MailReplyPolicy, MailReplyPolicySource};

const AUCTION_OPERATION: &str = "auction-ingest";
const DELIVERY_RETURN_OPERATION: &str = "delivery-return";
const DELIVERY_TAKE_OPERATION: &str = "delivery-take";
const DELIVERY_NOTICE_OPERATION: &str = "delivery-notice";

mod auction;
mod delivery;
mod routing;

#[derive(Debug, Clone)]
pub struct MailProcessorConfig {
    pub start_price: i64,
    pub equipment_filter: MailEquipmentFilter,
    /// 是否接受 `rekey` 主题；关闭时按未知邮件处理，用于回退。
    pub rekey_enabled: bool,
    /// 注册/轮换回信与说明信预算的唯一来源；每封邮件处理时读取一份完整、已校验的快照，
    /// 不保留第二份启动期预算。
    pub reply_policy: Arc<dyn MailReplyPolicySource>,
}

/// MoogleMail 业务处理器。
/// 拍卖、注册、轮换和换绑命令都先交给对应 owner，再通过无密文 delivery outbox 安排外部动作；
/// 结算退回邮件只记录可审计证据，由 Recovery/Settlement owner 后续消费。
pub struct MailProcessorService<E, A, D, U, C> {
    world_ids: Vec<String>,
    start_price: i64,
    equipment_filter: MailEquipmentFilter,
    rekey_enabled: bool,
    reply_policy: Arc<dyn MailReplyPolicySource>,
    metrics: Arc<dyn crate::ports::observability::external_metrics::ExternalMetricsObserver>,
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
    /// 建立可安全处理多个 MoogleMail 世界的共享业务处理器。
    pub fn new(
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
            // 默认起拍价必须落在合法出价区间；超出上限会在录入后形成无法被任何出价命中的拍品。
            || !(1..=auction_domain::MAX_BID_AMOUNT).contains(&config.start_price)
            || config.equipment_filter.validate().is_err()
            // 回信预算与启动配置、runtime 校验同一区间；来源实现必须返回完整快照，
            // 这里在装配时拒绝越界初值，避免零窗口或超上限的说明信预算进入处理循环。
            || !config.reply_policy.current().is_valid()
        {
            return Err(MailProcessingError::InvalidConfiguration);
        }
        Ok(Self {
            world_ids,
            start_price: config.start_price,
            equipment_filter: config.equipment_filter,
            rekey_enabled: config.rekey_enabled,
            reply_policy: config.reply_policy,
            metrics: crate::ports::observability::external_metrics::noop_external_metrics_observer(
            ),
            equipment,
            auctions,
            deliveries,
            users,
            clock,
        })
    }

    /// 替换外部指标观察者；发送预算抑制事件与请求指标共用同一闭集。
    pub fn with_external_metrics(
        mut self,
        metrics: Arc<dyn crate::ports::observability::external_metrics::ExternalMetricsObserver>,
    ) -> Self {
        self.metrics = metrics;
        self
    }

    /// 每次处理邮件读取一份完整预算策略。
    pub(super) fn reply_policy(&self) -> MailReplyPolicy {
        self.reply_policy.current()
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
    fn process(&self, message: MailMessage) -> crate::ports::mail::MailProcessingFuture<'_> {
        Box::pin(async move {
            let mut pending_equipment_diagnostic = None;
            let result = self.process_message(message, &mut pending_equipment_diagnostic).await;
            crate::ports::mail::MailProcessingAttempt { result, pending_equipment_diagnostic }
        })
    }

    fn route(
        &self,
        message: MailMessage,
        consignment_candidate: bool,
    ) -> crate::ports::mail::MailRoutingFuture<'_> {
        Box::pin(async move {
            let mut pending_equipment_diagnostic = None;
            self.route_message(message, consignment_candidate, &mut pending_equipment_diagnostic)
                .await
        })
    }
}
