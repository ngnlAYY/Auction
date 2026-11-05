//! Admin HTTP surface 的应用端口状态容器。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics;

use std::sync::Arc;

use auction_application::{
    AdminAuctionCreatePort, AdminAuctionReader, AdminAuctionSchedulePort, AdminAuditPort,
    AdminCredentialCheckPort, AdminCredentialHealthReader, AdminCredentialPort,
    AdminCredentialWorld, AdminForumPreviewPort, AdminIntakePort, AdminLotReader,
    AdminLotUpdatePort, AdminMailPort, AdminProxyEventReader, AdminRenameReader, AdminRulesPort,
    AdminRuntimeConfigPort, AdminSessionPort, AdminUserPort, AdminWorldPort,
    AuctionLifecycleCommandPort, DeliveryAdminPort, OperationsAlertConsole,
    OperationsAlertSourceReader, OperationsAlertStore, ProcessReadinessPort, PublicForumReader,
    ReadModelCache, RealtimeAdminPort, RecoveryCommandPort, RecoveryReportReader,
    SettlementCatalogReader, SettlementReportReader, WorkerControlPort, public_rules_cache_key,
};
use auction_observability::{HealthReader, ProcessMetrics};

use super::super::HttpBoundary;
use super::super::security::AdminHttpSecurity;

/// Admin surface 可使用的应用端口集合。
/// 管理命令、查询和会话按 owner 显式分离；缺失端口时只报告依赖不可用，不构造假数据。
#[derive(Clone, Default)]
pub struct AdminApiState {
    runtime_switches: Option<(
        Arc<dyn auction_application::RuntimeSwitchStore>,
        auction_application::RuntimeSwitches,
    )>,
    session: Option<Arc<dyn AdminSessionPort>>,
    lifecycle: Option<Arc<dyn AuctionLifecycleCommandPort>>,
    auction_create: Option<Arc<dyn AdminAuctionCreatePort>>,
    auction_schedule: Option<Arc<dyn AdminAuctionSchedulePort>>,
    admin_audit: Option<Arc<dyn AdminAuditPort>>,
    health: Option<Arc<dyn HealthReader>>,
    admin_intake: Option<Arc<dyn AdminIntakePort>>,
    admin_forum_preview: Option<Arc<dyn AdminForumPreviewPort>>,
    forum_reader: Option<Arc<dyn PublicForumReader>>,
    admin_mail: Option<Arc<dyn AdminMailPort>>,
    admin_rules: Option<Arc<dyn AdminRulesPort>>,
    admin_runtime: Option<Arc<dyn AdminRuntimeConfigPort>>,
    admin_credentials: Option<Arc<dyn AdminCredentialPort>>,
    credential_check: Option<Arc<dyn AdminCredentialCheckPort>>,
    admin_renames: Option<Arc<dyn AdminRenameReader>>,
    admin_users: Option<Arc<dyn AdminUserPort>>,
    admin_worlds: Option<Arc<dyn AdminWorldPort>>,
    recovery_commands: Option<Arc<dyn RecoveryCommandPort>>,
    settlement_report: Option<Arc<dyn SettlementReportReader>>,
    settlement_catalog: Option<Arc<dyn SettlementCatalogReader>>,
    recovery_report: Option<Arc<dyn RecoveryReportReader>>,
    admin_auctions: Option<Arc<dyn AdminAuctionReader>>,
    admin_lots: Option<Arc<dyn AdminLotReader>>,
    admin_lot_update: Option<Arc<dyn AdminLotUpdatePort>>,
    admin_proxy_events: Option<Arc<dyn AdminProxyEventReader>>,
    delivery_admin: Option<Arc<dyn DeliveryAdminPort>>,
    delivery_sent_proof: Option<Arc<auction_application::DeliverySentProofService>>,
    operations_alert: Option<Arc<dyn OperationsAlertStore>>,
    operations_alert_source: Option<Arc<dyn OperationsAlertSourceReader>>,
    operations_alert_console: Option<Arc<dyn OperationsAlertConsole>>,
    credential_health: Option<Arc<dyn AdminCredentialHealthReader>>,
    realtime_admin: Option<Arc<dyn RealtimeAdminPort>>,
    worker_control: Option<Arc<dyn WorkerControlPort>>,
    // 启动配置投影只读共享，避免每次提取 State 都复制列表及邮箱标识。
    dashboard_worlds: Arc<[AdminCredentialWorld]>,
    dashboard_mailbox_keys: Arc<[String]>,
    metrics: Option<Arc<ProcessMetrics>>,
    runtime_switch_snapshot: Option<auction_application::RuntimeSwitches>,
    readiness: Option<Arc<dyn ProcessReadinessPort>>,
    security: AdminHttpSecurity,
    public_rules_cache: Option<Arc<dyn ReadModelCache>>,
}

impl AdminApiState {
    /// 注入数据库意图读取端口和 Admin 自身应用快照，不依赖 runtime 文件。
    pub fn with_runtime_switches(
        mut self,
        store: Arc<dyn auction_application::RuntimeSwitchStore>,
        switches: auction_application::RuntimeSwitches,
    ) -> Self {
        self.runtime_switches = Some((store, switches));
        self
    }

    pub(crate) fn runtime_switches(
        &self,
    ) -> Option<&(
        Arc<dyn auction_application::RuntimeSwitchStore>,
        auction_application::RuntimeSwitches,
    )> {
        self.runtime_switches.as_ref()
    }

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
            runtime_switches: None,
            session,
            lifecycle,
            auction_create,
            auction_schedule: None,
            admin_audit,
            health,
            admin_intake,
            admin_forum_preview,
            forum_reader: None,
            admin_mail,
            admin_rules,
            admin_runtime,
            admin_credentials,
            credential_check: None,
            admin_renames,
            admin_users,
            admin_worlds: None,
            recovery_commands,
            settlement_report,
            settlement_catalog: None,
            recovery_report,
            admin_auctions,
            admin_lots,
            admin_lot_update,
            admin_proxy_events,
            delivery_admin,
            delivery_sent_proof: None,
            operations_alert,
            operations_alert_source,
            operations_alert_console: None,
            credential_health,
            realtime_admin,
            worker_control,
            dashboard_worlds: Arc::default(),
            dashboard_mailbox_keys: Arc::default(),
            metrics: None,
            runtime_switch_snapshot: None,
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

    /// 注入进程共享的运行时开关快照，供指标边界在请求入口读取。
    pub fn with_runtime_switch_snapshot(
        mut self,
        switches: auction_application::RuntimeSwitches,
    ) -> Self {
        self.runtime_switch_snapshot = Some(switches);
        self
    }

    /// 注入只读 readiness 检查；未注入时 `/health/ready` 明确返回未就绪。
    pub fn with_readiness(mut self, readiness: Arc<dyn ProcessReadinessPort>) -> Self {
        self.readiness = Some(readiness);
        self
    }

    /// 注入完整档凭据只读校验；缺少密钥或后端能力时不伪造可用状态。
    pub fn with_credential_check(
        mut self,
        check: Option<Arc<dyn AdminCredentialCheckPort>>,
    ) -> Self {
        self.credential_check = check;
        self
    }

    pub(crate) fn credential_check(&self) -> Option<&dyn AdminCredentialCheckPort> {
        self.credential_check.as_deref()
    }

    /// 注入论坛本地安全投影；Admin 刷新只读状态，不授权或触发远端重写。
    pub fn with_forum_reader(mut self, reader: Arc<dyn PublicForumReader>) -> Self {
        self.forum_reader = Some(reader);
        self
    }

    pub(crate) fn forum_reader(&self) -> Option<&dyn PublicForumReader> {
        self.forum_reader.as_deref()
    }

    /// 注入正式开拍前的计划结束时间修改端口。
    pub fn with_auction_schedule(mut self, schedule: Arc<dyn AdminAuctionSchedulePort>) -> Self {
        self.auction_schedule = Some(schedule);
        self
    }

    /// 注入数据库权威的业务世界读取与启停端口。
    pub fn with_admin_worlds(mut self, worlds: Arc<dyn AdminWorldPort>) -> Self {
        self.admin_worlds = Some(worlds);
        self
    }

    /// 注入结算报表专用的已结算拍卖目录。
    pub fn with_settlement_catalog(mut self, catalog: Arc<dyn SettlementCatalogReader>) -> Self {
        self.settlement_catalog = Some(catalog);
        self
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

    /// 注入 Admin dashboard 需要严格检查的 Worker world 范围；空集合表示当前进程没有可靠的
    /// Worker 配置投影，HTTP 层只展示已观察到的邮件 world，不把未配置 world 误报为故障。
    pub fn with_dashboard_worlds(mut self, worlds: Vec<AdminCredentialWorld>) -> Self {
        self.dashboard_worlds = worlds.into();
        self
    }

    /// 注入 Admin dashboard 需要严格检查的已配置邮箱标识；空集合表示不强制套用 Worker 配置范围。
    pub fn with_dashboard_mailbox_keys(mut self, mailbox_keys: Vec<String>) -> Self {
        self.dashboard_mailbox_keys = mailbox_keys.into();
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

    pub(crate) fn auction_schedule(&self) -> Option<&dyn AdminAuctionSchedulePort> {
        self.auction_schedule.as_deref()
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

    pub(crate) fn admin_worlds(&self) -> Option<&dyn AdminWorldPort> {
        self.admin_worlds.as_deref()
    }

    pub(crate) fn recovery_commands(&self) -> Option<&dyn RecoveryCommandPort> {
        self.recovery_commands.as_deref()
    }

    pub(crate) fn settlement_report(&self) -> Option<&dyn SettlementReportReader> {
        self.settlement_report.as_deref()
    }

    pub(crate) fn settlement_catalog(&self) -> Option<&dyn SettlementCatalogReader> {
        self.settlement_catalog.as_deref()
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

    /// 注入人工 Sent 证明服务；未注入时相关路由明确返回能力不可用。
    pub fn with_delivery_sent_proof(
        mut self,
        service: Arc<auction_application::DeliverySentProofService>,
    ) -> Self {
        self.delivery_sent_proof = Some(service);
        self
    }

    pub(crate) fn delivery_sent_proof(
        &self,
    ) -> Option<&auction_application::DeliverySentProofService> {
        self.delivery_sent_proof.as_deref()
    }

    pub(crate) fn operations_alert(&self) -> Option<&dyn OperationsAlertStore> {
        self.operations_alert.as_deref()
    }

    pub(crate) fn operations_alert_source(&self) -> Option<&dyn OperationsAlertSourceReader> {
        self.operations_alert_source.as_deref()
    }

    /// 注入告警台读写端口；未注入时告警台明确报告存储不可用。
    pub fn with_operations_alert_console(
        mut self,
        console: Option<Arc<dyn OperationsAlertConsole>>,
    ) -> Self {
        self.operations_alert_console = console;
        self
    }

    pub(crate) fn operations_alert_console(&self) -> Option<&dyn OperationsAlertConsole> {
        self.operations_alert_console.as_deref()
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
        if let Err(cause) = cache.invalidate(key).await {
            diagnostics::warn(
                "public_rules",
                "invalidate_cache",
                diagnostics::cache_kind(&cause),
                DiagnosticContext::default(),
            );
        }
    }
}
