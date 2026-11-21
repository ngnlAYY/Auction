//! 注入一口价挂单的持久端口、装备来源及登记就绪探针。

use std::sync::Arc;

use anyhow::Context;
use auction_application::consignment::{
    ConsignmentFuture, ListingLookupBudget, ListingMaintenanceService, LookupWorkStore,
    ResolveListingService, auto_close_command, lookup_reap_exhausted_identity,
};
use auction_application::core::diagnostics::DiagnosticError;
use auction_application::runtime_switches::RuntimeSwitches;
use auction_config::{AppConfig, DataKeyDomain};
use auction_integrations::{EquipmentKeyOpener, HvDataEquipmentReader, build_keyring};
use auction_jobs::consignment::{
    ConsignmentDependencies, ConsignmentLookupRuntime, ConsignmentReadinessProbe, ConsignmentWorker,
};
use auction_storage::DatabaseStore;
use tokio::sync::Notify;

use crate::bootstrap::SystemClock;

/// 提交准入同步后返回可受监管的任务，关停后的存量只交给补扫。
pub(super) async fn prepare(
    config: &AppConfig,
    store: &Arc<DatabaseStore>,
    switches: &RuntimeSwitches,
    worker_id: &str,
) -> anyhow::Result<Option<ConsignmentWorker>> {
    // run_config 已验证这份启动配置的制品能力，装配过程只读取同一份配置。
    let enabled = config.consignment.enabled();
    let lookup = if enabled {
        store.check_database_readiness().await.context("一口价挂单的数据库 schema 未就绪")?;
        Some(lookup_runtime(config, store, switches.clone(), worker_id)?)
    } else {
        None
    };
    let maintenance = ListingMaintenanceService::new(store.clone(), store.clone());
    // 不变量 2、5、6：必须确认代次同步提交，存量探针只在停用后运行，错误保持失败关闭。
    ConsignmentWorker::prepare(
        enabled,
        true,
        maintenance,
        lookup,
        store.has_active_consignment_listings(),
    )
    .await
    .context("一口价挂单准入同步或存量检查失败")
}

fn lookup_runtime(
    config: &AppConfig,
    store: &Arc<DatabaseStore>,
    switches: RuntimeSwitches,
    worker_id: &str,
) -> anyhow::Result<ConsignmentLookupRuntime> {
    let key_domain = DataKeyDomain::Equipment;
    // 不变量 7：开封器同时持有当前钥和保留的旧钥，错误只保留安全分类。
    let keyring = build_keyring(
        key_domain,
        config.data_key_current(key_domain),
        config.data_key_previous(key_domain).iter().map(|key| (key.id(), key.material())),
    )
    .context("装配一口价挂单的装备数据密钥环失败")?
    .context("启用一口价挂单的 Worker 必须配置当前装备数据密钥")?;
    let opener = Arc::new(EquipmentKeyOpener::new(keyring));
    let settings = config.consignment;
    let lookup = Arc::new(HvDataEquipmentReader::new(settings.lookup_max_response_bytes())?);
    // 不变量 5：预算交由领取事务计算数据库硬截止，经解析用例传给固定来源。
    let budget = ListingLookupBudget::new(
        settings.lookup_timeout_seconds(),
        settings.lookup_min_interval_ms(),
    )?;
    let work: Arc<dyn LookupWorkStore> =
        Arc::new(store.consignment_lookup(auto_close_command, lookup_reap_exhausted_identity));
    let clock: Arc<dyn auction_application::core::Clock> = Arc::new(SystemClock);
    let service = ResolveListingService::new(
        work.clone(),
        store.clone(),
        opener,
        lookup,
        move || switches.external_access_paused(),
        clock.clone(),
        settings.publication_ttls(),
    );
    Ok(ConsignmentLookupRuntime {
        service,
        work,
        budget,
        clock,
        worker_id: worker_id.to_owned(),
        wakeup: Arc::new(Notify::new()),
        readiness: Arc::new(InstalledLookupReadiness(Arc::clone(store))),
    })
}

struct InstalledLookupReadiness(Arc<DatabaseStore>);

impl ConsignmentReadinessProbe for InstalledLookupReadiness {
    fn dependencies(&self) -> ConsignmentFuture<'_, ConsignmentDependencies> {
        Box::pin(async move {
            // 此探针与已成功构造的不可变 opener、装备域 keyring、来源端口一同创建。
            // 不变量 5、6：每次续期前重新取得数据库与 schema 的证明，禁止探测真实来源。
            let database_ready = match self.0.check_database_readiness().await {
                Ok(()) => true,
                Err(error) => {
                    tracing::debug!(
                        kind = error.diagnostic_kind().label(),
                        "一口价挂单心跳未取得数据库就绪证明"
                    );
                    false
                }
            };
            Ok(ConsignmentDependencies {
                database: database_ready,
                schema: database_ready,
                opener: true,
                current_keyring: true,
                lookup: true,
            })
        })
    }
}
