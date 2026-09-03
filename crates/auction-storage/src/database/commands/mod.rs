//! 由数据库后端承载的应用命令组合器。
//!
//! 该层只负责把运行时选择的后端接到应用窄端口；竞价规则和事务步骤仍由应用层与各后端
//! 事务适配器拥有。

use std::sync::Arc;

use auction_application::{
    AdminAuctionCreatePort, AdminAuctionSchedulePort, AdminAuctionSummary,
    AdminCreateAuctionCommand, AdminRulesDocument, AdminRulesPort, AdminRulesPublishCommand,
    AdminRulesReader, AdminRulesRequest, AdminUpdateAuctionScheduleCommand, ApplicationFuture,
    AuctionLifecycleCommand, AuctionLifecycleCommandPort, AuctionLifecyclePolicy,
    AuctionLifecycleResult, AuctionLifecycleService, AuctionPolicyTemplate, Clock,
    CompetitionCommandOutcome, CompetitionCommandPort, CompetitionService, CreateProxyCommand,
    ExecuteSlowProxyCommand, LowerProxyCommand, PlaceBidCommand, SlowProxyCommandPort,
};
#[cfg(feature = "postgres")]
use auction_application::{
    AdminAuctionCreateService, AdminAuctionScheduleService, AdminRulesService,
};
use auction_domain::CompetitionPlan;

use super::DatabaseStore;

#[path = "admin/intake.rs"]
mod admin_intake;
pub use admin_intake::DatabaseAdminIntakeCommands;
#[path = "admin/mail.rs"]
mod admin_mail;
pub use admin_mail::DatabaseAdminMailCommands;
#[path = "admin/lot.rs"]
mod admin_lot;
pub use admin_lot::DatabaseAdminLotCommands;
#[path = "admin/runtime.rs"]
mod admin_runtime;
pub use admin_runtime::DatabaseAdminRuntimeCommands;

/// 绑定运行时数据库和时钟的 Public 竞价命令端口。
pub struct DatabaseCompetitionCommands<C> {
    store: Arc<DatabaseStore>,
    clock: Arc<C>,
}

impl<C> DatabaseCompetitionCommands<C>
where
    C: Clock + 'static,
{
    /// 建立不向 HTTP 暴露具体数据库类型的命令端口。
    pub fn new(store: Arc<DatabaseStore>, clock: Arc<C>) -> Self {
        Self { store, clock }
    }
}

impl<C> CompetitionCommandPort for DatabaseCompetitionCommands<C>
where
    C: Clock + 'static,
{
    fn place_bid(
        &self,
        command: PlaceBidCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome> {
        let store = Arc::clone(&self.store);
        let clock = Arc::clone(&self.clock);
        let _ = (&command, &clock);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()), clock);
                    service.place_bid(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()), clock);
                    service.place_bid(command).await
                }
            }
        })
    }

    fn create_proxy(
        &self,
        command: CreateProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome> {
        let store = Arc::clone(&self.store);
        let clock = Arc::clone(&self.clock);
        #[cfg(not(feature = "postgres"))]
        let _ = (&command, &clock);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()), clock);
                    service.create_proxy(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()), clock);
                    service.create_proxy(command).await
                }
            }
        })
    }

    fn lower_proxy(
        &self,
        command: LowerProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome> {
        let store = Arc::clone(&self.store);
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()), clock);
                    service.lower_proxy(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()), clock);
                    service.lower_proxy(command).await
                }
            }
        })
    }
}

impl<C> SlowProxyCommandPort for DatabaseCompetitionCommands<C>
where
    C: Clock + 'static,
{
    fn execute_slow_proxy(
        &self,
        command: ExecuteSlowProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionPlan> {
        let store = Arc::clone(&self.store);
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()), clock);
                    service.execute_slow_proxy(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()), clock);
                    service.execute_slow_proxy(command).await
                }
            }
        })
    }
}

/// 绑定运行时数据库和时钟的 Admin/Worker 生命周期命令端口。
pub struct DatabaseAuctionLifecycleCommands<C> {
    store: Arc<DatabaseStore>,
    clock: Arc<C>,
    policy: AuctionLifecyclePolicy,
}

impl<C> DatabaseAuctionLifecycleCommands<C>
where
    C: Clock + 'static,
{
    /// 建立不向 HTTP 暴露具体数据库类型、并冻结运行期授权策略的生命周期命令端口。
    pub fn new(store: Arc<DatabaseStore>, clock: Arc<C>, policy: AuctionLifecyclePolicy) -> Self {
        Self { store, clock, policy }
    }
}

impl<C> AuctionLifecycleCommandPort for DatabaseAuctionLifecycleCommands<C>
where
    C: Clock + 'static,
{
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
        let clock = Arc::clone(&self.clock);
        let policy = self.policy;
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service =
                        AuctionLifecycleService::new(Arc::new(store.clone()), clock, policy);
                    service.execute(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    let service =
                        AuctionLifecycleService::new(Arc::new(store.clone()), clock, policy);
                    service.execute(command).await
                }
            }
        })
    }
}

/// 绑定运行时数据库和时钟的 Admin 创建拍卖命令端口。
///
/// 创建拍卖依赖 PostgreSQL 完整档的 intake target、策略快照、审计和 realtime 事务；SQLite
/// Lite 保持显式能力缺失，不把 draft 写入伪装成完整迁移。
pub struct DatabaseAdminAuctionCommands<C> {
    store: Arc<DatabaseStore>,
    clock: Arc<C>,
    policy: AuctionPolicyTemplate,
}

/// 绑定运行时数据库和时钟的 Admin 规则读取/发布端口。
///
/// 规则读取在 PostgreSQL 与 SQLite Lite 均可用；版本发布依赖 PostgreSQL 完整档的规则命令
/// 账本、Serializable CAS、审计和重放语义，Lite 明确返回能力缺失。
pub struct DatabaseAdminRulesCommands<C> {
    store: Arc<DatabaseStore>,
    clock: Arc<C>,
}

impl<C> DatabaseAdminRulesCommands<C>
where
    C: Clock + 'static,
{
    /// 建立不向 HTTP 暴露具体数据库类型的规则端口。
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
        #[cfg(not(feature = "postgres"))]
        let _ = (&command, &clock);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = AdminRulesService::new(Arc::new(store.clone()), clock);
                    service.publish(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    Err(auction_application::ApplicationError::CapabilityUnavailable(
                        "Admin 规则发布仅支持 PostgreSQL 完整档",
                    ))
                }
            }
        })
    }
}

impl<C> DatabaseAdminAuctionCommands<C>
where
    C: Clock + 'static,
{
    /// 建立不向 HTTP 暴露具体数据库类型的创建命令端口。
    pub fn new(store: Arc<DatabaseStore>, clock: Arc<C>, policy: AuctionPolicyTemplate) -> Self {
        Self { store, clock, policy }
    }
}

impl<C> AdminAuctionCreatePort for DatabaseAdminAuctionCommands<C>
where
    C: Clock + 'static,
{
    fn create(
        &self,
        command: AdminCreateAuctionCommand,
    ) -> ApplicationFuture<'_, AdminAuctionSummary> {
        let store = Arc::clone(&self.store);
        let clock = Arc::clone(&self.clock);
        #[cfg(feature = "postgres")]
        let policy = self.policy.clone();
        #[cfg(not(feature = "postgres"))]
        let _ = (&command, &clock, &self.policy);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service =
                        AdminAuctionCreateService::new(Arc::new(store.clone()), clock, policy);
                    service.create(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    Err(auction_application::ApplicationError::CapabilityUnavailable(
                        "Admin 创建拍卖仅支持 PostgreSQL 完整档",
                    ))
                }
            }
        })
    }
}

impl<C> AdminAuctionSchedulePort for DatabaseAdminAuctionCommands<C>
where
    C: Clock + 'static,
{
    fn update_schedule(
        &self,
        command: AdminUpdateAuctionScheduleCommand,
    ) -> ApplicationFuture<'_, AdminAuctionSummary> {
        let store = Arc::clone(&self.store);
        #[cfg(not(feature = "postgres"))]
        let _ = &command;
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = AdminAuctionScheduleService::new(Arc::new(store.clone()));
                    service.update_schedule(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    Err(auction_application::ApplicationError::CapabilityUnavailable(
                        "Admin 拍卖计划修改仅支持 PostgreSQL 完整档",
                    ))
                }
            }
        })
    }
}
