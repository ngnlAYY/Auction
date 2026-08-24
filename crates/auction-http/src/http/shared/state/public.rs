//! Public HTTP surface 的应用端口状态容器。

use std::sync::Arc;

use auction_application::{
    AuctionReader, CompetitionCommandPort, ProcessReadinessPort, PublicAccountReader,
    PublicBidLogReader, PublicBlacklistReader, PublicCompensationReader, PublicForumReader,
    PublicLotReader, PublicPriceHistoryReader, PublicRealtimeReader, PublicRulesReader,
    PublicSessionPort, WorldReader,
};
use auction_observability::ProcessMetrics;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::super::HttpBoundary;
use super::super::realtime_hub::RealtimeHub;
use super::super::security::PublicHttpSecurity;

const MAX_PUBLIC_REALTIME_CONNECTIONS: usize = 128;

/// Public surface 可使用的应用端口集合。
///
/// 端口缺失时 handler 返回明确的依赖不可用，而不是构造示例数据或 panic；这样无数据库的
/// 编译/启动骨架仍可运行健康检查，配置了数据库时才暴露真实查询。
#[derive(Clone)]
pub struct PublicApiState {
    worlds: Option<Arc<dyn WorldReader>>,
    rules: Option<Arc<dyn PublicRulesReader>>,
    blacklist: Option<Arc<dyn PublicBlacklistReader>>,
    compensation: Option<Arc<dyn PublicCompensationReader>>,
    forum: Option<Arc<dyn PublicForumReader>>,
    auctions: Option<Arc<dyn AuctionReader>>,
    lots: Option<Arc<dyn PublicLotReader>>,
    bid_log: Option<Arc<dyn PublicBidLogReader>>,
    price_history: Option<Arc<dyn PublicPriceHistoryReader>>,
    realtime: Option<Arc<dyn PublicRealtimeReader>>,
    account: Option<Arc<dyn PublicAccountReader>>,
    session: Option<Arc<dyn PublicSessionPort>>,
    security: PublicHttpSecurity,
    commands: Option<Arc<dyn CompetitionCommandPort>>,
    metrics: Option<Arc<ProcessMetrics>>,
    readiness: Option<Arc<dyn ProcessReadinessPort>>,
    realtime_connections: Arc<Semaphore>,
    realtime_hub: Option<Arc<RealtimeHub>>,
}

impl Default for PublicApiState {
    fn default() -> Self {
        Self {
            worlds: None,
            rules: None,
            blacklist: None,
            compensation: None,
            forum: None,
            auctions: None,
            lots: None,
            bid_log: None,
            price_history: None,
            realtime: None,
            account: None,
            session: None,
            security: PublicHttpSecurity::default(),
            commands: None,
            metrics: None,
            readiness: None,
            realtime_connections: Arc::new(Semaphore::new(MAX_PUBLIC_REALTIME_CONNECTIONS)),
            realtime_hub: None,
        }
    }
}

impl PublicApiState {
    /// 建立 Public state。所有具体存储实现由装配根传入。
    // Public 各只读端口保持显式分离，避免用一个跨层大结构体绕过模块 owner。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        worlds: Option<Arc<dyn WorldReader>>,
        rules: Option<Arc<dyn PublicRulesReader>>,
        blacklist: Option<Arc<dyn PublicBlacklistReader>>,
        compensation: Option<Arc<dyn PublicCompensationReader>>,
        forum: Option<Arc<dyn PublicForumReader>>,
        auctions: Option<Arc<dyn AuctionReader>>,
        lots: Option<Arc<dyn PublicLotReader>>,
        bid_log: Option<Arc<dyn PublicBidLogReader>>,
        price_history: Option<Arc<dyn PublicPriceHistoryReader>>,
        realtime: Option<Arc<dyn PublicRealtimeReader>>,
        account: Option<Arc<dyn PublicAccountReader>>,
        commands: Option<Arc<dyn CompetitionCommandPort>>,
        session: Option<Arc<dyn PublicSessionPort>>,
        security: PublicHttpSecurity,
    ) -> Self {
        Self {
            worlds,
            rules,
            blacklist,
            compensation,
            forum,
            auctions,
            lots,
            bid_log,
            price_history,
            realtime,
            account,
            session,
            security,
            commands,
            metrics: None,
            readiness: None,
            realtime_connections: Arc::new(Semaphore::new(MAX_PUBLIC_REALTIME_CONNECTIONS)),
            realtime_hub: None,
        }
    }

    /// 注入进程本地 Prometheus 指标注册表；指标不属于任何业务端口。
    pub fn with_metrics(mut self, metrics: Arc<ProcessMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 注入只读 readiness 检查；未注入时 `/health/ready` 明确返回未就绪。
    pub fn with_readiness(mut self, readiness: Arc<dyn ProcessReadinessPort>) -> Self {
        self.readiness = Some(readiness);
        self
    }

    /// 注入 Public 进程内 realtime Hub；未注入时 WebSocket 明确报告依赖不可用。
    pub fn with_realtime_hub(mut self, hub: Arc<RealtimeHub>) -> Self {
        self.realtime_hub = Some(hub);
        self
    }

    /// 返回与本 state 同源的协议边界，供路由装配时建立全局 middleware。
    pub fn http_boundary(&self) -> HttpBoundary {
        self.security.boundary()
    }

    pub(crate) fn worlds(&self) -> Option<&dyn WorldReader> {
        self.worlds.as_deref()
    }

    pub(crate) fn auctions(&self) -> Option<&dyn AuctionReader> {
        self.auctions.as_deref()
    }

    pub(crate) fn rules(&self) -> Option<&dyn PublicRulesReader> {
        self.rules.as_deref()
    }

    pub(crate) fn blacklist(&self) -> Option<&dyn PublicBlacklistReader> {
        self.blacklist.as_deref()
    }

    pub(crate) fn compensation(&self) -> Option<&dyn PublicCompensationReader> {
        self.compensation.as_deref()
    }

    pub(crate) fn forum(&self) -> Option<&dyn PublicForumReader> {
        self.forum.as_deref()
    }

    pub(crate) fn lots(&self) -> Option<&dyn PublicLotReader> {
        self.lots.as_deref()
    }

    pub(crate) fn bid_log(&self) -> Option<&dyn PublicBidLogReader> {
        self.bid_log.as_deref()
    }

    pub(crate) fn price_history(&self) -> Option<&dyn PublicPriceHistoryReader> {
        self.price_history.as_deref()
    }

    pub(crate) fn realtime(&self) -> Option<&dyn PublicRealtimeReader> {
        self.realtime.as_deref()
    }

    pub(crate) fn account(&self) -> Option<&dyn PublicAccountReader> {
        self.account.as_deref()
    }

    pub(crate) fn session(&self) -> Option<&dyn PublicSessionPort> {
        self.session.as_deref()
    }

    pub(crate) fn commands(&self) -> Option<&dyn CompetitionCommandPort> {
        self.commands.as_deref()
    }

    pub(crate) fn security(&self) -> &PublicHttpSecurity {
        &self.security
    }

    pub(crate) fn metrics(&self) -> Option<&ProcessMetrics> {
        self.metrics.as_deref()
    }

    pub(crate) fn readiness(&self) -> Option<&dyn ProcessReadinessPort> {
        self.readiness.as_deref()
    }

    pub(crate) fn try_acquire_realtime_connection(&self) -> Option<OwnedSemaphorePermit> {
        self.realtime_connections.clone().try_acquire_owned().ok()
    }

    pub(crate) fn realtime_hub(&self) -> Option<Arc<RealtimeHub>> {
        self.realtime_hub.as_ref().map(Arc::clone)
    }
}
