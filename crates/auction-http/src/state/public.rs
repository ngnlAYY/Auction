//! Public HTTP surface 的应用端口状态容器。

use std::sync::Arc;

use auction_application::auction::{
    AuctionReader, CompetitionCommandPort, PublicAccountReader, PublicBidLogReader,
    PublicLotReader, PublicPriceHistoryReader, WorldReader,
};
use auction_application::forum::PublicForumReader;
use auction_application::operations::ProcessReadinessPort;
use auction_application::public::{
    PublicBlacklistReader, PublicCompensationReader, PublicEquipmentLinkResolver,
    PublicRulesReader, PublicSessionPort,
};
use auction_application::realtime::PublicRealtimeReader;
use auction_contracts::PublicRegistrationInfoResponse;
use auction_observability::ProcessMetrics;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

use super::super::HttpBoundary;
use crate::middleware::security::{AnonymousReadGate, PublicHttpSecurity};
use crate::realtime_hub::{RealtimeHub, RealtimeSubscriptionError};

const MAX_PUBLIC_DISCOVERY_CONNECTIONS: usize = 128;

/// Public surface 可使用的应用端口集合。
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
    equipment_links: Option<Arc<dyn PublicEquipmentLinkResolver>>,
    bid_log: Option<Arc<dyn PublicBidLogReader>>,
    price_history: Option<Arc<dyn PublicPriceHistoryReader>>,
    realtime: Option<Arc<dyn PublicRealtimeReader>>,
    account: Option<Arc<dyn PublicAccountReader>>,
    session: Option<Arc<dyn PublicSessionPort>>,
    registration_info: Option<Arc<PublicRegistrationInfoResponse>>,
    security: PublicHttpSecurity,
    watches: Option<Arc<dyn auction_application::public::LotWatchPort>>,
    watch_preference: Option<Arc<dyn auction_application::public::LotWatchPreferencePort>>,
    commands: Option<Arc<dyn CompetitionCommandPort>>,
    metrics: Option<Arc<ProcessMetrics>>,
    runtime_switch_snapshot: Option<auction_application::runtime_switches::RuntimeSwitches>,
    readiness: Option<Arc<dyn ProcessReadinessPort>>,
    realtime_discovery_connections: Arc<Semaphore>,
    realtime_hub: Option<Arc<RealtimeHub>>,
    realtime_shutdown: watch::Sender<bool>,
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
            equipment_links: None,
            bid_log: None,
            price_history: None,
            realtime: None,
            account: None,
            session: None,
            registration_info: None,
            security: PublicHttpSecurity::default(),
            watches: None,
            watch_preference: None,
            commands: None,
            metrics: None,
            runtime_switch_snapshot: None,
            readiness: None,
            realtime_discovery_connections: Arc::new(Semaphore::new(
                MAX_PUBLIC_DISCOVERY_CONNECTIONS,
            )),
            realtime_hub: None,
            realtime_shutdown: watch::channel(false).0,
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
        equipment_links: Option<Arc<dyn PublicEquipmentLinkResolver>>,
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
            equipment_links,
            bid_log,
            price_history,
            realtime,
            account,
            session,
            registration_info: None,
            security,
            watches: None,
            watch_preference: None,
            commands,
            metrics: None,
            runtime_switch_snapshot: None,
            readiness: None,
            realtime_discovery_connections: Arc::new(Semaphore::new(
                MAX_PUBLIC_DISCOVERY_CONNECTIONS,
            )),
            realtime_hub: None,
            realtime_shutdown: watch::channel(false).0,
        }
    }

    /// 仅注入已校验的公开 bot 名单，不把完整运行配置或凭据交给 HTTP 层。
    pub fn with_registration_info(mut self, bot_players: Vec<String>) -> Self {
        self.registration_info = Some(Arc::new(PublicRegistrationInfoResponse { bot_players }));
        self
    }

    pub(crate) fn registration_info(&self) -> Option<&PublicRegistrationInfoResponse> {
        self.registration_info.as_deref()
    }

    /// 注入当前用户的权威装备标记端口；缺失时明确返回 503。
    pub fn with_lot_watches(
        mut self,
        watches: Arc<dyn auction_application::public::LotWatchPort>,
    ) -> Self {
        self.watches = Some(watches);
        self
    }

    /// 注入账号级警告渠道；不影响旧标记兼容字段。
    pub fn with_lot_watch_preference(
        mut self,
        port: Arc<dyn auction_application::public::LotWatchPreferencePort>,
    ) -> Self {
        self.watch_preference = Some(port);
        self
    }

    pub(crate) fn watch_preference(
        &self,
    ) -> Option<&dyn auction_application::public::LotWatchPreferencePort> {
        self.watch_preference.as_deref()
    }

    pub(crate) fn watches(&self) -> Option<&dyn auction_application::public::LotWatchPort> {
        self.watches.as_deref()
    }

    /// 注入进程本地 Prometheus 指标注册表；指标不属于任何业务端口。
    pub fn with_metrics(mut self, metrics: Arc<ProcessMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 注入进程共享的运行时开关快照，供指标边界在请求入口读取。
    pub fn with_runtime_switch_snapshot(
        mut self,
        switches: auction_application::runtime_switches::RuntimeSwitches,
    ) -> Self {
        self.runtime_switch_snapshot = Some(switches);
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

    /// 停止新订阅，并通知已受理的 Public WebSocket 进入旧版 1001 有界关闭流程。
    pub fn begin_realtime_shutdown(&self) {
        if let Some(hub) = self.realtime_hub.as_ref() {
            hub.begin_shutdown();
        }
        self.realtime_shutdown.send_replace(true);
    }

    /// 在停止接收且 HTTP 排空后等待全部 WebSocket 释放配额，覆盖发现流及已受理的升级任务。
    /// 必须先调用 begin_realtime_shutdown；本方法不另建超时，沿用进程统一 drain 预算。
    pub async fn drain_realtime_connections(&self) -> Result<(), RealtimeSubscriptionError> {
        if let Some(hub) = self.realtime_hub.as_ref() {
            hub.drain().await?;
        }
        Ok(())
    }

    /// 返回与本 state 同源的协议边界，供路由装配时建立全局 middleware。
    pub fn http_boundary(&self) -> HttpBoundary {
        let boundary = self.security.boundary();
        let boundary = match &self.metrics {
            Some(metrics) => boundary.with_metrics(metrics.http()),
            None => boundary,
        };
        match &self.runtime_switch_snapshot {
            Some(switches) => boundary.with_runtime_switches(switches.clone()),
            None => boundary,
        }
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

    pub(crate) fn equipment_links(&self) -> Option<&dyn PublicEquipmentLinkResolver> {
        self.equipment_links.as_deref()
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

    /// 将同一会话服务交给长连接；连接只读复核，不持有整个 HTTP state。
    pub(crate) fn session_handle(&self) -> Option<Arc<dyn PublicSessionPort>> {
        self.session.clone()
    }

    pub(crate) fn commands(&self) -> Option<&dyn CompetitionCommandPort> {
        self.commands.as_deref()
    }

    pub(crate) fn security(&self) -> &PublicHttpSecurity {
        &self.security
    }

    /// 供路由装配获取匿名读闸门句柄；不暴露其余安全策略。
    pub fn anonymous_read_gate(&self) -> AnonymousReadGate {
        self.security.anonymous_read_gate()
    }

    pub(crate) fn metrics(&self) -> Option<&ProcessMetrics> {
        self.metrics.as_deref()
    }

    pub(crate) fn readiness(&self) -> Option<&dyn ProcessReadinessPort> {
        self.readiness.as_deref()
    }

    pub(crate) fn try_acquire_realtime_discovery(&self) -> Option<OwnedSemaphorePermit> {
        self.realtime_discovery_connections.clone().try_acquire_owned().ok()
    }

    pub(crate) fn realtime_hub(&self) -> Option<Arc<RealtimeHub>> {
        self.realtime_hub.as_ref().map(Arc::clone)
    }

    pub(crate) fn realtime_shutdown(&self) -> watch::Receiver<bool> {
        self.realtime_shutdown.subscribe()
    }
}
