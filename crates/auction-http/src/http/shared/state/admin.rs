//! Admin HTTP surface 的应用端口状态容器。

use std::sync::Arc;

use auction_application::{
    AdminAuctionCreatePort, AdminAuctionReader, AdminAuditPort, AdminCredentialHealthReader,
    AdminCredentialPort, AdminCredentialWorld, AdminForumPreviewPort, AdminIntakePort,
    AdminLotReader, AdminLotUpdatePort, AdminMailPort, AdminProxyEventReader, AdminRenameReader,
    AdminRulesPort, AdminRuntimeConfigPort, AdminSessionPort, AdminUserPort,
    AuctionLifecycleCommandPort, DeliveryAdminPort, OperationsAlertSourceReader,
    OperationsAlertStore, ProcessReadinessPort, ReadModelCache, RealtimeAdminPort,
    RecoveryCommandPort, RecoveryReportReader, SettlementReportReader, WorkerControlPort,
    public_rules_cache_key,
};
use auction_observability::{HealthReader, ProcessMetrics};

use super::super::HttpBoundary;
use super::super::security::AdminHttpSecurity;

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
    admin_credentials: Option<Arc<dyn AdminCredentialPort>>,
    admin_renames: Option<Arc<dyn AdminRenameReader>>,
    admin_users: Option<Arc<dyn AdminUserPort>>,
    recovery_commands: Option<Arc<dyn RecoveryCommandPort>>,
    settlement_report: Option<Arc<dyn SettlementReportReader>>,
    recovery_report: Option<Arc<dyn RecoveryReportReader>>,
    admin_auctions: Option<Arc<dyn AdminAuctionReader>>,
    admin_lots: Option<Arc<dyn AdminLotReader>>,
    admin_lot_update: Option<Arc<dyn AdminLotUpdatePort>>,
    admin_proxy_events: Option<Arc<dyn AdminProxyEventReader>>,
    delivery_admin: Option<Arc<dyn DeliveryAdminPort>>,
    operations_alert: Option<Arc<dyn OperationsAlertStore>>,
    operations_alert_source: Option<Arc<dyn OperationsAlertSourceReader>>,
    credential_health: Option<Arc<dyn AdminCredentialHealthReader>>,
    realtime_admin: Option<Arc<dyn RealtimeAdminPort>>,
    worker_control: Option<Arc<dyn WorkerControlPort>>,
    dashboard_worlds: Vec<AdminCredentialWorld>,
    dashboard_mailbox_keys: Vec<String>,
    metrics: Option<Arc<ProcessMetrics>>,
    readiness: Option<Arc<dyn ProcessReadinessPort>>,
    security: AdminHttpSecurity,
    public_rules_cache: Option<Arc<dyn ReadModelCache>>,
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
        admin_credentials: Option<Arc<dyn AdminCredentialPort>>,
        admin_renames: Option<Arc<dyn AdminRenameReader>>,
        admin_users: Option<Arc<dyn AdminUserPort>>,
        recovery_commands: Option<Arc<dyn RecoveryCommandPort>>,
        settlement_report: Option<Arc<dyn SettlementReportReader>>,
        recovery_report: Option<Arc<dyn RecoveryReportReader>>,
        admin_auctions: Option<Arc<dyn AdminAuctionReader>>,
        admin_lots: Option<Arc<dyn AdminLotReader>>,
        admin_lot_update: Option<Arc<dyn AdminLotUpdatePort>>,
        admin_proxy_events: Option<Arc<dyn AdminProxyEventReader>>,
        delivery_admin: Option<Arc<dyn DeliveryAdminPort>>,
        operations_alert: Option<Arc<dyn OperationsAlertStore>>,
        operations_alert_source: Option<Arc<dyn OperationsAlertSourceReader>>,
        credential_health: Option<Arc<dyn AdminCredentialHealthReader>>,
        realtime_admin: Option<Arc<dyn RealtimeAdminPort>>,
        worker_control: Option<Arc<dyn WorkerControlPort>>,
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
            admin_credentials,
            admin_renames,
            admin_users,
            recovery_commands,
            settlement_report,
            recovery_report,
            admin_auctions,
            admin_lots,
            admin_lot_update,
            admin_proxy_events,
            delivery_admin,
            operations_alert,
            operations_alert_source,
            credential_health,
            realtime_admin,
            worker_control,
            dashboard_worlds: Vec::new(),
            dashboard_mailbox_keys: Vec::new(),
            metrics: None,
            readiness: None,
            security,
            public_rules_cache: None,
        }
    }

    /// 注入进程本地 Prometheus 指标注册表；指标不读取 Admin 业务状态。
    pub fn with_metrics(mut self, metrics: Arc<ProcessMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 注入只读 readiness 检查；未注入时 `/health/ready` 明确返回未就绪。
    pub fn with_readiness(mut self, readiness: Arc<dyn ProcessReadinessPort>) -> Self {
        self.readiness = Some(readiness);
        self
    }

    /// 返回与本 state 同源的协议边界，供路由装配时建立全局 middleware。
    pub fn http_boundary(&self) -> HttpBoundary {
        self.security.boundary()
    }

    /// 注入 Admin dashboard 需要严格检查的 Worker world 范围；空集合表示当前进程没有可靠的
    /// Worker 配置投影，HTTP 层只展示已观察到的邮件 world，不把未配置 world 误报为故障。
    pub fn with_dashboard_worlds(mut self, worlds: Vec<AdminCredentialWorld>) -> Self {
        self.dashboard_worlds = worlds;
        self
    }

    /// 注入 Admin dashboard 需要严格检查的已配置邮箱标识；空集合表示不强制套用 Worker 配置范围。
    pub fn with_dashboard_mailbox_keys(mut self, mailbox_keys: Vec<String>) -> Self {
        self.dashboard_mailbox_keys = mailbox_keys;
        self
    }

    /// 注入 Public 规则缓存失效端口；缓存不是 Admin 规则权威，失效失败不能改变发布结果。
    pub fn with_public_rules_cache(mut self, cache: Arc<dyn ReadModelCache>) -> Self {
        self.public_rules_cache = Some(cache);
        self
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

    pub(crate) fn metrics(&self) -> Option<&ProcessMetrics> {
        self.metrics.as_deref()
    }

    pub(crate) fn readiness(&self) -> Option<&dyn ProcessReadinessPort> {
        self.readiness.as_deref()
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

    pub(crate) fn admin_credentials(&self) -> Option<&dyn AdminCredentialPort> {
        self.admin_credentials.as_deref()
    }

    pub(crate) fn admin_renames(&self) -> Option<&dyn AdminRenameReader> {
        self.admin_renames.as_deref()
    }

    pub(crate) fn admin_users(&self) -> Option<&dyn AdminUserPort> {
        self.admin_users.as_deref()
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

    pub(crate) fn operations_alert(&self) -> Option<&dyn OperationsAlertStore> {
        self.operations_alert.as_deref()
    }

    pub(crate) fn operations_alert_source(&self) -> Option<&dyn OperationsAlertSourceReader> {
        self.operations_alert_source.as_deref()
    }

    pub(crate) fn credential_health(&self) -> Option<&dyn AdminCredentialHealthReader> {
        self.credential_health.as_deref()
    }

    pub(crate) fn dashboard_worlds(&self) -> &[AdminCredentialWorld] {
        &self.dashboard_worlds
    }

    pub(crate) fn dashboard_mailbox_keys(&self) -> &[String] {
        &self.dashboard_mailbox_keys
    }

    pub(crate) fn realtime_admin(&self) -> Option<&dyn RealtimeAdminPort> {
        self.realtime_admin.as_deref()
    }

    pub(crate) fn worker_control(&self) -> Option<&dyn WorkerControlPort> {
        self.worker_control.as_deref()
    }

    pub(crate) fn security(&self) -> &AdminHttpSecurity {
        &self.security
    }

    pub(crate) async fn invalidate_public_rules_cache(&self, locale: &str) {
        let Some(cache) = self.public_rules_cache.as_deref() else {
            return;
        };
        let Some(key) = public_rules_cache_key(locale) else {
            return;
        };
        if cache.invalidate(key).await.is_err() {
            tracing::warn!(locale, "public rules cache invalidation failed");
        }
    }
}
