//! 由数据库后端承载的应用命令组合器；本层只把运行时选择的后端接到应用窄端口，
//! 竞价规则和事务步骤仍由应用层与各后端事务适配器拥有。

use std::sync::Arc;

use auction_application::admin::AdminRulesService;
use auction_application::admin::{
    AdminRulesDocument, AdminRulesPort, AdminRulesPublishCommand, AdminRulesReader,
    AdminRulesRequest,
};
use auction_application::auction::{
    ApplicationFuture, AuctionLifecycleCommand, AuctionLifecycleCommandPort,
    AuctionLifecyclePolicy, AuctionLifecycleResult, AuctionLifecycleService, AuctionPolicyTemplate,
    CompetitionCommandOutcome, CompetitionCommandPort, CompetitionService, CreateProxyCommand,
    ExecuteSlowProxyCommand, LowerProxyCommand, PlaceBidCommand, SlowProxyCommandPort,
};
use auction_application::core::Clock;
use auction_domain::{AuctionScheduleCalendar, CompetitionPlan};

use super::DatabaseStore;

mod admin;
pub use admin::mail::DatabaseAdminMailCommands;
pub use admin::runtime::DatabaseAdminRuntimeCommands;

/// 绑定运行时数据库的 Public 竞价命令端口；命令时间由锁后数据库时钟提供。
pub struct DatabaseCompetitionCommands {
    store: Arc<DatabaseStore>,
}

impl DatabaseCompetitionCommands {
    pub fn new(store: Arc<DatabaseStore>) -> Self {
        Self { store }
    }
}

impl CompetitionCommandPort for DatabaseCompetitionCommands {
    fn place_bid(
        &self,
        command: PlaceBidCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let store = store.as_ref();
            let service = CompetitionService::new(Arc::new(
                store.for_transaction(auction_observability::DatabaseTransactionOwner::Bid),
            ));
            service.place_bid(command).await
        })
    }

    fn create_proxy(
        &self,
        command: CreateProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let store = store.as_ref();
            let service = CompetitionService::new(Arc::new(
                store.for_transaction(auction_observability::DatabaseTransactionOwner::Proxy),
            ));
            service.create_proxy(command).await
        })
    }

    fn lower_proxy(
        &self,
        command: LowerProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let store = store.as_ref();
            let service = CompetitionService::new(Arc::new(
                store.for_transaction(auction_observability::DatabaseTransactionOwner::Proxy),
            ));
            service.lower_proxy(command).await
        })
    }
}

impl SlowProxyCommandPort for DatabaseCompetitionCommands {
    fn execute_slow_proxy(
        &self,
        command: ExecuteSlowProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionPlan> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let store = store.as_ref();
            let service = CompetitionService::new(Arc::new(
                store.for_transaction(auction_observability::DatabaseTransactionOwner::SlowProxy),
            ));
            service.execute_slow_proxy(command).await
        })
    }
}

/// 绑定运行时数据库的 Admin/Worker 生命周期命令端口；锁后读取权威时间。
pub struct DatabaseAuctionLifecycleCommands {
    store: Arc<DatabaseStore>,
    policy: AuctionLifecyclePolicy,
    successor_policy: AuctionPolicyTemplate,
    schedule_calendar: Option<AuctionScheduleCalendar>,
}

impl DatabaseAuctionLifecycleCommands {
    /// 建立不向 HTTP 暴露具体数据库类型、并冻结运行期授权策略的生命周期命令端口。
    pub fn new(
        store: Arc<DatabaseStore>,
        policy: AuctionLifecyclePolicy,
        successor_policy: AuctionPolicyTemplate,
    ) -> Self {
        Self { store, policy, successor_policy, schedule_calendar: None }
    }

    /// 注入启动期有效的周排期与早鸟日历；Admin 与 Worker 必须使用同一配置投影。
    pub fn with_schedule_calendar(mut self, calendar: Option<AuctionScheduleCalendar>) -> Self {
        self.schedule_calendar = calendar;
        self
    }
}

impl AuctionLifecycleCommandPort for DatabaseAuctionLifecycleCommands {
    fn allows_manual_early_open(&self) -> bool {
        self.policy.allows_manual_early_open()
    }

    fn allows_manual_early_settlement(&self) -> bool {
        self.policy.allows_manual_early_settlement()
    }

    fn execute(
        &self,
        command: AuctionLifecycleCommand,
    ) -> ApplicationFuture<'_, AuctionLifecycleResult> {
        let store = Arc::clone(&self.store);
        let policy = self.policy;
        let successor_policy = self.successor_policy.clone();
        let schedule_calendar = self.schedule_calendar.clone();
        Box::pin(async move {
            let store = store.as_ref();
            let service =
                AuctionLifecycleService::new(
                    Arc::new(store.for_transaction(
                        auction_observability::DatabaseTransactionOwner::Lifecycle,
                    )),
                    policy,
                    successor_policy,
                )
                .with_schedule_calendar(schedule_calendar);
            service.execute(command).await
        })
    }
}

/// 绑定运行时数据库和时钟的 Admin 规则读取/发布端口。
/// 版本发布使用规则命令账本、Serializable CAS、追加审计和持久重放。
pub struct DatabaseAdminRulesCommands<C> {
    store: Arc<DatabaseStore>,
    clock: Arc<C>,
}

impl<C> DatabaseAdminRulesCommands<C>
where
    C: Clock + 'static,
{
    pub fn new(store: Arc<DatabaseStore>, clock: Arc<C>) -> Self {
        Self { store, clock }
    }
}

impl<C> AdminRulesPort for DatabaseAdminRulesCommands<C>
where
    C: Clock + 'static,
{
    fn get(&self, request: AdminRulesRequest) -> ApplicationFuture<'_, Option<AdminRulesDocument>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { AdminRulesReader::get_admin_rules(store.as_ref(), request).await })
    }

    fn publish(
        &self,
        command: AdminRulesPublishCommand,
    ) -> ApplicationFuture<'_, AdminRulesDocument> {
        let store = Arc::clone(&self.store);
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            // 复用同一个共享存储句柄，不为每条命令深克隆 `DatabaseStore`。
            let service = AdminRulesService::new(store, clock);
            service.publish(command).await
        })
    }
}
