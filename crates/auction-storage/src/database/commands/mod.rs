//! 由数据库后端承载的应用命令组合器；本层只把运行时选择的后端接到应用窄端口，
//! 竞价规则和事务步骤仍由应用层与各后端事务适配器拥有。

use std::sync::Arc;

use auction_application::{
    AdminAuctionCreatePort, AdminAuctionSchedulePort, AdminAuctionSummary,
    AdminCreateAuctionCommand, AdminRulesDocument, AdminRulesPort, AdminRulesPublishCommand,
    AdminRulesReader, AdminRulesRequest, AdminUpdateAuctionScheduleCommand, AdminWorldPort,
    AdminWorldSetEnabledCommand, ApplicationFuture, AuctionLifecycleCommand,
    AuctionLifecycleCommandPort, AuctionLifecyclePolicy, AuctionLifecycleResult,
    AuctionLifecycleService, AuctionPolicyTemplate, Clock, CompetitionCommandOutcome,
    CompetitionCommandPort, CompetitionService, CreateProxyCommand, ExecuteSlowProxyCommand,
    LowerProxyCommand, PlaceBidCommand, SlowProxyCommandPort,
};
#[cfg(feature = "postgres")]
use auction_application::{
    AdminAuctionCreateService, AdminAuctionScheduleService, AdminRulesService, AdminWorldService,
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

/// 绑定运行时数据库的 Admin 业务世界读取与启停命令端口。
pub struct DatabaseAdminWorldCommands {
    store: Arc<DatabaseStore>,
}

impl DatabaseAdminWorldCommands {
    pub fn new(store: Arc<DatabaseStore>) -> Self {
        Self { store }
    }
}

impl AdminWorldPort for DatabaseAdminWorldCommands {
    fn list_worlds(&self) -> ApplicationFuture<'_, Vec<auction_application::WorldSnapshot>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let worlds = auction_application::WorldReader::list_worlds(store.as_ref()).await?;
            if worlds.iter().any(|world| !world.validate()) {
                return Err(auction_application::AdminWorldError::StoredResultCorrupt.into());
            }
            Ok(worlds)
        })
    }

    fn set_enabled(
        &self,
        command: AdminWorldSetEnabledCommand,
    ) -> ApplicationFuture<'_, auction_application::WorldSnapshot> {
        let store = Arc::clone(&self.store);
        #[cfg(not(feature = "postgres"))]
        let _ = &command;
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = AdminWorldService::new(Arc::new(store.clone()));
                    service.set_enabled(command).await
                }
            }
        })
    }
}

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
        #[cfg(not(feature = "postgres"))]
        let _ = &command;
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()));
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
        #[cfg(not(feature = "postgres"))]
        let _ = &command;
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()));
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
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()));
                    service.lower_proxy(command).await
                }
            }
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
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = CompetitionService::new(Arc::new(store.clone()));
                    service.execute_slow_proxy(command).await
                }
            }
        })
    }
}

/// 绑定运行时数据库的 Admin/Worker 生命周期命令端口；锁后读取权威时间。
pub struct DatabaseAuctionLifecycleCommands {
    store: Arc<DatabaseStore>,
    policy: AuctionLifecyclePolicy,
    successor_policy: AuctionPolicyTemplate,
}

impl DatabaseAuctionLifecycleCommands {
    /// 建立不向 HTTP 暴露具体数据库类型、并冻结运行期授权策略的生命周期命令端口。
    pub fn new(
        store: Arc<DatabaseStore>,
        policy: AuctionLifecyclePolicy,
        successor_policy: AuctionPolicyTemplate,
    ) -> Self {
        Self { store, policy, successor_policy }
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
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = AuctionLifecycleService::new(
                        Arc::new(store.clone()),
                        policy,
                        successor_policy,
                    );
                    service.execute(command).await
                }
            }
        })
    }
}

/// 绑定运行时数据库的 Admin 创建拍卖命令端口；锁后读取权威时间。
/// 创建拍卖将 intake target、策略快照、审计和 realtime 事件在同一事务提交。
pub struct DatabaseAdminAuctionCommands {
    store: Arc<DatabaseStore>,
    policy: AuctionPolicyTemplate,
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
        #[cfg(not(feature = "postgres"))]
        let _ = (&command, &clock);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = AdminRulesService::new(Arc::new(store.clone()), clock);
                    service.publish(command).await
                }
            }
        })
    }
}

impl DatabaseAdminAuctionCommands {
    pub fn new(store: Arc<DatabaseStore>, policy: AuctionPolicyTemplate) -> Self {
        Self { store, policy }
    }
}

impl AdminAuctionCreatePort for DatabaseAdminAuctionCommands {
    fn create(
        &self,
        command: AdminCreateAuctionCommand,
    ) -> ApplicationFuture<'_, AdminAuctionSummary> {
        let store = Arc::clone(&self.store);
        #[cfg(feature = "postgres")]
        let policy = self.policy.clone();
        #[cfg(not(feature = "postgres"))]
        let _ = (&command, &self.policy);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = AdminAuctionCreateService::new(Arc::new(store.clone()), policy);
                    service.create(command).await
                }
            }
        })
    }
}

impl AdminAuctionSchedulePort for DatabaseAdminAuctionCommands {
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
            }
        })
    }
}
