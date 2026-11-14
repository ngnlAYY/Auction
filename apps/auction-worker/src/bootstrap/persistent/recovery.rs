//! 恢复健康与资产对账运行时的装配：两者观察同一组结算世界，缺少读取端口时各按自身语义处理。

use std::sync::Arc;
use std::time::Duration;

use auction_application::reconciliation::{
    AssetInventoryReader, AssetReconciliationConfig, AssetReconciliationService,
};
use auction_application::recovery::{RecoveryHealthConfig, RecoveryHealthService};
use auction_application::settlement::SettlementFulfillmentEvidenceReader;
use auction_config::AppConfig;
use auction_domain::SettlementWorld;
use auction_jobs::{AssetReconciliationRuntime, RecoveryHealthRuntime};
use auction_storage::DatabaseStore;

/// 启用 MoogleMail 时只观察已配置邮箱的世界；未启用时两个结算世界都参与健康投影。
pub(super) fn recovery_worlds(config: &AppConfig) -> Vec<SettlementWorld> {
    if config.worker.mooglemail.enabled {
        config
            .worker
            .mooglemail
            .configured_mailboxes()
            .filter_map(|(world_id, _)| SettlementWorld::from_name(world_id))
            .collect()
    } else {
        vec![SettlementWorld::Main, SettlementWorld::Isekai]
    }
}

/// recovery health 必须有 Balance 读取端口；缺失说明装配错误，启动失败关闭。
pub(super) fn recovery_health_runtime(
    config: &AppConfig,
    store: &Arc<DatabaseStore>,
    balance_reader: Option<Arc<dyn SettlementFulfillmentEvidenceReader>>,
    worlds: &[SettlementWorld],
) -> anyhow::Result<RecoveryHealthRuntime> {
    let Some(balance_reader) = balance_reader else {
        anyhow::bail!("PostgreSQL recovery health 缺少 Balance 读取端口")
    };
    let recovery = &config.worker.recovery;
    let health_service = RecoveryHealthService::new(
        RecoveryHealthConfig {
            worlds: worlds.to_vec(),
            low_balance_threshold: recovery.low_balance_threshold,
            evidence_stale_after: Duration::from_secs(recovery.evidence_stale_after_seconds),
        },
        store.clone(),
        balance_reader,
    )
    .map_err(anyhow::Error::msg)?
    .with_funding_policy_reader(store.clone());
    RecoveryHealthRuntime::new(
        Arc::new(health_service),
        Duration::from_secs(recovery.health_refresh_interval_seconds),
    )
    .map_err(anyhow::Error::msg)
}

/// 只读 Armory 观察 + 数据库负债投影；未配置邮箱（无库存读取端口）时不产生报告。
pub(super) fn asset_reconciliation_runtime(
    config: &AppConfig,
    store: &Arc<DatabaseStore>,
    inventory_reader: Option<Arc<dyn AssetInventoryReader>>,
    worlds: &[SettlementWorld],
) -> anyhow::Result<Option<AssetReconciliationRuntime>> {
    let Some(inventory_reader) = inventory_reader else {
        return Ok(None);
    };
    let mut worlds = worlds.to_vec();
    worlds.sort_by_key(|world| world.as_str());
    worlds.dedup();
    let service = AssetReconciliationService::new(
        AssetReconciliationConfig {
            worlds,
            balance_evidence_max_age_seconds: config.worker.recovery.evidence_stale_after_seconds,
        },
        store.clone(),
        inventory_reader,
    )
    .map_err(anyhow::Error::msg)?;
    AssetReconciliationRuntime::new(
        Arc::new(service),
        auction_jobs::ASSET_RECONCILIATION_INTERVAL,
        auction_jobs::ASSET_RECONCILIATION_TIMEOUT,
    )
    .map(Some)
    .map_err(anyhow::Error::msg)
}
