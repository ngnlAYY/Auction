//! MoogleMail 处理器的配置、装配和稳定公共出口。

use std::sync::Arc;

use super::*;
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
    /// 同一账号在该窗口内最多回信一次的注册/轮换限制。
    pub account_reply_window_seconds: u64,
    /// 说明信（拒收、换绑结果、论坛回执）的 24 小时窗口。
    pub explanation_window_seconds: u64,
    /// 说明信窗口内的最大封数；0 表示不限制。
    pub explanation_limit: u32,
    /// 是否接受 `rekey` 主题；关闭时按未知邮件处理，用于回退。
    pub rekey_enabled: bool,
}

/// MoogleMail 业务处理器。
/// 拍卖、注册、轮换和换绑命令都先交给对应 owner，再通过无密文 delivery outbox 安排外部动作；
/// 结算退回邮件只记录可审计证据，由 Recovery/Settlement owner 后续消费。
pub struct MailProcessorService<E, A, D, U, C> {
    world_ids: Vec<String>,
    start_price: i64,
    equipment_filter: MailEquipmentFilter,
    account_reply_window_seconds: u64,
    explanation_window_seconds: u64,
    explanation_limit: u32,
    rekey_enabled: bool,
    reply_policy: Option<Arc<dyn MailReplyPolicySource>>,
    metrics: Arc<dyn crate::ports::external_metrics::ExternalMetricsObserver>,
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
            || !(60..=604_800).contains(&config.account_reply_window_seconds)
            || !(60..=604_800).contains(&config.explanation_window_seconds)
            || config.explanation_limit > 100
        {
            return Err(MailProcessingError::InvalidConfiguration);
        }
        Ok(Self {
            world_ids,
            start_price: config.start_price,
            equipment_filter: config.equipment_filter,
            account_reply_window_seconds: config.account_reply_window_seconds,
            explanation_window_seconds: config.explanation_window_seconds,
            explanation_limit: config.explanation_limit,
            rekey_enabled: config.rekey_enabled,
            reply_policy: None,
            metrics: crate::ports::external_metrics::noop_external_metrics_observer(),
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
        metrics: Arc<dyn crate::ports::external_metrics::ExternalMetricsObserver>,
    ) -> Self {
        self.metrics = metrics;
        self
    }

    /// 注入只读预算来源；未注入时沿用启动配置。
    pub fn with_reply_policy_source(mut self, source: Arc<dyn MailReplyPolicySource>) -> Self {
        self.reply_policy = Some(source);
        self
    }

    /// 每次处理邮件读取一份完整预算策略；未注入来源时沿用启动配置。
    pub(super) fn reply_policy(&self) -> MailReplyPolicy {
        self.reply_policy.as_ref().map_or(
            MailReplyPolicy {
                account_reply_window_seconds: self.account_reply_window_seconds,
                explanation_mail_window_seconds: self.explanation_window_seconds,
                explanation_mail_limit: self.explanation_limit,
            },
            |source| source.current(),
        )
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
}
