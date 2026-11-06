//! W11.1–W11.3 资产对账服务：只观察、只追加报告事实，不授权任何资金或资产动作。

use std::sync::Arc;

use chrono::{DateTime, Utc};

use auction_domain::SettlementWorld;

use crate::{
    ApplicationError, ApplicationFuture, AssetArmoryObservation, AssetInventoryReader,
    AssetReconciliationConfig, AssetReconciliationOutcome, AssetReconciliationPort,
    AssetReconciliationStore, RepositoryError,
};

/// 按 world 依次读取 Armory 观察，再由存储 owner 计算差异并追加报告。
pub struct AssetReconciliationService {
    config: AssetReconciliationConfig,
    store: Arc<dyn AssetReconciliationStore>,
    inventory: Arc<dyn AssetInventoryReader>,
}

impl AssetReconciliationService {
    /// 建立对账服务；world 闭集与顺序由启动配置固定，避免同一轮重复观察。
    pub fn new(
        config: AssetReconciliationConfig,
        store: Arc<dyn AssetReconciliationStore>,
        inventory: Arc<dyn AssetInventoryReader>,
    ) -> Result<Self, &'static str> {
        config.validate()?;
        Ok(Self { config, store, inventory })
    }
}

impl AssetReconciliationPort for AssetReconciliationService {
    fn reconcile(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, AssetReconciliationOutcome> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(ApplicationError::Repository(RepositoryError::Failed(
                    "资产对账观察时间无效".to_owned(),
                )));
            }
            let mut observations = Vec::with_capacity(self.config.worlds.len());
            for world in self.config.worlds.iter().copied() {
                let observation = match self.inventory.read_inventory(world, now).await {
                    Ok(mut observation) if observation.world == world => {
                        observation.equipment_ids.sort_unstable();
                        observation.equipment_ids.dedup();
                        if observation.validate() {
                            observation
                        } else {
                            unavailable(world, now, "inventory_observation_invalid")
                        }
                    }
                    Ok(_) => unavailable(world, now, "inventory_world_mismatch"),
                    Err(ApplicationError::FulfillmentAccountBusy) => {
                        unavailable(world, now, "account_busy")
                    }
                    Err(error) => {
                        let _ = error;
                        unavailable(world, now, "inventory_unavailable")
                    }
                };
                observations.push(observation);
            }
            let custody = self.store.reconcile_custody(observations).await?;
            let solvency = self.store.report_solvency().await?;
            Ok(AssetReconciliationOutcome { custody, solvency })
        })
    }
}

fn unavailable(
    world: SettlementWorld,
    observed_at: DateTime<Utc>,
    failure_code: &'static str,
) -> AssetArmoryObservation {
    AssetArmoryObservation {
        world,
        observed_at,
        equipment_ids: Vec::new(),
        complete: false,
        failure_code: Some(failure_code),
    }
}
