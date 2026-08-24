use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::{
    ApplicationError, ApplicationFuture, RecoveryBalanceStatus, RecoveryEvidenceStatus,
    RecoveryHealthConfig, RecoveryHealthRefreshPort, RecoveryHealthStore, RecoveryHealthUpdate,
    RepositoryError, SettlementFulfillmentEvidenceReader,
};

/// 读取外部 Balance 并刷新 recovery health 的应用服务。
pub struct RecoveryHealthService {
    config: RecoveryHealthConfig,
    store: Arc<dyn RecoveryHealthStore>,
    balances: Arc<dyn SettlementFulfillmentEvidenceReader>,
}

impl RecoveryHealthService {
    /// 建立 health 刷新服务；外部读取失败会落为 unavailable 投影。
    pub fn new(
        config: RecoveryHealthConfig,
        store: Arc<dyn RecoveryHealthStore>,
        balances: Arc<dyn SettlementFulfillmentEvidenceReader>,
    ) -> Result<Self, &'static str> {
        config.validate()?;
        Ok(Self { config, store, balances })
    }
}

impl RecoveryHealthRefreshPort for RecoveryHealthService {
    fn refresh(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(ApplicationError::Repository(RepositoryError::Failed(
                    "recovery health 刷新时间无效".to_owned(),
                )));
            }

            for world in self.config.worlds.iter().copied() {
                let mut update = RecoveryHealthUpdate {
                    world,
                    balance_credits: None,
                    balance_observed_at: None,
                    balance_status: RecoveryBalanceStatus::Unavailable,
                    evidence_last_success_at: None,
                    evidence_status: RecoveryEvidenceStatus::Unavailable,
                    updated_at: now,
                };

                if let Ok(snapshot) = self.balances.read_balance(world).await {
                    let valid_snapshot = snapshot.world == world
                        && snapshot.credits >= 0
                        && snapshot.observed_at.timestamp_nanos_opt().is_some()
                        && snapshot.observed_at <= now;
                    if valid_snapshot {
                        let observed_at = snapshot.observed_at;
                        update.balance_credits = Some(snapshot.credits);
                        update.balance_observed_at = Some(observed_at);
                        update.evidence_last_success_at = Some(observed_at);
                        if now
                            .signed_duration_since(observed_at)
                            .to_std()
                            .is_ok_and(|age| age > self.config.evidence_stale_after)
                        {
                            update.balance_status = RecoveryBalanceStatus::Stale;
                            update.evidence_status = RecoveryEvidenceStatus::Stale;
                        } else {
                            update.balance_status =
                                if snapshot.credits < self.config.low_balance_threshold {
                                    RecoveryBalanceStatus::Low
                                } else {
                                    RecoveryBalanceStatus::Ok
                                };
                            update.evidence_status = RecoveryEvidenceStatus::Fresh;
                        }
                    }
                }

                update.validate().map_err(|message| {
                    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
                })?;
                self.store.refresh_world_health(update).await?;
            }
            Ok(())
        })
    }
}
