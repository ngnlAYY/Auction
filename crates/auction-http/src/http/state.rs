//! Public HTTP state。

use std::sync::Arc;

use auction_application::{
    AdminAuctionCreatePort, AdminAuctionReader, AdminAuditPort, AdminForumPreviewPort,
    AdminIntakePort, AdminLotReader, AdminLotUpdatePort, AdminMailPort, AdminProxyEventReader,
    AdminRenameReader, AdminRulesPort, AdminRuntimeConfigPort, AdminSessionPort,
    AuctionLifecycleCommandPort, AuctionReader, CompetitionCommandPort, DeliveryAdminPort,
    PublicAccountReader, PublicBidLogReader, PublicBlacklistReader, PublicCompensationReader,
    PublicForumReader, PublicLotReader, PublicPriceHistoryReader, PublicRealtimeReader,
    PublicRulesReader, PublicSessionPort, RealtimeAdminPort, RecoveryCommandPort,
    RecoveryReportReader, SettlementReportReader, WorldReader,
};
use auction_observability::HealthReader;

use super::security::{AdminHttpSecurity, PublicHttpSecurity};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
    realtime_connections: Arc<Semaphore>,
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
            realtime_connections: Arc::new(Semaphore::new(MAX_PUBLIC_REALTIME_CONNECTIONS)),
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
            realtime_connections: Arc::new(Semaphore::new(MAX_PUBLIC_REALTIME_CONNECTIONS)),
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

    pub(crate) fn try_acquire_realtime_connection(&self) -> Option<OwnedSemaphorePermit> {
        self.realtime_connections.clone().try_acquire_owned().ok()
    }
}

/// Admin surface 可使用的应用端口集合。
///
/// 管理命令、查询和会话以后按 owner 逐项加入；缺失端口时只报告依赖不可用，不构造假数据。
#[derive(Clone, Default)]
pub struct AdminApiState {
    session: Option<Arc<dyn AdminSessionPort>>,
    lifecycle: Option<Arc<dyn AuctionLifecycleCommandPort>>,
    auction_create: Option<Arc<dyn AdminAuctionCreatePort>>,
    admin_audit: Option<Arc<dyn AdminAuditPort>>,
    health: Option<Arc<dyn HealthReader>>,
    admin_intake: Option<Arc<dyn AdminIntakePort>>,
    admin_forum_preview: Option<Arc<dyn AdminForumPreviewPort>>,
    admin_mail: Option<Arc<dyn AdminMailPort>>,
    admin_rules: Option<Arc<dyn AdminRulesPort>>,
    admin_runtime: Option<Arc<dyn AdminRuntimeConfigPort>>,
    admin_renames: Option<Arc<dyn AdminRenameReader>>,
    recovery_commands: Option<Arc<dyn RecoveryCommandPort>>,
    settlement_report: Option<Arc<dyn SettlementReportReader>>,
    recovery_report: Option<Arc<dyn RecoveryReportReader>>,
    admin_auctions: Option<Arc<dyn AdminAuctionReader>>,
    admin_lots: Option<Arc<dyn AdminLotReader>>,
    admin_lot_update: Option<Arc<dyn AdminLotUpdatePort>>,
    admin_proxy_events: Option<Arc<dyn AdminProxyEventReader>>,
    delivery_admin: Option<Arc<dyn DeliveryAdminPort>>,
    realtime_admin: Option<Arc<dyn RealtimeAdminPort>>,
    security: AdminHttpSecurity,
}

impl AdminApiState {
    /// 建立 Admin state。具体会话和生命周期存储由 Admin 装配根传入。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: Option<Arc<dyn AdminSessionPort>>,
        lifecycle: Option<Arc<dyn AuctionLifecycleCommandPort>>,
        auction_create: Option<Arc<dyn AdminAuctionCreatePort>>,
        admin_audit: Option<Arc<dyn AdminAuditPort>>,
        health: Option<Arc<dyn HealthReader>>,
        admin_intake: Option<Arc<dyn AdminIntakePort>>,
        admin_forum_preview: Option<Arc<dyn AdminForumPreviewPort>>,
        admin_mail: Option<Arc<dyn AdminMailPort>>,
        admin_rules: Option<Arc<dyn AdminRulesPort>>,
        admin_runtime: Option<Arc<dyn AdminRuntimeConfigPort>>,
        admin_renames: Option<Arc<dyn AdminRenameReader>>,
        recovery_commands: Option<Arc<dyn RecoveryCommandPort>>,
        settlement_report: Option<Arc<dyn SettlementReportReader>>,
        recovery_report: Option<Arc<dyn RecoveryReportReader>>,
        admin_auctions: Option<Arc<dyn AdminAuctionReader>>,
        admin_lots: Option<Arc<dyn AdminLotReader>>,
        admin_lot_update: Option<Arc<dyn AdminLotUpdatePort>>,
        admin_proxy_events: Option<Arc<dyn AdminProxyEventReader>>,
        delivery_admin: Option<Arc<dyn DeliveryAdminPort>>,
        realtime_admin: Option<Arc<dyn RealtimeAdminPort>>,
        security: AdminHttpSecurity,
    ) -> Self {
        Self {
            session,
            lifecycle,
            auction_create,
            admin_audit,
            health,
            admin_intake,
            admin_forum_preview,
            admin_mail,
            admin_rules,
            admin_runtime,
            admin_renames,
            recovery_commands,
            settlement_report,
            recovery_report,
            admin_auctions,
            admin_lots,
            admin_lot_update,
            admin_proxy_events,
            delivery_admin,
            realtime_admin,
            security,
        }
    }

    pub(crate) fn session(&self) -> Option<&dyn AdminSessionPort> {
        self.session.as_deref()
    }

    pub(crate) fn lifecycle(&self) -> Option<&dyn AuctionLifecycleCommandPort> {
        self.lifecycle.as_deref()
    }

    pub(crate) fn auction_create(&self) -> Option<&dyn AdminAuctionCreatePort> {
        self.auction_create.as_deref()
    }

    pub(crate) fn admin_audit(&self) -> Option<&dyn AdminAuditPort> {
        self.admin_audit.as_deref()
    }

    pub(crate) fn health(&self) -> Option<&dyn HealthReader> {
        self.health.as_deref()
    }

    pub(crate) fn admin_intake(&self) -> Option<&dyn AdminIntakePort> {
        self.admin_intake.as_deref()
    }

    pub(crate) fn admin_forum_preview(&self) -> Option<&dyn AdminForumPreviewPort> {
        self.admin_forum_preview.as_deref()
    }

    pub(crate) fn admin_mail(&self) -> Option<&dyn AdminMailPort> {
        self.admin_mail.as_deref()
    }

    pub(crate) fn admin_rules(&self) -> Option<&dyn AdminRulesPort> {
        self.admin_rules.as_deref()
    }

    pub(crate) fn admin_runtime(&self) -> Option<&dyn AdminRuntimeConfigPort> {
        self.admin_runtime.as_deref()
    }

    pub(crate) fn admin_renames(&self) -> Option<&dyn AdminRenameReader> {
        self.admin_renames.as_deref()
    }

    pub(crate) fn recovery_commands(&self) -> Option<&dyn RecoveryCommandPort> {
        self.recovery_commands.as_deref()
    }

    pub(crate) fn settlement_report(&self) -> Option<&dyn SettlementReportReader> {
        self.settlement_report.as_deref()
    }

    pub(crate) fn recovery_report(&self) -> Option<&dyn RecoveryReportReader> {
        self.recovery_report.as_deref()
    }

    pub(crate) fn admin_auctions(&self) -> Option<&dyn AdminAuctionReader> {
        self.admin_auctions.as_deref()
    }

    pub(crate) fn admin_lots(&self) -> Option<&dyn AdminLotReader> {
        self.admin_lots.as_deref()
    }

    pub(crate) fn admin_lot_update(&self) -> Option<&dyn AdminLotUpdatePort> {
        self.admin_lot_update.as_deref()
    }

    pub(crate) fn admin_proxy_events(&self) -> Option<&dyn AdminProxyEventReader> {
        self.admin_proxy_events.as_deref()
    }

    pub(crate) fn delivery_admin(&self) -> Option<&dyn DeliveryAdminPort> {
        self.delivery_admin.as_deref()
    }

    pub(crate) fn realtime_admin(&self) -> Option<&dyn RealtimeAdminPort> {
        self.realtime_admin.as_deref()
    }

    pub(crate) fn security(&self) -> &AdminHttpSecurity {
        &self.security
    }
}
